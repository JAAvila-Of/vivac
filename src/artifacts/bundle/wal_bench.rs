use super::*;
use crate::artifacts::sqlite::vfs::wal_tests::Writer;
use std::fs;
use std::path::PathBuf;

const SAMPLES: usize = 100;
const STACK_ALLOWANCE: usize = 128 * 1024;
const HTML_PREFIX: &[u8] = b"<!doctype html><script src=main.js></script>";

struct Fixture {
    writer: Writer,
    root: PathBuf,
    selection: BundleSelection,
    input_bytes: u64,
    retained_bytes: u64,
    database_bytes: u64,
    source_bytes: u64,
    wal_bytes: u64,
    source_sha256: String,
    wal_sha256: String,
    shm_sha256: String,
    rows: usize,
}

fn fingerprint(path: &Path) -> (u64, String) {
    let mut file = fs::File::open(path).unwrap();
    let mut buffer = [0u8; 16384];
    let mut digest = Sha256::new();
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
    }
    (size, format!("{:x}", digest.finalize()))
}

fn discard(root: &Path, object: &str) {
    let project = files::open_root(root).unwrap();
    files::store(&project, false)
        .unwrap()
        .remove_dir_all(object)
        .unwrap();
}

fn frontend(root: &Path, html_bytes: usize) {
    assert!(html_bytes >= HTML_PREFIX.len());
    let mut html = HTML_PREFIX.to_vec();
    let pattern = b"<!-- Selected synthetic SQLite WAL application -->\n";
    html.extend(
        pattern
            .iter()
            .copied()
            .cycle()
            .take(html_bytes - html.len()),
    );
    fs::write(root.join("index.html"), html).unwrap();
}

impl Fixture {
    fn new(target: Option<u64>, wal_heavy: bool) -> Self {
        let rows = target.map_or(1, |bytes| {
            ((bytes - runtime::BYTES) * 9 / 10 / 280) as usize
        });
        assert!((1..=100_000).contains(&rows));
        let value = "Synthetic SQLite application fixture data. ".repeat(6);
        let pending = "Pending WAL application fixture data. ".repeat(6);
        let before_bulk = if wal_heavy {
            "PRAGMA wal_checkpoint(TRUNCATE);"
        } else {
            ""
        };
        let after_bulk = if wal_heavy {
            ""
        } else {
            "PRAGMA wal_checkpoint(TRUNCATE);"
        };
        let sql = format!(
            "PRAGMA cache_size=-64; PRAGMA temp_store=MEMORY; PRAGMA synchronous=OFF;
             PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT NOT NULL);
             {before_bulk}
             BEGIN; WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<{rows})
             INSERT INTO items SELECT i,'{value}' FROM n; COMMIT;
             {after_bulk}
             UPDATE items SET value='{pending}' WHERE id=1;"
        );
        let writer = Writer::start_with_sql(&sql);
        let root = writer.path("").parent().unwrap().to_owned();
        frontend(&root, HTML_PREFIX.len());
        fs::write(
            root.join("main.js"),
            b"document.title = 'Synthetic SQLite WAL application';",
        )
        .unwrap();
        let selection = BundleSelection {
            root: ".".into(),
            entrypoint: "index.html".into(),
            members: vec!["source.db".into(), "index.html".into(), "main.js".into()],
            requirements: vec![],
            database: Some("source.db".into()),
            database_source: Some(DatabaseSource {
                profile: WAL_PROFILE.into(),
                wal: "source.db-wal".into(),
                shm: "source.db-shm".into(),
            }),
            runtime: Some(runtime::ID.into()),
            media_review: None,
        };
        let (source_bytes, source_sha256) = fingerprint(&writer.path(""));
        let (wal_bytes, wal_sha256) = fingerprint(&writer.path("-wal"));
        let (_, shm_sha256) = fingerprint(&writer.path("-shm"));
        assert!(
            wal_bytes > 32,
            "The fixture must include a real pending WAL commit."
        );
        if wal_heavy {
            assert!(
                wal_bytes >= 9 * source_bytes,
                "The WAL-dominant fixture must keep at least 90% of database input bytes in WAL."
            );
        }
        let javascript_bytes = fs::metadata(root.join("main.js")).unwrap().len();
        let fixed_input = source_bytes + wal_bytes + javascript_bytes + runtime::BYTES;
        assert!(fixed_input + HTML_PREFIX.len() as u64 <= MAX_BYTES);
        let warmup = capture(&root, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let mut manifest =
            read_manifest(&root, &warmup.object, &warmup.digest, warmup.size).unwrap();
        let database = manifest.database.as_ref().unwrap();
        assert_eq!(database.source_bytes, source_bytes);
        assert_eq!(database.source_sha256, source_sha256);
        let retained_source = database.source.as_ref().unwrap();
        assert_eq!(retained_source.profile, WAL_PROFILE);
        assert_eq!(retained_source.wal.bytes, wal_bytes);
        assert_eq!(retained_source.wal.sha256, wal_sha256);
        let database_bytes = manifest
            .members
            .iter()
            .find(|member| member.path == "source.db")
            .unwrap()
            .bytes;
        let fixed_retained = manifest
            .members
            .iter()
            .filter(|member| member.path != "index.html")
            .map(|member| member.bytes)
            .sum::<u64>();
        let requested = target.unwrap_or(warmup.size);
        let mut html_bytes = HTML_PREFIX.len() as u64;
        // Manifest size includes the decimal member size, so resolve its small fixed point.
        for _ in 0..8 {
            let manifest_bytes = manifest_bytes(&manifest).unwrap().len() as u64;
            html_bytes = requested
                .checked_sub(fixed_retained + manifest_bytes)
                .unwrap()
                .min(MAX_BYTES - fixed_input);
            manifest
                .members
                .iter_mut()
                .find(|member| member.path == "index.html")
                .unwrap()
                .bytes = html_bytes;
        }
        frontend(&root, html_bytes as usize);
        discard(&root, &warmup.object);
        let ready = capture(&root, &selection, &crate::id::ulid())
            .unwrap()
            .retained;
        let input_bytes = fixed_input + html_bytes;
        assert!(input_bytes <= MAX_BYTES && ready.size <= MAX_BYTES);
        if requested < MAX_BYTES {
            assert_eq!(ready.size, requested);
        } else {
            assert!(ready.size.max(input_bytes) >= MAX_BYTES - 64);
            assert!(
                ready.size >= MAX_BYTES - 64 * 1024,
                "The maximum fixture must remain close to 10 MiB."
            );
        }
        if target.is_some() {
            let available = requested - runtime::BYTES;
            assert!(
                database_bytes >= available * 8 / 10 && database_bytes <= available,
                "The representative database must occupy about 90% of available space."
            );
        }
        let retained_bytes = ready.size;
        discard(&root, &ready.object);
        let fixture = Self {
            writer,
            root,
            selection,
            input_bytes,
            retained_bytes,
            database_bytes,
            source_bytes,
            wal_bytes,
            source_sha256,
            wal_sha256,
            shm_sha256,
            rows,
        };
        fixture.assert_inputs_unchanged();
        fixture
    }

    fn assert_inputs_unchanged(&self) {
        assert_eq!(
            fingerprint(&self.writer.path("")),
            (self.source_bytes, self.source_sha256.clone())
        );
        assert_eq!(
            fingerprint(&self.writer.path("-wal")),
            (self.wal_bytes, self.wal_sha256.clone())
        );
        assert_eq!(fingerprint(&self.writer.path("-shm")).1, self.shm_sha256);
    }
}

#[test]
#[ignore = "Run explicitly in release mode with --test-threads=1 to measure WAL application retention."]
fn retained_wal_sqlite_application_release_benchmark() {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "Run this benchmark in release mode."
    );
    let mut failures = Vec::new();
    for (label, target, budget, wal_heavy) in [
        ("minimum_complete", None, 500.0, false),
        (
            "runtime_plus_one_mib_db_dominant",
            Some(runtime::BYTES + 1024 * 1024),
            500.0,
            false,
        ),
        ("near_ten_mib_db_dominant", Some(MAX_BYTES), 2000.0, false),
        (
            "runtime_plus_one_mib_wal_dominant",
            Some(runtime::BYTES + 1024 * 1024),
            500.0,
            true,
        ),
        ("near_ten_mib_wal_dominant", Some(MAX_BYTES), 2000.0, true),
    ] {
        let fixture = Fixture::new(target, wal_heavy);
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut maximum_rust = 0;
        let mut maximum_sqlite = 0;
        for _ in 0..SAMPLES {
            {
                let _engine = super::super::sqlite::test_engine_guard().unwrap();
                unsafe {
                    libsqlite3_sys::sqlite3_memory_highwater(1);
                }
            }
            let revision = crate::id::ulid();
            let started = std::time::Instant::now();
            let (result, rust_heap) = crate::test_alloc::measure(|| {
                capture(&fixture.root, &fixture.selection, &revision)
            });
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            maximum_rust = maximum_rust.max(rust_heap);
            {
                let _engine = super::super::sqlite::test_engine_guard().unwrap();
                let sqlite_heap = unsafe { libsqlite3_sys::sqlite3_memory_highwater(0) };
                maximum_sqlite = maximum_sqlite.max(usize::try_from(sqlite_heap).unwrap());
            }
            let retained = result.unwrap().retained;
            assert_eq!(retained.size, fixture.retained_bytes);
            // Cleanup and invariance checks are outside the measured capture.
            discard(&fixture.root, &retained.object);
        }
        fixture.assert_inputs_unchanged();
        samples.sort_by(f64::total_cmp);
        let auxiliary = maximum_rust + maximum_sqlite + STACK_ALLOWANCE + runtime::PACKED_LEN;
        eprintln!(
            "{}",
            json!({
                "benchmark":"wal_sqlite_application_retention", "case":label, "samples":SAMPLES,
                "scope":"capture_only_no_receipt_append", "profile":"release", "os":std::env::consts::OS,
                "arch":std::env::consts::ARCH, "filesystem_root":fixture.root,
                "input_bytes":fixture.input_bytes, "retained_bytes":fixture.retained_bytes,
                "input_limit_gap_bytes":MAX_BYTES-fixture.input_bytes,
                "retained_limit_gap_bytes":MAX_BYTES-fixture.retained_bytes,
                "database_source_bytes":fixture.source_bytes, "wal_source_bytes":fixture.wal_bytes,
            "wal_dominant":wal_heavy, "wal_fraction_of_database_input":fixture.wal_bytes as f64 / (fixture.source_bytes + fixture.wal_bytes) as f64,
                "database_snapshot_bytes":fixture.database_bytes, "database_rows":fixture.rows,
                "database_source_sha256":fixture.source_sha256, "wal_source_sha256":fixture.wal_sha256,
                "shm_source_sha256":fixture.shm_sha256, "source_inputs_unchanged":true,
                "runtime_bytes":runtime::BYTES, "other_retained_bytes":fixture.retained_bytes-fixture.database_bytes-runtime::BYTES,
                "source_profile":WAL_PROFILE, "transformation":SQLITE_TRANSFORMATION, "members":4,
                "guards":"complete_product_runtime_catalog_logical_sqlite_utf8_sensitive_metadata_wal_shared_leases_source_identity_git_native_exclusive",
                "p50_ms":samples[49], "p99_ms":samples[98], "max_ms":samples[99],
                "peak_rust_auxiliary_heap_bytes":maximum_rust, "peak_sqlite_heap_bytes":maximum_sqlite,
                "combined_conservative_heap_bytes":maximum_rust+maximum_sqlite,
                "stack_allowance_bytes":STACK_ALLOWANCE, "compiled_runtime_bytes":runtime::PACKED_LEN,
                "total_conservative_auxiliary_bytes":auxiliary
            })
        );
        if samples[98] >= budget {
            failures.push(format!(
                "{label} p99 {} ms exceeds {budget} ms",
                samples[98]
            ));
        }
        if auxiliary > 2 * 1024 * 1024 {
            failures.push(format!(
                "{label} combined auxiliary memory {auxiliary} bytes exceeds 2 MiB"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}
