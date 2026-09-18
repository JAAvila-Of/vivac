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
//!
//! **Also where the crate answers "do two paths name the same folder?"**
//! (`same_folder`), a question this module already had to work out for its
//! own worktree-following (`main_copy_of`) before `f612` gave it a second
//! caller in `registry.rs`. A case difference, an alias or a link naming
//! one real directory twice has nothing to do with git identity, but the
//! two questions share one home rather than the same fix living twice.

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

/// Where one repository is right now, read off files and spawning
/// nothing: this runs inside the write lock, on the write path.
///
/// `where_of` and the pieces under it have no caller outside this module's
/// own tests yet: `t594` tramo 4's task 1 is the reader alone, and tasks 2
/// through 4 are what write and read `where.changed` through it. Until one
/// of them lands, nothing outside `#[cfg(test)]` calls in, and rustc's own
/// dead-code detection -- the one `t594`'s plan already leans on for the
/// write-only fields of §2.7 -- catches a whole unreachable function just
/// as well as an unread field, hence the `allow` here and below.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) struct Head {
    /// The branch `HEAD` points at. Absent with a detached `HEAD`.
    pub branch: Option<String>,
    /// The commit `HEAD` resolves to. Absent when nothing could be read --
    /// an unborn branch, or a `HEAD` this process cannot make sense of.
    pub sha: Option<String>,
    /// A rebase is under way, so `branch` is the branch being rebased and
    /// the sha moves once per commit replayed. Written so that a rebase
    /// does not produce one event per commit (§2.4).
    pub rebasing: bool,
}

/// What a lane's declared repository answers when asked where it is.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum Where {
    Head(Head),
    /// The folder a lane declared holds no repository any more.
    Missing,
}

/// Where the repository at `repo_root` is. `Missing` when the folder is
/// not a working tree at all -- the lane declared it and it is gone --
/// which is a different answer from a `HEAD` that could not be read.
#[allow(dead_code)]
pub(crate) fn where_of(repo_root: &Path) -> Where {
    let Some(g) = Git::new(repo_root) else {
        return Where::Missing;
    };
    Where::Head(g.where_now())
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
/// `commondir` inside its gitdir.
pub(crate) fn main_copy_of(worktree_root: &Path) -> Option<PathBuf> {
    let location = locate_cached(worktree_root)?;
    let common_gitdir = common_gitdir(&location.gitdir)?;
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

/// The gitdir a reference has to be read from. A linked worktree keeps
/// `HEAD` in its own gitdir and every branch in the repository
/// `commondir` names, so `refs/heads/<branch>` is never under the
/// worktree's own gitdir and neither is `packed-refs` (`f439`). With no
/// `commondir` -- an ordinary checkout, or a submodule, which owns its
/// repository outright -- the gitdir is its own.
fn ref_gitdir(gitdir: &Path) -> PathBuf {
    match common_gitdir(gitdir) {
        Some(common) => common,
        None => gitdir.to_path_buf(),
    }
}

/// The common gitdir `commondir` names, resolved against the gitdir that
/// holds it. The path can be relative, and its `..` are walked off by hand
/// rather than through `canonicalize`, which on Windows returns a
/// `\\?\`-prefixed path that would break every comparison made against one
/// that was never canonicalized.
fn common_gitdir(gitdir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let rel = raw.trim();
    if rel.is_empty() {
        return None;
    }
    Some(if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        normalize(&gitdir.join(rel))
    })
}

/// Resolves `.` and `..` components one at a time, without touching the
/// filesystem the way `canonicalize` would.
///
/// `pub(crate)` rather than private for three callers outside this module,
/// and none of them is comparing two folders -- that is `same_folder`'s
/// job, below, and it is the criterion anybody asking whether two paths
/// are the same folder actually wants. These three want the lexical walk
/// itself, each for a reason of its own:
///
/// - `relocate::is_inside` walks a path's own ancestors, and an unresolved
///   `..` in the middle of it makes `Path::ancestors` treat that component
///   as just another name to strip rather than an instruction to go up
///   past the one before it -- exactly the bug `relocate ..` surfaced once
///   `is_inside` compared raw ancestors.
/// - `relocate::to_absolute` joins a destination that usually does not
///   exist yet, so `canonicalize` is not available to it at all; without
///   this, `vivac relocate ..` wrote `…\clone\..` into the registry
///   outright, and `Path::file_name` of a path ending in `..` is `None`.
/// - `registry::absolute` does the same for `--project`'s own value, which
///   is looser still: it may name no path on this disk at all, so asking
///   the filesystem about it before anything has established that it
///   exists would be asking the wrong question.
///
/// In all three the result is written down or walked, never compared
/// against a second spelling of the same folder.
pub(crate) fn normalize(p: &Path) -> PathBuf {
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

/// Whether `a` and `b` name the same folder on disk, even when they are
/// spelled differently. `normalize` above is the fast path -- a pure
/// string walk, no filesystem cost -- and it is enough for the ordinary
/// case of `.`/`..` and mixed separators. What it cannot catch is a
/// genuine difference in spelling: a case difference, an 8.3 alias, a
/// symlink, or a junction, all of which name the same directory while
/// disagreeing textually.
///
/// This is `f612`, closed once rather than patched twice: `ops::repo_at`
/// hit it first, comparing a path this process joined by hand against one
/// `anchor::main_copy_of` read out of files git itself wrote, and
/// `registry::path_disagrees` hit it again comparing a path the registry
/// wrote down at an earlier `cd` against the one the current `cd` spells
/// now -- two different sources for the same failure, which is exactly
/// why this lives here once rather than being fixed a third time
/// somewhere else.
///
/// `canonicalize` runs only as a fallback, after `normalize` already
/// disagrees, and its two results are compared and dropped in this same
/// expression -- never stored, returned, or logged anywhere. On Windows
/// `canonicalize` returns a `\\?\`-prefixed path that would poison any
/// comparison it survived into; nothing here outlives this call, so there
/// is nothing left to poison. Either side failing to canonicalize
/// (missing, no permission) answers `false`, the same as the textual
/// check alone would have.
///
/// For `repo_at`'s own comparison this fallback is not just convenient,
/// it is correct: git always resolves what it writes to one canonical
/// spelling, so the two sides genuinely do name the same folder whenever
/// `canonicalize` agrees. The registry's own callers carry no such
/// promise -- both sides are just whatever some `cd` happened to spell --
/// so there `canonicalize` is the best answer available, not a proof.
pub(crate) fn same_folder(a: &Path, b: &Path) -> bool {
    if normalize(a) == normalize(b) {
        return true;
    }
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
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
        let refs = ref_gitdir(&self.gitdir);
        if let Ok(s) = std::fs::read_to_string(refs.join(refname)) {
            let s = s.trim().to_string();
            if is_sha(&s) {
                return Some(s);
            }
        }
        let packed = std::fs::read_to_string(refs.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| {
            let (sha, name) = l.split_once(' ')?;
            (name.trim() == refname && is_sha(sha)).then(|| sha.to_string())
        })
    }

    /// The branch, the sha and whether a rebase is under way. The branch
    /// comes from `HEAD` unless git is replaying commits, in which case
    /// `HEAD` is detached and the branch being rebased is in
    /// `rebase-merge/head-name` (an interactive or merge rebase) or
    /// `rebase-apply/head-name` (`git am`, and `--apply`).
    #[allow(dead_code)] // called through `where_of`, whose own doc explains the gap.
    fn where_now(&self) -> Head {
        let sha = self.head();
        if let Some(branch) = self.rebasing_onto() {
            return Head {
                branch: Some(branch),
                sha,
                rebasing: true,
            };
        }
        Head {
            branch: self.head_branch(),
            sha,
            rebasing: false,
        }
    }

    /// The branch `HEAD` names, without its `refs/heads/` prefix. `None`
    /// with a detached `HEAD`, which is a value and not a failure.
    #[allow(dead_code)] // called through `where_of`, whose own doc explains the gap.
    fn head_branch(&self) -> Option<String> {
        let h = std::fs::read_to_string(self.gitdir.join("HEAD")).ok()?;
        let refname = h.trim().strip_prefix("ref:")?.trim().to_string();
        Some(short_branch(&refname))
    }

    /// The branch a rebase in progress is replaying onto its own tip.
    #[allow(dead_code)] // called through `where_of`, whose own doc explains the gap.
    fn rebasing_onto(&self) -> Option<String> {
        for dir in ["rebase-merge", "rebase-apply"] {
            let f = self.gitdir.join(dir).join("head-name");
            if let Ok(name) = std::fs::read_to_string(f) {
                let name = name.trim();
                if !name.is_empty() {
                    return Some(short_branch(name));
                }
            }
        }
        None
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

/// `refs/heads/feature/net10` -> `feature/net10`. A name that does not
/// start that way is kept whole: it is still what the checkout says it is
/// on, and inventing a shorter one would name a branch that does not
/// exist.
#[allow(dead_code)] // called through `where_of`, whose own doc explains the gap.
fn short_branch(refname: &str) -> String {
    refname
        .strip_prefix("refs/heads/")
        .unwrap_or(refname)
        .to_string()
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

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vivac-anchor-{name}-{}", crate::id::ulid()))
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_repo_with_one_commit(at: &Path) {
        std::fs::create_dir_all(at).unwrap();
        git(at, &["init", "-q"]);
        git(at, &["config", "user.email", "t@example.com"]);
        git(at, &["config", "user.name", "t"]);
        std::fs::write(at.join("f.txt"), "x").unwrap();
        git(at, &["add", "."]);
        git(at, &["commit", "-q", "-m", "first"]);
    }

    #[test]
    fn head_resolves_a_linked_worktrees_branch_through_commondir() {
        // A linked worktree keeps its own `HEAD` in its own gitdir and shares
        // every branch with the repository `commondir` names. Reading the
        // reference from the worktree's gitdir finds nothing: that is `f439`,
        // and it is why a worktree on a branch used to anchor nothing.
        let t = tmp("commondir-head");
        let main = t.join("main");
        let worktree = t.join("side");
        git_repo_with_one_commit(&main);
        git(
            &main,
            &["worktree", "add", worktree.to_str().unwrap(), "-b", "side"],
        );

        let g = Git::new(&worktree).expect("the worktree is a working tree");

        assert!(
            g.head().is_some(),
            "a worktree on a branch has a HEAD like any other checkout"
        );
    }

    #[test]
    fn a_rebase_in_progress_names_the_branch_being_rebased() {
        // Git detaches HEAD while it replays commits, so a rebase would look
        // like a new detached sha on every commit and write one event each
        // (§2.4). The branch name is in `rebase-merge/head-name`, and that is
        // what the lane is on.
        let t = tmp("rebase-merge");
        git_repo_with_one_commit(&t);
        let gitdir = t.join(".git");
        std::fs::create_dir_all(gitdir.join("rebase-merge")).unwrap();
        std::fs::write(
            gitdir.join("rebase-merge").join("head-name"),
            "refs/heads/side\n",
        )
        .unwrap();

        let w = where_of(&t);
        let Where::Head(h) = w else {
            panic!("the repository is there")
        };

        assert_eq!(h.branch.as_deref(), Some("side"));
        assert!(h.rebasing, "a rebase is under way");
    }

    #[test]
    fn a_loose_reference_gives_its_branch_and_sha() {
        let t = tmp("loose-ref");
        git_repo_with_one_commit(&t);
        git(&t, &["checkout", "-q", "-b", "develop"]);

        let h = Git::new(&t).unwrap().where_now();

        assert_eq!(h.branch.as_deref(), Some("develop"));
        assert!(h.sha.as_deref().is_some_and(is_sha));
        assert!(!h.rebasing);
    }

    #[test]
    fn a_packed_reference_gives_its_branch_and_sha() {
        let t = tmp("packed-ref");
        git_repo_with_one_commit(&t);
        git(&t, &["checkout", "-q", "-b", "develop"]);
        git(&t, &["pack-refs", "--all"]);
        // `pack-refs` already removes the loose file it packed on every git
        // version this crate supports, but the removal is what actually
        // routes this test through `packed-refs`, so it is done by hand too.
        std::fs::remove_file(t.join(".git").join("refs").join("heads").join("develop")).ok();

        let h = Git::new(&t).unwrap().where_now();

        assert_eq!(h.branch.as_deref(), Some("develop"));
        assert!(h.sha.as_deref().is_some_and(is_sha));
        assert!(!h.rebasing);
    }

    #[test]
    fn a_detached_head_gives_a_sha_and_no_branch() {
        let t = tmp("detached-head");
        git_repo_with_one_commit(&t);
        let sha = Git::new(&t)
            .unwrap()
            .head()
            .expect("a fresh commit resolves");
        git(&t, &["checkout", "-q", &sha]);

        let h = Git::new(&t).unwrap().where_now();

        assert_eq!(h.branch, None);
        assert_eq!(h.sha.as_deref(), Some(sha.as_str()));
        assert!(!h.rebasing);
    }

    #[test]
    fn a_rebase_apply_names_the_branch_too() {
        // `git am` and a plain `git rebase --apply` leave the branch being
        // rebased in `rebase-apply/head-name`, the older backend's version
        // of the file `rebase-merge` writes.
        let t = tmp("rebase-apply");
        git_repo_with_one_commit(&t);
        let gitdir = t.join(".git");
        std::fs::create_dir_all(gitdir.join("rebase-apply")).unwrap();
        std::fs::write(
            gitdir.join("rebase-apply").join("head-name"),
            "refs/heads/side\n",
        )
        .unwrap();

        let w = where_of(&t);
        let Where::Head(h) = w else {
            panic!("the repository is there")
        };

        assert_eq!(h.branch.as_deref(), Some("side"));
        assert!(h.rebasing, "a rebase is under way");
    }

    #[test]
    fn a_worktree_with_a_detached_head_gives_no_branch() {
        let t = tmp("worktree-detached");
        let main = t.join("main");
        let worktree = t.join("side");
        git_repo_with_one_commit(&main);
        git(
            &main,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        );

        let h = Git::new(&worktree)
            .expect("the worktree is a working tree")
            .where_now();

        assert_eq!(h.branch, None);
        assert!(h.sha.as_deref().is_some_and(is_sha));
    }

    #[test]
    fn a_submodule_is_not_a_worktree_and_answers_for_itself() {
        // A submodule's `.git` is a file too, but it carries no `commondir`:
        // it owns its repository, so its references are its own.
        let t = tmp("submodule");
        let sub_gitdir = t.join("modules").join("sub");
        std::fs::create_dir_all(sub_gitdir.join("refs").join("heads")).unwrap();
        let sha = "c".repeat(40);
        std::fs::write(
            sub_gitdir.join("refs").join("heads").join("feature"),
            format!("{sha}\n"),
        )
        .unwrap();
        std::fs::write(sub_gitdir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        let sub_dir = t.join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(
            sub_dir.join(".git"),
            format!("gitdir: {}\n", sub_gitdir.display()),
        )
        .unwrap();

        let h = Git::new(&sub_dir)
            .expect("the submodule is its own working tree")
            .where_now();

        assert_eq!(h.branch.as_deref(), Some("feature"));
        assert_eq!(h.sha.as_deref(), Some(sha.as_str()));
    }

    #[test]
    fn a_repository_that_is_gone_is_missing_not_unreadable() {
        let t = tmp("gone");
        assert_eq!(where_of(&t.join("nothing-here")), Where::Missing);
    }

    #[test]
    fn an_unreadable_head_is_a_head_that_knows_neither_branch_nor_sha() {
        // The folder is a working tree, so the lane's repository is there;
        // what failed is reading it. `Missing` would say the folder is gone,
        // which is a different and worse claim.
        let t = tmp("unreadable-head");
        std::fs::create_dir_all(t.join(".git")).unwrap();

        assert_eq!(
            where_of(&t),
            Where::Head(Head {
                branch: None,
                sha: None,
                rebasing: false,
            })
        );
    }

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
