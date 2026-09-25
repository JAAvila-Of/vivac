//! `vivac update` (`d802`), run against copies of the real binary rather
//! than the real one -- Windows renames the executable it runs from, and
//! `CARGO_BIN_EXE_vivac` is what every other test, and this file's own
//! next run, still needs whole.

mod common;
#[cfg(windows)]
use common::Sandbox;
#[cfg(windows)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// A folder nothing else in the suite touches, removed on drop.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> TempRoot {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "vivac-update-{name}-{}-{n}-{ts}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempRoot(dir)
    }

    /// A copy of the compiled binary at `<root>/<sub>/<its own file name>`.
    fn exe_in(&self, sub: &str) -> PathBuf {
        let dir = self.0.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(Path::new(BIN).file_name().unwrap());
        std::fs::copy(BIN, &exe).unwrap();
        exe
    }

    fn write_crates2(&self, json: &str) {
        std::fs::write(self.0.join(".crates2.json"), json).unwrap();
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn run(exe: &Path, args: &[&str]) -> (String, i32) {
    let o = Command::new(exe).args(args).output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
        o.status.code().unwrap_or(-1),
    )
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// The `.crates2.json` shape cargo itself writes for one install: an
/// `installs` object keyed by `"<pkg> <version> (<source>)"`. The fields
/// besides the key are never read by `update`; they are here so the
/// fixture reads like a real file.
fn crates2_json(key: &str) -> String {
    let bin = if cfg!(windows) { "vivac.exe" } else { "vivac" };
    format!(r#"{{"installs":{{"{key}":{{"bins":["{bin}"],"profile":"release"}}}}}}"#)
}

#[test]
fn extra_words_or_flags_are_a_usage_error_and_touch_nothing() {
    let root = TempRoot::new("extra");
    let exe = root.exe_in("x");
    let before = entries(&root.0.join("x"));

    let (out, code) = run(&exe, &["update", "extra"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("update takes no arguments"), "{out}");
    assert_eq!(entries(&root.0.join("x")), before);

    let (out, code) = run(&exe, &["update", "--dry-run"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(entries(&root.0.join("x")), before);
}

#[test]
fn a_crates_io_registry_key_is_reported_as_such() {
    let root = TempRoot::new("crates-io");
    let exe = root.exe_in("bin");
    root.write_crates2(&crates2_json(
        "vivac 0.15.5 (registry+https://github.com/rust-lang/crates.io-index)",
    ));

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("installed by cargo install from crates.io"),
        "{out}"
    );
    assert!(out.lines().any(|l| l == "    cargo install vivac"), "{out}");
}

#[test]
fn a_git_key_names_the_matching_command() {
    let root = TempRoot::new("git");
    let exe = root.exe_in("bin");
    root.write_crates2(&crates2_json(
        "vivac 0.15.5 (git+https://example.com/vivac.git?branch=main#deadbeef)",
    ));

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("from https://example.com/vivac.git"), "{out}");
    assert!(
        out.lines()
            .any(|l| l == "    cargo install --git https://example.com/vivac.git vivac"),
        "{out}"
    );
}

#[test]
fn a_path_key_names_the_matching_command() {
    let root = TempRoot::new("path");
    let exe = root.exe_in("bin");
    #[cfg(windows)]
    let (key, expected) = (
        "vivac 0.15.5 (path+file:///H:/tmp/vivac%20src)",
        "\"H:/tmp/vivac src\"",
    );
    #[cfg(not(windows))]
    let (key, expected) = (
        "vivac 0.15.5 (path+file:///home/user/vivac%20src)",
        "\"/home/user/vivac src\"",
    );
    root.write_crates2(&crates2_json(key));

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("from a local folder"), "{out}");
    assert!(
        out.lines()
            .any(|l| l == format!("    cargo install --path {expected}")),
        "{out}"
    );
}

/// The same map `update::archive_name` reads (`.github/workflows/release.yml`),
/// kept here rather than reached into the binary: the two are the same
/// list on purpose, and a drift between them is exactly what this asserts.
fn expected_archive(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("vivac-x86_64-unknown-linux-musl.tar.gz"),
        ("linux", "aarch64") => Some("vivac-aarch64-unknown-linux-musl.tar.gz"),
        ("macos", "x86_64") => Some("vivac-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Some("vivac-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Some("vivac-x86_64-pc-windows-msvc.zip"),
        _ => None,
    }
}

#[test]
fn with_no_cargo_install_it_names_the_platform_archive_or_says_there_is_none() {
    let root = TempRoot::new("archive");
    let exe = root.exe_in("tools");

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    match expected_archive(std::env::consts::OS, std::env::consts::ARCH) {
        Some(name) => {
            assert!(out.contains(name), "{out}");
            assert!(
                out.contains("https://github.com/JAAvila-Of/vivac/releases/latest"),
                "{out}"
            );
        }
        None => {
            assert!(
                out.contains("There is no release archive for this platform"),
                "{out}"
            );
        }
    }
}

#[cfg(not(windows))]
#[test]
fn on_linux_and_macos_update_leaves_no_previous_or_next_file() {
    let root = TempRoot::new("no-set-aside");
    let exe = root.exe_in("bin");
    let dir = root.0.join("bin");

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        entries(&dir),
        vec![exe.file_name().unwrap().to_string_lossy().into_owned()]
    );
}

// ---------------------------------------------------------------------------
// Windows: setting the running copy aside (`f773`).
// ---------------------------------------------------------------------------

#[cfg(windows)]
struct HeldOpen {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

#[cfg(windows)]
impl HeldOpen {
    /// Starts `exe mcp` against `sandbox`, stdin held open, and waits for
    /// its own reply to the handshake before returning -- proof the
    /// process is alive and not merely spawned.
    fn start(exe: &Path, sandbox: &Sandbox) -> HeldOpen {
        let mut child = Command::new(exe)
            .current_dir(&sandbox.0)
            .env("VIVAC_HOME", sandbox.global_home())
            .env("TZ", "UTC")
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        writeln!(
            input,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}}}"#
        )
        .unwrap();
        input.flush().unwrap();
        let mut reply = String::new();
        output.read_line(&mut reply).unwrap();
        assert!(!reply.is_empty(), "the server never answered");
        HeldOpen {
            child,
            input,
            output,
        }
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Silences "unused" on the two fields that only exist to keep the
        // pipes open for as long as `self` lives.
        let _ = &mut self.input;
        let _ = &mut self.output;
    }
}

#[cfg(windows)]
fn previous_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".previous-")
        })
        .collect()
}

#[cfg(windows)]
fn next_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains(".next-"))
        .collect()
}

#[cfg(windows)]
#[test]
fn update_sets_the_running_copy_aside_and_frees_its_own_path() {
    let root = TempRoot::new("set-aside");
    let exe = root.exe_in("bin");
    let dir = root.0.join("bin");
    let sandbox = Sandbox::new_seeded("update-set-aside-cwd");

    let server = HeldOpen::start(&exe, &sandbox);

    // Before `update`, the running copy cannot be overwritten.
    assert!(
        std::fs::copy(BIN, &exe).is_err(),
        "the copy should have been locked while the server ran"
    );

    let (out, code) = run(&exe, &["update"]);
    assert_eq!(code, 0, "{out}");
    let first_previous = previous_files(&dir);
    assert_eq!(first_previous.len(), 1, "{first_previous:?}");
    assert!(next_files(&dir).is_empty());

    let (version_out, version_code) = run(&exe, &["--version"]);
    assert_eq!(version_code, 0, "{version_out}");
    assert!(version_out.starts_with("vivac "), "{version_out}");

    // The path `update` freed now accepts a fresh copy -- it would have
    // failed with the same os error 5 above before this ran.
    std::fs::copy(BIN, &exe).expect("the freed path should accept a new copy");

    let (out2, code2) = run(&exe, &["update"]);
    assert_eq!(code2, 0, "{out2}");
    assert!(
        out2.contains("1 copy set aside by an earlier update is still running"),
        "{out2}"
    );
    assert!(
        previous_files(&dir).contains(&first_previous[0]),
        "the first .previous- did not survive: {:?}",
        previous_files(&dir)
    );

    server.stop();

    let (out3, code3) = run(&exe, &["update"]);
    assert_eq!(code3, 0, "{out3}");
    assert!(
        !previous_files(&dir).contains(&first_previous[0]),
        "the first .previous- is still there: {:?}",
        previous_files(&dir)
    );
}
