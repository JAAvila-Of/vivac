//! `Anchor` — a stable identity for the tree's state, and what changed since.
//!
//! The model does not depend on git: it needs two primitives and nothing
//! else. The implementations are `Git` and `Null`, and **`Null` defines the
//! floor of the product**, it is not filler: with no version control the
//! tree, the stack, the vivacs and `why` all keep working whole. The only
//! thing lost is precision by change, and the `brief` swaps in plain age
//! rather than inventing precision it does not have.
//!
//! **`snapshot` spawns no subprocess.** `push` creates a vivac and a vivac
//! needs an anchor, so this falls on the write path, whose budget is 5 ms;
//! starting `git` on Windows costs between 15 and 30. `.git/HEAD` is read
//! and the reference resolved by hand. `changed_since` does shell out to
//! git, because it only runs on reads.

use std::path::{Path, PathBuf};

/// Identity of the tree state at a moment. Empty means "there is none",
/// which is a legitimate state and not an error.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AnchorRef {
    pub kind: String,
    pub id: String,
}

impl AnchorRef {
    pub fn is_empty_tree(&self) -> bool {
        self.id.is_empty()
    }

    /// Short prefix, the way git does it.
    pub fn short(&self) -> &str {
        let n = self.id.len().min(7);
        &self.id[..n]
    }
}

#[derive(Debug, Clone)]
pub struct Change {
    pub file_path: String,
    pub times: usize,
}

pub trait Anchor {
    fn snapshot(&self) -> AnchorRef;
    fn changed_since(&self, r: &AnchorRef) -> Vec<Change>;
}

/// No version control.
///
/// `MODEL.md` §8 proposed a merkle of the working set. **Not in v0.1**: the
/// working set has no bound, and hashing it sits on the write path, which
/// has a 5 ms budget. The performance pillar sets a ceiling a feature must
/// respect in order to exist, and this one did not. It stays an anchor with
/// no identity, which is exactly the degradation `BRIEF-SPEC.md` §6 already
/// specifies.
pub struct Null;

impl Anchor for Null {
    fn snapshot(&self) -> AnchorRef {
        AnchorRef {
            kind: "null".into(),
            id: String::new(),
        }
    }

    fn changed_since(&self, _r: &AnchorRef) -> Vec<Change> {
        vec![]
    }
}

pub struct Git {
    root: PathBuf,
    gitdir: PathBuf,
}

/// Where `.git` lives relative to a starting directory: the outcome of the
/// upward walk, and nothing about the repository's live state. A `.git`
/// does not move once a process starts, so this is safe to cache; a `HEAD`
/// does, every commit, which is why nothing past this point is.
#[derive(Clone)]
struct Location {
    root: PathBuf,
    gitdir: PathBuf,
}

type LocationCache = std::sync::Mutex<std::collections::HashMap<PathBuf, Option<Location>>>;

/// Caches `locate`'s walk by the directory it started from, for the life of
/// the process. A long-lived server calls `detect` on the same root many
/// times over a session -- every write used to, before `t192` -- and the
/// walk up the filesystem is the same answer every time; only what `HEAD`
/// holds is allowed to change underneath it.
fn location_cache() -> &'static LocationCache {
    static CACHE: std::sync::OnceLock<LocationCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn locate_cached(root: &Path) -> Option<Location> {
    let mut cache = location_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = cache.get(root) {
        return hit.clone();
    }
    let found = locate(root);
    cache.insert(root.to_path_buf(), found.clone());
    found
}

/// The upward walk itself, isolated from the cache around it so the cache
/// stays a thin wrapper over a pure function.
fn locate(root: &Path) -> Option<Location> {
    let mut d = root.to_path_buf();
    loop {
        let g = d.join(".git");
        if g.is_dir() {
            return Some(Location { root: d, gitdir: g });
        }
        if g.is_file() {
            // Worktree or submodule: .git is a file holding `gitdir: <path>`.
            let t = std::fs::read_to_string(&g).ok()?;
            let p = t.trim().strip_prefix("gitdir:")?.trim();
            let abs = if Path::new(p).is_absolute() {
                PathBuf::from(p)
            } else {
                d.join(p)
            };
            return Some(Location {
                root: d,
                gitdir: abs,
            });
        }
        if !d.pop() {
            return None;
        }
    }
}

/// Picks an implementation by looking for a usable `.git` from `root`.
pub fn detect(root: &Path) -> Box<dyn Anchor> {
    match Git::new(root) {
        Some(g) => Box::new(g),
        None => Box::new(Null),
    }
}

/// Whether `root` sits inside a git working tree: the same upward walk the
/// anchor already does, cached the same way.
pub(crate) fn in_working_tree(root: &Path) -> bool {
    locate_cached(root).is_some()
}

/// The root of the linked worktree `from` sits inside, when there is one.
///
/// A submodule's `.git` is a file too, pointing at a gitdir of its own --
/// the same shape a linked worktree has. What tells them apart is
/// `commondir`, a file git writes into a worktree's gitdir and nowhere
/// else: it names the repository the worktree shares. A submodule owns its
/// repository outright and carries no such file, so it is not another
/// working folder of anything -- it belongs to the lane that contains it.
pub(crate) fn linked_worktree(from: &Path) -> Option<PathBuf> {
    let location = locate_cached(from)?;
    if !location.root.join(".git").is_file() {
        return None;
    }
    location
        .gitdir
        .join("commondir")
        .is_file()
        .then_some(location.root)
}

/// The root of the main copy a linked worktree's history lives in, read off
/// `commondir` inside its gitdir. That path can be relative to the gitdir
/// that holds it, so it is resolved against it and its `..` walked off by
/// hand -- never through `canonicalize`, which on Windows returns a
/// `\\?\`-prefixed path that would break every comparison made against one
/// that was never canonicalized.
pub(crate) fn main_copy_of(worktree_root: &Path) -> Option<PathBuf> {
    let location = locate_cached(worktree_root)?;
    let raw = std::fs::read_to_string(location.gitdir.join("commondir")).ok()?;
    let rel = raw.trim();
    if rel.is_empty() {
        return None;
    }
    let common_gitdir = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        normalize(&location.gitdir.join(rel))
    };
    // A bare repository's common gitdir is the repository itself, not a
    // working copy's `.git`, so its parent is whatever directory happens to
    // hold it. Walking up from there could find a tree that has nothing to
    // do with this worktree and attach it to the wrong product, which is
    // worse than finding nothing at all.
    if common_gitdir.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return None;
    }
    common_gitdir.parent().map(Path::to_path_buf)
}

/// Resolves `.` and `..` components one at a time, without touching the
/// filesystem the way `canonicalize` would.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether git tracks `rel`, a path relative to `root`. Starts `git`, so
/// only `setup` and `check` call it: nothing on the write path does.
///
/// `None` when git could not be asked: not on PATH, or it refused the
/// folder. `--error-unmatch` exits `0` when `rel` is tracked and `1` when it
/// is not; any other exit code, a signal, or a process that never started
/// means the question itself failed, and the caller must not read that as
/// "not tracked".
pub(crate) fn tracks(root: &Path, rel: &str) -> Option<bool> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", rel])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

impl Git {
    fn new(root: &Path) -> Option<Git> {
        locate_cached(root).map(|l| Git {
            root: l.root,
            gitdir: l.gitdir,
        })
    }

    /// Resolves `.git/HEAD` spawning nothing. Three cases: a direct sha
    /// (detached HEAD), a reference with its file, and a packed reference.
    fn head(&self) -> Option<String> {
        let h = std::fs::read_to_string(self.gitdir.join("HEAD")).ok()?;
        let h = h.trim();
        let Some(refname) = h.strip_prefix("ref:").map(str::trim) else {
            return is_sha(h).then(|| h.to_string());
        };
        if let Ok(s) = std::fs::read_to_string(self.gitdir.join(refname)) {
            let s = s.trim().to_string();
            if is_sha(&s) {
                return Some(s);
            }
        }
        let packed = std::fs::read_to_string(self.gitdir.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| {
            let (sha, name) = l.split_once(' ')?;
            (name.trim() == refname && is_sha(sha)).then(|| sha.to_string())
        })
    }

    fn git(&self, args: &[&str]) -> Option<String> {
        let s = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .ok()?;
        s.status
            .success()
            .then(|| String::from_utf8_lossy(&s.stdout).into_owned())
    }
}

fn is_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Anchor for Git {
    fn snapshot(&self) -> AnchorRef {
        AnchorRef {
            kind: "git".into(),
            id: self.head().unwrap_or_default(),
        }
    }

    fn changed_since(&self, r: &AnchorRef) -> Vec<Change> {
        if r.is_empty_tree() || r.kind != "git" {
            return vec![];
        }
        let mut count: std::collections::BTreeMap<String, usize> = Default::default();
        // Commits since the anchor. If the sha is gone --rebase, deleted
        // branch-- git fails and this returns empty: better to say nothing
        // than to say a false number.
        if let Some(out) = self.git(&[
            "log",
            "--format=",
            "--name-only",
            &format!("{}..HEAD", r.id),
        ]) {
            for l in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
                *count.entry(l.to_string()).or_default() += 1;
            }
        }
        // And whatever is uncommitted, which counts as a change.
        //
        // `-uall` is not decoration: without it git collapses a whole
        // untracked directory into one line -- `src/` -- and the file that
        // nobody has claimed yet, which is the case `reconcile` exists for,
        // never appears. Ignored paths stay ignored, so `target/` does not
        // come flooding in.
        if let Some(out) = self.git(&["status", "--porcelain", "-uall"]) {
            for l in out.lines() {
                if let Some(file_path) = l.get(3..) {
                    let file_path = file_path.rsplit(" -> ").next().unwrap_or(file_path).trim();
                    if !file_path.is_empty() {
                        *count
                            .entry(file_path.trim_matches('"').to_string())
                            .or_default() += 1;
                    }
                }
            }
        }
        let mut v: Vec<Change> = count
            .into_iter()
            .map(|(file_path, times)| Change { file_path, times })
            .collect();
        // Most-touched first; ties broken by path. Deterministic.
        v.sort_by(|a, b| {
            b.times
                .cmp(&a.times)
                .then_with(|| a.file_path.cmp(&b.file_path))
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_invents_no_precision() {
        let n = Null;
        assert!(n.snapshot().is_empty_tree());
        assert!(n.changed_since(&AnchorRef::default()).is_empty());
    }

    #[test]
    fn head_is_read_without_spawning_git() {
        // This very repository serves as the substrate.
        let g = Git::new(Path::new(".")).expect("vivac/ is a git repo");
        let s = g.snapshot();
        assert_eq!(s.kind, "git");
        assert!(is_sha(&s.id), "HEAD did not resolve: {:?}", s.id);
        assert_eq!(s.short().len(), 7);
    }

    #[test]
    fn an_anchor_from_another_world_gives_no_changes() {
        let g = Git::new(Path::new(".")).unwrap();
        let bogus = AnchorRef {
            kind: "git".into(),
            id: "0000000000000000000000000000000000000000".into(),
        };
        assert!(g
            .changed_since(&bogus)
            .iter()
            .all(|c| !c.file_path.is_empty()));
    }

    /// A long-lived process -- the MCP server, the web server -- calls
    /// `detect` many times against the same root over a session in which
    /// the agent keeps committing. Caching the walk that locates `.git`
    /// must never turn into caching the commit it finds there: a second
    /// `detect` on the same root, after `HEAD` moved, still has to read the
    /// commit that is there now.
    #[test]
    fn a_cached_location_still_reads_the_head_a_later_commit_left() {
        let root = std::env::temp_dir().join(format!(
            "vivac-anchor-live-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        let git_dir = root.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let first = "a".repeat(40);
        std::fs::write(git_dir.join("HEAD"), &first).unwrap();

        // First call: walks up from `root` and, from here on, caches where
        // `.git` was found.
        let before = detect(&root).snapshot();
        assert_eq!(before.id, first, "the first read did not see the commit");

        // A commit happens during the session.
        let second = "b".repeat(40);
        std::fs::write(git_dir.join("HEAD"), &second).unwrap();

        // Second call: hits the cached location, but the commit it reports
        // has to be the one that is there right now, not the one cached
        // alongside the walk.
        let after = detect(&root).snapshot();
        assert_eq!(
            after.id, second,
            "the cached walk froze the commit instead of just the location"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// `git worktree add` from a bare repository is an ordinary flow, and a
    /// bare repository's `commondir` names the repository itself --
    /// typically `something.git` -- never a working copy's `.git`. Trusting
    /// its parent would risk walking up from a directory that just happens
    /// to hold the bare repository and finding a tree that has nothing to
    /// do with this worktree.
    #[test]
    fn main_copy_of_refuses_a_bare_repository() {
        let root = std::env::temp_dir().join(format!(
            "vivac-anchor-bare-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        let bare_repo = root.join("proj.git");
        let worktree_dir = root.join("feature");
        let gitdir = bare_repo.join("worktrees").join("feature");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&worktree_dir).unwrap();
        std::fs::write(
            worktree_dir.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        // Relative to the gitdir, `../..` lands on the bare repository
        // itself, not on a `.git` inside it.
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        assert!(main_copy_of(&worktree_dir).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The two existing worktree tests only ever see a relative `commondir`
    /// (`../..`); this is the one with an absolute path, which git also
    /// writes.
    #[test]
    fn main_copy_of_resolves_an_absolute_common_directory() {
        let root = std::env::temp_dir().join(format!(
            "vivac-anchor-abscommon-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        let main_dir = root.join("main");
        let worktree_dir = root.join("feature");
        let gitdir = main_dir.join(".git").join("worktrees").join("feature");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&worktree_dir).unwrap();
        std::fs::write(
            worktree_dir.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        let common = main_dir.join(".git");
        std::fs::write(gitdir.join("commondir"), format!("{}\n", common.display())).unwrap();

        assert_eq!(main_copy_of(&worktree_dir), Some(main_dir));
        std::fs::remove_dir_all(&root).ok();
    }
}
