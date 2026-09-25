//! `vivac update` -- how this vivac was installed, and, with a person at a
//! terminal to ask, the command that installs it (`d802`, `d815`).
//!
//! `d802`'s own rule still holds when nobody is there to ask: with stdin not
//! a terminal -- an agent, a pipe, a script, every one of this file's own
//! tests -- this never installs anything and never touches the network,
//! only says what to type. `d815` adds the other half. With a real terminal
//! behind stdin, it plans first, asks `Install it? [y/N] `, and only a `y`
//! does the install: a cargo command it spawns itself, or a release
//! archive it downloads with `curl`, checks against `SHA256SUMS` and
//! unpacks with `tar` -- the system's own tools, spawned directly with an
//! argument vector, never through a shell. There is no `--yes`: installing
//! vivac is not an agent's job, and a script already has `cargo` itself.
//!
//! It needs no tree, and `main.rs` dispatches it before any store lookup
//! for that reason, the same as `--version`, `init` and `setup`. It is
//! never exposed through MCP, for the same reason those two are not.
//!
//! On Windows an executable in use cannot be overwritten or deleted, only
//! renamed (`f773`, measured), so every install here -- cargo's or the
//! archive's -- sets the copy that is running aside first, through
//! `windows_set_aside` below. On Linux and macOS nothing is set aside:
//! replacing a running binary already works there.

use crate::failure::Failure;
use crate::output::outln;
use crate::plan::{heading, render_items, PlanItem};
use crate::style::{self, Stream};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CRATES_IO_REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
const CRATES_IO_SPARSE: &str = "sparse+https://index.crates.io/";

const RELEASES_LATEST: &str = "https://github.com/JAAvila-Of/vivac/releases/latest";
const DOWNLOAD_BASE: &str = "https://github.com/JAAvila-Of/vivac/releases/latest/download";

const NO_ARCHIVE_TEXT: &str =
    "There is no release archive for this platform. With a Rust toolchain, \
     cargo install vivac builds it.";

/// What `.crates2.json`'s own key for `vivac` resolves to: the command that
/// reproduces the same install, and the two phrases that name its source --
/// one for the identification sentence (`described_as`, unchanged since
/// `d802`), one for the plan's own `run` row (`plan_source`, `d815`), which
/// reads after a "from" the row already carries and so drops the leading
/// "from " `described_as` needs for its own sentence.
struct CargoInstall {
    command: String,
    described_as: String,
    plan_source: String,
}

/// The text inside the last `( ... )` of a `.crates2.json` `installs` key
/// -- `"vivac 0.15.5 (registry+https://...)"` names its source there, and
/// neither the package name nor the version can hold a parenthesis of
/// their own.
fn key_source(key: &str) -> Option<&str> {
    let open = key.rfind('(')?;
    let close = key.rfind(')')?;
    if close <= open {
        return None;
    }
    Some(&key[open + 1..close])
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `%XX` decoded back to the byte it names. Hand-rolled rather than pulled
/// in: a `path+file://` source is the only percent-encoded text this binary
/// ever reads, and it is a handful of lines.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `path+file://`'s own leading slash before a drive letter: cargo writes
/// `path+file:///H:/x` for a path on `H:`, and `H:/x` -- not `/H:/x` -- is
/// what `cargo install --path` actually wants back. Unix paths keep their
/// own leading slash: it is the root, not a drive marker.
#[cfg(windows)]
fn strip_drive_slash(p: &str) -> String {
    let bytes = p.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        p[1..].to_string()
    } else {
        p.to_string()
    }
}

#[cfg(not(windows))]
fn strip_drive_slash(p: &str) -> String {
    p.to_string()
}

/// `key_source`'s text, read into the command that reproduces the same
/// install and the two phrases that name it. `None` for a source this does
/// not recognise -- the caller falls back to naming the release archive
/// instead.
fn classify_source(source: &str) -> Option<CargoInstall> {
    if source == CRATES_IO_REGISTRY || source == CRATES_IO_SPARSE {
        return Some(CargoInstall {
            command: "cargo install vivac".to_string(),
            described_as: "from crates.io".to_string(),
            plan_source: "crates.io".to_string(),
        });
    }
    if let Some(url) = source.strip_prefix("registry+") {
        return Some(CargoInstall {
            command: format!("cargo install vivac --index {url}"),
            described_as: format!("from the registry at {url}"),
            plan_source: format!("the registry at {url}"),
        });
    }
    if source.starts_with("sparse+") {
        return Some(CargoInstall {
            command: format!("cargo install vivac --index {source}"),
            described_as: format!("from the registry at {source}"),
            plan_source: format!("the registry at {source}"),
        });
    }
    if let Some(rest) = source.strip_prefix("git+") {
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        let url = &rest[..end];
        return Some(CargoInstall {
            command: format!("cargo install --git {url} vivac"),
            described_as: format!("from {url}"),
            plan_source: url.to_string(),
        });
    }
    if let Some(raw) = source.strip_prefix("path+file://") {
        let path = strip_drive_slash(&percent_decode(raw));
        // Quoted when it holds a space, so the line still pastes as one
        // command: double quotes read the same in PowerShell, cmd.exe and a
        // POSIX shell.
        let path = if path.contains(char::is_whitespace) {
            format!("\"{path}\"")
        } else {
            path
        };
        return Some(CargoInstall {
            command: format!("cargo install --path {path}"),
            described_as: "from a local folder".to_string(),
            plan_source: format!("the local folder {path}"),
        });
    }
    None
}

/// Whether `.crates2.json`'s own text names a cargo install of `vivac`, and
/// how. `None` for malformed JSON, a shape this does not expect, a file
/// with no `vivac` key, or a key whose source `classify_source` does not
/// recognise -- every one of those reads the same to the caller: not a
/// cargo install.
fn cargo_install_from_json(json: &str) -> Option<CargoInstall> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let installs = v.get("installs")?.as_object()?;
    let key = installs
        .keys()
        .find(|k| k.split_whitespace().next() == Some("vivac"))?;
    classify_source(key_source(key)?)
}

/// `d802`: a cargo install lives at `<prefix>/bin/vivac[.exe]`, with
/// `.crates2.json` one folder up from `bin`. Reading the file here, rather
/// than folding the check into `cargo_install_from_json`, keeps every
/// branch above testable against a literal string with no filesystem
/// involved at all.
fn detect_cargo_install(dir: &Path) -> Option<CargoInstall> {
    if dir.file_name() != Some(std::ffi::OsStr::new("bin")) {
        return None;
    }
    let json = std::fs::read_to_string(dir.parent()?.join(".crates2.json")).ok()?;
    cargo_install_from_json(&json)
}

/// The release archive `.github/workflows/release.yml` builds for `os` and
/// `arch`, or `None` where the workflow builds nothing for that pair.
fn archive_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("vivac-x86_64-unknown-linux-musl.tar.gz"),
        ("linux", "aarch64") => Some("vivac-aarch64-unknown-linux-musl.tar.gz"),
        ("macos", "x86_64") => Some("vivac-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Some("vivac-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Some("vivac-x86_64-pc-windows-msvc.zip"),
        _ => None,
    }
}

/// The binary's own name inside a release archive: `vivac.exe` on Windows,
/// `vivac` everywhere else.
fn exe_noun() -> &'static str {
    if cfg!(windows) {
        "vivac.exe"
    } else {
        "vivac"
    }
}

/// Setting the running copy aside: Windows only (`f773`). An executable in
/// use cannot be overwritten or deleted there, but it can be renamed, and
/// every one of the three renames below rolls back on its own failure so
/// that "Nothing was changed" stays true of whatever `update` prints next.
#[cfg(windows)]
mod windows_set_aside {
    use crate::failure::Failure;
    use std::path::Path;

    /// Sweeps stale `.previous-`/`.next-` copies next to `exe`, then moves
    /// the running copy out of the way: copy `exe` to a fresh `.next-`,
    /// rename `exe` itself to `.previous-`, rename `.next-` onto `exe`'s own
    /// path. Returns how many `.previous-` files the sweep found still
    /// running, for `update`'s own "still running" paragraph.
    pub(super) fn set_aside(exe: &Path, dir: &Path, stem: &str) -> Result<usize, Failure> {
        set_aside_excluding(exe, dir, stem, None)
    }

    /// [`set_aside`], except the sweep skips any entry whose name starts
    /// with `exclude_prefix`: `install_from_archive`'s own call reaches
    /// this once its own `<stem>.download-<ulid>...` work files already
    /// exist on disk, live and still needed, and a sweep with no way to
    /// tell them apart from a crash's leftovers would delete its own
    /// still-open download out from under it (`d815`).
    pub(super) fn set_aside_excluding(
        exe: &Path,
        dir: &Path,
        stem: &str,
        exclude_prefix: Option<&str>,
    ) -> Result<usize, Failure> {
        let still_running = sweep(dir, stem, exclude_prefix);
        let id = crate::id::ulid();
        let next = dir.join(format!("{stem}.next-{id}.exe"));
        let previous = dir.join(format!("{stem}.previous-{id}.exe"));

        if let Err(e) = std::fs::copy(exe, &next) {
            let _ = std::fs::remove_file(&next);
            return Err(Failure::set_aside(e));
        }
        if let Err(e) = std::fs::rename(exe, &previous) {
            let _ = std::fs::remove_file(&next);
            return Err(Failure::set_aside(e));
        }
        if let Err(e) = std::fs::rename(&next, exe) {
            let _ = std::fs::rename(&previous, exe);
            let _ = std::fs::remove_file(&next);
            return Err(Failure::set_aside(e));
        }
        Ok(still_running)
    }

    /// Removes every `<stem>.previous-*.exe` and `<stem>.next-*.exe` next
    /// to `exe`, left over from an earlier `update`. A `.next-` found here
    /// is always a leftover -- `set_aside` renames its own away before
    /// returning -- and a `.previous-` a session still has open cannot be
    /// removed yet; those are counted and returned. Every other file in
    /// `dir` is ignored, and a failure here never aborts the sweep.
    ///
    /// Also removes a stale `<stem>.download-*` file or folder, left behind
    /// by a release-archive install that crashed before its own cleanup ran
    /// (`d815`): the same kind of leftover as a `.next-`, just from the
    /// other install path. `exclude_prefix` skips a download this same
    /// call is still using -- see [`set_aside_excluding`].
    fn sweep(dir: &Path, stem: &str, exclude_prefix: Option<&str>) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let previous_prefix = format!("{stem}.previous-");
        let next_prefix = format!("{stem}.next-");
        let download_prefix = format!("{stem}.download-");
        let mut still_running = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if exclude_prefix.is_some_and(|p| name.starts_with(p)) {
                continue;
            }
            let is_previous = name.starts_with(&previous_prefix) && name.ends_with(".exe");
            let is_next = name.starts_with(&next_prefix) && name.ends_with(".exe");
            let is_download = name.starts_with(&download_prefix);
            if is_download {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            if !is_previous && !is_next {
                continue;
            }
            if std::fs::remove_file(entry.path()).is_err() && is_previous {
                still_running += 1;
            }
        }
        still_running
    }
}

/// `command`, split into the program and its arguments the way a shell
/// would, respecting the one quoting `classify_source` ever produces --
/// double quotes around a local path that holds a space (`d815`): the
/// cargo command itself is never run through a shell, so this is the only
/// place its own quoting has to be undone.
fn split_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in command.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// One line of a `SHA256SUMS` file, the line for `name`'s own hash: the
/// two-space form `sha256sum` writes by default, or its binary form
/// `<hex> *<name>`. Extra whitespace between the hash and the name is
/// tolerated, and the hash's own case is left to the caller to fold.
fn find_sha256(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let hash = parts.next()?;
        let rest = parts.next().unwrap_or("").trim_start();
        let rest = rest.strip_prefix('*').unwrap_or(rest);
        if rest == name {
            return Some(hash.to_string());
        }
    }
    None
}

/// The SHA-256 of `data`, lower case hex. `sha2` rather than hand-rolled
/// (`d815`): this is what a downloaded archive is checked against before
/// it ever runs or replaces anything, and a hash guarding that decision is
/// exactly the code `d138` already refused to write twice.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the vivac at `path` says it is, from its own `--version`: the
/// `X.Y.Z` after `vivac `, or `None` when it would not run or said
/// something else.
fn version_of(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .strip_prefix("vivac ")
        .map(str::to_string)
}

/// A spawn's outcome, the three ways `run_quiet`'s callers ever have to
/// tell apart: the tool ran and succeeded, the tool is not on this
/// machine, or the tool ran and refused.
enum SpawnOutcome {
    Done,
    NotFound,
    Failed(i32),
}

/// Runs `cmd` with every stream silenced -- `curl -fsSL` and `tar -xf`
/// already print nothing on their own success, and a failure here is read
/// from the exit code, not from anything they wrote (`d815`).
fn run_quiet(mut cmd: Command) -> SpawnOutcome {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    match cmd.status() {
        Ok(status) if status.success() => SpawnOutcome::Done,
        Ok(status) => SpawnOutcome::Failed(status.code().unwrap_or(-1)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SpawnOutcome::NotFound,
        Err(_) => SpawnOutcome::Failed(-1),
    }
}

fn still_running_text(n: usize) -> String {
    if n == 1 {
        "1 copy set aside by an earlier update is still running. A later \
         vivac update removes it once nothing runs it."
            .to_string()
    } else {
        format!(
            "{n} copies set aside by an earlier update are still running. A \
             later vivac update removes them once nothing runs them."
        )
    }
}

/// A cargo install failed after a confirmed yes (`d815`): exit code 5, the
/// same family as `SetAside` -- the executable's own folder is again what
/// this process could not finish putting a new binary into.
fn not_installed_cargo(reason: String, command: &str) -> Failure {
    let mut m = format!(
        "  Not installed: {reason}. The vivac that was running is still in place.\n  \
         To install by hand:  {command}"
    );
    if cfg!(windows) {
        m.push_str(
            "\n  If that fails with os error 5, a session started vivac in the \
             meantime: run vivac update again first.",
        );
    }
    Failure::NotInstalled(m)
}

/// A release-archive install failed (`d815`). `set_aside_happened` is
/// `true` only once the running copy has actually been moved aside -- the
/// last step, right before the final rename -- so every earlier failure
/// still reads "Nothing was changed."
fn not_installed_archive(
    reason: String,
    archive: &str,
    noun: &str,
    dir: &Path,
    set_aside_happened: bool,
) -> Failure {
    let trailer = if set_aside_happened {
        "The vivac that was running is still in place."
    } else {
        "Nothing was changed."
    };
    Failure::NotInstalled(format!(
        "  Not installed: {reason}. {trailer}\n  \
         To install by hand: download {archive} from {RELEASES_LATEST}, check it \
         against SHA256SUMS, and put the {noun} inside it in {}.",
        dir.display()
    ))
}

/// What a release-archive install did, once the download and the checksum
/// both held.
#[derive(Debug)]
enum ArchiveResult {
    /// The archive holds the same version this binary already is: nothing
    /// was set aside and nothing was replaced.
    AlreadyNewest(String),
    Replaced {
        new_version: String,
        still_running: usize,
    },
}

/// Downloads `archive` and its `SHA256SUMS` from `base`, checks it,
/// unpacks it with `tar`, and swaps `exe` for the binary inside -- the same
/// four steps the plan's own `download`, `check`, `set aside` and
/// `replace` rows name (`d815`). `base` is a parameter rather than the
/// constant `DOWNLOAD_BASE` so a test can pass a `file://` URL to a local
/// folder instead; `own_version` is this binary's own version, read once
/// by the caller for the same reason.
///
/// Every work file lives in `dir`, next to `exe` and on the same volume so
/// the final rename is atomic, named `<stem>.download-<ulid>...`, and is
/// removed before this returns, on every path out.
fn install_from_archive(
    base: &str,
    archive: &str,
    noun: &str,
    dir: &Path,
    exe: &Path,
    stem: &str,
    own_version: &str,
) -> Result<ArchiveResult, Failure> {
    let id = crate::id::ulid();
    let ext = archive.find('.').map(|i| &archive[i..]).unwrap_or("");
    let archive_path = dir.join(format!("{stem}.download-{id}{ext}"));
    let sums_path = dir.join(format!("{stem}.download-{id}.sums"));
    let extract_dir = dir.join(format!("{stem}.download-{id}"));

    // Against the real release, curl may follow GitHub's redirect to its
    // download host and nowhere else: never down to plain http, and never
    // below TLS 1.2. A test's `file://` base has no protocol to pin.
    let pin: &[&str] = if base.starts_with("https://") {
        &["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2"]
    } else {
        &[]
    };

    let cleanup = || {
        let _ = std::fs::remove_file(&archive_path);
        let _ = std::fs::remove_file(&sums_path);
        let _ = std::fs::remove_dir_all(&extract_dir);
    };
    let fail = |reason: String| {
        cleanup();
        Err(not_installed_archive(reason, archive, noun, dir, false))
    };

    let mut curl_archive = Command::new("curl");
    curl_archive
        .args(pin)
        .arg("-fsSL")
        .arg("-o")
        .arg(&archive_path)
        .arg(format!("{base}/{archive}"));
    match run_quiet(curl_archive) {
        SpawnOutcome::Done => {}
        SpawnOutcome::NotFound => return fail("curl is not on this machine".to_string()),
        SpawnOutcome::Failed(code) => {
            return fail(format!("the download failed (curl exited with {code})"))
        }
    }

    let mut curl_sums = Command::new("curl");
    curl_sums
        .args(pin)
        .arg("-fsSL")
        .arg("-o")
        .arg(&sums_path)
        .arg(format!("{base}/SHA256SUMS"));
    match run_quiet(curl_sums) {
        SpawnOutcome::Done => {}
        SpawnOutcome::NotFound => return fail("curl is not on this machine".to_string()),
        SpawnOutcome::Failed(code) => {
            return fail(format!("the download failed (curl exited with {code})"))
        }
    }

    let sums = std::fs::read_to_string(&sums_path).unwrap_or_default();
    let Some(expected) = find_sha256(&sums, archive) else {
        return fail(format!("SHA256SUMS has no line for {archive}"));
    };
    let bytes = std::fs::read(&archive_path).unwrap_or_default();
    if !sha256_hex(&bytes).eq_ignore_ascii_case(&expected) {
        return fail("the archive does not match its checksum in SHA256SUMS".to_string());
    }

    if std::fs::create_dir_all(&extract_dir).is_err() {
        return fail(format!("the archive holds no {noun} where it should"));
    }
    let mut tar_extract = Command::new("tar");
    tar_extract
        .arg("-xf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract_dir);
    match run_quiet(tar_extract) {
        SpawnOutcome::Done => {}
        SpawnOutcome::NotFound => return fail("tar is not on this machine".to_string()),
        SpawnOutcome::Failed(_) => {
            return fail(format!("the archive holds no {noun} where it should"))
        }
    }

    let target_stem = archive.strip_suffix(ext).unwrap_or(archive);
    let extracted = extract_dir.join(target_stem).join(noun);
    if !extracted.is_file() {
        return fail(format!("the archive holds no {noun} where it should"));
    }

    let Some(new_version) = version_of(&extracted) else {
        return fail("the new vivac did not say its version".to_string());
    };

    if new_version == own_version {
        cleanup();
        return Ok(ArchiveResult::AlreadyNewest(new_version));
    }

    #[cfg(windows)]
    let still_running = match windows_set_aside::set_aside_excluding(
        exe,
        dir,
        stem,
        Some(&format!("{stem}.download-{id}")),
    ) {
        Ok(n) => n,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    #[cfg(not(windows))]
    let still_running = 0usize;

    match std::fs::rename(&extracted, exe) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(exe) {
                    let mut perm = metadata.permissions();
                    perm.set_mode(perm.mode() | 0o111);
                    let _ = std::fs::set_permissions(exe, perm);
                }
            }
            cleanup();
            Ok(ArchiveResult::Replaced {
                new_version,
                still_running,
            })
        }
        Err(e) => {
            cleanup();
            Err(not_installed_archive(
                format!("could not put the new vivac in place: {e}"),
                archive,
                noun,
                dir,
                true,
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// The plan (`d815`, styled as `d792`/`f814`): the same `PlanItem`,
// `heading` and `render_items` `setup`'s own plans use, so `vivac update`
// reads like every other plan this CLI shows before it asks a question.
// ---------------------------------------------------------------------------

fn cargo_plan_items(c: &CargoInstall, v: &str) -> Vec<PlanItem> {
    let mut items = Vec::new();
    if cfg!(windows) {
        items.push(PlanItem::new(
            "set aside",
            "vivac.exe",
            format!("open sessions and vivac web keep {v} until they restart"),
        ));
    }
    items.push(PlanItem::new(
        "run",
        c.command.clone(),
        format!(
            "builds the newest vivac from {} and installs it here",
            c.plan_source
        ),
    ));
    items
}

fn archive_plan_items(archive: &str, v: &str) -> Vec<PlanItem> {
    let noun = exe_noun();
    let mut items = vec![
        PlanItem::new(
            "download",
            archive.to_string(),
            format!("from {RELEASES_LATEST}, with curl"),
        ),
        PlanItem::new(
            "check",
            "SHA256SUMS",
            "the archive against the checksum published beside it",
        ),
    ];
    if cfg!(windows) {
        items.push(PlanItem::new(
            "set aside",
            noun,
            format!("open sessions and vivac web keep {v} until they restart"),
        ));
    }
    items.push(PlanItem::new(
        "replace",
        noun,
        "with the one inside the archive, unpacked with tar",
    ));
    items
}

/// Prints `body` trimmed of its own trailing newlines, followed by exactly
/// one -- ahead of the prompt this always precedes, the same shape
/// `setup`'s own `print_plan` uses for its plans.
fn print_plan(body: &str) {
    let line = format!("{}\n", body.trim_end_matches('\n'));
    print!("{line}");
}

// ---------------------------------------------------------------------------
// A terminal: plan, ask, and only a yes installs (`d815`).
// ---------------------------------------------------------------------------

fn run_terminal(exe: &Path, dir: &Path, cargo: Option<CargoInstall>) -> Result<(), Failure> {
    let v = env!("CARGO_PKG_VERSION");
    match cargo {
        Some(c) => run_terminal_cargo(exe, dir, &c, v),
        None => match archive_name(std::env::consts::OS, std::env::consts::ARCH) {
            Some(archive) => run_terminal_archive(exe, dir, archive, v),
            None => {
                outln!("{NO_ARCHIVE_TEXT}");
                Ok(())
            }
        },
    }
}

fn run_terminal_cargo(exe: &Path, dir: &Path, c: &CargoInstall, v: &str) -> Result<(), Failure> {
    let plan_block = format!(
        "{}{}",
        heading(Stream::Out, "vivac update", dir),
        render_items(Stream::Out, &cargo_plan_items(c, v))
    );
    print_plan(&plan_block);
    if !crate::setup::ask("\nInstall it? [y/N] ") {
        outln!("\nNothing was changed.");
        return Ok(());
    }

    #[cfg(windows)]
    let still_running = {
        let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("vivac");
        windows_set_aside::set_aside(exe, dir, stem)?
    };
    #[cfg(not(windows))]
    let still_running = 0usize;

    let mut parts = split_command(&c.command);
    let program = parts.remove(0);
    let status = Command::new(&program)
        .args(&parts)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) if s.success() => {
            // cargo exits 0 when it had nothing to do as well, so what it
            // left at `exe` is asked rather than assumed: "Installed" is only
            // said of a version this one is not.
            let installed = version_of(exe);
            outln!();
            match installed.as_deref() {
                Some(new) if new == v => {
                    outln!(
                        "Already the newest: {} holds {v}, the same as this one. \
                         Nothing was replaced.",
                        c.plan_source
                    );
                    return Ok(());
                }
                Some(new) => outln!("{} vivac {new}.", style::good(Stream::Out, "Installed")),
                None => outln!("{}", style::good(Stream::Out, "Installed.")),
            }
            outln!();
            outln!(
                "Sessions and vivac web that are already open keep {v} until they \
                 restart; anything started after this runs the new one."
            );
            if still_running > 0 {
                outln!();
                outln!("{}", still_running_text(still_running));
            }
            outln!();
            outln!(
                "{} restart the sessions and vivac web you want on {}.",
                style::bold(Stream::Out, "Next:"),
                installed.as_deref().unwrap_or("the new version")
            );
            Ok(())
        }
        Ok(s) => Err(not_installed_cargo(
            format!("cargo install exited with {}", s.code().unwrap_or(-1)),
            &c.command,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(not_installed_cargo(
            "cargo is not on this machine".to_string(),
            &c.command,
        )),
        Err(e) => Err(Failure::Io(e)),
    }
}

fn run_terminal_archive(
    exe: &Path,
    dir: &Path,
    archive: &'static str,
    v: &str,
) -> Result<(), Failure> {
    let plan_block = format!(
        "{}{}",
        heading(Stream::Out, "vivac update", dir),
        render_items(Stream::Out, &archive_plan_items(archive, v))
    );
    print_plan(&plan_block);
    if !crate::setup::ask("\nInstall it? [y/N] ") {
        outln!("\nNothing was changed.");
        return Ok(());
    }

    let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("vivac");
    let noun = exe_noun();
    match install_from_archive(DOWNLOAD_BASE, archive, noun, dir, exe, stem, v)? {
        ArchiveResult::AlreadyNewest(found) => {
            outln!();
            outln!(
                "Already the newest: the release holds {found}, the same as this \
                 one. Nothing was replaced."
            );
            Ok(())
        }
        ArchiveResult::Replaced {
            new_version,
            still_running,
        } => {
            outln!();
            outln!(
                "{} vivac {new_version}.",
                style::good(Stream::Out, "Installed")
            );
            outln!();
            outln!(
                "Sessions and vivac web that are already open keep {v} until they \
                 restart; anything started after this runs the new one."
            );
            if still_running > 0 {
                outln!();
                outln!("{}", still_running_text(still_running));
            }
            outln!();
            outln!(
                "{} restart the sessions and vivac web you want on {new_version}.",
                style::bold(Stream::Out, "Next:")
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// No terminal: exactly the facts a terminal run's own plan already carries,
// said instead of asked -- unchanged since `d802`, only unwrapped and
// styled the way `d792`/`f814` already changed every other plan in this
// CLI (`d815`).
// ---------------------------------------------------------------------------

// `exe` is read only inside the Windows-only set-aside step below.
#[cfg_attr(not(windows), allow(unused_variables))]
fn run_no_terminal(exe: &Path, dir: &Path, cargo: Option<CargoInstall>) -> Result<(), Failure> {
    let v = env!("CARGO_PKG_VERSION");
    let dir_display = dir.display();

    #[cfg(windows)]
    let still_running = {
        let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("vivac");
        windows_set_aside::set_aside(exe, dir, stem)?
    };

    match &cargo {
        Some(c) => outln!("vivac {v}, installed by cargo install {}.", c.described_as),
        None => outln!("vivac {v}, in {dir_display}, not installed by cargo install."),
    }

    #[cfg(windows)]
    {
        outln!();
        outln!(
            "The copy that was running is set aside, so the install no longer \
             waits for sessions to close. Sessions and vivac web that are \
             already open keep {v} until they restart; anything started after \
             the install runs the new one."
        );
        if still_running > 0 {
            outln!();
            outln!("{}", still_running_text(still_running));
        }
    }

    #[cfg(not(windows))]
    {
        outln!();
        outln!(
            "Sessions and vivac web that are already open keep {v} until they \
             restart; anything started after the install runs the new one."
        );
    }

    match &cargo {
        Some(c) => {
            outln!();
            outln!("{} install it:", style::bold(Stream::Out, "Next:"));
            outln!();
            outln!("  {}", style::bold(Stream::Out, &c.command));
            #[cfg(windows)]
            {
                outln!();
                outln!(
                    "If it still fails with os error 5, a session started vivac in \
                     the meantime: run vivac update again, then the install."
                );
            }
            outln!();
            outln!("In a terminal, vivac update asks and does this for you.");
        }
        None => match archive_name(std::env::consts::OS, std::env::consts::ARCH) {
            Some(archive) => {
                let noun = exe_noun();
                outln!();
                outln!(
                    "{} download {archive} from {RELEASES_LATEST}, check it against \
                     the SHA256SUMS published there, and put the {noun} inside it \
                     in {dir_display}.",
                    style::bold(Stream::Out, "Next:")
                );
                #[cfg(windows)]
                {
                    outln!();
                    outln!(
                        "If that still fails because the file is in use, a session \
                         started vivac in the meantime: run vivac update again, \
                         then put it there."
                    );
                }
                outln!();
                outln!("In a terminal, vivac update asks and does this for you.");
            }
            None => {
                outln!();
                outln!("{NO_ARCHIVE_TEXT}");
            }
        },
    }

    Ok(())
}

/// `vivac update`: with a terminal to ask, plans the install and only a
/// confirmed `y` does it; without one, says what to type and touches
/// nothing (`d802`, `d815`). Never exposed through MCP, and never anything
/// the binary calls out over the network for on its own -- `curl` and
/// `tar` do that, spawned directly with an argument vector, never through
/// a shell.
pub fn run() -> Result<(), Failure> {
    let exe = std::env::current_exe().map_err(Failure::Io)?;
    let dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let cargo = detect_cargo_install(&dir);

    if crate::setup::stdin_is_terminal() {
        run_terminal(&exe, &dir, cargo)
    } else {
        run_no_terminal(&exe, &dir, cargo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crates_io_registry_key_is_recognised() {
        let json = r#"{"installs":{"vivac 0.15.5 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(c.command, "cargo install vivac");
        assert_eq!(c.described_as, "from crates.io");
        assert_eq!(c.plan_source, "crates.io");
    }

    #[test]
    fn a_sparse_crates_io_key_is_recognised() {
        let json =
            r#"{"installs":{"vivac 0.15.5 (sparse+https://index.crates.io/)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(c.command, "cargo install vivac");
        assert_eq!(c.described_as, "from crates.io");
        assert_eq!(c.plan_source, "crates.io");
    }

    #[test]
    fn a_registry_key_that_is_not_crates_io_names_its_own_index() {
        let json = r#"{"installs":{"vivac 0.15.5 (registry+https://example.com/my-index)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(
            c.command,
            "cargo install vivac --index https://example.com/my-index"
        );
        assert_eq!(
            c.described_as,
            "from the registry at https://example.com/my-index"
        );
        assert_eq!(
            c.plan_source,
            "the registry at https://example.com/my-index"
        );
    }

    #[test]
    fn a_sparse_key_that_is_not_crates_io_keeps_its_own_prefix() {
        let json = r#"{"installs":{"vivac 0.15.5 (sparse+https://example.com/my-index/)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(
            c.command,
            "cargo install vivac --index sparse+https://example.com/my-index/"
        );
        assert_eq!(
            c.described_as,
            "from the registry at sparse+https://example.com/my-index/"
        );
        assert_eq!(
            c.plan_source,
            "the registry at sparse+https://example.com/my-index/"
        );
    }

    #[test]
    fn a_git_key_strips_its_query_and_fragment() {
        let json = r#"{"installs":{"vivac 0.15.5 (git+https://github.com/example/vivac?branch=main#deadbeef)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(
            c.command,
            "cargo install --git https://github.com/example/vivac vivac"
        );
        assert_eq!(c.described_as, "from https://github.com/example/vivac");
        assert_eq!(c.plan_source, "https://github.com/example/vivac");
    }

    #[test]
    fn a_windows_path_key_decodes_percent_escapes() {
        let json = r#"{"installs":{"vivac 0.15.5 (path+file:///H:/tmp/vivac%20src)":{"bins":["vivac.exe"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        #[cfg(windows)]
        {
            assert_eq!(c.command, "cargo install --path \"H:/tmp/vivac src\"");
            assert_eq!(c.plan_source, "the local folder \"H:/tmp/vivac src\"");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(c.command, "cargo install --path \"/H:/tmp/vivac src\"");
            assert_eq!(c.plan_source, "the local folder \"/H:/tmp/vivac src\"");
        }
        assert_eq!(c.described_as, "from a local folder");
    }

    #[test]
    fn an_unrelated_crates_key_is_not_a_cargo_install_of_vivac() {
        let json = r#"{"installs":{"ripgrep 14.0.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"]}}}"#;
        assert!(cargo_install_from_json(json).is_none());
    }

    #[test]
    fn malformed_json_is_not_a_cargo_install() {
        assert!(cargo_install_from_json("{not json").is_none());
    }

    #[test]
    fn the_archive_map_matches_the_release_workflow() {
        assert_eq!(
            archive_name("linux", "x86_64"),
            Some("vivac-x86_64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(
            archive_name("linux", "aarch64"),
            Some("vivac-aarch64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(
            archive_name("macos", "x86_64"),
            Some("vivac-x86_64-apple-darwin.tar.gz")
        );
        assert_eq!(
            archive_name("macos", "aarch64"),
            Some("vivac-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            archive_name("windows", "x86_64"),
            Some("vivac-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(archive_name("freebsd", "x86_64"), None);
    }

    // -----------------------------------------------------------------
    // `split_command` (`d815`): the cargo command, split the way a shell
    // would, for all four source forms.
    // -----------------------------------------------------------------

    #[test]
    fn split_command_handles_every_cargo_source_form() {
        assert_eq!(
            split_command("cargo install vivac"),
            vec!["cargo", "install", "vivac"]
        );
        assert_eq!(
            split_command("cargo install vivac --index https://example.com/my-index"),
            vec![
                "cargo",
                "install",
                "vivac",
                "--index",
                "https://example.com/my-index"
            ]
        );
        assert_eq!(
            split_command("cargo install --git https://example.com/vivac.git vivac"),
            vec![
                "cargo",
                "install",
                "--git",
                "https://example.com/vivac.git",
                "vivac"
            ]
        );
        assert_eq!(
            split_command("cargo install --path \"H:/tmp/vivac src\""),
            vec!["cargo", "install", "--path", "H:/tmp/vivac src"]
        );
        assert_eq!(
            split_command("cargo install --path /home/user/vivac"),
            vec!["cargo", "install", "--path", "/home/user/vivac"]
        );
    }

    // -----------------------------------------------------------------
    // SHA256SUMS parsing and the hash itself (`d815`).
    // -----------------------------------------------------------------

    #[test]
    fn sha256_matches_the_known_vector_for_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn find_sha256_reads_the_two_space_form() {
        let sums = "aaaa  vivac-x86_64-unknown-linux-musl.tar.gz\nbbbb  vivac-x86_64-pc-windows-msvc.zip\n";
        assert_eq!(
            find_sha256(sums, "vivac-x86_64-pc-windows-msvc.zip"),
            Some("bbbb".to_string())
        );
    }

    #[test]
    fn find_sha256_reads_the_binary_star_form() {
        let sums = "cccc *vivac-x86_64-apple-darwin.tar.gz\n";
        assert_eq!(
            find_sha256(sums, "vivac-x86_64-apple-darwin.tar.gz"),
            Some("cccc".to_string())
        );
    }

    #[test]
    fn find_sha256_answers_none_with_no_matching_line() {
        let sums = "aaaa  something-else.tar.gz\n";
        assert_eq!(find_sha256(sums, "vivac-x86_64-pc-windows-msvc.zip"), None);
    }

    #[test]
    fn find_sha256_tolerates_extra_whitespace() {
        let sums = "aaaa\t\tvivac-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            find_sha256(sums, "vivac-aarch64-apple-darwin.tar.gz"),
            Some("aaaa".to_string())
        );
    }

    #[test]
    fn find_sha256_keeps_uppercase_hex_untouched() {
        let sums = "ABCDEF  vivac-aarch64-unknown-linux-musl.tar.gz\n";
        assert_eq!(
            find_sha256(sums, "vivac-aarch64-unknown-linux-musl.tar.gz"),
            Some("ABCDEF".to_string())
        );
    }

    // -----------------------------------------------------------------
    // The plan text, styles off (plain, the way every test in this suite
    // runs with no terminal and no `CLICOLOR_FORCE` behind it) -- exact
    // strings (`d815`).
    // -----------------------------------------------------------------

    fn cargo_fixture() -> CargoInstall {
        CargoInstall {
            command: "cargo install vivac".to_string(),
            described_as: "from crates.io".to_string(),
            plan_source: "crates.io".to_string(),
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_cargo_plan_names_every_row_on_windows() {
        let items = cargo_plan_items(&cargo_fixture(), "9.9.9");
        let expected_items = vec![
            PlanItem::new(
                "set aside",
                "vivac.exe",
                "open sessions and vivac web keep 9.9.9 until they restart",
            ),
            PlanItem::new(
                "run",
                "cargo install vivac",
                "builds the newest vivac from crates.io and installs it here",
            ),
        ];
        assert_eq!(
            render_items(Stream::Out, &items),
            render_items(Stream::Out, &expected_items)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_cargo_plan_has_no_set_aside_row_off_windows() {
        let items = cargo_plan_items(&cargo_fixture(), "9.9.9");
        let expected_items = vec![PlanItem::new(
            "run",
            "cargo install vivac",
            "builds the newest vivac from crates.io and installs it here",
        )];
        assert_eq!(
            render_items(Stream::Out, &items),
            render_items(Stream::Out, &expected_items)
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_archive_plan_names_every_row_on_windows() {
        let items = archive_plan_items("vivac-x86_64-pc-windows-msvc.zip", "9.9.9");
        let expected_items = vec![
            PlanItem::new(
                "download",
                "vivac-x86_64-pc-windows-msvc.zip",
                format!("from {RELEASES_LATEST}, with curl"),
            ),
            PlanItem::new(
                "check",
                "SHA256SUMS",
                "the archive against the checksum published beside it",
            ),
            PlanItem::new(
                "set aside",
                "vivac.exe",
                "open sessions and vivac web keep 9.9.9 until they restart",
            ),
            PlanItem::new(
                "replace",
                "vivac.exe",
                "with the one inside the archive, unpacked with tar",
            ),
        ];
        assert_eq!(
            render_items(Stream::Out, &items),
            render_items(Stream::Out, &expected_items)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_archive_plan_has_no_set_aside_row_off_windows() {
        let items = archive_plan_items("vivac-x86_64-unknown-linux-musl.tar.gz", "9.9.9");
        let expected_items = vec![
            PlanItem::new(
                "download",
                "vivac-x86_64-unknown-linux-musl.tar.gz",
                format!("from {RELEASES_LATEST}, with curl"),
            ),
            PlanItem::new(
                "check",
                "SHA256SUMS",
                "the archive against the checksum published beside it",
            ),
            PlanItem::new(
                "replace",
                "vivac",
                "with the one inside the archive, unpacked with tar",
            ),
        ];
        assert_eq!(
            render_items(Stream::Out, &items),
            render_items(Stream::Out, &expected_items)
        );
    }

    #[test]
    fn the_heading_names_the_command_and_the_folder() {
        let h = heading(Stream::Out, "vivac update", Path::new("here"));
        assert_eq!(h, "vivac update will, in here:\n\n");
    }

    // -----------------------------------------------------------------
    // The archive install function, against a `file://` base URL in a
    // temp folder (`d815`). Skipped, rather than failed, where `curl` or
    // `tar` are not on this machine's own PATH.
    // -----------------------------------------------------------------

    fn tool_on_path(name: &str) -> bool {
        !matches!(
            run_quiet({
                let mut c = Command::new(name);
                c.arg("--version");
                c
            }),
            SpawnOutcome::NotFound
        )
    }

    /// A folder nothing else touches, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("vivac-update-test-{name}-{}", crate::id::ulid()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The script `install_from_archive`'s own fixture runs for
    /// `--version`: a `.bat` on Windows (Rust's `Command` spawns one
    /// directly, wrapped through `cmd.exe` since the CVE-2024-24576 fix),
    /// a shebang script everywhere else.
    #[cfg(windows)]
    const FIXTURE_NOUN: &str = "vivac.bat";
    #[cfg(not(windows))]
    const FIXTURE_NOUN: &str = "vivac.sh";

    #[cfg(windows)]
    const FIXTURE_SCRIPT: &str = "@echo off\r\necho vivac 9.9.9\r\n";
    #[cfg(not(windows))]
    const FIXTURE_SCRIPT: &str = "#!/bin/sh\necho 'vivac 9.9.9'\n";

    fn write_fixture_script(path: &Path) {
        std::fs::write(path, FIXTURE_SCRIPT).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(path).unwrap().permissions();
            perm.set_mode(perm.mode() | 0o111);
            std::fs::set_permissions(path, perm).unwrap();
        }
    }

    /// Builds `<dir>/<stem>.tar.gz`, holding a folder `<stem>/` with the
    /// fixture script at `<stem>/<FIXTURE_NOUN>`, using the system `tar`
    /// (`d815`).
    fn build_fixture_archive(dir: &Path, stem: &str) -> PathBuf {
        let inner = dir.join(stem);
        std::fs::create_dir_all(&inner).unwrap();
        write_fixture_script(&inner.join(FIXTURE_NOUN));
        let archive_path = dir.join(format!("{stem}.tar.gz"));
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive_path)
            .arg("-C")
            .arg(dir)
            .arg(stem)
            .status()
            .unwrap();
        assert!(status.success(), "building the fixture archive failed");
        archive_path
    }

    fn file_url(dir: &Path) -> String {
        let display = dir.display().to_string().replace('\\', "/");
        if display.starts_with('/') {
            format!("file://{display}")
        } else {
            format!("file:///{display}")
        }
    }

    fn download_entries(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".download-"))
            .collect()
    }

    #[test]
    fn a_matching_archive_replaces_the_target_and_leaves_no_work_files() {
        if !tool_on_path("curl") || !tool_on_path("tar") {
            eprintln!("skipping: curl or tar is not on this machine's PATH");
            return;
        }
        let source = TempDir::new("source");
        let target_dir = TempDir::new("target");
        let stem = "vivac-fixture-target";
        let archive_path = build_fixture_archive(&source.0, stem);
        let archive_name = archive_path.file_name().unwrap().to_str().unwrap();

        let sums_path = source.0.join("SHA256SUMS");
        let hash = sha256_hex(&std::fs::read(&archive_path).unwrap());
        std::fs::write(&sums_path, format!("{hash}  {archive_name}\n")).unwrap();

        let exe = target_dir.0.join("current.bat");
        std::fs::write(&exe, "@echo off\r\necho vivac 0.0.0\r\n").unwrap();

        let base = file_url(&source.0);
        let result = install_from_archive(
            &base,
            archive_name,
            FIXTURE_NOUN,
            &target_dir.0,
            &exe,
            "current",
            "0.0.0",
        )
        .expect("install should succeed");

        match result {
            ArchiveResult::Replaced { new_version, .. } => assert_eq!(new_version, "9.9.9"),
            ArchiveResult::AlreadyNewest(_) => panic!("should have replaced, not matched"),
        }
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            FIXTURE_SCRIPT,
            "the target was not replaced with the fixture"
        );
        assert!(
            download_entries(&target_dir.0).is_empty(),
            "a work file was left behind: {:?}",
            download_entries(&target_dir.0)
        );
    }

    #[test]
    fn a_mismatching_checksum_leaves_the_target_untouched() {
        if !tool_on_path("curl") || !tool_on_path("tar") {
            eprintln!("skipping: curl or tar is not on this machine's PATH");
            return;
        }
        let source = TempDir::new("source-bad");
        let target_dir = TempDir::new("target-bad");
        let stem = "vivac-fixture-mismatch";
        let archive_path = build_fixture_archive(&source.0, stem);
        let archive_name = archive_path.file_name().unwrap().to_str().unwrap();

        let sums_path = source.0.join("SHA256SUMS");
        std::fs::write(&sums_path, format!("{} {}\n", "0".repeat(64), archive_name)).unwrap();

        let exe = target_dir.0.join("current.bat");
        let original = "@echo off\r\necho vivac 0.0.0\r\n";
        std::fs::write(&exe, original).unwrap();

        let base = file_url(&source.0);
        let err = install_from_archive(
            &base,
            archive_name,
            FIXTURE_NOUN,
            &target_dir.0,
            &exe,
            "current",
            "0.0.0",
        )
        .expect_err("a mismatching checksum should fail");
        assert!(
            err.message().contains("does not match its checksum"),
            "{}",
            err.message()
        );
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            original,
            "the target should be untouched"
        );
        assert!(
            download_entries(&target_dir.0).is_empty(),
            "a work file was left behind: {:?}",
            download_entries(&target_dir.0)
        );
    }
}
