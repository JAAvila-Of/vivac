mod common;

use common::Sandbox;
use serde_json::{json, Value};
use std::fs;

const OPERATION: &str = "01bbbbbbbbbbbbbbbbbbbbbbbb";
const SENSITIVE: &[u8] = b"<html><p>person@example.com</p></html>";
const DESCRIPTION: &str = "Selected fixture layout";

fn seeded(name: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    let root = sandbox.0.canonicalize().unwrap();
    assert!(!root.starts_with(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))));
    assert!(!root.ancestors().skip(1).any(|parent| {
        ["config", "events", "lane"]
            .into_iter()
            .any(|name| parent.join(".vivac").join(name).exists())
    }));
    sandbox.ok(&["init", "--yes"]);
    sandbox.ok(&[
        "push",
        "Review selected delivery",
        "--why",
        "Retain selected fixture",
    ]);
    sandbox
}

fn value(sandbox: &Sandbox, args: &[&str]) -> Value {
    serde_json::from_str(&sandbox.ok(args)).unwrap()
}

fn policy(sandbox: &Sandbox, setting: &str) -> Value {
    value(
        sandbox,
        &[
            "artifacts",
            "policy",
            &format!("--allow-insecure-artifacts={setting}"),
            "--json",
        ],
    )
}

fn capture_args<'a>(path: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "artifacts",
        "capture",
        path,
        "--from",
        "g1",
        "--description",
        DESCRIPTION,
        "--json",
    ];
    args.extend_from_slice(extra);
    args
}

fn assert_detected(inspection: &Value) {
    assert_eq!(
        inspection,
        &json!({
            "policy": "allow_insecure",
            "sensitivity": "detected",
            "coverage": "complete",
            "review_required": true,
        })
    );
}

fn assert_no_objects(sandbox: &Sandbox) {
    let objects = sandbox.0.join(".vivac-artifacts/objects");
    if objects.exists() {
        assert_eq!(fs::read_dir(objects).unwrap().count(), 0);
    }
}

#[test]
fn policy_is_strict_by_default_and_explicit_changes_do_not_append_events() {
    let sandbox = seeded("artifact-policy-settings");
    let path = sandbox.0.join(".vivac/config");
    let before = fs::read(&path).unwrap();
    let events = sandbox.log();
    assert_eq!(
        value(&sandbox, &["artifacts", "policy", "--json"])["allow_insecure_artifacts"],
        false
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let original: Value = serde_json::from_slice(&before).unwrap();
    for (setting, expected) in [("true", true), ("false", false), ("1", true), ("0", false)] {
        let response = policy(&sandbox, setting);
        assert_eq!(response["allow_insecure_artifacts"], expected);
        assert_eq!(response["event_recorded"], false);
        let config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["allow_insecure_artifacts"], expected);
        for key in ["version", "project_id", "actor", "share"] {
            assert_eq!(config[key], original[key]);
        }
        assert_eq!(sandbox.log(), events);
    }
    for setting in ["2", "-1", "1.0", "yes", "null"] {
        let before = fs::read(&path).unwrap();
        let (_, code) = sandbox.run(&[
            "artifacts",
            "policy",
            &format!("--allow-insecure-artifacts={setting}"),
            "--json",
        ]);
        assert_ne!(code, 0);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(sandbox.log(), events);
    }
}

#[test]
fn config_consent_accepts_boolean_binary_and_null_without_read_time_mutation() {
    let sandbox = seeded("artifact-policy-config-values");
    let path = sandbox.0.join(".vivac/config");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (input, expected) in [
        (Value::Null, false),
        (json!(false), false),
        (json!(0), false),
        (json!(true), true),
        (json!(1), true),
    ] {
        let mut config = original.clone();
        config["allow_insecure_artifacts"] = input;
        let bytes = serde_json::to_vec(&config).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            value(&sandbox, &["artifacts", "policy", "--json"])["allow_insecure_artifacts"],
            expected
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    for input in [
        json!(2),
        json!(-1),
        json!(1.0),
        json!("private-input-sentinel"),
        json!([]),
        json!({"private-input-sentinel":true}),
    ] {
        let mut config = original.clone();
        config["allow_insecure_artifacts"] = input;
        let bytes = serde_json::to_vec(&config).unwrap();
        fs::write(&path, &bytes).unwrap();
        let (output, code) = sandbox.run(&["artifacts", "policy", "--json"]);
        assert_ne!(code, 0);
        assert!(!output.contains("private-input-sentinel"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn explicit_sensitive_custody_preserves_bytes_and_reports_safe_inspection_metadata() {
    let sandbox = seeded("artifact-policy-sensitive");
    fs::write(sandbox.0.join("design.html"), SENSITIVE).unwrap();
    let before = sandbox.log();
    assert_ne!(sandbox.run(&capture_args("design.html", &[])).1, 0);
    assert_eq!(sandbox.log(), before);
    assert_no_objects(&sandbox);
    policy(&sandbox, "true");
    let captured = value(&sandbox, &capture_args("design.html", &[]));
    let delivery = &captured["delivery"];
    assert_detected(&delivery["inspection"]);
    let artifact = delivery["artifact"].as_str().unwrap();
    let revision = delivery["revision"].as_str().unwrap();
    assert!(sandbox.log().contains("artifact.captured_v3"));
    assert!(!sandbox.log().contains("person@example.com"));
    let shown = value(&sandbox, &["artifacts", "show", artifact, "--json"]);
    assert_detected(&shown["revisions"][0]["inspection"]);
    let listed = value(&sandbox, &["artifacts", "list", "--all", "--json"]);
    assert_detected(&listed["artifacts"][0]["inspection"]);
    policy(&sandbox, "false");
    let checked = value(
        &sandbox,
        &[
            "artifacts",
            "check",
            artifact,
            "--revision",
            revision,
            "--json",
        ],
    );
    assert_eq!(checked["retained"]["status"], "matches");
    assert_eq!(checked["source"]["status"], "matches");
    assert_detected(&checked["inspection"]);
    for metadata in [&shown, &listed, &checked] {
        assert!(!metadata.to_string().contains("person@example.com"));
    }
    value(
        &sandbox,
        &[
            "artifacts",
            "restore",
            artifact,
            "--revision",
            revision,
            "--to",
            "restored.html",
            "--json",
        ],
    );
    assert_eq!(
        fs::read(sandbox.0.join("restored.html")).unwrap(),
        SENSITIVE
    );
    let before = sandbox.log();
    assert_ne!(sandbox.run(&capture_args("design.html", &[])).1, 0);
    assert_eq!(sandbox.log(), before);
}

#[test]
fn operation_retries_use_existing_consent_and_new_revisions_require_current_consent() {
    let sandbox = seeded("artifact-policy-revisions");
    fs::write(sandbox.0.join("design.html"), SENSITIVE).unwrap();
    policy(&sandbox, "1");
    let first = value(
        &sandbox,
        &capture_args("design.html", &["--operation", OPERATION]),
    );
    let artifact = first["delivery"]["artifact"].as_str().unwrap();
    let revision = first["delivery"]["revision"].as_str().unwrap();
    policy(&sandbox, "0");
    fs::remove_file(sandbox.0.join("design.html")).unwrap();
    let before = sandbox.log();
    let retry = value(
        &sandbox,
        &capture_args("design.html", &["--operation", OPERATION]),
    );
    assert_eq!(retry["already_recorded"], true);
    assert_eq!(retry["delivery"], first["delivery"]);
    assert_eq!(sandbox.log(), before);
    fs::write(
        sandbox.0.join("design.html"),
        b"<html>person@example.com changed</html>",
    )
    .unwrap();
    let continuation = capture_args(
        "design.html",
        &["--artifact", artifact, "--previous", revision],
    );
    assert_ne!(sandbox.run(&continuation).1, 0);
    assert_eq!(sandbox.log(), before);
    policy(&sandbox, "true");
    let next = value(&sandbox, &continuation);
    assert_eq!(next["delivery"]["artifact"], artifact);
    assert_eq!(next["delivery"]["previous"], revision);
    assert_ne!(next["delivery"]["revision"], revision);
    assert_detected(&next["delivery"]["inspection"]);
}

#[test]
fn permission_does_not_relax_metadata_or_structural_guards() {
    let sandbox = seeded("artifact-policy-guards");
    policy(&sandbox, "true");
    fs::write(
        sandbox.0.join("design.html"),
        b"<html>Selected fixture</html>",
    )
    .unwrap();
    fs::write(
        sandbox.0.join("person@example.com.html"),
        b"<html>Selected fixture</html>",
    )
    .unwrap();
    fs::write(sandbox.0.join("invalid.html"), [0xff, 0xfe]).unwrap();
    fs::write(sandbox.0.join("token.html"), vec![b'x'; 64 * 1024 + 1]).unwrap();
    let before = sandbox.log();
    let mut description = capture_args("design.html", &[]);
    let position = description
        .iter()
        .position(|arg| *arg == DESCRIPTION)
        .unwrap();
    description[position] = "person@example.com";
    for args in [
        description,
        capture_args("person@example.com.html", &[]),
        capture_args("invalid.html", &[]),
        capture_args("token.html", &[]),
    ] {
        let (output, code) = sandbox.run(&args);
        assert_ne!(code, 0);
        assert!(!output.contains("person@example.com"));
        assert_eq!(sandbox.log(), before);
        assert_no_objects(&sandbox);
    }
}

fn bundle_args() -> Vec<&'static str> {
    vec![
        "artifacts",
        "capture",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--description",
        DESCRIPTION,
        "--json",
    ]
}

fn selection(sandbox: &Sandbox, selected: &Value) {
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(selected).unwrap(),
    )
    .unwrap();
}

fn retained_bundle(sandbox: &Sandbox, delivery: &Value) -> std::path::PathBuf {
    sandbox
        .0
        .join(".vivac-artifacts")
        .join(delivery["object"].as_str().unwrap())
}

fn assert_restored_members(sandbox: &Sandbox, delivery: &Value, destination: &str) {
    let retained = retained_bundle(sandbox, delivery);
    let manifest: Value =
        serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap()).unwrap();
    for member in manifest["members"].as_array().unwrap() {
        let path = member["path"].as_str().unwrap();
        assert_eq!(
            fs::read(sandbox.0.join(destination).join(path)).unwrap(),
            fs::read(retained.join(path)).unwrap(),
            "restored member must match retained bytes: {path}"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn permissive_png_without_review_records_partial_coverage_and_restores_after_disabling() {
    for sensitive_metadata in [false, true] {
        let sandbox = seeded("artifact-policy-png");
        fs::create_dir(sandbox.0.join("app")).unwrap();
        fs::write(
            sandbox.0.join("app/index.html"),
            b"<!doctype html><img src=image.png>",
        )
        .unwrap();
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            if sensitive_metadata {
                encoder
                    .add_text_chunk("Contact".into(), "person@example.com".into())
                    .unwrap();
            }
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 0, 0, 255]).unwrap();
            writer.finish().unwrap();
        }
        fs::write(sandbox.0.join("app/image.png"), &bytes).unwrap();
        let selected = json!({"root":"app", "entrypoint":"index.html", "members":["index.html", "image.png"], "requirements":[]});
        selection(&sandbox, &selected);
        let before = sandbox.log();
        assert_ne!(sandbox.run(&bundle_args()).1, 0);
        assert_eq!(sandbox.log(), before);
        assert_no_objects(&sandbox);
        policy(&sandbox, "true");
        let captured = value(&sandbox, &bundle_args());
        let delivery = &captured["delivery"];
        let inspection = json!({"policy":"allow_insecure", "sensitivity":if sensitive_metadata {"detected"} else {"unknown"}, "coverage":"partial", "review_required":true});
        assert_eq!(delivery["inspection"], inspection);
        let retained = retained_bundle(&sandbox, delivery);
        let manifest: Value =
            serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema_version"], 6);
        assert_eq!(manifest["inspection"], inspection);
        assert_eq!(fs::read(retained.join("image.png")).unwrap(), bytes);
        let mut wrong_review = selected.clone();
        wrong_review["media_review"] = json!({"profile":"reviewed-media-v1", "members":[{"path":"image.png", "sha256":"0".repeat(64)}]});
        selection(&sandbox, &wrong_review);
        let before = sandbox.log();
        assert_ne!(sandbox.run(&bundle_args()).1, 0);
        assert_eq!(sandbox.log(), before);
        selection(&sandbox, &selected);
        policy(&sandbox, "false");
        let artifact = delivery["artifact"].as_str().unwrap();
        let checked = value(&sandbox, &["artifacts", "check", artifact, "--json"]);
        assert_eq!(checked["retained"]["status"], "matches");
        assert_eq!(checked["source"]["status"], "matches");
        assert_eq!(checked["inspection"], inspection);
        value(
            &sandbox,
            &[
                "artifacts",
                "restore",
                artifact,
                "--revision",
                delivery["revision"].as_str().unwrap(),
                "--to",
                "restored",
                "--json",
            ],
        );
        assert_restored_members(&sandbox, delivery, "restored");
        assert!(!sandbox.log().contains("person@example.com"));
        assert!(!checked.to_string().contains("person@example.com"));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn database(path: &std::path::Path, sql: &str) {
    use libsqlite3_sys as ffi;
    use std::ffi::CString;
    use std::ptr;
    static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = FIXTURES.lock().unwrap();
    let name = CString::new(path.to_str().unwrap()).unwrap();
    let sql = CString::new(sql).unwrap();
    unsafe {
        let mut db = ptr::null_mut();
        assert_eq!(ffi::sqlite3_open(name.as_ptr(), &mut db), ffi::SQLITE_OK);
        let result = ffi::sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut());
        assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
        assert_eq!(result, ffi::SQLITE_OK);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn permissive_sqlite_retains_sensitive_defaults_and_rows_with_complete_inspection() {
    let sandbox = seeded("artifact-policy-sqlite");
    fs::create_dir(sandbox.0.join("app")).unwrap();
    fs::write(
        sandbox.0.join("app/index.html"),
        b"<!doctype html><script src=main.js></script>",
    )
    .unwrap();
    fs::write(
        sandbox.0.join("app/main.js"),
        b"document.title='Selected database';",
    )
    .unwrap();
    database(&sandbox.0.join("app/data.sqlite"), "CREATE TABLE items(id INTEGER PRIMARY KEY,label TEXT DEFAULT 'person@example.com'); INSERT INTO items(id) VALUES(1);");
    let capabilities = value(&sandbox, &["artifacts", "capabilities", "--json"]);
    let runtime = &capabilities["delivery_types"]["sqlite_application"]["runtime"]
        ["available_for_capture"][0];
    assert!(runtime.as_str().is_some());
    selection(
        &sandbox,
        &json!({"root":"app", "entrypoint":"index.html", "members":["index.html","main.js","data.sqlite"], "requirements":[], "database":"data.sqlite", "runtime":runtime}),
    );
    let before = sandbox.log();
    assert_ne!(sandbox.run(&bundle_args()).1, 0);
    assert_eq!(sandbox.log(), before);
    assert_no_objects(&sandbox);
    policy(&sandbox, "true");
    let captured = value(&sandbox, &bundle_args());
    let delivery = &captured["delivery"];
    assert_detected(&delivery["inspection"]);
    let retained = retained_bundle(&sandbox, delivery);
    let manifest: Value =
        serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 7);
    assert_detected(&manifest["inspection"]);
    let verify = "CREATE TEMP TABLE verified(value INTEGER CHECK(value=1)); INSERT INTO verified SELECT count(*) FROM items WHERE label='person@example.com'; INSERT INTO verified SELECT count(*) FROM sqlite_schema WHERE sql LIKE '%person@example.com%';";
    database(&retained.join("data.sqlite"), verify);
    policy(&sandbox, "false");
    let artifact = delivery["artifact"].as_str().unwrap();
    let checked = value(&sandbox, &["artifacts", "check", artifact, "--json"]);
    assert_eq!(checked["retained"]["status"], "matches");
    assert_eq!(checked["source"]["status"], "matches");
    assert_detected(&checked["inspection"]);
    database(
        &sandbox.0.join("app/data.sqlite"),
        "INSERT INTO items VALUES(2,'Changed source');",
    );
    let (output, code) = sandbox.run(&["artifacts", "check", artifact, "--json"]);
    assert_eq!(code, 1);
    let changed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(changed["retained"]["status"], "matches");
    assert_eq!(changed["source"]["status"], "changed");
    assert_detected(&changed["inspection"]);
    value(
        &sandbox,
        &[
            "artifacts",
            "restore",
            artifact,
            "--revision",
            delivery["revision"].as_str().unwrap(),
            "--to",
            "restored",
            "--json",
        ],
    );
    assert_restored_members(&sandbox, delivery, "restored");
    database(&sandbox.0.join("restored/data.sqlite"), verify);
    assert!(!sandbox.log().contains("person@example.com"));
    assert!(!checked.to_string().contains("person@example.com"));
    assert!(!changed.to_string().contains("person@example.com"));
}
