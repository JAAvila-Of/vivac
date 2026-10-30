//! `vivac relocate <destination>` -- moves a tree to another folder and
//! leaves the folder it left as one of its own lanes, its thread intact.
//!
//! The motivating case: a tree planted inside a git clone has to move to the
//! folder that holds the product, which survives the clone. The clone keeps
//! working afterwards -- it is now a lane, not the tree's own folder -- and
//! nothing about its stack, its focus or its counters changes underfoot.
//!
//! **The idea this whole module answers to:** a new destination must not
//! appear as a partial tree. An empty tree accepts writes -- `Store::open`
//! seeds one the moment `config` or `events` is missing where the other
//! is not -- so a failure partway through never loses the history, it
//! *replaces* it with a different one nobody asked for, in silence. Every
//! step below keeps the origin's log intact until the registry records the
//! move. A destination whose `.vivac/` already exists is still filled in
//! place, preserving its unrelated files; it does not get the new
//! directory's whole-publication guarantee.
//!
//! Ten steps (`t594` corrects the eight the original task named):
//!
//! 1. Refuse from anywhere but the folder that holds the tree.
//! 2. Refuse a destination that is this folder itself, or sits inside it.
//!    Made absolute against the current directory here, before anything
//!    below compares or touches it.
//! 3. Take the tree's write lock. Everything after this runs with it held.
//! 4. Refuse a tree with no events yet: it has no identity (`d201`) for
//!    the registry or the origin's own lane file to be keyed by. Then,
//!    with that identity in hand and before a byte is copied anywhere,
//!    refuse a move made from a copy: the registry still points this
//!    project at another folder, that folder is still there holding a tree
//!    with this same first event, and so the project does not live here.
//! 5. Refuse a destination that already holds a tree or a lane. Create it
//!    if it does not exist yet.
//! 6. For a new `.vivac/`, prepare the copy in a unique sibling directory;
//!    otherwise use the existing directory in place. Copy `events` and
//!    `config` to temporary names, compare each against the origin byte
//!    for byte, and synchronize them before assigning their real names.
//!    Publish a new directory with one rename, updating the rollback
//!    paths before synchronizing its parent. Any failure here undoes only
//!    what this run created and leaves the origin untouched.
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
//! 9. In the tree now at the destination: its own `.vivac/lane`, minted
//!    fresh, and `lane.declared` under the name its folder gives it
//!    (`d681`) -- without it the destination fell to the implicit `main`
//!    rule and answered as the very lane the origin just kept. Then
//!    `lane.claimed` for `main`, and `lane.declared` for the lane that
//!    stays, when either is owed. Past step 7 there is nothing left to
//!    roll back -- the move already happened -- so a failure here still
//!    exits 0 and says what is missing, rather than reporting a move
//!    that did not happen when it did.
//! 10. Print what moved, and where.
//!
//! **Step 6's own byte mismatch has no trigger a black-box test can
//! reach.** The copy and the read that verifies it run back to back
//! inside one synchronous call, with no point in between where anything
//! else could land a write to the source -- not without adding an
//! injection point to this very path, which is worse than the gap it
//! would close. `verified::compare` itself is a plain enough function to
//! probe on its own, though, and its own unit test does that directly.
//!
//! **That the comparison runs at all, and that it agreed, is not only a
//! promise a test can check -- it is something the compiler checks too,
//! and this needed two rounds of this task to actually be true.** The
//! first round gave `verify_copy` a `Verified` return type and made
//! `commit_copy` require one, and its own commit said deleting the
//! comparison was now a build failure. That was wrong, and a re-review
//! measured why: `Verified` was a private struct sitting beside
//! `verify_copy` and `commit_copy` in this same module, so
//! `let _ = same_bytes(a, b)?;` -- calling the comparison and throwing its
//! verdict away -- still built one, passed `cargo clippy -D warnings`, and
//! passed every test. What actually caught a deletion was the comparison
//! function going unused, a `dead_code` lint any trivial call silences; it
//! said nothing about what the call did with its answer. `Verified` now
//! lives in its own module, [`verified`], with [`verified::compare`] the
//! only function inside it that can see the private field its lone
//! constructor needs. No code outside that module can write `Verified(())`
//! or otherwise produce one -- not `verify_copy`, not `commit_copy`, not
//! whatever gets written beside them next -- so both the old bypasses fail
//! to compile now, not just to lint clean. What `tests/relocate.rs`
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
//! tests pin preparation and publication separately, and exercise rollback
//! both before and after the new directory takes its final name. A killed
//! process can leave a temporary sibling behind; cleanup cannot run after
//! process death. No transaction across origin, destination and registry
//! is claimed.
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

    // Step 4's second half, and the reason it sits here rather than any
    // later: with the identity in hand, and before a byte has been copied
    // anywhere, whether this folder is the one the project lives in at
    // all. Read under the lock step 3 already holds, through the registry
    // itself rather than any reading of it this module keeps -- and read
    // once here, spent again at step 7, so a folder with nowhere to record
    // a move still fails there, exactly as it always has.
    let store_dir = crate::store::store_dir();
    if let Some(elsewhere) = store_dir
        .as_deref()
        .and_then(|d| crate::registry::path_elsewhere(d, &first_event_id, &located.root))
    {
        return Err(copy_refusal(elsewhere.name.as_deref()));
    }

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
    // (`t594`).
    let destination_vivac = destination_abs.join(crate::store::DIR);
    let mut written = Written::prepare(&destination_vivac)?;
    let active_dir = written.active_dir.clone();
    let (log_tmp, config_tmp, log_verified, config_verified) =
        match verify_copy(&origin_vivac, &active_dir, &mut written) {
            Ok(v) => v,
            Err(e) => {
                written.undo();
                return Err(e);
            }
        };
    if let Err(e) = commit_copy(
        log_verified,
        config_verified,
        &log_tmp,
        &config_tmp,
        &origin_vivac,
        &active_dir,
        &mut written,
    ) {
        written.undo();
        return Err(e);
    }
    if let Err(e) = written.publish() {
        written.undo();
        return Err(Failure::Io(e));
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
    let Some(store_dir) = store_dir else {
        written.undo();
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
        written.undo();
        return Err(Failure::Io(e));
    }

    // Step 8. The origin's own bookkeeping. Past step 7 there is nothing
    // left to roll back, so a failure partway through here is reported
    // with what it actually means, not the bare IO error underneath it --
    // see `write_origin_bookkeeping`'s own doc for the order, and why it
    // is safe to be interrupted anywhere in it.
    if let Err(e) = write_origin_bookkeeping(&origin_vivac, &stays_lane, first_event_id.clone()) {
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
        &first_event_id,
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
    // `f717`/`d874`: the harness to name comes from files already on
    // `cwd`'s own disk, not from a guess -- a person set up for Codex used
    // to be told to run `vivac setup claude-code` regardless, sent to
    // hooks that were never theirs. Read at `cwd`, the origin's own
    // folder (step 1 already pinned it there), never at `destination`:
    // the origin is what a harness was actually set up against, and the
    // destination has nothing of its own yet. When neither marker is
    // found both commands are still offered, as a choice rather than a
    // guess at which one applies.
    let found = detected_setup_words(cwd);
    let advice_words: Vec<&str> = if found.is_empty() {
        vec!["claude-code", "codex"]
    } else {
        found.clone()
    };
    outln!("  The new folder holds the tree and answers as a lane of its own. To give");
    if found.is_empty() {
        outln!("  it the hooks, the server and the skill, run the one for your harness:");
    } else {
        outln!("  it the hooks, the server and the skill:");
    }
    for word in &advice_words {
        outln!("    vivac setup {word}");
    }
    outln!("  Restart any session open on this tree.");
    if !marked {
        if advice_words.len() == 1 {
            outln!("  The moved tree could not be marked from here. Run this in it:");
        } else {
            outln!("  The moved tree could not be marked from here. Run one of these in it:");
        }
        for word in &advice_words {
            outln!("    vivac setup {word}");
        }
    }
    Ok(0)
}

/// `f717`/`d874`: which harness's setup command step 10 names, read from
/// files a harness itself already wrote at `dir` -- never guessed from
/// which one happens to be running this process, since `relocate` is a
/// binary that any harness can invoke the same way. Two independent
/// checks rather than one, because a folder can carry both, and neither
/// crowds the other out. Claude Code first, then Codex, when both are
/// found (`d874`'s own order); an empty list means neither is, and `run`
/// falls back to naming both as a choice.
fn detected_setup_words(dir: &Path) -> Vec<&'static str> {
    let mut words = Vec::new();
    if dir.join(".claude").join("settings.json").is_file() {
        words.push("claude-code");
    }
    if dir.join(".codex").join("hooks.json").is_file() {
        words.push("codex");
    }
    words
}

/// The refusal step 4 raises for a move made from a copy: this folder
/// holds a tree that starts with the same first event as the one the
/// registry points at, and that other folder is still there holding it.
///
/// Refused outright rather than recorded on the way past. Adding the other
/// folder to `copies` would fix the warning and leave the harm exactly
/// where it was: `path` would still move to this copy's own destination,
/// and every lane that finds its tree through the registry would follow it
/// there and append to it -- the silent divergence `t594` §4.7 exists to
/// prevent, reached through this command rather than around it. What lands
/// in the wrong tree is a write, so the path is cut, not signposted.
///
/// The other folder is named, and a name the redaction guard rejects is
/// not written down at all (`d600`): `label_for` is the same fallback
/// `setup`'s own tree-below refusal reads through.
fn copy_refusal(name: Option<&str>) -> Failure {
    let label = crate::registry::label_for(name);
    Failure::Model(format!(
        "  This folder is a copy of the tree in {label}, so the project does not live\n  \
         here. Moving this copy would point every lane at it and leave that tree\n  \
         behind: run relocate in {label} instead."
    ))
}

/// `destination` made absolute against `cwd`, by joining rather than
/// `canonicalize`: the destination usually does not exist yet, and
/// canonicalizing a path that is not there fails. Then normalized
/// lexically -- `.` and `..` resolved one component at a time, no disk
/// access -- which is not cosmetic: `vivac relocate ..` used to leave the
/// registry holding `…\clone\..` outright, and `Path::file_name` of a path
/// ending in `..` is `None`, so `render::project_name` read that back as
/// the bare word `"-"` rather than the folder's own name (`t594`) --
/// every project this ever ran on would have collided on
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
/// nothing else. `t594`: an earlier round of this task
/// tore down the destination's whole `.vivac/` on any failure, which took
/// a destination's own `notes.txt` and an unrelated `events.relocated`
/// from an earlier move along with it.
struct Written {
    active_dir: PathBuf,
    final_dir: PathBuf,
    fresh: bool,
    published: bool,
    files: Vec<PathBuf>,
}

impl Written {
    fn prepare(final_dir: &Path) -> std::io::Result<Self> {
        let fresh = match std::fs::symlink_metadata(final_dir) {
            Ok(metadata) if metadata.file_type().is_dir() => false,
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the destination's .vivac path already exists",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => return Err(e),
        };
        let active_dir = if fresh {
            let parent = final_dir.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "destination has no parent",
                )
            })?;
            let staging = parent.join(format!(".vivac.{}.tmp", crate::id::ulid()));
            std::fs::create_dir(&staging)?;
            staging
        } else {
            final_dir.to_path_buf()
        };
        Ok(Self {
            active_dir,
            final_dir: final_dir.to_path_buf(),
            fresh,
            published: false,
            files: Vec::new(),
        })
    }

    fn publish(&mut self) -> std::io::Result<()> {
        self.publish_with_sync(crate::store::sync_dir)
    }

    fn publish_with_sync(
        &mut self,
        sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        if !self.fresh || self.published {
            return Ok(());
        }
        match std::fs::symlink_metadata(&self.final_dir) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the destination's .vivac path appeared before publication",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::fs::rename(&self.active_dir, &self.final_dir)?;
        for file in &mut self.files {
            *file = self.final_dir.join(file.file_name().unwrap());
        }
        self.active_dir = self.final_dir.clone();
        self.published = true;
        sync_parent(self.final_dir.parent().unwrap())
    }

    /// Removes every file this run wrote, then the `.vivac/` directory
    /// itself -- but only when this run created it, and only
    /// `remove_dir`, never `remove_dir_all`: it refuses on its own the
    /// moment anything besides these files is still inside, which is
    /// exactly the case this must leave alone.
    fn undo(&self) {
        for f in &self.files {
            std::fs::remove_file(f).ok();
        }
        if self.fresh {
            std::fs::remove_dir(&self.active_dir).ok();
        }
    }
}

/// `Verified`'s own module. `t594`: a unit test proved a
/// plain `bool`, or a `Verified` sitting as a private struct beside
/// `verify_copy` and `commit_copy` in `relocate` itself, is not enough --
/// `let _ = same_bytes(a, b)?;` called the comparison, threw its verdict
/// away, and still built a `Verified` right there, because Rust's privacy
/// is per-module and all three lived in the same one. Giving `Verified` a
/// module of its own, with [`compare`] as the only thing inside it that can
/// see the private field, closes that: no code outside this module can
/// write `Verified(())`, so the only way to hold one is to have called
/// `compare` and gotten `Some` back.
mod verified {
    use std::path::Path;

    /// Proof that two files were read back and compared byte for byte, and
    /// agreed. The field is private to this module, so the only way to
    /// hold one is to have done the comparison -- there is no black-box
    /// trigger for a mismatch, so the guarantee cannot be a test and has
    /// to be this.
    pub struct Verified(());

    /// `Ok(Some(Verified))` when `a` and `b` agree byte for byte,
    /// `Ok(None)` when they do not. The only function in the crate that
    /// can build a `Verified`.
    pub fn compare(a: &Path, b: &Path) -> std::io::Result<Option<Verified>> {
        if std::fs::read(a)? == std::fs::read(b)? {
            Ok(Some(Verified(())))
        } else {
            Ok(None)
        }
    }
}
use verified::Verified;

/// Step 6, first half: copies `events` and `config` from `origin_vivac`
/// into temporary names inside `destination_vivac`, and compares each
/// against the origin byte for byte. Returns the two temporary paths and a
/// `Verified` per pair on success, for `commit_copy` to spend.
fn verify_copy(
    origin_vivac: &Path,
    destination_vivac: &Path,
    written: &mut Written,
) -> Result<(PathBuf, PathBuf, Verified, Verified), Failure> {
    let log_tmp = destination_vivac.join(format!("events.{}.tmp", crate::id::ulid()));
    written.files.push(log_tmp.clone());
    std::fs::copy(origin_vivac.join(crate::store::LOG), &log_tmp)?;

    let config_tmp = destination_vivac.join(format!("config.{}.tmp", crate::id::ulid()));
    written.files.push(config_tmp.clone());
    std::fs::copy(origin_vivac.join(crate::store::CONFIG), &config_tmp)?;

    // The one comparison this whole operation's safety rests on: if the
    // copy did not end up with exactly what the origin has, nothing past
    // this point is trusted with either the tree's real name or the
    // origin's own log -- and with no `Verified` pair to hand
    // `commit_copy`, nothing past this point can even compile a call to
    // it.
    let log_verified = verified::compare(&origin_vivac.join(crate::store::LOG), &log_tmp)?;
    let config_verified = verified::compare(&origin_vivac.join(crate::store::CONFIG), &config_tmp)?;
    let (Some(log_verified), Some(config_verified)) = (log_verified, config_verified) else {
        return Err(Failure::Io(std::io::Error::other(
            "the copy at the destination did not match the source byte for byte",
        )));
    };

    Ok((log_tmp, config_tmp, log_verified, config_verified))
}

/// Step 6, second half: writes `.gitignore` and a fresh `lock`, then
/// renames the two temporary files `verify_copy` already checked onto the
/// names `store::already_planted` looks at. Only reachable with both
/// `Verified` values in hand -- see [`verified`]'s own doc.
///
/// A `.gitignore` the destination already had is left exactly as it was,
/// the same promise `store::write_gitignore` already makes for the branch
/// that writes one from nothing: `t594`, a rollback that
/// deleted one the destination brought with it because this used to copy
/// over it and track the result as its own regardless. `t594` applies
/// the same rule to `lock`: a destination that already
/// had one keeps it, rather than a rollback deleting a lock this run never
/// created.
fn commit_copy(
    _log_verified: Verified,
    _config_verified: Verified,
    log_tmp: &Path,
    config_tmp: &Path,
    origin_vivac: &Path,
    destination_vivac: &Path,
    written: &mut Written,
) -> Result<(), Failure> {
    let destination_gitignore = destination_vivac.join(crate::store::GITIGNORE);
    if !destination_gitignore.is_file() {
        write_new_gitignore(&destination_gitignore, written, |destination| {
            use std::io::Write;
            let origin_gitignore = origin_vivac.join(crate::store::GITIGNORE);
            if origin_gitignore.is_file() {
                let mut source = std::fs::File::open(origin_gitignore)?;
                std::io::copy(&mut source, destination)?;
                destination.set_permissions(source.metadata()?.permissions())?;
            } else {
                destination.write_all(b"*\n")?;
            }
            Ok(())
        })?;
    }

    let lock_path = destination_vivac.join(crate::store::LOCK);
    if !lock_path.is_file() {
        if written.fresh {
            written.files.push(lock_path.clone());
        }
        std::fs::File::create(&lock_path)?;
        if !written.fresh {
            written.files.push(lock_path);
        }
    }

    // Both temporaries already hold whole, verified copies, so they are
    // synced rather than rewritten, and only then take their final names
    // (`d946`).
    crate::store::sync_file(log_tmp)?;
    crate::store::sync_file(config_tmp)?;

    let log_final = destination_vivac.join(crate::store::LOG);
    std::fs::rename(log_tmp, &log_final)?;
    written.files.push(log_final);

    let config_final = destination_vivac.join(crate::store::CONFIG);
    std::fs::rename(config_tmp, &config_final)?;
    written.files.push(config_final);

    crate::store::sync_dir(destination_vivac)?;

    Ok(())
}

fn write_new_gitignore(
    path: &Path,
    written: &mut Written,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    written.files.push(path.to_path_buf());
    write(&mut file)?;
    Ok(())
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
/// - Its own `.vivac/lane` and `lane.declared` (`d681`): a fresh id from
///   `lane::new_id()`, and a name taken from `destination`'s own folder
///   the same way `setup::claude_code::plan_lane` names any other lane
///   from a folder, through `lane::declared_name` -- the one place that
///   rule lives, reused here rather than repeated. No repositories: a
///   lane with none declared yet is ordinary here, and scanning for them
///   is `setup`'s job, not this one's, the first time it runs at the
///   destination. Without this the destination carried no lane file of
///   its own, `store::locate` there fell to the implicit `main` rule, and
///   `main` ended up answering from both folders at once.
/// - `lane.claimed` for `main`, only when the origin was the implicit
///   founding lane (`was_implicit_main`): `main` is retired there, so a
///   later `setup` run at the destination mints it a lane of its own
///   instead of reclaiming a name that already belongs elsewhere
///   (`setup::claude_code::plan_lane`'s own sixth case).
/// - `lane.declared` for `stays_lane`, when `--lane-name` renamed it, or
///   when the origin already had repositories of its own and needs its
///   declaration carried forward under the very same lane id.
///
/// The destination's own declaration is signed as itself; the other two
/// are signed as `stays_lane`, the folder that stays announcing this
/// about itself -- the same convention `ops::declare_lane` already uses.
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
    first_event_id: &str,
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

    let mut stays_bodies = Vec::new();
    if was_implicit_main {
        stays_bodies.push(Body::LaneClaimed {
            lane: crate::lane::MAIN.to_string(),
        });
    }
    if let Some(name) = redeclare_name {
        stays_bodies.push(Body::LaneDeclared {
            lane: stays_lane.to_string(),
            name,
            repos: existing_repos,
        });
    }

    // The destination's own identity (`d681`): every folder that ends up
    // holding a tree needs a lane file of its own, or resolution there
    // falls to the implicit `main` rule -- the very lane the origin just
    // kept. The file goes down before the event that announces it, the
    // same ordering `write_lane` in `setup/claude_code.rs` already keeps
    // for the same reason: an event with no file behind it would leave
    // this folder not knowing whose thread it is.
    let own_id = crate::lane::new_id();
    let folder_name = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let own_name = crate::lane::declared_name(&own_id, &folder_name);
    crate::lane::write(
        &destination.join(crate::store::DIR),
        &crate::lane::Lane {
            version: 1,
            id: own_id.clone(),
            project: first_event_id.to_string(),
        },
    )?;

    let mut moved = Store::open(destination.to_path_buf())?;
    let moved_lock = moved.lock_for_write()?;
    let mut next_seq = tree.seq;
    if !stays_bodies.is_empty() {
        let appended = moved.append(
            &moved_lock,
            stays_lane,
            stays_bodies,
            next_seq,
            tree.has_governance,
        )?;
        next_seq = appended.events.last().map_or(next_seq, |e| e.seq);
    }
    moved.append(
        &moved_lock,
        &own_id,
        vec![Body::LaneDeclared {
            lane: own_id.clone(),
            name: own_name,
            repos: Vec::new(),
        }],
        next_seq,
        tree.has_governance,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id;

    #[test]
    fn failed_gitignore_write_removes_partial_file_and_preserves_existing_files() {
        use std::io::Write;

        let root = temp_dir("partial-gitignore");
        seeded_located(&root);
        let origin_vivac = root.join(crate::store::DIR);
        let source_files: Vec<_> = [
            crate::store::LOG,
            crate::store::CONFIG,
            crate::store::GITIGNORE,
        ]
        .into_iter()
        .map(|name| (name, std::fs::read(origin_vivac.join(name)).unwrap()))
        .collect();
        let destination_vivac = root.join("destination").join(crate::store::DIR);
        std::fs::create_dir_all(&destination_vivac).unwrap();
        for name in ["notes.txt", "events.relocated"] {
            std::fs::write(destination_vivac.join(name), name.as_bytes()).unwrap();
        }
        let mut written = Written::prepare(&destination_vivac).unwrap();
        verify_copy(&origin_vivac, &destination_vivac, &mut written).unwrap();
        let gitignore = destination_vivac.join(crate::store::GITIGNORE);

        let error = write_new_gitignore(&gitignore, &mut written, |file| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("gitignore write failed"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "gitignore write failed");
        assert_eq!(std::fs::read(&gitignore).unwrap(), b"partial");
        written.undo();

        assert!(
            !gitignore.exists(),
            "rollback must remove a partially written .gitignore"
        );
        assert_eq!(std::fs::read_dir(&destination_vivac).unwrap().count(), 2);
        for name in ["notes.txt", "events.relocated"] {
            assert_eq!(
                std::fs::read(destination_vivac.join(name)).unwrap(),
                name.as_bytes()
            );
        }
        for (name, bytes) in source_files {
            assert_eq!(std::fs::read(origin_vivac.join(name)).unwrap(), bytes);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_gitignore_copies_source_bytes_and_permissions_or_uses_fallback() {
        for copy_source in [true, false] {
            let root = temp_dir("gitignore-content");
            seeded_located(&root);
            let origin_vivac = root.join(crate::store::DIR);
            let source = origin_vivac.join(crate::store::GITIGNORE);
            let expected = if copy_source {
                let bytes = b"*\n!retained\n".as_slice();
                std::fs::write(&source, bytes).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640))
                        .unwrap();
                }
                bytes
            } else {
                std::fs::remove_file(&source).unwrap();
                b"*\n".as_slice()
            };
            let destination_vivac = root.join("destination").join(crate::store::DIR);
            std::fs::create_dir_all(&destination_vivac).unwrap();

            let written = committed_copy(&origin_vivac, &destination_vivac);
            let destination = destination_vivac.join(crate::store::GITIGNORE);

            assert_eq!(std::fs::read(&destination).unwrap(), expected);
            if copy_source {
                let source_permissions = std::fs::metadata(&source).unwrap().permissions();
                let destination_permissions =
                    std::fs::metadata(&destination).unwrap().permissions();
                assert_eq!(
                    source_permissions.readonly(),
                    destination_permissions.readonly()
                );
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    assert_eq!(source_permissions.mode(), destination_permissions.mode());
                }
            }
            written.undo();
            assert!(!destination.exists());
            if copy_source {
                assert_eq!(std::fs::read(&source).unwrap(), expected);
            } else {
                assert!(!source.exists());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn gitignore_reservation_collision_never_tracks_or_changes_the_existing_file() {
        for fresh in [true, false] {
            let root = temp_dir("gitignore-collision");
            std::fs::create_dir(&root).unwrap();
            let destination_vivac = root.join(crate::store::DIR);
            if !fresh {
                std::fs::create_dir(&destination_vivac).unwrap();
            }
            let mut written = Written::prepare(&destination_vivac).unwrap();
            let gitignore = written.active_dir.join(crate::store::GITIGNORE);
            std::fs::write(&gitignore, b"keep this file\n").unwrap();

            let error = write_new_gitignore(&gitignore, &mut written, |_| {
                panic!("a failed reservation must not call the writer")
            })
            .unwrap_err();

            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            assert!(written.files.is_empty());
            written.undo();
            assert_eq!(std::fs::read(&gitignore).unwrap(), b"keep this file\n");
            std::fs::remove_dir_all(root).unwrap();
        }
    }

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

    #[test]
    fn preparing_a_copy_keeps_the_destination_unpublished_and_its_ancestor_visible() {
        let ancestor = temp_dir("unpublished-copy");
        seeded_located(&ancestor);
        let origin = ancestor.join("origin");
        seeded_located(&origin);
        let destination = ancestor.join("destination");
        std::fs::create_dir_all(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = Written::prepare(&destination_vivac).unwrap();
        let active_dir = written.active_dir.clone();
        let (log_tmp, config_tmp, log_verified, config_verified) =
            verify_copy(&origin.join(crate::store::DIR), &active_dir, &mut written).unwrap();

        assert!(!destination_vivac.exists());
        commit_copy(
            log_verified,
            config_verified,
            &log_tmp,
            &config_tmp,
            &origin.join(crate::store::DIR),
            &active_dir,
            &mut written,
        )
        .unwrap();

        let published = destination_vivac.exists();
        let discovered = crate::store::find_root(&destination);
        std::fs::remove_dir_all(&ancestor).unwrap();

        assert!(
            !published,
            "preparing a copy must not publish the destination's .vivac directory"
        );
        assert_eq!(
            discovered.unwrap(),
            Some(ancestor),
            "an unpublished destination must still resolve to the real ancestor tree"
        );
    }

    fn committed_copy(origin: &Path, destination: &Path) -> Written {
        let mut written = Written::prepare(destination).unwrap();
        let active_dir = written.active_dir.clone();
        let (log_tmp, config_tmp, log_verified, config_verified) =
            verify_copy(origin, &active_dir, &mut written).unwrap();
        commit_copy(
            log_verified,
            config_verified,
            &log_tmp,
            &config_tmp,
            origin,
            &active_dir,
            &mut written,
        )
        .unwrap();
        written
    }

    #[test]
    fn publishing_a_copy_exposes_the_complete_tree_and_undo_preserves_foreign_files() {
        let ancestor = temp_dir("publish-copy");
        seeded_located(&ancestor);
        let origin = ancestor.join("origin");
        seeded_located(&origin);
        let origin_vivac = origin.join(crate::store::DIR);
        let destination = ancestor.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = committed_copy(&origin_vivac, &destination_vivac);
        let staging = written.active_dir.clone();

        written.publish().unwrap();

        assert!(!staging.exists());
        assert_eq!(written.active_dir, destination_vivac);
        assert_eq!(
            crate::store::find_root(&destination).unwrap(),
            Some(destination.clone())
        );
        for name in [crate::store::LOG, crate::store::CONFIG] {
            assert_eq!(
                std::fs::read(destination_vivac.join(name)).unwrap(),
                std::fs::read(origin_vivac.join(name)).unwrap()
            );
        }
        let foreign = destination_vivac.join("notes.txt");
        std::fs::write(&foreign, b"keep").unwrap();

        written.undo();

        assert_eq!(std::fs::read(&foreign).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&destination_vivac).unwrap().count(), 1);
        assert!(origin_vivac.join(crate::store::LOG).is_file());
        std::fs::remove_file(foreign).unwrap();
        written.undo();
        assert!(!destination_vivac.exists());
        std::fs::remove_dir_all(ancestor).unwrap();
    }

    #[test]
    fn failed_parent_sync_after_publication_cleans_the_final_directory() {
        let root = temp_dir("parent-sync-failed");
        seeded_located(&root);
        let origin_vivac = root.join(crate::store::DIR);
        let log = std::fs::read(origin_vivac.join(crate::store::LOG)).unwrap();
        let config = std::fs::read(origin_vivac.join(crate::store::CONFIG)).unwrap();
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = committed_copy(&origin_vivac, &destination_vivac);
        let staging = written.active_dir.clone();
        let mut sync_called = false;

        let result = written.publish_with_sync(|parent| {
            sync_called = true;
            assert_eq!(parent, destination.as_path());
            assert!(!staging.exists());
            assert_eq!(
                std::fs::read(destination_vivac.join(crate::store::LOG)).unwrap(),
                log
            );
            assert_eq!(
                std::fs::read(destination_vivac.join(crate::store::CONFIG)).unwrap(),
                config
            );
            Err(std::io::Error::other("parent sync failed"))
        });

        assert!(sync_called);
        assert!(result.is_err());
        assert_eq!(written.active_dir, destination_vivac);
        written.undo();

        assert!(!destination_vivac.exists());
        assert!(!staging.exists());
        assert_eq!(
            std::fs::read(origin_vivac.join(crate::store::LOG)).unwrap(),
            log
        );
        assert_eq!(
            std::fs::read(origin_vivac.join(crate::store::CONFIG)).unwrap(),
            config
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_refuses_a_destination_that_appeared_and_cleans_only_the_staging_copy() {
        let root = temp_dir("publish-collision");
        seeded_located(&root);
        let origin_vivac = root.join(crate::store::DIR);
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = committed_copy(&origin_vivac, &destination_vivac);
        let staging = written.active_dir.clone();
        std::fs::create_dir(&destination_vivac).unwrap();
        let foreign = destination_vivac.join("notes.txt");
        std::fs::write(&foreign, b"keep").unwrap();

        assert_eq!(
            written.publish().unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        written.undo();

        assert!(!staging.exists());
        assert_eq!(std::fs::read(foreign).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&destination_vivac).unwrap().count(), 1);
        assert!(origin_vivac.join(crate::store::LOG).is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_copy_cleans_the_staging_directory_without_publishing() {
        let root = temp_dir("copy-failed");
        seeded_located(&root);
        let origin_vivac = root.join(crate::store::DIR);
        std::fs::remove_file(origin_vivac.join(crate::store::CONFIG)).unwrap();
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = Written::prepare(&destination_vivac).unwrap();
        let staging = written.active_dir.clone();

        assert!(verify_copy(&origin_vivac, &staging, &mut written).is_err());
        written.undo();

        assert!(!staging.exists());
        assert!(!destination_vivac.exists());
        assert!(origin_vivac.join(crate::store::LOG).is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preparation_refuses_an_existing_file_without_creating_a_staging_directory() {
        let root = temp_dir("prepare-failed");
        std::fs::create_dir(&root).unwrap();
        let destination_vivac = root.join(crate::store::DIR);
        std::fs::write(&destination_vivac, b"keep").unwrap();

        assert!(Written::prepare(&destination_vivac).is_err());

        assert_eq!(std::fs::read(&destination_vivac).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_commit_cleans_the_staging_directory_without_publishing() {
        let root = temp_dir("commit-failed");
        seeded_located(&root);
        let origin_vivac = root.join(crate::store::DIR);
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_vivac = destination.join(crate::store::DIR);
        let mut written = Written::prepare(&destination_vivac).unwrap();
        let staging = written.active_dir.clone();
        let (log_tmp, config_tmp, log_verified, config_verified) =
            verify_copy(&origin_vivac, &staging, &mut written).unwrap();
        std::fs::remove_file(&log_tmp).unwrap();

        assert!(commit_copy(
            log_verified,
            config_verified,
            &log_tmp,
            &config_tmp,
            &origin_vivac,
            &staging,
            &mut written,
        )
        .is_err());
        written.undo();

        assert!(!staging.exists());
        assert!(!destination_vivac.exists());
        assert!(origin_vivac.join(crate::store::CONFIG).is_file());
        assert!(origin_vivac.join(crate::store::LOG).is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Gives `store::store_dir()` a private, real answer for as long as
    /// this value lives: `run` calls it directly for step 7's registry
    /// write, with no parameter to hand it a path instead, and under
    /// `cfg(test)` it now refuses outright to answer with this machine's
    /// real home (`t594`) -- the very thing this exists to avoid
    /// needing in the first place.
    ///
    /// `std::env::set_var` is process-global and the test harness runs
    /// threads in parallel, so two of these racing would be worse than no
    /// test at all (`store::resolve_store_dir`'s own doc says the same);
    /// this serializes every caller through one lock, and restores
    /// whatever `VIVAC_HOME` said before on drop rather than leaving it
    /// pointed at a directory this test is about to delete.
    struct IsolatedVivacHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        home: std::path::PathBuf,
        previous: Option<std::ffi::OsString>,
    }

    impl IsolatedVivacHome {
        fn new(prefix: &str) -> IsolatedVivacHome {
            static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
            let guard = LOCK
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let home = temp_dir(prefix);
            let previous = std::env::var_os("VIVAC_HOME");
            std::env::set_var("VIVAC_HOME", &home);
            IsolatedVivacHome {
                _guard: guard,
                home,
                previous,
            }
        }
    }

    impl Drop for IsolatedVivacHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var("VIVAC_HOME", v),
                None => std::env::remove_var("VIVAC_HOME"),
            }
            std::fs::remove_dir_all(&self.home).ok();
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
        let _home = IsolatedVivacHome::new("old-process-vivac-home");
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
    fn compare_holds_a_verified_for_two_identical_files_and_none_for_two_that_differ() {
        let dir = temp_dir("compare-verified");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        let c = dir.join("c");
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, b"hello").unwrap();
        std::fs::write(&c, b"world").unwrap();

        assert!(
            verified::compare(&a, &b).unwrap().is_some(),
            "two identical files must agree"
        );
        assert!(
            verified::compare(&a, &c).unwrap().is_none(),
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
        // A file where a directory is needed: writing the destination's
        // own `.vivac/lane` underneath it cannot even create the path, so
        // this fails before ever reaching a lock.
        std::fs::write(&destination, b"not a directory").unwrap();
        let tree = Tree::default();

        let result = write_claim_and_declaration(
            &destination,
            &tree,
            true,
            crate::lane::MAIN,
            None,
            "01SEEDEVENTAAAAAAAAAAAAAAA",
        );

        assert!(
            result.is_err(),
            "a destination that cannot be opened must fail write_claim_and_declaration"
        );

        std::fs::remove_file(&destination).ok();
    }

    /// `t594` (B3): `--lane-name` with a value the
    /// redaction guard rejects must fall back rather than fail the move
    /// -- `lane::declared_name` already promises exactly this for every
    /// other caller that names a lane from a word a person typed, and
    /// this pins it for `relocate`'s own.
    #[test]
    fn a_lane_name_the_guard_rejects_falls_back_without_failing() {
        let _home = IsolatedVivacHome::new("lane-name-guard-vivac-home");
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

    /// `d681`: the destination is the folder that now holds the tree, and
    /// leaving it with no `.vivac/lane` of its own let `store::locate`
    /// fall to the implicit `main` rule there too -- the very lane the
    /// origin just kept. Both folders answered as `main` at once; reading
    /// from either did no harm, but writing from the destination would
    /// have signed its own work as a lane that actually lives at the
    /// origin.
    #[test]
    fn the_destination_answers_as_its_own_lane_and_the_origin_keeps_answering_as_main() {
        let _home = IsolatedVivacHome::new("own-lane-vivac-home");
        let origin = temp_dir("own-lane-origin");
        let located = seeded_located(&origin);
        let destination = temp_dir("own-lane-dest");

        let code = run(&located, &destination, None, &origin).unwrap();
        assert_eq!(code, 0);

        let at_destination = crate::store::locate(&destination).unwrap().unwrap();
        let destination_lane = at_destination
            .lane
            .expect("the destination must carry its own lane file");
        assert_ne!(
            destination_lane.id,
            crate::lane::MAIN,
            "the destination must not answer as the origin's own main"
        );

        let at_origin = crate::store::locate(&origin).unwrap().unwrap();
        assert_eq!(
            at_origin.lane.map(|l| l.id),
            Some(crate::lane::MAIN.to_string()),
            "the origin must keep answering as the lane it always was"
        );

        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&destination).ok();
    }
}
