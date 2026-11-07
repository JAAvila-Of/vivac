use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "vivac-device-proof-{name}-{}-{next}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for directory in [
            "home",
            "profile",
            "installed",
            "project/.vivac",
            "project/.claude",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "home/registry",
            "profile/credentials",
            "installed/vivac.exe",
            "project/.vivac/events",
            "project/.vivac/config",
            "project/.mcp.json",
            "project/.claude/settings.json",
        ] {
            fs::write(root.join(file), b"fakeSensitiveSentinel").unwrap();
        }
        Self(root)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vivac"));
        command
            .current_dir(self.0.join("project"))
            .env("VIVAC_HOME", self.0.join("home"))
            .env("HOME", self.0.join("profile"))
            .env("USERPROFILE", self.0.join("profile"))
            .env("CODEX_HOME", self.0.join("profile/.codex"))
            .env("TZ", "UTC")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn write(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    fn refused(&self, args: &[&str]) -> Output {
        let before = snapshot(&self.0);
        let output = self.run(args);
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(output.stdout.is_empty(), "unexpected public output");
        assert!(!output.stderr.is_empty(), "missing refusal");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fakeSensitiveSentinel"));
        assert_eq!(snapshot(&self.0), before, "refusal changed fixture");
        output
    }

    fn verify_refused(&self, path: &Path) {
        self.refused(&[
            "device-key",
            "verify-proof",
            path.to_str().unwrap(),
            "--json",
        ]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        let target = fs::canonicalize(&self.0).unwrap();
        assert!(target.starts_with(&temporary) && target != temporary);
        fs::remove_dir_all(target).unwrap();
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let linked = metadata.file_type().is_symlink();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if linked {
            files.insert(relative, b"link".to_vec());
        } else if metadata.is_dir() {
            files.insert(relative, b"directory".to_vec());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), files);
            }
        } else if metadata.is_file() {
            files.insert(relative, fs::read(path).unwrap());
        } else {
            files.insert(relative, b"special".to_vec());
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn signing() -> SigningKey {
    // RFC 8032 test vector 1; this public fixture never enters a credential store.
    SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

fn challenge() -> Value {
    let time = now();
    json!({
        "version": 1,
        "organization_id": "00000000000000000000000001",
        "principal_id": "00000000000000000000000002",
        "device_id": "00000000000000000000000003",
        "credential_id": "00000000000000000000000004",
        "public_key": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "operation_id": "00000000000000000000000005",
        "base_revision": 0x0102030405060708_u64,
        "nonce": "ab".repeat(32),
        "issued_at": time - 1,
        "expires_at": time + 299,
    })
}

fn signed(challenge: Value) -> Value {
    // Independent wire fixture: fixed RFC public key, nonce and identifiers.
    // Only the clock and revision vary for the cryptographically valid negative cases.
    let mut bytes = b"vivac.device.enrollment-proof.v1\0\
00000000000000000000000001000000000000000000000000020000000000000000000000000300000000000000000000000004".to_vec();
    bytes.extend_from_slice(signing().verifying_key().as_bytes());
    bytes.extend_from_slice(b"00000000000000000000000005");
    bytes.extend_from_slice(&challenge["base_revision"].as_u64().unwrap().to_be_bytes());
    bytes.extend_from_slice(&[0xab; 32]);
    bytes.extend_from_slice(&challenge["issued_at"].as_u64().unwrap().to_be_bytes());
    bytes.extend_from_slice(&challenge["expires_at"].as_u64().unwrap().to_be_bytes());
    assert_eq!(bytes.len(), 251);
    json!({
        "challenge": challenge,
        "signature": hex(&signing().sign(&bytes).to_bytes()),
        "status": "possession_only",
        "authority": "not_checked",
        "challenge_use": "not_checked",
    })
}

fn document(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let fields: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "authority",
            "challenge",
            "challenge_use",
            "signature",
            "status"
        ]
    );
    assert_eq!(value["status"], "possession_only");
    assert_eq!(value["authority"], "not_checked");
    assert_eq!(value["challenge_use"], "not_checked");
    let signature = value["signature"].as_str().unwrap();
    assert_eq!(signature.len(), 128);
    assert!(signature
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    value
}

#[test]
fn public_fixture_verifies_without_authority_or_consumption_and_output_is_reusable() {
    let fixture = Fixture::new("public");
    let expected = signed(challenge());
    let path = fixture.write("proof.json", &expected);
    let before = snapshot(&fixture.0);
    for _ in 0..2 {
        let output = fixture.run(&[
            "device-key",
            "verify-proof",
            path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(document(output), expected);
    }
    let output = fixture.run(&["device-key", "verify-proof", path.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("possession"));
    assert!(text.contains("not checked"));
    assert_eq!(snapshot(&fixture.0), before);
    let output = fixture.run(&[
        "device-key",
        "verify-proof",
        path.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{output:?}");
    let saved = fixture.0.join("saved.json");
    fs::write(&saved, output.stdout).unwrap();
    let before = snapshot(&fixture.0);
    for _ in 0..2 {
        assert_eq!(
            document(fixture.run(&[
                "device-key",
                "verify-proof",
                saved.to_str().unwrap(),
                "--json"
            ])),
            expected
        );
    }
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn every_signed_field_and_fixed_label_is_bound() {
    let fixture = Fixture::new("binding");
    let original = signed(challenge());
    for (field, replacement) in [
        ("version", json!(2)),
        ("organization_id", json!("00000000000000000000000006")),
        ("principal_id", json!("00000000000000000000000006")),
        ("device_id", json!("00000000000000000000000006")),
        ("credential_id", json!("00000000000000000000000006")),
        ("operation_id", json!("00000000000000000000000006")),
        (
            "public_key",
            json!(hex(SigningKey::from_bytes(&[1; 32])
                .verifying_key()
                .as_bytes())),
        ),
        ("base_revision", json!(1)),
        ("nonce", json!("ac".repeat(32))),
        (
            "issued_at",
            json!(original["challenge"]["issued_at"].as_u64().unwrap() + 1),
        ),
        (
            "expires_at",
            json!(original["challenge"]["expires_at"].as_u64().unwrap() - 1),
        ),
    ] {
        let mut changed = original.clone();
        changed["challenge"][field] = replacement;
        let path = fixture.write("changed.json", &changed);
        fixture.verify_refused(&path);
    }
    for field in ["status", "authority", "challenge_use"] {
        let mut changed = original.clone();
        changed[field] = json!("authorized_fakeSensitiveSentinel");
        let path = fixture.write("label.json", &changed);
        fixture.verify_refused(&path);
    }
    let mut changed = original;
    let mut signature = changed["signature"].as_str().unwrap().as_bytes().to_vec();
    signature[0] = if signature[0] == b'0' { b'1' } else { b'0' };
    changed["signature"] = json!(String::from_utf8(signature).unwrap());
    fixture.verify_refused(&fixture.write("signature.json", &changed));
}

#[test]
fn valid_signatures_do_not_bypass_clock_or_integer_bounds() {
    let fixture = Fixture::new("clock");
    let time = now();
    for (issued, expires) in [
        (time - 600, time - 300),
        (time + 300, time + 600),
        (time - 1, time + 300),
        (time, time),
        (time, time - 1),
        (0, u64::MAX),
        (u64::MAX - 299, u64::MAX),
        (u64::MAX, 0),
    ] {
        let mut request = challenge();
        request["issued_at"] = json!(issued);
        request["expires_at"] = json!(expires);
        fixture.verify_refused(&fixture.write("clock.json", &signed(request)));
    }
    let mut request = challenge();
    request["base_revision"] = json!(u64::MAX);
    let expected = signed(request);
    let path = fixture.write("revision.json", &expected);
    assert_eq!(
        document(fixture.run(&[
            "device-key",
            "verify-proof",
            path.to_str().unwrap(),
            "--json"
        ])),
        expected
    );
    for field in ["base_revision", "issued_at", "expires_at"] {
        let text = serde_json::to_string(&expected).unwrap();
        let original = format!("\"{field}\":{}", expected["challenge"][field]);
        let path = fixture.0.join("overflow.json");
        fs::write(
            &path,
            text.replace(&original, &format!("\"{field}\":18446744073709551616")),
        )
        .unwrap();
        fixture.verify_refused(&path);
    }
}

#[test]
fn strict_json_and_canonical_representations_refuse_without_echoing_payloads() {
    let fixture = Fixture::new("format");
    let original = signed(challenge());
    for (field, replacement) in [
        ("version", json!(0)),
        ("version", json!(1.0)),
        ("organization_id", json!("80000000000000000000000000")),
        ("principal_id", json!("0000000000000000000000000I")),
        ("device_id", json!("0000000000000000000000000é")),
        ("credential_id", json!("fakeSensitiveSentinel")),
        ("operation_id", json!("0000000000000000000000000A")),
        (
            "public_key",
            json!(original["challenge"]["public_key"]
                .as_str()
                .unwrap()
                .to_uppercase()),
        ),
        ("public_key", json!("00".repeat(32))),
        ("public_key", json!("ff".repeat(32))),
        ("nonce", json!("AB".repeat(32))),
        ("nonce", json!("ab".repeat(31))),
        ("base_revision", json!(-1)),
        ("issued_at", json!("fakeSensitiveSentinel")),
    ] {
        let mut changed = original.clone();
        changed["challenge"][field] = replacement;
        fixture.verify_refused(&fixture.write("format.json", &changed));
    }
    for signature in ["AA".repeat(64), "00".repeat(63), "gg".repeat(64)] {
        let mut changed = original.clone();
        changed["signature"] = json!(signature);
        fixture.verify_refused(&fixture.write("format.json", &changed));
    }
    let text = serde_json::to_string(&original).unwrap();
    for bytes in [
        text.replacen('{', "{\"extra\":\"fakeSensitiveSentinel\",", 1),
        text.replace(
            "\"version\":1",
            "\"version\":1,\"extra\":\"fakeSensitiveSentinel\"",
        ),
        text.replace("\"version\":1", "\"version\":1,\"version\":1"),
        text.replace(
            "\"status\":\"possession_only\"",
            "\"status\":\"possession_only\",\"status\":\"possession_only\"",
        ),
        format!("{text} {{\"fakeSensitiveSentinel\":1}}"),
        text.replace("\"version\":1", "\"version\":1e0"),
        "fakeSensitiveSentinel".into(),
        String::new(),
    ] {
        let path = fixture.0.join("invalid.json");
        fs::write(&path, bytes).unwrap();
        fixture.verify_refused(&path);
    }
    for field in original.as_object().unwrap().keys() {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        fixture.verify_refused(&fixture.write("missing.json", &missing));
    }
    for field in original["challenge"].as_object().unwrap().keys() {
        let mut missing = original.clone();
        missing["challenge"].as_object_mut().unwrap().remove(field);
        fixture.verify_refused(&fixture.write("missing.json", &missing));
    }
}

#[test]
fn proof_arguments_and_help_never_write() {
    let fixture = Fixture::new("arguments");
    let proof = fixture.write("proof.json", &signed(challenge()));
    let request = fixture.write("challenge.json", &challenge());
    let key = fixture.0.join("missing-key");
    let key = key.to_str().unwrap();
    let proof = proof.to_str().unwrap();
    let request = request.to_str().unwrap();
    for args in [
        vec!["device-key", "prove"],
        vec!["device-key", "verify-proof"],
        vec!["device-key", "prove", key],
        vec!["device-key", "prove", key, "--challenge"],
        vec!["device-key", "prove", key, "--challenge", ""],
        vec!["device-key", "prove", key, "--challenge", "relative"],
        vec!["device-key", "prove", "relative", "--challenge", request],
        vec![
            "device-key",
            "prove",
            key,
            "--challenge",
            request,
            "--challenge",
            request,
        ],
        vec![
            "device-key",
            "prove",
            key,
            "--challenge",
            request,
            "--unknown",
        ],
        vec!["device-key", "prove", key, "--challenge", request, "extra"],
        vec!["device-key", "verify-proof", ""],
        vec!["device-key", "verify-proof", "relative"],
        vec!["device-key", "verify-proof", proof, "--challenge", request],
        vec!["device-key", "verify-proof", proof, "--unknown"],
        vec!["device-key", "verify-proof", proof, "extra"],
    ] {
        fixture.refused(&args);
    }
    let before = snapshot(&fixture.0);
    document(fixture.run(&["device-key", "verify-proof", proof, "--json", "--json"]));
    for args in [
        vec!["device-key", "--help"],
        vec!["device-key", "prove", "--help"],
        vec!["device-key", "verify-proof", "--help"],
    ] {
        let output = fixture.run(&args);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("prove"));
        assert!(text.contains("verify-proof"));
    }
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn proof_inputs_refuse_directories_multiple_links_and_oversized_files() {
    let fixture = Fixture::new("files");
    let proof = fixture.write("proof.json", &signed(challenge()));
    fixture.verify_refused(&fixture.0.join("missing"));
    fixture.verify_refused(&fixture.0.join("project"));
    fs::hard_link(&proof, fixture.0.join("alias.json")).unwrap();
    fixture.verify_refused(&proof);
    fixture.verify_refused(&fixture.0.join("alias.json"));
    fs::remove_file(fixture.0.join("alias.json")).unwrap();
    let mut bytes = fs::read(&proof).unwrap();
    bytes.resize(8 * 1024, b' ');
    fs::write(&proof, &bytes).unwrap();
    let before = snapshot(&fixture.0);
    document(fixture.run(&[
        "device-key",
        "verify-proof",
        proof.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(snapshot(&fixture.0), before);
    bytes.push(b' ');
    fs::write(&proof, bytes).unwrap();
    fixture.verify_refused(&proof);
}

#[cfg(not(windows))]
#[test]
fn proving_requires_windows_before_credential_access() {
    let fixture = Fixture::new("unsupported");
    let request = fixture.write("challenge.json", &challenge());
    let invalid = fixture.0.join("invalid-key");
    fs::write(&invalid, b"fakeSensitiveSentinel").unwrap();
    for key in [invalid, fixture.0.join("missing/child")] {
        let output = fixture.refused(&[
            "device-key",
            "prove",
            key.to_str().unwrap(),
            "--challenge",
            request.to_str().unwrap(),
            "--json",
        ]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("requires Windows"));
    }
}

#[cfg(unix)]
#[test]
fn proof_symbolic_links_and_linked_ancestors_are_refused() {
    let fixture = Fixture::new("symlinks");
    let proof = fixture.write("proof.json", &signed(challenge()));
    let alias = fixture.0.join("alias.json");
    std::os::unix::fs::symlink(&proof, &alias).unwrap();
    fixture.verify_refused(&alias);
    fs::create_dir(fixture.0.join("inputs")).unwrap();
    fs::rename(&proof, fixture.0.join("inputs/proof.json")).unwrap();
    std::os::unix::fs::symlink(fixture.0.join("inputs"), fixture.0.join("linked")).unwrap();
    fixture.verify_refused(&fixture.0.join("linked/proof.json"));
}

#[cfg(unix)]
#[test]
fn fifo_input_is_refused_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let fixture = Fixture::new("fifo");
    let path = fixture.0.join("proof.fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: the terminated path is inside this test's temporary fixture.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let before = snapshot(&fixture.0);
    let mut child = fixture
        .command(&[
            "device-key",
            "verify-proof",
            path.to_str().unwrap(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("proof verification blocked on a FIFO");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert_eq!(snapshot(&fixture.0), before);
}

#[cfg(windows)]
fn create(fixture: &Fixture) -> (PathBuf, Value) {
    let key = fixture.0.join("key");
    let output = fixture.run(&["device-key", "create", key.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    (key, serde_json::from_slice(&output.stdout).unwrap())
}

#[cfg(windows)]
fn matching_challenge(receipt: &Value) -> Value {
    let mut value = challenge();
    for field in ["device_id", "credential_id", "public_key"] {
        value[field] = receipt[field].clone();
    }
    value
}

#[cfg(windows)]
#[test]
fn protected_key_proof_round_trip_preserves_package_and_application_paths() {
    let fixture = Fixture::new("protected");
    let (key, receipt) = create(&fixture);
    let expected = matching_challenge(&receipt);
    let request = fixture.write("challenge.json", &expected);
    let before = snapshot(&fixture.0);
    let output = fixture.run(&[
        "device-key",
        "prove",
        key.to_str().unwrap(),
        "--challenge",
        request.to_str().unwrap(),
        "--json",
        "--json",
    ]);
    let proof = document(output.clone());
    assert_eq!(proof["challenge"], expected);
    assert_eq!(snapshot(&fixture.0), before);
    let saved = fixture.0.join("proof.json");
    fs::write(&saved, output.stdout).unwrap();
    let before = snapshot(&fixture.0);
    for _ in 0..2 {
        assert_eq!(
            document(fixture.run(&[
                "device-key",
                "verify-proof",
                saved.to_str().unwrap(),
                "--json"
            ])),
            proof
        );
    }
    let text = fixture.run(&[
        "device-key",
        "prove",
        key.to_str().unwrap(),
        "--challenge",
        request.to_str().unwrap(),
    ]);
    assert!(text.status.success(), "{text:?}");
    assert!(text.stderr.is_empty());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("possession"));
    assert!(text.contains("not checked"));
    assert_eq!(snapshot(&fixture.0), before);
}

#[cfg(windows)]
#[test]
fn protected_key_refuses_mismatching_challenges_and_invalid_inputs_without_writes() {
    let fixture = Fixture::new("protected-refusal");
    let (key, receipt) = create(&fixture);
    for field in ["device_id", "credential_id", "public_key"] {
        let mut changed = matching_challenge(&receipt);
        changed[field] = challenge()[field].clone();
        let request = fixture.write("challenge.json", &changed);
        fixture.refused(&[
            "device-key",
            "prove",
            key.to_str().unwrap(),
            "--challenge",
            request.to_str().unwrap(),
            "--json",
        ]);
    }
    let request = fixture.write("challenge.json", &matching_challenge(&receipt));
    let alias = fixture.0.join("alias.json");
    fs::hard_link(&request, &alias).unwrap();
    for path in [&request, &alias, &fixture.0.join("project")] {
        fixture.refused(&[
            "device-key",
            "prove",
            key.to_str().unwrap(),
            "--challenge",
            path.to_str().unwrap(),
            "--json",
        ]);
    }
    fs::remove_file(alias).unwrap();
    let text = serde_json::to_string(&matching_challenge(&receipt)).unwrap();
    let mut padded = text.as_bytes().to_vec();
    padded.resize(8 * 1024, b' ');
    fs::write(&request, &padded).unwrap();
    let before = snapshot(&fixture.0);
    document(fixture.run(&[
        "device-key",
        "prove",
        key.to_str().unwrap(),
        "--challenge",
        request.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(snapshot(&fixture.0), before);
    padded.push(b' ');
    for bytes in [
        padded,
        b"{\"fakeSensitiveSentinel\":1}".to_vec(),
        format!("{text} {{}}").into_bytes(),
        text.replace("\"version\":1", "\"version\":1,\"version\":1")
            .into_bytes(),
        text.replacen('{', "{\"extra\":\"fakeSensitiveSentinel\",", 1)
            .into_bytes(),
        vec![0xff],
    ] {
        fs::write(&request, bytes).unwrap();
        fixture.refused(&[
            "device-key",
            "prove",
            key.to_str().unwrap(),
            "--challenge",
            request.to_str().unwrap(),
            "--json",
        ]);
    }
}

#[cfg(windows)]
#[test]
fn linked_proof_and_challenge_paths_are_refused() {
    let fixture = Fixture::new("linked-inputs");
    let (key, receipt) = create(&fixture);
    fs::create_dir(fixture.0.join("inputs")).unwrap();
    let proof = fixture.write("inputs/proof.json", &signed(challenge()));
    let request = fixture.write("inputs/challenge.json", &matching_challenge(&receipt));
    let junction = fixture.0.join("junction");
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(fixture.0.join("inputs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction fixture unsupported: {output:?}"
    );
    fixture.verify_refused(&junction.join("proof.json"));
    fixture.refused(&[
        "device-key",
        "prove",
        key.to_str().unwrap(),
        "--challenge",
        junction.join("challenge.json").to_str().unwrap(),
        "--json",
    ]);
    fs::remove_dir(junction).unwrap();
    for (source, alias, prove) in [
        (&proof, fixture.0.join("proof-link.json"), false),
        (&request, fixture.0.join("challenge-link.json"), true),
    ] {
        match std::os::windows::fs::symlink_file(source, &alias) {
            Ok(()) => {
                if prove {
                    fixture.refused(&[
                        "device-key",
                        "prove",
                        key.to_str().unwrap(),
                        "--challenge",
                        alias.to_str().unwrap(),
                        "--json",
                    ]);
                } else {
                    fixture.verify_refused(&alias);
                }
                fs::remove_file(alias).unwrap();
            }
            Err(error) if error.raw_os_error() == Some(1314) => {
                eprintln!("File symlink fixture unsupported: Windows requires the symlink privilege; junction and hard-link rejection are tested separately.");
            }
            Err(error) => panic!("creating file symlink fixture: {error}"),
        }
    }
}
