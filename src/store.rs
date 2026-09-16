//! The store: one directory, three files.
//!
//! ```text
//! .vivac/
//!   events    append-only log, one JSON per line   <- SOURCE OF TRUTH
//!   config    project_id and opaque actor
//!   index     derived projection of `events`       <- DISPOSABLE, REGENERABLE
//! ```
//!
//! `index` is not SQLite and not a second home for any state `events` does
//! not already hold: deleting it changes no command's output, only how long
//! building a `Tree` takes. `index.rs` owns its format and every rule about
//! when it is trusted, refreshed or thrown away; this module only names
//! where it lives.

use crate::anchor;
use crate::failure::Failure;
use crate::{clock, id};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".vivac";
pub const LOG: &str = "events";
pub const CONFIG: &str = "config";
pub const INDEX: &str = "index";
pub const LOCK: &str = "lock";
/// The lane file's own name (`d595`). `lane::FILE` reexports it, so the
/// literal is written here and nowhere else.
pub const LANE: &str = "lane";

/// How long a writer waits for another one before it gives up (`d598`).
pub(crate) const LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
/// How long it keeps retrying with a bare yield before it starts sleeping a
/// millisecond between tries: a handoff between two writers takes
/// microseconds, and a sleep would round that up to a timer tick.
const LOCK_SPIN: std::time::Duration = std::time::Duration::from_millis(50);

/// `t594` §4.9: every `.vivac/` ignores itself. One line, `*`, which a git
/// reads as "everything here, this file included", so no file of the
/// user's is touched and a clone never carries a copy of the log.
pub const GITIGNORE: &str = ".gitignore";

/// Where the global store lives, read from the environment.
///
/// `VIVAC_HOME` names the directory itself, the same shape as `CARGO_HOME`:
/// unset, it defaults to `$HOME/.cargo` and, set, *is* the directory. A Rust
/// developer already knows the rule.
pub fn store_dir() -> Option<PathBuf> {
    resolve_store_dir(
        std::env::var_os("VIVAC_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
}

/// Pure: given the three variables, where does the store go?
///
/// Split from `store_dir` so the tests never mutate the environment.
/// `std::env::set_var` is process-global and the test harness runs threads in
/// parallel; two tests setting `VIVAC_HOME` would race and the failure would
/// be intermittent, which is worse than no test at all.
fn resolve_store_dir(
    vivac_home: Option<&OsStr>,
    home: Option<&OsStr>,
    userprofile: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(v) = non_blank(vivac_home) {
        return Some(PathBuf::from(v));
    }
    resolve_home_dir(home, userprofile).map(|h| h.join(DIR))
}

/// Where the user's home directory is, from the environment: `HOME` and
/// then `USERPROFILE`, the same two variables `store_dir` falls back to once
/// `VIVAC_HOME` is not set. Split out so `setup` can refuse to run there
/// without a second search of its own (`t579` §4.1).
pub fn home_dir() -> Option<PathBuf> {
    resolve_home_dir(
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
}

/// Pure half of `home_dir`, for the same reason `resolve_store_dir` is split
/// from `store_dir`.
fn resolve_home_dir(home: Option<&OsStr>, userprofile: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(h) = non_blank(home) {
        return Some(PathBuf::from(h));
    }
    if let Some(u) = non_blank(userprofile) {
        return Some(PathBuf::from(u));
    }
    None
}

/// `None` for a variable that is unset, empty or made only of whitespace: an
/// exported-but-empty variable is a common shell accident, and treating it as
/// "the store is at the filesystem root" would be actively harmful.
fn non_blank(v: Option<&OsStr>) -> Option<&OsStr> {
    let v = v?;
    match v.to_str() {
        Some(s) if s.trim().is_empty() => None,
        _ => Some(v),
    }
}

/// `config`'s `version`, once it is known to be one of the three shapes this
/// release can act on. `d444`: a tree that gains its first pillar or rule
/// turns this from `One` to `Locked`, in place, before the event that
/// creates it is appended -- and a release earlier than that fails to parse
/// `Locked`'s own sentence, which is the whole point. `Lanes` does the same
/// the moment a tree gains a lane.
///
/// No `#[derive(Serialize, Deserialize)]`: none of the three is an enum tag
/// in the usual sense, one is the bare integer `1` and the other two are
/// strings, and `check_config_version` -- not this type -- is what tells a
/// genuinely unknown version apart from one of these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigVersion {
    One,
    Locked,
    Lanes,
}

/// The sentence a config's `version` becomes the moment its tree gains a
/// pillar or a rule. Literal and without a number: release-plz decides the
/// number when it publishes, and every release from here on reads the
/// sentence exactly as it reads `1`. `d444`.
pub const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

/// What a tree's `config` version becomes the moment it holds lanes. Same
/// mechanism as `d444`'s own sentence and for the same reason: a release
/// that does not know lanes must stop with a sentence a person can act on,
/// not read half a tree and act on it.
///
/// 0.12 reads both sentences, so a tree that holds pillars and lanes says
/// this one and loses nothing.
pub const LANE_SENTENCE: &str =
    "this tree holds lanes, and this vivac is too old to read them: update vivac";

impl Serialize for ConfigVersion {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ConfigVersion::One => s.serialize_u32(1),
            ConfigVersion::Locked => s.serialize_str(LOCK_SENTENCE),
            ConfigVersion::Lanes => s.serialize_str(LANE_SENTENCE),
        }
    }
}

impl<'de> Deserialize<'de> for ConfigVersion {
    /// Only ever reached once `check_config_version` has already let the raw
    /// value through: a `1`, the lock sentence, or the lane sentence.
    /// Anything else refuses generically here, which is `d444`'s "como hoy"
    /// for a version this deserializer was never meant to explain --
    /// negative, a float, an object, `null`.
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        match &v {
            serde_json::Value::Number(n) if n.as_u64() == Some(1) => Ok(ConfigVersion::One),
            serde_json::Value::String(s) if s == LOCK_SENTENCE => Ok(ConfigVersion::Locked),
            serde_json::Value::String(s) if s == LANE_SENTENCE => Ok(ConfigVersion::Lanes),
            _ => Err(serde::de::Error::custom("unsupported config version")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: ConfigVersion,
    pub project_id: String,
    /// Opaque identifier for this install. **It carries no email and no name**:
    /// the security pillar forbids it, and vetoes `MODEL.md` §3.4.
    pub actor: String,
}

impl Config {
    fn new_seeded() -> Config {
        Config {
            version: ConfigVersion::One,
            project_id: id::ulid(),
            actor: format!("a_{}", &id::ulid()[..12]),
        }
    }
}

pub struct Store {
    pub root: PathBuf,
    pub config: Config,
    /// The lane every event `append` writes signs as its own. `lane::MAIN`
    /// until `with_lane` says otherwise, which is what keeps a tree nobody
    /// ran `setup` on writing exactly what 0.11 wrote.
    lane: String,
    /// Whether `events` was already there the moment this store opened it.
    /// A process that opens a tree whose log is already gone still recreates
    /// it on the next append, the same as planting would. What this guards
    /// against is narrower: a process that had the log open while it was
    /// there, and then had it moved out from underneath it, fails instead
    /// of silently starting a fresh one in its place.
    log_present: bool,
}

/// Walks up from `from_dir` looking for a `.vivac/`. No daemon and no environment
/// variable: the same rule as git, already in everyone's fingers.
///
/// The global store is a `.vivac/` as well, and it sits in the home directory,
/// so without this it answers the walk: any directory under a home and outside
/// a project resolves to the home itself, and a `push` there writes into the
/// global store instead of refusing. `d206` already said the upward search must
/// not find it, and this is that sentence. It asks what the directory holds
/// rather than where it sits, because `VIVAC_HOME` can move the store and a
/// rule that compared paths would fail exactly when somebody moved it.
pub fn find_root(from_dir: &Path) -> Option<PathBuf> {
    let mut d = from_dir.to_path_buf();
    loop {
        let candidate = d.join(DIR);
        if candidate.is_dir() && !crate::registry::marks_global_store(&candidate) {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

/// What resolving a working folder answers: not just where the tree is,
/// but whose thread this folder is.
#[derive(Debug)]
pub struct Located {
    /// The folder whose `.vivac/` holds `events` and `config`.
    pub root: PathBuf,
    /// The folder whose thread this is. Equal to `root` for a tree whose
    /// own folder is its founding lane, which is every tree today.
    ///
    /// The folder a lane file gets written to, which is setup's job (`t594`
    /// §4.5). Resolution answers it here so that nobody has to walk up
    /// twice.
    pub lane_dir: PathBuf,
    /// The lane as `.vivac/lane` names it. `None` when the folder holding
    /// the tree carries no lane file: the implicit `main` of rule 2.
    pub lane: Option<crate::lane::Lane>,
    /// The root of the linked worktree the command was run inside, when
    /// there is one. Whether it is a lane of its own is not a question the
    /// filesystem can answer -- it depends on the repositories the lane
    /// declared, which are in the log -- so it is answered elsewhere.
    ///
    /// A submodule inside a linked worktree does not come out as one: the
    /// upward walk that finds it stops at the submodule's own `.git`, never
    /// reaching the worktree's, so this reads `None` there. Whoever decides
    /// the lane has to account for that.
    pub worktree: Option<PathBuf>,
}

/// Resolves `from_dir` to the tree it belongs to and the lane it is.
///
/// Walks up looking for a `.vivac/`, the same walk `find_root` always did,
/// and reads only small files past that: a lane file, and the log's own
/// first line when a lane's project has to be checked against an ancestor.
/// **No git and no folded log**: this runs on every process start, and the
/// write budget it shares the process with is 5 ms.
///
/// `None` for exactly the case `find_root` used to answer that way: no
/// `.vivac/` anywhere above `from_dir`, nor -- for a linked worktree --
/// above the main copy its history lives in either.
pub fn locate(from_dir: &Path) -> Result<Option<Located>, Failure> {
    locate_from(from_dir, store_dir().as_deref())
}

/// `locate`'s own algorithm, with the registry's directory taken as an
/// argument rather than read from the environment: `store_dir` reads
/// `VIVAC_HOME`, and mutating that in a test races every other test in the
/// same process, the same reason `resolve_store_dir` above is split from
/// `store_dir`.
fn locate_from(from_dir: &Path, registry_dir: Option<&Path>) -> Result<Option<Located>, Failure> {
    let worktree = anchor::linked_worktree(from_dir);
    let mut found = locate_here(from_dir, registry_dir)?;
    if found.is_none() {
        // `git worktree add ../feature`: the worktree lives outside the
        // folder that holds the product, and nothing above it will ever
        // carry a `.vivac/` of the tree's own.
        if let Some(worktree_root) = &worktree {
            if let Some(main_root) = anchor::main_copy_of(worktree_root) {
                found = locate_here(&main_root, registry_dir)?;
            }
        }
    }
    Ok(found.map(|mut l| {
        l.worktree = worktree;
        l
    }))
}

/// The upward walk for the nearest `.vivac/`, and what it means once found:
/// a lane to resolve, or the implicit `main` every tree with none is.
fn locate_here(from_dir: &Path, registry_dir: Option<&Path>) -> Result<Option<Located>, Failure> {
    let Some(d) = find_root(from_dir) else {
        return Ok(None);
    };
    match crate::lane::read(&d.join(DIR))? {
        Some(l) => resolve_lane(&d, l, registry_dir).map(Some),
        None => Ok(Some(Located {
            root: d.clone(),
            lane_dir: d,
            lane: None,
            worktree: None,
        })),
    }
}

/// Where the tree is for a folder that carries `.vivac/lane`, once it is
/// known this folder does not hold that tree itself: the nearest ancestor
/// whose `.vivac/` holds `events` or `config` **and** whose first event is
/// the lane's own project -- an ancestor that fails the second half is some
/// other tree's and is walked past, not stopped at -- and only then the
/// registry, keyed by that same project.
fn resolve_lane(
    lane_dir: &Path,
    lane: crate::lane::Lane,
    registry_dir: Option<&Path>,
) -> Result<Located, Failure> {
    if already_planted(lane_dir) {
        return Ok(Located {
            root: lane_dir.to_path_buf(),
            lane_dir: lane_dir.to_path_buf(),
            lane: Some(lane),
            worktree: None,
        });
    }
    let mut up = lane_dir.to_path_buf();
    while up.pop() {
        if already_planted(&up) && first_event_id(&up).as_deref() == Some(lane.project.as_str()) {
            return Ok(Located {
                root: up,
                lane_dir: lane_dir.to_path_buf(),
                lane: Some(lane),
                worktree: None,
            });
        }
    }
    if let Some(registry_dir) = registry_dir {
        if let Some(root) = crate::registry::root_of(registry_dir, &lane.project) {
            return Ok(Located {
                root,
                lane_dir: lane_dir.to_path_buf(),
                lane: Some(lane),
                worktree: None,
            });
        }
    }
    // `t594` fix-2, finding 1: writing this folder's own `.vivac/lane` is
    // what takes away the one path that used to resolve it. A linked
    // worktree with *no* lane file at all never reaches this function --
    // `locate_here` answers `None` for it, and `locate_from`'s own
    // fallback retries from the worktree's main copy. Once a lane file
    // exists here, resolution comes through this function instead, and
    // that fallback is never reached: neither the ancestor walk above nor
    // the registry knows anything about a path found by retrying from a
    // main copy. So this tries the retry itself, last, with the same
    // fingerprint check the ancestor walk above already uses -- a main
    // copy of some other repository entirely does not carry this lane's
    // `project` as its first event, so it is walked past rather than
    // mistaken for the right one. The registry stays a convenience that
    // can fail quietly (`registry::note` swallows its own errors): this
    // is what keeps a folder from going unusable just because it could
    // not be written to.
    if let Some(worktree_root) = crate::anchor::linked_worktree(lane_dir) {
        if let Some(main_root) = crate::anchor::main_copy_of(&worktree_root) {
            let mut up = main_root;
            loop {
                if already_planted(&up)
                    && first_event_id(&up).as_deref() == Some(lane.project.as_str())
                {
                    return Ok(Located {
                        root: up,
                        lane_dir: lane_dir.to_path_buf(),
                        lane: Some(lane),
                        worktree: None,
                    });
                }
                if !up.pop() {
                    break;
                }
            }
        }
    }
    Err(Failure::tree_not_found())
}

/// Whether `root/.vivac/` already holds a tree worth opening rather than
/// creating: a config or a log. `f566`: an empty `.vivac/` -- one that exists
/// as a directory but holds neither -- is planted like a new one, and `init`
/// and `setup` share this one check rather than each guessing it their own
/// way.
pub fn already_planted(root: &Path) -> bool {
    let dir = root.join(DIR);
    dir.join(CONFIG).is_file() || dir.join(LOG).is_file()
}

/// `t594` §4.9: every `.vivac/` ignores itself, tree or lane. Creates the
/// directory if it is not there, and writes nothing over a file that
/// already exists -- somebody may have added a line of their own.
pub fn write_gitignore(vivac_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(vivac_dir)?;
    let ignore = vivac_dir.join(GITIGNORE);
    if !ignore.exists() {
        fs::write(&ignore, "*\n")?;
    }
    Ok(())
}

/// The log's length and modification time: the whole change detector.
/// The log only grows, so a different length is exact; the time rides
/// along for a rewrite that lands on the same byte count.
pub(crate) fn fingerprint(log: &Path) -> (u64, Option<std::time::SystemTime>) {
    match fs::metadata(log) {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    }
}

/// The same pair as `fingerprint`, read off a handle already open, so the
/// file it is still reading can be compared against whatever the path
/// names now.
pub(crate) fn fingerprint_in(f: &File) -> (u64, Option<std::time::SystemTime>) {
    match f.metadata() {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    }
}

/// One tree's write lock, held for as long as this value lives (`d598`).
/// Dropping it releases the lock, and so does the process dying: the
/// operating system lets go of every lock a dead process held, so there
/// is never a stale lock to clean up by hand.
///
/// It locks `.vivac/lock` and never the log: on Windows a lock taken with
/// `LockFileEx` is mandatory, and a locked log would refuse its readers.
pub struct WriteLock {
    file: File,
    path: PathBuf,
}

impl WriteLock {
    /// Whether this lock is the one that covers `lock_path`. `Store::append`
    /// asks before it writes: a lock is only a lock over the tree whose file
    /// it holds, and taking one tree's lock to write another's would look
    /// exactly like holding no lock at all (`f602`).
    pub fn covers(&self, lock_path: &Path) -> bool {
        self.path == lock_path
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Takes the lock at `path`, retrying until `deadline`. A handoff is
/// microseconds, so it yields for `LOCK_SPIN` before it starts sleeping.
pub(crate) fn lock_with_deadline(
    path: &Path,
    deadline: std::time::Duration,
) -> Result<WriteLock, Failure> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => {
                return Ok(WriteLock {
                    file,
                    path: path.to_path_buf(),
                })
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        let waited = start.elapsed();
        if waited >= deadline {
            return Err(Failure::busy(deadline));
        }
        if waited < LOCK_SPIN {
            std::thread::yield_now();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// The `id` of line 1 of `<root>/.vivac/events`, without folding the rest of
/// the log. An empty log, an unreadable file or a first line that will not
/// parse all come back `None`; the caller decides what that means.
pub fn first_event_id(root: &Path) -> Option<String> {
    let f = File::open(root.join(DIR).join(LOG)).ok()?;
    let mut line = String::new();
    BufReader::new(f).read_line(&mut line).ok()?;
    if line.trim().is_empty() {
        return None;
    }
    let e: crate::event::Event = serde_json::from_str(line.trim_end()).ok()?;
    Some(e.id)
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Store, Failure> {
        let p = root.join(DIR).join(CONFIG);
        let config = match fs::read_to_string(&p) {
            Ok(s) => read_config(&s)?,
            Err(_) => {
                // A `.vivac/` with no config comes from an earlier version or a
                // half-finished delete. Fill it in rather than fail: the tree,
                // which is what matters, lives in `events`. `d444`: if the log
                // already carries a pillar, a rule or a lane, the regenerated
                // config is born locked to whichever of those sentences the
                // log backs up -- deleting one file must never hand an older
                // release a config that looks readable over a tree it is not.
                let c = Config {
                    version: regenerated_version(&root),
                    ..Config::new_seeded()
                };
                write_config(&root, &c)?;
                c
            }
        };
        let log_present = root.join(DIR).join(LOG).is_file();
        Ok(Store {
            root,
            config,
            lane: crate::lane::MAIN.to_string(),
            log_present,
        })
    }

    /// For a `.vivac/` that does not exist yet. `f566`: `init` on one that
    /// already does calls `open` instead, so this always writes a fresh
    /// config -- calling it over an existing tree would hand it a new
    /// `project_id` and drop `d444`'s lock back to `1`.
    pub fn create(root: &Path) -> std::io::Result<Store> {
        let d = root.join(DIR);
        fs::create_dir_all(&d)?;
        let config = Config::new_seeded();
        write_config(root, &config)?;
        if !d.join(LOG).exists() {
            File::create(d.join(LOG))?;
        }
        write_gitignore(&d)?;
        Ok(Store {
            root: root.to_path_buf(),
            config,
            lane: crate::lane::MAIN.to_string(),
            log_present: true,
        })
    }

    /// Sets which lane this store signs every event as. Builder-style,
    /// consuming `self`, so a caller that never calls it keeps the `main`
    /// that `open` and `create` already set -- which is what keeps a tree
    /// nobody ran `setup` on signing exactly what it always has.
    pub fn with_lane(mut self, lane: String) -> Store {
        self.lane = lane;
        self
    }

    pub fn log(&self) -> PathBuf {
        self.root.join(DIR).join(LOG)
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join(DIR).join(INDEX)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(DIR).join(LOCK)
    }

    /// Takes this tree's write lock (`d598`), waiting for another writer
    /// for up to five seconds. Hold it from the moment the tree is brought
    /// up to date until the append is done.
    pub fn lock_for_write(&self) -> Result<WriteLock, Failure> {
        lock_with_deadline(&self.lock_path(), LOCK_DEADLINE)
    }
}

fn write_config(root: &Path, c: &Config) -> std::io::Result<()> {
    let mut f = File::create(root.join(DIR).join(CONFIG))?;
    f.write_all(serde_json::to_string_pretty(c)?.as_bytes())?;
    f.write_all(b"\n")
}

/// `d444`'s own protection: the config is written to a sibling temporary
/// file and renamed over the real one, never edited in place. A process
/// that dies between the two steps leaves the old config exactly as it
/// was -- there is no window where `config` itself is half-written.
fn write_config_atomic(root: &Path, c: &Config) -> std::io::Result<()> {
    let dir = root.join(DIR);
    let tmp = dir.join("config.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(c)?.as_bytes())?;
        f.write_all(b"\n")?;
    }
    fs::rename(&tmp, dir.join(CONFIG))
}

/// Parses `config`'s text into a `Config`, refusing the two shapes of
/// `version` `t411` §27 gives a name to before letting `serde_json` see the
/// rest: a non-negative integer that is not `1`, or a string that is not
/// `d444`'s own sentence. Every other shape -- absent, negative, a float, an
/// object, `null` -- is left to `Config`'s own `Deserialize`, which fails
/// exactly as it did before `d444`.
fn read_config(raw: &str) -> Result<Config, Failure> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    check_config_version(v.get("version"))?;
    serde_json::from_value(v).map_err(|e| Failure::Io(std::io::Error::other(e)))
}

fn check_config_version(version: Option<&serde_json::Value>) -> Result<(), Failure> {
    match version {
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(1) => Ok(()),
            Some(other) => Err(Failure::newer_vivac(format!(
                "This tree was written by a newer vivac: its config has version {other}, \
                 which this version does not know. Update vivac to read it. Nothing was \
                 written."
            ))),
            // Negative or non-integer: not one of the two known shapes, and
            // not a value worth a friendly message either. Falls through to
            // the generic config-read failure, same as before `d444`.
            None => Ok(()),
        },
        Some(serde_json::Value::String(s)) if s == LOCK_SENTENCE => Ok(()),
        Some(serde_json::Value::String(s)) if s == LANE_SENTENCE => Ok(()),
        Some(serde_json::Value::String(s)) => Err(Failure::newer_vivac(format!(
            "This tree was written by a newer vivac: its config says {s:?}. Update vivac \
             to read it. Nothing was written."
        ))),
        _ => Ok(()),
    }
}

/// The rare path `Store::open` takes when `config` itself is missing: which
/// sentence, if any, the log already backs up. Reads the whole log --
/// something no ordinary read ever pays for -- because a vanished config is
/// itself the unusual case, and regenerating one that looks readable by any
/// release over a tree that already governs something, or already holds a
/// lane, would undo the very lock `d444` and `t594` §2.6 exist to keep.
///
/// A lane wins over a pillar or a rule when a log somehow carries both: 0.12
/// reads both sentences, so a tree that holds pillars and lanes still says
/// this one and loses nothing, and there is no third sentence for "both" to
/// pick instead.
fn regenerated_version(root: &Path) -> ConfigVersion {
    let Ok(f) = File::open(root.join(DIR).join(LOG)) else {
        return ConfigVersion::One;
    };
    let mut governed = false;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["payload"]["type"].as_str() {
            Some("lane.declared") | Some("lane.claimed") => return ConfigVersion::Lanes,
            Some("node.created")
                if matches!(v["payload"]["kind"].as_str(), Some("pillar") | Some("rule")) =>
            {
                governed = true;
            }
            _ => {}
        }
    }
    if governed {
        ConfigVersion::Locked
    } else {
        ConfigVersion::One
    }
}

/// `config`'s version, read directly and without writing anything: `None`
/// for a tree with no config at all, or one this release cannot make sense
/// of. `Store::open` would fill a missing one in, and that write is exactly
/// what a caller that must never write -- `setup`'s own `--dry-run` -- is
/// not allowed to trigger just by asking what version a tree is on
/// (`t594` fix-1, finding 6).
pub(crate) fn peek_config_version(root: &Path) -> Option<ConfigVersion> {
    let raw = fs::read_to_string(root.join(DIR).join(CONFIG)).ok()?;
    read_config(&raw).ok().map(|c| c.version)
}

/// What an append left behind: the events as written, plus where the last
/// of them begins and where the file now ends. A caller that keeps a tree
/// in memory needs those two numbers to stay current without reading back
/// what it just wrote -- and reading it back is not free: opening the log
/// again right after writing it costs about six milliseconds a write on
/// this machine, more than the whole write budget (`f599`).
pub struct Appended {
    pub events: Vec<crate::event::Event>,
    /// The log's length right before this append, so a caller can tell
    /// whether these lines are a clean continuation of what it already
    /// folded, or landed behind bytes it never saw (`f599`).
    pub previous_len: u64,
    pub last_line_offset: u64,
    pub end_offset: u64,
}

impl Store {
    /// Reads the whole log. An unreadable line **does not abort**: it is
    /// counted and skipped. A half-written log has to stay readable, or the
    /// tool that keeps the thread becomes the one that loses it.
    ///
    /// One case refuses instead of skipping: `t411` §13, a line that is
    /// well-formed JSON but names an event type or a node kind this version
    /// does not know. That line was written by a newer vivac, and reading
    /// past it in silence would mean acting on a tree this version cannot
    /// actually see all of.
    pub fn read_all(&self) -> Result<(Vec<crate::event::Event>, usize), Failure> {
        read_all_from(&self.log())
    }

    /// Appends events at the end. One line per event, rewriting nothing.
    ///
    /// This is the critical path of the agent's turn: a p99 < 5 ms budget.
    /// That is why there is no `fsync` --on Windows it costs more than the
    /// whole budget-- and why it opens in `append` mode, which makes each
    /// single-line write atomic. Atomic lines do not make two writers agree
    /// on `seq` and `num`, though: the write lock is an argument here, not a
    /// convention a caller could forget or take twice. Without one this does
    /// not compile, and `lock.covers` refuses one taken on another tree's
    /// `.vivac/lock` (`f602`).
    ///
    /// `tree_already_governed` is `d444`'s own check, paid before any of
    /// `body` reaches disk: the config locks in place, first, so a process
    /// that dies between the two leaves an unlocked config over a tree with
    /// no pillar and no rule, which is harmless.
    ///
    /// Returns the events as written and where their bytes landed, so a
    /// caller that keeps the tree in memory applies exactly those and never
    /// stamps them a second time (`f590`), and a caller that keeps a
    /// resident tree never has to read the log back to learn where its own
    /// write landed (`f599`).
    pub fn append(
        &mut self,
        lock: &WriteLock,
        body: Vec<crate::event::Body>,
        from_seq: u64,
        tree_already_governed: bool,
    ) -> std::io::Result<Appended> {
        if !lock.covers(&self.lock_path()) {
            return Err(std::io::Error::other("write lock does not cover this tree"));
        }
        self.lock_if_needed(&body, tree_already_governed)?;
        let mut buf = String::with_capacity(256 * body.len());
        let mut written = Vec::with_capacity(body.len());
        let mut last_line_start = 0usize;
        for (i, c) in body.into_iter().enumerate() {
            let e = crate::event::Event {
                seq: from_seq + i as u64 + 1,
                id: id::ulid(),
                ts: clock::now_rfc3339(),
                actor: self.config.actor.clone(),
                lane: self.lane.clone(),
                payload: c,
            };
            last_line_start = buf.len();
            buf.push_str(&serde_json::to_string(&e).map_err(std::io::Error::other)?);
            buf.push('\n');
            written.push(e);
        }
        let mut f = OpenOptions::new()
            .create(!self.log_present)
            .append(true)
            .open(self.log())?;
        let previous_len = f.metadata()?.len();
        f.write_all(buf.as_bytes())?;
        self.log_present = true;
        Ok(Appended {
            previous_len,
            last_line_offset: previous_len + last_line_start as u64,
            end_offset: previous_len + buf.len() as u64,
            events: written,
        })
    }

    /// `d444`: locks the config in place the moment this tree gains its
    /// first pillar or rule -- before the event that creates one is
    /// appended. A no-op once the config is already locked, and a no-op for
    /// every write that neither creates a pillar or a rule nor lands on a
    /// tree that already has one.
    fn lock_if_needed(
        &mut self,
        body: &[crate::event::Body],
        tree_already_governed: bool,
    ) -> std::io::Result<()> {
        if self.config.version != ConfigVersion::One {
            return Ok(());
        }
        let creates_governance = body.iter().any(|b| {
            matches!(
                b,
                crate::event::Body::NodeCreated {
                    kind: crate::event::Kind::Pillar | crate::event::Kind::Rule,
                    ..
                }
            )
        });
        if !tree_already_governed && !creates_governance {
            return Ok(());
        }
        let locked = Config {
            version: ConfigVersion::Locked,
            project_id: self.config.project_id.clone(),
            actor: self.config.actor.clone(),
        };
        write_config_atomic(&self.root, &locked)?;
        self.config = locked;
        Ok(())
    }

    /// Locks the config in place the moment this tree gains a lane, the same
    /// mechanism `lock_if_needed` uses for a pillar or a rule and by the same
    /// `write_config_atomic`. A no-op once the config already says `Lanes`.
    ///
    /// Takes the write lock as an argument for the same reason `append`
    /// does: without one this does not compile, and `lock.covers` refuses
    /// one taken on another tree's `.vivac/lock` (`f602`) -- `config.tmp`'s
    /// own name is fixed, so it is only safe with nobody else writing at
    /// the same time.
    pub fn lock_lanes_in_config(&mut self, lock: &WriteLock) -> std::io::Result<()> {
        if !lock.covers(&self.lock_path()) {
            return Err(std::io::Error::other("write lock does not cover this tree"));
        }
        if self.config.version == ConfigVersion::Lanes {
            return Ok(());
        }
        let locked = Config {
            version: ConfigVersion::Lanes,
            project_id: self.config.project_id.clone(),
            actor: self.config.actor.clone(),
        };
        write_config_atomic(&self.root, &locked)?;
        self.config = locked;
        Ok(())
    }
}

/// The read `Store::read_all` runs, taken as a free function of a path
/// rather than a method: `index.rs`'s own tail read (`read_tracked`) keeps a
/// separate implementation for its own reasons (`LOADING.md` §4), but when
/// it hits a line `t411` §13 refuses over, it falls back to a full read from
/// byte zero rather than reconstructing this file's own line count -- and
/// that full read is this function, so the two paths report the very same
/// line number for the very same line.
pub(crate) fn read_all_from(path: &Path) -> Result<(Vec<crate::event::Event>, usize), Failure> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(e) => return Err(e.into()),
    };
    let mut reader = BufReader::new(f);
    let mut events = Vec::new();
    let mut broken = 0usize;
    let mut line_no = 0usize;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        let n = reader.read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        line_no += 1;
        if raw.last() != Some(&b'\n') {
            // An append that stopped mid-write is not a line until it
            // ends -- the same rule `index::read_tracked` follows, so a
            // log the two of them read agrees on what it holds (`f599`).
            if !String::from_utf8_lossy(&raw).trim().is_empty() {
                broken += 1;
            }
            break;
        }
        let mut bytes = raw.as_slice();
        if bytes.last() == Some(&b'\n') {
            bytes = &bytes[..bytes.len() - 1];
        }
        if bytes.last() == Some(&b'\r') {
            bytes = &bytes[..bytes.len() - 1];
        }
        let line = String::from_utf8(bytes.to_vec()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(e) => events.push(e),
            Err(_) => match crate::event::unknown_reason_for(&line) {
                Some(reason) => return Err(newer_vivac_failure(line_no, reason)),
                None => broken += 1,
            },
        }
    }
    Ok((events, broken))
}

/// The exact wording of `t411` §13's refusal, for the one line that earned
/// it. `line_no` is 1-based, matching what a text editor would show.
pub(crate) fn newer_vivac_failure(line_no: usize, reason: crate::event::UnknownReason) -> Failure {
    let path = format!("{DIR}/{LOG}");
    let detail = match reason {
        crate::event::UnknownReason::EventType(t) => {
            format!("is an event this version does not know ({t})")
        }
        crate::event::UnknownReason::NodeKind(k) => {
            format!("creates a node of a type this version does not know ({k})")
        }
        crate::event::UnknownReason::Shape(t) => {
            format!("is a {t} event whose fields this version cannot read")
        }
    };
    Failure::newer_vivac(format!(
        "This tree was written by a newer vivac: line {line_no} of {path} {detail}. \
         Update vivac to read it. Nothing was written."
    ))
}

impl Store {
    /// Writes already-built events, keeping their original timestamp. Only
    /// `import` uses it: a tree from elsewhere keeps its dates, because
    /// otherwise the migration flattens the only timeline it had. Takes the
    /// write lock as an argument for the same reason `append` does: without
    /// one this does not compile, and `lock.covers` refuses one taken on
    /// another tree's `.vivac/lock` (`f602`).
    pub fn write_raw(
        &self,
        lock: &WriteLock,
        events: &[crate::event::Event],
    ) -> std::io::Result<()> {
        if !lock.covers(&self.lock_path()) {
            return Err(std::io::Error::other("write lock does not cover this tree"));
        }
        let mut buf = String::with_capacity(256 * events.len());
        for e in events {
            buf.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(!self.log_present)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_upward() {
        let tmp = std::env::temp_dir().join(format!("vivac-t-{}", id::ulid()));
        let depth_of = tmp.join("a").join("b").join("c");
        fs::create_dir_all(&depth_of).unwrap();
        assert!(find_root(&depth_of).is_none());
        Store::create(&tmp).unwrap();
        assert_eq!(find_root(&depth_of).unwrap(), tmp);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_global_store_does_not_answer_the_walk() {
        // The collision as it shipped: the global store is a `.vivac/` too, so
        // a directory with no project above it resolved to the home directory
        // and wrote there without saying so.
        let tmp = std::env::temp_dir().join(format!("vivac-t-{}", id::ulid()));
        let deep = tmp.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        Store::create(&tmp).unwrap();
        assert_eq!(find_root(&deep).unwrap(), tmp);
        crate::registry::note(&tmp.join(DIR), "01aaaaaaaaaaaaaaaaaaaaaaaa", &deep);
        assert_ne!(find_root(&deep), Some(tmp.clone()));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_project_under_the_global_store_still_wins() {
        // Skipping the global store must not cost a real project below it.
        let tmp = std::env::temp_dir().join(format!("vivac-t-{}", id::ulid()));
        let project = tmp.join("work");
        let deep = project.join("src").join("deep");
        fs::create_dir_all(&deep).unwrap();
        Store::create(&tmp).unwrap();
        crate::registry::note(&tmp.join(DIR), "01aaaaaaaaaaaaaaaaaaaaaaaa", &project);
        Store::create(&project).unwrap();
        assert_eq!(find_root(&deep).unwrap(), project);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_actor_carries_no_personal_data() {
        let c = Config::new_seeded();
        assert!(c.actor.starts_with("a_"));
        assert!(!c.actor.contains('@'));
        assert_ne!(c.actor, whoami_ish());
    }

    fn whoami_ish() -> String {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default()
    }

    #[test]
    fn vivac_home_wins_and_is_used_as_is() {
        let got = resolve_store_dir(
            Some(OsStr::new("/somewhere/store")),
            Some(OsStr::new("/home/anyone")),
            Some(OsStr::new("C:\\Users\\anyone")),
        );
        assert_eq!(got, Some(PathBuf::from("/somewhere/store")));
    }

    #[test]
    fn blank_vivac_home_falls_through() {
        let got = resolve_store_dir(
            Some(OsStr::new("   ")),
            Some(OsStr::new("/home/anyone")),
            None,
        );
        assert_eq!(got, Some(PathBuf::from("/home/anyone").join(DIR)));
    }

    #[test]
    fn home_alone_appends_dir() {
        let got = resolve_store_dir(None, Some(OsStr::new("/home/anyone")), None);
        assert_eq!(got, Some(PathBuf::from("/home/anyone").join(DIR)));
    }

    #[test]
    fn userprofile_used_when_home_is_absent() {
        let got = resolve_store_dir(None, None, Some(OsStr::new("C:\\Users\\anyone")));
        assert_eq!(got, Some(PathBuf::from("C:\\Users\\anyone").join(DIR)));
    }

    #[test]
    fn home_wins_over_userprofile() {
        let got = resolve_store_dir(
            None,
            Some(OsStr::new("/home/anyone")),
            Some(OsStr::new("C:\\Users\\anyone")),
        );
        assert_eq!(got, Some(PathBuf::from("/home/anyone").join(DIR)));
    }

    #[test]
    fn nothing_set_means_no_global_store() {
        assert_eq!(resolve_store_dir(None, None, None), None);
    }

    #[test]
    fn first_event_id_on_an_empty_log_is_none() {
        let tmp = std::env::temp_dir().join(format!("vivac-fe-{}", id::ulid()));
        Store::create(&tmp).unwrap();
        assert_eq!(first_event_id(&tmp), None);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn first_event_id_reads_line_one_without_folding() {
        let tmp = std::env::temp_dir().join(format!("vivac-fe-{}", id::ulid()));
        let mut s = Store::create(&tmp).unwrap();
        let lock = s.lock_for_write().unwrap();
        // A log large enough that folding the whole thing would be visible
        // in the timing, if this ever regressed into calling `read_all`.
        for _ in 0..500 {
            s.append(
                &lock,
                vec![crate::event::Body::NodeNoted {
                    node: "t1".into(),
                    note: "filler".into(),
                }],
                0,
                false,
            )
            .unwrap();
        }
        let first_line = fs::read_to_string(s.log())
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        let want: crate::event::Event = serde_json::from_str(&first_line).unwrap();
        assert_eq!(first_event_id(&tmp), Some(want.id));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_second_writer_waits_for_the_lock_and_then_gives_up() {
        let tmp = std::env::temp_dir().join(format!("vivac-lock-{}", id::ulid()));
        fs::create_dir_all(&tmp).unwrap();
        Store::create(&tmp).unwrap();
        let s = Store::open(tmp.clone()).unwrap();
        let held = s.lock_for_write().unwrap();
        let second = lock_with_deadline(&s.lock_path(), std::time::Duration::from_millis(200));
        assert!(
            matches!(second, Err(Failure::Busy(_))),
            "the lock let a second writer in"
        );
        drop(held);
        assert!(
            lock_with_deadline(&s.lock_path(), std::time::Duration::from_millis(200)).is_ok(),
            "dropping the first lock did not release it"
        );
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_busy_failure_names_the_deadline_it_was_given() {
        let f = Failure::busy(std::time::Duration::from_secs(5));
        assert_eq!(f.code(), 5);
        assert!(
            f.message().contains("held this tree for 5 seconds"),
            "{}",
            f.message()
        );
    }

    #[test]
    fn append_never_recreates_a_log_that_vanished() {
        let tmp = std::env::temp_dir().join(format!("vivac-vanished-{}", id::ulid()));
        fs::create_dir_all(&tmp).unwrap();
        Store::create(&tmp).unwrap();
        let mut s = Store::open(tmp.clone()).unwrap();
        let lock = s.lock_for_write().unwrap();
        fs::remove_file(s.log()).unwrap();
        let body = vec![crate::event::Body::NodeNoted {
            node: "01VANISHEDAAAAAAAAAAAAAAAA".into(),
            note: "x".into(),
        }];
        assert!(
            s.append(&lock, body, 0, false).is_err(),
            "append wrote into a log that is gone"
        );
        assert!(
            !s.log().exists(),
            "append created a new log where the old one was"
        );
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn appending_with_another_trees_lock_is_refused() {
        let a = std::env::temp_dir().join(format!("vivac-locka-{}", id::ulid()));
        let b = std::env::temp_dir().join(format!("vivac-lockb-{}", id::ulid()));
        Store::create(&a).unwrap();
        Store::create(&b).unwrap();
        let mut sa = Store::open(a.clone()).unwrap();
        let sb = Store::open(b.clone()).unwrap();
        let wrong = sb.lock_for_write().unwrap();
        let body = vec![crate::event::Body::NodeNoted {
            node: "t1".into(),
            note: "x".into(),
        }];
        assert!(
            sa.append(&wrong, body, 0, false).is_err(),
            "append accepted a lock taken on a different tree"
        );
        fs::remove_dir_all(&a).ok();
        fs::remove_dir_all(&b).ok();
    }

    fn locate_tmp(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vivac-locate-{prefix}-{}", id::ulid()))
    }

    /// Writes the `.git` file a linked worktree and a submodule both carry:
    /// a file, at `working_dir`, naming a `gitdir` elsewhere.
    fn write_git_file(working_dir: &Path, gitdir: &Path) {
        fs::create_dir_all(working_dir).unwrap();
        fs::create_dir_all(gitdir).unwrap();
        fs::write(
            working_dir.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
    }

    #[test]
    fn the_tree_in_this_very_folder() {
        let tmp = locate_tmp("here");
        Store::create(&tmp).unwrap();
        let located = locate(&tmp).unwrap().unwrap();
        assert_eq!(located.root, tmp);
        assert_eq!(located.lane_dir, tmp);
        assert!(located.lane.is_none());
        assert!(located.worktree.is_none());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn an_empty_vivac_directory_still_answers_the_walk() {
        // `f566`: a `.vivac/` that holds neither a log nor a config is the
        // shape a half-finished delete leaves behind, and it resolves to
        // itself, exactly as it did before lanes. Walking past it to the
        // tree above would quietly move somebody's work to another tree.
        let tmp = locate_tmp("empty-vivac");
        fs::create_dir_all(tmp.join(DIR)).unwrap();
        let located = locate(&tmp).unwrap().unwrap();
        assert_eq!(located.root, tmp);
        assert_eq!(located.lane_dir, tmp);
        assert!(located.lane.is_none());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_lane_below_the_tree_finds_it_walking_up() {
        let tmp = locate_tmp("below");
        let mut s = Store::create(&tmp).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.append(
            &lock,
            vec![crate::event::Body::NodeNoted {
                node: "t1".into(),
                note: "seed".into(),
            }],
            0,
            false,
        )
        .unwrap();
        drop(lock);
        let project = first_event_id(&tmp).unwrap();
        let lane_dir = tmp.join("lane");
        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: project.clone(),
        };
        crate::lane::write(&lane_dir.join(DIR), &lane).unwrap();

        let located = locate(&lane_dir).unwrap().unwrap();
        assert_eq!(located.root, tmp);
        assert_eq!(located.lane_dir, lane_dir);
        assert_eq!(located.lane.unwrap().project, project);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_lane_outside_the_tree_finds_it_through_the_registry() {
        let lane_dir = locate_tmp("outside");
        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: "01OUTSIDEPROJECTAAAAAAAAAA".into(),
        };
        crate::lane::write(&lane_dir.join(DIR), &lane).unwrap();

        let registry_dir = locate_tmp("outside-registry");
        let noted_root = locate_tmp("outside-fake-root");
        crate::registry::note(&registry_dir, &lane.project, &noted_root);

        let located = locate_from(&lane_dir, Some(&registry_dir))
            .unwrap()
            .unwrap();
        assert_eq!(located.root, noted_root);
        assert_eq!(located.lane_dir, lane_dir);

        fs::remove_dir_all(&lane_dir).ok();
        fs::remove_dir_all(&registry_dir).ok();
    }

    #[test]
    fn a_lane_whose_tree_the_registry_does_not_know_refuses_with_exit_4() {
        let lane_dir = locate_tmp("unknown");
        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: "01UNKNOWNPROJECTAAAAAAAAAA".into(),
        };
        crate::lane::write(&lane_dir.join(DIR), &lane).unwrap();
        let registry_dir = locate_tmp("unknown-registry");

        let err = locate_from(&lane_dir, Some(&registry_dir)).unwrap_err();
        assert_eq!(err.code(), 4);
        assert!(
            err.message().contains("registry does not know"),
            "{}",
            err.message()
        );
        fs::remove_dir_all(&lane_dir).ok();
    }

    #[test]
    fn a_subfolder_belongs_to_the_nearest_lane_above() {
        let outer = locate_tmp("nearest-outer");
        Store::create(&outer).unwrap(); // a distractor tree, further up
        let lane_dir = outer.join("consumer");
        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: "01NEARESTPROJECTAAAAAAAAAA".into(),
        };
        crate::lane::write(&lane_dir.join(DIR), &lane).unwrap();
        let deep = lane_dir.join("x").join("y");
        fs::create_dir_all(&deep).unwrap();

        let registry_dir = locate_tmp("nearest-registry");
        let noted_root = locate_tmp("nearest-fake-root");
        crate::registry::note(&registry_dir, &lane.project, &noted_root);

        let located = locate_from(&deep, Some(&registry_dir)).unwrap().unwrap();
        assert_eq!(
            located.lane_dir, lane_dir,
            "picked a farther .vivac/ than the nearest one"
        );
        assert_eq!(located.root, noted_root);

        fs::remove_dir_all(&outer).ok();
        fs::remove_dir_all(&registry_dir).ok();
    }

    #[test]
    fn a_linked_worktree_inside_the_lane_is_reported_as_a_worktree() {
        let tmp = locate_tmp("wt-inside");
        let worktree_dir = tmp.join("feature");
        let gitdir = tmp
            .join("main")
            .join(".git")
            .join("worktrees")
            .join("feature");
        write_git_file(&worktree_dir, &gitdir);
        fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: "01WTINSIDEPROJECTAAAAAAAAA".into(),
        };
        crate::lane::write(&worktree_dir.join(DIR), &lane).unwrap();

        let registry_dir = locate_tmp("wt-inside-registry");
        let noted_root = locate_tmp("wt-inside-fake-root");
        crate::registry::note(&registry_dir, &lane.project, &noted_root);

        let deep = worktree_dir.join("src").join("deep");
        fs::create_dir_all(&deep).unwrap();

        let located = locate_from(&deep, Some(&registry_dir)).unwrap().unwrap();
        assert_eq!(located.root, noted_root);
        assert_eq!(
            located.lane_dir, worktree_dir,
            "who the lane really is gets decided elsewhere, not here"
        );
        assert_eq!(located.worktree, Some(worktree_dir.clone()));

        fs::remove_dir_all(&tmp).ok();
        fs::remove_dir_all(&registry_dir).ok();
    }

    #[test]
    fn a_linked_worktree_outside_any_lane_finds_the_tree_through_its_main_copy() {
        let tmp = locate_tmp("wt-outside");
        let main_dir = tmp.join("main");
        Store::create(&main_dir).unwrap();
        let worktree_dir = tmp.join("feature");
        let gitdir = main_dir.join(".git").join("worktrees").join("feature");
        write_git_file(&worktree_dir, &gitdir);
        fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let located = locate(&worktree_dir).unwrap().unwrap();
        assert_eq!(located.root, main_dir);
        assert_eq!(located.lane_dir, main_dir);
        assert!(located.lane.is_none());
        assert_eq!(located.worktree, Some(worktree_dir));

        fs::remove_dir_all(&tmp).ok();
    }

    /// `t594` fix-2, finding 1: once the folder inside a linked worktree
    /// carries its own `.vivac/lane`, resolution takes `resolve_lane`
    /// rather than the plain "no lane" fallback the test above exercises
    /// -- and until this fix, that path never tried the worktree's main
    /// copy at all, so a lane file with no registry entry to back it up
    /// used to leave the folder unable to find its own tree.
    #[test]
    fn a_lane_file_inside_a_linked_worktree_still_resolves_through_its_main_copy() {
        let tmp = locate_tmp("wt-lane-fallback");
        let main_dir = tmp.join("main");
        let mut s = Store::create(&main_dir).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.append(
            &lock,
            vec![crate::event::Body::NodeNoted {
                node: "t1".into(),
                note: "seed".into(),
            }],
            0,
            false,
        )
        .unwrap();
        drop(lock);
        let project = first_event_id(&main_dir).unwrap();

        let worktree_dir = tmp.join("feature");
        let gitdir = main_dir.join(".git").join("worktrees").join("feature");
        write_git_file(&worktree_dir, &gitdir);
        fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let lane = crate::lane::Lane {
            version: 1,
            id: crate::lane::new_id(),
            project: project.clone(),
        };
        crate::lane::write(&worktree_dir.join(DIR), &lane).unwrap();

        // No registry at all: the only path left back to the tree is the
        // main-copy retry inside `resolve_lane` itself.
        let located = locate_from(&worktree_dir, None).unwrap().unwrap();
        assert_eq!(located.root, main_dir);
        assert_eq!(located.lane_dir, worktree_dir);
        assert_eq!(located.lane.unwrap().project, project);

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_submodule_is_not_a_worktree() {
        let tmp = locate_tmp("submodule");
        Store::create(&tmp).unwrap();
        let sub_dir = tmp.join("vendor").join("lib");
        let gitdir = tmp.join(".git-modules").join("lib");
        write_git_file(&sub_dir, &gitdir);
        // No `commondir` written: a submodule owns its own repository.

        let located = locate(&sub_dir).unwrap().unwrap();
        assert_eq!(located.root, tmp);
        assert!(
            located.worktree.is_none(),
            "a submodule was reported as a linked worktree"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_global_store_still_does_not_answer_the_walk() {
        let tmp = locate_tmp("global");
        let deep = tmp.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        Store::create(&tmp).unwrap();
        crate::registry::note(&tmp.join(DIR), "01aaaaaaaaaaaaaaaaaaaaaaaa", &deep);
        assert!(locate(&deep).unwrap().is_none());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn an_event_is_signed_by_the_lane_that_wrote_it() {
        let tmp = std::env::temp_dir().join(format!("vivac-sign-{}", id::ulid()));
        Store::create(&tmp).unwrap();
        let mut s = Store::open(tmp.clone())
            .unwrap()
            .with_lane("01M2XYZ".into());
        let lock = s.lock_for_write().unwrap();
        let w = s
            .append(
                &lock,
                vec![crate::event::Body::NodeNoted {
                    node: "t1".into(),
                    note: "x".into(),
                }],
                0,
                false,
            )
            .unwrap();
        assert_eq!(w.events[0].lane, "01M2XYZ");
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_store_nobody_told_a_lane_still_signs_main() {
        // Every tree that exists today, and every tree where nobody has run
        // setup: the log has to stay byte for byte what 0.11 wrote.
        let tmp = std::env::temp_dir().join(format!("vivac-signmain-{}", id::ulid()));
        Store::create(&tmp).unwrap();
        let mut s = Store::open(tmp.clone()).unwrap();
        let lock = s.lock_for_write().unwrap();
        let w = s
            .append(
                &lock,
                vec![crate::event::Body::NodeNoted {
                    node: "t1".into(),
                    note: "x".into(),
                }],
                0,
                false,
            )
            .unwrap();
        assert_eq!(w.events[0].lane, crate::lane::MAIN);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn locking_the_config_for_lanes_is_idempotent_and_atomic() {
        // Same mechanism as `d444`'s own sentence: the config is written to a
        // sibling and renamed, and saying it twice writes once.
        let tmp = std::env::temp_dir().join(format!("vivac-lanelock-{}", id::ulid()));
        let mut s = Store::create(&tmp).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.lock_lanes_in_config(&lock).unwrap();
        assert_eq!(s.config.version, ConfigVersion::Lanes);
        let text = fs::read_to_string(tmp.join(DIR).join(CONFIG)).unwrap();
        assert!(text.contains(LANE_SENTENCE));

        s.lock_lanes_in_config(&lock).unwrap();
        assert_eq!(s.config.version, ConfigVersion::Lanes);
        assert!(
            fs::read_dir(tmp.join(DIR))
                .unwrap()
                .filter_map(|e| e.ok())
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")),
            "a temporary file was left behind"
        );
        fs::remove_dir_all(&tmp).ok();
    }

    /// `t594` fix-1, finding 5: a config that vanishes over a log that
    /// already holds a `lane.declared` must come back locked to the lanes
    /// sentence, the same as `d444` already does for a pillar or a rule --
    /// `log_already_governed`'s blind spot before this test existed.
    #[test]
    fn a_missing_config_regenerates_the_lanes_sentence_when_the_log_has_a_lane_event() {
        let tmp = std::env::temp_dir().join(format!("vivac-relock-{}", id::ulid()));
        let mut s = Store::create(&tmp).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.append(
            &lock,
            vec![crate::event::Body::LaneDeclared {
                lane: crate::lane::MAIN.to_string(),
                name: crate::lane::MAIN.to_string(),
                repos: vec![],
            }],
            0,
            false,
        )
        .unwrap();
        drop(lock);
        fs::remove_file(tmp.join(DIR).join(CONFIG)).unwrap();

        let reopened = Store::open(tmp.clone()).unwrap();
        assert_eq!(reopened.config.version, ConfigVersion::Lanes);
        fs::remove_dir_all(&tmp).ok();
    }
}
