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
    pub id: String,
    ctx: ops::Ctx,
    seen: (u64, Option<SystemTime>),
}

impl Project {
    pub fn open(root: PathBuf, id: String) -> Result<Project, Failure> {
        let ctx = ops::Ctx::load(store::Store::open(root.clone())?)?;
        let seen = fingerprint(&ctx.store.log());
        Ok(Project {
            root,
            id,
            ctx,
            seen,
        })
    }

    /// The tree as it is on disk right now: re-folds when the log moved.
    pub fn current(&mut self) -> Result<&ops::Ctx, Failure> {
        let now = fingerprint(&self.ctx.store.log());
        if now != self.seen {
            self.ctx = ops::Ctx::load(store::Store::open(self.root.clone())?)?;
            self.seen = now;
        }
        Ok(&self.ctx)
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
        let ids = assign_ids(&unique);
        let mut projects = Vec::with_capacity(unique.len());
        for (root, id) in unique.into_iter().zip(ids) {
            projects.push(Project::open(root, id)?);
        }
        Ok(Registry { projects })
    }

    /// The first root the process was given. `open` refuses an empty list, so
    /// there is always one.
    pub fn first(&mut self) -> &mut Project {
        &mut self.projects[0]
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

/// The name each root goes by. Its own function because the rule is the whole
/// point of the registry, and because it is what the tests aim at.
fn assign_ids(roots: &[PathBuf]) -> Vec<String> {
    let bare: Vec<String> = roots
        .iter()
        .map(|r| {
            r.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".into())
        })
        .collect();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ids = Vec::with_capacity(bare.len());
    for name in &bare {
        let id = if used.contains(name) {
            let mut suffix = 2;
            loop {
                let candidate = format!("{name}-{suffix}");
                if !used.contains(&candidate) {
                    break candidate;
                }
                suffix += 1;
            }
        } else {
            name.clone()
        };
        used.insert(id.clone());
        ids.push(id);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_root_gets_its_bare_directory_name() {
        let ids = assign_ids(&[PathBuf::from("/work/vivac")]);
        assert_eq!(ids, vec!["vivac".to_string()]);
    }

    #[test]
    fn two_roots_with_the_same_directory_name_get_a_number() {
        let ids = assign_ids(&[PathBuf::from("/a/vivac"), PathBuf::from("/b/vivac")]);
        assert_eq!(ids, vec!["vivac".to_string(), "vivac-2".to_string()]);
    }

    #[test]
    fn three_roots_with_the_same_directory_name_count_up() {
        let ids = assign_ids(&[
            PathBuf::from("/a/vivac"),
            PathBuf::from("/b/vivac"),
            PathBuf::from("/c/vivac"),
        ]);
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
        let ids = assign_ids(&[PathBuf::from("/")]);
        assert_eq!(ids, vec!["-".to_string()]);
    }

    #[test]
    fn a_suffix_that_is_already_taken_is_skipped() {
        let ids = assign_ids(&[
            PathBuf::from("/x/a"),
            PathBuf::from("/y/a"),
            PathBuf::from("/z/a-2"),
        ]);
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
