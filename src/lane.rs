//! `.vivac/lane`: which lane a working folder is, and of which tree.
//!
//! One tree per product, one lane per working folder (`d595`). The tree
//! itself lives in exactly one folder; every other folder that works on it
//! carries this file and nothing else.
//!
//! **It holds no path.** Where the tree lives is the registry's job and the
//! registry's alone (`f267`): a second home for that answer would need its
//! own curation rule, which is `f89`. It holds no repositories and no name
//! either -- those are in the log, where `lane.declared` puts them.

// Nothing reads a lane file yet: resolving a working folder's lane is the
// commit after this one (`t594` §2.3), and that is where this comes off.
// Kept scoped to this module and to one commit rather than left to make the
// clippy gate red, which every commit of this branch has to pass.
#![allow(dead_code)]

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
/// says it.
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

/// Reads `vivac_dir/lane`. `Ok(None)` only for the one case that really is
/// an absence: no file at all, the ordinary shape of a folder that holds
/// the tree itself.
///
/// Everything else that keeps this from handing back a `Lane` refuses
/// instead of falling back to `None` -- unreadable, not JSON, JSON with the
/// wrong shape. A lane file exists only in a folder that does *not* hold
/// the tree, so reading a corrupt one as absent would feed the resolution
/// that follows the story of a folder with no lane at all; `Store::open`
/// would then seed that folder a config of its own, turning a folder that
/// belongs to another tree into a fresh, empty one, splitting the product
/// in two without telling anybody. The same convention `read_config`
/// already uses in `store.rs`.
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
                 which this version does not know. Update vivac to read it. Nothing was \
                 written."
            )));
        }
    }
    Ok(())
}

/// Writes `vivac_dir/lane` whole: a temporary sibling, named with a ULID so
/// two writers never collide, then a rename over the real file. A process
/// that dies between the two leaves the old file exactly as it was.
///
/// Also writes `vivac_dir`'s own `.gitignore`: a lane folder holds no tree,
/// so nothing else would ever have written it.
pub fn write(vivac_dir: &Path, lane: &Lane) -> std::io::Result<()> {
    std::fs::create_dir_all(vivac_dir)?;
    let tmp = vivac_dir.join(format!("{FILE}.{}.tmp", crate::id::ulid()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(lane)?.as_bytes())?;
        f.write_all(b"\n")?;
    }
    std::fs::rename(&tmp, vivac_dir.join(FILE))?;
    crate::store::write_gitignore(vivac_dir)
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
}
