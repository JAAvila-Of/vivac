#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

mod common;
use common::Sandbox;
use libsqlite3_sys as ffi;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{ffi::CString, fs, path::Path, ptr, sync::Mutex};

fn database(path: &Path, sql: &str) {
    static FIXTURES: Mutex<()> = Mutex::new(());
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

fn fixture(name: &str, sql: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    assert!(!sandbox
        .0
        .canonicalize()
        .unwrap()
        .starts_with(crate_root.parent().unwrap()));
    assert!(!sandbox
        .0
        .ancestors()
        .skip(1)
        .any(|parent| ["config", "events", "lane"]
            .into_iter()
            .any(|name| parent.join(".vivac").join(name).exists())));
    sandbox.ok(&["init", "--yes"]);
    sandbox.ok(&[
        "push",
        "Review selected database application",
        "--why",
        "retain a complete revision",
    ]);
    fs::create_dir(sandbox.0.join("app")).unwrap();
    fs::write(
        sandbox.0.join("app/index.html"),
        b"<!doctype html><script src=main.js></script>",
    )
    .unwrap();
    fs::write(
        sandbox.0.join("app/main.js"),
        b"document.title = 'Database delivery';",
    )
    .unwrap();
    database(&sandbox.0.join("app/data.sqlite"), sql);
    selection(&sandbox, Some("data.sqlite"));
    sandbox
}

fn selection(sandbox: &Sandbox, selected: Option<&str>) {
    let mut value = json!({"root":"app","entrypoint":"index.html","members":["index.html","main.js","data.sqlite"],"requirements":[]});
    if let Some(path) = selected {
        value["database"] = json!(path);
        value["runtime"] = json!(PRODUCT_RUNTIME_ID);
    }
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

fn capture(sandbox: &Sandbox, extra: &[&str]) -> Value {
    let mut args = vec![
        "artifacts",
        "capture",
        "--description",
        "Selected database application",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--json",
    ];
    args.extend_from_slice(extra);
    serde_json::from_str::<Value>(&sandbox.ok(&args)).unwrap()["delivery"].clone()
}

fn object(sandbox: &Sandbox, delivery: &Value) -> std::path::PathBuf {
    sandbox
        .0
        .join(".vivac-artifacts")
        .join(delivery["object"].as_str().unwrap())
}

const DATABASE_FIXTURE: &str = "CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT NOT NULL); INSERT INTO items VALUES(1,'First revision');";
struct WalWriter(std::process::Child);
impl WalWriter {
    fn stop(&mut self) {
        use std::io::Write;
        self.0.stdin.as_mut().unwrap().write_all(b"exit\n").unwrap();
        assert!(self.0.wait().unwrap().success());
    }
}
impl Drop for WalWriter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wal_writer(sandbox: &Sandbox) -> WalWriter {
    let ready = sandbox.0.join("writer-ready");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "wal_source_process", "--nocapture"])
        .env("VIVAC_TEST_WAL_ROOT", &sandbox.0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut writer = WalWriter(child);
    let start = std::time::Instant::now();
    while !ready.exists() {
        assert!(writer.0.try_wait().unwrap().is_none(), "WAL writer exited");
        assert!(start.elapsed().as_secs() < 15, "WAL writer did not start");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    writer
}
#[test]
fn wal_source_process() {
    let Some(root) = std::env::var_os("VIVAC_TEST_WAL_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let name = CString::new(root.join("app/data.sqlite").to_str().unwrap()).unwrap();
    unsafe {
        let mut db = ptr::null_mut();
        assert_eq!(ffi::sqlite3_open(name.as_ptr(), &mut db), ffi::SQLITE_OK);
        assert_eq!(ffi::sqlite3_exec(db, c"PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO items VALUES(2,'Committed in WAL');".as_ptr(), None, ptr::null_mut(), ptr::null_mut()), ffi::SQLITE_OK);
        fs::write(root.join("writer-ready"), b"ready").unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
    }
}
fn select_wal(sandbox: &Sandbox) {
    let mut selected: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("selection.json")).unwrap()).unwrap();
    selected["database_source"] =
        json!({"profile":"readonly-wal-v1","wal":"data.sqlite-wal","shm":"data.sqlite-shm"});
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
}
#[test]
fn explicit_wal_capture_retains_committed_rows_and_exact_source_metadata() {
    let sandbox = fixture("sqlite-wal-opt-in", DATABASE_FIXTURE);
    let mut writer = wal_writer(&sandbox);
    assert_capture_refused(&sandbox);
    select_wal(&sandbox);
    let db = fs::read(sandbox.0.join("app/data.sqlite")).unwrap();
    let wal = fs::read(sandbox.0.join("app/data.sqlite-wal")).unwrap();
    let shm = fs::read(sandbox.0.join("app/data.sqlite-shm")).unwrap();
    let delivery = capture(&sandbox, &[]);
    let retained = object(&sandbox, &delivery);
    let manifest: Value =
        serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 5);
    assert_eq!(
        manifest["database"]["source"],
        json!({"profile":"readonly-wal-v1","wal":{"path":"data.sqlite-wal","bytes":wal.len(),"sha256":format!("{:x}",Sha256::digest(&wal))},"shm":"data.sqlite-shm"})
    );
    assert_eq!(
        manifest["database"]["source_sha256"],
        format!("{:x}", Sha256::digest(&db))
    );
    assert_eq!(manifest["database"]["profile"], "sqlite-standalone-v1");
    assert!(!retained.join("data.sqlite-wal").exists());
    assert!(!retained.join("data.sqlite-shm").exists());
    database(&retained.join("data.sqlite"), "CREATE TEMP TABLE expected(id INTEGER PRIMARY KEY,label TEXT NOT NULL); INSERT INTO expected VALUES(1,'First revision'),(2,'Committed in WAL'); CREATE TEMP TABLE verified(value INTEGER CHECK(value=0)); INSERT INTO verified SELECT count(*) FROM (SELECT * FROM items EXCEPT SELECT * FROM expected); INSERT INTO verified SELECT count(*) FROM (SELECT * FROM expected EXCEPT SELECT * FROM items);");
    assert_eq!(fs::read(sandbox.0.join("app/data.sqlite")).unwrap(), db);
    assert_eq!(
        fs::read(sandbox.0.join("app/data.sqlite-wal")).unwrap(),
        wal
    );
    assert_eq!(
        fs::read(sandbox.0.join("app/data.sqlite-shm")).unwrap(),
        shm
    );
    let artifact = delivery["artifact"].as_str().unwrap();
    let revision = delivery["revision"].as_str().unwrap();
    let checked: Value =
        serde_json::from_str(&sandbox.ok(&["artifacts", "check", artifact, "--json"])).unwrap();
    assert_eq!(checked["source"]["status"], "matches");
    assert_eq!(checked["retained"]["status"], "matches");
    database(
        &sandbox.0.join("app/data.sqlite"),
        "UPDATE items SET label='Later source' WHERE id=2;",
    );
    let (checked, status) = sandbox.run(&["artifacts", "check", artifact, "--json"]);
    assert_ne!(status, 0);
    let checked: Value = serde_json::from_str(&checked).unwrap();
    assert_eq!(checked["source"]["status"], "changed");
    assert_eq!(checked["retained"]["status"], "matches");
    writer.stop();
    for suffix in ["-wal", "-shm"] {
        let path = sandbox.0.join(format!("app/data.sqlite{suffix}"));
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }
    sandbox.ok(&["artifacts", "check", "--store", "--json"]);
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
        fs::read(sandbox.0.join("restored/data.sqlite")).unwrap(),
        fs::read(retained.join("data.sqlite")).unwrap()
    );
    assert!(!sandbox.log().contains("Committed in WAL"));
    assert!(!sandbox.log().contains("First revision"));
}
fn assert_capture_refused(sandbox: &Sandbox) {
    let before = sandbox.log();
    let (_, status) = sandbox.run(&[
        "artifacts",
        "capture",
        "--description",
        "Selected database application",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--json",
    ]);
    assert_ne!(status, 0);
    assert_eq!(sandbox.log(), before);
    let objects = sandbox.0.join(".vivac-artifacts/objects");
    assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
}

#[test]
fn wal_selector_rejects_null_unknown_alias_and_non_database_inputs() {
    let sandbox = fixture("sqlite-wal-selector", DATABASE_FIXTURE);
    select_wal(&sandbox);
    let base: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("selection.json")).unwrap()).unwrap();
    let mut cases = Vec::new();
    for value in [
        Value::Null,
        json!({}),
        json!({"profile":"readonly-wal-v1","wal":"data.sqlite-wal","shm":"data.sqlite-shm","extra":true}),
    ] {
        let mut selected = base.clone();
        selected["database_source"] = value;
        cases.push(selected);
    }
    for (field, values) in [
        (
            "profile",
            vec![Value::Null, json!("unknown"), json!("READONLY-WAL-V1")],
        ),
        (
            "wal",
            vec![
                Value::Null,
                json!("DATA.sqlite-wal"),
                json!("../data.sqlite-wal"),
                json!("file:data.sqlite-wal"),
                json!("other/data.sqlite-wal"),
            ],
        ),
        (
            "shm",
            vec![
                Value::Null,
                json!("DATA.sqlite-shm"),
                json!("./data.sqlite-shm"),
                json!("data.sqlite-wal"),
            ],
        ),
    ] {
        for value in values {
            let mut selected = base.clone();
            selected["database_source"][field] = value;
            cases.push(selected);
        }
    }
    for field in ["database", "runtime"] {
        let mut selected = base.clone();
        selected.as_object_mut().unwrap().remove(field);
        cases.push(selected);
    }
    for selected in cases {
        fs::write(
            sandbox.0.join("selection.json"),
            serde_json::to_vec(&selected).unwrap(),
        )
        .unwrap();
        assert_capture_refused(&sandbox);
    }
}

#[test]
fn wal_capture_refuses_missing_auxiliary_journal_and_hard_links_atomically() {
    for name in [
        "missing-wal",
        "missing-shm",
        "journal",
        "hard-link",
        "hard-link-db",
        "hard-link-shm",
        "partial-wal",
    ] {
        let sandbox = fixture(name, DATABASE_FIXTURE);
        let writer = wal_writer(&sandbox);
        drop(writer);
        select_wal(&sandbox);
        match name {
            "missing-wal" => fs::rename(
                sandbox.0.join("app/data.sqlite-wal"),
                sandbox.0.join("app/hidden-wal"),
            )
            .unwrap(),
            "missing-shm" => fs::rename(
                sandbox.0.join("app/data.sqlite-shm"),
                sandbox.0.join("app/hidden-shm"),
            )
            .unwrap(),
            "journal" => fs::write(sandbox.0.join("app/data.sqlite-journal"), []).unwrap(),
            "hard-link" => fs::hard_link(
                sandbox.0.join("app/data.sqlite-wal"),
                sandbox.0.join("app/alias"),
            )
            .unwrap(),
            "hard-link-db" => fs::hard_link(
                sandbox.0.join("app/data.sqlite"),
                sandbox.0.join("app/alias"),
            )
            .unwrap(),
            "hard-link-shm" => fs::hard_link(
                sandbox.0.join("app/data.sqlite-shm"),
                sandbox.0.join("app/alias"),
            )
            .unwrap(),
            "partial-wal" => fs::write(sandbox.0.join("app/data.sqlite-wal"), [0; 31]).unwrap(),
            _ => unreachable!(),
        }
        assert_capture_refused(&sandbox);
    }
}
#[cfg(unix)]
#[test]
fn wal_capture_refuses_symbolic_auxiliary_links_atomically() {
    for suffix in ["-wal", "-shm"] {
        let sandbox = fixture("sqlite-wal-link", DATABASE_FIXTURE);
        let writer = wal_writer(&sandbox);
        drop(writer);
        select_wal(&sandbox);
        fs::rename(
            sandbox.0.join(format!("app/data.sqlite{suffix}")),
            sandbox.0.join("app/hidden"),
        )
        .unwrap();
        std::os::unix::fs::symlink("hidden", sandbox.0.join(format!("app/data.sqlite{suffix}")))
            .unwrap();
        assert_capture_refused(&sandbox);
    }
}
#[test]
fn wal_capture_refuses_sensitive_rows_and_unsupported_schema_atomically() {
    for (name, sql) in [
        (
            "wal-secret",
            "UPDATE items SET label='person@example.com' WHERE id=2;",
        ),
        ("wal-view", "CREATE VIEW visible AS SELECT * FROM items;"),
        ("wal-blob", "ALTER TABLE items ADD COLUMN binary BLOB;"),
    ] {
        let sandbox = fixture(name, DATABASE_FIXTURE);
        let _writer = wal_writer(&sandbox);
        database(&sandbox.0.join("app/data.sqlite"), sql);
        select_wal(&sandbox);
        assert_capture_refused(&sandbox);
    }
}
#[test]
fn wal_capture_accepts_an_empty_wal_after_checkpoint() {
    let sandbox = fixture("sqlite-empty-wal", DATABASE_FIXTURE);
    let _writer = wal_writer(&sandbox);
    database(
        &sandbox.0.join("app/data.sqlite"),
        "PRAGMA wal_checkpoint(TRUNCATE);",
    );
    assert_eq!(
        fs::metadata(sandbox.0.join("app/data.sqlite-wal"))
            .unwrap()
            .len(),
        0
    );
    select_wal(&sandbox);
    let delivery = capture(&sandbox, &[]);
    let manifest: Value = serde_json::from_slice(
        &fs::read(object(&sandbox, &delivery).join(".vivac-delivery.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["database"]["source"]["wal"]["bytes"], 0);
    assert_eq!(
        manifest["database"]["source"]["wal"]["sha256"],
        format!("{:x}", Sha256::digest([]))
    );
}
#[test]
fn wal_source_budget_includes_frontend_and_runtime_bytes() {
    let sandbox = fixture("sqlite-wal-source-budget", DATABASE_FIXTURE);
    let writer = wal_writer(&sandbox);
    drop(writer);
    select_wal(&sandbox);
    let database_bytes = fs::metadata(sandbox.0.join("app/data.sqlite"))
        .unwrap()
        .len();
    fs::OpenOptions::new()
        .write(true)
        .open(sandbox.0.join("app/data.sqlite-wal"))
        .unwrap()
        .set_len(10 * 1024 * 1024 - database_bytes)
        .unwrap();
    let before = sandbox.log();
    let (message, status) = sandbox.run(&[
        "artifacts",
        "capture",
        "--description",
        "Selected database application",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--json",
    ]);
    assert_ne!(status, 0);
    assert!(
        message.contains("complete source bundle exceeds 10 MiB"),
        "{message}"
    );
    assert_eq!(sandbox.log(), before);
    let objects = sandbox.0.join(".vivac-artifacts/objects");
    assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
}
#[cfg(unix)]
#[test]
fn wal_capture_reads_readonly_sources_without_creating_source_files() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = fixture("sqlite-readonly-wal", DATABASE_FIXTURE);
    let _writer = wal_writer(&sandbox);
    select_wal(&sandbox);
    let app = sandbox.0.join("app");
    for name in ["data.sqlite", "data.sqlite-wal", "data.sqlite-shm"] {
        fs::set_permissions(app.join(name), fs::Permissions::from_mode(0o444)).unwrap();
    }
    fs::set_permissions(&app, fs::Permissions::from_mode(0o555)).unwrap();
    let mut before: Vec<_> = fs::read_dir(&app)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let delivery = capture(&sandbox, &[]);
    assert_eq!(delivery["member_count"], 4);
    let mut after: Vec<_> = fs::read_dir(&app)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    before.sort();
    after.sort();
    assert_eq!(before, after);
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["data.sqlite", "data.sqlite-wal", "data.sqlite-shm"] {
        fs::set_permissions(app.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
}
const PRODUCT_RUNTIME_ID: &str = "sqljs-asm-1.14.2";
const PRODUCT_RUNTIME_PATH: &str = "vivac-sqlite.js";
const PRODUCT_RUNTIME_BYTES: u64 = 1357427;
const PRODUCT_RUNTIME_SHA256: &str =
    "a5709877905821fcae8aeb6789c53302eb0ffc50473f81dc2bfbdd93a16f9b38";

fn product_selection(sandbox: &Sandbox) -> Value {
    let mut selection: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("selection.json")).unwrap()).unwrap();
    selection["runtime"] = json!(PRODUCT_RUNTIME_ID);
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selection).unwrap(),
    )
    .unwrap();
    selection
}

#[test]
fn product_runtime_selector_rejects_incomplete_unknown_and_reserved_inputs() {
    let sandbox = fixture("product-runtime-selector", DATABASE_FIXTURE);
    let mut base: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("selection.json")).unwrap()).unwrap();
    base.as_object_mut().unwrap().remove("runtime");
    let mut cases = vec![base.clone()];
    let mut valid = base.clone();
    valid["runtime"] = json!(PRODUCT_RUNTIME_ID);
    for (key, value) in [
        ("runtime", Value::Null),
        ("runtime", json!("")),
        ("runtime", json!("unknown-runtime")),
        ("database", Value::Null),
        ("database", json!("")),
    ] {
        let mut selection = valid.clone();
        selection[key] = value;
        cases.push(selection);
    }
    let mut missing_database = valid.clone();
    missing_database.as_object_mut().unwrap().remove("database");
    cases.push(missing_database);
    let mut static_null = json!({"root":"app","entrypoint":"index.html","members":["index.html","main.js"],"runtime":null});
    cases.push(static_null.clone());
    static_null.as_object_mut().unwrap().remove("runtime");
    static_null["database"] = Value::Null;
    cases.push(static_null);
    for path in [
        PRODUCT_RUNTIME_PATH,
        "VIVAC-SQLITE.JS",
        "vivac-sqlite.js/child.js",
        "Vivac-Sqlite.Js/child.js",
    ] {
        let mut selection = valid.clone();
        selection["members"]
            .as_array_mut()
            .unwrap()
            .push(json!(path));
        cases.push(selection);
    }
    let before = sandbox.log();
    for selection in cases {
        fs::write(
            sandbox.0.join("selection.json"),
            serde_json::to_vec(&selection).unwrap(),
        )
        .unwrap();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--description",
                    "Selected database application",
                    "--bundle",
                    "selection.json",
                    "--from",
                    "g1",
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(sandbox.log(), before);
        let objects = sandbox.0.join(".vivac-artifacts/objects");
        assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
    }
}

#[test]
fn product_runtime_is_generated_without_opening_a_project_copy_and_restores_exact_bytes() {
    let sandbox = fixture("product-runtime-generated", DATABASE_FIXTURE);
    product_selection(&sandbox);
    fs::write(
        sandbox.0.join("app").join(PRODUCT_RUNTIME_PATH),
        b"person@example.com",
    )
    .unwrap();
    let first = capture(&sandbox, &["--operation", "01eeeeeeeeeeeeeeeeeeeeeeee"]);
    assert_eq!(first["runtime"], PRODUCT_RUNTIME_ID);
    assert_eq!(first["runtime_sha256"], PRODUCT_RUNTIME_SHA256);
    assert_eq!(first["member_count"], 4);
    let retained = object(&sandbox, &first);
    let bytes = fs::read(retained.join(PRODUCT_RUNTIME_PATH)).unwrap();
    assert_eq!(bytes.len() as u64, PRODUCT_RUNTIME_BYTES);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        PRODUCT_RUNTIME_SHA256
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(
        manifest["runtime"],
        json!({"id":PRODUCT_RUNTIME_ID,"path":PRODUCT_RUNTIME_PATH,"bytes":PRODUCT_RUNTIME_BYTES,"sha256":PRODUCT_RUNTIME_SHA256})
    );
    assert_eq!(
        manifest["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|member| member["path"] == PRODUCT_RUNTIME_PATH)
            .count(),
        1
    );
    let artifact = first["artifact"].as_str().unwrap();
    let revision = first["revision"].as_str().unwrap();
    let check: Value =
        serde_json::from_str(&sandbox.ok(&["artifacts", "check", artifact, "--json"])).unwrap();
    assert_eq!(check["source"]["status"], "matches");
    let mut selected: Value =
        serde_json::from_slice(&fs::read(sandbox.0.join("selection.json")).unwrap()).unwrap();
    selected["runtime"] = json!("unknown-runtime");
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    let (changed, code) = sandbox.run(&["artifacts", "check", artifact, "--json"]);
    assert_ne!(code, 0);
    let changed: Value = serde_json::from_str(&changed).unwrap();
    assert_eq!(changed["source"]["status"], "changed");
    assert_eq!(changed["retained"]["status"], "matches");
    selected["runtime"] = json!(PRODUCT_RUNTIME_ID);
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    database(
        &sandbox.0.join("app/data.sqlite"),
        "UPDATE items SET label='Later data' WHERE id=1;",
    );
    capture(&sandbox, &["--artifact", artifact, "--previous", revision]);
    let shown: Value =
        serde_json::from_str(&sandbox.ok(&["artifacts", "show", artifact, "--json"])).unwrap();
    assert_eq!(shown["revisions"].as_array().unwrap().len(), 2);
    for entry in shown["revisions"].as_array().unwrap() {
        assert_eq!(entry["runtime"], PRODUCT_RUNTIME_ID);
        assert_eq!(entry["runtime_sha256"], PRODUCT_RUNTIME_SHA256);
    }
    let listed: Value =
        serde_json::from_str(&sandbox.ok(&["artifacts", "list", "--all", "--json"])).unwrap();
    assert_eq!(listed["artifacts"][0]["runtime"], PRODUCT_RUNTIME_ID);
    assert_eq!(
        listed["artifacts"][0]["runtime_sha256"],
        PRODUCT_RUNTIME_SHA256
    );
    fs::remove_dir_all(sandbox.0.join("app")).unwrap();
    fs::remove_file(sandbox.0.join("selection.json")).unwrap();
    let retry = capture(&sandbox, &["--operation", "01eeeeeeeeeeeeeeeeeeeeeeee"]);
    assert_eq!(retry["revision"], revision);
    assert_eq!(retry["runtime"], PRODUCT_RUNTIME_ID);
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
        fs::read(sandbox.0.join("restored").join(PRODUCT_RUNTIME_PATH)).unwrap(),
        bytes
    );
    for path in [
        "index.html",
        "main.js",
        "data.sqlite",
        ".vivac-delivery.json",
    ] {
        assert_eq!(
            fs::read(sandbox.0.join("restored").join(path)).unwrap(),
            fs::read(retained.join(path)).unwrap()
        );
    }
}

#[test]
fn product_runtime_missing_truncated_modified_or_extra_resources_refuse_restore() {
    for change in ["missing", "truncated", "modified", "extra"] {
        let sandbox = fixture("product-runtime-tamper", DATABASE_FIXTURE);
        product_selection(&sandbox);
        let delivery = capture(&sandbox, &[]);
        let retained = object(&sandbox, &delivery);
        let path = retained.join(PRODUCT_RUNTIME_PATH);
        match change {
            "missing" => fs::remove_file(path).unwrap(),
            "truncated" => {
                let mut bytes = fs::read(&path).unwrap();
                bytes.pop();
                fs::write(path, bytes).unwrap();
            }
            "modified" => {
                let mut bytes = fs::read(&path).unwrap();
                bytes[0] ^= 1;
                fs::write(path, bytes).unwrap();
            }
            "extra" => fs::write(retained.join("extra.js"), b"Unlisted resource").unwrap(),
            _ => unreachable!(),
        }
        let before = sandbox.log();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "restore",
                    delivery["artifact"].as_str().unwrap(),
                    "--revision",
                    delivery["revision"].as_str().unwrap(),
                    "--to",
                    "refused",
                    "--json"
                ])
                .1,
            0
        );
        assert!(!sandbox.0.join("refused").exists());
        assert_eq!(sandbox.log(), before);
    }
}

#[test]
fn product_runtime_does_not_exempt_author_javascript_from_the_content_guard() {
    let sandbox = fixture("product-runtime-author-guard", DATABASE_FIXTURE);
    let mut selection = product_selection(&sandbox);
    fs::write(
        sandbox.0.join("app/author.js"),
        b"const contact = 'person@example.com';",
    )
    .unwrap();
    selection["members"]
        .as_array_mut()
        .unwrap()
        .push(json!("author.js"));
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selection).unwrap(),
    )
    .unwrap();
    let before = sandbox.log();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "capture",
                "--description",
                "Selected database application",
                "--bundle",
                "selection.json",
                "--from",
                "g1",
                "--json"
            ])
            .1,
        0
    );
    assert_eq!(sandbox.log(), before);
    let objects = sandbox.0.join(".vivac-artifacts/objects");
    assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
}

#[test]
fn static_bundles_keep_author_javascript_at_the_product_runtime_name_or_parent() {
    for path in ["vivac-sqlite.js", "VIVAC-SQLITE.JS/author.js"] {
        let sandbox = fixture("static-author-runtime-name", DATABASE_FIXTURE);
        let selected = sandbox.0.join("app").join(path);
        fs::create_dir_all(selected.parent().unwrap()).unwrap();
        let authored = b"document.title = 'Author supplied static program';";
        fs::write(selected, authored).unwrap();
        fs::write(
            sandbox.0.join("selection.json"),
            serde_json::to_vec(
                &json!({"root":"app","entrypoint":"index.html","members":["index.html",path]}),
            )
            .unwrap(),
        )
        .unwrap();
        let delivery = capture(&sandbox, &[]);
        assert_eq!(delivery["delivery_type"], "bundle");
        let retained = object(&sandbox, &delivery);
        let manifest: Value =
            serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert!(manifest.get("runtime").is_none());
        assert!(manifest.get("database").is_none());
        assert_eq!(manifest["members"].as_array().unwrap().len(), 2);
        assert_eq!(fs::read(retained.join(path)).unwrap(), authored);
        sandbox.ok(&[
            "artifacts",
            "restore",
            delivery["artifact"].as_str().unwrap(),
            "--revision",
            delivery["revision"].as_str().unwrap(),
            "--to",
            "restored",
            "--json",
        ]);
        assert_eq!(
            fs::read(sandbox.0.join("restored").join(path)).unwrap(),
            authored
        );
    }
}

#[test]
fn product_runtime_counts_toward_the_member_limit_without_dropping_selected_files() {
    let sandbox = fixture("sqlite-runtime-member-limit", DATABASE_FIXTURE);
    let mut selected = product_selection(&sandbox);
    for index in 0..60 {
        let path = format!("part-{index}.js");
        fs::write(
            sandbox.0.join("app").join(&path),
            b"document.title = 'Selected member';",
        )
        .unwrap();
        selected["members"]
            .as_array_mut()
            .unwrap()
            .push(json!(path));
    }
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    let delivery = capture(&sandbox, &[]);
    assert_eq!(delivery["member_count"], 64);
    let retained = object(&sandbox, &delivery);
    for path in selected["members"].as_array().unwrap() {
        assert!(retained.join(path.as_str().unwrap()).is_file());
    }
    assert!(retained.join(PRODUCT_RUNTIME_PATH).is_file());
    fs::write(
        sandbox.0.join("app/overflow.js"),
        b"document.title = 'Overflow member';",
    )
    .unwrap();
    selected["members"]
        .as_array_mut()
        .unwrap()
        .push(json!("overflow.js"));
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(&selected).unwrap(),
    )
    .unwrap();
    let before = sandbox.log();
    let (error, code) = sandbox.run(&[
        "artifacts",
        "capture",
        "--description",
        "Selected database application",
        "--bundle",
        "selection.json",
        "--from",
        "g1",
        "--json",
    ]);
    assert_ne!(code, 0);
    assert!(error.contains("64 members"));
    assert_eq!(sandbox.log(), before);
    assert_eq!(
        fs::read_dir(sandbox.0.join(".vivac-artifacts/objects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn sqlite_application_retains_revisions_source_observation_retry_and_exact_restore() {
    let sandbox = fixture("sqlite-application-lifecycle", DATABASE_FIXTURE);
    database(&sandbox.0.join("app/data.sqlite"), "PRAGMA secure_delete=OFF; INSERT INTO items VALUES(2,'person@example.com'); DELETE FROM items WHERE id=2;");
    let source = fs::read(sandbox.0.join("app/data.sqlite")).unwrap();
    let first = capture(&sandbox, &["--operation", "01cccccccccccccccccccccccc"]);
    assert_eq!(first["delivery_type"], "sqlite_application");
    assert_eq!(first["database"], "data.sqlite");
    assert_eq!(first["profile"], "sqlite-standalone-v1");
    let retained = object(&sandbox, &first);
    let manifest: Value =
        serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(manifest["delivery_type"], "sqlite_application");
    assert_eq!(manifest["database"]["path"], "data.sqlite");
    assert_eq!(manifest["database"]["profile"], "sqlite-standalone-v1");
    assert_eq!(manifest["database"]["transformation"], "logical-export-v1");
    assert_eq!(manifest["database"]["source_bytes"], source.len());
    assert_eq!(
        manifest["database"]["source_sha256"],
        format!("{:x}", Sha256::digest(&source))
    );
    let normalized = fs::read(retained.join("data.sqlite")).unwrap();
    assert_ne!(normalized, source);
    assert!(!normalized
        .windows(b"person@example.com".len())
        .any(|part| part == b"person@example.com"));
    assert_eq!(fs::read(sandbox.0.join("app/data.sqlite")).unwrap(), source);
    let artifact = first["artifact"].as_str().unwrap();
    let revision = first["revision"].as_str().unwrap();
    let checked: Value =
        serde_json::from_str(&sandbox.ok(&["artifacts", "check", artifact, "--json"])).unwrap();
    assert_eq!(checked["source"]["status"], "matches");
    assert_eq!(checked["retained"]["status"], "matches");
    let reference = format!("artifact:{artifact}@{revision}");
    sandbox.ok(&[
        "decide",
        "Keep the reviewed data",
        "--reason",
        "preserve the selected evidence",
        "--ref",
        &reference,
    ]);
    database(
        &sandbox.0.join("app/data.sqlite"),
        "UPDATE items SET label='Second revision' WHERE id=1;",
    );
    let second = capture(&sandbox, &["--artifact", artifact, "--previous", revision]);
    assert_eq!(second["previous"], revision);
    sandbox.ok(&["why", "g1"]);
    let index = sandbox.0.join(".vivac/index");
    assert!(index.is_file());
    let cached = sandbox.ok(&["artifacts", "show", artifact, "--json"]);
    fs::remove_file(index).unwrap();
    let rebuilt = sandbox.ok(&["artifacts", "show", artifact, "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&cached).unwrap(),
        serde_json::from_str::<Value>(&rebuilt).unwrap()
    );
    fs::remove_dir_all(sandbox.0.join("app")).unwrap();
    fs::remove_file(sandbox.0.join("selection.json")).unwrap();
    let retry = capture(&sandbox, &["--operation", "01cccccccccccccccccccccccc"]);
    assert_eq!(retry["revision"], revision);
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
    for path in [
        "index.html",
        "main.js",
        "data.sqlite",
        ".vivac-delivery.json",
    ] {
        assert_eq!(
            fs::read(sandbox.0.join("restored").join(path)).unwrap(),
            fs::read(retained.join(path)).unwrap()
        );
    }
    let log = sandbox.log();
    assert!(log.contains("artifact.sqlite_application_captured"));
    assert!(!log.contains("First revision"));
    assert!(!log.contains("person@example.com"));
    sandbox.ok(&["artifacts", "check", "--store", "--json"]);
}

#[test]
fn sqlite_requires_explicit_exact_selection_and_refuses_unsafe_databases_atomically() {
    for (name, sql) in [("blob", "CREATE TABLE items(id INTEGER PRIMARY KEY, value BLOB); INSERT INTO items VALUES(1,x'4142');"), ("view", "CREATE TABLE items(id INTEGER PRIMARY KEY); CREATE VIEW visible AS SELECT * FROM items;"), ("secret", "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES(1,'person@example.com');"), ("missing-key", "CREATE TABLE items(value TEXT);")] {
        let sandbox = fixture(name, sql);
        let events = sandbox.log();
        let source = fs::read(sandbox.0.join("app/data.sqlite")).unwrap();
        let (_, code) = sandbox.run(&["artifacts", "capture", "--description", "Selected database application", "--bundle", "selection.json", "--from", "g1", "--json"]);
        assert_ne!(code, 0, "unsafe database accepted: {name}");
        assert_eq!(sandbox.log(), events);
        assert_eq!(fs::read(sandbox.0.join("app/data.sqlite")).unwrap(), source);
        let objects = sandbox.0.join(".vivac-artifacts/objects");
        assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
    }
    let sandbox = fixture("sqlite-explicit-selection", DATABASE_FIXTURE);
    for selected in [None, Some("DATA.sqlite"), Some("main.js")] {
        selection(&sandbox, selected);
        let before = sandbox.log();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--description",
                    "Selected database application",
                    "--bundle",
                    "selection.json",
                    "--from",
                    "g1",
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(sandbox.log(), before);
    }
}

#[test]
fn sqlite_auxiliary_files_refuse_capture_without_a_receipt_or_partial_object() {
    let sandbox = fixture("sqlite-auxiliary-refusal", DATABASE_FIXTURE);
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = sandbox.0.join(format!("app/data.sqlite{suffix}"));
        fs::write(&path, b"").unwrap();
        let before = sandbox.log();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--description",
                    "Selected database application",
                    "--bundle",
                    "selection.json",
                    "--from",
                    "g1",
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(sandbox.log(), before);
        fs::remove_file(path).unwrap();
        let objects = sandbox.0.join(".vivac-artifacts/objects");
        assert!(!objects.exists() || fs::read_dir(objects).unwrap().next().is_none());
    }
}

#[test]
fn sqlite_and_static_bundle_identities_cannot_change_kind() {
    let sandbox = fixture("sqlite-bundle-identity", DATABASE_FIXTURE);
    let sqlite = capture(&sandbox, &[]);
    fs::write(
        sandbox.0.join("static.json"),
        serde_json::to_vec(
            &json!({"root":"app","entrypoint":"index.html","members":["index.html","main.js"]}),
        )
        .unwrap(),
    )
    .unwrap();
    let static_delivery: Value = serde_json::from_str::<Value>(&sandbox.ok(&[
        "artifacts",
        "capture",
        "--description",
        "Static application",
        "--bundle",
        "static.json",
        "--from",
        "g1",
        "--json",
    ]))
    .unwrap()["delivery"]
        .clone();
    for (previous, selector) in [
        (&sqlite, "static.json"),
        (&static_delivery, "selection.json"),
    ] {
        let before = sandbox.log();
        let objects = fs::read_dir(sandbox.0.join(".vivac-artifacts/objects"))
            .unwrap()
            .count();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "capture",
                    "--description",
                    "Selected database application",
                    "--bundle",
                    selector,
                    "--from",
                    "g1",
                    "--artifact",
                    previous["artifact"].as_str().unwrap(),
                    "--previous",
                    previous["revision"].as_str().unwrap(),
                    "--json"
                ])
                .1,
            0
        );
        assert_eq!(sandbox.log(), before);
        assert_eq!(
            fs::read_dir(sandbox.0.join(".vivac-artifacts/objects"))
                .unwrap()
                .count(),
            objects
        );
    }
}

#[test]
fn retained_sqlite_corruption_refuses_restore_before_publication() {
    let sandbox = fixture("sqlite-retained-corruption", DATABASE_FIXTURE);
    let delivery = capture(&sandbox, &[]);
    let saved = object(&sandbox, &delivery).join("data.sqlite");
    let mut bytes = fs::read(&saved).unwrap();
    bytes[0] ^= 1;
    fs::write(saved, bytes).unwrap();
    let before = sandbox.log();
    assert_ne!(
        sandbox
            .run(&[
                "artifacts",
                "restore",
                delivery["artifact"].as_str().unwrap(),
                "--revision",
                delivery["revision"].as_str().unwrap(),
                "--to",
                "restored",
                "--json"
            ])
            .1,
        0
    );
    assert!(!sandbox.0.join("restored").exists());
    assert_eq!(sandbox.log(), before);
}

#[test]
fn sqlite_preview_binary_is_only_served_from_the_opaque_resource_base_after_run() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::process::{Child, Command, Stdio};
    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn request(host: &str, method: &str, path: &str, headers: &str, body: &str) -> Vec<u8> {
        let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(socket, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).unwrap();
        response
    }
    let sandbox = fixture("sqlite-preview-binary", DATABASE_FIXTURE);
    let delivery = capture(&sandbox, &[]);
    let bytes = fs::read(object(&sandbox, &delivery).join("data.sqlite")).unwrap();
    let mut process = Running(
        Command::new(env!("CARGO_BIN_EXE_vivac"))
            .current_dir(&sandbox.0)
            .env("VIVAC_HOME", sandbox.global_home())
            .args([
                "artifacts",
                "preview",
                delivery["artifact"].as_str().unwrap(),
                "--revision",
                delivery["revision"].as_str().unwrap(),
                "--json",
                "--no-open",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(process.0.stdout.take().unwrap());
    let mut text = String::new();
    let ready: Value = loop {
        assert!(
            output.read_line(&mut text).unwrap() > 0,
            "preview did not start: {text}"
        );
        if let Ok(value) = serde_json::from_str(&text) {
            break value;
        }
    };
    let (host, token) = ready["url"]
        .as_str()
        .unwrap()
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let wrapper = request(host, "GET", &format!("/{token}"), "", "");
    let wrapper = std::str::from_utf8(&wrapper).unwrap();
    assert!(wrapper.contains("Runtime pinned to this revision"));
    assert!(wrapper.contains(PRODUCT_RUNTIME_ID));
    assert!(wrapper.contains(PRODUCT_RUNTIME_SHA256));
    assert!(wrapper.contains("Updating vivac does not replace this runtime."));
    let refused = request(host, "GET", "/data.sqlite", "Origin: null\r\n", "");
    assert!(refused.starts_with(b"HTTP/1.1 403"));
    assert!(!refused
        .windows(b"Access-Control-Allow-Origin".len())
        .any(|part| part == b"Access-Control-Allow-Origin"));
    assert!(!refused.windows(bytes.len()).any(|part| part == bytes));
    assert!(request(host, "GET", "/data.sqlite", "", "").starts_with(b"HTTP/1.1 404"));
    let body = format!("token={token}");
    let run = request(
        host,
        "POST",
        "/run",
        &format!("Origin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\n"),
        &body,
    );
    let run = std::str::from_utf8(&run).unwrap();
    assert!(run.starts_with("HTTP/1.1 303"));
    let entry = run
        .lines()
        .find_map(|line| line.strip_prefix("Location: "))
        .unwrap();
    let resources = entry.strip_suffix("index.html").unwrap();
    assert!(!resources.contains(token));
    let response = request(
        host,
        "GET",
        &format!("{resources}data.sqlite"),
        "Origin: null\r\n",
        "",
    );
    let boundary = response
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = std::str::from_utf8(&response[..boundary]).unwrap();
    assert!(headers.starts_with("HTTP/1.1 200"));
    assert!(headers.contains("Content-Type: application/vnd.sqlite3\r\n"));
    assert!(headers.contains("Access-Control-Allow-Origin: null\r\n"));
    assert!(headers.contains(&format!("connect-src http://{host}{resources};")));
    assert!(!headers.contains("connect-src 'self'"));
    assert!(!headers.contains(&format!("connect-src http://{host};")));
    assert!(!headers.contains("Access-Control-Allow-Credentials"));
    assert!(!headers.contains("unsafe-eval"));
    assert!(!headers.contains("wasm-unsafe-eval"));
    assert!(!headers.contains("allow-same-origin"));
    assert_eq!(&response[boundary..], bytes);
    assert!(request(
        host,
        "GET",
        &format!("{resources}.vivac-delivery.json"),
        "Origin: null\r\n",
        ""
    )
    .starts_with(b"HTTP/1.1 404"));
}

#[test]
fn sqlite_mcp_retry_uses_the_same_receipt_as_cli_after_selector_loss() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    struct Server {
        child: Child,
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    impl Server {
        fn ask(&mut self, id: u64, method: &str, params: Value) -> Value {
            writeln!(
                self.input,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .unwrap();
            self.input.flush().unwrap();
            let mut line = String::new();
            assert!(self.output.read_line(&mut line).unwrap() > 0);
            let reply: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["id"], id);
            assert!(reply.get("error").is_none(), "{reply}");
            reply
        }
    }
    let sandbox = fixture("sqlite-mcp-cli-contract", DATABASE_FIXTURE);
    let operation = "01dddddddddddddddddddddddd";
    let cli = capture(&sandbox, &["--operation", operation]);
    fs::remove_dir_all(sandbox.0.join("app")).unwrap();
    fs::remove_file(sandbox.0.join("selection.json")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&sandbox.0)
        .env("VIVAC_HOME", sandbox.global_home())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut server = Server {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()),
        child,
    };
    server.ask(1, "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"sqlite-tests","version":"1"}}));
    writeln!(
        server.input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    server.input.flush().unwrap();
    let reply = server.ask(2, "tools/call", json!({"name":"vivac_artifacts","arguments":{"operation":"capture","description":"Selected database application","bundle":"selection.json","from":"g1","operation_id":operation}}));
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let mcp: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(mcp["exit_code"], 0);
    assert_eq!(mcp["result"]["already_recorded"], true);
    assert_eq!(mcp["result"]["delivery"], cli);
}

#[test]
fn manifest_database_shape_is_strict_even_with_a_matching_custody_digest() {
    fn refuse(sandbox: &Sandbox, delivery: &Value, manifest: &Value) {
        let path = object(sandbox, delivery).join(".vivac-delivery.json");
        let old = fs::read(&path).unwrap();
        let bytes = serde_json::to_vec(manifest).unwrap();
        let total = delivery["size"].as_u64().unwrap() - old.len() as u64 + bytes.len() as u64;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let events = sandbox
            .log()
            .lines()
            .map(|line| {
                let mut event: Value = serde_json::from_str(line).unwrap();
                if event["payload"]["revision"] == delivery["revision"] {
                    event["payload"]["digest"] = json!(digest);
                    event["payload"]["size"] = json!(total);
                    event["payload"]["manifest_bytes"] = json!(bytes.len());
                }
                serde_json::to_string(&event).unwrap() + "\n"
            })
            .collect::<String>();
        fs::write(path, bytes).unwrap();
        fs::write(sandbox.0.join(".vivac/events"), events).unwrap();
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "restore",
                    delivery["artifact"].as_str().unwrap(),
                    "--revision",
                    delivery["revision"].as_str().unwrap(),
                    "--to",
                    "refused",
                    "--json"
                ])
                .1,
            0
        );
        assert!(!sandbox.0.join("refused").exists());
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "check",
                    delivery["artifact"].as_str().unwrap(),
                    "--json"
                ])
                .1,
            0
        );
    }
    let sandbox = fixture("static-manifest-database-null", DATABASE_FIXTURE);
    fs::write(
        sandbox.0.join("selection.json"),
        serde_json::to_vec(
            &json!({"root":"app","entrypoint":"index.html","members":["index.html","main.js"]}),
        )
        .unwrap(),
    )
    .unwrap();
    let delivery = capture(&sandbox, &[]);
    let mut manifest: Value = serde_json::from_slice(
        &fs::read(object(&sandbox, &delivery).join(".vivac-delivery.json")).unwrap(),
    )
    .unwrap();
    manifest["database"] = Value::Null;
    refuse(&sandbox, &delivery, &manifest);
    for absent in [false, true] {
        let sandbox = fixture("sqlite-manifest-missing-database", DATABASE_FIXTURE);
        let delivery = capture(&sandbox, &[]);
        let mut manifest: Value = serde_json::from_slice(
            &fs::read(object(&sandbox, &delivery).join(".vivac-delivery.json")).unwrap(),
        )
        .unwrap();
        if absent {
            manifest.as_object_mut().unwrap().remove("database");
        } else {
            manifest["database"] = Value::Null;
        }
        refuse(&sandbox, &delivery, &manifest);
    }
    for case in [
        "absent",
        "null",
        "unknown_id",
        "origin",
        "path",
        "size",
        "hash",
        "member_hash",
        "duplicate",
        "experimental_v2",
    ] {
        let sandbox = fixture("sqlite-runtime-manifest-guards", DATABASE_FIXTURE);
        let delivery = capture(&sandbox, &[]);
        let mut manifest: Value = serde_json::from_slice(
            &fs::read(object(&sandbox, &delivery).join(".vivac-delivery.json")).unwrap(),
        )
        .unwrap();
        match case {
            "absent" => {
                manifest.as_object_mut().unwrap().remove("runtime");
            }
            "null" => manifest["runtime"] = Value::Null,
            "unknown_id" => manifest["runtime"]["id"] = json!("unknown-runtime"),
            "origin" => manifest["runtime"]["origin"] = json!("product"),
            "path" => manifest["runtime"]["path"] = json!("main.js"),
            "size" => manifest["runtime"]["bytes"] = json!(PRODUCT_RUNTIME_BYTES - 1),
            "hash" => manifest["runtime"]["sha256"] = json!("0".repeat(64)),
            "member_hash" => {
                manifest["members"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|member| member["path"] == PRODUCT_RUNTIME_PATH)
                    .unwrap()["sha256"] = json!("0".repeat(64));
            }
            "duplicate" => {
                let generated = manifest["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|member| member["path"] == PRODUCT_RUNTIME_PATH)
                    .unwrap()
                    .clone();
                manifest["members"].as_array_mut().unwrap().push(generated);
            }
            "experimental_v2" => {
                manifest["schema_version"] = json!(2);
                manifest.as_object_mut().unwrap().remove("runtime");
            }
            _ => unreachable!(),
        }
        refuse(&sandbox, &delivery, &manifest);
    }
}

#[test]
fn sqlite_receipt_metadata_refuses_invalid_paths_profiles_and_missing_required_fields() {
    let sandbox = fixture("sqlite-receipt-guards", DATABASE_FIXTURE);
    let delivery = capture(&sandbox, &[]);
    let original = sandbox.log();
    for (field, value) in [
        ("database", Some(json!("../data.sqlite"))),
        ("database", Some(json!("main.js"))),
        ("profile", Some(json!("unknown-profile"))),
        ("database", None),
        ("profile", None),
        ("description", None),
        ("runtime", None),
        ("runtime_sha256", None),
        ("runtime", Some(Value::Null)),
        ("runtime", Some(json!("unknown-runtime"))),
        ("runtime_sha256", Some(json!("0".repeat(64)))),
    ] {
        let log = original
            .lines()
            .map(|line| {
                let mut event: Value = serde_json::from_str(line).unwrap();
                if event["payload"]["revision"] == delivery["revision"] {
                    if let Some(value) = &value {
                        event["payload"][field] = value.clone();
                    } else {
                        event["payload"].as_object_mut().unwrap().remove(field);
                    }
                }
                serde_json::to_string(&event).unwrap() + "\n"
            })
            .collect::<String>();
        fs::write(sandbox.0.join(".vivac/events"), log).unwrap();
        let index = sandbox.0.join(".vivac/index");
        if index.exists() {
            fs::remove_file(index).unwrap();
        }
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "show",
                    delivery["artifact"].as_str().unwrap(),
                    "--json"
                ])
                .1,
            0
        );
        assert_ne!(
            sandbox
                .run(&[
                    "artifacts",
                    "restore",
                    delivery["artifact"].as_str().unwrap(),
                    "--revision",
                    delivery["revision"].as_str().unwrap(),
                    "--to",
                    "refused",
                    "--json"
                ])
                .1,
            0
        );
        assert!(!sandbox.0.join("refused").exists());
    }
}
