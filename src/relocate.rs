//! `vivac relocate <destination>` -- moves a tree to another folder and
//! leaves the folder it left as one of its own lanes, its thread intact.
//!
//! The motivating case: a tree planted inside a git clone has to move to the
//! folder that holds the product, which survives the clone. The clone keeps
//! working afterwards -- it is now a lane, not the tree's own folder -- and
//! nothing about its stack, its focus or its counters changes underfoot.
//!
//! Eight steps, and the order is the guarantee:
//!
//! 1. Refuse from anywhere but the folder that holds the tree.
//! 2. Take the tree's write lock. Everything after this runs with it held.
//! 3. Refuse a destination that already holds a tree or a lane.
//! 4. Copy `events`, `config` and `.gitignore` there, and a fresh `lock`.
//! 5. Compare the two copies byte for byte. A mismatch rolls the copy back
//!    and fails without having touched the origin -- this is what makes the
//!    move safe, not an explanation of it.
//! 6. Point the registry at the destination.
//! 7. In the origin: rename `events` and `config` out of the way, drop the
//!    stale index, and write `.vivac/lane` so this folder keeps answering as
//!    whichever lane it always was.
//! 8. Print what moved, and where.
//!
//! Steps 6 and 7 run in the reverse of that order below: `registry::note`
//! believes a second root is a copy rather than a move for as long as the
//! path already on file still shows a live tree (`registry.rs`), so noting
//! the destination before the origin's own `events` is renamed away would
//! record a copy and never move `path` at all. The origin is silent about
//! this either way -- what changes is only which of the two steps a reader
//! finds first below.
//!
//! Not reachable over MCP (`t594` §4.6): it moves data and does not undo a
//! step, the same reason `abandon` and `restore` stay CLI-only.

use crate::event::{Body, Repo};
use crate::failure::Failure;
use crate::model::Tree;
use crate::output::outln;
use crate::registry::Sighting;
use crate::store::{Located, Store};
use std::path::Path;

/// The origin's own copies, renamed out of the way rather than deleted:
/// `t594` §4.6 leaves the history readable on disk, not just in the log
/// that moved.
const RELOCATED_LOG: &str = "events.relocated";
const RELOCATED_CONFIG: &str = "config.relocated";

pub fn run(located: &Located, destination: &Path, lane_name: Option<&str>) -> Result<i32, Failure> {
    // Step 1: `located.root == located.lane_dir` is the same test every
    // other write-time check in this crate uses for "is this folder the
    // tree's own" -- two fields `store::locate` already resolved together,
    // not two paths written by different programs, so `anchor::same_folder`
    // has nothing to do here.
    if located.root != located.lane_dir {
        return Err(Failure::Model(
            "  Run relocate in the folder that holds the tree, not in one of its lanes."
                .to_string(),
        ));
    }

    // Step 2. Everything below runs with this held, and it is dropped -- and
    // so released -- when `run` returns, success or failure alike.
    let origin = Store::open(located.root.clone())?;
    let lock = origin.lock_for_write()?;

    // Read the tree now, under the lock, once for the whole operation: the
    // repositories every lane declared (step 6) and this folder's own
    // governance (step 7's append) both come from here, and nothing past
    // this point changes what the log says until step 7 renames it away.
    let (events, broken) = origin.read_all()?;
    let tree = crate::model::fold(&events, broken);

    // `d201`: the registry, and every `.vivac/lane` file, is keyed by the
    // *first event's own id* -- never `Config::project_id`, which
    // `Store::open` mints fresh any time `config` itself has to be
    // regenerated, `registry.rs`'s own doc says so by name. `events.first`
    // rather than a second read of the file this process already has open.
    // `None` for a tree with nothing in it yet: there is no identity to
    // give the folder this leaves behind, so the registry write below is
    // skipped rather than keyed by a value nothing else will ever compare
    // against, the same silence `main.rs` already answers an empty tree
    // with.
    let project_id: Option<String> = events.first().map(|e| e.id.clone());

    let origin_vivac = located.root.join(crate::store::DIR);

    // Step 3. `destination` is compared against the origin with
    // `same_folder`, never raw: it is a path this process just parsed
    // against one `store::locate` resolved by an entirely different route,
    // and a second spelling of the same folder must read as busy, not as
    // free to write into.
    if crate::anchor::same_folder(destination, &located.root)
        || destination_holds_a_tree_or_lane(destination)
    {
        return Err(Failure::Model(format!(
            "  {} already holds a tree or a lane. Choose a folder with neither.",
            destination.display()
        )));
    }
    std::fs::create_dir_all(destination)?;

    // Step 4.
    let destination_vivac = destination.join(crate::store::DIR);
    std::fs::create_dir_all(&destination_vivac)?;
    std::fs::copy(
        origin_vivac.join(crate::store::LOG),
        destination_vivac.join(crate::store::LOG),
    )?;
    std::fs::copy(
        origin_vivac.join(crate::store::CONFIG),
        destination_vivac.join(crate::store::CONFIG),
    )?;
    let origin_gitignore = origin_vivac.join(crate::store::GITIGNORE);
    if origin_gitignore.is_file() {
        std::fs::copy(
            &origin_gitignore,
            destination_vivac.join(crate::store::GITIGNORE),
        )?;
    } else {
        crate::store::write_gitignore(&destination_vivac)?;
    }
    std::fs::File::create(destination_vivac.join(crate::store::LOCK))?;

    // Step 5. The one comparison this whole operation's safety rests on: if
    // the destination did not end up with exactly what the origin has, the
    // copy is torn down and nothing about the origin is touched -- no
    // rename, no lane file, no registry write.
    let log_matches = same_bytes(
        &origin_vivac.join(crate::store::LOG),
        &destination_vivac.join(crate::store::LOG),
    )?;
    let config_matches = same_bytes(
        &origin_vivac.join(crate::store::CONFIG),
        &destination_vivac.join(crate::store::CONFIG),
    )?;
    if !log_matches || !config_matches {
        std::fs::remove_dir_all(&destination_vivac).ok();
        return Err(Failure::Io(std::io::Error::other(
            "the copy at the destination did not match the source byte for byte; \
             nothing was moved",
        )));
    }

    // The lane that stays at the origin: its own id, or `lane::MAIN` for the
    // implicit founding lane every tree without a lane file of its own is.
    // `was_implicit_main` is exactly the second case, and it is what step 7
    // needs to know whether `main` itself needs claiming.
    let was_implicit_main = located.lane.is_none();
    let stays_lane = located
        .lane
        .as_ref()
        .map(|l| l.id.clone())
        .unwrap_or_else(|| crate::lane::MAIN.to_string());

    // Step 7 runs before step 6, the reverse of how they read above, and
    // that is deliberate rather than a slip: `registry::note`'s own
    // `path_disagrees` asks whether the path already on file *still shows
    // a live tree* before it believes a second root is a move rather than
    // a copy (`registry.rs`, `d201`). Noting the destination while the
    // origin's `events` was still sitting there under its own name would
    // answer that question wrong -- both would look alive, so the write
    // that should point the registry at the destination would instead
    // record the destination as a copy and leave `path` exactly where it
    // was. Renaming the origin's own files away first is what makes the
    // note below a move.
    //
    // Renamed, not deleted: the history stays readable at the origin under
    // its old name, for whoever goes looking. The stale index is dropped
    // outright -- it is derived and regenerates on the next read, and one
    // built against a log that no longer lives here would just be wrong.
    std::fs::rename(
        origin_vivac.join(crate::store::LOG),
        origin_vivac.join(RELOCATED_LOG),
    )?;
    std::fs::rename(
        origin_vivac.join(crate::store::CONFIG),
        origin_vivac.join(RELOCATED_CONFIG),
    )?;
    std::fs::remove_file(origin_vivac.join(crate::store::INDEX)).ok();
    crate::lane::write(
        &origin_vivac,
        &crate::lane::Lane {
            version: 1,
            id: stays_lane.clone(),
            // Falls back to `Config::project_id` only for a tree with no
            // events at all: there is no first event to be keyed by yet,
            // and no lookup this folder could fail differently over.
            project: project_id
                .clone()
                .unwrap_or_else(|| origin.config.project_id.clone()),
        },
    )?;

    // Step 6. `repos` is the union of every root commit any lane this tree
    // ever declared, so a later `setup` anywhere finds this product mapped
    // no matter which lane's repository it is asked about.
    if let (Some(store_dir), Some(project_id)) = (crate::store::store_dir(), &project_id) {
        let repos = union_repo_roots(&tree);
        let _ = crate::registry::note(
            &store_dir,
            project_id,
            Sighting {
                root: destination,
                lane: Some((&stays_lane, &located.root)),
                repos: Some(&repos),
            },
        );
    }

    // `origin`'s own `.vivac/lock` is left exactly where it is: it cannot be
    // deleted while `lock` still holds it -- that is the very lock this
    // whole operation is running under -- and letting it go just to delete
    // it would open a race with whoever took it in the meantime. An empty
    // lock file in a folder that holds no tree means nothing: what makes a
    // folder a tree is `events` or `config`, and this folder has neither
    // any more. Nobody should "clean it up" later.
    let _ = &lock;

    write_claim_and_declaration(
        destination,
        &tree,
        was_implicit_main,
        &stays_lane,
        lane_name,
    )?;

    // Step 8. `destination` is printed exactly as the caller passed it: they
    // just typed it, and the security pillar's ban on paths in the log has
    // nothing to say about handing someone back their own words.
    outln!(
        "  Moved the tree to {} and checked it byte for byte.",
        destination.display()
    );
    outln!("  This folder stays one of its lanes, with its own thread.");
    outln!("  The old log is kept here as .vivac/{RELOCATED_LOG}.");
    outln!("  Restart any session open on this tree.");
    Ok(0)
}

/// Whether `destination`'s own `.vivac/` already holds a tree (`events` or
/// `config`) or a lane (`lane`). Checked before anything is created there,
/// on top of the `same_folder` check in `run`: this is a plain existence
/// check at one path, not a comparison of two, so it needs nothing beyond
/// what the filesystem already resolves through any spelling on its own.
fn destination_holds_a_tree_or_lane(destination: &Path) -> bool {
    let vivac = destination.join(crate::store::DIR);
    vivac.join(crate::store::LOG).is_file()
        || vivac.join(crate::store::CONFIG).is_file()
        || vivac.join(crate::store::LANE).is_file()
}

/// Whether the two files are identical, byte for byte. Small enough files --
/// a log, a config -- that reading each one whole is the plain way to ask.
fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read(a)? == std::fs::read(b)?)
}

/// Every root commit any lane of `tree` ever declared, deduplicated and
/// sorted for a deterministic write: `registry::note`'s own `Sighting.repos`
/// wants the union across every lane, not just the one that stays at the
/// origin, so a `setup` run from any of this product's repositories finds
/// it already mapped.
fn union_repo_roots(tree: &Tree) -> Vec<String> {
    let mut roots: Vec<String> = tree
        .lanes
        .values()
        .flat_map(|state| state.repos.iter())
        .filter_map(|repo| repo.root.clone())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// The administrative events step 7 owes the tree now living at
/// `destination`, once the origin's own bookkeeping is already written:
///
/// - `lane.claimed` for `main`, only when the origin was the implicit
///   founding lane (`was_implicit_main`): `main` is retired there, so a
///   later `setup` run at the destination mints it a lane of its own
///   instead of reclaiming a name that already belongs elsewhere
///   (`setup::claude_code::plan_lane`'s own sixth case).
/// - `lane.declared` for `stays_lane`, when `--lane-name` renamed it, or
///   when the origin already had repositories of its own and needs its
///   declaration carried forward under the very same lane id.
///
/// Both are signed as `stays_lane`: the folder that stays announces this
/// about itself, the same convention `ops::declare_lane` already uses.
/// Appended through a fresh `Store` and its own lock on `destination`,
/// never the origin's -- `run`'s own lock only ever covers `origin`'s file,
/// and taking it a second time here would be exactly the mistake
/// `Store::append` already refuses (`f602`).
fn write_claim_and_declaration(
    destination: &Path,
    tree: &Tree,
    was_implicit_main: bool,
    stays_lane: &str,
    lane_name: Option<&str>,
) -> Result<(), Failure> {
    let existing = tree.lanes.get(stays_lane);
    let existing_repos: Vec<Repo> = existing.map(|s| s.repos.clone()).unwrap_or_default();
    let existing_name = existing.map(|s| s.name.clone()).unwrap_or_default();

    // A name the guard rejects falls back to `lane::name_for` rather than
    // failing the operation (`d600`): `lane::declared_name` already makes
    // that promise for every other caller that names a lane from a word a
    // person typed, and `--lane-name` is exactly that.
    let redeclare_name: Option<String> = match lane_name {
        Some(name) => Some(crate::lane::declared_name(stays_lane, name)),
        None if was_implicit_main && !existing_repos.is_empty() => Some(existing_name),
        None => None,
    };

    let mut bodies = Vec::new();
    if was_implicit_main {
        bodies.push(Body::LaneClaimed {
            lane: crate::lane::MAIN.to_string(),
        });
    }
    if let Some(name) = redeclare_name {
        bodies.push(Body::LaneDeclared {
            lane: stays_lane.to_string(),
            name,
            repos: existing_repos,
        });
    }
    if bodies.is_empty() {
        return Ok(());
    }

    let mut moved = Store::open(destination.to_path_buf())?;
    let moved_lock = moved.lock_for_write()?;
    moved.append(
        &moved_lock,
        stays_lane,
        bodies,
        tree.seq,
        tree.has_governance,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vivac-relocate-{prefix}-{}", id::ulid()))
    }

    fn seeded_located(root: &Path) -> Located {
        Store::create(root).unwrap();
        Located {
            root: root.to_path_buf(),
            lane_dir: root.to_path_buf(),
            lane: None,
            worktree: None,
        }
    }

    /// The property `tests/relocate.rs` cannot exercise: a process that
    /// opened this tree's `Store` *before* `relocate` ran -- an MCP server
    /// resident on the folder it started in, most realistically -- still
    /// holds a handle whose `log_present` remembers the file that was there
    /// at the time. `Store::append` already refuses to recreate a log that
    /// vanished underneath it (`append_never_recreates_a_log_that_vanished`,
    /// `store.rs`); this is that same guarantee, exercised as a consequence
    /// of the rename `relocate` itself just made, not of a file deleted by
    /// hand.
    #[test]
    fn an_old_process_writing_after_the_move_creates_no_log() {
        let origin = temp_dir("old-process-origin");
        let located = seeded_located(&origin);
        let mut old_process = Store::open(origin.clone()).unwrap();
        let destination = temp_dir("old-process-dest");

        let code = run(&located, &destination, None).unwrap();
        assert_eq!(code, 0);
        assert!(
            !origin
                .join(crate::store::DIR)
                .join(crate::store::LOG)
                .is_file(),
            "relocate must have already renamed the origin's own log away"
        );

        let lock = old_process.lock_for_write().unwrap();
        let body = vec![crate::event::Body::NodeNoted {
            node: "01OLDPROCESSAAAAAAAAAAAAAA".into(),
            note: "written by a handle opened before the move".into(),
        }];
        let result = old_process.append(&lock, crate::lane::MAIN, body, 0, false);
        assert!(
            result.is_err(),
            "a handle opened before the move recreated the log relocate just renamed away"
        );
        assert!(
            !origin
                .join(crate::store::DIR)
                .join(crate::store::LOG)
                .is_file(),
            "a new events file appeared at the origin after an old handle wrote to it"
        );

        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&destination).ok();
    }
}
