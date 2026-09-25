//! `.vivac/lane`: which lane a working folder is, and of which tree.
//!
//! One tree per product, one lane per working folder (`d595`). The tree
//! itself lives in exactly one folder, and every other folder that works
//! on it carries this file and nothing else -- except the one folder
//! whose own `main` has been claimed elsewhere (`d597`, §6.9): it still
//! holds the tree, and it still is not `main` any more, so `setup` mints
//! it a lane of its own too, right beside `config` and `events`
//! (`t594`).
//!
//! **It holds no path.** Where the tree lives is the registry's job and the
//! registry's alone (`f267`): a second home for that answer would need its
//! own curation rule, which is `f89`. It holds no repositories and no name
//! either -- those are in the log, where `lane.declared` puts them.

use crate::failure::Failure;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// The name of the file itself. `store::LANE` is the one place that name is
/// written; this reexports it so nobody has to know which module owns it.
pub const FILE: &str = crate::store::LANE;

/// The founding lane of every tree. As opaque as a ULID, and kept as a
/// word only because every event written before lanes existed already
/// says it. It is the id and nothing else: what a reader sees is the
/// lane's name, which `setup` takes from the folder it is, exactly like
/// every other lane's (`d624`). Reading this word off a brief was how
/// people took it for a git branch.
pub const MAIN: &str = "main";

const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Lane {
    pub version: u32,
    pub id: String,
    pub project: String,
}

/// A fresh id for a lane. As opaque as any other id this crate mints.
pub fn new_id() -> String {
    crate::id::ulid()
}

/// What a lane is called when its folder's own name cannot be written
/// down (`d600`): `lane-` and the first four characters of its id. The
/// name is withheld, never the lane -- refusing to join a folder because
/// of what it is called would cost the person their tree over a word.
pub fn name_for(id: &str, _folder_name: &str) -> String {
    format!("lane-{}", &id[..id.len().min(4)])
}

/// `folder_name`, or `name_for`'s fallback once the redaction guard
/// rejects it (`d600`): the folder's own name never reaches the log
/// either way. The one place this rule is written: `setup::plan_lane`
/// and `Ctx::emit`'s own join both name a lane from a folder, and a
/// security rule copied into two places is a rule that can go on
/// agreeing with itself only until someone edits one of them
/// (`t594`).
pub fn declared_name(id: &str, folder_name: &str) -> String {
    match crate::redact::check_field("lane name", folder_name) {
        Some(_) => name_for(id, folder_name),
        None => folder_name.to_string(),
    }
}

/// Reads `vivac_dir/lane`. `Ok(None)` only for the one case that really is
/// an absence: no file at all, the ordinary shape of a folder that holds
/// the tree itself and has never had its own `main` claimed elsewhere.
///
/// Everything else that keeps this from handing back a `Lane` refuses
/// instead of falling back to `None` -- unreadable, not JSON, JSON with the
/// wrong shape. A lane file almost always names a folder that does *not*
/// hold the tree, so reading a corrupt one as absent would feed the
/// resolution that follows the story of a folder with no lane at all;
/// `Store::open` would then seed that folder a config of its own, turning
/// a folder that belongs to another tree into a fresh, empty one,
/// splitting the product in two without telling anybody. The one folder
/// that holds the tree and still carries this file -- `main` claimed
/// elsewhere, `t594` -- already has its own config, so that
/// particular disaster does not reach it; but reading its corrupt file as
/// absent would just as quietly undo the very thing the file exists to
/// say, and land the folder back in front of §6.9's own refusal. The same
/// convention `read_config` already uses in `store.rs`.
///
/// The one refusal with its own sentence is a `version` this release does
/// not know: that shape is understood well enough to say it is not
/// supported, unlike the other two, which are not understood at all.
pub fn read(vivac_dir: &Path) -> Result<Option<Lane>, Failure> {
    let raw = match std::fs::read_to_string(vivac_dir.join(FILE)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    check_lane_version(v.get("version"))?;
    let lane: Lane =
        serde_json::from_value(v).map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    Ok(Some(lane))
}

/// Refuses a `version` this release does not know before the rest of the
/// shape is even looked at, the same order `check_config_version` uses in
/// `store.rs`.
fn check_lane_version(version: Option<&serde_json::Value>) -> Result<(), Failure> {
    if let Some(other) = version.and_then(|v| v.as_u64()) {
        if other != VERSION as u64 {
            return Err(Failure::newer_vivac(format!(
                "This tree was written by a newer vivac: .vivac/lane has version {other}, \
                 which this version does not know. Update vivac to read it. A session or \
                 vivac web opened before an update keeps the old vivac until it restarts. \
                 Nothing was written."
            )));
        }
    }
    Ok(())
}

/// Writes `vivac_dir/lane` whole. `vivac_dir` itself is the part that used
/// to appear in two steps -- `create_dir_all` first, `lane` only after --
/// and a reader resolving that folder in between saw a `.vivac/` with
/// neither a tree nor a lane file: exactly the hollow shape `f719`'s
/// `find_root` refuses over (`f735`). A concurrent auto-join -- the
/// `SessionStart` hook and the first `push` both resolving the same
/// worktree, one of them holding the tree's lock and the other's
/// `find_root` not needing it -- could catch that window.
///
/// `store::build_fresh` closes it: when `vivac_dir` does not exist yet,
/// `lane` and `.gitignore` are written into a sibling temporary directory
/// first, and only that whole directory is ever renamed onto `vivac_dir`.
/// A reader now sees either no `.vivac/` at all, or one that already holds
/// `lane` -- never anything in between. `Store::create` builds a tree the
/// same way, through the same helper, rather than a second copy of these
/// mechanics (`f724`).
///
/// When `vivac_dir` already exists, behaviour is exactly what it always
/// was: `lane` is replaced through its own temporary file and rename, then
/// `.gitignore` is written if it is not there yet. A directory that already
/// exists cannot turn hollow by this call -- it was either already whole,
/// or some earlier write left it in a shape this call does not judge.
pub fn write(vivac_dir: &Path, lane: &Lane) -> std::io::Result<()> {
    let built = crate::store::build_fresh(vivac_dir, |tmp_dir| {
        write_lane_file(tmp_dir, lane)?;
        crate::store::write_gitignore(tmp_dir)
    })?;
    if built {
        crate::store::mark_write();
        return Ok(());
    }
    write_in_place(vivac_dir, lane)
}

/// Today's behaviour, unchanged: `vivac_dir` is created if it is not
/// already there (a no-op in that case), `lane` is replaced through its own
/// temporary file and rename, and `.gitignore` is written if it is missing.
fn write_in_place(vivac_dir: &Path, lane: &Lane) -> std::io::Result<()> {
    std::fs::create_dir_all(vivac_dir)?;
    write_lane_file(vivac_dir, lane)?;
    crate::store::mark_write();
    crate::store::write_gitignore(vivac_dir)
}

/// `vivac_dir/lane`, replaced whole: a temporary sibling *file*, named with
/// a ULID so two writers never collide, then a rename over the real one. A
/// process that dies between the two leaves the old file exactly as it was.
fn write_lane_file(vivac_dir: &Path, lane: &Lane) -> std::io::Result<()> {
    let tmp = vivac_dir.join(format!("{FILE}.{}.tmp", crate::id::ulid()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(lane)?.as_bytes())?;
        f.write_all(b"\n")?;
    }
    std::fs::rename(&tmp, vivac_dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vivac(prefix: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("vivac-{prefix}-{}", crate::id::ulid()));
        let dir = root.join(crate::store::DIR);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_lane_file_round_trips() {
        let dir = temp_vivac("rt");
        let l = Lane {
            version: 1,
            id: "01M2AAAAAAAAAAAAAAAAAAAAAA".into(),
            project: "01M1BBBBBBBBBBBBBBBBBBBBBB".into(),
        };
        write(&dir, &l).unwrap();
        let back = read(&dir).unwrap().unwrap();
        assert_eq!(back.id, l.id);
        assert_eq!(back.project, l.project);
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn no_lane_file_is_not_a_failure() {
        let dir = temp_vivac("absent");
        assert!(read(&dir).unwrap().is_none());
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn a_version_this_release_does_not_know_refuses() {
        let dir = temp_vivac("newer");
        std::fs::write(
            dir.join(FILE),
            br#"{"version":2,"id":"01M2","project":"01M1"}"#,
        )
        .unwrap();
        let e = read(&dir).unwrap_err();
        assert_eq!(e.code(), 5);
        assert!(e.message().contains("newer vivac"), "{}", e.message());
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn the_lane_file_is_replaced_whole_and_never_half_written() {
        // Same rule as `config`: a temporary sibling and a rename, so a
        // process that dies between the two leaves the old file intact.
        let dir = temp_vivac("atomic");
        let first = Lane {
            version: 1,
            id: "01A".into(),
            project: "01P".into(),
        };
        write(&dir, &first).unwrap();
        let second = Lane {
            version: 1,
            id: "01B".into(),
            project: "01P".into(),
        };
        write(&dir, &second).unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().id, "01B");
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")),
            "a temporary file was left behind"
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn writing_a_lane_keeps_the_folder_out_of_git() {
        // A lane folder holds no tree, so nothing else would have written the
        // ignore file for it, and a clone would carry the lane file along.
        let dir = temp_vivac("ignored");
        write(
            &dir,
            &Lane {
                version: 1,
                id: "01A".into(),
                project: "01P".into(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(crate::store::GITIGNORE)).unwrap(),
            "*\n"
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn the_lane_file_holds_three_fields_and_no_room_for_a_path() {
        // `f267`: where a tree lives is the registry's answer and only its
        // answer. The guarantee is the shape -- there is no field a path could
        // be put in -- and not the spelling of what happens to be written
        // today, so this pins the keys rather than their values.
        let dir = temp_vivac("shape");
        write(
            &dir,
            &Lane {
                version: 1,
                id: "01A".into(),
                project: "01P".into(),
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.join(crate::store::LANE)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["id", "project", "version"]);
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn a_lane_file_that_is_not_json_refuses() {
        // A hand-edited or half-written file has to fail loudly: reading it
        // as absent would let the resolution that follows treat this folder
        // as one with no lane, and seed it a tree of its own.
        let dir = temp_vivac("notjson");
        std::fs::write(dir.join(FILE), b"not json at all").unwrap();
        let e = read(&dir).unwrap_err();
        assert_ne!(
            e.code(),
            4,
            "a corrupt lane file was read as though there were no tree here: {}",
            e.message()
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn a_lane_file_with_the_wrong_shape_refuses() {
        // Well-formed JSON that is not a `Lane` is exactly as unreadable as
        // garbage that is not JSON at all: both name a file this release
        // cannot make sense of, not a folder with no lane.
        let dir = temp_vivac("wrongshape");
        std::fs::write(dir.join(FILE), br#"{"version":1}"#).unwrap();
        let e = read(&dir).unwrap_err();
        assert_ne!(
            e.code(),
            4,
            "a corrupt lane file was read as though there were no tree here: {}",
            e.message()
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    /// `f735`: `write` used to `create_dir_all(vivac_dir)` and only
    /// *afterwards* write `lane` and `.gitignore`, so a reader resolving
    /// this same folder in between -- `store::find_root`, `f719` -- could
    /// see a `.vivac/` that held neither a tree nor a lane, and refuse.
    /// The window is a handful of syscalls wide, so this spins a reader in
    /// a tight loop against a writer, round after round, each round
    /// starting from a `vivac_dir` that does not exist yet at all -- the
    /// shape a fresh worktree's auto-join races on (`ops.rs`, the
    /// `SessionStart` hook against the first `push`, `t594`). Against the
    /// pre-fix `write`, this caught the hollow shape on the very first
    /// round; 500 leaves ample margin. `thread::scope`, the same idiom
    /// `registry.rs`'s own race test already uses, needs no `Arc`: the
    /// reader only ever borrows `dir` and `stop`, and both outlive it.
    #[test]
    fn a_reader_racing_a_fresh_write_never_sees_vivac_dir_hollow() {
        const ROUNDS: usize = 500;
        for round in 0..ROUNDS {
            let root = std::env::temp_dir().join(format!("vivac-race-{}", crate::id::ulid()));
            let dir = root.join(crate::store::DIR);
            let hollow_seen = std::sync::atomic::AtomicBool::new(false);
            let stop = std::sync::atomic::AtomicBool::new(false);
            std::thread::scope(|s| {
                s.spawn(|| {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        if dir.is_dir() && !dir.join(FILE).is_file() {
                            hollow_seen.store(true, std::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                    }
                });
                let lane = Lane {
                    version: 1,
                    id: "01A".into(),
                    project: "01P".into(),
                };
                write(&dir, &lane).unwrap();
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            });
            std::fs::remove_dir_all(&root).ok();
            assert!(
                !hollow_seen.load(std::sync::atomic::Ordering::Relaxed),
                "a reader observed a hollow .vivac/ on round {round} of {ROUNDS}"
            );
        }
    }

    #[test]
    fn a_fresh_vivac_dir_holds_lane_and_gitignore_with_no_temp_sibling_left_behind() {
        // The whole point of building `vivac_dir` in a sibling first: once
        // `write` returns, `root` holds nothing but the finished `.vivac/`
        // -- no `.tmp`-suffixed directory left lying beside it.
        let root = std::env::temp_dir().join(format!("vivac-fresh-{}", crate::id::ulid()));
        let dir = root.join(crate::store::DIR);
        assert!(
            !dir.exists(),
            "the fixture must start with no .vivac/ at all"
        );
        let l = Lane {
            version: 1,
            id: "01A".into(),
            project: "01P".into(),
        };
        write(&dir, &l).unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().id, "01A");
        assert_eq!(
            std::fs::read_to_string(dir.join(crate::store::GITIGNORE)).unwrap(),
            "*\n"
        );
        assert_eq!(
            only_entry_of(&root),
            crate::store::DIR,
            "a temporary sibling directory was left behind"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn writing_into_an_existing_vivac_dir_keeps_writing_in_place() {
        // `vivac_dir` already there: the fresh-sibling machinery never
        // engages at all, so this is `write_in_place` alone -- no sibling
        // directory appears next to `vivac_dir` either.
        let dir = temp_vivac("existing");
        let l = Lane {
            version: 1,
            id: "01A".into(),
            project: "01P".into(),
        };
        write(&dir, &l).unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().id, "01A");
        let root = dir.parent().unwrap();
        assert_eq!(only_entry_of(root), crate::store::DIR);
        std::fs::remove_dir_all(root).ok();
    }

    // `write`'s own fallback -- `build_fresh` backing off when `vivac_dir`
    // already exists, rather than a second copy of that mechanic living
    // here -- is tested once, in `store.rs`'s own tests, where the helper
    // itself lives (`f724`).

    /// The single file or directory name inside `dir`, panicking unless
    /// there is exactly one -- the shape every test above expects once a
    /// write has finished and cleaned up after itself.
    fn only_entry_of(dir: &Path) -> String {
        let entries: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one entry, found {entries:?}"
        );
        entries.into_iter().next().unwrap()
    }
}
