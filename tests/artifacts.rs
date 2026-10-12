mod common;

use common::Sandbox;
use serde_json::Value;
use std::fs;

const ABSENT_REVISION: &str = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const OPERATION: &str = "01bbbbbbbbbbbbbbbbbbbbbbbb";

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

fn seeded(name: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    let root = sandbox.0.canonicalize().unwrap();
    assert_isolated(&root);
    sandbox.ok(&["init", "--yes"]);
    sandbox.ok(&[
        "push",
        "Review the selected delivery",
        "--why",
        "retain the reviewed revision",
    ]);
    sandbox
}

#[test]
fn an_ancestral_tree_is_rejected_before_fixture_initialization() {
    let sandbox = Sandbox::new_empty("artifact-ancestry-guard");
    let root = sandbox.0.canonicalize().unwrap();
    assert_isolated(&root);
    let marker = root.join(".vivac").join("config");
    fs::create_dir(root.join(".vivac")).unwrap();
    fs::write(&marker, b"Synthetic ancestry marker").unwrap();
    let child = root.join("nested");
    fs::create_dir(&child).unwrap();
    assert!(std::panic::catch_unwind(|| assert_isolated(&child)).is_err());
    assert!(!child.join(".vivac").exists());
    assert_eq!(fs::read(marker).unwrap(), b"Synthetic ancestry marker");
}

fn value(sandbox: &Sandbox, args: &[&str]) -> Value {
    serde_json::from_str(&sandbox.ok(args)).unwrap()
}

fn capture(sandbox: &Sandbox, path: &str) -> Value {
    value(
        sandbox,
        &["artifacts", "capture", path, "--from", "g1", "--json"],
    )["delivery"]
        .clone()
}

#[test]
fn revisions_survive_source_loss_and_decisions_keep_the_reviewed_reference() {
    let sandbox = seeded("artifact-revisions");
    let first = b"<html><h1>First selected delivery</h1></html>\n";
    let second = b"<html><h1>Second selected delivery</h1></html>\n";
    fs::write(sandbox.0.join("design.html"), first).unwrap();
    let delivery = capture(&sandbox, "design.html");
    let artifact = delivery["artifact"].as_str().unwrap();
    let revision = delivery["revision"].as_str().unwrap();
    let reference = delivery["reference"].as_str().unwrap();
    assert_eq!(reference, format!("artifact:{artifact}@{revision}"));
    assert!(delivery["decision_references"]
        .as_array()
        .unwrap()
        .is_empty());
    sandbox.ok(&[
        "decide",
        "Accept the first layout",
        "--reason",
        "the person reviewed this revision",
        "--ref",
        reference,
    ]);
    let decision = value(&sandbox, &["why", "d2", "--json"]);
    assert_eq!(decision["node"]["refs"], serde_json::json!([reference]));
    fs::write(sandbox.0.join("design.html"), second).unwrap();
    let next = value(
        &sandbox,
        &[
            "artifacts",
            "capture",
            "design.html",
            "--artifact",
            artifact,
            "--from",
            "g1",
            "--json",
        ],
    );
    assert!(!sandbox.log().contains("First selected delivery"));
    assert!(!sandbox.log().contains("Second selected delivery"));
    let latest = &next["delivery"];
    assert_eq!(latest["artifact"], artifact);
    assert_ne!(latest["revision"], revision);
    assert!(latest["decision_references"].as_array().unwrap().is_empty());
    assert_eq!(latest["decision_references_total"], 0);
    let saved = value(
        &sandbox,
        &[
            "artifacts",
            "show",
            artifact,
            "--revision",
            revision,
            "--json",
        ],
    );
    assert_eq!(saved["revisions"][0]["reference"], reference);
    assert_eq!(
        saved["revisions"][0]["decision_references"][0]["alias"],
        "d2"
    );
    assert_eq!(saved["revisions"][0]["digest"], delivery["digest"]);
    let listed = value(&sandbox, &["artifacts", "list", "--all", "--json"]);
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(listed["artifacts"][0]["revision"], latest["revision"]);
    fs::remove_file(sandbox.0.join("design.html")).unwrap();
    let (checked, code) = sandbox.run(&[
        "artifacts",
        "check",
        artifact,
        "--revision",
        revision,
        "--json",
    ]);
    assert_eq!(code, 1, "{checked}");
    let checked: Value = serde_json::from_str(&checked).unwrap();
    assert_eq!(checked["source"]["status"], "missing");
    assert_eq!(checked["retained"]["status"], "matches");
    assert_eq!(checked["approval"], "not_inferred");
    let log = sandbox.log();
    sandbox.ok(&[
        "artifacts",
        "restore",
        artifact,
        "--revision",
        revision,
        "--to",
        "recovered.html",
        "--json",
    ]);
    assert_eq!(fs::read(sandbox.0.join("recovered.html")).unwrap(), first);
    let (_, code) = sandbox.run(&[
        "artifacts",
        "restore",
        artifact,
        "--to",
        "recovered.html",
        "--json",
    ]);
    assert_ne!(code, 0);
    assert_eq!(fs::read(sandbox.0.join("recovered.html")).unwrap(), first);
    assert_eq!(sandbox.log(), log);
    let unknown = format!("artifact:{artifact}@{ABSENT_REVISION}");
    assert_ne!(
        sandbox
            .run(&[
                "decide",
                "Use an unknown revision",
                "--reason",
                "test an unresolved reference",
                "--ref",
                &unknown
            ])
            .1,
        0
    );
    assert_eq!(sandbox.log(), log);
}

#[test]
fn pagination_returns_every_artifact_once_and_reads_do_not_infer_checks() {
    let sandbox = seeded("artifact-pages");
    let mut expected = Vec::new();
    for name in ["first.html", "second.html", "third.html"] {
        fs::write(sandbox.0.join(name), b"Selected local delivery\n").unwrap();
        expected.push(
            capture(&sandbox, name)["artifact"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let mut actual = Vec::new();
    let mut offset = 0;
    loop {
        let page = value(
            &sandbox,
            &[
                "artifacts",
                "list",
                "--all",
                "--limit",
                "1",
                "--offset",
                &offset.to_string(),
                "--json",
            ],
        );
        assert_eq!(page["total"], 3);
        assert_eq!(page["bytes_checked"], false);
        let entries = page["artifacts"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["retained_check"], "unverified");
        actual.push(entries[0]["artifact"].as_str().unwrap().to_string());
        match page["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    assert_ne!(
        sandbox
            .run(&["artifacts", "list", "--all", "--limit", "101", "--json"])
            .1,
        0
    );
}

#[test]
fn a_retried_operation_records_one_delivery_without_rereading_changed_source() {
    let sandbox = seeded("artifact-retry");
    fs::write(sandbox.0.join("design.html"), b"Reviewed content\n").unwrap();
    let args = [
        "artifacts",
        "capture",
        "design.html",
        "--from",
        "g1",
        "--operation",
        OPERATION,
        "--json",
    ];
    let first = value(&sandbox, &args);
    let log = sandbox.log();
    fs::write(sandbox.0.join("design.html"), b"Unrelated later content\n").unwrap();
    let retried = value(&sandbox, &args);
    assert_eq!(retried["already_recorded"], true);
    assert_eq!(retried["delivery"], first["delivery"]);
    assert_eq!(sandbox.log(), log);
    fs::write(sandbox.0.join("other.html"), b"Other delivery\n").unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "capture",
                "other.html",
                "--from",
                "g1",
                "--operation",
                OPERATION,
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(sandbox.log(), log);
}

#[test]
fn unsafe_or_oversized_bodies_never_enter_the_log_or_a_delivery() {
    let sandbox = seeded("artifact-privacy");
    let log = sandbox.log();
    for body in [
        b"Private fixture marker ghp_16C7e42F292c6912E7710c838347Ae178B4a".as_slice(),
        b"Private fixture marker person@example.com",
        b"Private fixture marker \xff",
    ] {
        fs::write(sandbox.0.join("unsafe.html"), body).unwrap();
        let (reply, code) = sandbox.run(&[
            "artifacts",
            "capture",
            "unsafe.html",
            "--from",
            "g1",
            "--json",
        ]);
        assert_ne!(code, 0, "{reply}");
        assert!(!reply.contains("Private fixture marker"));
        assert!(!reply.contains("ghp_"));
        assert!(!reply.contains("person@example.com"));
        assert_eq!(sandbox.log(), log);
    }
    fs::write(
        sandbox.0.join("large.html"),
        vec![b' '; 10 * 1024 * 1024 + 1],
    )
    .unwrap();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "capture",
                "large.html",
                "--from",
                "g1",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(sandbox.log(), log);
    assert!(
        value(&sandbox, &["artifacts", "list", "--all", "--json"])["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let body = vec![b' '; 10 * 1024 * 1024];
    fs::write(sandbox.0.join("limit.html"), &body).unwrap();
    let accepted = capture(&sandbox, "limit.html");
    assert_eq!(accepted["size"], body.len() as u64);
    assert!(!sandbox.log().contains("Private fixture marker"));
}

#[test]
fn every_node_write_rejects_unknown_revisions_before_append_and_accepts_saved_references() {
    for command in ["push", "add", "decide"] {
        let sandbox = seeded(&format!("artifact-reference-{command}"));
        fs::write(sandbox.0.join("design.html"), b"First reviewed layout\n").unwrap();
        let saved = capture(&sandbox, "design.html");
        let artifact = saved["artifact"].as_str().unwrap();
        let reference = saved["reference"].as_str().unwrap();
        fs::write(sandbox.0.join("design.html"), b"Later layout\n").unwrap();
        value(
            &sandbox,
            &[
                "artifacts",
                "capture",
                "design.html",
                "--artifact",
                artifact,
                "--from",
                "g1",
                "--json",
            ],
        );
        let log = sandbox.log();
        let unknown = format!("artifact:{artifact}@{ABSENT_REVISION}");
        let reason = if command == "decide" {
            "--reason"
        } else {
            "--why"
        };
        let (reply, code) = sandbox.run(&[
            command,
            "Review the retained layout",
            reason,
            "the reviewed revision determines the work",
            "--ref",
            &unknown,
        ]);
        assert_ne!(
            code, 0,
            "{command} accepted an unresolved artifact revision: {reply}"
        );
        assert_eq!(
            sandbox.log(),
            log,
            "{command} appended before rejecting the reference"
        );
        sandbox.ok(&[
            command,
            "Review the retained layout",
            reason,
            "the reviewed revision determines the work",
            "--ref",
            reference,
        ]);
        assert_ne!(sandbox.log(), log);
    }
}

#[test]
fn store_check_reports_surviving_bytes_without_attributing_objects_from_an_incomplete_log() {
    use std::io::Write;

    let sandbox = seeded("artifact-incomplete-log");
    let body = b"Selected layout survives a damaged log\n";
    fs::write(sandbox.0.join("design.html"), body).unwrap();
    let saved = capture(&sandbox, "design.html");
    let store = sandbox.0.join(".vivac-artifacts");
    let retained = store.join(saved["object"].as_str().unwrap());
    let unknown = format!("objects/{ABSENT_REVISION}.blob");
    fs::copy(&retained, store.join(&unknown)).unwrap();
    let events = sandbox.0.join(".vivac").join("events");
    fs::OpenOptions::new()
        .append(true)
        .open(&events)
        .unwrap()
        .write_all(b"{\"payload\":")
        .unwrap();
    let log = fs::read(&events).unwrap();
    let config_path = sandbox.0.join(".vivac").join("config");
    let config = fs::read(&config_path).unwrap();
    let (reply, code) = sandbox.run(&["artifacts", "check", "--store", "--json"]);
    assert_eq!(code, 1, "{reply}");
    let checked: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(checked["log_integrity"], "incomplete");
    assert_eq!(checked["provenance_complete"], false);
    assert_eq!(checked["broken_lines"], 1);
    let receipts = checked["recorded"].as_array().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["revision"], saved["revision"]);
    assert_eq!(receipts[0]["retained"]["status"], "matches");
    assert_eq!(checked["unattributed"], serde_json::json!([unknown]));
    assert_eq!(checked["orphans"], serde_json::json!([]));
    assert_eq!(fs::read(&retained).unwrap(), body);
    assert_eq!(fs::read(store.join(&unknown)).unwrap(), body);
    assert_eq!(fs::read(&events).unwrap(), log);
    assert_eq!(fs::read(&config_path).unwrap(), config);
}

#[test]
fn operation_retry_requires_the_original_explicit_previous_revision() {
    let sandbox = seeded("artifact-retry-previous");
    fs::write(sandbox.0.join("design.html"), b"First selected layout\n").unwrap();
    let first = capture(&sandbox, "design.html");
    let artifact = first["artifact"].as_str().unwrap();
    let previous = first["revision"].as_str().unwrap();
    fs::write(sandbox.0.join("design.html"), b"Second selected layout\n").unwrap();
    let args = [
        "artifacts",
        "capture",
        "design.html",
        "--from",
        "g1",
        "--artifact",
        artifact,
        "--previous",
        previous,
        "--operation",
        OPERATION,
        "--json",
    ];
    let second = value(&sandbox, &args);
    let log = sandbox.log();
    fs::remove_file(sandbox.0.join("design.html")).unwrap();
    let retry = value(&sandbox, &args);
    assert_eq!(retry["already_recorded"], true);
    assert_eq!(retry["delivery"], second["delivery"]);
    let (_, code) = sandbox.run(&[
        "artifacts",
        "capture",
        "design.html",
        "--from",
        "g1",
        "--artifact",
        artifact,
        "--previous",
        ABSENT_REVISION,
        "--operation",
        OPERATION,
        "--json",
    ]);
    assert_ne!(code, 0, "retry accepted a different previous revision");
    assert_eq!(sandbox.log(), log);
}

#[test]
fn incomplete_log_allows_only_explicit_unknown_object_rescue() {
    use std::io::Write;
    let sandbox = seeded("artifact-incomplete-rescue");
    let body = b"Selected layout for explicit rescue\n";
    fs::write(sandbox.0.join("design.html"), body).unwrap();
    let saved = capture(&sandbox, "design.html");
    let revision = saved["revision"].as_str().unwrap();
    let artifact = saved["artifact"].as_str().unwrap();
    let objects = sandbox.0.join(".vivac-artifacts").join("objects");
    fs::copy(
        objects.join(format!("{revision}.blob")),
        objects.join(format!("{ABSENT_REVISION}.blob")),
    )
    .unwrap();
    let events = sandbox.0.join(".vivac").join("events");
    let original = fs::read_to_string(&events).unwrap();
    let mut duplicate: Value = original
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|event| event["payload"]["type"] == "artifact.captured")
        .unwrap();
    duplicate["seq"] = serde_json::json!(duplicate["seq"].as_u64().unwrap() + 1);
    duplicate["id"] = serde_json::json!("01dddddddddddddddddddddddd");
    let mut log = fs::OpenOptions::new().append(true).open(&events).unwrap();
    writeln!(log, "{duplicate}").unwrap();
    log.write_all(b"{\"payload\":").unwrap();
    drop(log);
    let before = fs::read(&events).unwrap();
    let config_path = sandbox.0.join(".vivac").join("config");
    let config = fs::read(&config_path).unwrap();
    let recovered = value(
        &sandbox,
        &[
            "artifacts",
            "restore",
            "--orphan",
            ABSENT_REVISION,
            "--to",
            "unknown.html",
            "--json",
        ],
    );
    assert_eq!(recovered["provenance"], "unknown");
    assert_eq!(recovered["event_recorded"], false);
    assert!(recovered.get("producer").is_none());
    assert!(recovered.get("approval").is_none());
    assert_eq!(fs::read(sandbox.0.join("unknown.html")).unwrap(), body);
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                "--orphan",
                revision,
                "--to",
                "registered.html",
                "--json"
            ])
            .1,
        0
    );
    assert!(!sandbox.0.join("registered.html").exists());
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                artifact,
                "--to",
                "normal.html",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(fs::read(&events).unwrap(), before);
    assert_eq!(fs::read(&config_path).unwrap(), config);
}
