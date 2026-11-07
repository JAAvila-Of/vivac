mod common;

use common::Sandbox;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    fs::create_dir_all(sandbox.global_home()).unwrap();
    for name in ["profile", "installed", "project/.vivac", "project/.claude"] {
        fs::create_dir_all(sandbox.0.join(name)).unwrap();
    }
    for name in [
        "installed/vivac.exe",
        "project/.vivac/events",
        "project/.vivac/config",
        "project/.claude/settings.json",
        "project/.mcp.json",
        "profile/credentials",
    ] {
        fs::write(sandbox.0.join(name), b"synthetic sentinel").unwrap();
    }
    fs::write(
        sandbox.global_home().join("registry"),
        b"synthetic registry",
    )
    .unwrap();
    sandbox
}

fn run(sandbox: &Sandbox, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(sandbox.0.join("project"))
        .env("VIVAC_HOME", sandbox.global_home())
        .env("HOME", sandbox.0.join("profile"))
        .env("USERPROFILE", sandbox.0.join("profile"))
        .env("CODEX_HOME", sandbox.0.join("profile/.codex"))
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let linked = metadata.file_type().is_symlink();
        if linked {
            files.insert(relative, b"link".to_vec());
        } else if metadata.is_dir() {
            files.insert(relative, b"directory".to_vec());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), files);
            }
        } else {
            files.insert(relative, fs::read(path).unwrap());
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn refused(sandbox: &Sandbox, args: &[&str]) -> Output {
    let before = snapshot(&sandbox.0);
    let home = snapshot(sandbox.global_home());
    let output = run(sandbox, args);
    assert!(!output.status.success(), "accepted {args:?}");
    assert!(output.stdout.is_empty(), "unexpected public output");
    assert!(!output.stderr.is_empty(), "missing refusal");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic"));
    assert_eq!(snapshot(&sandbox.0), before, "refusal changed files");
    assert_eq!(
        snapshot(sandbox.global_home()),
        home,
        "refusal changed home"
    );
    output
}

#[test]
fn malformed_requests_and_help_never_write() {
    let sandbox = fixture("device-key-arguments");
    let destination = sandbox.0.join("key");
    let path = destination.to_str().unwrap();
    for args in [
        vec!["device-key"],
        vec!["device-key", "unknown"],
        vec!["device-key", "create"],
        vec!["device-key", "show"],
        vec!["device-key", "create", ""],
        vec!["device-key", "create", "relative"],
        vec!["device-key", "show", "relative"],
        vec!["device-key", "create", path, "--unknown"],
        vec!["device-key", "show", path, "--unknown"],
        vec!["device-key", "create", path, "extra"],
        vec!["device-key", "show", path, "extra"],
        vec!["device-key", "--help", "extra"],
        vec!["device-key", "--help", "--unknown"],
        vec!["device-key", "create", path, "--help"],
    ] {
        refused(&sandbox, &args);
    }
    let before = snapshot(&sandbox.0);
    let home = snapshot(sandbox.global_home());
    for args in [
        vec!["device-key", "--help"],
        vec!["device-key", "create", "--help"],
        vec!["device-key", "show", "--help"],
    ] {
        let output = run(&sandbox, &args);
        assert!(output.status.success(), "{output:?}");
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("create <absolute-new-directory>"));
        assert!(help.contains("show <absolute-directory>"));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(snapshot(&sandbox.0), before);
    assert_eq!(snapshot(sandbox.global_home()), home);
}

#[cfg(not(windows))]
#[test]
fn unsupported_platform_refuses_before_credential_access() {
    let sandbox = fixture("device-key-unsupported");
    let invalid = sandbox.0.join("invalid");
    fs::write(&invalid, b"synthetic invalid package").unwrap();
    for destination in [invalid, sandbox.0.join("missing/child")] {
        for operation in ["create", "show"] {
            let output = refused(
                &sandbox,
                &[
                    "device-key",
                    operation,
                    destination.to_str().unwrap(),
                    "--json",
                ],
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("requires Windows"));
        }
    }
    let output = run(&sandbox, &["--help"]);
    assert!(output.status.success());
}

#[cfg(windows)]
fn receipt(output: Output) -> serde_json::Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let object = value.as_object().unwrap();
    let mut fields: Vec<_> = object.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "credential_id",
            "device_id",
            "protection",
            "public_key",
            "status"
        ]
    );
    assert_eq!(value["protection"], "windows_current_user_dpapi");
    assert_eq!(value["status"], "local_key_only");
    let key = value["public_key"].as_str().unwrap();
    assert_eq!(key.len(), 64);
    assert!(key
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    for field in ["device_id", "credential_id"] {
        let id = value[field].as_str().unwrap();
        assert_eq!(id.len(), 26);
        assert!((b'0'..=b'7').contains(&id.as_bytes()[0]));
        assert!(id
            .bytes()
            .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte)));
    }
    assert_ne!(value["device_id"], value["credential_id"]);
    value
}

#[cfg(windows)]
fn create(sandbox: &Sandbox, name: &str) -> (PathBuf, serde_json::Value) {
    let path = sandbox.0.join(name);
    let value = receipt(run(
        sandbox,
        &["device-key", "create", path.to_str().unwrap(), "--json"],
    ));
    (path, value)
}

#[cfg(windows)]
#[test]
fn independent_credentials_reload_in_new_processes_without_other_writes() {
    let sandbox = fixture("device-key-reload");
    let before = snapshot(&sandbox.0);
    let home = snapshot(sandbox.global_home());
    let (first, first_receipt) = create(&sandbox, "first");
    let (second, second_receipt) = create(&sandbox, "second");
    for field in ["device_id", "credential_id", "public_key"] {
        assert_ne!(first_receipt[field], second_receipt[field]);
    }
    for (path, expected) in [(&first, first_receipt), (&second, second_receipt)] {
        let stored = snapshot(path);
        assert_eq!(stored.len(), 2, "package must have exactly one file");
        let bytes = fs::read(path.join("device-key.bin")).unwrap();
        assert!(!bytes.is_empty() && bytes.len() <= 16 * 1024);
        let loaded = receipt(run(
            &sandbox,
            &["device-key", "show", path.to_str().unwrap(), "--json"],
        ));
        assert_eq!(loaded, expected);
        assert_eq!(snapshot(path), stored);
        let text = run(&sandbox, &["device-key", "show", path.to_str().unwrap()]);
        assert!(text.status.success());
        assert!(text.stderr.is_empty());
        let text = String::from_utf8(text.stdout).unwrap();
        assert!(text.contains(expected["public_key"].as_str().unwrap()));
        assert!(text.contains("local key only"));
    }
    let root = fs::canonicalize(&sandbox.0).unwrap();
    for path in [first, second] {
        let target = fs::canonicalize(path).unwrap();
        assert!(target.starts_with(&root) && target != root);
        fs::remove_dir_all(target).unwrap();
    }
    assert_eq!(snapshot(&sandbox.0), before);
    assert_eq!(snapshot(sandbox.global_home()), home);
}

#[cfg(windows)]
#[test]
fn creation_refuses_existing_missing_parent_and_guarded_destinations() {
    let sandbox = fixture("device-key-destination");
    fs::create_dir(sandbox.0.join("existing")).unwrap();
    fs::write(sandbox.0.join("existing/sentinel"), b"preserve").unwrap();
    fs::write(sandbox.0.join("existing-file"), b"preserve").unwrap();
    fs::create_dir_all(sandbox.0.join("git/.git")).unwrap();
    fs::create_dir(sandbox.0.join("worktree")).unwrap();
    fs::write(sandbox.0.join("worktree/.git"), b"gitdir: synthetic").unwrap();
    fs::create_dir_all(sandbox.0.join("other/.vivac")).unwrap();
    for destination in [
        ".git",
        ".GIT",
        ".git.",
        ".vivac",
        ".ViVaC",
        ".vivac. ",
        "existing",
        "existing-file",
        "missing/key",
        "git/key",
        "git/.git/key",
        "worktree/key",
        "other/.vivac/key",
    ] {
        let path = sandbox.0.join(destination);
        refused(
            &sandbox,
            &["device-key", "create", path.to_str().unwrap(), "--json"],
        );
    }
    let (path, _) = create(&sandbox, "complete");
    refused(
        &sandbox,
        &["device-key", "create", path.to_str().unwrap(), "--json"],
    );
    for destination in ["git/moved", "other/.vivac/moved"] {
        let moved = sandbox.0.join(destination);
        fs::rename(&path, &moved).unwrap();
        refused(
            &sandbox,
            &["device-key", "show", moved.to_str().unwrap(), "--json"],
        );
        fs::rename(moved, &path).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn damaged_or_extended_packages_are_never_repaired() {
    let sandbox = fixture("device-key-damage");
    for damage in [
        "empty",
        "truncated",
        "wrapper",
        "length",
        "trailing",
        "changed",
        "oversized",
        "extra",
        "missing",
        "hard-link",
    ] {
        let (path, _) = create(&sandbox, damage);
        let file = path.join("device-key.bin");
        let mut bytes = fs::read(&file).unwrap();
        match damage {
            "empty" => fs::write(&file, []).unwrap(),
            "truncated" => fs::write(&file, &bytes[..bytes.len() / 2]).unwrap(),
            "wrapper" => {
                bytes[0] ^= 1;
                fs::write(&file, bytes).unwrap();
            }
            "length" => {
                let offset = b"vivac.dpapi-key.v1\0".len();
                bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
                fs::write(&file, bytes).unwrap();
            }
            "trailing" => {
                bytes.push(0);
                fs::write(&file, bytes).unwrap();
            }
            "changed" => {
                let index = bytes.len() / 2;
                bytes[index] ^= 1;
                fs::write(&file, bytes).unwrap();
            }
            "oversized" => fs::write(&file, vec![b'X'; 16 * 1024 + 1]).unwrap(),
            "extra" => fs::write(path.join("extra"), b"synthetic extra member").unwrap(),
            "missing" => fs::remove_file(&file).unwrap(),
            "hard-link" => fs::hard_link(&file, sandbox.0.join("alias")).unwrap(),
            _ => unreachable!(),
        }
        refused(
            &sandbox,
            &["device-key", "show", path.to_str().unwrap(), "--json"],
        );
    }
}

#[cfg(windows)]
#[test]
fn junction_packages_and_ancestors_are_refused() {
    let sandbox = fixture("device-key-junction");
    let (target, _) = create(&sandbox, "target");
    let alias = sandbox.0.join("alias");
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&alias)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction fixture unsupported: {output:?}"
    );
    refused(
        &sandbox,
        &["device-key", "show", alias.to_str().unwrap(), "--json"],
    );
    let child = alias.join("child");
    refused(
        &sandbox,
        &["device-key", "create", child.to_str().unwrap(), "--json"],
    );
    fs::remove_dir(alias).unwrap();
}

#[cfg(windows)]
#[test]
fn symbolic_links_are_refused_when_the_platform_allows_the_fixture() {
    let sandbox = fixture("device-key-symlink");
    let (target, _) = create(&sandbox, "target");
    let alias = sandbox.0.join("alias");
    match std::os::windows::fs::symlink_dir(&target, &alias) {
        Ok(()) => {
            refused(
                &sandbox,
                &["device-key", "show", alias.to_str().unwrap(), "--json"],
            );
            fs::remove_dir(alias).unwrap();
        }
        Err(error) if error.raw_os_error() == Some(1314) => {
            eprintln!("Directory symlink fixture unsupported: Windows requires the symlink privilege; junction rejection is tested separately.");
        }
        Err(error) => panic!("creating directory symlink fixture: {error}"),
    }
    let (package, _) = create(&sandbox, "file-link");
    let source = target.join("device-key.bin");
    let file = package.join("device-key.bin");
    fs::remove_file(&file).unwrap();
    match std::os::windows::fs::symlink_file(source, &file) {
        Ok(()) => {
            refused(
                &sandbox,
                &["device-key", "show", package.to_str().unwrap(), "--json"],
            );
            fs::remove_file(file).unwrap();
        }
        Err(error) if error.raw_os_error() == Some(1314) => {
            eprintln!("File symlink fixture unsupported: Windows requires the symlink privilege; hard-link rejection is tested separately.");
        }
        Err(error) => panic!("creating file symlink fixture: {error}"),
    }
}

#[cfg(windows)]
fn broaden_private_acl(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    let text: Vec<u16> = "D:P(A;OICI;FA;;;WD)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    let mut acl = ptr::null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    // SAFETY: strings are terminated and the descriptor stays live during the ACL update.
    unsafe {
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut()
            ),
            0
        );
        assert_ne!(
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted),
            0
        );
        let result = SetNamedSecurityInfoW(
            name.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        );
        LocalFree(descriptor);
        assert_eq!(result, 0);
    }
}

#[cfg(windows)]
#[test]
fn broad_directory_and_file_permissions_are_refused_without_repair() {
    let sandbox = fixture("device-key-permissions");
    for file in [false, true] {
        let (path, _) = create(&sandbox, if file { "file" } else { "directory" });
        broaden_private_acl(&if file {
            path.join("device-key.bin")
        } else {
            path.clone()
        });
        refused(
            &sandbox,
            &["device-key", "show", path.to_str().unwrap(), "--json"],
        );
    }
}
