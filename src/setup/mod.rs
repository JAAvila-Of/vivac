//! `vivac setup` — writes what a harness needs, after showing it.
//!
//! `t565` §7, amended by `t579` §4. This module holds what every harness
//! shares: the two roots a harness is set up against, the all-or-nothing
//! commit with its verification and rollback, the confirmation prompt, and
//! the fingerprint that tells an untouched generated file apart from an
//! edited one. `claude_code` is the one harness that exists today, with its
//! own files, its own hook shape and its own rules for "already there"
//! versus "conflict" (`t565` §9 on `INTEGRATION.md`).
//!
//! Not exposed over MCP: setup reforms the environment rather than
//! recording work, which is for whoever has a terminal, the same reason
//! `abandon` and `restore` stay off that server.

mod claude_code;
pub mod json;

use crate::args::Args;
use crate::failure::Failure;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

const HARNESSES: &[&str] = &["claude-code"];

pub fn dispatch(cwd: &Path, a: &Args) -> Result<i32, Failure> {
    if a.has("dry-run") && a.has("yes") {
        return Err(Failure::usage(
            "--dry-run writes nothing, so there is nothing for --yes to confirm.\n\n  \
             Give one or the other.",
        ));
    }
    if a.has("join") && a.has("new-tree") {
        return Err(Failure::usage(
            "--join joins a tree that already exists, and --new-tree plants a \
             separate one, so they contradict each other.\n\n  Give one or the other.",
        ));
    }
    // `f632`: `--join` takes a value, so with nothing after it the parser
    // records the flag as present and its value as absent, and every reader
    // downstream only ever asks for the value -- `opt("join")`, never
    // `has("join")`. Left unchecked, that fell straight through to the
    // plant branch and gave a second tree to someone who asked to join one.
    if a.has("join") && a.opt("join").is_none() {
        return Err(Failure::usage(
            "--join needs the project to join, and nothing followed it. Without \
             that word setup plants instead of joining, which is a second tree \
             for a product that already has one.\n\n  \
             vivac setup claude-code --join <project>",
        ));
    }
    if let [first, ..] = a.extra(1) {
        return Err(Failure::usage(format!(
            "setup does not take \"{first}\".\n\n  It takes one word of its own: the harness to set up."
        )));
    }
    let Some(harness) = a.positional(0) else {
        return Err(Failure::usage(
            "vivac setup needs the harness to set up:  vivac setup claude-code\n  \
             It knows claude-code today.",
        ));
    };
    match harness {
        "claude-code" => claude_code::run(&resolve_roots(cwd)?, a),
        other => Err(Failure::usage(format!(
            "vivac setup does not know \"{other}\" yet. It knows: {}",
            HARNESSES.join(", ")
        ))),
    }
}

/// The two roots setup writes into (`t579` §4). Claude Code never reads
/// `.claude/settings.json` or `.mcp.json` from a folder above the one it was
/// opened in, so its own files always go in `here`. The tree is shared
/// instead, the same one every other command finds: `tree` is the `.vivac/`
/// found walking up from `here`, or `here` itself when none exists yet, so a
/// fresh project gets one planted right where it is opened.
pub struct Roots {
    pub here: PathBuf,
    pub tree: PathBuf,
    /// `None` when there is no tree yet and `setup` is about to plant one.
    pub located: Option<crate::store::Located>,
}

/// Resolves both roots, and refuses if the tree root turns out to be the
/// very folder that holds the registry of every tree on the machine (`t565`
/// §7.2): a project's tree living inside that one would mix a project with
/// the registry that lists every project.
pub fn resolve_roots(cwd: &Path) -> Result<Roots, Failure> {
    let located = crate::store::locate(cwd)?;
    let tree = located
        .as_ref()
        .map(|l| l.root.clone())
        .unwrap_or_else(|| cwd.to_path_buf());
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let is_registry = crate::store::store_dir().is_some_and(|d| canon(&d) == canon(&tree));
    if is_registry {
        return Err(registry_refusal(&tree));
    }
    Ok(Roots {
        here: cwd.to_path_buf(),
        tree,
        located,
    })
}

/// The text `t565` §7.2 refuses with, naming `path`: shared between
/// `resolve_roots`, where `path` is the tree root itself, and
/// [`refuse_home_or_global_store`], where `path` is a `.vivac/` found
/// underneath it (`t579` §4.1).
fn registry_refusal(path: &Path) -> Failure {
    Failure::Model(format!(
        "  {} holds the registry of the trees on this machine, so it cannot\n  \
         hold a tree too. Run setup inside a project.",
        path.display()
    ))
}

/// Two more ways for the roots `resolve_roots` already accepted to still be
/// dangerous to write into, caught by `claude_code::run` before it decides
/// between planting and joining (`t579` §4.1, `f583`, `d584`). It used to be
/// caught by `apply` alone, which is exactly what let `--join` bypass it
/// (`t594`): a guard checked inside one branch is a guard
/// the next branch does not have. Both were reachable under 0.10.0, and
/// `--undo` never calls this: undoing whatever an earlier setup wrote there
/// is always safe, and is the only way out for whoever already fell into
/// either shape.
///
/// - `here` itself is the user's home folder: Claude Code's settings and
///   skills there belong to every project that opens in it, not to this
///   one, and the security pillar forbids setup from writing them there.
/// - `tree`'s own `.vivac/` is the global store: `resolve_roots` only
///   catches the tree root equalling `store_dir()` directly, but `find_root`
///   skips a global store it finds walking up and falls back to `here`, so
///   a global store can also turn up as the `.vivac/` `apply` would plant or
///   reuse right under the current directory.
pub fn refuse_home_or_global_store(roots: &Roots) -> Option<Failure> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if crate::store::home_dir().is_some_and(|h| canon(&h) == canon(&roots.here)) {
        return Some(Failure::Model(format!(
            "  {} is your home folder. Claude Code's settings and skills here are\n  \
             yours for every project, not this one's, and setup never writes there.\n  \
             Run setup in the folder you open Claude Code in, inside a project.",
            roots.here.display()
        )));
    }
    let vivac_dir = roots.tree.join(crate::store::DIR);
    let is_global_store = crate::registry::marks_global_store(&vivac_dir)
        || crate::store::store_dir().is_some_and(|d| canon(&d) == canon(&vivac_dir));
    if is_global_store {
        return Some(registry_refusal(&vivac_dir));
    }
    None
}

/// The closest strict ancestor of `dir` holding a `.git` entry, folder or
/// worktree file alike -- `dir` itself never counts. Filesystem only: `t579`
/// §4 forbids running git or any other program to answer this.
pub fn git_root_above(dir: &Path) -> Option<PathBuf> {
    let mut d = dir.to_path_buf();
    while d.pop() {
        if d.join(".git").exists() {
            return Some(d);
        }
    }
    None
}

/// What a `PlannedWrite` does to its file.
pub enum Action {
    Write(String),
    /// Removing the file entirely -- an `--undo` that empties a JSON file
    /// out, or a skill no longer wanted. Goes through the same commit as a
    /// write, so it shares its rollback.
    Delete,
}

/// A structural check on a JSON write, run against the file re-read and
/// re-parsed after writing: see [`PlannedWrite::preserved`].
type PreservedCheck = Box<dyn Fn(&json::Value) -> bool>;

/// One file `setup` is about to change, with what was there before so a
/// failed commit can put it back exactly.
pub struct PlannedWrite {
    pub path: PathBuf,
    pub action: Action,
    /// `None` for a file that does not exist yet: rollback removes it
    /// instead of restoring bytes that never existed.
    pub original: Option<Vec<u8>>,
    /// For a JSON write: checked against the file re-read and re-parsed
    /// after writing, in whichever direction this call goes -- a superset
    /// of the original for `apply`, a subsequence of it for `--undo`.
    /// `t565` §7.6's second check, on top of the byte-for-byte one every
    /// write already gets. `None` for the skill file and for every
    /// `Delete`, neither of which this applies to.
    pub preserved: Option<PreservedCheck>,
}

impl PlannedWrite {
    pub fn write(path: PathBuf, content: String, original: Option<Vec<u8>>) -> PlannedWrite {
        PlannedWrite {
            path,
            action: Action::Write(content),
            original,
            preserved: None,
        }
    }

    pub fn delete(path: PathBuf, original: Vec<u8>) -> PlannedWrite {
        PlannedWrite {
            path,
            action: Action::Delete,
            original: Some(original),
            preserved: None,
        }
    }
}

fn sibling_temp(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// A temporary file next to `path`, renamed over it -- `write_config_atomic`'s
/// own shape, so a process that dies mid-write leaves the original untouched.
///
/// A temporary left behind by a failed write or rename is removed before the
/// error goes up. It holds the whole new file, and a settings file can carry
/// credentials in its `env` block: a `settings.json.tmp` lying next to the
/// original is exactly the copy under another name that §7.6 keeps setup
/// from ever making.
fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sibling_temp(path);
    let written = std::fs::write(&tmp, content).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn apply_action(w: &PlannedWrite) -> std::io::Result<()> {
    match &w.action {
        Action::Write(content) => write_atomic(&w.path, content.as_bytes()),
        Action::Delete => remove_if_present(&w.path),
    }
}

/// Puts `w` back exactly as it was: whether it was written or deleted makes
/// no difference to undoing it, only what `original` says was there.
fn restore_one(w: &PlannedWrite) -> std::io::Result<()> {
    match &w.original {
        Some(bytes) => write_atomic(&w.path, bytes),
        None => remove_if_present(&w.path),
    }
}

fn verify_one(w: &PlannedWrite) -> bool {
    match &w.action {
        Action::Write(content) => {
            let Ok(actual) = std::fs::read(&w.path) else {
                return false;
            };
            if actual != content.as_bytes() {
                return false;
            }
            match &w.preserved {
                Some(check) => std::str::from_utf8(&actual)
                    .ok()
                    .and_then(|s| json::parse(s).ok())
                    .is_some_and(|v| check(&v)),
                None => true,
            }
        }
        Action::Delete => !w.path.exists(),
    }
}

/// Rolls every one of `writes` back, in reverse, and returns the paths it
/// could not restore. `t565` §7.6: the order matters because a later write
/// can depend on an earlier one existing (a directory it lives in).
///
/// Public so a caller whose own next step fails *after* a successful commit
/// -- planting `.vivac/`, say -- can undo the commit by hand and report it
/// with [`failure_with_rollback`], the same way a failure inside the commit
/// itself does.
pub fn rollback(writes: &[PlannedWrite]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for w in writes.iter().rev() {
        if restore_one(w).is_err() {
            failed.push(w.path.clone());
        }
    }
    failed
}

enum Cause {
    Write(std::io::Error),
    Verify,
}

/// The exit-5 text for a rollback that just ran: `clause` names what went
/// wrong, and `unrestored` lists whatever the rollback itself could not put
/// back -- empty in the common case, where it says so plainly instead.
pub fn failure_with_rollback(clause: String, unrestored: &[PathBuf]) -> Failure {
    let message = if unrestored.is_empty() {
        format!("{clause}, so setup put every file it\n  touched back as it was.")
    } else {
        let mut m = format!("{clause}, and setup could not put these back as they were:\n");
        for p in unrestored {
            m.push_str(&format!("      {}\n", p.display()));
        }
        m.push_str(
            "  setup keeps no copy on disk, so the only other copy is whatever\n  \
             version control holds.",
        );
        m
    };
    Failure::Io(std::io::Error::other(message))
}

fn rollback_failure(path: &Path, cause: Cause, writes: &[PlannedWrite]) -> Failure {
    let unrestored = rollback(writes);
    let clause = match cause {
        Cause::Write(e) => format!("{} could not be written ({e})", path.display()),
        Cause::Verify => format!("{} did not read back as written", path.display()),
    };
    failure_with_rollback(clause, &unrestored)
}

/// Applies every one of `writes`, in order; on any failure to write or to
/// verify, rolls every one of them back and fails instead of leaving a
/// partial result. `t565` §7.6: this is what makes the four pieces all or
/// nothing, the same promise the security pillar was given.
pub fn commit(writes: &[PlannedWrite]) -> Result<(), Failure> {
    for (i, w) in writes.iter().enumerate() {
        if let Err(e) = apply_action(w) {
            // `w` itself never changed -- a failed write leaves its target
            // exactly as it was, since `write_atomic`'s last step is one
            // rename -- so only the entries before it need rolling back.
            return Err(rollback_failure(&w.path, Cause::Write(e), &writes[..i]));
        }
    }
    for w in writes {
        if !verify_one(w) {
            return Err(rollback_failure(&w.path, Cause::Verify, writes));
        }
    }
    Ok(())
}

/// Whether typing `y` or `yes`, in any mix of case, answered a question.
/// Split out so the interpretation has a test that never has to fake a
/// terminal.
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Prints `prompt`, flushes it out with no line of its own, and reads one
/// line of the answer. Only ever called once a terminal is known to be
/// there -- `--yes` and the no-terminal refusal both skip this entirely.
pub fn ask(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    is_yes(&line)
}

pub fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// 64-bit FNV-1a over `data`. Not a cryptographic hash and not meant to be
/// one: it only has to tell an untouched generated file apart from an
/// edited one, and that is what keeps this from pulling in a dependency
/// (`t565` §7.4).
pub fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_the_reference_vectors() {
        // The two vectors fnvhash.info publishes for FNV-1a/64.
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn is_yes_takes_any_case_of_y_or_yes() {
        for s in ["y", "Y", "yes", "YES", "Yes", "  y  ", "y\n"] {
            assert!(is_yes(s), "{s:?} should count as yes");
        }
        for s in ["n", "no", "", "yep", "sure"] {
            assert!(!is_yes(s), "{s:?} should not count as yes");
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vivac-setup-{name}-{}", crate::id::ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A rename that fails must not leave the temporary behind: it holds the
    /// whole new file, credentials in `env` included, under another name.
    /// A directory where the file should go makes the rename fail on every
    /// platform.
    #[test]
    fn a_failed_rename_leaves_no_temporary_behind() {
        let dir = temp_dir("rename-fails");
        let target = dir.join("settings.json");
        std::fs::create_dir_all(target.join("occupied")).unwrap();
        assert!(write_atomic(&target, b"{\"env\":{}}").is_err());
        assert!(
            !sibling_temp(&target).exists(),
            "the temporary survived the failed rename"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_committed_write_leaves_every_file_as_written() {
        let dir = temp_dir("commit-ok");
        let writes = vec![
            PlannedWrite::write(dir.join("a.txt"), "A".to_string(), None),
            PlannedWrite::write(dir.join("b.txt"), "B".to_string(), None),
        ];
        commit(&writes).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "B");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `t565` §11 test 15, and the coordinator's point A: a write that fails
    /// on the second file rolls the first one back too, exactly like a
    /// verification failure would. A directory sitting where the second
    /// file needs to be a file is the fault: `write_atomic`'s rename onto it
    /// fails on every platform this runs on, unlike a permission bit.
    #[test]
    fn a_write_failure_rolls_the_earlier_files_back() {
        let dir = temp_dir("commit-write-fails");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        std::fs::write(&first, "original first").unwrap();
        std::fs::create_dir(&second).unwrap(); // the fault

        let writes = vec![
            PlannedWrite::write(
                first.clone(),
                "new first".into(),
                Some(b"original first".to_vec()),
            ),
            PlannedWrite::write(second.clone(), "new second".into(), None),
        ];
        let err = commit(&writes).unwrap_err();
        let msg = err.message();
        assert!(msg.contains("could not be written"), "{msg}");
        assert!(msg.contains("put every file it"), "{msg}");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "original first");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_verification_failure_rolls_every_file_back() {
        let dir = temp_dir("commit-verify-fails");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        std::fs::write(&first, "original first").unwrap();

        // `commit` only ever writes `content` itself, so the only way to
        // reach the verification path is a write whose declared content
        // does not match what actually lands on disk -- the shape of a
        // `preserved` check failing.
        let writes = vec![
            PlannedWrite::write(
                first.clone(),
                "new first".into(),
                Some(b"original first".to_vec()),
            ),
            PlannedWrite {
                path: second.clone(),
                action: Action::Write("new second".into()),
                original: None,
                preserved: Some(Box::new(|_| false)),
            },
        ];
        let err = commit(&writes).unwrap_err();
        assert!(err.message().contains("did not read back as written"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "original first");
        assert!(
            !second.exists(),
            "a file with no original should be removed"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Point A's other half: `--undo` deleting a file is part of the same
    /// transaction, and a delete that cannot complete rolls a write next to
    /// it back too.
    #[test]
    fn a_failed_delete_rolls_a_sibling_write_back() {
        let dir = temp_dir("commit-delete-fails");
        let written = dir.join("written.txt");
        let undone = dir.join("undone.txt");
        std::fs::write(&written, "original written").unwrap();
        std::fs::create_dir(&undone).unwrap(); // remove_file fails on a directory

        let writes = vec![
            PlannedWrite::write(
                written.clone(),
                "new written".into(),
                Some(b"original written".to_vec()),
            ),
            PlannedWrite::delete(undone.clone(), b"{}".to_vec()),
        ];
        let err = commit(&writes).unwrap_err();
        assert!(
            err.message().contains("could not be written")
                || err.message().contains("Input/output")
        );
        assert_eq!(
            std::fs::read_to_string(&written).unwrap(),
            "original written"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// When rollback itself cannot put a file back, the failure says so and
    /// names it, instead of a false "put back as it was".
    #[test]
    fn an_unrestorable_file_is_named_rather_than_claimed_fixed() {
        let dir = temp_dir("commit-unrestorable");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        std::fs::write(&second, "second").unwrap();
        // `first`'s rollback target cannot be written to at all: a
        // directory sits where the restored file would need to be.
        std::fs::create_dir(&first).unwrap();

        let writes = vec![
            PlannedWrite::write(
                first.clone(),
                "new first".into(),
                Some(b"original first".to_vec()),
            ),
            PlannedWrite::write(
                second.clone(),
                "new second".into(),
                Some(b"second".to_vec()),
            ),
        ];
        let unrestored = rollback(&writes);
        assert_eq!(unrestored, vec![first.clone()]);

        let msg = failure_with_rollback("something failed".to_string(), &unrestored).message();
        assert!(
            msg.contains("could not put these back as they were"),
            "{msg}"
        );
        assert!(msg.contains(&first.display().to_string()), "{msg}");
        assert!(msg.contains("version control"), "{msg}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
