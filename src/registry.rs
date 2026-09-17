//! The project registry: `<store_dir>/projects`.
//!
//! One file, one job: remember which projects exist on this machine and
//! where, so a later fan-out (`d232`) does not have to be told by hand. It
//! is not verified data and not a disposable projection either -- `f267` --
//! and past `path` it also holds what using a project has turned up since:
//! the root commit of every repository its lanes declare (`repos`), the
//! folder each lane is (`lanes`), and any other folder seen holding a tree
//! that starts with the same first event (`copies`). What is unique to this
//! file is not what those four say -- a repository's root commit is also in
//! the log, in `event::Repo::root` -- it is *where* each one is: `path`,
//! every lane's folder, and every copy's folder are written down nowhere
//! else.
//!
//! Keyed by the id of each project's first event (`d201`), not by
//! `Config::project_id`: `Store::open` silently regenerates a missing
//! `config`, which would mint a fresh id for a project that already has one.
//!
//! Written as a side effect of using a project, never as its own command,
//! and never allowed to turn a working command into a failing one: see
//! `note`.

use crate::failure::Failure;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE: &str = "projects";

/// Whether this `.vivac/` is the global store rather than a project's.
/// The registry is the mark: nothing else writes that file, and asking
/// what a directory holds keeps working after `VIVAC_HOME` moves it,
/// which comparing paths would not.
pub fn marks_global_store(dir: &Path) -> bool {
    dir.join(FILE).is_file()
}

const VERSION: u32 = 2;

/// What the registry knows about one project.
///
/// `path` is where it lives. `repos` holds the root commit of every
/// repository the tree's lanes declare, so `setup` can tell that a folder
/// it has never seen holds a product that is already mapped. `lanes` maps
/// each lane id to the folder it is -- the only place a lane's path is
/// ever written down, since `.vivac/lane` deliberately holds none. `copies`
/// holds every other folder seen holding a tree that starts with this same
/// first event (`d201`): `path` never moves off whichever folder used a
/// `vivac` command first while the registry already knew this project, and
/// every other one is recorded here instead, so it can be told about too.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Project {
    path: String,
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    lanes: BTreeMap<String, String>,
    #[serde(default)]
    copies: Vec<String>,
}

/// Version 1 wrote a project as a bare path string. It is read and never
/// written: the next write that has something to say replaces the whole
/// file with version 2, so there is no migration step anywhere.
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    V1(String),
    V2(Project),
}

#[derive(Deserialize)]
struct OnDisk {
    version: u32,
    projects: BTreeMap<String, Entry>,
}

#[derive(Serialize)]
struct ToDisk<'a> {
    version: u32,
    projects: &'a BTreeMap<String, Project>,
}

/// What the caller knows about the project it just used.
pub struct Sighting<'a> {
    /// The folder the tree lives in.
    pub root: &'a Path,
    /// The lane the command ran in, and its folder. `None` for a folder
    /// that has not joined yet: the next command in it will say so.
    pub lane: Option<(&'a str, &'a Path)>,
    /// The root commits of every repository this tree's lanes declare.
    /// `None` means "leave what is on file": only the commands that fold
    /// the tree before noting it -- `setup` and `relocate` -- know this.
    pub repos: Option<&'a [String]>,
}

/// What using a project turned out to say about it.
pub enum Noted {
    /// Nothing worth telling anybody.
    Fine,
    /// One or more other folders hold a tree that starts with the same
    /// event, so each of them -- together with this one and whichever
    /// folder is asking -- is a copy of the rest (`d201`). Which folder is
    /// "the" original is **not** what this answers. The registry has room
    /// for one `path` per project, so whichever folder used a `vivac`
    /// command first while the registry already knew this project keeps
    /// that slot, and `path` never moves off it; every other folder
    /// sighted since is recorded in `copies` instead. That first folder
    /// can be the original or a copy in reality -- nothing here knows
    /// which one came first, only which one reached the registry first --
    /// so every folder eventually learns the same membership, itself
    /// excluded: `live_others` is the one function both `note` and
    /// `copy_of` build this from, so a folder never learns a different
    /// set of siblings depending on which of them is asking.
    ///
    /// `first` and `rest` rather than one list: a copy with nobody to name
    /// is not representable, so `copy_notice` never has to guard against
    /// an empty one that only a comment promised could not happen.
    ///
    /// Each folder is named, never its path, and a name is withheld when
    /// the redaction guard rejects it (`d600`) -- this text reaches the
    /// agent's context. Order matches `copies`: insertion order, never
    /// reshuffled.
    Copy {
        first: Option<String>,
        rest: Vec<Option<String>>,
    },
}

/// Records what `s` says about the project keyed by `project_id`.
///
/// Steady state is two small reads and no write: an absent key is inserted,
/// a key that already says exactly this writes nothing, and a key that
/// says something else -- a different path, a new lane, an addition to
/// `repos` -- is updated in place. A copy (`Noted::Copy`) is the one case
/// that never moves `path`: the registry already points elsewhere, and
/// that elsewhere still holds a tree with `project_id`'s own first event,
/// so `s.root` is recorded into `copies` instead, the first time it is
/// seen -- steady state for a copy already known is a no-write read too.
///
/// Never fails. The registry serves a surface that does not exist yet, so a
/// missing or unwritable `store_dir`, a `projects` file that will not
/// parse, or one written by a newer vivac than this, all leave the
/// caller's own result untouched -- and answer `Noted::Fine`, the same as
/// nothing worth telling. A file that will not parse is replaced wholesale
/// on the next successful write, not repaired.
pub fn note(store_dir: &Path, project_id: &str, s: Sighting<'_>) -> Noted {
    try_note(store_dir, project_id, &s).unwrap_or(Noted::Fine)
}

/// Points the registry at `s.root` outright, for a caller that already
/// knows this is a move and has nothing to infer.
///
/// `note`'s own `decide` asks whether `path` still shows a live tree to
/// tell a move from a copy, because an ordinary command never knows which
/// one it is looking at -- it only has a folder and a sighting. `relocate`
/// is not that caller: it just renamed the origin's own `events` out of
/// the way itself, so calling `note` and hoping `path_disagrees` reads the
/// silence correctly is asking one function to re-derive a fact its
/// caller already holds. This skips the guess and writes `s.root` in
/// directly, the way `apply_sighting` always has for an ordinary sighting.
///
/// **Fails the caller.** `note` never does, because for an ordinary write
/// the registry is a comfort a command can do without. `relocate` cannot
/// afford that: once the origin's own log is gone, the registry is the
/// only durable record of where the tree went, and a caller that cannot
/// tell this failed has no way to roll back and warn instead of leaving a
/// tree nothing durable points at.
pub fn record_move(store_dir: &Path, project_id: &str, s: Sighting<'_>) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir)?;
    let path = store_dir.join(FILE);
    let _lock = crate::store::lock_with_deadline(&store_dir.join(LOCK), LOCK_WAIT)
        .map_err(|e| std::io::Error::other(e.message()))?;
    let Some(mut projects) = read(&path) else {
        return Err(std::io::Error::other(
            "the registry was written by a newer vivac and cannot be updated",
        ));
    };
    apply_sighting(&mut projects, project_id, &s);
    write(store_dir, &path, &projects)
}

/// Whether another folder on this machine still holds a tree that starts
/// with this same event. Read-only: unlike `note`, it never writes, so a
/// reading command can ask without the registry moving under it -- and,
/// because it never writes, it can never lean on a write to have already
/// cleaned anything up: `live_others` does its own filtering, every time,
/// including filtering `root` itself out.
///
/// No folder is favored: `path` and every entry in `copies` are all
/// candidates alike, so whichever folder is asking sees every other live
/// one, itself excluded -- the folder on `path` and the folder not on it
/// go through the very same call.
pub fn copy_of(store_dir: &Path, project_id: &str, root: &Path) -> Noted {
    let Some(projects) = read(&store_dir.join(FILE)) else {
        return Noted::Fine;
    };
    let Some(existing) = projects.get(project_id) else {
        return Noted::Fine;
    };
    copy_or_fine(live_others(existing, project_id, root))
}

/// What every surface that reports one or more copies prints: the heading
/// and the body, chosen together in the very same call so the two can
/// never disagree about how many folders there are. `check` used to read
/// a standalone `COPY_HEADING` and this function's body separately --
/// harmless while there was only ever one shape, and exactly the kind of
/// split that a second shape (this one; `t594` §4.7, fix round 2) would
/// desync the moment only one of the two remembered to change.
///
/// Kept in the one module that already owns what a copy is (`Noted::Copy`,
/// `live_others`) rather than in whichever surface prints it first: `check`
/// today, and the brief and the per-write stderr notice that `t594` §4.7
/// still owes. A security-relevant sentence copied into more than one call
/// site only agrees with itself until somebody edits one of them, which is
/// exactly what happened elsewhere in this work two days before it was
/// written the first time.
///
/// Each surface lays the result out to its own shape -- `check` indents its
/// blocks differently from the brief -- so only the heading and the words
/// are shared; indentation is the caller's.
pub struct CopyNotice {
    pub heading: &'static str,
    pub body: String,
}

/// The width every paragraph below wraps to, before the command line that
/// follows it. A name list has as many names as there are copies, which
/// is not bounded, so splitting it into lines by hand is wrong by
/// construction and not by oversight; `render::wrap` is the same
/// word-wrap `why` and `changes` already read through.
const NOTICE_WIDTH: usize = 76;

/// Wraps `prose`, then puts `command` on a line of its own after it, one
/// indent deeper -- whole, never wrapped, since a command line broken
/// across two lines is not one anybody can paste.
fn wrapped_with_command(prose: &str, command: &str) -> String {
    let mut lines = crate::render::wrap(prose, NOTICE_WIDTH, "");
    lines.push(format!("  {command}"));
    lines.join("\n")
}

/// `first` is the one other folder `Noted::Copy` is guaranteed to carry;
/// `rest` is every one after it, empty for the ordinary case of exactly
/// one copy.
pub fn copy_notice(first: Option<&str>, rest: &[Option<String>]) -> CopyNotice {
    if rest.is_empty() {
        return match first {
            Some(name) => CopyNotice {
                heading: "COPY OF ANOTHER TREE",
                body: wrapped_with_command(
                    &format!(
                        "This tree starts with the same event as the one in folder \"{name}\", \
                         so one of them is a copy, and copies diverge in silence. Keep one: \
                         delete the other, or delete this one and join this folder to it with"
                    ),
                    &format!("vivac setup claude-code --join {}", quote_if_needed(name)),
                ),
            },
            // The command used to sit inline at the end of this
            // paragraph rather than on its own line like its siblings --
            // a mistake in the original prose, not a deliberate
            // difference: form 1 is the same sentence with the name
            // filled in, and its own command already stood on its own
            // line. Fixed here so all five forms wrap the same way.
            None => CopyNotice {
                heading: "COPY OF ANOTHER TREE",
                body: wrapped_with_command(
                    "This tree starts with the same event as one in another folder on this \
                     machine, so one of them is a copy, and copies diverge in silence. Keep \
                     one: delete the other, or delete this one and join this folder to it \
                     with",
                    "vivac setup claude-code --join <path to that folder>",
                ),
            },
        };
    }
    let total = 1 + rest.len();
    let named: Vec<&str> = first
        .into_iter()
        .chain(rest.iter().filter_map(|o| o.as_deref()))
        .collect();
    let names = named
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let command = "vivac setup claude-code --join <the folder you kept>";
    let body = if named.len() == total {
        wrapped_with_command(
            &format!(
                "These folders on this machine start with the same event as this one: \
                 {names}. They are copies of each other, and copies diverge in silence. \
                 Keep one, delete the rest, and join the folders you still work in to the \
                 one you kept:"
            ),
            command,
        )
    } else if named.is_empty() {
        wrapped_with_command(
            "Other folders on this machine hold a tree that starts with the same event as \
             this one, under names this tool will not write down. They are copies of each \
             other, and copies diverge in silence. Keep one, delete the rest, and join the \
             folders you still work in to the one you kept:",
            command,
        )
    } else {
        wrapped_with_command(
            &format!(
                "These folders on this machine start with the same event as this one: \
                 {names}. More hold it too, under names this tool will not write down. \
                 They are copies of each other, and copies diverge in silence. Keep one, \
                 delete the rest, and join the folders you still work in to the one you \
                 kept:"
            ),
            command,
        )
    };
    CopyNotice {
        heading: "COPIES OF THIS TREE",
        body,
    }
}

/// Whether `name` is safe to paste into a shell unquoted: only letters,
/// digits, `-`, `_` and `.`. The short list is the safe one and the long
/// list is the dangerous one, so this names what is allowed rather than
/// what is not -- a folder can be called almost anything, and guessing
/// which of the rest a given shell treats specially is how `A&B` used to
/// get through unquoted and split in two.
fn shell_safe(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `pub(crate)`, not private: `setup` (`t594` §4.5) quotes a project name
/// the same way when it prints `--join <name>`, the same command this
/// file's own `copy_notice` already prints -- one rule for what a shell
/// needs quoted, not two that could drift.
pub(crate) fn quote_if_needed(name: &str) -> String {
    if shell_safe(name) {
        name.to_string()
    } else {
        format!("\"{name}\"")
    }
}

/// The registry's own lock, in the global store. It is **not** any tree's
/// lock: two different projects can be planted at the same time, and the
/// file they both write is this one. Taken only when there is something to
/// write -- the steady state is two small reads and no write, and that is
/// what keeps `note` off the write budget (`f603`).
const LOCK: &str = "registry.lock";

/// Nobody holds this lock longer than a rename takes, so a second is
/// already generous for either of this file's two writers, not just the
/// one that can afford to give up quietly. `note` can never fail its
/// caller, so for it this is about not hanging a command that already has
/// its own answer. `record_move` is the other one, and it cannot make
/// that same trade: giving up here aborts a move already under way, out
/// loud rather than in silence, which is the right failure for a lock
/// that should only ever be held for a handoff measured in microseconds
/// -- a full second stuck on it already means something else is wrong,
/// not merely running.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// What a read of the registry says to do next.
enum Decision {
    /// Nothing to write; this is the whole answer.
    Done(Noted),
    /// A copy not recorded yet: add `s.root` to the entry's `copies`,
    /// `path` untouched. Carries the `Noted::Copy` already worked out
    /// from `live_others`, so the write side never has to recompute it.
    RecordCopy(Noted),
    /// An ordinary sighting with something new to say: fold `s` into the
    /// entry the way `apply_sighting` always has.
    RecordSighting,
}

/// One read, decided: whether there is nothing to do, a copy to add to
/// `copies`, or an ordinary field to fold in. Called once outside the
/// lock, to decide whether there is anything worth taking it for, and
/// once again under it, on a fresh read, since another writer may have
/// landed between the two.
fn decide(projects: &BTreeMap<String, Project>, project_id: &str, s: &Sighting<'_>) -> Decision {
    if let Some(existing) = projects.get(project_id) {
        if path_disagrees(existing, project_id, s.root) {
            let copy = copy_or_fine(live_others(existing, project_id, s.root));
            let known = existing
                .copies
                .iter()
                .any(|c| crate::anchor::same_folder(Path::new(c), s.root));
            return if known {
                Decision::Done(copy)
            } else {
                Decision::RecordCopy(copy)
            };
        }
    }
    if unchanged(projects, project_id, s) {
        return Decision::Done(Noted::Fine);
    }
    Decision::RecordSighting
}

fn try_note(store_dir: &Path, project_id: &str, s: &Sighting<'_>) -> std::io::Result<Noted> {
    let path = store_dir.join(FILE);
    let Some(projects) = read(&path) else {
        return Ok(Noted::Fine);
    };
    if let Decision::Done(outcome) = decide(&projects, project_id, s) {
        return Ok(outcome);
    }
    std::fs::create_dir_all(store_dir)?;
    let _lock = crate::store::lock_with_deadline(&store_dir.join(LOCK), LOCK_WAIT)
        .map_err(|e| std::io::Error::other(e.message()))?;
    // Read again under the lock: the value that decided there was work to
    // do was read outside it, and another writer may have landed since.
    let Some(mut projects) = read(&path) else {
        return Ok(Noted::Fine);
    };
    match decide(&projects, project_id, s) {
        Decision::Done(outcome) => Ok(outcome),
        Decision::RecordCopy(outcome) => {
            let entry = projects.entry(project_id.to_string()).or_default();
            entry.copies.push(s.root.to_string_lossy().into_owned());
            prune_dead_copies(entry, project_id);
            write(store_dir, &path, &projects)?;
            Ok(outcome)
        }
        Decision::RecordSighting => {
            apply_sighting(&mut projects, project_id, s);
            write(store_dir, &path, &projects)?;
            Ok(Noted::Fine)
        }
    }
}

/// Whether `root` disagrees with what is on `path`, and `path`'s own tree
/// is still there to prove it: the boolean `decide` needs to tell a copy
/// sighting from an ordinary one. That is not a move: both exist, so one
/// is a copy of the other. Short-circuits before ever reading `path`'s own
/// log when `root` already agrees with it -- the ordinary case, on every
/// command -- so the common path costs nothing extra here.
///
/// `same_folder`, not a raw comparison: `path` and `root` are two
/// spellings of a folder built by genuinely independent means -- one read
/// back from a previous sighting, the other from the `cd` in force right
/// now -- and a case difference or an alias between them is `f612`, the
/// same class `ops::repo_at` already had to answer for once.
fn path_disagrees(existing: &Project, project_id: &str, root: &Path) -> bool {
    let other = Path::new(&existing.path);
    !crate::anchor::same_folder(other, root)
        && crate::store::first_event_id(other).as_deref() == Some(project_id)
}

/// Every folder this registry knows might hold `project_id`'s tree, other
/// than `root` itself, verified alive right now: `path` is one candidate
/// and every entry in `copies` is another, on equal footing. A dead one --
/// a folder that no longer has a tree to show for this event, `root`
/// itself included when a write that would have said so never landed --
/// is silently dropped rather than reported. `note` (a fresh copy, or one
/// already known) and `copy_of` (`path` itself, or any other folder,
/// asked either way) both build `Noted::Copy` from this one list, so no
/// folder ever learns a different set of siblings than any other.
fn live_others(existing: &Project, project_id: &str, root: &Path) -> Vec<Option<String>> {
    std::iter::once(existing.path.as_str())
        .chain(existing.copies.iter().map(String::as_str))
        .map(Path::new)
        .filter(|p| !crate::anchor::same_folder(p, root))
        .filter(|p| crate::store::first_event_id(p).as_deref() == Some(project_id))
        .map(folder_name)
        .collect()
}

/// `Noted::Fine` for an empty list, `Noted::Copy` for anything else --
/// the one place that split happens, so `Noted::Copy` itself never has to
/// represent "a copy with nobody in it".
fn copy_or_fine(mut others: Vec<Option<String>>) -> Noted {
    if others.is_empty() {
        Noted::Fine
    } else {
        let first = others.remove(0);
        Noted::Copy {
            first,
            rest: others,
        }
    }
}

/// The folder's own name, or nothing when the redaction guard rejects it.
/// Never the path: where a copy sits is this machine's business, and this
/// name travels into an agent's context.
pub fn folder_name(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    match crate::redact::check_field("project name", &name) {
        Some(_) => None,
        None => Some(name),
    }
}

/// Whether `s` says nothing that is not already on file for `project_id`:
/// the no-write path that keeps `note` off the write budget (`f603`).
/// `s.repos` and `s.lane` read as "leave what is on file" when absent, so
/// neither counts against an entry that has nothing to compare them to.
///
/// `path` and a lane's folder are both compared with `same_folder`, never
/// as raw strings: entering the very folder this project's own entry
/// already names, under a second spelling, must read as unchanged, not as
/// a new value to write -- `f612` a third time is exactly the mistake this
/// function existed to avoid catching.
fn unchanged(projects: &BTreeMap<String, Project>, project_id: &str, s: &Sighting<'_>) -> bool {
    let Some(p) = projects.get(project_id) else {
        return false;
    };
    if !crate::anchor::same_folder(Path::new(&p.path), s.root) {
        return false;
    }
    if let Some(repos) = s.repos {
        if p.repos.as_slice() != repos {
            return false;
        }
    }
    if let Some((id, dir)) = s.lane {
        let same = p
            .lanes
            .get(id)
            .is_some_and(|existing| crate::anchor::same_folder(Path::new(existing), dir));
        if !same {
            return false;
        }
    }
    true
}

/// Drops every `copies` entry whose folder no longer holds a tree with
/// `project_id`'s own first event, the moment this project has anything
/// else to write anyway. Only ever called from a write already in
/// progress: `copy_of` never prunes, since a read must never move the
/// registry out from under whoever else might be reading it at the same
/// time -- a dead entry still answers correctly there (`first_event_id`
/// is re-checked on every read), it just waits for a real reason to leave
/// the file.
fn prune_dead_copies(entry: &mut Project, project_id: &str) {
    entry
        .copies
        .retain(|c| crate::store::first_event_id(Path::new(c)).as_deref() == Some(project_id));
}

/// Folds `s` into `projects`, minting the entry when `project_id` is new.
/// `s.repos` and `s.lane` only ever add: `None` leaves what is already
/// there.
///
/// `copies` is pruned twice here, for two different reasons: dead entries
/// go first (`prune_dead_copies` -- a copy that gets deleted stops being
/// listed the next time there is anything to write, not only the next
/// time somebody reads), and only then does an entry that now names
/// `path` itself get dropped, with `same_folder` rather than a raw
/// comparison -- the folder `copies` recorded a sighting of can later
/// become `path` in its own right (its old owner's tree gone, `s.root`
/// unchanged from what that entry already said), and a copy of yourself
/// is not a copy of anything.
fn apply_sighting(projects: &mut BTreeMap<String, Project>, project_id: &str, s: &Sighting<'_>) {
    let entry = projects.entry(project_id.to_string()).or_default();
    entry.path = s.root.to_string_lossy().into_owned();
    prune_dead_copies(entry, project_id);
    entry
        .copies
        .retain(|c| !crate::anchor::same_folder(Path::new(c), s.root));
    if let Some(repos) = s.repos {
        entry.repos = repos.to_vec();
    }
    if let Some((id, dir)) = s.lane {
        entry
            .lanes
            .insert(id.to_string(), dir.to_string_lossy().into_owned());
    }
}

/// Where the tree keyed by `project_id` lives, as the registry last heard.
/// A lane names its tree by that key and by nothing else, so this is the
/// lookup a working folder that does not hold the tree depends on.
pub fn root_of(store_dir: &Path, project_id: &str) -> Option<PathBuf> {
    read(&store_dir.join(FILE))
        .unwrap_or_default()
        .get(project_id)
        .map(|p| PathBuf::from(&p.path))
}

/// Every root the registry currently points at, in no particular order.
/// `find --everywhere` (`d273`) is the first reader that wants the roots
/// themselves rather than the id each one is keyed by, so the map's keys
/// stay inside this module the way `note`'s already do.
pub fn roots(store_dir: &Path) -> Vec<PathBuf> {
    read(&store_dir.join(FILE))
        .unwrap_or_default()
        .into_values()
        .map(|p| PathBuf::from(p.path))
        .filter(|r| !marks_global_store(&r.join(crate::store::DIR)))
        .collect()
}

/// A project on this machine that shares at least one repository with the
/// folder being set up. Named by its folder, never by its path: this text
/// reaches an agent's context, and `d600` withholds a name the redaction
/// guard rejects.
pub struct Sharing {
    pub name: Option<String>,
    pub root: PathBuf,
    /// The root commits both hold, so the caller can name its own copies
    /// of them by the folder names it already has.
    pub shared: Vec<String>,
}

/// Every project the registry knows that shares at least one of `repos` --
/// root commits -- with the folder being set up: `t594` §4.5, the check
/// that tells a folder `setup` has never seen apart from one that still
/// holds a product already mapped. A single shared repository is enough,
/// the same reason a fresh root that adds one more repository to a product
/// is still that product.
///
/// Most shared repositories first, ties broken by name -- a withheld name
/// sorts after every real one, since there is nothing to compare it
/// against.
pub fn sharing_repos(store_dir: &Path, repos: &[String]) -> Vec<Sharing> {
    let Some(projects) = read(&store_dir.join(FILE)) else {
        return Vec::new();
    };
    let mut found: Vec<Sharing> = projects
        .values()
        .filter_map(|p| {
            let shared: Vec<String> = p
                .repos
                .iter()
                .filter(|r| repos.contains(r))
                .cloned()
                .collect();
            if shared.is_empty() {
                return None;
            }
            let root = PathBuf::from(&p.path);
            Some(Sharing {
                name: folder_name(&root),
                root,
                shared,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.shared.len().cmp(&a.shared.len()).then_with(|| {
            a.name
                .as_deref()
                .unwrap_or("\u{10FFFF}")
                .cmp(b.name.as_deref().unwrap_or("\u{10FFFF}"))
        })
    });
    found
}

/// Resolves `--project`'s value against the registry: a bare name -- the
/// directory's own base name, exactly what `find --everywhere` prints --
/// tried first, and a path otherwise. `d273`'s second half: a hit
/// `find --everywhere` returns names its project this way, and this is what
/// lets `why` open it.
///
/// Two roots can carry the same base name, and choosing between them would
/// answer a question about the wrong tree while looking right, so a name
/// that matches more than one root refuses instead of guessing. The message
/// never names the candidates by path -- the security pillar allows nothing
/// but a project's name across this boundary, and two candidates sharing a
/// name have no path-free way to tell apart, so the count is what it names.
/// A name that matches no root falls through to being read as a path;
/// `Store::open` is what answers whether that path holds a project at all.
pub fn resolve(spec: &str) -> Result<PathBuf, Failure> {
    let known = crate::store::store_dir()
        .map(|d| roots(&d))
        .unwrap_or_default();
    let mut matches: Vec<PathBuf> = known
        .into_iter()
        .filter(|root| crate::render::project_name(root) == spec)
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Ok(PathBuf::from(spec)),
        n => Err(Failure::usage(format!(
            "\"{spec}\" names {n} projects on this machine. Pass a path instead."
        ))),
    }
}

/// Reads the registry. `None` when the file names a version newer than
/// `VERSION`: a registry written by a newer vivac is not understood, and
/// -- the same silence `note` already answers a missed lock with -- is
/// never overwritten. Anything else that will not parse, version 1's own
/// bare strings included, reads as the projects it can make out; garbage
/// reads as no projects at all rather than `None`, and gets replaced whole
/// on the next write that has something to say, same as it always has.
fn read(path: &Path) -> Option<BTreeMap<String, Project>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Some(BTreeMap::new());
    };
    let on_disk: OnDisk = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Some(BTreeMap::new()),
    };
    if on_disk.version > VERSION {
        return None;
    }
    Some(
        on_disk
            .projects
            .into_iter()
            .map(|(id, entry)| {
                let project = match entry {
                    Entry::V1(path) => Project {
                        path,
                        ..Project::default()
                    },
                    Entry::V2(p) => p,
                };
                (id, project)
            })
            .collect(),
    )
}

fn write(
    store_dir: &Path,
    path: &Path,
    projects: &BTreeMap<String, Project>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir)?;
    let tmp = store_dir.join(format!("{FILE}.{}.tmp", crate::id::ulid()));
    let payload = ToDisk {
        version: VERSION,
        projects,
    };
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&payload)?.as_bytes())?;
        f.write_all(b"\n")?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{id, store};

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vivac-{prefix}-{}", id::ulid()))
    }

    /// A `Sighting` with nothing but a root, the shape every test that does
    /// not care about lanes or repos wants.
    fn sighting(root: &Path) -> Sighting<'_> {
        Sighting {
            root,
            lane: None,
            repos: None,
        }
    }

    /// Plants a fresh tree at `root` and gives it one event, so it has a
    /// first event id to be keyed by.
    fn seed_at(root: &Path) -> String {
        let mut s = store::Store::create(root).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.append(
            &lock,
            crate::lane::MAIN,
            vec![crate::event::Body::NodeNoted {
                node: "t1".into(),
                note: "seed".into(),
            }],
            0,
            false,
        )
        .unwrap();
        store::first_event_id(root).unwrap()
    }

    /// A seeded project under a fresh temp folder.
    fn seeded_project(prefix: &str) -> (std::path::PathBuf, String) {
        let root = temp_dir(prefix);
        let id = seed_at(&root);
        (root, id)
    }

    #[test]
    fn a_fresh_registry_gets_the_project_inserted() {
        let store_dir = temp_dir("reg");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, sighting(&root));

        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(
            projects.get(&id).map(|p| p.path.as_str()),
            Some(root.to_string_lossy().as_ref())
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_different_projects_both_appear() {
        let store_dir = temp_dir("reg");
        let (root_a, id_a) = seeded_project("proj-a");
        let (root_b, id_b) = seeded_project("proj-b");

        note(&store_dir, &id_a, sighting(&root_a));
        note(&store_dir, &id_b, sighting(&root_b));

        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects.get(&id_a).unwrap().path, root_a.to_string_lossy());
        assert_eq!(projects.get(&id_b).unwrap().path, root_b.to_string_lossy());

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }

    #[test]
    fn an_unwritable_store_directory_never_fails_the_caller() {
        // A file where a directory is expected: `create_dir_all` cannot make
        // a directory out of it, and `note` still has to return nothing to
        // panic on.
        let blocked = temp_dir("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let (root, id) = seeded_project("proj");

        note(&blocked, &id, sighting(&root));

        std::fs::remove_file(&blocked).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    /// The one thing `note` cannot do and `record_move` exists for: `path`
    /// moves even while the folder already on file still shows a live
    /// tree. `note`'s own `decide` would read that as a copy and leave
    /// `path` exactly where it was -- `a_second_folder_with_the_same_first_event_is_a_copy`,
    /// above, is that behaviour, pinned on purpose. `record_move` never
    /// asks the question.
    #[test]
    fn record_move_points_path_at_the_destination_even_though_the_origin_still_shows_a_tree() {
        let store_dir = temp_dir("reg-move");
        let (origin, id) = seeded_project("move-origin");
        let destination = temp_dir("move-destination");
        note(&store_dir, &id, sighting(&origin));

        record_move(&store_dir, &id, sighting(&destination)).unwrap();

        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(
            projects.get(&id).unwrap().path,
            destination.to_string_lossy(),
            "record_move must not read a live origin as reason to call this a copy"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&origin).ok();
    }

    /// `note`'s whole contract is that it never fails its caller. This is
    /// the opposite contract, on purpose: `relocate` has nothing durable
    /// left to point at the tree once its own log is gone, so a caller
    /// that cannot tell this write failed has no way to roll back.
    #[test]
    fn record_move_fails_the_caller_when_the_store_directory_cannot_be_written() {
        let blocked = temp_dir("move-blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let (root, id) = seeded_project("move-blocked-proj");

        let result = record_move(&blocked, &id, sighting(&root));

        assert!(
            result.is_err(),
            "a blocked store directory must fail record_move"
        );

        std::fs::remove_file(&blocked).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_nonexistent_store_directory_never_fails_the_caller() {
        let store_dir = temp_dir("does-not-exist-yet");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, sighting(&root));
        assert!(store_dir.join(FILE).exists());

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn garbage_in_projects_never_fails_the_caller_and_gets_replaced() {
        let store_dir = temp_dir("reg");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join(FILE), b"not json at all").unwrap();
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, sighting(&root));

        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(projects.get(&id).unwrap().path, root.to_string_lossy());

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn root_of_answers_a_noted_project_and_none_for_an_unknown_key() {
        let store_dir = temp_dir("reg");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, sighting(&root));

        assert_eq!(root_of(&store_dir, &id), Some(root.clone()));
        assert_eq!(root_of(&store_dir, "01nosuchprojectaaaaaaaaaaa"), None);

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_writers_do_not_lose_a_project() {
        let store_dir = temp_dir("reg-race");
        std::fs::create_dir_all(&store_dir).unwrap();
        let projects: Vec<(std::path::PathBuf, String)> = (0..8)
            .map(|i| seeded_project(&format!("race-{i}")))
            .collect();
        std::thread::scope(|s| {
            for (root, id) in &projects {
                let dir = store_dir.clone();
                s.spawn(move || note(&dir, id, sighting(root)));
            }
        });
        let noted = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(
            noted.len(),
            8,
            "a concurrent write dropped a project the registry already had"
        );
        std::fs::remove_dir_all(&store_dir).ok();
        for (root, _) in &projects {
            std::fs::remove_dir_all(root).ok();
        }
    }

    #[test]
    fn the_registry_is_never_left_half_written() {
        // The torn read `f603` names: a reader that opens the file between
        // truncate and write parses an empty registry and answers that the
        // machine knows no projects at all. With a rename there is no such
        // window: the file is either the old one or the new one.
        let store_dir = temp_dir("reg-torn");
        let (root, id) = seeded_project("torn");
        note(&store_dir, &id, sighting(&root));
        let before = std::fs::read(store_dir.join(FILE)).unwrap();
        let (root2, id2) = seeded_project("torn2");
        let reader = {
            let dir = store_dir.clone();
            std::thread::spawn(move || {
                let mut empty = 0;
                for _ in 0..2_000 {
                    if let Ok(t) = std::fs::read_to_string(dir.join(FILE)) {
                        if serde_json::from_str::<OnDisk>(&t)
                            .map(|c| c.projects.is_empty())
                            .unwrap_or(true)
                        {
                            empty += 1;
                        }
                    }
                }
                empty
            })
        };
        // Alternating the lane keeps every one of these 200 rounds a real
        // write -- copy detection (`t594` §4.7) would otherwise turn a
        // second root noted for the same project into a no-op, and the
        // reader above would never see a rename at all. The lane, not
        // `repos`: every command `main.rs` runs passes one, and `repos`
        // has no production caller yet, so alternating that would stress
        // a write nothing real makes.
        let lane_a_dir = temp_dir("torn-lane-a");
        let lane_b_dir = temp_dir("torn-lane-b");
        for i in 0..200 {
            // Same lane id every round, its folder flipped back and forth:
            // `lanes` only ever grows, so alternating the id instead would
            // stop writing the moment both ids had been seen once each.
            let dir = if i % 2 == 0 { &lane_a_dir } else { &lane_b_dir };
            note(
                &store_dir,
                &id2,
                Sighting {
                    root: &root2,
                    lane: Some(("lane-torn", dir)),
                    repos: None,
                },
            );
        }
        assert_eq!(reader.join().unwrap(), 0, "a reader saw an empty registry");
        assert!(!before.is_empty());
        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&root2).ok();
    }

    #[test]
    fn a_version_1_file_is_read_and_rewritten_as_version_2() {
        let store_dir = temp_dir("reg-v1");
        std::fs::create_dir_all(&store_dir).unwrap();
        let id_a = "01aaaaaaaaaaaaaaaaaaaaaaaa";
        let id_b = "01bbbbbbbbbbbbbbbbbbbbbbbb";
        std::fs::write(
            store_dir.join(FILE),
            format!(r#"{{"version":1,"projects":{{"{id_a}":"/old/a","{id_b}":"/old/b"}}}}"#),
        )
        .unwrap();
        let new_root = std::path::PathBuf::from("/new/a");

        note(&store_dir, id_a, sighting(&new_root));

        let text = std::fs::read_to_string(store_dir.join(FILE)).unwrap();
        let on_disk: OnDisk = serde_json::from_str(&text).unwrap();
        assert_eq!(on_disk.version, VERSION);
        assert_eq!(on_disk.projects.len(), 2);
        match &on_disk.projects[id_a] {
            Entry::V2(p) => assert_eq!(p.path, new_root.to_string_lossy()),
            Entry::V1(_) => panic!("id_a is still a bare string after the rewrite"),
        }
        match &on_disk.projects[id_b] {
            Entry::V2(p) => assert_eq!(p.path, "/old/b"),
            Entry::V1(_) => panic!("id_b is still a bare string after the rewrite"),
        }

        std::fs::remove_dir_all(&store_dir).ok();
    }

    #[test]
    fn nothing_changed_writes_nothing() {
        let store_dir = temp_dir("reg-nothing");
        let (root, id) = seeded_project("nothing");

        note(&store_dir, &id, sighting(&root));
        let before = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();
        note(&store_dir, &id, sighting(&root));
        let after = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();

        assert_eq!(
            before, after,
            "the no-write case is the one that protects the budget"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn copy_of_never_writes() {
        let store_dir = temp_dir("reg-copy-of");
        let (root, id) = seeded_project("copy-of");
        note(&store_dir, &id, sighting(&root));
        let before = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();

        let outcome = copy_of(&store_dir, &id, &root);

        let after = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();
        assert!(matches!(outcome, Noted::Fine));
        assert_eq!(before, after, "copy_of must never write to the registry");

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    /// The write that would ordinarily clear a stale self-reference --
    /// `apply_sighting` moving `path` onto this very folder, dropping it
    /// from `copies` -- never has to happen for `copy_of` to answer
    /// correctly: it promises to keep working when the registry cannot be
    /// written at all, and a read must never lean on a write it cannot
    /// see has happened.
    #[test]
    fn copy_of_never_names_the_folder_asking_about_itself() {
        let store_dir = temp_dir("reg-self-copy");
        let (original, id) = seeded_project("self-copy-original");
        let copy_root = temp_dir("self-copy-copy");
        std::fs::create_dir_all(copy_root.join(store::DIR)).unwrap();
        std::fs::copy(
            original.join(store::DIR).join(store::LOG),
            copy_root.join(store::DIR).join(store::LOG),
        )
        .unwrap();

        note(&store_dir, &id, sighting(&original));
        // A genuine copy, sighted once: this is what writes `copy_root`
        // into `copies` in the first place.
        note(&store_dir, &id, sighting(&copy_root));

        // The original's own tree is gone, so the write that would move
        // `path` onto `copy_root` and drop it from `copies` never
        // happens -- `copy_of` never writes, whether or not the registry
        // even could.
        std::fs::remove_dir_all(&original).unwrap();

        let outcome = copy_of(&store_dir, &id, &copy_root);
        assert!(
            matches!(outcome, Noted::Fine),
            "a stale self-reference in copies must never name the folder asking about itself"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&copy_root).ok();
    }

    #[test]
    fn a_newer_registry_is_left_alone() {
        let store_dir = temp_dir("reg-newer");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join(FILE), br#"{"version":99,"projects":{}}"#).unwrap();
        let before = std::fs::read(store_dir.join(FILE)).unwrap();
        let (root, id) = seeded_project("too-new");

        let outcome = note(&store_dir, &id, sighting(&root));

        let after = std::fs::read(store_dir.join(FILE)).unwrap();
        assert!(matches!(outcome, Noted::Fine));
        assert_eq!(
            before, after,
            "a registry from a newer vivac must not be overwritten"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_second_folder_with_the_same_first_event_is_a_copy() {
        let store_dir = temp_dir("reg-copy");
        let (first, id) = seeded_project("copy-first");
        let second = temp_dir("copy-second");
        // A second, real tree with the very same first event: not a
        // fixture, an actual copy of the first (`d201`).
        std::fs::create_dir_all(second.join(store::DIR)).unwrap();
        std::fs::copy(
            first.join(store::DIR).join(store::LOG),
            second.join(store::DIR).join(store::LOG),
        )
        .unwrap();
        let expected_name = first.file_name().unwrap().to_string_lossy().into_owned();

        note(&store_dir, &id, sighting(&first));
        let outcome = note(&store_dir, &id, sighting(&second));

        match outcome {
            Noted::Copy { first, rest } => {
                assert_eq!(first, Some(expected_name));
                assert!(rest.is_empty());
            }
            Noted::Fine => panic!("a second tree with the same first event is a copy, not a move"),
        }
        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(
            projects.get(&id).unwrap().path,
            first.to_string_lossy(),
            "the registry must keep pointing at the folder already on file"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn a_folder_that_moved_is_not_a_copy() {
        let store_dir = temp_dir("reg-moved");
        let (old_root, id) = seeded_project("moved-from");
        let new_root = temp_dir("moved-to");

        note(&store_dir, &id, sighting(&old_root));
        // The folder moved: nothing is left where it used to be.
        std::fs::remove_dir_all(&old_root).unwrap();
        let outcome = note(&store_dir, &id, sighting(&new_root));

        assert!(matches!(outcome, Noted::Fine));
        let projects = read(&store_dir.join(FILE)).unwrap();
        assert_eq!(projects.get(&id).unwrap().path, new_root.to_string_lossy());

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&new_root).ok();
    }

    #[test]
    fn a_copy_whose_folder_name_the_guard_rejects_is_not_named() {
        let rejected_name = "someone@example.com";
        assert!(
            crate::redact::check_field("project name", rejected_name).is_some(),
            "the guard must actually reject this name, or the test proves nothing"
        );

        let parent = temp_dir("reg-copy-redacted-parent");
        std::fs::create_dir_all(&parent).unwrap();
        let first = parent.join(rejected_name);
        let id = seed_at(&first);
        let second = temp_dir("reg-copy-redacted-second");
        std::fs::create_dir_all(second.join(store::DIR)).unwrap();
        std::fs::copy(
            first.join(store::DIR).join(store::LOG),
            second.join(store::DIR).join(store::LOG),
        )
        .unwrap();
        let store_dir = temp_dir("reg-copy-redacted-store");

        note(&store_dir, &id, sighting(&first));
        let outcome = note(&store_dir, &id, sighting(&second));

        assert!(
            matches!(outcome, Noted::Copy { first: None, rest } if rest.is_empty()),
            "a folder name the guard rejects must not reach the caller"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&parent).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn lanes_learn_from_whoever_runs_a_command() {
        let store_dir = temp_dir("reg-lanes");
        let (root, id) = seeded_project("lanes");
        let lane_a_dir = temp_dir("lane-a");
        let lane_b_dir = temp_dir("lane-b");

        note(
            &store_dir,
            &id,
            Sighting {
                root: &root,
                lane: Some(("lane-a", &lane_a_dir)),
                repos: None,
            },
        );
        note(
            &store_dir,
            &id,
            Sighting {
                root: &root,
                lane: Some(("lane-b", &lane_b_dir)),
                repos: None,
            },
        );

        let projects = read(&store_dir.join(FILE)).unwrap();
        let project = projects.get(&id).unwrap();
        assert_eq!(
            project.lanes.get("lane-a").map(String::as_str),
            Some(lane_a_dir.to_string_lossy().as_ref())
        );
        assert_eq!(
            project.lanes.get("lane-b").map(String::as_str),
            Some(lane_b_dir.to_string_lossy().as_ref())
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    /// A second, independent spelling of `p`'s own folder name -- every
    /// ASCII letter's case swapped -- answered rather than assumed, so a
    /// caller with nothing to swap (a name with no letters at all) skips
    /// its own test with a reason instead of silently comparing a path
    /// against itself. Windows only: case is what `f612` was actually
    /// caught by, and this crate makes no claim about case on a
    /// filesystem where it is significant.
    #[cfg(windows)]
    fn second_spelling(p: &Path) -> Option<PathBuf> {
        let name = p.file_name()?.to_str()?;
        let other: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        (other != name).then(|| p.with_file_name(other))
    }

    #[cfg(not(windows))]
    fn second_spelling(_p: &Path) -> Option<PathBuf> {
        None
    }

    /// The second harm `f612` did here and not in `ops::repo_at`: a false
    /// `Noted::Copy` returns before `try_note` ever reaches
    /// `apply_sighting`, so a sighting from the second spelling taught the
    /// registry nothing at all -- not just "no copy", the lane it carried
    /// went with it, in silence.
    #[test]
    fn a_folder_reached_by_two_spellings_still_gets_its_lane_recorded() {
        let store_dir = temp_dir("reg-spelling-lane");
        let (root, id) = seeded_project("Spelling");
        let lane_dir = temp_dir("spelling-lane-dir");

        note(
            &store_dir,
            &id,
            Sighting {
                root: &root,
                lane: Some(("lane-a", &lane_dir)),
                repos: None,
            },
        );

        let Some(second) = second_spelling(&root) else {
            eprintln!(
                "skipped: this platform offers no second spelling of the same folder to test with"
            );
            std::fs::remove_dir_all(&store_dir).ok();
            std::fs::remove_dir_all(&root).ok();
            return;
        };

        note(
            &store_dir,
            &id,
            Sighting {
                root: &second,
                lane: Some(("lane-b", &lane_dir)),
                repos: None,
            },
        );

        let projects = read(&store_dir.join(FILE)).unwrap();
        let project = projects.get(&id).unwrap();
        assert!(
            project.lanes.contains_key("lane-b"),
            "the second spelling's own sighting never reached the registry: {:?}",
            project.lanes
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    /// `unchanged`'s own comparison, not `decide`'s: the folder already on
    /// `path` re-entered under a second spelling must read as unchanged,
    /// or every differently-cased command run in it would rewrite the
    /// registry for nothing -- exactly the write budget
    /// `nothing_changed_writes_nothing` already guards, a second spelling
    /// standing in for a second, identical call.
    #[test]
    fn same_folder_by_two_spellings_writes_nothing_the_second_time() {
        let store_dir = temp_dir("reg-spelling-unchanged");
        let (root, id) = seeded_project("Spelling-Unchanged");

        note(&store_dir, &id, sighting(&root));
        let before = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();

        let Some(second) = second_spelling(&root) else {
            eprintln!(
                "skipped: this platform offers no second spelling of the same folder to test with"
            );
            std::fs::remove_dir_all(&store_dir).ok();
            std::fs::remove_dir_all(&root).ok();
            return;
        };
        note(&store_dir, &id, sighting(&second));
        let after = std::fs::metadata(store_dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();

        assert_eq!(
            before, after,
            "a second spelling of the folder already on file must not write the registry again"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
