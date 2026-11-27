//! The folder a session runs in, fixed the first time vivac receives a hook
//! from it (`d4146`).
//!
//! A harness runs a session's hooks from wherever the agent's shell is
//! standing. Claude Code keeps the shell's directory across tool calls, so
//! an agent that `cd`s into a linked worktree halfway through has its
//! compact `SessionStart`, its `UserPromptSubmit` and its `Stop` run from
//! that worktree. Resolving each of them from there moved the session to
//! another lane, or joined the worktree as a new one, and the brief that
//! came back after a compact had no focus (`f4143`). The session is the
//! lane's, and the lane is the folder it opened in (`docs/LANES.md`), so
//! every hook of one session answers for that folder.
//!
//! The folder is kept under the store directory, the one the registry
//! lives in, and never under the temporary directory: on Linux that one is
//! shared between accounts, and a file there could be read or replaced by
//! another of them. The file's name is a digest, so the session identifier
//! never reaches a filename.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

const DIR: &str = "sessions";

/// How long a kept folder outlives its last hook. A session picked up again
/// after this is a fresh start and fixes its folder again.
const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a hook says about itself, read off its payload.
pub(super) struct Hook<'a> {
    pub event: Option<&'a str>,
    pub source: Option<&'a str>,
    pub session: Option<&'a str>,
}

impl Hook<'_> {
    /// Whether this hook runs from the folder the session started in, so
    /// that folder can be kept. Claude Code starts, picks up again and clears a
    /// session from its own folder; a compact runs from wherever the shell
    /// is, and so does every other hook.
    fn is_fresh_start(&self) -> bool {
        self.event == Some("SessionStart")
            && matches!(self.source, Some("startup" | "resume" | "clear"))
    }
}

/// The folder `hook` answers for: the one kept for its session; otherwise
/// `project_dir`, the folder Claude Code says the session started in; and
/// only then `cwd`. Keeps the answer for the rest of the session when it
/// can be trusted to be the session's own: `project_dir` was given, or this
/// is a fresh start -- and `holds_a_tree` says there is a tree to answer
/// for, so a session outside every tree leaves nothing behind. Best effort:
/// a store that cannot be read or written only costs the session its kept
/// folder, never the hook.
pub(super) fn resolve(
    cwd: PathBuf,
    hook: &Hook<'_>,
    project_dir: Option<&std::ffi::OsStr>,
    store: Option<&Path>,
    holds_a_tree: impl Fn(&Path) -> bool,
) -> PathBuf {
    let project_dir = project_dir
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir());
    let (Some(session), Some(store)) = (hook.session, store) else {
        return project_dir.unwrap_or(cwd);
    };
    let path = kept_path(store, session);
    if let Some(kept) = read_kept(&path) {
        refresh(&path);
        return kept;
    }
    let trusted = project_dir.is_some() || hook.is_fresh_start();
    let folder = project_dir.unwrap_or(cwd);
    if trusted && holds_a_tree(&folder) {
        keep(store, &path, &folder);
    }
    folder
}

fn kept_path(store: &Path, session: &str) -> PathBuf {
    let digest = Sha256::digest(format!("vivac.session-folder.v1:{session}").as_bytes());
    store.join(DIR).join(format!("{digest:x}"))
}

/// The kept folder, if there is one and it is still a folder.
fn read_kept(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let folder = PathBuf::from(text.strip_suffix('\n')?);
    (folder.is_absolute() && folder.is_dir()).then_some(folder)
}

fn refresh(path: &Path) {
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        file.set_modified(SystemTime::now()).ok();
    }
}

/// Writes `folder` to `path` whole, through a sibling temporary file and a
/// rename, and drops folders nobody has used for [`KEEP_FOR`]. A folder
/// whose name is not UTF-8 is not kept: the hook still answers for it.
fn keep(store: &Path, path: &Path, folder: &Path) {
    let Some(text) = folder.to_str() else {
        return;
    };
    let dir = store.join(DIR);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    prune(&dir, path);
    let tmp = dir.join(format!(".tmp-{}", crate::lane::new_id()));
    if std::fs::write(&tmp, format!("{text}\n")).is_ok() && std::fs::rename(&tmp, path).is_err() {
        std::fs::remove_file(&tmp).ok();
    }
}

fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age > KEEP_FOR);
        if stale && entry.file_type().is_ok_and(|t| t.is_file()) {
            std::fs::remove_file(&path).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vivac-session-folder-{name}-{}",
            crate::lane::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hook<'a>(event: &'a str, source: Option<&'a str>, session: &'a str) -> Hook<'a> {
        Hook {
            event: Some(event),
            source,
            session: Some(session),
        }
    }

    #[test]
    fn a_fresh_start_keeps_its_folder_for_the_sessions_later_hooks() {
        let (store, start, moved) = (temp("store"), temp("start"), temp("moved"));
        let opened = resolve(
            start.clone(),
            &hook("SessionStart", Some("startup"), "s"),
            None,
            Some(&store),
            |_| true,
        );
        assert_eq!(opened, start);
        let compact = resolve(
            moved.clone(),
            &hook("SessionStart", Some("compact"), "s"),
            None,
            Some(&store),
            |_| true,
        );
        assert_eq!(compact, start);
        let stop = resolve(
            moved.clone(),
            &hook("Stop", None, "s"),
            None,
            Some(&store),
            |_| true,
        );
        assert_eq!(stop, start);
        let other = resolve(
            moved.clone(),
            &hook("SessionStart", Some("compact"), "t"),
            None,
            Some(&store),
            |_| true,
        );
        assert_eq!(other, moved, "another session borrowed this one's folder");
        for d in [store, start, moved] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn the_kept_file_is_named_by_a_digest_and_holds_only_the_folder() {
        let (store, start) = (temp("store"), temp("start"));
        resolve(
            start.clone(),
            &hook("SessionStart", Some("startup"), "secret-session-id"),
            None,
            Some(&store),
            |_| true,
        );
        let names: Vec<String> = std::fs::read_dir(store.join(DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(!names[0].contains("secret"), "{names:?}");
        assert_eq!(names[0].len(), 64, "{names:?}");
        let text = std::fs::read_to_string(store.join(DIR).join(&names[0])).unwrap();
        assert_eq!(text, format!("{}\n", start.to_str().unwrap()));
        for d in [store, start] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn a_kept_folder_that_is_gone_gives_way_to_the_project_folder() {
        let (store, start, project, moved) =
            (temp("store"), temp("start"), temp("project"), temp("moved"));
        resolve(
            start.clone(),
            &hook("SessionStart", Some("startup"), "s"),
            None,
            Some(&store),
            |_| true,
        );
        std::fs::remove_dir_all(&start).unwrap();
        let answer = resolve(
            moved.clone(),
            &hook("Stop", None, "s"),
            Some(project.as_os_str()),
            Some(&store),
            |_| true,
        );
        assert_eq!(answer, project);
        let again = resolve(
            moved.clone(),
            &hook("Stop", None, "s"),
            None,
            Some(&store),
            |_| true,
        );
        assert_eq!(
            again, project,
            "the project folder was not kept in its place"
        );
        for d in [store, project, moved] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn without_a_session_or_a_store_nothing_is_kept() {
        let (store, cwd, project) = (temp("store"), temp("cwd"), temp("project"));
        let anonymous = Hook {
            event: Some("SessionStart"),
            source: Some("startup"),
            session: None,
        };
        assert_eq!(
            resolve(cwd.clone(), &anonymous, None, Some(&store), |_| true),
            cwd
        );
        assert!(!store.join(DIR).exists());
        let h = hook("SessionStart", Some("compact"), "s");
        assert_eq!(
            resolve(cwd.clone(), &h, Some(project.as_os_str()), None, |_| true),
            project
        );
        assert_eq!(resolve(cwd.clone(), &h, None, None, |_| true), cwd);
        for d in [store, cwd, project] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn a_project_folder_that_is_not_one_is_ignored() {
        let (store, cwd) = (temp("store"), temp("cwd"));
        let h = hook("Stop", None, "s");
        let relative = std::ffi::OsStr::new("relative/dir");
        assert_eq!(
            resolve(cwd.clone(), &h, Some(relative), Some(&store), |_| true),
            cwd
        );
        let missing = cwd.join("missing");
        assert_eq!(
            resolve(
                cwd.clone(),
                &h,
                Some(missing.as_os_str()),
                Some(&store),
                |_| true
            ),
            cwd
        );
        assert!(
            !store.join(DIR).exists(),
            "a hook that is not a fresh start kept the folder it was run from"
        );
        for d in [store, cwd] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn a_session_outside_every_tree_keeps_nothing() {
        let (store, start) = (temp("store"), temp("start"));
        let answer = resolve(
            start.clone(),
            &hook("SessionStart", Some("startup"), "s"),
            None,
            Some(&store),
            |_| false,
        );
        assert_eq!(answer, start);
        assert!(!store.join(DIR).exists(), "a folder with no tree was kept");
        for d in [store, start] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn folders_unused_for_a_month_are_dropped_when_another_is_kept() {
        let (store, start) = (temp("store"), temp("start"));
        let dir = store.join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old");
        std::fs::write(&old, "x\n").unwrap();
        let month_ago = SystemTime::now() - KEEP_FOR - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(month_ago)
            .unwrap();
        let fresh = dir.join("fresh");
        std::fs::write(&fresh, "x\n").unwrap();
        resolve(
            start.clone(),
            &hook("SessionStart", Some("startup"), "s"),
            None,
            Some(&store),
            |_| true,
        );
        assert!(!old.exists(), "a month-old folder was kept");
        assert!(fresh.exists(), "a fresh folder was dropped");
        for d in [store, start] {
            std::fs::remove_dir_all(d).ok();
        }
    }
}
