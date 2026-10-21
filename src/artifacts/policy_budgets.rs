//! Opt-in release measurements for complete file and static-bundle capture.
//! Run individual ignored tests with `--release --ignored --exact`.

use super::bundle::{self, BundleSelection};
use super::files;
use super::policy::{CapturePolicy, Coverage, Inspection, Sensitivity};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MIB: usize = 1024 * 1024;
const SAMPLES: usize = 100;
const AUXILIARY_LIMIT: usize = 2 * MIB;
const STACK_ALLOWANCE: usize = 128 * 1024;

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let root = super::standalone_root(&format!("policy-budget-{label}"));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This path is a uniquely named child of the system temporary directory.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source_hash(path: &Path) -> (String, u64) {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
    }
    (format!("{:x}", digest.finalize()), size)
}

fn checked_policy(policy: CapturePolicy) -> &'static str {
    match policy {
        CapturePolicy::Strict => "strict",
        CapturePolicy::AllowInsecure => "allow_insecure",
    }
}

fn report(label: &str, policy: CapturePolicy, target: usize, mut samples: Vec<f64>, peak: usize) {
    samples.sort_by(f64::total_cmp);
    let p50 = samples[49];
    let p99 = samples[98];
    let maximum = samples[99];
    let peak_with_stack = peak + STACK_ALLOWANCE;
    eprintln!(
        "{}",
        serde_json::json!({
            "benchmark": "artifact_capture",
            "case": label,
            "policy": checked_policy(policy),
            "target_bytes": target,
            "samples": SAMPLES,
            "p50_ms": p50,
            "p99_ms": p99,
            "max_ms": maximum,
            "peak_rust_heap_bytes": peak,
            "stack_allowance_bytes": STACK_ALLOWANCE,
            "peak_heap_plus_stack_allowance_bytes": peak_with_stack,
            "auxiliary_budget_bytes": AUXILIARY_LIMIT
        })
    );
    let time_limit = if target <= MIB {
        500.0
    } else if target <= 10 * MIB {
        2_000.0
    } else {
        20_000.0
    };
    assert!(
        p99 < time_limit,
        "{label} p99 was {p99} ms (limit {time_limit} ms)"
    );
    assert!(
        peak_with_stack <= AUXILIARY_LIMIT,
        "{label} heap plus stack allowance was {peak_with_stack} bytes"
    );
}

pub(super) fn require_release_mode() {
    #[cfg(debug_assertions)]
    panic!("Run this benchmark in release mode.");
}

fn measure_file(policy: CapturePolicy, target: usize) {
    require_release_mode();
    let fixture = Fixture::new(&format!("file-{}-{target}", checked_policy(policy)));
    let source = fixture.path().join("delivery.html");
    let mut file = File::create(&source).unwrap();
    let prefix = b"<!doctype html><html><head><title>Measured delivery</title></head><body><main>";
    let suffix = b"</main></body></html>";
    let marker = if policy == CapturePolicy::AllowInsecure {
        b"<p>person@example.com</p>\n".as_slice()
    } else {
        b"".as_slice()
    };
    let unit = b"<p>Retained delivery preview with local controls.</p>\n";
    assert!(prefix.len() + suffix.len() + marker.len() <= target);
    file.write_all(prefix).unwrap();
    write_repeated_unit(
        &mut file,
        unit,
        target - prefix.len() - suffix.len() - marker.len(),
    );
    file.write_all(marker).unwrap();
    file.write_all(suffix).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let (expected_digest, expected_size) = source_hash(&source);
    assert_eq!(expected_size, target as u64);

    let mut samples = Vec::with_capacity(SAMPLES);
    let mut peak = 0usize;
    for _ in 0..SAMPLES {
        let revision = crate::id::ulid();
        let started = Instant::now();
        let (result, allocated) = crate::test_alloc::measure(|| {
            files::capture_with_policy(fixture.path(), "delivery.html", &revision, policy)
        });
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        peak = peak.max(allocated);
        let (retained, inspection) = result.unwrap();
        assert_eq!(retained.size, target as u64);
        assert_eq!(retained.digest, expected_digest);
        if policy == CapturePolicy::AllowInsecure {
            assert_permissive_inspection(&inspection);
        } else {
            assert_strict_inspection(&inspection);
        }
        fs::remove_file(
            fixture
                .path()
                .join(".vivac-artifacts")
                .join(&retained.object),
        )
        .unwrap();
    }
    assert_eq!(source_hash(&source), (expected_digest, expected_size));
    report("file", policy, target, samples, peak);
}

fn bundle_selection() -> BundleSelection {
    BundleSelection {
        root: ".".into(),
        entrypoint: "index.html".into(),
        members: vec![
            "assets/a.css".into(),
            "assets/b.css".into(),
            "index.html".into(),
        ],
        requirements: vec![],
        database: None,
        database_source: None,
        runtime: None,
        media_review: None,
    }
}

fn write_repeated_unit(file: &mut File, unit: &[u8], bytes: usize) {
    const BUFFER_BYTES: usize = 64 * 1024;
    assert!(!unit.is_empty() && unit.len() <= BUFFER_BYTES);
    let complete_units = bytes / unit.len();
    let remainder = bytes % unit.len();
    let units_per_buffer = (BUFFER_BYTES / unit.len()).max(1);
    let block = unit.repeat(units_per_buffer);
    for _ in 0..complete_units / units_per_buffer {
        file.write_all(&block).unwrap();
    }
    for _ in 0..complete_units % units_per_buffer {
        file.write_all(unit).unwrap();
    }
    let spaces = [b' '; BUFFER_BYTES];
    file.write_all(&spaces[..remainder]).unwrap();
}

fn assert_permissive_inspection(inspection: &Inspection) {
    assert_eq!(inspection.policy, CapturePolicy::AllowInsecure);
    assert_eq!(inspection.sensitivity, Sensitivity::Detected);
    assert_eq!(inspection.coverage, Coverage::Complete);
    assert!(inspection.review_required);
}

fn assert_strict_inspection(inspection: &Inspection) {
    assert_eq!(inspection.policy, CapturePolicy::Strict);
    assert_eq!(inspection.sensitivity, Sensitivity::NotDetected);
    assert_eq!(inspection.coverage, Coverage::Complete);
    assert!(!inspection.review_required);
}

fn write_html_member(path: &Path, size: usize, links: bool, policy: CapturePolicy) {
    let prefix = if links {
        b"<!doctype html><html><head><link rel=stylesheet href=assets/a.css><link rel=stylesheet href=assets/b.css></head><body><main>".as_slice()
    } else {
        b"<!doctype html><html><head><title>Measured delivery</title></head><body><main>".as_slice()
    };
    let suffix = b"</main></body></html>";
    let marker = if policy == CapturePolicy::AllowInsecure {
        b"<p>person@example.com</p>\n".as_slice()
    } else {
        b"".as_slice()
    };
    let unit = b"<p>Retained delivery preview with local controls.</p>\n";
    assert!(prefix.len() + suffix.len() + marker.len() <= size);
    let mut file = File::create(path).unwrap();
    file.write_all(prefix).unwrap();
    write_repeated_unit(
        &mut file,
        unit,
        size - prefix.len() - suffix.len() - marker.len(),
    );
    file.write_all(marker).unwrap();
    file.write_all(suffix).unwrap();
    file.sync_all().unwrap();
    assert_eq!(fs::metadata(path).unwrap().len(), size as u64);
}

fn write_css_member(path: &Path, size: usize) {
    let prefix = b"/* measured stylesheet */\n";
    let suffix = b"\n";
    let unit = b".measure { color: #123456; background: #f5f7fa; }\n";
    assert!(prefix.len() + suffix.len() <= size);
    let mut file = File::create(path).unwrap();
    file.write_all(prefix).unwrap();
    write_repeated_unit(&mut file, unit, size - prefix.len() - suffix.len());
    file.write_all(suffix).unwrap();
    file.sync_all().unwrap();
    assert_eq!(fs::metadata(path).unwrap().len(), size as u64);
}

fn write_bundle_sources(root: &Path, payload_bytes: usize, policy: CapturePolicy) {
    fs::create_dir_all(root.join("assets")).unwrap();
    let member_bytes = payload_bytes / 3;
    let extra = payload_bytes % 3;
    write_html_member(
        &root.join("index.html"),
        member_bytes + usize::from(extra > 0),
        true,
        policy,
    );
    write_css_member(
        &root.join("assets/a.css"),
        member_bytes + usize::from(extra > 1),
    );
    write_css_member(&root.join("assets/b.css"), member_bytes);
}

fn remove_bundle(root: &Path, object: &str) {
    fs::remove_dir_all(root.join(".vivac-artifacts").join(object)).unwrap();
}

fn calibrate_bundle(fixture: &Fixture, target: usize, policy: CapturePolicy) -> BundleSelection {
    let selection = bundle_selection();
    let mut payload = target - 2 * 1024;
    for _ in 0..6 {
        write_bundle_sources(fixture.path(), payload, policy);
        let revision = crate::id::ulid();
        let result = bundle::capture_with_policy(fixture.path(), &selection, &revision, policy);
        match result {
            Ok(retained) if retained.retained.size == target as u64 => {
                if policy == CapturePolicy::AllowInsecure {
                    assert_permissive_inspection(
                        retained.inspection.as_ref().expect("permissive report"),
                    );
                    let persisted = bundle::read_manifest(
                        fixture.path(),
                        &retained.retained.object,
                        &retained.retained.digest,
                        retained.retained.size,
                    )
                    .unwrap();
                    assert_permissive_inspection(persisted.inspection.as_ref().unwrap());
                } else {
                    assert!(retained.inspection.is_none());
                }
                remove_bundle(fixture.path(), &retained.retained.object);
                return selection;
            }
            Ok(retained) => {
                if policy == CapturePolicy::AllowInsecure {
                    assert_permissive_inspection(
                        retained.inspection.as_ref().expect("permissive report"),
                    );
                    let persisted = bundle::read_manifest(
                        fixture.path(),
                        &retained.retained.object,
                        &retained.retained.digest,
                        retained.retained.size,
                    )
                    .unwrap();
                    assert_permissive_inspection(persisted.inspection.as_ref().unwrap());
                }
                let actual = retained.retained.size as usize;
                remove_bundle(fixture.path(), &retained.retained.object);
                if actual > target {
                    payload -= actual - target;
                } else {
                    payload += target - actual;
                }
            }
            Err(error) => panic!("Bundle calibration failed: {error:?}"),
        }
    }
    panic!("Could not calibrate the complete bundle to {target} bytes.");
}

fn bundle_source_hashes(root: &Path, selection: &BundleSelection) -> Vec<(String, u64)> {
    selection
        .members
        .iter()
        .map(|member| source_hash(&root.join(member)))
        .collect()
}

fn measure_bundle(policy: CapturePolicy, target: usize) {
    require_release_mode();
    let fixture = Fixture::new(&format!("bundle-{}-{target}", checked_policy(policy)));
    let selection = calibrate_bundle(&fixture, target, policy);
    let expected_sources = bundle_source_hashes(fixture.path(), &selection);
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut peak = 0usize;
    for _ in 0..SAMPLES {
        let revision = crate::id::ulid();
        let started = Instant::now();
        let (result, allocated) = crate::test_alloc::measure(|| {
            bundle::capture_with_policy(fixture.path(), &selection, &revision, policy)
        });
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        peak = peak.max(allocated);
        let retained = result.unwrap();
        assert_eq!(retained.retained.size, target as u64);
        if policy == CapturePolicy::AllowInsecure {
            assert_permissive_inspection(retained.inspection.as_ref().expect("permissive report"));
        } else {
            assert!(retained.inspection.is_none());
        }
        remove_bundle(fixture.path(), &retained.retained.object);
    }
    assert_eq!(
        bundle_source_hashes(fixture.path(), &selection),
        expected_sources
    );
    report("static_bundle_html_2css", policy, target, samples, peak);
}

macro_rules! ignored_case {
    ($name:ident, $measure:ident, $policy:ident, $target:expr) => {
        #[test]
        #[ignore = "Run explicitly in release mode to measure 100 complete captures."]
        fn $name() {
            $measure(CapturePolicy::$policy, $target);
        }
    };
}

ignored_case!(file_strict_one_mib, measure_file, Strict, MIB);
ignored_case!(file_strict_ten_mib, measure_file, Strict, 10 * MIB);
ignored_case!(file_strict_hundred_mib, measure_file, Strict, 100 * MIB);
ignored_case!(
    file_allow_insecure_one_mib,
    measure_file,
    AllowInsecure,
    MIB
);
ignored_case!(
    file_allow_insecure_ten_mib,
    measure_file,
    AllowInsecure,
    10 * MIB
);
ignored_case!(
    file_allow_insecure_hundred_mib,
    measure_file,
    AllowInsecure,
    100 * MIB
);
ignored_case!(bundle_strict_one_mib, measure_bundle, Strict, MIB);
ignored_case!(bundle_strict_ten_mib, measure_bundle, Strict, 10 * MIB);
ignored_case!(bundle_strict_hundred_mib, measure_bundle, Strict, 100 * MIB);
ignored_case!(
    bundle_allow_insecure_one_mib,
    measure_bundle,
    AllowInsecure,
    MIB
);
ignored_case!(
    bundle_allow_insecure_ten_mib,
    measure_bundle,
    AllowInsecure,
    10 * MIB
);
ignored_case!(
    bundle_allow_insecure_hundred_mib,
    measure_bundle,
    AllowInsecure,
    100 * MIB
);
