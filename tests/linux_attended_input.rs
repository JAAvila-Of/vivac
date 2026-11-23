#![cfg(target_os = "linux")]

// The disposable helpers exercise terminal interaction with a foreground PTY.
// This target exercises the input and encryption code in Vivac's test graph.
#[allow(dead_code)]
#[path = "../src/device_keys/linux_attended_terminal.rs"]
mod attended_terminal;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_attended_creation_input.rs"]
mod creation_input;
#[allow(dead_code)]
#[path = "../src/device_keys/linux_creation_ciphertext.rs"]
mod package_ciphertext;

#[test]
fn internal_linux_startup_refuses_uninherited_and_noncanonical_descriptors() {
    for arguments in [
        &["--internal-linux-keeper", "unexpected"][..],
        &["--internal-linux-supervisor"][..],
        &["--internal-linux-supervisor", "0"][..],
        &["--internal-linux-supervisor", "03"][..],
        &["--internal-linux-supervisor", "2147483647"][..],
        &["--internal-linux-supervisor", "3", "unexpected"][..],
        &["--internal-linux-verifier"][..],
        &["--internal-linux-verifier", "03"][..],
        &["--internal-linux-verifier", "2147483647"][..],
        &["--internal-linux-verifier", "3", "unexpected"][..],
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(arguments)
            .output()
            .expect("run the product binary");
        assert_eq!(result.status.code(), Some(1), "{arguments:?}");
        assert!(result.stdout.is_empty(), "{arguments:?}");
        assert_eq!(result.stderr, b"Linux custody startup refused.\n");
    }
}

#[test]
fn generated_secret_encoding_protects_an_openssh_package_key() {
    use ssh_key::{Cipher, Kdf, PrivateKey};

    let comment = "vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002";
    let password = [b'a'; 64];
    package_ciphertext::validate_comment(comment).expect("valid credential comment");
    let encoded = package_ciphertext::serialize_encrypted(comment, &password)
        .expect("produce encrypted OpenSSH key");
    let key = PrivateKey::from_openssh(encoded.as_bytes()).expect("parse encrypted OpenSSH key");
    assert!(key.is_encrypted());
    assert_eq!(key.cipher(), Cipher::Aes256Gcm);
    assert!(matches!(key.kdf(), Kdf::Bcrypt { salt, rounds } if salt.len() == 16 && *rounds == 64));
    let public = key.public_key().key_data().clone();
    let decrypted = key
        .decrypt(password)
        .expect("decrypt with the fixture secret");
    assert_eq!(decrypted.comment(), comment);
    assert_eq!(decrypted.public_key().key_data(), &public);
}

#[test]
fn path_based_creation_refuses_linux() {
    let directory =
        std::env::temp_dir().join(format!("vivac-linux-attended-input-{}", std::process::id()));
    assert!(!directory.exists());
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .args(["device-key", "create"])
        .arg(&directory)
        .arg("--json")
        .output()
        .expect("run the public device-key command");
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("Device key refused: protected device-key storage requires Windows or macOS."));
    assert!(!directory.exists());
}

#[test]
fn fixed_store_creation_refuses_caller_paths_and_ids() {
    for extra in [
        vec!["/tmp/vivac-key"],
        vec!["--credential-id", "00000000000000000000000001"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(["device-key", "create-linux", "--json"])
            .args(extra)
            .output()
            .expect("run the Linux device-key command");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn linux_provision_preview_lists_the_complete_numeric_slot() {
    let command = |arguments: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(["device-key", "provision-linux"])
            .args(arguments)
            .output()
            .expect("run Linux provisioning command")
    };
    let output = command(&["--uid", "65534", "--gid", "65533", "--json"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["status"], "plan");
    let entries = plan["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 11);
    assert_eq!(
        entries[9],
        serde_json::json!({
            "path": "/var/lib/vivac/custody-v1/users/65534/state/endpoints-v1",
            "uid": 65534, "gid": 65533, "mode": "0700", "action": "create",
        })
    );
    assert_eq!(
        entries[10],
        serde_json::json!({
            "path": "/var/lib/vivac/custody-v1/users/65534/state/sessions-v1",
            "uid": 65534, "gid": 65533, "mode": "0700", "action": "create",
        })
    );
    for arguments in [
        &["--uid", "65534", "--gid", "0", "--yes"][..],
        &[
            "--uid", "65534", "--uid", "65533", "--gid", "65534", "--yes",
        ][..],
        &["--uid", "065534", "--gid", "65534", "--yes"][..],
        &[
            "--uid", "65534", "--gid", "65534", "--path", "/tmp", "--yes",
        ][..],
    ] {
        let output = command(arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn endpoint_provision_refuses_caller_paths_and_ids() {
    for arguments in [
        &["/tmp/vivac-endpoints", "--yes"][..],
        &["--uid", "65534", "--yes"][..],
        &["--gid", "65534", "--yes"][..],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(["device-key", "provision-linux-endpoints"])
            .args(arguments)
            .output()
            .expect("run Linux endpoint provisioning command");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn session_provision_refuses_caller_paths_and_ids() {
    for arguments in [
        &["/tmp/vivac-sessions", "--yes"][..],
        &["--uid", "65534", "--yes"][..],
        &["--gid", "65534", "--yes"][..],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(["device-key", "provision-linux-sessions"])
            .args(arguments)
            .output()
            .expect("run Linux session provisioning command");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
