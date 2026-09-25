//! `vivac update` -- how this vivac was installed, and the command that
//! replaces it (`d802`).
//!
//! It makes no network call, downloads nothing and runs no installer
//! (`d742`, `r487`): the whole point is to say what the person should type
//! next, never to type it for them. It needs no tree, and `main.rs`
//! dispatches it before any store lookup for that reason, the same as
//! `--version`, `init` and `setup`.
//!
//! On Windows an executable in use cannot be overwritten or deleted, only
//! renamed (`f773`, measured), so every run also sets the copy that is
//! running aside before it prints anything -- see `windows_set_aside`
//! below. On Linux and macOS nothing is set aside: replacing a running
//! binary already works there.

use crate::failure::Failure;
use crate::output::outln;
use std::path::{Path, PathBuf};

/// The width every paragraph below wraps to, the same convention
/// `registry::wrapped_with_command`'s own `NOTICE_WIDTH` already uses for
/// fixed prose ahead of a command line.
const WIDTH: usize = 76;

const CRATES_IO_REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
const CRATES_IO_SPARSE: &str = "sparse+https://index.crates.io/";

/// What `.crates2.json`'s own key for `vivac` resolves to: the command that
/// reproduces the same install, and the phrase that names its source.
struct CargoInstall {
    command: String,
    described_as: String,
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
/// install and the phrase that names it. `None` for a source this does not
/// recognise -- the caller falls back to naming the release archive
/// instead.
fn classify_source(source: &str) -> Option<CargoInstall> {
    if source == CRATES_IO_REGISTRY || source == CRATES_IO_SPARSE {
        return Some(CargoInstall {
            command: "cargo install vivac".to_string(),
            described_as: "from crates.io".to_string(),
        });
    }
    if let Some(url) = source.strip_prefix("registry+") {
        return Some(CargoInstall {
            command: format!("cargo install vivac --index {url}"),
            described_as: format!("from the registry at {url}"),
        });
    }
    if source.starts_with("sparse+") {
        return Some(CargoInstall {
            command: format!("cargo install vivac --index {source}"),
            described_as: format!("from the registry at {source}"),
        });
    }
    if let Some(rest) = source.strip_prefix("git+") {
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        let url = &rest[..end];
        return Some(CargoInstall {
            command: format!("cargo install --git {url} vivac"),
            described_as: format!("from {url}"),
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
        let still_running = sweep(dir, stem);
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
    fn sweep(dir: &Path, stem: &str) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let previous_prefix = format!("{stem}.previous-");
        let next_prefix = format!("{stem}.next-");
        let mut still_running = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let is_previous = name.starts_with(&previous_prefix) && name.ends_with(".exe");
            let is_next = name.starts_with(&next_prefix) && name.ends_with(".exe");
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

/// Wraps `text` at [`WIDTH`] and prints it, one line per call to `outln!`,
/// each carrying the two-space margin every other read in this CLI prints
/// its prose with.
fn paragraph(text: &str) {
    for line in crate::render::wrap(text, WIDTH, "") {
        outln!("  {line}");
    }
}

/// `vivac update`: no network call, nothing downloaded, no installer run
/// (`d742`, `r487`). Says how this vivac was installed and the exact
/// command to replace it -- and, on Windows, sets the running copy aside
/// first, so the install it names no longer waits for every open session
/// to close.
pub fn run() -> Result<(), Failure> {
    let exe = std::env::current_exe().map_err(Failure::Io)?;
    let dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let cargo = detect_cargo_install(&dir);

    #[cfg(windows)]
    let still_running = {
        let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("vivac");
        windows_set_aside::set_aside(&exe, &dir, stem)?
    };

    let v = env!("CARGO_PKG_VERSION");
    let dir_display = dir.display();

    match &cargo {
        Some(c) => paragraph(&format!(
            "vivac {v}, installed by cargo install {}.",
            c.described_as
        )),
        None => paragraph(&format!(
            "vivac {v}, in {dir_display}, not installed by cargo install."
        )),
    }

    #[cfg(windows)]
    {
        outln!();
        paragraph(&format!(
            "The copy that was running is set aside, so the install no longer \
             waits for sessions to close. Sessions and vivac web that are already \
             open keep {v} until they restart; anything started after the install \
             runs the new one."
        ));
        if still_running > 0 {
            outln!();
            paragraph(&if still_running == 1 {
                "1 copy set aside by an earlier update is still running. A later \
                 vivac update removes it once nothing runs it."
                    .to_string()
            } else {
                format!(
                    "{still_running} copies set aside by an earlier update are still \
                     running. A later vivac update removes them once nothing runs \
                     them."
                )
            });
        }
    }

    outln!();
    match &cargo {
        Some(c) => {
            paragraph("Now run:");
            outln!();
            outln!("    {}", c.command);
            #[cfg(windows)]
            {
                outln!();
                paragraph(
                    "If it still fails with os error 5, a session started vivac in \
                     the meantime: run vivac update again, then the install.",
                );
            }
        }
        None => match archive_name(std::env::consts::OS, std::env::consts::ARCH) {
            Some(archive) => {
                let noun = if cfg!(windows) { "vivac.exe" } else { "vivac" };
                paragraph(&format!(
                    "Now download {archive} from \
                     https://github.com/JAAvila-Of/vivac/releases/latest, check it \
                     against the SHA256SUMS published there, and put the {noun} \
                     inside it in {dir_display}."
                ));
                #[cfg(windows)]
                {
                    outln!();
                    paragraph(
                        "If that still fails because the file is in use, a session \
                         started vivac in the meantime: run vivac update again, then \
                         put it there.",
                    );
                }
            }
            None => paragraph(
                "There is no release archive for this platform. With a Rust \
                 toolchain, cargo install vivac builds it.",
            ),
        },
    }

    #[cfg(not(windows))]
    {
        outln!();
        paragraph(&format!(
            "Sessions and vivac web that are already open keep {v} until they \
             restart; anything started after the install runs the new one."
        ));
    }

    Ok(())
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
    }

    #[test]
    fn a_sparse_crates_io_key_is_recognised() {
        let json =
            r#"{"installs":{"vivac 0.15.5 (sparse+https://index.crates.io/)":{"bins":["vivac"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        assert_eq!(c.command, "cargo install vivac");
        assert_eq!(c.described_as, "from crates.io");
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
    }

    #[test]
    fn a_windows_path_key_decodes_percent_escapes() {
        let json = r#"{"installs":{"vivac 0.15.5 (path+file:///H:/tmp/vivac%20src)":{"bins":["vivac.exe"]}}}"#;
        let c = cargo_install_from_json(json).expect("should be a cargo install");
        #[cfg(windows)]
        assert_eq!(c.command, "cargo install --path \"H:/tmp/vivac src\"");
        #[cfg(not(windows))]
        assert_eq!(c.command, "cargo install --path \"/H:/tmp/vivac src\"");
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
}
