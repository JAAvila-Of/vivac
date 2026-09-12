//! The project registry: `<store_dir>/projects`.
//!
//! One file, one job: remember which projects exist on this machine and
//! where, so a later fan-out (`d232`) does not have to be told by hand. It is
//! not verified data and not a disposable projection either -- `f267` -- the
//! project id and its path live nowhere else, so this file is the one place
//! that answer holds.
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
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Contents {
    version: u32,
    projects: BTreeMap<String, String>,
}

impl Contents {
    fn empty() -> Contents {
        Contents {
            version: VERSION,
            projects: BTreeMap::new(),
        }
    }
}

/// Records that `root` is the project keyed by `project_id`.
///
/// Steady state is two small reads and no write: an absent key is inserted,
/// a key already pointing at `root` writes nothing, and a key pointing
/// somewhere else is updated in place -- `d201`'s "same project, new path",
/// caught without anybody having to say so.
///
/// Never fails. The registry serves a surface that does not exist yet, so a
/// missing or unwritable `store_dir`, or a `projects` file that will not
/// parse, all leave the caller's own result untouched. A file that will not
/// parse is replaced wholesale on the next successful write, not repaired.
pub fn note(store_dir: &Path, project_id: &str, root: &Path) {
    let _ = try_note(store_dir, project_id, root);
}

fn try_note(store_dir: &Path, project_id: &str, root: &Path) -> std::io::Result<()> {
    let path = store_dir.join(FILE);
    let mut contents = read(&path);
    let value = root.to_string_lossy().into_owned();
    if contents.projects.get(project_id) == Some(&value) {
        return Ok(());
    }
    contents.projects.insert(project_id.to_string(), value);
    write(store_dir, &path, &contents)
}

/// Every root the registry currently points at, in no particular order.
/// `find --everywhere` (`d273`) is the first reader that wants the roots
/// themselves rather than the id each one is keyed by, so the map's keys
/// stay inside this module the way `note`'s already do.
pub fn roots(store_dir: &Path) -> Vec<PathBuf> {
    read(&store_dir.join(FILE))
        .projects
        .into_values()
        .map(PathBuf::from)
        .filter(|r| !marks_global_store(&r.join(crate::store::DIR)))
        .collect()
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

fn read(path: &Path) -> Contents {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(Contents::empty)
}

fn write(store_dir: &Path, path: &Path, contents: &Contents) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir)?;
    let mut f = File::create(path)?;
    f.write_all(serde_json::to_string_pretty(contents)?.as_bytes())?;
    f.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{id, store};

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vivac-{prefix}-{}", id::ulid()))
    }

    /// A seeded project with at least one event, so it has a first event id
    /// to be keyed by.
    fn seeded_project(prefix: &str) -> (std::path::PathBuf, String) {
        let root = temp_dir(prefix);
        let mut s = store::Store::create(&root).unwrap();
        s.append(
            vec![crate::event::Body::NodeNoted {
                node: "t1".into(),
                note: "seed".into(),
            }],
            0,
            false,
        )
        .unwrap();
        let id = store::first_event_id(&root).unwrap();
        (root, id)
    }

    #[test]
    fn a_fresh_registry_gets_the_project_inserted() {
        let store_dir = temp_dir("reg");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, &root);

        let text = std::fs::read_to_string(store_dir.join(FILE)).unwrap();
        let contents: Contents = serde_json::from_str(&text).unwrap();
        assert_eq!(
            contents.projects.get(&id),
            Some(&root.to_string_lossy().into_owned())
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn registering_the_same_project_twice_writes_nothing() {
        let store_dir = temp_dir("reg");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, &root);
        let before = std::fs::read(store_dir.join(FILE)).unwrap();
        note(&store_dir, &id, &root);
        let after = std::fs::read(store_dir.join(FILE)).unwrap();

        assert_eq!(
            before, after,
            "the no-write case is the one that protects the budget"
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_moved_project_updates_in_place() {
        let store_dir = temp_dir("reg");
        let (old_root, id) = seeded_project("proj");
        let new_root = temp_dir("proj-moved");

        note(&store_dir, &id, &old_root);
        note(&store_dir, &id, &new_root);

        let text = std::fs::read_to_string(store_dir.join(FILE)).unwrap();
        let contents: Contents = serde_json::from_str(&text).unwrap();
        assert_eq!(contents.projects.len(), 1);
        assert_eq!(
            contents.projects.get(&id),
            Some(&new_root.to_string_lossy().into_owned())
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&old_root).ok();
    }

    #[test]
    fn two_different_projects_both_appear() {
        let store_dir = temp_dir("reg");
        let (root_a, id_a) = seeded_project("proj-a");
        let (root_b, id_b) = seeded_project("proj-b");

        note(&store_dir, &id_a, &root_a);
        note(&store_dir, &id_b, &root_b);

        let text = std::fs::read_to_string(store_dir.join(FILE)).unwrap();
        let contents: Contents = serde_json::from_str(&text).unwrap();
        assert_eq!(contents.projects.len(), 2);
        assert_eq!(
            contents.projects.get(&id_a),
            Some(&root_a.to_string_lossy().into_owned())
        );
        assert_eq!(
            contents.projects.get(&id_b),
            Some(&root_b.to_string_lossy().into_owned())
        );

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

        note(&blocked, &id, &root);

        std::fs::remove_file(&blocked).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_nonexistent_store_directory_never_fails_the_caller() {
        let store_dir = temp_dir("does-not-exist-yet");
        let (root, id) = seeded_project("proj");

        note(&store_dir, &id, &root);
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

        note(&store_dir, &id, &root);

        let text = std::fs::read_to_string(store_dir.join(FILE)).unwrap();
        let contents: Contents = serde_json::from_str(&text).unwrap();
        assert_eq!(
            contents.projects.get(&id),
            Some(&root.to_string_lossy().into_owned())
        );

        std::fs::remove_dir_all(&store_dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
