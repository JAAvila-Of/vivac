//! One project's tree, held by a process that outlives its calls.
//!
//! Every other command is one command, one process, one fold: it reads the log,
//! folds it, does its work and exits. A server does not get that. It answers many
//! calls out of one fold, and it is not the only writer -- the agent keeps working
//! through the CLI while the server is up. So the tree it hands out has to be the
//! tree on disk, not the one it read when it started.
//!
//! The check is the log's length and its mtime. It costs one `stat` per call and it
//! catches every append, because appending is the only way the log ever changes. The
//! length alone would carry that: the log only grows, so a different length is an
//! exact change detector. The modification time rides along for the one case a length
//! cannot see, which is a rewrite that lands on the same byte count.
//!
//! This began inside the MCP server, the first thing here to outlive its own calls.
//! The web is the second, and it needs the same thing over more than one root, so it
//! moved out here.

use crate::event::Event;
use crate::failure::Failure;
use crate::{ops, store};
use std::path::PathBuf;
use std::time::SystemTime;

fn fingerprint(log: &std::path::Path) -> (u64, Option<SystemTime>) {
    match std::fs::metadata(log) {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    }
}

/// One root, folded, with enough of a fingerprint to know when it moved.
pub struct Project {
    pub root: PathBuf,
    /// What goes in a URL: the directory's name with every run of characters
    /// outside `A-Za-z0-9._-` collapsed to a single `-`. Unique across the
    /// registry, because a link that has been saved is permanent.
    pub id: String,
    /// The directory's name as it is on disk, which is what a page shows.
    pub name: String,
    ctx: ops::Ctx,
    /// The events the fold was built from, kept because a reader that groups
    /// the log by what happened -- `changes`, and the Today page that shows
    /// the same stretch -- needs them and the fold does not carry them.
    log: Vec<Event>,
    seen: (u64, Option<SystemTime>),
}

impl Project {
    pub fn open(root: PathBuf, name: String, id: String) -> Result<Project, Failure> {
        let (ctx, log) = ops::Ctx::load_with_log(store::Store::open(root.clone())?)?;
        let seen = fingerprint(&ctx.store.log());
        Ok(Project {
            root,
            id,
            name,
            ctx,
            log,
            seen,
        })
    }

    /// The tree as it is on disk right now: re-folds when the log moved.
    pub fn current(&mut self) -> Result<&ops::Ctx, Failure> {
        self.current_with_log().map(|(c, _)| c)
    }

    /// The same refresh, handing back the events as well. The two travel
    /// together on purpose: a page built from this fold and that log has to
    /// be built from the same read, or it can show a stretch the tree beside
    /// it does not agree with.
    pub fn current_with_log(&mut self) -> Result<(&ops::Ctx, &[Event]), Failure> {
        let now = fingerprint(&self.ctx.store.log());
        if now != self.seen {
            let (ctx, log) = ops::Ctx::load_with_log(store::Store::open(self.root.clone())?)?;
            self.ctx = ctx;
            self.log = log;
            self.seen = now;
        }
        Ok((&self.ctx, &self.log))
    }
}

/// Every root the process was asked to serve, each folded once.
pub struct Registry {
    projects: Vec<Project>,
}

impl Registry {
    pub fn open(roots: Vec<PathBuf>) -> Result<Registry, Failure> {
        if roots.is_empty() {
            return Err(Failure::usage(
                "vivac needs at least one root to serve.".to_string(),
            ));
        }
        let unique = dedup_by_target(roots);
        let pairs = assign_names_and_ids(&unique);
        let mut projects = Vec::with_capacity(unique.len());
        for (root, (name, id)) in unique.into_iter().zip(pairs) {
            projects.push(Project::open(root, name, id)?);
        }
        Ok(Registry { projects })
    }

    /// The first root the process was given. `open` refuses an empty list, so
    /// there is always one.
    pub fn first(&mut self) -> &mut Project {
        &mut self.projects[0]
    }

    /// The project a URL's `id` names, if the registry has one. The `id` is
    /// compared against what the registry already holds and never handed to
    /// the filesystem, which is what makes a `..` in a path uninteresting.
    pub fn by_id(&mut self, id: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.id == id)
    }

    /// Every project in the registry, in the order the roots were given. The
    /// index page pairs each `id` with its `name` from here.
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }
}

/// Collapses roots that point at the same place, keeping the first spelling
/// they arrived with. Two entries are the same place when `canonicalize`
/// agrees on both, and a root `canonicalize` cannot resolve is compared as
/// written instead of dropped.
fn dedup_by_target(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut keys: Vec<PathBuf> = Vec::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let key = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if keys.contains(&key) {
            continue;
        }
        keys.push(key);
        out.push(root);
    }
    out
}

/// The URL-safe form of a directory's bare name: every run of characters
/// outside `A-Za-z0-9._-` collapses to a single `-`.
fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

/// The name and the id each root goes by. Its own function because the rule
/// is the whole point of the registry, and because it is what the tests aim
/// at.
///
/// Collisions are resolved on the `id`, never on the `name`: the `id` is the
/// only one of the two that has to be unique, because it is the one that
/// goes in a URL.
fn assign_names_and_ids(roots: &[PathBuf]) -> Vec<(String, String)> {
    let names: Vec<String> = roots
        .iter()
        .map(|r| {
            r.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".into())
        })
        .collect();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut pairs = Vec::with_capacity(names.len());
    for name in &names {
        let bare = sanitize(name);
        let id = if used.contains(&bare) {
            let mut suffix = 2;
            loop {
                let candidate = format!("{bare}-{suffix}");
                if !used.contains(&candidate) {
                    break candidate;
                }
                suffix += 1;
            }
        } else {
            bare
        };
        used.insert(id.clone());
        pairs.push((name.clone(), id));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_of(pairs: Vec<(String, String)>) -> Vec<String> {
        pairs.into_iter().map(|(_, id)| id).collect()
    }

    #[test]
    fn a_single_root_gets_its_bare_directory_name() {
        let ids = ids_of(assign_names_and_ids(&[PathBuf::from("/work/vivac")]));
        assert_eq!(ids, vec!["vivac".to_string()]);
    }

    #[test]
    fn two_roots_with_the_same_directory_name_get_a_number() {
        let ids = ids_of(assign_names_and_ids(&[
            PathBuf::from("/a/vivac"),
            PathBuf::from("/b/vivac"),
        ]));
        assert_eq!(ids, vec!["vivac".to_string(), "vivac-2".to_string()]);
    }

    #[test]
    fn three_roots_with_the_same_directory_name_count_up() {
        let ids = ids_of(assign_names_and_ids(&[
            PathBuf::from("/a/vivac"),
            PathBuf::from("/b/vivac"),
            PathBuf::from("/c/vivac"),
        ]));
        assert_eq!(
            ids,
            vec![
                "vivac".to_string(),
                "vivac-2".to_string(),
                "vivac-3".to_string(),
            ]
        );
    }

    #[test]
    fn a_root_with_no_directory_name_falls_back_to_a_dash() {
        let ids = ids_of(assign_names_and_ids(&[PathBuf::from("/")]));
        assert_eq!(ids, vec!["-".to_string()]);
    }

    #[test]
    fn a_suffix_that_is_already_taken_is_skipped() {
        let ids = ids_of(assign_names_and_ids(&[
            PathBuf::from("/x/a"),
            PathBuf::from("/y/a"),
            PathBuf::from("/z/a-2"),
        ]));
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "a duplicate id slipped through: {ids:?}"
        );
    }

    #[test]
    fn a_name_with_characters_a_url_cannot_carry_becomes_an_id_that_can() {
        let pairs = assign_names_and_ids(&[PathBuf::from("/work/my repo#1")]);
        assert_eq!(
            pairs,
            vec![("my repo#1".to_string(), "my-repo-1".to_string())]
        );
    }

    #[test]
    fn collisions_are_resolved_on_the_id_and_the_names_are_left_alone() {
        let pairs =
            assign_names_and_ids(&[PathBuf::from("/a/my repo"), PathBuf::from("/b/my-repo")]);
        assert_eq!(
            pairs,
            vec![
                ("my repo".to_string(), "my-repo".to_string()),
                ("my-repo".to_string(), "my-repo-2".to_string()),
            ]
        );
    }

    #[test]
    fn a_name_with_nothing_a_url_can_carry_still_gets_an_id() {
        let pairs = assign_names_and_ids(&[PathBuf::from("/work/###")]);
        assert!(!pairs[0].1.is_empty());
    }

    #[test]
    fn the_same_root_given_twice_is_one_project() {
        let tmp = std::env::temp_dir().join(format!("vivac-project-t-{}", crate::id::ulid()));
        std::fs::create_dir_all(&tmp).unwrap();
        store::Store::create(&tmp).unwrap();
        let want = tmp.file_name().unwrap().to_string_lossy().into_owned();
        let mut registry = Registry::open(vec![tmp.clone(), tmp.clone()])
            .unwrap_or_else(|e| panic!("{}", e.message()));
        assert_eq!(registry.first().id, want);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
