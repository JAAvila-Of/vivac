#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

mod common;
use common::Sandbox;
use serde_json::{json, Value};
use std::fs;

fn assert_isolated(root: &std::path::Path) {
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let workspace = crate_root
        .parent()
        .filter(|parent| parent.parent().is_some())
        .unwrap_or(&crate_root);
    assert!(
        !root.starts_with(workspace),
        "artifact fixtures must be outside the workspace"
    );
    assert!(
        !root
            .ancestors()
            .skip(1)
            .any(|parent| ["config", "events", "lane"]
                .into_iter()
                .any(|name| parent.join(".vivac").join(name).exists())),
        "artifact fixtures must not inherit a tree"
    );
}

fn fixture(name: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    assert_isolated(&sandbox.0.canonicalize().unwrap());
    sandbox.ok(&["init", "--yes"]);
    sandbox.ok(&[
        "push",
        "Review selected static delivery",
        "--why",
        "retain a complete revision",
    ]);
    fs::create_dir_all(sandbox.0.join("app/assets")).unwrap();
    fs::write(sandbox.0.join("app/index.html"), b"<!doctype html><link rel=stylesheet href=assets/style.css><script src=assets/main.js></script>").unwrap();
    fs::write(
        sandbox.0.join("app/assets/style.css"),
        b"body { color: navy; }",
    )
    .unwrap();
    fs::write(
        sandbox.0.join("app/assets/main.js"),
        b"document.title = 'Selected delivery';",
    )
    .unwrap();
    fs::write(sandbox.0.join("selection.json"), serde_json::to_vec(&json!({"root":"app","entrypoint":"index.html","members":["index.html","assets/style.css","assets/main.js"],"requirements":[]})).unwrap()).unwrap();
    sandbox
}

#[test]
fn an_ancestral_tree_is_refused_before_bundle_fixture_initialization() {
    let sandbox = Sandbox::new_empty("bundle-ancestry-guard");
    let root = sandbox.0.canonicalize().unwrap();
    assert_isolated(&root);
    fs::create_dir(root.join(".vivac")).unwrap();
    let marker = root.join(".vivac/config");
    fs::write(&marker, b"Synthetic ancestry marker").unwrap();
    let child = root.join("nested");
    fs::create_dir(&child).unwrap();
    assert!(std::panic::catch_unwind(|| assert_isolated(&child)).is_err());
    assert!(!child.join(".vivac").exists());
    assert_eq!(fs::read(marker).unwrap(), b"Synthetic ancestry marker");
}
fn capture(sandbox: &Sandbox, extra: &[&str]) -> Value {
    let mut args = vec![
        "artifacts",
        "capture",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--json",
    ];
    args.extend_from_slice(extra);
    serde_json::from_str::<Value>(&sandbox.ok(&args)).unwrap()["delivery"].clone()
}
#[test]
fn selected_bundle_revisions_restore_complete_original_after_source_removal() {
    let sandbox = fixture("bundle-revisions-restore");
    let first = capture(&sandbox, &[]);
    assert_eq!(first["delivery_type"], "bundle");
    assert_eq!(first["member_count"], 3);
    let artifact = first["artifact"].as_str().unwrap();
    let revision = first["revision"].as_str().unwrap();
    let reference = format!("artifact:{artifact}@{revision}");
    sandbox.ok(&[
        "decide",
        "Review original delivery",
        "--reason",
        "reviewed complete original",
        "--ref",
        &reference,
    ]);
    fs::write(
        sandbox.0.join("app/assets/style.css"),
        b"body { color: green; }",
    )
    .unwrap();
    let second = capture(&sandbox, &["--artifact", artifact, "--previous", revision]);
    assert_ne!(second["revision"], first["revision"]);
    let decision: Value = serde_json::from_str(&sandbox.ok(&["why", "d2", "--json"])).unwrap();
    assert_eq!(decision["node"]["refs"], json!([reference]));
    let log = fs::read_to_string(sandbox.0.join(".vivac/events")).unwrap();
    assert!(!log.contains("assets/style.css"));
    assert!(!log.contains("body { color:"));
    fs::remove_dir_all(sandbox.0.join("app")).unwrap();
    sandbox.ok(&[
        "artifacts",
        "restore",
        artifact,
        "--revision",
        revision,
        "--to",
        "restored",
        "--json",
    ]);
    assert_eq!(
        fs::read(sandbox.0.join("restored/assets/style.css")).unwrap(),
        b"body { color: navy; }"
    );
    assert!(sandbox.0.join("restored/index.html").is_file());
    assert!(sandbox.0.join("restored/assets/main.js").is_file());
    let manifest: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("restored/.vivac-delivery.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["members"].as_array().unwrap().len(), 3);
    let (_, code) = sandbox.run(&[
        "artifacts",
        "restore",
        artifact,
        "--revision",
        revision,
        "--to",
        "restored",
        "--json",
    ]);
    assert_ne!(code, 0);
    fs::write(sandbox.0.join("occupied"), b"Preserve occupied file").unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                artifact,
                "--revision",
                revision,
                "--to",
                "occupied",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(
        fs::read(sandbox.0.join("occupied")).unwrap(),
        b"Preserve occupied file"
    );
}
#[test]
fn bundle_capture_retry_does_not_reopen_changed_or_deleted_selector() {
    let sandbox = fixture("bundle-idempotent-selector");
    let first = capture(&sandbox, &["--operation", "01bbbbbbbbbbbbbbbbbbbbbbbb"]);
    fs::remove_file(sandbox.0.join("selection.json")).unwrap();
    fs::remove_dir_all(sandbox.0.join("app")).unwrap();
    let second = capture(&sandbox, &["--operation", "01bbbbbbbbbbbbbbbbbbbbbbbb"]);
    assert_eq!(first["revision"], second["revision"]);
    let (_, code) = sandbox.run(&[
        "artifacts",
        "capture",
        "--bundle",
        "another-selection.json",
        "--from",
        "g1",
        "--operation",
        "01bbbbbbbbbbbbbbbbbbbbbbbb",
        "--json",
    ]);
    assert_ne!(code, 0);
}

#[test]
fn file_identity_cannot_change_into_bundle_and_capture_forms_are_exclusive() {
    let sandbox = fixture("bundle-capture-identity-type");
    let file: Value = serde_json::from_str(&sandbox.ok(&[
        "artifacts",
        "capture",
        "app/index.html",
        "--from",
        "g1",
        "--json",
    ]))
    .unwrap();
    let delivery = &file["delivery"];
    let events = sandbox.0.join(".vivac/events");
    let before = fs::read(&events).unwrap();
    for args in [
        vec![
            "artifacts",
            "capture",
            "--bundle",
            "selection.json",
            "--from",
            "g1",
            "--artifact",
            delivery["artifact"].as_str().unwrap(),
            "--previous",
            delivery["revision"].as_str().unwrap(),
            "--json",
        ],
        vec![
            "artifacts",
            "capture",
            "app/index.html",
            "--bundle",
            "selection.json",
            "--from",
            "g1",
            "--json",
        ],
    ] {
        assert_ne!(sandbox.run(&args).1, 0);
        assert_eq!(fs::read(&events).unwrap(), before);
    }
}

#[test]
fn bundle_requirements_are_preserved_as_metadata_without_importing_dependencies() {
    let sandbox = fixture("bundle-requirements-metadata");
    let requirements = json!(["external dependency", "Run only after review"]);
    let selection = json!({"root":"app","entrypoint":"index.html","members":["index.html"],"requirements":requirements});
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selection).unwrap(),
    )
    .unwrap();
    let original_script = fs::read(sandbox.0.join("app/assets/main.js")).unwrap();
    let delivery = capture(&sandbox, &[]);
    assert_eq!(delivery["member_count"], 1);
    let object = sandbox
        .0
        .join(".vivac-artifacts")
        .join(delivery["object"].as_str().unwrap());
    let manifest: Value =
        serde_json::from_slice(&fs::read(object.join(".vivac-delivery.json")).unwrap()).unwrap();
    assert_eq!(manifest["requirements"], requirements);
    assert_eq!(manifest["members"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["members"][0]["path"], "index.html");
    assert!(!object.join("assets").exists());
    assert!(!sandbox.0.join("node_modules").exists());
    assert!(!sandbox.0.join("app/node_modules").exists());
    assert_eq!(
        fs::read(sandbox.0.join("app/assets/main.js")).unwrap(),
        original_script
    );
    let shown: Value = serde_json::from_str(&sandbox.ok(&[
        "artifacts",
        "show",
        delivery["artifact"].as_str().unwrap(),
        "--revision",
        delivery["revision"].as_str().unwrap(),
        "--json",
    ]))
    .unwrap();
    assert_eq!(shown["requirements"], requirements);
    assert_eq!(shown["total_members"], 1);
    assert_eq!(shown["bytes_checked"], false);
    let log = fs::read_to_string(sandbox.0.join(".vivac/events")).unwrap();
    assert!(!log.contains("external dependency"));
}

#[test]
fn bundle_capture_refuses_invalid_selectors_and_every_unsafe_member_without_receipts() {
    let sandbox = fixture("bundle-refused-inputs");
    let events = sandbox.0.join(".vivac/events");
    let before = fs::read(&events).unwrap();
    let valid = fs::read(sandbox.0.join("selection.json")).unwrap();
    for (case, selection) in [
        (
            "unknown selection field",
            json!({"root":"app","entrypoint":"index.html","members":["index.html"],"requirements":[],"unexpected":true}),
        ),
        (
            "member parent traversal",
            json!({"root":"app","entrypoint":"index.html","members":["index.html","../outside.js"],"requirements":[]}),
        ),
        (
            "case-colliding members",
            json!({"root":"app","entrypoint":"index.html","members":["index.html","INDEX.html"],"requirements":[]}),
        ),
        (
            "non-HTML entrypoint",
            json!({"root":"app","entrypoint":"assets/main.js","members":["assets/main.js"],"requirements":[]}),
        ),
        (
            "sensitive requirement",
            json!({"root":"app","entrypoint":"index.html","members":["index.html"],"requirements":["person@example.com"]}),
        ),
    ] {
        fs::write(
            sandbox.0.join("selection.json"),
            serde_json::to_vec(&selection).unwrap(),
        )
        .unwrap();
        let (output, code) = sandbox.run(&[
            "artifacts",
            "capture",
            "--bundle",
            "selection.json",
            "--from",
            "g1",
            "--json",
        ]);
        assert_ne!(code, 0, "selector case {case}: {output}");
        assert_eq!(
            fs::read(&events).unwrap(),
            before,
            "selector case {case} recorded an event"
        );
    }
    for bytes in [b"\xff".as_slice(), b"{malformed selection".as_slice()] {
        fs::write(sandbox.0.join("selection.json"), bytes).unwrap();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--bundle",
                    "selection.json",
                    "--from",
                    "g1",
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(fs::read(&events).unwrap(), before);
    }
    fs::write(sandbox.0.join("selection.json"), valid).unwrap();
    for bytes in [
        b"\xff".as_slice(),
        b"-----BEGIN PRIVATE KEY-----\nSynthetic refused key\n-----END PRIVATE KEY-----".as_slice(),
    ] {
        fs::write(sandbox.0.join("app/assets/main.js"), bytes).unwrap();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--bundle",
                    "selection.json",
                    "--from",
                    "g1",
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(fs::read(&events).unwrap(), before);
    }
}

#[test]
fn bundle_check_checks_members_outside_visible_page_and_detects_unexpected_files() {
    let sandbox = fixture("bundle-check-all-members");
    let delivery = capture(&sandbox, &[]);
    let artifact = delivery["artifact"].as_str().unwrap();
    let revision = delivery["revision"].as_str().unwrap();
    let object = sandbox
        .0
        .join(".vivac-artifacts")
        .join(delivery["object"].as_str().unwrap());
    let shown: Value = serde_json::from_str(&sandbox.ok(&[
        "artifacts",
        "show",
        artifact,
        "--revision",
        revision,
        "--limit",
        "1",
        "--offset",
        "0",
        "--json",
    ]))
    .unwrap();
    assert_eq!(shown["members"].as_array().unwrap().len(), 1);
    assert_eq!(shown["members"][0]["path"], "assets/main.js");
    fs::write(object.join("assets/style.css"), b"body { color: red; }").unwrap();
    let (output, code) = sandbox.run(&[
        "artifacts",
        "check",
        artifact,
        "--revision",
        revision,
        "--json",
    ]);
    assert_ne!(code, 0);
    let result: Value = serde_json::from_str(&output).unwrap();
    assert_ne!(result["retained"]["status"], "matches");
    assert!(result.get("error").is_none());
    fs::write(object.join("assets/style.css"), b"body { color: navy; }").unwrap();
    fs::write(
        object.join("unselected.js"),
        b"document.title = 'Unexpected';",
    )
    .unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "check",
                artifact,
                "--revision",
                revision,
                "--json"
            ])
            .1,
        0
    );
    assert_ne!(
        sandbox.run(&["artifacts", "check", "--store", "--json"]).1,
        0
    );
}

#[test]
fn bundle_capture_member_and_aggregate_limits_do_not_record_partial_revision() {
    let sandbox = fixture("bundle-capture-limits");
    let before = fs::read(sandbox.0.join(".vivac/events")).unwrap();
    let members: Vec<_> = (0..65)
        .map(|index| format!("member-{index}.html"))
        .collect();
    for member in &members {
        fs::write(
            sandbox.0.join("app").join(member),
            b"<html>Selected member</html>",
        )
        .unwrap();
    }
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(
            &json!({"root":"app","entrypoint":"member-0.html","members":members,"requirements":[]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "capture",
                "--bundle",
                "selection.json",
                "--from",
                "g1",
                "--json"
            ])
            .1,
        0
    );
    fs::write(
        sandbox.0.join("app/index.html"),
        vec![b' '; 6 * 1024 * 1024],
    )
    .unwrap();
    fs::write(
        sandbox.0.join("app/assets/style.css"),
        vec![b' '; 5 * 1024 * 1024],
    )
    .unwrap();
    fs::write(sandbox.0.join("selection.json"), serde_json::to_vec(&json!({"root":"app","entrypoint":"index.html","members":["index.html","assets/style.css"],"requirements":[]})).unwrap()).unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "capture",
                "--bundle",
                "selection.json",
                "--from",
                "g1",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(fs::read(sandbox.0.join(".vivac/events")).unwrap(), before);
}

#[test]
fn bundle_with_project_root_selection_preserves_metadata_without_claiming_integrity() {
    let sandbox = fixture("bundle-root-dot");
    fs::write(sandbox.0.join("selection.json"), serde_json::to_vec(&json!({"root":".","entrypoint":"app/index.html","members":["app/index.html","app/assets/style.css","app/assets/main.js"],"requirements":[]})).unwrap()).unwrap();
    let delivery = capture(&sandbox, &[]);
    assert_eq!(delivery["source"], ".");
    let shown: Value = serde_json::from_str(&sandbox.ok(&[
        "artifacts",
        "show",
        delivery["artifact"].as_str().unwrap(),
        "--revision",
        delivery["revision"].as_str().unwrap(),
        "--limit",
        "1",
        "--json",
    ]))
    .unwrap();
    assert_eq!(shown["bytes_checked"], false);
    assert_eq!(shown["total_members"], 3);
    assert_eq!(shown["members"].as_array().unwrap().len(), 1);
    assert_eq!(shown["members_next_offset"], 1);
    assert_eq!(shown["manifest_status"]["members_checked"], false);
}

fn copy_object(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_object(&entry.path(), &target);
        } else {
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn orphan_bundle_recovery_checks_complete_object_without_inventing_provenance() {
    let sandbox = fixture("bundle-orphan-recovery");
    let delivery = capture(&sandbox, &[]);
    let orphan = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    let object = format!("objects/{orphan}.bundle");
    let store = sandbox.0.join(".vivac-artifacts");
    copy_object(
        &store.join(delivery["object"].as_str().unwrap()),
        &store.join(&object),
    );
    let before = fs::read(sandbox.0.join(".vivac/events")).unwrap();
    let (output, code) = sandbox.run(&["artifacts", "check", "--store", "--json"]);
    assert_ne!(code, 0);
    let checked: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(checked["orphans"], json!([object]));
    assert_eq!(
        checked["bundle_objects"][0]["retained"]["status"],
        "matches"
    );
    assert_eq!(checked["bundle_objects"][0]["provenance"], "unknown");
    let restored: Value = serde_json::from_str(&sandbox.ok(&[
        "artifacts",
        "restore",
        "--orphan",
        orphan,
        "--to",
        "rescued",
        "--json",
    ]))
    .unwrap();
    assert_eq!(restored["provenance"], "unknown");
    assert!(restored.get("producer").is_none());
    assert_eq!(
        fs::read(sandbox.0.join("rescued/assets/style.css")).unwrap(),
        b"body { color: navy; }"
    );
    assert!(sandbox.0.join("rescued/.vivac-delivery.json").is_file());
    fs::remove_file(store.join(&object).join(".vivac-delivery.json")).unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                "--orphan",
                orphan,
                "--to",
                "broken-rescue",
                "--json"
            ])
            .1,
        0
    );
    assert!(!sandbox.0.join("broken-rescue").exists());
    assert_eq!(fs::read(sandbox.0.join(".vivac/events")).unwrap(), before);
}

#[test]
fn bundle_history_survives_index_deletion_and_missing_manifest_cannot_restore() {
    let sandbox = fixture("bundle-index-manifest");
    let first = capture(&sandbox, &[]);
    let artifact = first["artifact"].as_str().unwrap();
    let revision = first["revision"].as_str().unwrap();
    fs::write(
        sandbox.0.join("app/assets/style.css"),
        b"body { color: green; }",
    )
    .unwrap();
    capture(&sandbox, &["--artifact", artifact, "--previous", revision]);
    let args = [
        "artifacts",
        "show",
        artifact,
        "--revision",
        revision,
        "--json",
    ];
    let before: Value = serde_json::from_str(&sandbox.ok(&args)).unwrap();
    sandbox.ok(&["why", "g1", "--json"]);
    let index = sandbox.0.join(".vivac/index");
    assert!(
        index.exists(),
        "normal reading must create the disposable index fixture"
    );
    fs::remove_file(&index).unwrap();
    let after: Value = serde_json::from_str(&sandbox.ok(&args)).unwrap();
    assert_eq!(after, before);
    let object = sandbox
        .0
        .join(".vivac-artifacts")
        .join(first["object"].as_str().unwrap());
    fs::remove_file(object.join(".vivac-delivery.json")).unwrap();
    let damaged: Value = serde_json::from_str(&sandbox.ok(&args)).unwrap();
    assert_eq!(damaged["manifest_status"]["status"], "unavailable");
    assert_eq!(damaged["revisions"][0]["entrypoint"], "index.html");
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "check",
                artifact,
                "--revision",
                revision,
                "--json"
            ])
            .1,
        0
    );
    assert_ne!(
        sandbox.run(&["artifacts", "check", "--store", "--json"]).1,
        0
    );
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                artifact,
                "--revision",
                revision,
                "--to",
                "incomplete",
                "--json"
            ])
            .1,
        0
    );
    assert!(!sandbox.0.join("incomplete").exists());
}

#[test]
fn bundle_restore_refuses_an_occupied_directory_link_and_preserves_its_target() {
    let sandbox = fixture("bundle-occupied-link");
    let delivery = capture(&sandbox, &[]);
    let target = sandbox.0.join("target");
    let destination = sandbox.0.join("linked");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("preserved.txt"), b"Preserve linked target").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &destination).unwrap();
    #[cfg(windows)]
    {
        let created = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&destination)
            .arg(&target)
            .output()
            .unwrap();
        assert!(created.status.success(), "junction fixture creation failed");
    }
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                delivery["artifact"].as_str().unwrap(),
                "--revision",
                delivery["revision"].as_str().unwrap(),
                "--to",
                "linked",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(
        fs::read(target.join("preserved.txt")).unwrap(),
        b"Preserve linked target"
    );
    assert!(!target.join("index.html").exists());
}

#[test]
fn unknown_bundle_capability_event_is_refused_without_rewriting_the_log() {
    let sandbox = fixture("bundle-newer-reader");
    capture(&sandbox, &[]);
    let events = sandbox.0.join(".vivac/events");
    let future = fs::read_to_string(&events).unwrap().replace(
        "artifact.bundle_captured",
        "artifact.bundle_captured.future",
    );
    fs::write(&events, &future).unwrap();
    assert_ne!(sandbox.run(&["artifacts", "list", "--all", "--json"]).1, 0);
    assert_eq!(fs::read_to_string(&events).unwrap(), future);
}
