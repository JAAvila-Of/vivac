//! Which repositories a folder holds: `t594` §4.5.1's own walk, two levels
//! down from the folder that is scanned.
//!
//! A `.git` -- folder or file, a submodule and a linked worktree mark one
//! the same way -- ends the walk right there: nothing nested inside a
//! repository already found is a repository of its own. A `.vivac/` is
//! never entered either, since a lane or a tree never holds a repository.

use crate::event::Repo;
use std::path::{Path, PathBuf};

/// The deepest a repository can sit beneath the folder that is scanned.
/// `t594` §4.5.1 fixes it at two, which is also what keeps a symlink cycle
/// from running away with the walk: nothing extra has to guard against
/// one when the recursion cannot go past this anyway.
const MAX_DEPTH: u32 = 2;

/// Every repository `folder` holds. Sorted by path, so two runs over an
/// unchanged folder produce the same list and `setup` can tell "nothing
/// changed" from "something did".
pub fn scan(folder: &Path) -> Vec<Repo> {
    let mut found = Vec::new();
    walk(folder, folder, 0, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn walk(base: &Path, dir: &Path, depth: u32, found: &mut Vec<Repo>) {
    if dir.join(".git").exists() {
        if let Some(repo) = Repo::relative(base, dir, root_commit(dir)) {
            found.push(repo);
        }
        // Never descend into a repository already found: whatever sits
        // inside it belongs to that repository, not to this walk.
        return;
    }
    if depth == MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| p.file_name().is_some_and(|n| n != crate::store::DIR))
        .collect();
    subdirs.sort();
    for sub in subdirs {
        walk(base, &sub, depth + 1, found);
    }
}

/// `git rev-list --max-parents=0 HEAD`, run inside `repo`: the commit it
/// was born from, which is what makes two clones of one repository
/// recognisable without a remote URL ever being written down (`d597`).
/// The lowest by text order, when there is more than one.
///
/// `t594` §4.5.1: this is the one place `setup` runs git, since setup is
/// nowhere near the write budget and the spec says so outright.
///
/// `None` for a repository whose git fails -- no commits, git itself
/// missing, or anything else: losing the whole folder over one repository
/// with no root commit would cost far more than a lane that carries one
/// un-rooted repository does.
fn root_commit(repo: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .min()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vivac-repos-{name}-{}", crate::id::ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A `.git` folder is enough to mark a repository for this walk: no
    /// commit has to exist, since `root_commit` accounts for that on its
    /// own.
    fn make_repo(at: &Path) {
        std::fs::create_dir_all(at.join(".git")).unwrap();
    }

    fn real_git_repo(at: &Path) {
        std::fs::create_dir_all(at).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(at)
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(at.join("f.txt"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
    }

    #[test]
    fn a_repository_two_levels_down_is_found_and_one_three_levels_down_is_not() {
        let dir = temp_dir("depth");
        make_repo(&dir.join("x").join("y"));
        make_repo(&dir.join("p").join("q").join("r"));
        let found = scan(&dir);
        let paths: Vec<&str> = found.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["x/y"], "{found:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_walk_does_not_descend_into_a_repository_it_already_found() {
        let dir = temp_dir("nodescend");
        make_repo(&dir.join("outer"));
        make_repo(&dir.join("outer").join("inner"));
        let found = scan(&dir);
        let paths: Vec<&str> = found.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["outer"], "{found:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_walk_never_enters_a_vivac_directory() {
        let dir = temp_dir("skipvivac");
        make_repo(&dir.join(crate::store::DIR).join("nested"));
        let found = scan(&dir);
        assert!(found.is_empty(), "{found:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_folder_itself_being_the_repository_is_recorded_as_a_dot() {
        let dir = temp_dir("dot");
        make_repo(&dir);
        let found = scan(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, ".");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn paths_are_relative_and_use_forward_slashes_on_every_platform() {
        let dir = temp_dir("slashes");
        make_repo(&dir.join("nested").join("webapi"));
        let found = scan(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "nested/webapi");
        assert!(!found[0].path.contains('\\'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repository_whose_git_fails_is_kept_with_no_root_commit() {
        let dir = temp_dir("nogit");
        make_repo(&dir.join("broken"));
        let found = scan(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "broken");
        assert_eq!(found[0].root, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_scans_of_an_unchanged_folder_are_equal() {
        let dir = temp_dir("stable");
        real_git_repo(&dir.join("webapi"));
        make_repo(&dir.join("broken"));
        let first = scan(&dir);
        let second = scan(&dir);
        assert_eq!(first, second);
        std::fs::remove_dir_all(&dir).ok();
    }

    // `d600`: a folder name the redaction guard rejects never reaches the
    // log, and the lane is declared anyway under a name derived only from
    // its own id. `name_for` ignores the folder name it is handed, so a
    // test that only calls it proves nothing about the guard itself --
    // `tests/lanes.rs`'s `a_worktree_named_a_secret_never_writes_it_to_the_log`
    // is the one that declares a real lane from a rejected name and reads
    // the real log (`t594`).
}
