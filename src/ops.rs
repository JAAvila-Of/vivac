//! The operations that write. Every one goes through the redaction guard.
//!
//! All but one refuse outright when it finds something. The exception is the
//! session opening: it is written by a hook, with no author present to reword
//! anything, so it replaces the field with the name of the rule that refused
//! it and writes the seam anyway. Losing the seam would cost more than the
//! field is worth, and a refusal that is recorded is not a refusal in silence.
//!
//! Capture hangs off the seams of the work, never off a judgement of
//! relevance. That is the one thing actually measured: over 170 real minutes,
//! `push`/`pop` --which cannot be skipped without leaving the work half done--
//! were called nine times, and the operation that asked "is this worth
//! keeping?" was called zero times, under a protocol declared mandatory.

use crate::anchor::{self, Anchor};
use crate::event::{Against, Arm, Body, Event, Flag, Kind, State, VivacKind};
use crate::failure::{Failure, R};
use crate::model::{fold, Node, Tree};
use crate::outcome::{self, Outcome};
use crate::params;
use crate::store::Store;
use crate::{id, redact};
use std::path::{Path, PathBuf};

/// Whose thread the context answers from, decided by the caller because
/// only the caller knows where the command was run.
pub enum Whose<'a> {
    /// What `store::locate` answered for the folder the command ran in.
    Resolved(&'a crate::store::Located),
    /// No resolution to go on: another project's tree read from outside,
    /// or a test. Answers as the founding lane, which is what every tree
    /// that exists today is, and never writes from anywhere else.
    Founding,
    /// The caller already decided which lane this is, and signs with it
    /// outright: `setup`, declaring a lane from its own `repos::scan`
    /// rather than from what the log already says. `t594` fix-1, finding
    /// G: without this, the only way to have `setup` sign as the lane it
    /// just planned was to build with `Founding` and overwrite `lane`,
    /// `tree` and `store` by hand afterwards -- an invariant that held
    /// only because someone remembered to keep the three assignments in
    /// step with `finish`. Never pending, and exempt from §6.9
    /// (`lock_for_write`): a folder the caller already named outright has
    /// nothing left for that refusal to protect, and `setup` is precisely
    /// the command §6.9's own message sends you to.
    Declared(String),
}

/// A linked worktree that is nobody's lane yet. It becomes one the first
/// time anything writes from it, and not before: a worktree the harness
/// created and will throw away should not leave a lane behind for having
/// been looked at.
pub struct PendingLane {
    /// The worktree's own root, where its `.vivac/lane` goes.
    pub dir: PathBuf,
    /// The folder's name, **raw**: the redaction guard runs in `emit`
    /// instead of here (`t594` fix-1, finding F), because only there does
    /// the id that `lane::name_for` decorates the fallback with already
    /// exist. Never written to disk unguarded -- `emit` is the only
    /// reader, and it never forwards this without checking it first.
    pub name: String,
    /// Always `{ path: ".", root }`: a worktree is one repository, itself.
    /// `root` is the root commit **the lane that holds this repository
    /// already declared**, never asked of git again -- the datum is in the
    /// log, and this runs on the write path.
    pub repo: crate::event::Repo,
}

/// The view a pending lane's own `Ctx` reads from until it joins: a key
/// that can never be a real lane's (`lane::MAIN`, or a ULID `lane::new_id`
/// mints), so `Tree::state` and everything built on it answers empty
/// rather than as whatever `main` happens to hold. `t594` §2.3, rule 3:
/// "its stack is empty, its focus does not exist and the brief has no
/// HERE" is exactly what `Tree::for_lane` already gives a lane nobody
/// wrote to; this only has to pick a key nobody ever will.
const PENDING_VIEW: &str = "";

pub struct Ctx {
    pub store: Store,
    /// The lane this context runs as: `Some(lane::MAIN)` for a tree's own
    /// folder, the lane's own id for any other, and `None` for a linked
    /// worktree that has not joined the tree yet (`pending_lane` is what
    /// tells the two apart -- the founding lane is never `None`, `t594`
    /// §2.3). Set by every constructor, and applied to `store` there too:
    /// the store is what signs every event, so a `Ctx` and the events it
    /// writes never disagree about whose thread they are.
    pub lane: Option<String>,
    pub tree: Tree,
    pub anchor: Box<dyn Anchor>,
    /// The log's fingerprint taken **before** it was read: a write that
    /// lands between that read and `lock_for_write` changes it, and a
    /// write that landed just before is folded in anyway and only costs
    /// one needless reload.
    pub seen: (u64, Option<std::time::SystemTime>),
    /// What the last `emit` wrote, so a caller keeping a resident tree
    /// never has to read the log back to learn where its own writes
    /// landed (`f599`).
    pub wrote: Option<crate::store::Appended>,
    /// The write lock, once `lock_for_write` has taken it. `None` until
    /// then, and set back to `None` by `unlock`.
    pub lock: Option<crate::store::WriteLock>,
    /// A linked worktree decided (`resolve_whose`) to be nobody's lane yet.
    /// Read and cleared inside `emit`, which mints the id, writes the
    /// file and declares it in the very same append -- and nowhere
    /// earlier: `lock_for_write` runs before an operation knows whether
    /// it has anything to write at all, and joining there left a lane
    /// file and a `lane.declared` behind a command that changed nothing
    /// (`t594` fix-1, finding B).
    pending_lane: Option<PendingLane>,
    /// Whether the caller already decided this lane (`Whose::Declared`)
    /// rather than it being resolved from a folder. §6.9's refusal
    /// (`lock_for_write`) exists for a folder that resolved to `main` on
    /// its own; it does not apply here, because the one caller that ever
    /// sets this is `setup`, the command §6.9's own message names as the
    /// way out (`t594` fix-1, finding D).
    caller_declared: bool,
}

impl Ctx {
    /// The only place `self.tree` is ever replaced. Whatever rebuilt it --
    /// a fresh fold, a reload from the index, one folded from events
    /// already read -- the context keeps looking from its own lane:
    /// `self.lane` is the folder's and does not move just because the tree
    /// underneath it did. Two call sites used to assign `self.tree`
    /// directly and disagreed about this, one of them only under lock
    /// contention (`t594` task 6, review round 1): a reload nobody routed
    /// through here answers from `main` while the store keeps signing as
    /// whatever lane this context actually is.
    fn adopt(&mut self, tree: Tree) {
        self.tree = tree;
        match &self.lane {
            Some(l) => self.tree.for_lane(l),
            // A worktree waiting to join reads from a key nobody has ever
            // written to, not from `main`'s: `t594` §2.3 rule 3 wants its
            // stack empty and its brief without a HERE, and this is what
            // gives it that without inventing a second way to ask for it.
            None if self.pending_lane.is_some() => self.tree.for_lane(PENDING_VIEW),
            None => {}
        }
    }

    /// Finishes building a `Ctx` once its tree is already folded and the
    /// lane it runs as is already decided. Shared by every constructor
    /// below. `store` carries no lane of its own to set here any more
    /// (`f608`, third time -- see `Store::append`'s own doc): `self.lane`
    /// is the only copy, and `emit` is what hands it to `append` on every
    /// write, so a `Ctx` and what it writes cannot drift apart the way a
    /// `Store` left holding a stale one could.
    fn finish(
        store: Store,
        tree: Tree,
        seen: (u64, Option<std::time::SystemTime>),
        lane: Option<String>,
        pending_lane: Option<PendingLane>,
        caller_declared: bool,
    ) -> Ctx {
        let anchor = anchor::detect(&store.root);
        let mut ctx = Ctx {
            store,
            lane,
            tree: Tree::default(),
            anchor,
            seen,
            wrote: None,
            lock: None,
            pending_lane,
            caller_declared,
        };
        ctx.adopt(tree);
        ctx
    }

    /// For a command that only ever reads. `LOADING.md` §4: this is a read,
    /// so it is free to refresh the derived index once its tail passes the
    /// threshold -- see `index::load`.
    pub fn load(store: Store, whose: Whose) -> Result<Ctx, Failure> {
        Ctx::load_opt(store, true, whose)
    }

    /// For a command that may append to the log. Still free to read a warm
    /// or stale index -- applying its tail is cheap enough for the write
    /// budget -- but it must never pay to rewrite the file itself
    /// (`LOADING.md` §4 "Cuándo se reescribe").
    pub fn load_for_write(store: Store, whose: Whose) -> Result<Ctx, Failure> {
        Ctx::load_opt(store, false, whose)
    }

    fn load_opt(store: Store, allow_index_refresh: bool, whose: Whose) -> Result<Ctx, Failure> {
        let seen = crate::store::fingerprint(&store.log());
        let tree = crate::index::load(&store, allow_index_refresh)?;
        // `t594` §2.3: whose lane a folder is needs the tree already
        // folded -- it depends on the repositories a lane declared, and
        // that is in the log -- so it is decided here, and nowhere else.
        // `t594` branch-fix-2 #1: read off the fold itself, not `config`'s
        // own sentence, which can say either more or less than the log
        // actually backs up (`Tree::has_a_declared_lane`'s own doc).
        let tree_has_lanes = tree.has_a_declared_lane();
        let (lane, pending_lane, caller_declared) = resolve_whose(whose, &tree, tree_has_lanes);
        Ok(Ctx::finish(
            store,
            tree,
            seen,
            lane,
            pending_lane,
            caller_declared,
        ))
    }

    /// Same read `changes` and `why` need, handing back the events instead
    /// of dropping them. Both need the log's own fields -- `actor`, `lane`,
    /// the exact payload -- which the derived index does not carry, so this
    /// always folds the whole log rather than going through `index::load`:
    /// there is no tail to apply that would save the read those two need
    /// anyway.
    pub fn load_with_log(store: Store, whose: Whose) -> Result<(Ctx, Vec<Event>), Failure> {
        let seen = crate::store::fingerprint(&store.log());
        let (events, broken) = store.read_all()?;
        let tree = fold(&events, broken);
        let tree_has_lanes = tree.has_a_declared_lane();
        let (lane, pending_lane, caller_declared) = resolve_whose(whose, &tree, tree_has_lanes);
        let ctx = Ctx::finish(store, tree, seen, lane, pending_lane, caller_declared);
        Ok((ctx, events))
    }

    /// A `Ctx` over events already read, for a caller that keeps them --
    /// `project.rs`'s resident tree among them. `t594` fix-1, finding A:
    /// this used to keep its own `Option<String>`, "answer as the founding
    /// lane" spelled as `None` rather than as `Whose::Founding` -- the
    /// exact ambiguity the enum exists to rule out -- and a worktree
    /// joined by the CLI kept answering as `main` the moment it was
    /// served by `vivac mcp` or `vivac web` instead. `Registry::open`
    /// decides the `Whose` for each root it opens, `Resolved` for the one
    /// the process started in and `Founding` for every other.
    pub fn from_events(
        store: Store,
        events: &[Event],
        broken: usize,
        seen: (u64, Option<std::time::SystemTime>),
        whose: Whose,
    ) -> Ctx {
        let tree = fold(events, broken);
        let tree_has_lanes = tree.has_a_declared_lane();
        let (lane, pending_lane, caller_declared) = resolve_whose(whose, &tree, tree_has_lanes);
        Ctx::finish(store, tree, seen, lane, pending_lane, caller_declared)
    }

    /// Replaces what this context knows about the tree -- the store handle, the
    /// folded tree and the fingerprint it was folded at -- without replacing the
    /// context itself. The write lock, which belongs to the caller's turn and
    /// not to the fold, survives: replacing the whole context mid-write would
    /// drop the lock on the floor and leave the write running with nothing
    /// holding the tree (`f602`). `wrote` does not survive -- it is the record
    /// of one particular write, and a fold triggered by a read that happens to
    /// run between two writes must not leave a stale one behind for the next
    /// `emit` to append to.
    pub fn refold(
        &mut self,
        store: Store,
        events: &[Event],
        broken: usize,
        seen: (u64, Option<std::time::SystemTime>),
    ) {
        self.store = store;
        // `adopt`, not a direct assignment: `self.lane` is the context's
        // own and does not move just because the tree underneath it did.
        self.adopt(fold(events, broken));
        self.anchor = anchor::detect(&self.store.root);
        self.seen = seen;
        self.wrote = None;
    }

    /// Takes the tree's write lock (`d598`) and brings the tree up to date.
    /// **Idempotent**: a `Ctx` that already holds it returns without taking it
    /// again, so an operation that locks on its own inside a caller that
    /// already locked does not wait five seconds for itself (`f602`).
    ///
    /// Returns whether *this* call is the one that took it. An operation that
    /// locks on its own releases only what it took: releasing a lock somebody
    /// above it is still holding would leave that caller writing with nothing
    /// holding the tree, which is the same bug the argument to `append` exists
    /// to make impossible.
    pub fn lock_for_write(&mut self) -> Result<bool, Failure> {
        if self.lock.is_some() {
            return Ok(false);
        }
        // `t594` §2.3 rule 3, and §6.9: a folder that holds the tree, has
        // no lane file of its own and answers as `main` only because
        // nothing said otherwise is fine -- until some other folder has
        // claimed `main` for itself (`lane.claimed`, `d597`), at which
        // point this one can still be read but must not write. Checked
        // before the lock is even taken, so a read never pays for it and a
        // refusal never has to let go of one. `caller_declared` exempts a
        // lane the caller already named outright (`Whose::Declared`):
        // `setup` is the only caller that ever sets it, and it is the
        // very command §6.9's own message sends you to -- refusing it too
        // would be a message that answers itself (`t594` fix-1, finding
        // D).
        if !self.caller_declared
            && self.lane.as_deref() == Some(crate::lane::MAIN)
            && self.tree.main_claimed
        {
            return Err(Failure::not_a_lane());
        }
        let lock = self.store.lock_for_write()?;
        let now = crate::store::fingerprint(&self.store.log());
        if now != self.seen {
            // `adopt`, not a direct assignment (`t594` task 6, review round 1):
            // this is the reload a second writer's append forces, and it
            // used to leave this context reading `main` while its store
            // kept signing as whatever lane it actually is.
            self.adopt(crate::index::load(&self.store, false)?);
            self.seen = now;
        }
        self.lock = Some(lock);
        Ok(true)
    }

    /// Whether this context currently holds the write lock. Only the tests
    /// read it, the same way only they read `Project::full_folds`: nothing
    /// else needs to ask, since every caller either took the lock itself or
    /// trusts the one that did.
    #[cfg(test)]
    pub fn holds_write_lock(&self) -> bool {
        self.lock.is_some()
    }

    /// Releases the write lock. The CLI never calls it -- the process ends and
    /// the operating system lets go -- and the resident server calls it after
    /// every write, because it outlives its own writes.
    pub fn unlock(&mut self) {
        self.lock = None;
    }

    /// Writes and **then applies in memory**, so that whatever gets printed
    /// next is the state after the operation and not the one before it.
    /// What is applied is what was written, stamp included, which is what a
    /// fresh fold of the log would apply (`f590`).
    fn emit(&mut self, bodies: Vec<Body>) -> R {
        let mut bodies = bodies;
        // `t594` §2.3 rule 3, paso 3, moved here in fix-1 round 1
        // (finding B): a worktree joins the moment something actually
        // writes, never merely because the write lock was taken.
        // `lock_for_write` runs before an operation even knows whether it
        // has anything to write -- `pop` on an empty stack, a `note`
        // naming an id that does not resolve, the redaction guard
        // refusing -- and joining there left a lane file, a
        // `lane.declared` and a locked config behind a command that
        // changed nothing. `emit` is the one place that is only ever
        // reached once there is something real to append, so joining
        // here and declaring it happen in the very same call: the two
        // land together or neither does.
        if let Some(pending) = &self.pending_lane {
            let dir = pending.dir.clone();
            let folder_name = pending.name.clone();
            let repo = pending.repo.clone();
            let lock = self.lock.as_ref().ok_or_else(|| {
                Failure::Io(std::io::Error::other("write without the tree's lock"))
            })?;
            // `t594` branch-fix-1 #3: `pending_lane` was decided before
            // this lock was even taken, and `lock_for_write`'s own reload
            // re-plays the tree but never asks again whose folder this
            // is. Two processes that both resolve pending before either
            // writes -- the `SessionStart` hook and the first `push` an
            // MCP client sends, a very ordinary pair -- would otherwise
            // each mint an id of their own; the file keeps the second,
            // and everything the first wrote -- its stack, its focus,
            // its counters -- sits behind an id nobody ever reads again,
            // invisible to `check`. Reading the file again here, with
            // the lock already held, is the one place left that can
            // still catch the other writer: if it is there now, this
            // adopts it and signs with it, rather than declaring a
            // second lane for the same folder.
            if let Some(joined) = crate::lane::read(&dir.join(crate::store::DIR))? {
                self.lane = Some(joined.id.clone());
                self.tree.for_lane(&joined.id);
                self.pending_lane = None;
            } else {
                self.store.lock_lanes_in_config(lock)?;
                // `t594` branch-fix-1 #2: a worktree only ever gets this
                // far when the tree already has a lane declared
                // somewhere, which means at least one event already
                // exists -- so there is nothing left to seed here, and
                // the question of whether an empty `repos` list would
                // have lied about one does not arise either.
                //
                // This should be unreachable now that `tree_has_lanes`
                // reads `Tree::has_a_declared_lane`, the fold itself,
                // rather than `config`'s own sentence (`t594`
                // branch-fix-2 #1): a declared lane is an event, so a
                // pending worktree can only exist once there is a first
                // one to read here. It was reachable when `config` was
                // the question instead -- `config` outliving a log a
                // crash or a hand-deleted `events` left with nothing in
                // it, `main` still claiming lanes existed when nothing
                // any more said which one. A `Failure` and not another
                // `expect`, so the day something moves this gate again
                // without moving this along with it, the answer is a
                // sentence and not a panic with a backtrace.
                let Some(project) = crate::store::first_event_id(&self.store.root) else {
                    return Err(Failure::Io(std::io::Error::other(
                        "This tree says a lane was declared, but its log has no first \
                         event to found a new one on. Run this from the tree's own \
                         folder first: an ordinary write there recovers a log that was \
                         deleted or emptied, the same way it always has.",
                    )));
                };
                let id = crate::lane::new_id();
                let lane_file = crate::lane::Lane {
                    version: 1,
                    id: id.clone(),
                    project,
                };
                crate::lane::write(&dir.join(crate::store::DIR), &lane_file)?;
                self.lane = Some(id.clone());
                self.tree.for_lane(&id);
                self.pending_lane = None;
                // `t594` fix-1, finding F: redacted here, not when the
                // pending lane was first noticed, because only here does
                // the id exist to decorate the fallback with. Goes
                // through `lane::declared_name`, the one place this rule
                // is written (`t594` branch-fix-1 #7), the same as
                // `setup` already does, so two redacted lanes on the
                // same tree no longer share the bare word `lane`.
                let name = crate::lane::declared_name(&id, &folder_name);
                bodies.insert(
                    0,
                    Body::LaneDeclared {
                        lane: id,
                        name,
                        repos: vec![repo],
                    },
                );
            }
        }
        // `d444`: the one bit `Store::append`'s own write-lock needs and
        // cannot see for itself -- whether this tree already has a pillar
        // or a rule, from a write before this one.
        let already_governed = self.tree.has_governance;
        let lock = self
            .lock
            .as_ref()
            .ok_or_else(|| Failure::Io(std::io::Error::other("write without the tree's lock")))?;
        let lane = self.lane.as_deref().unwrap_or(crate::lane::MAIN);
        let appended = self
            .store
            .append(lock, lane, bodies, self.tree.seq, already_governed)?;
        for e in &appended.events {
            self.tree.apply(e.seq, &e.ts, &e.lane, &e.payload);
        }
        self.seen = crate::store::fingerprint(&self.store.log());
        // Kept for a caller that maintains a resident tree (`f599`): more
        // than one `emit` can run under a single `Project::write`, so a
        // second append's events join the first's and its own offsets win.
        // An `emit` that wrote nothing -- `focus` and `restore` can, though
        // no MCP tool reaches either today -- leaves `wrote` exactly as it
        // was: there is nothing new to fold in, and an empty append's own
        // offsets would only overwrite a real one's with a no-op.
        if !appended.events.is_empty() {
            match self.wrote.take() {
                Some(mut w) => {
                    w.events.extend(appended.events);
                    w.last_line_offset = appended.last_line_offset;
                    w.end_offset = appended.end_offset;
                    self.wrote = Some(w);
                }
                None => self.wrote = Some(appended),
            }
        }
        Ok(())
    }

    fn resolve(&self, s: &str) -> Result<&crate::model::Node, Failure> {
        self.tree
            .resolve(s)
            .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))
    }
}

/// `t594` §2.3, paso 1: decides whose lane a folder is, once its tree is
/// already folded, and whether the caller already named it outright
/// (the third element -- `t594` fix-1, finding G). `Whose::Founding` and
/// `Whose::Declared` never have anything to decide -- there is no
/// `Located` to read a worktree off -- and never leave a lane pending.
/// Only `Declared` is exempt from §6.9 (`lock_for_write`): it is the one
/// case where the caller, not a resolved folder, is the reason this
/// answers as it does.
///
/// For `Whose::Resolved`, three cases:
///
/// 1. `Located.worktree` is `None`: the lane is `Located`'s, as it always
///    was before this task.
/// 2. It is `Some(w)` and `w` is one of the repositories the lane found
///    already declared: still `Located`'s lane. The worktree is that
///    lane's own repository at that path, and nothing more.
/// 3. It is `Some(w)`, it is not, **and `tree_has_lanes`**: `w` is another
///    lane, pending until it writes.
///
/// `tree_has_lanes` gates case 3 on purpose (`t594` branch-fix-1 #2): it
/// is the justification the task that built this already wrote down --
/// "only happens when the repository already declared a lane" -- and
/// never wired in. Without it, `session.started` alone -- a hook, not a
/// person, and one `.claude/settings.json` usually ships versioned so a
/// worktree the harness throws away can fire it before anyone runs
/// `setup` anywhere -- silently converts a tree nobody asked to convert.
/// With it, a worktree over a tree that has never had a lane declared
/// reads as `Located`'s lane, same as case 1, and writes nothing of its
/// own until a real lane exists to check it against.
fn resolve_whose(
    whose: Whose,
    tree: &Tree,
    tree_has_lanes: bool,
) -> (Option<String>, Option<PendingLane>, bool) {
    let located = match whose {
        Whose::Founding => return (Some(crate::lane::MAIN.to_string()), None, false),
        Whose::Declared(id) => return (Some(id), None, true),
        Whose::Resolved(l) => l,
    };
    let found_lane = located
        .lane
        .as_ref()
        .map(|l| l.id.clone())
        .unwrap_or_else(|| crate::lane::MAIN.to_string());
    let Some(w) = located.worktree.as_ref() else {
        return (Some(found_lane), None, false);
    };
    let declared: &[crate::event::Repo] = tree
        .lanes
        .get(&found_lane)
        .map(|s| s.repos.as_slice())
        .unwrap_or(&[]);
    if repo_at(declared, &located.lane_dir, w).is_some() {
        return (Some(found_lane), None, false);
    }
    if !tree_has_lanes {
        return (Some(found_lane), None, false);
    }
    // Paso 4: the root commit is whatever the lane already declared for
    // the repository whose `.git` is this worktree's `commondir` -- never
    // asked of git again, since the datum is already in the log and this
    // runs on the write path.
    let root = anchor::main_copy_of(w).and_then(|main_root| {
        repo_at(declared, &located.lane_dir, &main_root).and_then(|r| r.root.clone())
    });
    let folder_name = w
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (
        None,
        Some(PendingLane {
            dir: w.clone(),
            // `d600`'s own guard runs in `emit`, not here (`t594` fix-1,
            // finding F): see `PendingLane::name`.
            name: folder_name,
            repo: crate::event::Repo {
                path: ".".to_string(),
                root,
            },
        }),
        false,
    )
}

/// The declared repository, if any, whose path resolves to `target` once
/// joined to `lane_dir`: `anchor::main_copy_of`'s own criterion (`t594`
/// task 4). `anchor::same_folder` is what decides it -- this is the
/// comparison `f612` was first found in, between a path this process
/// joined by hand and one read out of files git itself wrote, so a case
/// difference or an 8.3 alias (what a Windows CI runner's own temp
/// directory actually handed out) makes the two disagree textually while
/// still naming the same folder.
fn repo_at<'a>(
    declared: &'a [crate::event::Repo],
    lane_dir: &Path,
    target: &Path,
) -> Option<&'a crate::event::Repo> {
    declared
        .iter()
        .find(|r| anchor::same_folder(&lane_dir.join(&r.path), target))
}

/// Builds a vivac out of the stack as it stands right now.
///
/// The `working_set` is **not measured**: measuring which files the pitch
/// touched would need a `post_tool` hook, which is not in Tier 0. It is
/// derived from the `governs` the stack declares, which is what there is, and
/// the `brief` says so rather than pretending it observed it.
fn vivac(
    ctx: &Ctx,
    kind: VivacKind,
    next_intent: &str,
    node_ref: Option<String>,
    label: &str,
) -> Body {
    let stack: Vec<(String, String)> = ctx
        .tree
        .stack()
        .iter()
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| (n.alias(), n.title(&ctx.tree).to_string()))
        .collect();
    let mut working_set: Vec<String> = ctx
        .tree
        .stack()
        .iter()
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .flat_map(|n| n.governs(&ctx.tree).into_iter().map(str::to_string))
        .collect();
    working_set.sort();
    working_set.dedup();
    Body::VivacCreated {
        vivac: id::ulid(),
        num: ctx.tree.next_vivac_num.max(1),
        kind,
        stack,
        working_set,
        next_intent: next_intent.to_string(),
        anchor: ctx.anchor.snapshot(),
        node_ref,
        label: label.to_string(),
    }
}

/// No text reaches the log without coming through here.
fn guard_text(fields: &[(&str, &str)]) -> R {
    match redact::check_fields(fields) {
        Some(h) => Err(Failure::Redaction(Box::new(h))),
        None => Ok(()),
    }
}

/// Text off an untrusted payload, made safe to store without failing the
/// write. What the guard objected to never gets in; what replaces it names
/// the rule and nothing else, so the log shows that a refusal happened
/// without repeating what caused it.
fn guarded_or_refused(field: &str, text: &str) -> String {
    match redact::check_field(field, text) {
        Some(f) => format!("refused: {}", f.rule),
        None => text.to_string(),
    }
}

/// `--root` given together with `--parent` on `add` or `decide`: both name
/// where a node is born, and a node is born in one place. `t533` §1.1.
fn root_and_parent_error() -> Failure {
    Failure::usage(
        "--root and --parent both say where it is born, and a node is born in one place.\n  \
         Keep the one you mean.",
    )
}

fn kind_of(raw: Option<&str>, fallback: Kind) -> Result<Kind, Failure> {
    match raw {
        None => Ok(fallback),
        Some(s) => Kind::parse(s)
            .ok_or_else(|| Failure::usage(format!("Unknown type: {s}. They are: {}", Kind::ALL))),
    }
}

/// The same guard `note` is held to: one line, not empty. `d415`.
fn validate_arm_text(s: &str) -> Result<(), Failure> {
    if s.trim().is_empty() {
        return Err(Failure::usage("An arm cannot be empty."));
    }
    if s.contains('\n') {
        return Err(Failure::usage(
            "An arm is one line: write it the way it would be typed.",
        ));
    }
    Ok(())
}

/// Is this slashed-and-unnormalized folder absolute? Checked on every
/// system regardless of which one is running, per `d441`: the log travels,
/// and a path only Windows would call absolute still carries this machine's
/// layout once it lands somewhere else.
fn is_absolute_arm_dir(slashed: &str) -> bool {
    if slashed.starts_with('/') || slashed.starts_with('~') {
        return true;
    }
    let mut chars = slashed.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

/// `./vivac/` -> `vivac`; `vivac\src` (already slashed to `vivac/src`) stays
/// `vivac/src`; `.`, `./` or `.\` (slashed to `./`) -> `.`. `d441`.
fn normalize_arm_dir(slashed: &str) -> String {
    let parts: Vec<&str> = slashed
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Checks 3 through 6 of `d441`'s six, shared by `--arm-dir` (`add`, `push`)
/// and `--dir` (`arm`): once a folder is known to have been given at all --
/// check 1 or 2, which differ in wording by caller -- absolute, `..`,
/// redaction and existence are the same check regardless of which flag
/// named it.
fn validate_arm_dir(raw: &str, tree_root: &Path) -> Result<String, Failure> {
    let slashed = raw.replace('\\', "/");
    if is_absolute_arm_dir(&slashed) {
        return Err(Failure::usage(
            "An arm's folder is relative to the one that holds .vivac: an \
             absolute path would write this machine's layout into the log.",
        ));
    }
    if slashed.split('/').any(|c| c == "..") {
        return Err(Failure::usage(
            "An arm's folder has to be inside the one that holds .vivac.",
        ));
    }
    let normalized = normalize_arm_dir(&slashed);
    guard_text(&[("dir", &normalized)])?;
    if !tree_root.join(&normalized).is_dir() {
        return Err(Failure::usage(format!(
            "There is no folder {normalized} inside the one that holds .vivac."
        )));
    }
    Ok(normalized)
}

/// The wording of the missing-folder and folder-without-arm messages, in the
/// two vocabularies a flag can be named in: the CLI's own `--flag`, and
/// MCP's bare argument name. §21: "the same texts, with `arm_dir` in place
/// of `--arm-dir`, `dir` in place of `--dir` and `arm` in place of `--arm`."
fn needs_arm_dir_message(via_mcp: bool) -> String {
    let flag = if via_mcp { "arm_dir" } else { "--arm-dir" };
    format!(
        "An arm needs {flag}: the folder it runs in, relative to the one \
         that holds .vivac. Use . for that folder itself."
    )
}

fn arm_dir_without_arm_message(via_mcp: bool) -> String {
    let (dir_flag, arm_flag) = if via_mcp {
        ("arm_dir", "arm")
    } else {
        ("--arm-dir", "--arm")
    };
    format!("{dir_flag} says where an arm runs, and no {arm_flag} was given.")
}

fn needs_dir_message(via_mcp: bool) -> String {
    let flag = if via_mcp { "dir" } else { "--dir" };
    format!(
        "An arm needs {flag}: the folder it runs in, relative to the one \
         that holds .vivac. Use . for that folder itself."
    )
}

/// `--arm`/`--arm-dir`, checked against the type it is born with. `d415`:
/// only a rule may carry one, and it may carry none -- a rule with no arm is
/// judged. `d441`: whenever one or more `--arm` are given, `--arm-dir` is
/// mandatory and names the one folder every arm in this call runs in.
/// `via_mcp` only ever changes which vocabulary the missing-folder messages
/// use, never the check itself.
fn arms_of(
    ctx: &Ctx,
    raw: Vec<String>,
    dir: Option<String>,
    kind: Kind,
    via_mcp: bool,
) -> Result<Vec<Arm>, Failure> {
    if !raw.is_empty() && kind != Kind::Rule {
        return Err(Failure::usage(format!(
            "Only a rule has an arm; this would be {}.",
            kind.with_article()
        )));
    }
    if raw.is_empty() {
        if dir.is_some() {
            return Err(Failure::usage(arm_dir_without_arm_message(via_mcp)));
        }
        return Ok(vec![]);
    }
    let dir = match dir {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Err(Failure::usage(needs_arm_dir_message(via_mcp))),
    };
    let normalized = validate_arm_dir(&dir, &ctx.store.root)?;
    for (i, a) in raw.iter().enumerate() {
        validate_arm_text(a)?;
        if raw[..i].contains(a) {
            return Err(Failure::usage(format!("The same arm is given twice: {a}")));
        }
    }
    Ok(raw
        .into_iter()
        .map(|command| Arm {
            dir: normalized.clone(),
            command,
        })
        .collect())
}

/// Splits one `--against` entry on the **first** `:`: the id to its left,
/// the sentence to its right, both trimmed. `t426` §2.1: the sentence may
/// carry more colons of its own.
fn split_against_entry(raw: &str) -> Result<(&str, &str), Failure> {
    let form_error =
        || Failure::usage("--against needs an id and a sentence: --against \"r12: why it holds\"");
    let (id, why) = raw.split_once(':').ok_or_else(form_error)?;
    let (id, why) = (id.trim(), why.trim());
    if id.is_empty() || why.is_empty() {
        return Err(form_error());
    }
    Ok((id, why))
}

/// `--against`, checked against what it points at. `t426` §2.1 and §2.2:
/// shared by `decide`, `push`, `add` and `declare`, since every one of them
/// judges an entry by the same questions -- only `push` and `add` ever
/// call it with a `kind` that is not already `Kind::Decision`, since a
/// decision is the only kind that may carry one.
///
/// Every check runs **before** anything is written, in order: the form of
/// each entry, that the id exists, that it names a pillar or a rule, that
/// it still governs, and that no id repeats within this one call.
fn against_of(ctx: &Ctx, raw: Vec<String>, kind: Kind) -> Result<Vec<Against>, Failure> {
    if !raw.is_empty() && kind != Kind::Decision {
        return Err(Failure::usage(format!(
            "--against goes on a decision, and this is {}",
            kind.with_article()
        )));
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut seen: Vec<u64> = Vec::with_capacity(raw.len());
    for entry in &raw {
        let (id, why) = split_against_entry(entry)?;
        let n = ctx
            .tree
            .resolve(id)
            .ok_or_else(|| Failure::usage(format!("No such node: {id}.")))?;
        if !matches!(n.kind, Kind::Pillar | Kind::Rule) {
            return Err(Failure::usage(format!(
                "--against points at a pillar or a rule, and {} is {}",
                n.alias(),
                n.kind.with_article()
            )));
        }
        if !n.state.is_open() {
            return Err(Failure::usage(format!(
                "--against points at what still governs, and {} is {}: vivac rules lists what does",
                n.alias(),
                n.state.word(n.kind)
            )));
        }
        if seen.contains(&n.num) {
            return Err(Failure::usage(format!(
                "--against names {} twice",
                n.alias()
            )));
        }
        seen.push(n.num);
        out.push(Against {
            node: n.id.clone(),
            why: why.to_string(),
        });
    }
    Ok(out)
}

/// What it takes to create a node, named rather than positional.
///
/// `title` and `why` stay borrowed rather than owned: every caller still
/// needs its own copy afterwards (a title goes into the vivac, `add`'s
/// `where_at` reads the parent, not this), so taking a slice costs nothing
/// and asking for an owned `String` here would just make each caller clone
/// one it already had.
struct Born<'a> {
    title: &'a str,
    why: &'a str,
    kind: Kind,
    parent: Option<String>,
    refs: Vec<String>,
    governs: Vec<String>,
    blocks: bool,
    arms: Vec<Arm>,
    /// Already validated by `against_of`. Empty for every kind that is not
    /// a decision, since only a decision may carry one.
    against: Vec<Against>,
}

/// Creates a node. Returns the event, the alias number assigned, and
/// whether the `against` key was written empty -- `d445`'s `no_against`,
/// which `push`, `add` and `decide` each fold into their own `Outcome`.
///
/// Takes a `Born` already extracted rather than `&Args`: the three ops that
/// call this (`push`, `add`, `decide`) do not all read the fields the same
/// way (`add` defaults `why` with `.opt_or`, `push` demands it), so the
/// reading stays with each caller and only the shared write comes here.
fn born(ctx: &Ctx, b: Born) -> Result<(Body, u64, String, bool), Failure> {
    let mut fields: Vec<(&str, &str)> = vec![("title", b.title), ("why", b.why)];
    fields.extend(b.refs.iter().map(|r| ("ref", r.as_str())));
    fields.extend(b.governs.iter().map(|g| ("governs", g.as_str())));
    fields.extend(b.arms.iter().map(|a| ("arm", a.command.as_str())));
    fields.extend(b.against.iter().map(|a| ("against", a.why.as_str())));
    guard_text(&fields)?;

    let node = id::ulid();
    let num = ctx.tree.next_num.max(1);
    // `t426` §1.1: `Some` only for a decision born while at least one
    // pillar or rule is open -- the same predicate `vivac rules` lists
    // under -- and `Some(vec![])` when nothing was declared. Every other
    // node keeps writing exactly the bytes it always has.
    let against = (b.kind == Kind::Decision && ctx.tree.has_open_governance()).then_some(b.against);
    let no_against = against.as_ref().is_some_and(Vec::is_empty);
    Ok((
        Body::NodeCreated {
            node: node.clone(),
            num,
            kind: b.kind,
            title: b.title.to_string(),
            why: b.why.to_string(),
            parent: b.parent,
            blocks: b.blocks,
            refs: b.refs,
            governs: b.governs,
            arms: b.arms,
            against,
        },
        num,
        node,
        no_against,
    ))
}

/// `push` — open a detour. It is **the** operation: the provenance edge is
/// created here on its own, with nobody having to remember to declare it.
///
/// `--root` (`t533` §1) means born with no parent, nothing more: the kind
/// still defaults the way it always has for a parentless node (`Kind::Goal`),
/// and `add`/`decide`'s own stacks never move either way. On `push`, it also
/// leaves the stack holding only the new node -- what was on it stays open in
/// the tree, and the events that record it are the same ones `focus` already
/// writes crossing branches: a `stack.popped` per node that leaves, top to
/// bottom, then the `stack.pushed` of the new one.
pub fn push(ctx: &mut Ctx, p: params::Push) -> Result<Outcome, Failure> {
    let parent = if p.root {
        None
    } else {
        ctx.tree.focus().map(|n| n.id.clone())
    };
    let kind = kind_of(
        p.kind.as_deref(),
        if parent.is_none() {
            Kind::Goal
        } else {
            Kind::Task
        },
    )?;
    let arms = arms_of(ctx, p.arms, p.arm_dir, kind, p.via_mcp)?;
    let against = against_of(ctx, p.against, kind)?;
    let (ev, num, node, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms,
            against,
        },
    )?;
    // The old stack, bottom to top, kept only for `--root`: it is what
    // `left_stack` reports and what the vivac below freezes.
    let old_stack: Vec<u64> = if p.root {
        ctx.tree.stack().to_vec()
    } else {
        Vec::new()
    };
    // The vivac goes **before** the push: it freezes the stack at the moment
    // of the fork, which is the belay where you make yourself safe before
    // setting off. The `next_intent` is the child being opened, because that
    let v = vivac(ctx, VivacKind::Push, &p.title, parent, "");
    let mut evs = vec![v, ev];
    for &n in old_stack.iter().rev() {
        if let Some(left) = ctx.tree.node_by_num(n) {
            evs.push(Body::Popped {
                node: left.id.clone(),
            });
        }
    }
    evs.push(Body::Pushed { node });
    ctx.emit(evs)?;

    let left_stack: Vec<String> = old_stack
        .iter()
        .filter_map(|&n| ctx.tree.node_by_num(n))
        .map(|n| n.alias())
        .collect();
    // The deepest of what left that is still open or parked: the one worth
    // naming to get back to. Depth here means position in the old stack, top
    // first, not how far it is from any root.
    let back_to: Option<String> = old_stack
        .iter()
        .rev()
        .filter_map(|&n| ctx.tree.node_by_num(n))
        .find(|n| matches!(n.state, State::Active | State::Suspended))
        .map(|n| n.alias());

    // `emit` already applied the push in memory, so the stack includes the
    // new node and there is no need to add one.
    let depth_of = ctx.tree.stack_depth();
    // §6.1: intervene, never block. A deep stack is almost never lack of
    // discipline: the root goal moved and nobody re-rooted. `--root` always
    // leaves the stack one level deep, so this never fires for it.
    let advice = if depth_of >= 4 {
        // The node named is the **bottom of this stack**, never the tree's
        // first root: the number measures the stack (`f156`), so taking the
        // number from one place and the node from another gives a true count
        // pointing at the wrong goal. It showed up as soon as there was more
        // than one root -- which is what `promote` exists to make -- and the
        // advice named whichever root was written first, closed or not
        // (`f331`).
        ctx.tree.stack_bottom().map(|root| outcome::DepthAdvice {
            depth: depth_of,
            root_alias: root.alias(),
            root_title: root.title(&ctx.tree).to_string(),
            // `t533` §2.5: the bottom can now be closed or parked, since
            // `done`/`park` no longer unstack a node that is not the top.
            root_mark: (!root.state.is_open()).then(|| root.state.word(root.kind).to_string()),
        })
    } else {
        None
    };
    Ok(Outcome::Pushed {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        blocks: p.blocks,
        advice,
        no_against,
        left_stack,
        back_to,
    })
}

/// `pop` — close the focus and come back to the parent with context.
///
/// `f552` (`t533` §2.2): the focus can now be something `done` or `park`
/// closed while it sat below the top of the stack, and the path only reached
/// it once everything above it was popped in turn. Popping it does not undo
/// that: nothing is open to close, so no `state.changed` is written, the
/// closure rule is never consulted, and `--force` changes nothing. The
/// `stack.popped` and the vivac are written exactly as they always are.
pub fn pop(ctx: &mut Ctx, p: params::Pop) -> Result<Outcome, Failure> {
    let focus = ctx
        .tree
        .focus()
        .ok_or_else(|| {
            Failure::usage(
                "The stack is empty. Open something:  vivac push \"<title>\" --why \"<reason>\"",
            )
        })?
        .clone();
    let outcome_text = p.outcome.as_str();
    let next = p.next.as_deref().unwrap_or(outcome_text);
    guard_text(&[("outcome", outcome_text), ("next", next)])?;
    let v = vivac(ctx, VivacKind::Pop, next, Some(focus.id.clone()), "");
    // Trap: two separate `emit`s in a row, not one lot like `push` -- one
    // inside `close_node` (or the bare pop below), one here for the vivac --
    // and the parent's counts below have to be read only after both, or the
    // number comes out wrong.
    let closed = if focus.state.is_open() {
        close_node(ctx, &focus, outcome_text, p.force, true)?
    } else {
        ctx.emit(vec![Body::Popped {
            node: focus.id.clone(),
        }])?;
        outcome::Closed {
            alias: focus.alias(),
            title: focus.title(&ctx.tree).to_string(),
            force: p.force,
            already: Some(focus.state.word(focus.kind).to_string()),
        }
    };
    ctx.emit(vec![v])?;
    let parent = match focus.parent.and_then(|p| ctx.tree.node_by_num(p)) {
        Some(parent) => Some(outcome::PoppedTo {
            alias: parent.alias(),
            title: parent.title(&ctx.tree).to_string(),
            counts: ctx.tree.counts(parent.num),
        }),
        None => None,
    };
    Ok(Outcome::Popped { closed, parent })
}

/// `park` — what produces DO NOT TOUCH NOW; without it that section always
/// comes out empty. The closure rule does not stop it: parking claims nothing
/// finished, and if parking cost more than ignoring, nobody would park.
/// Whether a word is shaped like the name of a node.
///
/// A bare number, or one character of type prefix and a number: `25`, `f25`.
/// Prose never looks like that, so a word that does and resolves to nothing is
/// a typo rather than a reason, and saying so beats guessing.
fn looks_like_an_id(s: &str) -> bool {
    let s = s.trim().trim_start_matches('#');
    let mut c = s.chars();
    let Some(first) = c.next() else {
        return false;
    };
    let rest = c.as_str();
    if first.is_ascii_digit() {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// The node an operation acts on where naming it is optional, and the reason
/// written beside it.
///
/// **Two words are not ambiguous**: the first is an id and it has to resolve.
/// `park f74 "<reason>"` used to fall through to the focus when `f74` named
/// nothing, parking a node nobody had written down, filing `f74` itself as the
/// reason, dropping the reason actually typed, and exiting 0. The hole was
/// `and_then`, which flattens "did not resolve" into the same `None` as "was
/// not given" (`f74`).
///
/// One word **is** ambiguous by the grammar, because a reason is as good a
/// word as an alias. So it is resolved, and only a word shaped like an id has
/// to succeed.
fn named_or_focus(
    ctx: &Ctx,
    node: Option<&str>,
    reason: Option<&str>,
    usage: &'static str,
) -> Result<(Node, String), Failure> {
    let focus = || {
        ctx.tree
            .focus()
            .cloned()
            .ok_or_else(|| Failure::usage(usage))
    };
    match (node, reason) {
        (Some(s), Some(r)) => Ok((ctx.resolve(s)?.clone(), r.to_string())),
        (Some(w), None) => match ctx.tree.resolve(w) {
            Some(n) => Ok((n.clone(), String::new())),
            None if looks_like_an_id(w) => Err(Failure::usage(format!("No such node: {w}."))),
            None => Ok((focus()?, w.to_string())),
        },
        _ => Ok((focus()?, String::new())),
    }
}

pub fn park(ctx: &mut Ctx, p: params::Park) -> Result<Outcome, Failure> {
    let (node, reason) = named_or_focus(
        ctx,
        p.node.as_deref(),
        p.reason.as_deref(),
        "usage: vivac park [<id>] [\"<reason>\"]",
    )?;
    let reason = reason.as_str();
    guard_text(&[("reason", reason)])?;
    let mut evs = vec![vivac(
        ctx,
        VivacKind::Park,
        reason,
        Some(node.id.clone()),
        "",
    )];
    evs.push(Body::StateChanged {
        node: node.id.clone(),
        state: State::Suspended,
        outcome: reason.to_string(),
        forced: false,
    });
    // `t533` §2.1: only when it is the stack's own top. Anywhere else, the
    // path still runs through it and the spine marks it parked instead.
    if ctx.tree.stack().last() == Some(&node.num) {
        evs.push(Body::Popped {
            node: node.id.clone(),
        });
    }
    ctx.emit(evs)?;
    Ok(Outcome::Parked {
        alias: node.alias(),
        title: node.title(&ctx.tree).to_string(),
    })
}

/// The closure rule. `MODEL.md` §7, and the **only** rule in the model that
/// refuses a user operation.
///
/// It earns that privilege because the case it prevents is measured: an
/// audit marked DONE with its findings open took 26 days to be spotted.
/// Without this, the model lets the same mistake happen again.
fn close_node(
    ctx: &mut Ctx,
    n: &crate::model::Node,
    outcome: &str,
    force: bool,
    unstack: bool,
) -> Result<crate::outcome::Closed, Failure> {
    if !force {
        let pending_count = ctx.tree.open_blockers(n.num);
        if !pending_count.is_empty() {
            let mut m = format!(
                "  {} CANNOT close: {} open closure condition(s)\n",
                n.alias(),
                pending_count.len()
            );
            for c in &pending_count {
                m.push_str(&format!("\n      {:<6} {}", c.alias(), c.title(&ctx.tree)));
            }
            m.push_str(&format!(
                "\n\n  A run closes with its findings, not with its report.\n  \
                 Closing it anyway leaves a trace:  vivac done {} --force",
                n.num
            ));
            return Err(Failure::Model(m));
        }
    }
    let mut evs = vec![Body::StateChanged {
        node: n.id.clone(),
        state: State::Done,
        outcome: outcome.to_string(),
        forced: force,
    }];
    // `t533` §2.1: only when it is the stack's own top. Anywhere else, the
    // path still runs through it and the spine marks it closed instead.
    if unstack && ctx.tree.stack().last() == Some(&n.num) {
        evs.push(Body::Popped { node: n.id.clone() });
    }
    ctx.emit(evs)?;
    Ok(crate::outcome::Closed {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        force,
        already: None,
    })
}

pub fn done(ctx: &mut Ctx, p: params::Done) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    guard_text(&[("outcome", &p.outcome)])?;
    let closed = close_node(ctx, &n, &p.outcome, p.force, true)?;
    Ok(Outcome::Done { closed })
}

/// `add` — a node without touching the stack. It is how a tree that already
/// existed elsewhere gets in, and how a finding hangs off something that is
pub fn add(ctx: &mut Ctx, p: params::Add) -> Result<Outcome, Failure> {
    if p.root && p.parent.is_some() {
        return Err(root_and_parent_error());
    }
    let parent = if p.root {
        None
    } else {
        match &p.parent {
            Some(s) => Some(ctx.resolve(s)?.id.clone()),
            None => ctx.tree.focus().map(|n| n.id.clone()),
        }
    };
    let kind = kind_of(
        p.kind.as_deref(),
        if parent.is_none() {
            Kind::Goal
        } else {
            Kind::Task
        },
    )?;
    let arms = arms_of(ctx, p.arms, p.arm_dir, kind, p.via_mcp)?;
    let against = against_of(ctx, p.against, kind)?;
    let (ev, num, _, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms,
            against,
        },
    )?;
    ctx.emit(vec![ev])?;
    let parent_info = parent
        .and_then(|id| ctx.tree.node(&id))
        .map(|n| outcome::AddedUnder {
            alias: n.alias(),
            title: n.title(&ctx.tree).to_string(),
        });
    Ok(Outcome::Added {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        parent: parent_info,
        blocks: p.blocks,
        no_against,
    })
}

pub fn note(ctx: &mut Ctx, p: params::Note) -> Result<Outcome, Failure> {
    let (n, note) = match (p.node.as_deref(), p.note.as_deref()) {
        (Some(s), Some(t)) => (ctx.resolve(s)?.clone(), t.to_string()),
        (Some(t), None) => {
            let f = ctx
                .tree
                .focus()
                .ok_or_else(|| Failure::usage("usage: vivac note [<id>] \"<note>\""))?;
            (f.clone(), t.to_string())
        }
        _ => return Err(Failure::usage("usage: vivac note [<id>] \"<note>\"")),
    };
    guard_text(&[("note", &note)])?;
    ctx.emit(vec![Body::NodeNoted {
        node: n.id.clone(),
        note,
    }])?;
    Ok(Outcome::Noted { alias: n.alias() })
}

pub fn block(ctx: &mut Ctx, p: params::Block) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    let Some(parent) = n.parent.and_then(|p| ctx.tree.node_by_num(p)) else {
        return Err(Failure::usage(format!(
            "{} is the root: there is no parent to block.",
            n.alias()
        )));
    };
    let blocks = !p.off;
    let (pa, pt) = (parent.alias(), parent.title(&ctx.tree).to_string());
    ctx.emit(vec![Body::BlockChanged {
        node: n.id.clone(),
        blocks,
    }])?;
    Ok(Outcome::Blocked {
        alias: n.alias(),
        blocks,
        parent_alias: pa,
        parent_title: pt,
    })
}

/// `promote` — the focus becomes a goal of its own and the stack is cut there.
///
/// The provenance chain is **kept**: where it was born does not change just
/// because its rank did. Without this operation, the depth warning has no way
/// out and ends up being ignored.
pub fn promote(ctx: &mut Ctx, p: params::Promote) -> Result<Outcome, Failure> {
    let n = match p.id {
        Some(s) => ctx.resolve(&s)?.clone(),
        None => ctx
            .tree
            .focus()
            .ok_or_else(|| Failure::usage("usage: vivac promote [<id>]"))?
            .clone(),
    };
    ctx.emit(vec![Body::Promoted { node: n.id.clone() }])?;
    let parent = n
        .parent
        .and_then(|id| ctx.tree.node_by_num(id))
        .map(|parent| outcome::StillBornFrom {
            alias: parent.alias(),
            title: parent.title(&ctx.tree).to_string(),
        });
    Ok(Outcome::Promoted {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        parent,
    })
}

/// `abandon` — discard. It costs the same as `pop` on purpose: if abandoning
/// were dearer than ignoring, nobody would abandon and in three months the
/// tree would be noise.
///
/// The cascade is **not** the default. `MODEL.md` §6 wants it with a
/// confirmation and the list up front, and a non-interactive CLI cannot
/// confirm anything: it shows what would fall and asks for an explicit
///
/// **Rescue does not reparent** (`d33`). `MODEL.md` §6 said to re-parent the
/// descendant onto a living ancestor; that rewrites the birth, and invariant
/// 11 says a thing is born in one place. A rescued node stays where it was
/// born: alive, under an abandoned parent. It is the same shape as an open
/// finding under a closed batch, which the tree already knows how to show and
/// the brief already knows how to count.
pub fn abandon(ctx: &mut Ctx, p: params::Abandon) -> Result<Outcome, Failure> {
    let (n, reason) = named_or_focus(
        ctx,
        p.node.as_deref(),
        p.reason.as_deref(),
        "usage: vivac abandon [<id>] \"<reason>\"",
    )?;
    let reason = reason.as_str();
    guard_text(&[("reason", reason)])?;

    // Rescuing a node rescues its descendants. Saving the parent and letting
    // the children die would be a half rescue nobody asked for, and would
    // orphan exactly what was meant to be kept.
    let mut rescued: std::collections::HashSet<String> = Default::default();
    for s in p.rescue {
        let r = ctx
            .tree
            .resolve(&s)
            .ok_or_else(|| Failure::usage(format!("no such node: {s}")))?;
        let (rid, r_num, ralias) = (r.id.clone(), r.num, r.alias());
        if rid == n.id {
            return Err(Failure::usage(format!(
                "{ralias} is the one being abandoned; it cannot be rescued from itself"
            )));
        }
        if !ctx.tree.descendants(n.num).iter().any(|d| d.id == rid) {
            return Err(Failure::usage(format!(
                "{ralias} does not hang off {}: there is nothing to rescue it from",
                n.alias()
            )));
        }
        rescued.insert(rid.clone());
        for d in ctx.tree.descendants(r_num) {
            rescued.insert(d.id.clone());
        }
    }

    let (falling, saved): (Vec<&Node>, Vec<&Node>) = ctx
        .tree
        .descendants(n.num)
        .into_iter()
        .filter(|d| d.state.is_open())
        .partition(|d| !rescued.contains(&d.id));

    // Only what falls unnamed needs confirming. If everything was rescued,
    // there is nothing left to confirm.
    if !falling.is_empty() && !p.cascade {
        let mut m = format!(
            "  {}  {}\n  has {} open descendant(s) with no rescue:\n",
            n.alias(),
            n.title(&ctx.tree),
            falling.len()
        );
        for d in &falling {
            m.push_str(&format!("\n      {:<6} {}", d.alias(), d.title(&ctx.tree)));
        }
        m.push_str("\n\n  Abandon all of it:     vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --cascade");
        m.push_str("\n  Save some of it:       vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --rescue <id>");
        m.push_str("\n  Save it as a goal:     vivac promote <id>");
        return Err(Failure::Model(m));
    }

    let mut evs = vec![Body::StateChanged {
        node: n.id.clone(),
        state: State::Abandoned,
        outcome: reason.to_string(),
        forced: false,
    }];
    let falling_count = falling.len();
    let saved_lines: Vec<(String, String)> = saved
        .iter()
        .map(|d| (d.alias(), d.title(&ctx.tree).to_string()))
        .collect();
    for d in falling {
        evs.push(Body::StateChanged {
            node: d.id.clone(),
            state: State::Abandoned,
            outcome: format!("cascaded from {}", n.alias()),
            forced: false,
        });
    }
    // The stack is the path to the focus and cannot cross an abandoned node,
    // so everything hanging off the abandoned one leaves it --the rescued
    // included, which stays alive but stops being on the path--.
    let mut out_of_scope: Vec<(u64, String)> = vec![(n.num, n.id.clone())];
    out_of_scope.extend(
        ctx.tree
            .descendants(n.num)
            .iter()
            .map(|d| (d.num, d.id.clone())),
    );
    for (num, id) in out_of_scope {
        if ctx.tree.stack().contains(&num) {
            evs.push(Body::Popped { node: id });
        }
    }

    ctx.emit(evs)?;
    Ok(Outcome::Abandoned {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        cascaded: (falling_count > 0).then_some(falling_count),
        rescued: saved_lines
            .into_iter()
            .map(|(alias, title)| outcome::RescuedNode { alias, title })
            .collect(),
    })
}

/// `focus` — step back into a node that already exists.
///
/// Without this the stack only works inside one session: the next day the log
/// holds the whole tree and the stack is empty, and there is no way to say "I
/// am on this" without opening a new node, which is exactly the litter to be
/// avoided. The stack becomes the path from the root down to the node, which
/// is what working on it means.
pub fn focus(ctx: &mut Ctx, p: params::Focus) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();

    if !n.state.is_open() && !p.reopen {
        // Parking says "maybe I will be back", so returning is the normal
        // operation and asks no permission. Closing claims something finished:
        // undoing that has to be deliberate.
        if n.state != State::Suspended {
            return Err(Failure::Model(format!(
                "  {} is {}. Going back into it undoes that claim.\n\n  \
                 If it really was not finished:  vivac focus {} --reopen",
                n.alias(),
                n.state.word(n.kind),
                n.num
            )));
        }
    }

    let lineage: Vec<(u64, String)> = ctx
        .tree
        .ancestors(n.num)
        .iter()
        .map(|p| (p.num, p.id.clone()))
        .collect();
    let mut evs: Vec<Body> = ctx
        .tree
        .stack()
        .iter()
        .filter(|num| !lineage.iter().any(|(lineage_num, _)| lineage_num == *num))
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| Body::Popped { node: n.id.clone() })
        .collect();
    if !n.state.is_open() {
        evs.push(Body::StateChanged {
            node: n.id.clone(),
            state: State::Active,
            outcome: String::new(),
            forced: false,
        });
    }
    for (num, id) in &lineage {
        if !ctx.tree.stack().contains(num) {
            evs.push(Body::Pushed { node: id.clone() });
        }
    }
    let revived = !n.state.is_open();
    ctx.emit(evs)?;
    // Trap: `render::stack` used to be called from here, reading `a` for its
    // own `--json` on its own. `main.rs` calls it separately now, after this
    // `Outcome` is printed -- `render.rs` is not touched, and the flag never
    // reached this call site from the CLI anyway (`focus` is not allowed
    // `--json` in `main.rs`'s table).
    Ok(Outcome::Focused {
        alias: n.alias(),
        revived,
    })
}

/// `flag <id> <flag> --why <reason>` — raise or clear a flag.
///
/// The reason is **mandatory** when raising it. `BRIEF-SPEC.md` §10 tests it
/// as a contract: a flag with no reason informs nobody, it only adds noise to
/// the brief, and within a week they all get ignored.
pub fn flag(ctx: &mut Ctx, p: params::Flag) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    let flag = Flag::parse(&p.flag).ok_or_else(|| {
        Failure::usage(format!("Unknown flag: {}. They are: {}", p.flag, Flag::ALL))
    })?;

    if p.off {
        ctx.emit(vec![Body::FlagCleared {
            node: n.id.clone(),
            flag,
        }])?;
        return Ok(Outcome::Flagged {
            alias: n.alias(),
            flag: flag.word().to_string(),
            change: outcome::FlagChange::Off,
        });
    }
    let reason = p.why.ok_or_else(|| {
        Failure::usage(
            "Missing --why. A flag with no reason informs nobody: in two weeks\n  \
             nobody will know what needed looking at, and they all get ignored.",
        )
    })?;
    guard_text(&[("reason", &reason)])?;
    ctx.emit(vec![Body::FlagRaised {
        node: n.id.clone(),
        flag,
        reason: reason.clone(),
    }])?;
    Ok(Outcome::Flagged {
        alias: n.alias(),
        flag: flag.word().to_string(),
        change: outcome::FlagChange::Raised {
            title: n.title(&ctx.tree).to_string(),
            reason,
        },
    })
}

/// `arm <id> "<command>" [--off]` — record or remove what verifies a rule.
///
/// Vivac never runs it: `d415`. Shaped like `flag`, and like `flag` it
/// arms or disarms a closed node. Where it parts from `flag` is the
/// repeat: a flag folds into a set, so raising it twice changes nothing,
/// but arms fold into a list, so a repeated arm would show twice and the
/// removal of an absent one would write a line that changes no answer.
/// Both are refused before anything is written.
pub fn arm(ctx: &mut Ctx, p: params::Arm) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    if n.kind != Kind::Rule {
        return Err(Failure::usage(format!(
            "Only a rule has an arm; {} is {}.",
            n.alias(),
            n.kind.with_article()
        )));
    }
    let dir = match &p.dir {
        Some(d) if !d.trim().is_empty() => d.clone(),
        _ => return Err(Failure::usage(needs_dir_message(p.via_mcp))),
    };
    let dir = validate_arm_dir(&dir, &ctx.store.root)?;
    validate_arm_text(&p.command)?;
    let has = n
        .arms(&ctx.tree)
        .contains(&(dir.as_str(), p.command.as_str()));
    if p.off && !has {
        return Err(Failure::usage(format!(
            "{0} has no such arm; vivac why {0} lists the ones it has.",
            n.alias()
        )));
    }
    if !p.off && has {
        return Err(Failure::usage(format!(
            "{} already has that arm.",
            n.alias()
        )));
    }
    guard_text(&[("arm", &p.command)])?;
    let body = if p.off {
        Body::ArmRemoved {
            node: n.id.clone(),
            dir: dir.clone(),
            command: p.command.clone(),
        }
    } else {
        Body::ArmAdded {
            node: n.id.clone(),
            dir: dir.clone(),
            command: p.command.clone(),
        }
    };
    ctx.emit(vec![body])?;
    Ok(Outcome::Armed {
        alias: n.alias(),
        dir,
        arm: p.command,
        change: if p.off {
            outcome::ArmChange::Removed
        } else {
            outcome::ArmChange::Added
        },
    })
}

/// `decide` — record a decision.
///
/// The discarded alternatives are optional in the schema and mandatory in
/// practice: without them, in a month the agent proposes again what you
/// already rejected.
pub fn decide(ctx: &mut Ctx, p: params::Decide) -> Result<Outcome, Failure> {
    if p.root && p.parent.is_some() {
        return Err(root_and_parent_error());
    }
    let superseded = match &p.supersedes {
        Some(s) => Some(ctx.resolve(s)?.clone()),
        None => None,
    };

    let mut body = p.reason.clone();
    if !p.alternatives.is_empty() {
        body.push_str(&format!("  |  discarded: {}", p.alternatives.join("; ")));
    }
    let parent = if p.root {
        None
    } else {
        match &p.parent {
            Some(s) => Some(ctx.resolve(s)?.id.clone()),
            None => ctx.tree.focus().map(|n| n.id.clone()),
        }
    };
    let against = against_of(ctx, p.against, Kind::Decision)?;
    let (ev, num, _, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &body,
            kind: Kind::Decision,
            parent,
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms: vec![],
            against,
        },
    )?;

    let mut evs = vec![ev];
    if let Some(v) = &superseded {
        // `supersedes` forms a chain: the old one becomes superseded, not deleted.
        evs.push(Body::StateChanged {
            node: v.id.clone(),
            state: State::Superseded,
            outcome: format!("superseded by d{num}"),
            forced: false,
        });
    }
    ctx.emit(evs)?;
    Ok(Outcome::Decided {
        alias: format!("d{num}"),
        title: p.title,
        superseded: superseded.map(|v| outcome::SupersededNode { alias: v.alias() }),
        no_alternatives: p.alternatives.is_empty(),
        no_against,
    })
}

/// `declare <decision> --against "<id>: <why>"` — record, after the fact,
/// what a decision was judged against. `t426` §2.2: unlike `--against` at
/// birth, the decision may be in any state -- it declares a fact about the
/// past, not a claim about what it still governs.
pub fn declare(ctx: &mut Ctx, p: params::Declare) -> Result<Outcome, Failure> {
    let (Some(id), false) = (p.id.as_deref(), p.against.is_empty()) else {
        return Err(Failure::usage(
            "usage: vivac declare <decision> --against \"r12: <why>\"",
        ));
    };
    let n = ctx.resolve(id)?.clone();
    if n.kind != Kind::Decision {
        return Err(Failure::usage(format!(
            "vivac declare takes a decision, and {} is {}",
            n.alias(),
            n.kind.with_article()
        )));
    }
    let entries = against_of(ctx, p.against, Kind::Decision)?;
    // What the decision already declares, at birth or later, cannot be
    // declared again.
    let mut already: Vec<u64> = n.against.iter().map(|a| a.node).collect();
    for e in &entries {
        let (num, alias) = ctx
            .tree
            .node(&e.node)
            .map(|x| (x.num, x.alias()))
            .unwrap_or((u64::MAX, e.node.clone()));
        if already.contains(&num) {
            return Err(Failure::usage(format!(
                "{} already declares {alias}",
                n.alias()
            )));
        }
        already.push(num);
    }
    guard_text(
        &entries
            .iter()
            .map(|a| ("against", a.why.as_str()))
            .collect::<Vec<_>>(),
    )?;
    ctx.emit(vec![Body::AgainstAdded {
        node: n.id.clone(),
        against: entries.clone(),
    }])?;
    Ok(Outcome::Declared {
        alias: n.alias(),
        against: entries
            .into_iter()
            .map(|a| outcome::DeclaredPair {
                node: ctx.tree.node(&a.node).map(|x| x.alias()).unwrap_or(a.node),
                why: a.why,
            })
            .collect(),
    })
}

/// `save [label]` — a safe stop on purpose.
pub fn save(ctx: &mut Ctx, p: params::Save) -> Result<Outcome, Failure> {
    guard_text(&[("label", &p.label), ("next", &p.next)])?;
    let v = vivac(ctx, VivacKind::Manual, &p.next, None, &p.label);
    let num = ctx.tree.next_vivac_num.max(1);
    ctx.emit(vec![v])?;
    // With no VCS no precision is faked: the vivac is worth the same, but
    // restoring it will only give plain age, not a diff.
    let anchor = ctx.anchor.snapshot();
    Ok(Outcome::Saved {
        num,
        label: p.label,
        anchor,
        next: p.next,
    })
}

/// A saved entry resolved against the live tree once, up front: `num`,
/// whether it is still open, and the word for when it is not -- `None` when
/// a hand edit or an old bug left the alias resolving to nothing at all.
type MatchedEntry = Option<(u64, bool, String)>;

/// The word for a saved entry that fell out of the rebuilt path: its state,
/// or `"gone"` when it no longer resolves at all.
fn lost_state(matched: &MatchedEntry) -> String {
    matched
        .as_ref()
        .map(|(_, _, word)| word.clone())
        .unwrap_or_else(|| "gone".to_string())
}

/// What `restore` rebuilds from a vivac's saved stack: the real lineage to
/// put back (bottom to top), what stays on it despite not being open
/// (`kept`), and everything the saved stack named that fell out of the
/// rebuilt path entirely (`lost`), bottom to top itself.
///
/// `t533` §2.3: the stack it rebuilds is always a contiguous stretch of the
/// real tree's lineage, even when the vivac itself was saved by a version
/// whose own stack could skip over a closed ancestor. `N` is the saved
/// entry's own **deepest still-open node**; `B` is the first saved entry, from
/// the bottom, that is `N` or one of its ancestors. The new stack is the real
/// lineage from `B` to `N` -- contiguous by construction, never the saved
/// list itself. Every node from `B` to `N` that is not open is still on the
/// path, and says so rather than pretending it is open. Everything else
/// named by the saved stack is lost: above `N` as always, and below `B` too
/// -- a saved stack that is a subsequence of its top's lineage, which is what
/// every version has ever written, can only leave something unresolved down
/// there, but `restore` never drops what it leaves out in silence.
///
/// Split out from `restore` itself so this can be tested against a folded
/// `Tree` and a hand-built saved stack alone, with no store to write through.
fn restore_path(
    tree: &Tree,
    saved_stack: &[(String, String)],
) -> (
    Vec<(u64, String)>,
    Vec<outcome::KeptNode>,
    Vec<outcome::LostNode>,
) {
    let matched: Vec<MatchedEntry> = saved_stack
        .iter()
        .map(|(alias, _)| {
            tree.resolve(alias)
                .map(|n| (n.num, n.state.is_open(), n.state.word(n.kind).to_string()))
        })
        .collect();
    // `N`: the deepest (closest to the old top) saved entry that is still
    // open.
    let deepest_open = matched
        .iter()
        .rposition(|m| m.as_ref().is_some_and(|(_, open, _)| *open));

    let mut kept: Vec<outcome::KeptNode> = Vec::new();
    let mut lost: Vec<outcome::LostNode> = Vec::new();
    let mut lineage: Vec<(u64, String)> = Vec::new();

    match deepest_open {
        None => {
            // Nothing saved is still open: the whole point is lost, exactly
            // as it always has been.
            for (i, (alias, title)) in saved_stack.iter().enumerate() {
                lost.push(outcome::LostNode {
                    alias: alias.clone(),
                    title: title.clone(),
                    state: lost_state(&matched[i]),
                });
            }
        }
        Some(deepest_open) => {
            let (deepest_num, _, _) = matched[deepest_open].clone().unwrap();
            // The real lineage of `N`, root first: contiguous by
            // construction, unlike the saved list it may have come from.
            let full_lineage = tree.ancestors(deepest_num);
            // `B`: the first saved entry, from the bottom, that is `N` or one
            // of its ancestors.
            let bottom_index = (0..=deepest_open)
                .find(|&i| {
                    matched[i]
                        .as_ref()
                        .is_some_and(|(num, _, _)| full_lineage.iter().any(|a| a.num == *num))
                })
                .unwrap_or(deepest_open);
            let (bottom_num, _, _) = matched[bottom_index].clone().unwrap();
            let start = full_lineage
                .iter()
                .position(|a| a.num == bottom_num)
                .unwrap_or(0);
            for n in &full_lineage[start..] {
                lineage.push((n.num, n.id.clone()));
                if !n.state.is_open() {
                    kept.push(outcome::KeptNode {
                        alias: n.alias(),
                        title: n.title(tree).to_string(),
                        state: n.state.word(n.kind).to_string(),
                    });
                }
            }
            // Below `B`: never reached the lineage at all, and still named.
            for (i, (alias, title)) in saved_stack.iter().enumerate().take(bottom_index) {
                lost.push(outcome::LostNode {
                    alias: alias.clone(),
                    title: title.clone(),
                    state: lost_state(&matched[i]),
                });
            }
            // Above `N`: as always.
            for (i, (alias, title)) in saved_stack.iter().enumerate().skip(deepest_open + 1) {
                lost.push(outcome::LostNode {
                    alias: alias.clone(),
                    title: title.clone(),
                    state: lost_state(&matched[i]),
                });
            }
        }
    }
    (lineage, kept, lost)
}

/// `restore <v>` — go back to a vivac.
///
/// **It never touches the working tree.** Mixing context navigation with tree
/// manipulation turns a tool for attention into a branch manager worse than
/// git. It rebuilds the stack and presents the diff; `restore_path` above
/// does the rebuilding.
pub fn restore(ctx: &mut Ctx, p: params::Restore) -> Result<Outcome, Failure> {
    let v = ctx
        .tree
        .vivac(&p.vivac)
        .ok_or_else(|| Failure::usage(format!("No such vivac: {}.", p.vivac)))?
        .clone();

    // `d598`: git runs before the lock. A saved vivac never changes, so
    // what changed since its anchor is the same either side of the lock;
    // only the stack is decided under it, on the tree as it is then.
    let changes = ctx.anchor.changed_since(&v.anchor);
    let mine = ctx.lock_for_write()?;

    let (lineage, kept, lost) = restore_path(&ctx.tree, &v.stack);

    let mut evs: Vec<Body> = ctx
        .tree
        .stack()
        .iter()
        .filter(|num| !lineage.iter().any(|(lineage_num, _)| lineage_num == *num))
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| Body::Popped { node: n.id.clone() })
        .collect();
    for (num, id) in &lineage {
        if !ctx.tree.stack().contains(num) {
            evs.push(Body::Pushed { node: id.clone() });
        }
    }
    ctx.emit(evs)?;
    // The write is done and nothing after this reads or writes the tree
    // under the lock: `main.rs` still renders the stack before it returns,
    // and holding the lock through that would be a window nobody asked
    // for. Only released if this call is the one that took it: releasing a
    // lock a caller above is still holding would leave that caller writing
    // with nothing holding the tree (`f602`).
    if mine {
        ctx.unlock();
    }

    let anchor = if v.anchor.is_empty_tree() {
        outcome::RestoreAnchor::Empty
    } else if changes.is_empty() {
        outcome::RestoreAnchor::NoChanges {
            anchor_short: v.anchor.short().to_string(),
        }
    } else {
        outcome::RestoreAnchor::Changed {
            anchor_short: v.anchor.short().to_string(),
            changes: changes
                .iter()
                .map(|c| outcome::ChangeLine {
                    file_path: c.file_path.clone(),
                    times: c.times,
                })
                .collect(),
            working_set: v.working_set.clone(),
        }
    };
    // Trap: `render::stack` used to be called from here too, on the same `a`
    // it read `--json` from on its own. `main.rs` calls it separately now,
    // after this `Outcome` is printed -- `restore` is allowed no flags at all
    // in `main.rs`'s table, so `--json` never reached this call site either.
    Ok(Outcome::Restored {
        alias: v.alias(),
        kind: v.kind.word().to_string(),
        ts: v.ts,
        label: v.label,
        next_intent: v.next_intent,
        kept,
        lost,
        anchor,
    })
}

/// An automatic stop, for the end-of-session hook.
pub fn auto_vivac(
    ctx: &mut Ctx,
    kind: VivacKind,
    next: &str,
    label: &str,
) -> Result<Outcome, Failure> {
    guard_text(&[("next", next), ("label", label)])?;
    let v = vivac(ctx, kind, next, None, label);
    ctx.emit(vec![v])?;
    Ok(Outcome::AutoStopped)
}

/// The opening of a session, for the start hook.
///
/// It records **what the brief claimed** --the focus it named and the stop it
/// showed as the last one-- so that *was the brief followed?* can be answered
/// by comparing that against the first node touched afterwards, instead of by
/// somebody's judgement.
///
/// These are inputs and never a verdict: what counts as *following* the brief
/// lives in whoever reads, not in the log. Storing the comparison instead of
/// its terms would freeze a definition that may well turn out to be wrong.
///
/// `source` and `session` come off a payload this program did not write, so
/// they go through the guard like any other text. Unlike everywhere else, a
/// finding does not stop the write: the field is replaced by the rule that
/// refused it and the opening is recorded regardless. The rest of the guard
/// can afford to refuse because somebody is there to reword the sentence; a
/// hook has nobody, and a hook that fails is a hook that gets switched off.
///
/// `focus` and `vivac` are handed in rather than read off `ctx.tree`: the
/// caller takes them from what the brief actually painted, before
/// `lock_for_write` could have reloaded the tree out from under it.
pub fn session_started(
    ctx: &mut Ctx,
    source: &str,
    session: Option<String>,
    focus: Option<String>,
    vivac: Option<String>,
) -> Result<Outcome, Failure> {
    let source = guarded_or_refused("source", source);
    let session = session.map(|s| guarded_or_refused("session", &s));
    ctx.emit(vec![Body::SessionStarted {
        source,
        focus,
        vivac,
        session,
    }])?;
    Ok(Outcome::SessionOpened)
}

/// Declares this folder a lane of the tree, and locks the tree's config so
/// that a vivac too old to know lanes stops instead of reading half of it
/// (`d444`, §2.6).
///
/// `ctx` already holds the write lock and already runs as the lane being
/// declared (`Ctx::load_for_write`, then `lock_for_write`): this only ever
/// locks the config and then emits, in that order, never the other way.
/// A process that dies between the two leaves the config asking for a
/// vivac that knows lanes with no `lane.declared` to back it up yet, and
/// that is nothing to worry about -- the sentence it wrote is already
/// true, and the next `setup` writes the event that is still missing.
///
/// **Not** where this folder's own `.vivac/lane` gets written, when this
/// is a brand new lane: the caller (`write_lane`, `setup/claude_code.rs`)
/// puts that file down *before* this is even called, never after. The
/// reverse -- an event with no file behind it -- would leave this very
/// folder not knowing whose thread it is, and it would keep signing as
/// `main` while the tree it just wrote to says otherwise, which is the one
/// ordering nothing here is allowed to permit.
pub fn declare_lane(ctx: &mut Ctx, name: String, repos: Vec<crate::event::Repo>) -> R {
    let lock = ctx
        .lock
        .as_ref()
        .ok_or_else(|| Failure::Io(std::io::Error::other("write without the tree's lock")))?;
    ctx.store.lock_lanes_in_config(lock)?;
    let lane = ctx
        .lane
        .clone()
        .unwrap_or_else(|| crate::lane::MAIN.to_string());
    ctx.emit(vec![Body::LaneDeclared { lane, name, repos }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use crate::model::fold;

    fn created(seq: u64, num: u64, kind: Kind, parent: Option<&str>) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-13T00:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: format!("n{num}"),
                num,
                kind,
                title: format!("Node {num}"),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        }
    }

    /// `t533` §2.3, the hole a ruling caught: a saved entry sitting below `B`
    /// that does not resolve at all used to vanish from the report -- not
    /// `kept`, since it never reaches the rebuilt path, and not `lost`
    /// either, because that loop only ever looked above `N`. A saved stack
    /// that is a subsequence of its top's lineage -- what every version has
    /// ever written -- can only leave something unresolved in that slot, but
    /// `restore` still has to say so.
    #[test]
    fn a_saved_entry_below_the_path_that_does_not_resolve_is_reported_lost() {
        let events = vec![
            created(1, 1, Kind::Goal, None),
            created(2, 2, Kind::Task, Some("n1")),
            created(3, 3, Kind::Task, Some("n2")),
            created(4, 4, Kind::Task, Some("n3")),
        ];
        let tree = fold(&events, 0);
        let saved_stack = vec![
            ("f9".to_string(), "Nothing here resolves".to_string()),
            ("g1".to_string(), "Node 1".to_string()),
            ("t2".to_string(), "Node 2".to_string()),
            ("t3".to_string(), "Node 3".to_string()),
            ("t4".to_string(), "Node 4".to_string()),
        ];

        let (lineage, kept, lost) = restore_path(&tree, &saved_stack);

        assert_eq!(lineage.len(), 4, "the whole open lineage rebuilds");
        assert!(kept.is_empty(), "nothing on this path is closed");
        assert_eq!(
            lost.len(),
            1,
            "the entry below B must not vanish from the report: {lost:?}"
        );
        assert_eq!(lost[0].alias, "f9");
        assert_eq!(lost[0].state, "gone");
    }

    fn seeded_ctx(name: &str) -> (std::path::PathBuf, Ctx) {
        let tmp = std::env::temp_dir().join(format!("vivac-ops-{name}-{}", id::ulid()));
        let store = Store::create(&tmp).unwrap();
        (tmp, Ctx::load(store, Whose::Founding).unwrap())
    }

    #[test]
    fn taking_the_write_lock_twice_does_not_deadlock() {
        let (tmp, mut ctx) = seeded_ctx("relock");
        ctx.lock_for_write().unwrap();
        ctx.lock_for_write()
            .expect("a second take blocked on the first");
        ctx.unlock();
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn an_inner_release_does_not_take_the_lock_from_the_caller_above() {
        let (_tmp, mut ctx) = seeded_ctx("inner-release");
        assert!(
            ctx.lock_for_write().unwrap(),
            "the first take should be the one that locks"
        );
        let mine = ctx.lock_for_write().unwrap();
        assert!(
            !mine,
            "a second take must not claim the lock it already holds"
        );
        if mine {
            ctx.unlock();
        }
        assert!(
            ctx.holds_write_lock(),
            "an inner release dropped the caller's lock"
        );
        ctx.unlock();
    }

    /// `t594` task 6, review round 1: `lock_for_write` used to reload the tree by
    /// assigning `self.tree` directly, the one call site `adopt` did not
    /// yet cover, so a context on a lane other than `main` that reloaded
    /// under the lock -- because a second writer appended while it
    /// waited, exactly the two-writer scenario this whole stretch of work
    /// exists for -- came back reading `main` while its own store signed
    /// as the lane it actually is.
    #[test]
    fn a_reload_under_the_lock_keeps_answering_from_its_own_lane() {
        let tmp = std::env::temp_dir().join(format!("vivac-ops-lane-reload-{}", id::ulid()));
        let store = Store::create(&tmp).unwrap();
        let located = crate::store::Located {
            root: tmp.clone(),
            lane_dir: tmp.clone(),
            lane: Some(crate::lane::Lane {
                version: 1,
                id: "b".to_string(),
                project: String::new(),
            }),
            worktree: None,
        };
        let mut ctx = Ctx::load(store, Whose::Resolved(&located)).unwrap();

        // A second writer, signing as `main`, appends underneath: the
        // seam `lock_for_write` reloads for.
        let mut other = Store::open(tmp.clone()).unwrap();
        let lock = other.lock_for_write().unwrap();
        other
            .append(
                &lock,
                crate::lane::MAIN,
                vec![Body::Pushed {
                    node: "ghost".to_string(),
                }],
                0,
                false,
            )
            .unwrap();
        drop(lock);

        ctx.lock_for_write().unwrap();
        assert_eq!(
            ctx.tree.lane(),
            "b",
            "the reload under the lock forgot which lane this context is"
        );
        ctx.unlock();
        std::fs::remove_dir_all(&tmp).ok();
    }
}
