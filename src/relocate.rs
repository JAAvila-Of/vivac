//! `vivac relocate <destination>` -- moves a tree to another folder and
//! leaves the folder it left as one of its own lanes, its thread intact.
//!
//! The motivating case: a tree planted inside a git clone has to move to the
//! folder that holds the product, which survives the clone. The clone keeps
//! working afterwards -- it is now a lane, not the tree's own folder -- and
//! nothing about its stack, its focus or its counters changes underfoot.
//!
//! **The idea this whole module answers to:** no intermediate state may
//! look like an empty tree. An empty tree accepts writes -- `Store::open`
//! seeds one the moment `config` or `events` is missing where the other
//! is not -- so a failure partway through never loses the history, it
//! *replaces* it with a different one nobody asked for, in silence. Every
//! step below is ordered, or made fallible, or both, so that no crash
//! between any two of them leaves a folder `store::already_planted` would
//! call a tree that is not the real one.
//!
//! Ten steps (`t594` fix-2 corrects the eight the original task named):
//!
//! 1. Refuse from anywhere but the folder that holds the tree.
//! 2. Refuse a destination that is this folder itself, or sits inside it.
//!    Made absolute against the current directory here, before anything
//!    below compares or touches it.
//! 3. Take the tree's write lock. Everything after this runs with it held.
//! 4. Refuse a tree with no events yet: it has no identity (`d201`) for
//!    the registry or the origin's own lane file to be keyed by.
//! 5. Refuse a destination that already holds a tree or a lane. Create it
//!    if it does not exist yet.
//! 6. Copy `events` and `config` to temporary names, compare each against
//!    the origin byte for byte, then rename both onto their real names --
//!    the one point where a truncated write could otherwise land under a
//!    name `store::already_planted` trusts. `.gitignore` and a fresh
//!    `lock` go straight to their real names; neither one is what that
//!    function looks at. Any failure here undoes only what this run
//!    itself created and leaves the origin untouched.
//! 7. Point the registry at the destination, through the one entry that
//!    is told rather than asked to guess (`registry::record_move`) and
//!    that fails the caller when it cannot land. A failure here undoes
//!    step 6 the same way step 6 undoes itself, and leaves the origin
//!    untouched: past this step, the registry is the only durable record
//!    of where the tree went, and `relocate` cannot claim success without
//!    it.
//! 8. In the origin: write `.vivac/lane` first, then rename `config` and
//!    `events` out of the way (in that order -- see `run`'s own comment),
//!    then drop the stale index. Every point in this sequence still reads
//!    as a real folder: either the origin's own tree, still there, or a
//!    lane whose tree the registry already relocated to in step 7.
//! 9. In the tree now at the destination: `lane.claimed` for `main`, and
//!    `lane.declared` when it is owed. Past step 7 there is nothing left
//!    to roll back -- the move already happened -- so a failure here
//!    still exits 0 and says what is missing, rather than reporting a
//!    move that did not happen when it did.
//! 10. Print what moved, and where.
//!
//! **Step 6's own byte mismatch has no trigger a black-box test can
//! reach.** The copy and the read that verifies it run back to back
//! inside one synchronous call, with no point in between where anything
//! else could land a write to the source -- not without adding an
//! injection point to this very path, which is worse than the gap it
//! would close. `same_bytes` itself is a plain enough function to probe
//! on its own, though, and its own unit tests do that directly.
//!
//! **That the comparison runs at all is no longer only a promise a test
//! can check -- it is now something the compiler checks too.** An earlier
//! round of this task deleted the comparison outright and the whole suite
//! stayed green, because nothing forced the temporary files' rename onto
//! their real names to have a reason to run. `verify_copy` now returns a
//! `Verified` -- a private, dataless type only it can build -- and
//! `commit_copy`, the one place those renames happen, takes one as an
//! argument. Deleting the comparison either leaves `Verified` built on
//! nothing, which a reviewer reading the diff sees immediately, or it
//! orphans `same_bytes`, which `cargo clippy -D warnings` -- already
//! required to pass -- refuses to build at all. What `tests/relocate.rs`
//! exercises from outside is the rollback itself, shared by every failure
//! step 6 can report: a copy that never lands at all, forced by a
//! `.vivac/.gitignore` at the destination that is already a directory. No
//! operating system lets a file land where a directory already sits, so
//! that trigger is deterministic and needs nothing this crate does not
//! already have. A hole named here is a hole; one left unsaid would be a
//! lie.
//!
//! **The temporary names' own specific benefit -- surviving a process
//! killed outright mid-copy, not merely an error Rust propagates in the
//! ordinary way -- has no black-box trigger either.** Every failure a
//! test can provoke still runs this module's own cleanup code, which
//! already removes a stray `.tmp` and a truncated final-named file alike;
//! only a `SIGKILL` between two writes would show the difference, and
//! nothing here asks a test to kill itself to prove a point. What the
//! tests below do pin is the promise that actually matters: in every
//! failure this code can report, no `events` or `config` ever appears
//! under its real name before the byte comparison has already passed.
//!
//! **Step 9's own failure has no black-box trigger either, for a
//! different reason.** By the time it runs, the move already happened
//! and the registry already says so; making it fail on its own -- rather
//! than as a side effect of steps 1 through 8 not having run at all --
//! would need the destination's own fresh `.vivac/lock` already held by
//! something else at the exact moment step 9 takes it, which is a race
//! against this function's own internal timing, not a state a black-box
//! test can simply ask for. `write_claim_and_declaration`'s own unit
//! tests exercise its failure directly instead, which is the one thing
//! `run`'s `.is_ok()` actually depends on.
//!
//! Not reachable over MCP (`t594` §4.6): it moves data and does not undo a
//! step, the same reason `abandon` and `restore` stay CLI-only.

use crate::event::{Body, Repo};
use crate::failure::Failure;
use crate::model::Tree;
use crate::output::outln;
use crate::registry::Sighting;
use crate::store::{Located, Store};
use std::path::{Path, PathBuf};

/// The origin's own copies, renamed out of the way rather than deleted:
/// `t594` §4.6 leaves the history readable on disk, not just in the log
/// that moved.
const RELOCATED_LOG: &str = "events.relocated";
const RELOCATED_CONFIG: &str = "config.relocated";

/// `cwd` is an argument rather than read here with `std::env::current_dir`,
/// on purpose: this function's own unit tests build a `Located` for a tree
/// that lives nowhere near the test binary's real working directory, and
/// reading the environment directly would make step 1 refuse every one of
/// them, or force each test to move the whole process's own current
/// directory -- shared, global state a parallel test run cannot afford to
/// touch. `main.rs` passes the same `cwd` it already resolved `located`
/// from.
pub fn run(
    located: &Located,
    destination: &Path,
    lane_name: Option<&str>,
    cwd: &Path,
) -> Result<i32, Failure> {
    // Step 1: the current directory has to *be* the tree's own folder, not
    // merely resolve to the same root and lane the way any subfolder of it
    // also would -- `store::locate` walks upward, so `located.root ==
    // located.lane_dir` stays true three levels down, and `relocate` from
    // there used to succeed and then talk about a lane and a log that were
    // never in that subfolder to begin with. `same_folder`, not a raw
    // comparison: `cwd` came straight from the operating system and
    // `located.root` came back from `store::locate`'s own walk, two
    // independent sources for the same folder.
    if located.root != located.lane_dir || !crate::anchor::same_folder(cwd, &located.root) {
        return Err(Failure::Model(
            "  Run relocate in the folder that holds the tree, not in one of its lanes."
                .to_string(),
        ));
    }

    // Step 2. Absolute first: the two checks right after it compare
    // `destination` against `located.root` with `same_folder`, and both
    // `Path::ancestors` and `canonicalize`'s own fallback need something
    // rooted to walk, not a string that only means anything relative to
    // wherever this process happens to be standing.
    let destination_abs = to_absolute(destination, cwd);
    if crate::anchor::same_folder(&destination_abs, &located.root) {
        return Err(Failure::Model(
            "  The destination is this folder, so there is nothing to move.".to_string(),
        ));
    }
    if is_inside(&destination_abs, &located.root) {
        return Err(Failure::Model(
            "  The destination is inside this folder. Moving the tree into itself\n  \
             would leave it with two."
                .to_string(),
        ));
    }

    // Step 3. Everything below runs with this held, and it is dropped --
    // and so released -- when `run` returns, success or failure alike.
    let origin = Store::open(located.root.clone())?;
    let lock = origin.lock_for_write()?;

    // Read the tree now, under the lock, once for the whole operation: the
    // repositories every lane declared (step 7) and this folder's own
    // governance (step 9's append) both come from here, and nothing past
    // this point changes what the log says until step 8 renames it away.
    let (events, broken) = origin.read_all()?;
    let tree = crate::model::fold(&events, broken);

    // Step 4. `d201`: a tree with no first event has no identity yet, so
    // there is nothing for the registry or the origin's own lane file to
    // be keyed by, and nothing anywhere else that could break by this
    // folder simply being moved by hand instead.
    let Some(first_event_id) = events.first().map(|e| e.id.clone()) else {
        return Err(Failure::Model(
            "  This tree has no events yet, so nothing points at it and nothing would\n  \
             break: move the folder yourself."
                .to_string(),
        ));
    };

    let origin_vivac = located.root.join(crate::store::DIR);

    // Step 5. `destination_holds_a_tree_or_lane` is a plain existence check
    // at one path, not a comparison of two, so it needs nothing beyond
    // what the filesystem already resolves through any spelling on its
    // own -- unlike the two checks in step 2, above.
    if destination_holds_a_tree_or_lane(&destination_abs) {
        return Err(Failure::Model(format!(
            "  {} already holds a tree or a lane. Choose a folder with neither.",
            destination.display()
        )));
    }
    std::fs::create_dir_all(&destination_abs)?;

    // Step 6. `written` is filled in as it happens, so a rollback removes
    // exactly what this run created and nothing that was already there
    // (`t594` fix-2, finding M1).
    let destination_vivac = destination_abs.join(crate::store::DIR);
    let mut written = Written {
        vivac_dir_created: !destination_vivac.is_dir(),
        files: Vec::new(),
    };
    let (log_tmp, config_tmp, verified) =
        match verify_copy(&origin_vivac, &destination_vivac, &mut written) {
            Ok(v) => v,
            Err(e) => {
                written.undo(&destination_vivac);
                return Err(e);
            }
        };
    if let Err(e) = commit_copy(
        verified,
        &log_tmp,
        &config_tmp,
        &origin_vivac,
        &destination_vivac,
        &mut written,
    ) {
        written.undo(&destination_vivac);
        return Err(e);
    }

    // The lane that stays at the origin: its own id, or `lane::MAIN` for the
    // implicit founding lane every tree without a lane file of its own is.
    // `was_implicit_main` is exactly the second case, and it is what step 9
    // needs to know whether `main` itself needs claiming.
    let was_implicit_main = located.lane.is_none();
    let stays_lane = located
        .lane
        .as_ref()
        .map(|l| l.id.clone())
        .unwrap_or_else(|| crate::lane::MAIN.to_string());

    // Step 7. `record_move`, not `note`: `relocate` already knows this is a
    // move, so it tells the registry rather than asking it to guess from
    // whether the origin still looks alive -- guessing is `note`'s own
    // `path_disagrees`, and it is exactly why an earlier round of this task
    // had to run this step after step 8 instead of before it, the order
    // the specification actually wants. With a fallible, explicit write
    // the order goes back to what it should always have been: the
    // registry lands before the origin's own log is touched at all, and a
    // failure here undoes step 6 and stops, the origin never having lost
    // anything.
    let repos = union_repo_roots(&tree);
    let Some(store_dir) = crate::store::store_dir() else {
        written.undo(&destination_vivac);
        return Err(Failure::Io(std::io::Error::other(
            "no VIVAC_HOME to record the move in; the registry would lose the tree",
        )));
    };
    if let Err(e) = crate::registry::record_move(
        &store_dir,
        &first_event_id,
        Sighting {
            root: &destination_abs,
            lane: Some((&stays_lane, &located.root)),
            repos: Some(&repos),
        },
    ) {
        written.undo(&destination_vivac);
        return Err(Failure::Io(e));
    }

    // Step 8. The origin's own bookkeeping. Past step 7 there is nothing
    // left to roll back, so a failure partway through here is reported
    // with what it actually means, not the bare IO error underneath it --
    // see `write_origin_bookkeeping`'s own doc for the order, and why it
    // is safe to be interrupted anywhere in it.
    if let Err(e) = write_origin_bookkeeping(&origin_vivac, &stays_lane, first_event_id) {
        return Err(Failure::Io(std::io::Error::other(format!(
            "{e}\n\n  Something failed while updating this folder's own bookkeeping. \
             The destination already has the tree, and the registry already points \
             there, but this folder may still hold a working copy of its own, or may \
             not. Run `vivac check` here, and at the destination, to see how they \
             compare."
        ))));
    }

    // `origin`'s own `.vivac/lock` is left exactly where it is: it cannot be
    // deleted while `lock` still holds it -- that is the very lock this
    // whole operation is running under -- and letting it go just to delete
    // it would open a race with whoever took it in the meantime. An empty
    // lock file in a folder that holds no tree means nothing: what makes a
    // folder a tree is `events` or `config`, and this folder has neither
    // any more. Nobody should "clean it up" later.
    let _ = &lock;

    // Step 9. Past step 7 the move already happened and the registry
    // already says so, so a failure marking the moved tree is not this
    // operation's own failure to report -- it still exits 0, and step 10
    // says what is missing instead.
    let marked = write_claim_and_declaration(
        &destination_abs,
        &tree,
        was_implicit_main,
        &stays_lane,
        lane_name,
    )
    .is_ok();

    // Step 10. `destination` is printed exactly as the caller passed it:
    // they just typed it, and the security pillar's ban on paths in the
    // log has nothing to say about handing someone back their own words.
    outln!(
        "  Moved the tree to {} and checked it byte for byte.",
        destination.display()
    );
    outln!("  This folder stays one of its lanes, with its own thread.");
    outln!("  The old log is kept here as .vivac/{RELOCATED_LOG}.");
    outln!("  The new folder holds the tree but is not a lane yet. To work there:");
    outln!("    vivac setup claude-code");
    outln!("  Restart any session open on this tree.");
    if !marked {
        outln!("  The moved tree could not be marked from here. Run this in it:");
        outln!("    vivac setup claude-code");
    }
    Ok(0)
}

/// `destination` made absolute against `cwd`, by joining rather than
/// `canonicalize`: the destination usually does not exist yet, and
/// canonicalizing a path that is not there fails. Then normalized
/// lexically -- `.` and `..` resolved one component at a time, no disk
/// access -- which is not cosmetic: `vivac relocate ..` used to leave the
/// registry holding `…\clone\..` outright, and `Path::file_name` of a path
/// ending in `..` is `None`, so `render::project_name` read that back as
/// the bare word `"-"` rather than the folder's own name (`t594` fix-3,
/// finding N1) -- every project this ever ran on would have collided on
/// that one name in `find --everywhere`.
///
/// Nothing this touches is a promise about the destination's real, on-disk
/// spelling -- only `same_folder`'s own fallback goes that far, and only
/// for paths that exist -- it just gives every comparison and every
/// filesystem call past this point something rooted and free of `..` to
/// work with.
fn to_absolute(destination: &Path, cwd: &Path) -> PathBuf {
    let joined = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        cwd.join(destination)
    };
    crate::anchor::normalize(&joined)
}

/// Whether `destination` sits inside `origin`: every proper ancestor of
/// `destination`, walked syntactically (`Path::ancestors`, no filesystem
/// access -- `destination` may not exist yet) and compared against
/// `origin` with `same_folder`, the same primitive every other path
/// comparison in this module goes through, rather than a raw prefix
/// check that a name merely starting with the same characters would
/// satisfy by accident.
///
/// `destination` is normalized first, and that is not cosmetic:
/// `Path::ancestors` treats every component as just another name to
/// strip off the end, so an unresolved `..` in the middle -- `to_absolute`
/// only joins, it never resolves one -- makes the ancestor right before
/// it the very folder the `..` was meant to cancel out, and `relocate ..`
/// would see its own destination as sitting inside itself.
fn is_inside(destination: &Path, origin: &Path) -> bool {
    crate::anchor::normalize(destination)
        .ancestors()
        .skip(1)
        .any(|ancestor| crate::anchor::same_folder(ancestor, origin))
}

/// Whether `destination`'s own `.vivac/` already holds a tree (`events` or
/// `config`) or a lane (`lane`). Checked before anything is created there,
/// on top of the `same_folder` checks in `run`: this is a plain existence
/// check at one path, not a comparison of two, so it needs nothing beyond
/// what the filesystem already resolves through any spelling on its own.
fn destination_holds_a_tree_or_lane(destination: &Path) -> bool {
    let vivac = destination.join(crate::store::DIR);
    vivac.join(crate::store::LOG).is_file()
        || vivac.join(crate::store::CONFIG).is_file()
        || vivac.join(crate::store::LANE).is_file()
}

/// What step 6 has created so far, so a rollback removes exactly that and
/// nothing else. `t594` fix-2, finding M1: an earlier round of this task
/// tore down the destination's whole `.vivac/` on any failure, which took
/// a destination's own `notes.txt` and an unrelated `events.relocated`
/// from an earlier move along with it.
struct Written {
    /// Whether this run is the one that created `destination`'s own
    /// `.vivac/` directory. `undo` only ever removes that directory, and
    /// only when this is `true`.
    vivac_dir_created: bool,
    files: Vec<PathBuf>,
}

impl Written {
    /// Removes every file this run wrote, then the `.vivac/` directory
    /// itself -- but only when this run created it, and only
    /// `remove_dir`, never `remove_dir_all`: it refuses on its own the
    /// moment anything besides these files is still inside, which is
    /// exactly the case this must leave alone.
    fn undo(&self, destination_vivac: &Path) {
        for f in &self.files {
            std::fs::remove_file(f).ok();
        }
        if self.vivac_dir_created {
            std::fs::remove_dir(destination_vivac).ok();
        }
    }
}

/// Proof that the destination's own temporary copies of `events` and
/// `config` already matched the origin byte for byte. The only way to
/// build one is `verify_copy`, and the only thing it is good for is
/// handing to `commit_copy`, the one place the temporary names are ever
/// renamed onto the names `store::already_planted` looks at.
///
/// This exists because a unit test proved a plain `bool`, or a single
/// function that copies and renames in a row, is not enough: deleting the
/// comparison this type stands for left the rest of the module compiling
/// and the whole suite green, because nothing forced the renames to prove
/// they had a reason to run. Splitting the copy into two functions and
/// putting this between them turns that same deletion into a compiler
/// error -- `commit_copy` will not take a `bool`, or nothing at all, it
/// takes a `Verified`, and the only place one of those comes from is a
/// comparison that actually ran and actually agreed. No fields, no
/// `Clone`: the type itself is the guarantee, and it is spent the moment
/// it is used.
struct Verified(());

/// Step 6, first half: copies `events` and `config` from `origin_vivac`
/// into temporary names inside `destination_vivac`, and compares each
/// against the origin byte for byte. Returns the two temporary paths and a
/// `Verified` on success, for `commit_copy` to spend.
fn verify_copy(
    origin_vivac: &Path,
    destination_vivac: &Path,
    written: &mut Written,
) -> Result<(PathBuf, PathBuf, Verified), Failure> {
    std::fs::create_dir_all(destination_vivac)?;

    let log_tmp = destination_vivac.join(format!("events.{}.tmp", crate::id::ulid()));
    std::fs::copy(origin_vivac.join(crate::store::LOG), &log_tmp)?;
    written.files.push(log_tmp.clone());

    let config_tmp = destination_vivac.join(format!("config.{}.tmp", crate::id::ulid()));
    std::fs::copy(origin_vivac.join(crate::store::CONFIG), &config_tmp)?;
    written.files.push(config_tmp.clone());

    // The one comparison this whole operation's safety rests on: if the
    // copy did not end up with exactly what the origin has, nothing past
    // this point is trusted with either the tree's real name or the
    // origin's own log -- and with no `Verified` to hand `commit_copy`,
    // nothing past this point can even compile a call to it.
    if !same_bytes(&origin_vivac.join(crate::store::LOG), &log_tmp)?
        || !same_bytes(&origin_vivac.join(crate::store::CONFIG), &config_tmp)?
    {
        return Err(Failure::Io(std::io::Error::other(
            "the copy at the destination did not match the source byte for byte",
        )));
    }

    Ok((log_tmp, config_tmp, Verified(())))
}

/// Step 6, second half: writes `.gitignore` and a fresh `lock`, then
/// renames the two temporary files `verify_copy` already checked onto the
/// names `store::already_planted` looks at. Only reachable with a
/// `Verified` in hand -- see its own doc.
///
/// A `.gitignore` the destination already had is left exactly as it was,
/// the same promise `store::write_gitignore` already makes for the branch
/// that writes one from nothing: `t594` fix-3, finding N3, a rollback that
/// deleted one the destination brought with it because this used to copy
/// over it and track the result as its own regardless.
fn commit_copy(
    _verified: Verified,
    log_tmp: &Path,
    config_tmp: &Path,
    origin_vivac: &Path,
    destination_vivac: &Path,
    written: &mut Written,
) -> Result<(), Failure> {
    let destination_gitignore = destination_vivac.join(crate::store::GITIGNORE);
    if !destination_gitignore.is_file() {
        let origin_gitignore = origin_vivac.join(crate::store::GITIGNORE);
        if origin_gitignore.is_file() {
            std::fs::copy(&origin_gitignore, &destination_gitignore)?;
        } else {
            crate::store::write_gitignore(destination_vivac)?;
        }
        written.files.push(destination_gitignore);
    }

    let lock_path = destination_vivac.join(crate::store::LOCK);
    std::fs::File::create(&lock_path)?;
    written.files.push(lock_path);

    let log_final = destination_vivac.join(crate::store::LOG);
    std::fs::rename(log_tmp, &log_final)?;
    written.files.push(log_final);

    let config_final = destination_vivac.join(crate::store::CONFIG);
    std::fs::rename(config_tmp, &config_final)?;
    written.files.push(config_final);

    Ok(())
}

/// Whether the two files are identical, byte for byte. Small enough files --
/// a log, a config -- that reading each one whole is the plain way to ask.
fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read(a)? == std::fs::read(b)?)
}

/// Step 8: the origin's own bookkeeping, in an order chosen so that every
/// state in between still reads as a real folder:
///
/// - `.vivac/lane` is written *before* anything is renamed. With `events`
///   and `config` still both there, this folder is still `already_planted`
///   on its own terms, so it keeps resolving to itself -- the same folder
///   it always was, now also carrying a lane file that happens to already
///   say what it will answer once the rename below lands.
/// - `config` is renamed before `events`, not the order they are named in
///   prose. With `config` gone and `events` still there, `Store::open`
///   regenerates a config -- a real cost, a mismatched `project_id` and
///   `actor` on whatever writes next -- but reads and writes alike still
///   land on the one real, intact log. Renaming `events` first instead
///   would leave `config` momentarily alone: `Store::open` would read it
///   fine, but with no `events` file to answer for, a write would open one
///   fresh and empty right there, which is exactly the failure this whole
///   module exists to close. Once `events` is renamed too,
///   `already_planted` finally answers `false`, and resolution falls
///   through to the registry, which step 7 already pointed at the
///   destination.
/// - The stale index is dropped last and outright: it is derived and
///   regenerates on the next read, and one built against a log that no
///   longer lives here would just be wrong.
///
/// Nothing here rolls anything back on failure: by the time this runs, the
/// registry already points at the destination, so a folder this leaves
/// half done is a copy that never finished tidying up, not a lost tree.
fn write_origin_bookkeeping(
    origin_vivac: &Path,
    stays_lane: &str,
    first_event_id: String,
) -> std::io::Result<()> {
    crate::lane::write(
        origin_vivac,
        &crate::lane::Lane {
            version: 1,
            id: stays_lane.to_string(),
            project: first_event_id,
        },
    )?;
    std::fs::rename(
        origin_vivac.join(crate::store::CONFIG),
        origin_vivac.join(RELOCATED_CONFIG),
    )?;
    std::fs::rename(
        origin_vivac.join(crate::store::LOG),
        origin_vivac.join(RELOCATED_LOG),
    )?;
    std::fs::remove_file(origin_vivac.join(crate::store::INDEX)).ok();
    Ok(())
}

/// Every root commit any lane of `tree` ever declared, deduplicated and
/// sorted for a deterministic write: `registry::record_move`'s own
/// `Sighting.repos` wants the union across every lane, not just the one
/// that stays at the origin, so a `setup` run from any of this product's
/// repositories finds it already mapped.
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

/// The administrative events step 9 owes the tree now living at
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
/// `Store::append` already refuses (`f602`). A failure here reaches the
/// caller as an ordinary `Err`, but by the time `run` calls this the move
/// itself has already happened and cannot be undone -- see `run`'s own
/// step 9, which reports `Err` as a line of missing bookkeeping, not as
/// the operation's own failure.
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
        let mut s = Store::create(root).unwrap();
        let lock = s.lock_for_write().unwrap();
        s.append(
            &lock,
            crate::lane::MAIN,
            vec![crate::event::Body::NodeNoted {
                node: "t1".into(),
                note: "seed".into(),
            }],
            0,
            false,
        )
        .unwrap();
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

        let code = run(&located, &destination, None, &origin).unwrap();
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

    #[test]
    fn same_bytes_is_true_for_two_identical_files_and_false_for_two_that_differ() {
        let dir = temp_dir("same-bytes");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        let c = dir.join("c");
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, b"hello").unwrap();
        std::fs::write(&c, b"world").unwrap();

        assert!(
            same_bytes(&a, &b).unwrap(),
            "two identical files must agree"
        );
        assert!(
            !same_bytes(&a, &c).unwrap(),
            "two files with different content must not agree"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `run`'s own step 9: past step 7 the move already happened, so a
    /// failure marking the moved tree is not the operation's own failure
    /// to report -- `run` only checks whether this succeeded.
    ///
    /// Reaching that state through a full `run` call, with steps 1
    /// through 8 already succeeded, has no trigger a black-box test can
    /// reach: it would need the destination's own fresh `.vivac/lock`
    /// already held by something else at the exact moment step 9 takes
    /// it, which is a race against `run`'s own internal timing rather
    /// than a state a test can simply ask for. What this proves instead
    /// is the one thing `run`'s own `.is_ok()` actually depends on: the
    /// function fails honestly when it cannot write.
    #[test]
    fn write_claim_and_declaration_fails_when_the_destination_cannot_be_opened() {
        let destination = temp_dir("claim-blocked");
        // A file where `Store::open` needs a directory: writing
        // `.vivac/config` underneath it cannot even create the path, so
        // this fails before ever reaching a lock.
        std::fs::write(&destination, b"not a directory").unwrap();
        let tree = Tree::default();

        let result =
            write_claim_and_declaration(&destination, &tree, true, crate::lane::MAIN, None);

        assert!(
            result.is_err(),
            "a destination that cannot be opened must fail write_claim_and_declaration"
        );

        std::fs::remove_file(&destination).ok();
    }

    /// `t594` fix-2, finding 11 (B3): `--lane-name` with a value the
    /// redaction guard rejects must fall back rather than fail the move
    /// -- `lane::declared_name` already promises exactly this for every
    /// other caller that names a lane from a word a person typed, and
    /// this pins it for `relocate`'s own.
    #[test]
    fn a_lane_name_the_guard_rejects_falls_back_without_failing() {
        let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
        assert!(
            crate::redact::check_field("lane name", secret).is_some(),
            "the guard must actually reject this name, or the test proves nothing"
        );

        let origin = temp_dir("lane-name-guard-origin");
        let located = seeded_located(&origin);
        let destination = temp_dir("lane-name-guard-dest");

        let code = run(&located, &destination, Some(secret), &origin).unwrap();
        assert_eq!(code, 0);

        let log =
            std::fs::read_to_string(destination.join(crate::store::DIR).join(crate::store::LOG))
                .unwrap();
        assert!(
            !log.contains(secret),
            "a name the guard rejects must never reach the log: {log}"
        );

        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&destination).ok();
    }
}
