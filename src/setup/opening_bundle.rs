use super::PlannedWrite;
use crate::agents::adapters::digest;
use crate::failure::Failure;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

const BUNDLE: &str = ".claude/skills/vivac-opening";
const FILES: [&str; 3] = [
    ".claude-plugin/plugin.json",
    "hooks/hooks.json",
    "hooks/register.mjs",
];
const RECEIPT: &str = ".vivac-owned.json";

#[derive(Clone, Copy)]
pub(super) struct BundleVersion {
    pub(super) version: &'static str,
    pub(super) files: [&'static str; 3],
}

pub(super) static CURRENT: BundleVersion = BundleVersion {
    version: "opening-reference-v3",
    files: [
        include_str!("claude-opening/.claude-plugin/plugin.json"),
        include_str!("claude-opening/hooks/hooks.json"),
        include_str!("claude-opening/hooks/register.mjs"),
    ],
};
static PREVIOUS: BundleVersion = BundleVersion {
    version: "opening-reference-v1",
    files: [
        include_str!("claude-opening-versions/opening-reference-v1/.claude-plugin/plugin.json"),
        include_str!("claude-opening-versions/opening-reference-v1/hooks/hooks.json"),
        include_str!("claude-opening-versions/opening-reference-v1/hooks/register.mjs"),
    ],
};
static PREVIOUS_V2: BundleVersion = BundleVersion {
    version: "opening-reference-v2",
    files: [
        include_str!("claude-opening-versions/opening-reference-v2/.claude-plugin/plugin.json"),
        include_str!("claude-opening-versions/opening-reference-v2/hooks/hooks.json"),
        include_str!("claude-opening-versions/opening-reference-v2/hooks/register.mjs"),
    ],
};
pub(super) static CATALOG: [BundleVersion; 3] = [PREVIOUS, PREVIOUS_V2, CURRENT];

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Created,
    Updated,
    Current,
    Removed,
    Kept,
    Absent,
}

pub(super) struct Plan {
    pub(super) outcome: Outcome,
    pub(super) writes: Vec<PlannedWrite>,
    cleanup: Option<PathBuf>,
    cleanup_on_failure: Option<PathBuf>,
}

#[derive(Debug)]
pub(super) struct ApplyResult {
    pub(super) cleanup_failures: Vec<PathBuf>,
}

const FILE_LIMIT: u64 = 1024 * 1024;
const RECEIPT_LIMIT: u64 = 16 * 1024;

// Only definitions compiled into the caller belong in this catalog. Receipt
// contents never supply a definition, even when their hashes match disk.
pub(super) fn canonical_receipt(version: &BundleVersion) -> String {
    #[derive(serde::Serialize)]
    struct Receipt<'a> {
        files: std::collections::BTreeMap<&'static str, String>,
        schema: u8,
        version: &'a str,
    }
    let receipt = Receipt {
        files: FILES
            .into_iter()
            .zip(version.files.map(|file| digest(file.as_bytes())))
            .collect(),
        schema: 1,
        version: version.version,
    };
    format!(
        "{}\n",
        serde_json::to_string(&receipt).expect("compiled receipt is serializable")
    )
}

pub(super) enum Inspection {
    Absent,
    Intact {
        version: &'static BundleVersion,
        originals: [Vec<u8>; 4],
    },
    Edited {
        version: &'static BundleVersion,
        originals: [Vec<u8>; 4],
    },
}

fn conflict(path: &Path, reason: &str) -> Failure {
    Failure::Model(format!(
        "Opening bundle conflict at {}: {reason}.",
        path.display()
    ))
}

fn linked(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn metadata(path: &Path, directory: bool, limit: u64) -> Result<bool, Failure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Failure::Io(error)),
    };
    if linked(&metadata) {
        return Err(conflict(path, "links and reparse points are not supported"));
    }
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(conflict(path, "unexpected file type"));
    }
    if !directory && metadata.len() > limit {
        return Err(conflict(path, "file exceeds its size limit"));
    }
    Ok(true)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Failure> {
    let file = File::open(path).map_err(Failure::Io)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(Failure::Io)?;
    if bytes.len() as u64 > limit {
        return Err(conflict(path, "file exceeds its size limit"));
    }
    if std::str::from_utf8(&bytes).is_err() {
        return Err(conflict(path, "file is not UTF-8"));
    }
    Ok(bytes)
}

/// Checks only the fixed paths; unrelated files are neither read nor owned.
/// These checks do not protect against a concurrent filesystem replacement.
pub(super) fn inspect(
    root: &Path,
    catalog: &'static [BundleVersion],
) -> Result<Inspection, Failure> {
    if !metadata(root, true, 0)? {
        return Err(conflict(root, "project root does not exist"));
    }
    let bundle = root.join(BUNDLE);
    let mut bundle_present = false;
    for relative in [".claude", ".claude/skills", BUNDLE] {
        let present = metadata(&root.join(relative), true, 0)?;
        if relative == BUNDLE {
            bundle_present = present;
        }
    }
    for relative in [".claude-plugin", "hooks"] {
        metadata(&bundle.join(relative), true, 0)?;
    }
    let paths = [
        bundle.join(FILES[0]),
        bundle.join(FILES[1]),
        bundle.join(FILES[2]),
        bundle.join(RECEIPT),
    ];
    let mut present = [false; 4];
    // Complete path validation precedes every read, including a bad receipt.
    for (index, path) in paths.iter().enumerate() {
        present[index] = metadata(
            path,
            false,
            if index == 3 {
                RECEIPT_LIMIT
            } else {
                FILE_LIMIT
            },
        )?;
    }
    if !bundle_present {
        return Ok(Inspection::Absent);
    }
    if present.iter().any(|present| !present) {
        return Err(conflict(&bundle, "owned files or receipt are missing"));
    }
    let receipt = read_bounded(&paths[3], RECEIPT_LIMIT)?;
    let Some(version) = catalog
        .iter()
        .find(|version| canonical_receipt(version).as_bytes() == receipt)
    else {
        return Err(conflict(
            &paths[3],
            "receipt is not a known canonical receipt",
        ));
    };
    let originals = [
        read_bounded(&paths[0], FILE_LIMIT)?,
        read_bounded(&paths[1], FILE_LIMIT)?,
        read_bounded(&paths[2], FILE_LIMIT)?,
        receipt,
    ];
    let intact = originals[..3]
        .iter()
        .zip(version.files)
        .all(|(actual, expected)| {
            actual == expected.as_bytes() && digest(actual) == digest(expected.as_bytes())
        });
    if intact {
        Ok(Inspection::Intact { version, originals })
    } else {
        Ok(Inspection::Edited { version, originals })
    }
}

pub(super) fn plan_update(
    root: &Path,
    catalog: &'static [BundleVersion],
    current: &'static BundleVersion,
) -> Result<Plan, Failure> {
    if !catalog
        .iter()
        .any(|version| version.version == current.version && version.files == current.files)
    {
        return Err(conflict(
            &root.join(BUNDLE),
            "target version is not in the compiled catalog",
        ));
    }
    let (outcome, originals) = match inspect(root, catalog)? {
        Inspection::Absent => (Outcome::Created, None),
        Inspection::Intact { version, originals } => {
            if version.version == current.version && version.files == current.files {
                return Ok(Plan {
                    outcome: Outcome::Current,
                    writes: Vec::new(),
                    cleanup: None,
                    cleanup_on_failure: None,
                });
            }
            (Outcome::Updated, Some(originals))
        }
        Inspection::Edited { .. } => {
            return Err(conflict(&root.join(BUNDLE), "owned payload was edited"));
        }
    };
    let contents = [
        current.files[0].to_string(),
        current.files[1].to_string(),
        current.files[2].to_string(),
        canonical_receipt(current),
    ];
    let paths = [FILES[0], FILES[1], FILES[2], RECEIPT];
    let writes = paths
        .into_iter()
        .zip(contents)
        .enumerate()
        .map(|(index, (path, content))| {
            PlannedWrite::write(
                root.join(BUNDLE).join(path),
                content,
                originals.as_ref().map(|bytes| bytes[index].clone()),
            )
        })
        .collect();
    let cleanup_on_failure = if outcome == Outcome::Created {
        Some(root.join(BUNDLE))
    } else {
        None
    };
    Ok(Plan {
        outcome,
        writes,
        cleanup: None,
        cleanup_on_failure,
    })
}

pub(super) fn plan_undo(root: &Path, catalog: &'static [BundleVersion]) -> Result<Plan, Failure> {
    match inspect(root, catalog)? {
        Inspection::Absent => Ok(Plan {
            outcome: Outcome::Absent,
            writes: Vec::new(),
            cleanup: None,
            cleanup_on_failure: None,
        }),
        Inspection::Edited { .. } => Ok(Plan {
            outcome: Outcome::Kept,
            writes: Vec::new(),
            cleanup: None,
            cleanup_on_failure: None,
        }),
        Inspection::Intact { originals, .. } => {
            let bundle = root.join(BUNDLE);
            let writes = [FILES[0], FILES[1], FILES[2], RECEIPT]
                .into_iter()
                .zip(originals)
                .map(|(path, original)| PlannedWrite::delete(bundle.join(path), original))
                .collect();
            Ok(Plan {
                outcome: Outcome::Removed,
                writes,
                cleanup: Some(bundle),
                cleanup_on_failure: None,
            })
        }
    }
}

impl Plan {
    pub(super) fn with_writes(mut self, additional: Vec<PlannedWrite>) -> Result<Self, Failure> {
        let mut destinations = std::collections::HashSet::new();
        for write in additional.iter().chain(&self.writes) {
            if !destinations.insert(&write.path) {
                return Err(conflict(&write.path, "duplicate planned destination"));
            }
        }
        let mut writes = additional;
        writes.append(&mut self.writes);
        self.writes = writes;
        Ok(self)
    }

    pub(super) fn commit(&self) -> Result<ApplyResult, Failure> {
        if let Err(failure) = super::commit(&self.writes) {
            if let Some(bundle) = &self.cleanup_on_failure {
                let residual = cleanup_directories(bundle, true);
                if !residual.is_empty() {
                    let mut warning =
                        String::from("Opening bundle rollback could not remove these paths:");
                    for path in residual {
                        warning.push_str(&format!("\n  {}", path.display()));
                    }
                    return match failure {
                        Failure::Io(error) => Err(Failure::Io(std::io::Error::new(
                            error.kind(),
                            format!("{error}\n{warning}"),
                        ))),
                        other => Err(other),
                    };
                }
            }
            return Err(failure);
        }
        let cleanup_failures = self
            .cleanup
            .as_ref()
            .map(|bundle| cleanup_directories(bundle, false))
            .unwrap_or_default();
        Ok(ApplyResult { cleanup_failures })
    }
}

fn cleanup_directories(bundle: &Path, report_nonempty: bool) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for directory in [
        bundle.join("hooks"),
        bundle.join(".claude-plugin"),
        bundle.to_path_buf(),
    ] {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::DirectoryNotEmpty && !report_nonempty => {}
            Err(_) => failed.push(directory),
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum FixtureAction {
        Preview,
        Apply,
        Undo,
        Inspect,
    }

    #[derive(serde::Deserialize)]
    enum FixtureVersion {
        #[serde(rename = "v1")]
        V1,
        #[serde(rename = "v2")]
        V2,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureRequest {
        root: String,
        action: FixtureAction,
        version: FixtureVersion,
    }

    #[derive(serde::Serialize)]
    struct FixtureResult {
        outcome: &'static str,
        #[serde(rename = "plannedwrites")]
        planned_writes: usize,
        applied: bool,
        #[serde(rename = "cleanupfailures")]
        cleanup_failures: usize,
        expected_receipt_sha256: Option<String>,
        receipt_sha256: Option<String>,
        version: Option<&'static str>,
        error_category: Option<&'static str>,
    }

    fn fixture_request(input: &str) -> Result<FixtureRequest, &'static str> {
        if input.len() > 16 * 1024 {
            return Err("request");
        }
        serde_json::from_str(input).map_err(|_| "request")
    }

    fn fixture_root(root: &str) -> Result<PathBuf, &'static str> {
        let normalized = root.replace('\\', "/");
        let path = PathBuf::from(normalized);
        let fixture = path.parent().ok_or("scope")?;
        let base = fixture.parent().ok_or("scope")?;
        let suffix = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("claude-bundle-rust-cycle-"))
            .ok_or("scope")?;
        if path.file_name().and_then(|name| name.to_str()) != Some("project")
            || base.file_name().and_then(|name| name.to_str()) != Some("instruction-reception")
            || base
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some(".scratch")
            || suffix.is_empty()
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err("scope");
        }
        Ok(path)
    }

    fn fixture_directories(root: &Path) -> Result<(), &'static str> {
        for path in root.ancestors() {
            let metadata = fs::symlink_metadata(path).map_err(|_| "scope_io")?;
            if linked(&metadata) || !metadata.is_dir() {
                return Err("scope");
            }
        }
        Ok(())
    }

    fn fixture_ancestor_store(root: &Path) -> Result<(), &'static str> {
        for path in root.ancestors() {
            match fs::symlink_metadata(path.join(".vivac")) {
                Ok(_) => return Err("ancestor_store"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("scope_io"),
            }
        }
        Ok(())
    }

    fn fixture_catalog() -> &'static [BundleVersion; 2] {
        static CATALOG: std::sync::OnceLock<[BundleVersion; 2]> = std::sync::OnceLock::new();
        CATALOG.get_or_init(|| {
            [
                ("v1", "1.0.0", "SYNTHETIC_NATIVE_BUNDLE_VERSION_v1_1a34"),
                ("v2", "2.0.0", "SYNTHETIC_NATIVE_BUNDLE_VERSION_v2_1a34"),
            ]
            .map(|(version, manifest_version, tag)| {
                let mut manifest: serde_json::Value =
                    serde_json::from_str(CURRENT.files[0]).unwrap();
                manifest["version"] = manifest_version.into();
                let manifest = format!("{}\n", serde_json::to_string(&manifest).unwrap());
                let anchor = "additionalContext: [...(result.additionalContext ?? []), state.marker]";
                assert_eq!(CURRENT.files[2].matches(anchor).count(), 1);
                let module = CURRENT.files[2].replace(
                    anchor,
                    &format!(
                        "additionalContext: [...(result.additionalContext ?? []), state.marker, '{tag}']"
                    ),
                );
                BundleVersion {
                    version,
                    files: [
                        Box::leak(manifest.into_boxed_str()),
                        CURRENT.files[1],
                        Box::leak(module.into_boxed_str()),
                    ],
                }
            })
        })
    }

    fn fixture_outcome(outcome: &Outcome) -> &'static str {
        match outcome {
            Outcome::Created => "created",
            Outcome::Updated => "updated",
            Outcome::Current => "current",
            Outcome::Removed => "removed",
            Outcome::Kept => "kept",
            Outcome::Absent => "absent",
        }
    }

    fn fixture_failure(failure: Failure) -> &'static str {
        match failure {
            Failure::Model(_) => "conflict",
            _ => "io",
        }
    }

    fn fixture_run(input: &str, result: &mut FixtureResult) -> Result<(), &'static str> {
        let request = fixture_request(input)?;
        let root = fixture_root(&request.root)?;
        if !root.is_absolute() {
            return Err("scope");
        }
        fixture_directories(&root)?;
        fixture_ancestor_store(root.parent().ok_or("scope")?)?;
        let catalog = fixture_catalog();
        let current = &catalog[match request.version {
            FixtureVersion::V1 => 0,
            FixtureVersion::V2 => 1,
        }];
        result.expected_receipt_sha256 = Some(digest(canonical_receipt(current).as_bytes()));
        if !matches!(request.action, FixtureAction::Inspect) {
            let plan = match request.action {
                FixtureAction::Undo => plan_undo(&root, catalog),
                _ => plan_update(&root, catalog, current),
            }
            .map_err(fixture_failure)?;
            result.outcome = fixture_outcome(&plan.outcome);
            result.planned_writes = plan.writes.len();
            if !matches!(request.action, FixtureAction::Preview) {
                let applied = plan.commit().map_err(|_| "commit")?;
                result.applied = true;
                result.cleanup_failures = applied.cleanup_failures.len();
            }
        }
        // Removal can leave additional files in the bundle directory. Inspecting
        // that directory as an installed bundle would misclassify successful undo.
        if matches!(request.action, FixtureAction::Undo)
            && matches!(result.outcome, "removed" | "absent")
        {
            return Ok(());
        }
        match inspect(&root, catalog).map_err(fixture_failure)? {
            Inspection::Absent => {
                if matches!(request.action, FixtureAction::Inspect) {
                    result.outcome = "absent";
                }
            }
            Inspection::Intact { originals, version } => {
                result.receipt_sha256 = Some(digest(&originals[3]));
                result.version = Some(version.version);
                if matches!(request.action, FixtureAction::Inspect) {
                    result.outcome = "intact";
                }
            }
            Inspection::Edited { originals, version } => {
                result.receipt_sha256 = Some(digest(&originals[3]));
                result.version = Some(version.version);
                if matches!(request.action, FixtureAction::Inspect) {
                    result.outcome = "edited";
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "Requires an explicitly scoped native fixture request"]
    fn native_bundle_cycle_fixture() {
        use std::io::Write;
        let mut result = FixtureResult {
            outcome: "error",
            planned_writes: 0,
            applied: false,
            cleanup_failures: 0,
            expected_receipt_sha256: None,
            receipt_sha256: None,
            version: None,
            error_category: None,
        };
        let status = match std::env::var("VIVAC_OPENING_FIXTURE_REQUEST") {
            Ok(input) => fixture_run(&input, &mut result),
            Err(_) => Err("request"),
        };
        if let Err(category) = status {
            result.outcome = if category == "conflict" {
                "conflict"
            } else {
                "error"
            };
            result.error_category = Some(category);
        }
        let mut stdout = std::io::stdout().lock();
        writeln!(
            stdout,
            "VIVAC_OPENING_FIXTURE_RESULT {}",
            serde_json::to_string(&result).unwrap()
        )
        .unwrap();
        stdout.flush().unwrap();
        assert!(status.is_ok(), "Opening bundle fixture request failed");
    }

    #[test]
    fn compiled_catalog_updates_and_undoes_real_v2_without_adopting_edits() {
        static PREVIOUS_CATALOG: [BundleVersion; 1] = [BundleVersion {
            version: "opening-reference-v2",
            files: [
                include_str!(
                    "claude-opening-versions/opening-reference-v2/.claude-plugin/plugin.json"
                ),
                include_str!("claude-opening-versions/opening-reference-v2/hooks/hooks.json"),
                include_str!("claude-opening-versions/opening-reference-v2/hooks/register.mjs"),
            ],
        }];
        assert_eq!(
            digest(PREVIOUS_CATALOG[0].files[2].as_bytes()),
            "415161b5c5a3a02f02cdb147b741d06a53e7a9a57cb44a408fe503d8efe6ba8c"
        );
        for action in ["update", "undo", "edited"] {
            let project = Project::new();
            plan_update(&project.0, &PREVIOUS_CATALOG, &PREVIOUS_CATALOG[0])
                .unwrap()
                .commit()
                .unwrap();
            if action == "edited" {
                fs::write(&owned_paths(&project)[2], "export const edited = true;\n").unwrap();
                let before = snapshot(&project);
                assert!(plan_update(&project.0, &super::CATALOG, &super::CURRENT).is_err());
                let undo = plan_undo(&project.0, &super::CATALOG).unwrap();
                assert_eq!(undo.outcome, Outcome::Kept);
                undo.commit().unwrap();
                assert_eq!(snapshot(&project), before);
            } else if action == "undo" {
                let undo = plan_undo(&project.0, &super::CATALOG).unwrap();
                assert_eq!(undo.outcome, Outcome::Removed);
                undo.commit().unwrap();
                assert!(owned_paths(&project).iter().all(|path| !path.exists()));
            } else {
                let update = plan_update(&project.0, &super::CATALOG, &super::CURRENT).unwrap();
                assert_eq!(update.outcome, Outcome::Updated);
                update.commit().unwrap();
                assert!(matches!(
                    inspect(&project.0, &super::CATALOG).unwrap(),
                    Inspection::Intact { version, .. } if version.version == "opening-reference-v3"
                ));
                for (path, expected) in owned_paths(&project)[..3].iter().zip(super::CURRENT.files)
                {
                    assert_eq!(fs::read(path).unwrap(), expected.as_bytes());
                }
                let undo = plan_undo(&project.0, &super::CATALOG).unwrap();
                assert_eq!(undo.outcome, Outcome::Removed);
                undo.commit().unwrap();
                assert!(owned_paths(&project).iter().all(|path| !path.exists()));
            }
        }
    }

    #[test]
    fn compiled_current_catalog_installs_the_exact_embedded_payloads() {
        let project = Project::new();
        assert_eq!(super::CURRENT.version, "opening-reference-v3");
        assert_eq!(super::CATALOG.len(), 3);
        let plan = plan_update(&project.0, &super::CATALOG, &super::CURRENT).unwrap();
        assert_eq!(plan.writes.len(), 4);
        plan.commit().unwrap();
        for (path, expected) in owned_paths(&project)[..3].iter().zip(super::CURRENT.files) {
            assert_eq!(fs::read(path).unwrap(), expected.as_bytes());
        }
        assert_eq!(
            fs::read(&owned_paths(&project)[3]).unwrap(),
            canonical_receipt(&super::CURRENT).as_bytes()
        );
        assert!(matches!(
            inspect(&project.0, &super::CATALOG).unwrap(),
            Inspection::Intact { .. }
        ));
    }

    #[test]
    fn native_fixture_versions_are_known_distinct_and_derived_from_current() {
        let catalog = fixture_catalog();
        assert_ne!(
            canonical_receipt(&catalog[0]),
            canonical_receipt(&catalog[1])
        );
        for (version, tag, manifest_version) in [
            (
                &catalog[0],
                "SYNTHETIC_NATIVE_BUNDLE_VERSION_v1_1a34",
                "1.0.0",
            ),
            (
                &catalog[1],
                "SYNTHETIC_NATIVE_BUNDLE_VERSION_v2_1a34",
                "2.0.0",
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(version.files[0]).unwrap()["version"],
                manifest_version
            );
            assert!(version.files[2].contains(tag));
            assert_eq!(version.files[1], super::CURRENT.files[1]);
        }
    }

    #[test]
    fn fixture_request_bounds_and_external_scope_reject_unrelated_roots() {
        let request = r#"{"root":"C:/.scratch/instruction-reception/claude-bundle-rust-cycle-test/project","action":"preview","version":"v1"}"#;
        assert!(fixture_request(&format!(
            "{request}{}",
            " ".repeat(16 * 1024 - request.len())
        ))
        .is_ok());
        assert!(fixture_request(&format!(
            "{request}{}",
            " ".repeat(16 * 1024 + 1 - request.len())
        ))
        .is_err());
        assert!(fixture_root(&fixture_request(request).unwrap().root).is_ok());
        for root in [
            "project",
            "C:/unrelated-project",
            "C:/.scratch/instruction-reception/claude-bundle-rust-cycle-test",
            "C:/.scratch/instruction-reception/claude-bundle-rust-cycle-test/../project",
            "C:/.scratch/instruction-reception/claude-bundle-rust-cycle-/project",
        ] {
            assert!(fixture_root(root).is_err());
        }
    }

    #[test]
    fn fixture_directory_guard_rejects_linked_ancestors_without_following_them() {
        let project = Project::new();
        let target = project.0.join("target");
        fs::create_dir_all(target.join("project")).unwrap();
        let link = project.0.join("linked");
        match make_link(&target, &link, true) {
            Ok(()) => {}
            #[cfg(windows)]
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("Directory symlink creation unavailable: {error}");
                return;
            }
            Err(error) => panic!("symlink creation failed: {error}"),
        }
        assert_eq!(fixture_directories(&link.join("project")), Err("scope"));
        assert!(target.join("project").is_dir());
    }

    static V1: BundleVersion = BundleVersion {
        version: "fixture-v1",
        files: [
            "{\"name\":\"fixture-one\"}\n",
            "{\"hooks\":{}}\n",
            "export {};\n",
        ],
    };
    static V2: BundleVersion = BundleVersion {
        version: "fixture-v2",
        files: [
            "{\"name\":\"fixture-two\"}\n",
            "{\"hooks\":{}}\n",
            "export const fixture = 2;\n",
        ],
    };
    static CATALOG: [BundleVersion; 2] = [V1, V2];

    struct Project(PathBuf);
    impl Project {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("vivac-opening-{}", crate::id::ulid()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn fresh_plan_has_four_writes_and_does_not_create_directories() {
        let project = Project::new();
        let plan = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
        assert_eq!(plan.outcome, Outcome::Created);
        assert_eq!(plan.writes.len(), 4);
        assert!(plan.writes.iter().all(|write| write.original.is_none()));
        assert!(!project.0.join(".claude").exists());
        super::super::commit(&plan.writes).unwrap();
        for (file, content) in FILES.iter().zip(V1.files) {
            assert_eq!(
                fs::read(project.0.join(BUNDLE).join(file)).unwrap(),
                content.as_bytes()
            );
        }
        let current = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
        assert_eq!(current.outcome, Outcome::Current);
        assert!(current.writes.is_empty());
    }

    #[test]
    fn combined_create_orders_external_writes_before_bundle_and_receipt_last() {
        let project = Project::new();
        let settings = project.0.join("settings.json");
        let original = b"original settings\r\n";
        fs::write(&settings, original).unwrap();
        let plan = plan_update(&project.0, &CATALOG, &CATALOG[0])
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "new settings\n".into(),
                Some(original.to_vec()),
            )])
            .unwrap();
        assert_eq!(plan.outcome, Outcome::Created);
        assert_eq!(plan.writes.len(), 5);
        assert_eq!(plan.writes[0].path, settings);
        assert_eq!(plan.writes[4].path, project.0.join(BUNDLE).join(RECEIPT));
        assert_eq!(fs::read(&settings).unwrap(), original);
        assert!(!project.0.join(BUNDLE).exists());
        assert!(plan.commit().unwrap().cleanup_failures.is_empty());
        assert_eq!(fs::read(settings).unwrap(), b"new settings\n");
        assert!(matches!(
            inspect(&project.0, &CATALOG).unwrap(),
            Inspection::Intact { .. }
        ));
    }

    #[test]
    fn combined_update_bundle_failure_restores_external_settings_and_bundle_bytes() {
        let project = Project::new();
        install(&project);
        let before = snapshot(&project);
        let settings = project.0.join("settings.json");
        let original = b"original settings\r\n";
        fs::write(&settings, original).unwrap();
        let plan = plan_update(&project.0, &CATALOG, &CATALOG[1])
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "new settings".into(),
                Some(original.to_vec()),
            )])
            .unwrap();
        super::super::FAILURE_AFTER_PUBLICATION
            .with(|failure| failure.replace(Some(project.0.join(BUNDLE).join(RECEIPT))));
        assert!(plan
            .commit()
            .unwrap_err()
            .message()
            .contains("directory sync failure"));
        assert_eq!(snapshot(&project), before);
        assert_eq!(fs::read(settings).unwrap(), original);
    }

    #[test]
    fn combined_fresh_bundle_failure_restores_settings_and_cleans_owned_directories() {
        let project = Project::new();
        let settings = project.0.join("settings.json");
        let original = b"original settings\r\n";
        fs::write(&settings, original).unwrap();
        let plan = plan_update(&project.0, &CATALOG, &CATALOG[0])
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "new settings".into(),
                Some(original.to_vec()),
            )])
            .unwrap();
        super::super::FAILURE_AFTER_PUBLICATION
            .with(|failure| failure.replace(Some(project.0.join(BUNDLE).join(RECEIPT))));
        assert!(plan.commit().is_err());
        assert_eq!(fs::read(settings).unwrap(), original);
        assert!(matches!(
            inspect(&project.0, &CATALOG).unwrap(),
            Inspection::Absent
        ));
        assert!(project.0.join(".claude/skills").is_dir());
        assert!(!project.0.join(BUNDLE).exists());
    }

    #[test]
    fn combined_undo_external_failure_preserves_bundle_and_settings() {
        let project = Project::new();
        install(&project);
        let before = snapshot(&project);
        let settings = project.0.join("settings.json");
        let original = b"original settings\r\n";
        fs::write(&settings, original).unwrap();
        let plan = plan_undo(&project.0, &CATALOG)
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "restored configuration".into(),
                Some(original.to_vec()),
            )])
            .unwrap();
        super::super::FAILURE_AFTER_PUBLICATION
            .with(|failure| failure.replace(Some(settings.clone())));
        assert!(plan.commit().is_err());
        assert_eq!(snapshot(&project), before);
        assert_eq!(fs::read(settings).unwrap(), original);
        assert!(project.0.join(BUNDLE).is_dir());
    }

    #[test]
    fn combined_undo_late_delete_failure_restores_deleted_bundle_files_and_settings() {
        let project = Project::new();
        install(&project);
        let before = snapshot(&project);
        let settings = project.0.join("settings.json");
        let original = b"original settings\r\n";
        fs::write(&settings, original).unwrap();
        let mut undo = plan_undo(&project.0, &CATALOG).unwrap();
        let blocked = project.0.join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("foreign"), b"foreign sentinel").unwrap();
        undo.writes[3].path = blocked.clone();
        let plan = undo
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "restored configuration".into(),
                Some(original.to_vec()),
            )])
            .unwrap();
        assert!(plan.commit().is_err());
        assert_eq!(snapshot(&project), before);
        assert_eq!(fs::read(settings).unwrap(), original);
        assert_eq!(
            fs::read(blocked.join("foreign")).unwrap(),
            b"foreign sentinel"
        );
    }

    #[test]
    fn composition_rejects_duplicates_in_additional_bundle_and_between_both_without_writes() {
        for case in 0..3 {
            let project = Project::new();
            let settings = project.0.join("settings.json");
            let original = b"original settings\r\n";
            fs::write(&settings, original).unwrap();
            let mut plan = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
            let additional = match case {
                0 => vec![
                    PlannedWrite::write(settings.clone(), "first".into(), Some(original.to_vec())),
                    PlannedWrite::delete(settings.clone(), original.to_vec()),
                ],
                1 => vec![PlannedWrite::write(
                    plan.writes[3].path.clone(),
                    "foreign receipt".into(),
                    None,
                )],
                _ => {
                    plan.writes[3].path = plan.writes[0].path.clone();
                    Vec::new()
                }
            };
            let error = plan.with_writes(additional).err().unwrap();
            assert!(error.message().contains("duplicate planned destination"));
            assert_eq!(fs::read(settings).unwrap(), original);
            assert!(!project.0.join(".claude").exists());
        }
    }

    #[test]
    fn current_and_edited_kept_plans_commit_only_additional_writes() {
        let project = Project::new();
        install(&project);
        let settings = project.0.join("settings.json");
        fs::write(&settings, b"old settings").unwrap();
        let before = snapshot(&project);
        let current = plan_update(&project.0, &CATALOG, &CATALOG[0])
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "current settings".into(),
                Some(b"old settings".to_vec()),
            )])
            .unwrap();
        assert_eq!(current.outcome, Outcome::Current);
        assert_eq!(current.writes.len(), 1);
        current.commit().unwrap();
        assert_eq!(snapshot(&project), before);
        assert_eq!(fs::read(&settings).unwrap(), b"current settings");
        fs::write(&owned_paths(&project)[2], b"edited by owner\r\n").unwrap();
        let edited = snapshot(&project);
        let kept = plan_undo(&project.0, &CATALOG)
            .unwrap()
            .with_writes(vec![PlannedWrite::write(
                settings.clone(),
                "kept settings".into(),
                Some(b"current settings".to_vec()),
            )])
            .unwrap();
        assert_eq!(kept.outcome, Outcome::Kept);
        assert_eq!(kept.writes.len(), 1);
        assert!(kept.commit().unwrap().cleanup_failures.is_empty());
        assert_eq!(snapshot(&project), edited);
        assert_eq!(fs::read(settings).unwrap(), b"kept settings");
    }

    #[test]
    fn fresh_failure_removes_empty_owned_directories_and_allows_retry() {
        let project = Project::new();
        let mut plan = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
        let blocked = project.0.join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("occupied"), "foreign sentinel").unwrap();
        plan.writes[3].path = blocked.clone();
        assert!(plan.commit().is_err());
        assert!(matches!(
            inspect(&project.0, &CATALOG).unwrap(),
            Inspection::Absent
        ));
        assert!(project.0.join(".claude/skills").is_dir());
        assert_eq!(
            fs::read(blocked.join("occupied")).unwrap(),
            b"foreign sentinel"
        );
        install(&project);
        assert!(matches!(
            inspect(&project.0, &CATALOG).unwrap(),
            Inspection::Intact { .. }
        ));
    }

    #[test]
    fn fresh_failure_after_receipt_publication_removes_the_complete_bundle() {
        let project = Project::new();
        let plan = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
        super::super::FAILURE_AFTER_PUBLICATION
            .with(|failure| failure.replace(Some(project.0.join(BUNDLE).join(RECEIPT))));
        let error = plan.commit().unwrap_err();
        assert!(error.message().contains("directory sync failure"));
        assert!(matches!(
            inspect(&project.0, &CATALOG).unwrap(),
            Inspection::Absent
        ));
        assert!(project.0.join(".claude/skills").is_dir());
        install(&project);
    }

    #[test]
    fn fresh_failure_preserves_extra_files_and_reports_the_residual_bundle() {
        let project = Project::new();
        let mut plan = plan_update(&project.0, &CATALOG, &CATALOG[0]).unwrap();
        let blocked = project.0.join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("occupied"), "foreign sentinel").unwrap();
        plan.writes[3].path = blocked;
        let bundle = project.0.join(BUNDLE);
        fs::create_dir_all(bundle.join("hooks")).unwrap();
        let extra = bundle.join("hooks/foreign.txt");
        fs::write(&extra, "extra sentinel").unwrap();
        let error = plan.commit().unwrap_err();
        assert_eq!(fs::read(extra).unwrap(), b"extra sentinel");
        assert!(owned_paths(&project).iter().all(|path| !path.exists()));
        assert!(
            error.message().contains(&bundle.display().to_string()),
            "{}",
            error.message()
        );
        assert!(error.message().contains("could not be written"));
        assert_refused(&project);
    }

    fn install(project: &Project) {
        plan_update(&project.0, &CATALOG, &CATALOG[0])
            .unwrap()
            .commit()
            .unwrap();
    }

    fn owned_paths(project: &Project) -> [PathBuf; 4] {
        [FILES[0], FILES[1], FILES[2], RECEIPT].map(|path| project.0.join(BUNDLE).join(path))
    }

    fn snapshot(project: &Project) -> [Vec<u8>; 4] {
        owned_paths(project).map(|path| fs::read(path).unwrap())
    }

    fn assert_refused(project: &Project) {
        assert!(inspect(&project.0, &CATALOG).is_err());
        assert!(plan_update(&project.0, &CATALOG, &CATALOG[1]).is_err());
        assert!(plan_undo(&project.0, &CATALOG).is_err());
    }

    #[test]
    fn receipt_is_fixed_order_compact_sha256_with_one_newline() {
        let version = BundleVersion {
            version: "vectors",
            files: ["abc", "", "a"],
        };
        assert_eq!(
            canonical_receipt(&version),
            concat!(
                "{\"files\":{\".claude-plugin/plugin.json\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",",
                "\"hooks/hooks.json\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\",",
                "\"hooks/register.mjs\":\"ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb\"},",
                "\"schema\":1,\"version\":\"vectors\"}\n"
            )
        );
    }

    #[test]
    fn known_versions_update_and_undo_preserve_foreign_parents() {
        let project = Project::new();
        fs::create_dir_all(project.0.join(".claude/skills/foreign")).unwrap();
        let foreign = project.0.join(".claude/skills/foreign/keep.txt");
        fs::write(&foreign, b"foreign").unwrap();
        install(&project);
        let before = snapshot(&project);
        assert!(
            matches!(inspect(&project.0, &CATALOG).unwrap(), Inspection::Intact { version, .. } if version.version == V1.version)
        );
        let update = plan_update(&project.0, &CATALOG, &CATALOG[1]).unwrap();
        assert_eq!(update.outcome, Outcome::Updated);
        assert_eq!(update.writes.len(), 4);
        for (write, original) in update.writes.iter().zip(&before) {
            assert_eq!(write.original.as_ref().unwrap(), original);
        }
        assert_eq!(snapshot(&project), before);
        update.commit().unwrap();
        assert!(
            matches!(inspect(&project.0, &CATALOG).unwrap(), Inspection::Intact { version, .. } if version.version == V2.version)
        );
        let current = plan_update(&project.0, &CATALOG, &CATALOG[1]).unwrap();
        assert_eq!(current.outcome, Outcome::Current);
        assert!(current.writes.is_empty());
        let undo = plan_undo(&project.0, &CATALOG).unwrap();
        assert_eq!(undo.outcome, Outcome::Removed);
        assert_eq!(undo.writes.len(), 4);
        assert!(undo
            .writes
            .iter()
            .all(|write| matches!(write.action, super::super::Action::Delete)));
        let applied = undo.commit().unwrap();
        assert!(applied.cleanup_failures.is_empty());
        assert!(!project.0.join(BUNDLE).exists());
        assert!(project.0.join(".claude/skills").is_dir());
        assert_eq!(fs::read(foreign).unwrap(), b"foreign");
        let absent = plan_undo(&project.0, &CATALOG).unwrap();
        assert_eq!(absent.outcome, Outcome::Absent);
        assert!(absent.writes.is_empty());
    }

    #[test]
    fn each_edited_payload_blocks_update_and_keeps_the_entire_bundle_on_undo() {
        for index in 0..3 {
            let project = Project::new();
            install(&project);
            fs::write(&owned_paths(&project)[index], "edited by owner\r\n").unwrap();
            let before = snapshot(&project);
            assert!(
                matches!(inspect(&project.0, &CATALOG).unwrap(), Inspection::Edited { version, originals } if version.version == V1.version && originals == before)
            );
            assert!(plan_update(&project.0, &CATALOG, &CATALOG[1]).is_err());
            let undo = plan_undo(&project.0, &CATALOG).unwrap();
            assert_eq!(undo.outcome, Outcome::Kept);
            assert!(undo.writes.is_empty());
            undo.commit().unwrap();
            assert_eq!(snapshot(&project), before);
        }
    }

    #[test]
    fn unknown_extra_files_are_not_read_owned_or_deleted() {
        let project = Project::new();
        install(&project);
        let extras = [
            "extra.bin",
            "hooks/foreign.bin",
            ".claude-plugin/foreign.bin",
        ];
        for extra in extras {
            fs::write(project.0.join(BUNDLE).join(extra), [0xff, 0xfe]).unwrap();
        }
        let update = plan_update(&project.0, &CATALOG, &CATALOG[1]).unwrap();
        update.commit().unwrap();
        plan_undo(&project.0, &CATALOG).unwrap().commit().unwrap();
        for extra in extras {
            assert_eq!(
                fs::read(project.0.join(BUNDLE).join(extra)).unwrap(),
                [0xff, 0xfe]
            );
        }
        assert!(project.0.join(BUNDLE).is_dir());
    }

    #[test]
    fn missing_receipt_never_adopts_an_existing_bundle() {
        for populated in [false, true] {
            let project = Project::new();
            if populated {
                install(&project);
                fs::remove_file(project.0.join(BUNDLE).join(RECEIPT)).unwrap();
            } else {
                fs::create_dir_all(project.0.join(BUNDLE)).unwrap();
            }
            assert_refused(&project);
        }
    }

    #[test]
    fn each_missing_owned_file_is_a_conflict() {
        for index in 0..4 {
            let project = Project::new();
            install(&project);
            fs::remove_file(&owned_paths(&project)[index]).unwrap();
            assert_refused(&project);
        }
    }

    #[test]
    fn noncanonical_unknown_duplicate_and_extra_receipts_are_conflicts() {
        let canonical = canonical_receipt(&CATALOG[0]);
        let mut extra_route: serde_json::Value = serde_json::from_str(&canonical).unwrap();
        extra_route["files"]["../outside"] = serde_json::json!(digest(b"outside"));
        let candidates = [
            "not json".to_string(),
            canonical.trim_end().to_string(),
            format!(" {canonical}"),
            canonical.replace("fixture-v1", "unknown-version"),
            canonical.replace("\"schema\":1", "\"schema\":1,\"schema\":1"),
            canonical.replace("\"schema\":1", "\"schema\":\"1\""),
            canonical.replace("\"schema\":1", "\"extra\":0,\"schema\":1"),
            format!("{}\n", serde_json::to_string(&extra_route).unwrap()),
            canonical.replace(
                &digest(V1.files[0].as_bytes()),
                &digest(V1.files[0].as_bytes()).to_uppercase(),
            ),
        ];
        for candidate in candidates {
            let project = Project::new();
            install(&project);
            fs::write(project.0.join(BUNDLE).join(RECEIPT), candidate.as_bytes()).unwrap();
            let before = snapshot(&project);
            assert_refused(&project);
            assert_eq!(snapshot(&project), before);
        }
    }

    #[test]
    fn rehashing_an_edited_payload_does_not_prove_ownership() {
        let project = Project::new();
        install(&project);
        fs::write(&owned_paths(&project)[0], "forged").unwrap();
        let forged = canonical_receipt(&CATALOG[0])
            .replace(&digest(V1.files[0].as_bytes()), &digest(b"forged"));
        fs::write(&owned_paths(&project)[3], forged).unwrap();
        let before = snapshot(&project);
        assert_refused(&project);
        assert_eq!(snapshot(&project), before);
    }

    #[test]
    fn every_owned_file_requires_utf8_and_a_bounded_size() {
        for index in 0..4 {
            let project = Project::new();
            install(&project);
            let path = &owned_paths(&project)[index];
            fs::write(path, [0xff]).unwrap();
            assert_refused(&project);
            let limit = if index == 3 {
                RECEIPT_LIMIT
            } else {
                FILE_LIMIT
            };
            fs::write(path, vec![b'a'; limit as usize + 1]).unwrap();
            let error = inspect(&project.0, &CATALOG).err().unwrap();
            assert!(error.message().contains("size limit"));
            assert_refused(&project);
        }
    }

    #[test]
    fn every_fixed_directory_and_file_rejects_the_wrong_type() {
        for relative in [
            ".claude",
            ".claude/skills",
            BUNDLE,
            ".claude/skills/vivac-opening/hooks",
            ".claude/skills/vivac-opening/.claude-plugin",
        ] {
            let project = Project::new();
            let path = project.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "file where directory belongs").unwrap();
            assert_refused(&project);
        }
        for index in 0..4 {
            let project = Project::new();
            install(&project);
            let path = &owned_paths(&project)[index];
            fs::remove_file(path).unwrap();
            fs::create_dir(path).unwrap();
            assert_refused(&project);
        }
    }

    #[test]
    fn project_root_must_exist_and_be_a_directory() {
        let project = Project::new();
        let missing = project.0.join("missing");
        assert!(inspect(&missing, &CATALOG).is_err());
        let file = project.0.join("file");
        fs::write(&file, "root").unwrap();
        assert!(inspect(&file, &CATALOG).is_err());
    }

    #[test]
    fn a_failed_fourth_write_restores_all_three_payloads_exactly() {
        let project = Project::new();
        install(&project);
        let before = snapshot(&project);
        let mut plan = plan_update(&project.0, &CATALOG, &CATALOG[1]).unwrap();
        // Inject a fourth destination that cannot be replaced, leaving the
        // original receipt untouched while the three earlier writes roll back.
        let blocked = project.0.join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("occupied"), "keep").unwrap();
        plan.writes[3].path = blocked.clone();
        let error = plan.commit().unwrap_err();
        assert!(error
            .message()
            .contains("setup restored every planned destination\n  it changed."));
        assert_eq!(snapshot(&project), before);
        assert_eq!(fs::read(blocked.join("occupied")).unwrap(), b"keep");
    }

    #[test]
    fn updates_preserve_foreign_files_at_the_old_temporary_names() {
        let project = Project::new();
        install(&project);
        let temporaries = owned_paths(&project).map(|path| super::super::sibling_temp(&path));
        for temporary in &temporaries {
            fs::write(temporary, b"foreign temporary sentinel").unwrap();
        }
        plan_update(&project.0, &CATALOG, &CATALOG[1])
            .unwrap()
            .commit()
            .unwrap();
        for temporary in &temporaries {
            assert_eq!(fs::read(temporary).unwrap(), b"foreign temporary sentinel");
        }
        assert!(
            matches!(inspect(&project.0, &CATALOG).unwrap(), Inspection::Intact { version, .. } if version.version == V2.version)
        );
    }

    #[test]
    fn updates_preserve_foreign_symlinks_at_the_old_temporary_names() {
        let project = Project::new();
        install(&project);
        let outside =
            [0, 1, 2, 3].map(|index| project.0.join(format!("outside-sentinel-{index}.txt")));
        for path in &outside {
            fs::write(path, b"outside sentinel").unwrap();
        }
        let temporaries = owned_paths(&project).map(|path| super::super::sibling_temp(&path));
        for (temporary, outside) in temporaries.iter().zip(&outside) {
            match make_link(outside, temporary, false) {
                Ok(()) => {}
                #[cfg(windows)]
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("File symlink creation unavailable: {error}");
                    return;
                }
                Err(error) => panic!("symlink creation failed: {error}"),
            }
        }
        plan_update(&project.0, &CATALOG, &CATALOG[1])
            .unwrap()
            .commit()
            .unwrap();
        for (temporary, outside) in temporaries.iter().zip(&outside) {
            assert_eq!(fs::read(outside).unwrap(), b"outside sentinel");
            assert!(fs::symlink_metadata(temporary)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_link(temporary).unwrap(), *outside);
        }
    }

    #[cfg(unix)]
    fn make_link(source: &Path, target: &Path, _directory: bool) -> std::io::Result<()> {
        std::os::unix::fs::symlink(source, target)
    }

    #[cfg(windows)]
    fn make_link(source: &Path, target: &Path, directory: bool) -> std::io::Result<()> {
        if directory {
            std::os::windows::fs::symlink_dir(source, target)
        } else {
            std::os::windows::fs::symlink_file(source, target)
        }
    }

    #[test]
    fn linked_fixed_files_are_rejected_even_with_an_invalid_receipt() {
        for index in 0..4 {
            let project = Project::new();
            install(&project);
            let path = &owned_paths(&project)[index];
            let outside = project.0.join("outside.txt");
            fs::write(&outside, "external file").unwrap();
            fs::remove_file(path).unwrap();
            match make_link(&outside, path, false) {
                Ok(()) => {}
                #[cfg(windows)]
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("File symlink creation unavailable: {error}");
                    return;
                }
                Err(error) => panic!("symlink creation failed: {error}"),
            }
            if index != 3 {
                fs::write(&owned_paths(&project)[3], "invalid receipt").unwrap();
            }
            let error = inspect(&project.0, &CATALOG).err().unwrap();
            assert!(error.message().contains("links and reparse"));
            assert_refused(&project);
            assert_eq!(fs::read(&outside).unwrap(), b"external file");
        }
    }

    #[test]
    fn linked_project_root_and_fixed_directories_are_rejected() {
        for relative in [
            "linked-root",
            ".claude",
            ".claude/skills",
            BUNDLE,
            ".claude/skills/vivac-opening/hooks",
            ".claude/skills/vivac-opening/.claude-plugin",
        ] {
            let project = Project::new();
            let outside = project.0.join("outside");
            fs::create_dir(&outside).unwrap();
            let path = project.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match make_link(&outside, &path, true) {
                Ok(()) => {}
                #[cfg(windows)]
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("Directory symlink creation unavailable: {error}");
                    return;
                }
                Err(error) => panic!("symlink creation failed: {error}"),
            }
            let root = if relative == "linked-root" {
                &path
            } else {
                &project.0
            };
            let error = inspect(root, &CATALOG).err().unwrap();
            assert!(error.message().contains("links and reparse"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_real_windows_junction_is_rejected_as_a_reparse_point() {
        let project = Project::new();
        let outside = project.0.join("outside");
        fs::create_dir(&outside).unwrap();
        let junction = project.0.join(".claude");
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {:?}",
            output.status
        );
        use std::os::windows::fs::MetadataExt;
        assert_ne!(
            fs::symlink_metadata(&junction).unwrap().file_attributes() & 0x400,
            0
        );
        let error = inspect(&project.0, &CATALOG).err().unwrap();
        assert!(error.message().contains("links and reparse"));
        assert_refused(&project);
        fs::remove_dir(junction).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_owned_file_reports_io_without_writes() {
        use std::os::windows::fs::OpenOptionsExt;
        let project = Project::new();
        install(&project);
        let before = snapshot(&project);
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&owned_paths(&project)[0])
            .unwrap();
        assert!(matches!(inspect(&project.0, &CATALOG), Err(Failure::Io(_))));
        assert_refused(&project);
        drop(lock);
        assert_eq!(snapshot(&project), before);
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_failure_reports_a_path_after_successful_file_deletion() {
        use std::os::windows::fs::OpenOptionsExt;
        let project = Project::new();
        install(&project);
        let undo = plan_undo(&project.0, &CATALOG).unwrap();
        let hooks = project.0.join(BUNDLE).join("hooks");
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(0x0200_0000)
            .open(&hooks)
            .unwrap();
        let applied = undo.commit().unwrap();
        assert_eq!(applied.cleanup_failures, vec![hooks]);
        assert!(owned_paths(&project).iter().all(|path| !path.exists()));
        assert!(project.0.join(BUNDLE).is_dir());
        drop(lock);
    }

    #[cfg(unix)]
    #[test]
    fn an_owned_socket_is_rejected_as_an_unexpected_type() {
        let root = std::path::Path::new("/tmp").join(format!("vo-{}", crate::id::ulid()));
        fs::create_dir(&root).unwrap();
        let project = Project(root);
        install(&project);
        let path = &owned_paths(&project)[1];
        fs::remove_file(path).unwrap();
        let socket = std::os::unix::net::UnixListener::bind(path).unwrap();
        let error = inspect(&project.0, &CATALOG).err().unwrap();
        assert!(error.message().contains("unexpected file type"));
        assert_refused(&project);
        drop(socket);
    }
}
