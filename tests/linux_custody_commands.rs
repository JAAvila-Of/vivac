#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::{Command, Stdio};

const CREDENTIAL: &str = "00000000000000000000000001";

fn command(operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vivac"));
    command.args(["device-key", operation]);
    command
}

fn unattended_command() -> Command {
    use std::os::unix::process::CommandExt;

    let mut invocation = command("open-linux");
    invocation.args([CREDENTIAL, "--json"]).stdin(Stdio::null());
    // SAFETY: setsid is async-signal-safe and the child owns its new session.
    unsafe {
        invocation.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    invocation
}

#[test]
fn custody_commands_reject_malformed_inputs_before_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_vivac"))
        .arg("device-key")
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"Specify a valid device-key command.\n");

    for operation in ["open-linux", "close-linux", "verify-linux-possession"] {
        for arguments in [
            vec![],
            vec![""],
            vec!["0000000000000000000000000"],
            vec!["80000000000000000000000001"],
            vec!["0000000000000000000000000I"],
            vec!["0000000000000000000000000l"],
            vec!["/tmp/custody"],
            vec![CREDENTIAL, "extra"],
            vec![CREDENTIAL, "--json", "--json"],
            vec![CREDENTIAL, "--json=true"],
            vec![CREDENTIAL, "--path", "/tmp/custody"],
        ] {
            let output = command(operation).args(&arguments).output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{operation} {arguments:?}");
            assert!(output.stdout.is_empty(), "{operation} {arguments:?}");
            assert!(!output.stderr.is_empty(), "{operation} {arguments:?}");
            assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
        }
        let output = command(operation)
            .arg(OsString::from_vec(vec![0xff]))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}

#[test]
fn unattended_opening_returns_a_final_refusal_receipt() {
    let output = unattended_command().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({"credential_id": CREDENTIAL, "status": "refused"})
    );
}

#[test]
fn failed_receipt_delivery_returns_io_failure() {
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = unattended_command().stdout(full).output().unwrap();
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(output.stderr, b"Linux custody receipt delivery failed.\n");
}

#[test]
fn opening_limits_refuse_lengthening_values() {
    let output = unattended_command()
        .env("VIVAC_LINUX_SESSION_IDLE_SECONDS", "43201")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn device_key_help_advertises_linux_custody_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_vivac"))
        .args(["device-key", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("vivac device-key open-linux <credential-id> [--json]"));
    assert!(help.contains("vivac device-key close-linux <credential-id> [--json]"));
    assert!(help.contains("vivac device-key verify-linux-possession <credential-id> [--json]"));
}

#[test]
fn close_without_a_provisioned_store_is_refused() {
    let output = command("close-linux")
        .args([CREDENTIAL, "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({"credential_id": CREDENTIAL, "status": "refused"})
    );
}
