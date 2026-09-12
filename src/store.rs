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
    if let Some(h) = non_blank(home) {
        return Some(PathBuf::from(h).join(DIR));
    }
    if let Some(u) = non_blank(userprofile) {
        return Some(PathBuf::from(u).join(DIR));
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

/// `config`'s `version`, once it is known to be one of the two shapes this
/// release can act on. `d444`: a tree that gains its first pillar or rule
/// turns this from `One` to `Locked`, in place, before the event that
/// creates it is appended -- and a release earlier than that fails to parse
/// `Locked`'s own sentence, which is the whole point.
///
/// No `#[derive(Serialize, Deserialize)]`: neither shape is an enum tag in
/// the usual sense, one is the bare integer `1` and the other is a string,
/// and `check_config_version` -- not this type -- is what tells a genuinely
/// unknown version apart from one of these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigVersion {
    One,
    Locked,
}

/// The sentence a config's `version` becomes the moment its tree gains a
/// pillar or a rule. Literal and without a number: release-plz decides the
/// number when it publishes, and every release from here on reads the
/// sentence exactly as it reads `1`. `d444`.
pub const LOCK_SENTENCE: &str =
    "this tree holds pillars and rules, and this vivac is too old to read them: update vivac";

impl Serialize for ConfigVersion {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ConfigVersion::One => s.serialize_u32(1),
            ConfigVersion::Locked => s.serialize_str(LOCK_SENTENCE),
        }
    }
}

impl<'de> Deserialize<'de> for ConfigVersion {
    /// Only ever reached once `check_config_version` has already let the raw
    /// value through: a `1`, or the lock sentence. Anything else refuses
    /// generically here, which is `d444`'s "como hoy" for a version this
    /// deserializer was never meant to explain -- negative, a float, an
    /// object, `null`.
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        match &v {
            serde_json::Value::Number(n) if n.as_u64() == Some(1) => Ok(ConfigVersion::One),
            serde_json::Value::String(s) if s == LOCK_SENTENCE => Ok(ConfigVersion::Locked),
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
                // already carries a pillar or a rule, the regenerated config is
                // born locked -- deleting one file must never hand an older
                // release a config that looks readable over a tree it is not.
                let c = if log_already_governed(&root) {
                    Config {
                        version: ConfigVersion::Locked,
                        ..Config::new_seeded()
                    }
                } else {
                    Config::new_seeded()
                };
                write_config(&root, &c)?;
                c
            }
        };
        Ok(Store { root, config })
    }

    pub fn create(root: &Path) -> std::io::Result<Store> {
        let d = root.join(DIR);
        fs::create_dir_all(&d)?;
        let config = Config::new_seeded();
        write_config(root, &config)?;
        if !d.join(LOG).exists() {
            File::create(d.join(LOG))?;
        }
        Ok(Store {
            root: root.to_path_buf(),
            config,
        })
    }

    pub fn log(&self) -> PathBuf {
        self.root.join(DIR).join(LOG)
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join(DIR).join(INDEX)
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
        Some(serde_json::Value::String(s)) => Err(Failure::newer_vivac(format!(
            "This tree was written by a newer vivac: its config says {s:?}. Update vivac \
             to read it. Nothing was written."
        ))),
        _ => Ok(()),
    }
}

/// The rare path `Store::open` takes when `config` itself is missing:
/// whether the log already holds a pillar or a rule, in any state. Reads
/// the whole log -- something no ordinary read ever pays for -- because a
/// vanished config is itself the unusual case, and regenerating one that
/// looks readable by any release over a tree that already governs
/// something would undo the very lock `d444` exists to keep.
fn log_already_governed(root: &Path) -> bool {
    let Ok(f) = File::open(root.join(DIR).join(LOG)) else {
        return false;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["payload"]["type"] == "node.created"
            && matches!(v["payload"]["kind"].as_str(), Some("pillar") | Some("rule"))
        {
            return true;
        }
    }
    false
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
    /// single-line write atomic and removes the need for a lock.
    ///
    /// `tree_already_governed` is `d444`'s own check, paid before any of
    /// `body` reaches disk: the config locks in place, first, so a process
    /// that dies between the two leaves an unlocked config over a tree with
    /// no pillar and no rule, which is harmless.
    pub fn append(
        &mut self,
        body: Vec<crate::event::Body>,
        from_seq: u64,
        tree_already_governed: bool,
    ) -> std::io::Result<()> {
        self.lock_if_needed(&body, tree_already_governed)?;
        let mut buf = String::with_capacity(256 * body.len());
        for (i, c) in body.into_iter().enumerate() {
            let e = crate::event::Event {
                seq: from_seq + i as u64 + 1,
                id: id::ulid(),
                ts: clock::now_rfc3339(),
                actor: self.config.actor.clone(),
                lane: "main".into(),
                payload: c,
            };
            buf.push_str(&serde_json::to_string(&e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
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
    let mut events = Vec::new();
    let mut broken = 0usize;
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(e) => events.push(e),
            Err(_) => match crate::event::unknown_reason_for(&line) {
                Some(reason) => return Err(newer_vivac_failure(i + 1, reason)),
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
    /// otherwise the migration flattens the only timeline it had.
    pub fn write_raw(&self, events: &[crate::event::Event]) -> std::io::Result<()> {
        let mut buf = String::with_capacity(256 * events.len());
        for e in events {
            buf.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
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
        // A log large enough that folding the whole thing would be visible
        // in the timing, if this ever regressed into calling `read_all`.
        for _ in 0..500 {
            s.append(
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
}
