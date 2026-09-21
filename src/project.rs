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
//! A rewrite of a line before the last one that lands on the same byte
//! count is invisible to this tree, exactly as it is to the derived
//! index (`index.rs`, "Staying current"): the log is append-only, and
//! nothing vivac does rewrites it.
//!
//! This began inside the MCP server, the first thing here to outlive its own calls.
//! The web is the second, and it needs the same thing over more than one root, so it
//! moved out here.

use crate::event::Event;
use crate::failure::Failure;
use crate::{ops, store};
// `t594`: `web` (`src/web`) is not allowed to reach into
// `crate::store` directly (`tests/identifiers.rs`'s `web_never_touches_the_store`),
// and it needs `Located` to build the `Whose` it hands `Registry::open`. This is
// the one indirect path that guard already expects to survive: through the
// project layer, not around it.
pub use crate::store::Located;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One root, folded, with enough of a fingerprint to know when it moved.
pub struct Project {
    pub root: PathBuf,
    /// The readable half of a URL: this project's name with every run of
    /// characters outside `A-Za-z0-9._-` collapsed to a single `-`. **Not
    /// unique** -- two projects with the same name share it, and `d374`
    /// says an ambiguous one is refused rather than guessed at. It used to
    /// be made unique with a `-2` suffix; `f373` measured what that cost:
    /// the suffix is positional, so the first root leaving the registry
    /// handed its URL to the second and a saved link opened the wrong tree
    /// without an error.
    pub slug: String,

    /// This project's own name, which is what a page shows: the one saved
    /// with `--name`, and the directory's when nobody fixed one (`f679`).
    ///
    /// Read once, when the project is opened, the same as it has always
    /// been: a folder renamed while the server is up keeps showing the old
    /// name until it restarts, and a name saved while it is up arrives the
    /// same way. The permanent link does not depend on either -- it is the
    /// first event's id, and `ulid` reads that off the fold every time.
    pub name: String,
    ctx: ops::Ctx,
    /// The events the fold was built from, kept because a reader that groups
    /// the log by what happened -- `changes`, and the Today page that shows
    /// the same stretch -- needs them and the fold does not carry them.
    log: Vec<Event>,
    seen: (u64, Option<SystemTime>),
    /// Where the last fold stopped reading, and the event it read last:
    /// what lets a refresh read only what was appended since (`f599`),
    /// the rule `index.rs` "Staying current" already follows.
    fold_end: u64,
    last: Option<crate::index::LastEvent>,
    /// Broken lines already behind `fold_end`: an unterminated tail is not
    /// among them, since it sits past `fold_end` and is re-evaluated, never
    /// consumed, on every read -- see `refresh_if_stale`.
    committed_broken: usize,
    /// The log, kept open so a refresh never has to reopen it: reopening a
    /// file another process just wrote costs milliseconds on this machine,
    /// under contention, which is the whole write budget (`f599`). `None`
    /// is not a failure -- a refresh still works by path, just slower.
    log_file: Option<File>,
    /// How many times this project folded its whole log after opening.
    /// Only the tests read it: it is how `f599` is proven, not measured.
    #[cfg(test)]
    pub(crate) full_folds: usize,
}

impl Project {
    /// `whose` is which lane this project answers -- and, through
    /// `Project::write`, signs -- as. `Registry::open` is the one place
    /// that decides it: `Whose::Resolved` for the root the process
    /// started in, `Whose::Founding` for every other -- which reads and
    /// writes as the founding lane, same as any tree nobody ran `setup`
    /// in.
    pub fn open(
        root: PathBuf,
        name: String,
        slug: String,
        whose: ops::Whose<'_>,
    ) -> Result<Project, Failure> {
        let store = store::Store::open(root.clone())?;
        let seen = store::fingerprint(&store.log());
        let log_file = File::open(store.log()).ok();
        let read = crate::index::read_tracked(&store.log(), 0)?;
        let committed_broken = read.broken;
        let mut ctx = ops::Ctx::from_events(store, &read.events, committed_broken, seen, whose);
        ctx.tree.broken_lines = committed_broken + usize::from(read.unterminated);
        Ok(Project {
            root,
            slug,
            name,
            ctx,
            log: read.events,
            seen,
            fold_end: read.end_offset,
            last: read.last,
            committed_broken,
            log_file,
            #[cfg(test)]
            full_folds: 0,
        })
    }

    /// The permanent half of this project's URL: the id of its first event,
    /// which is what the registry is keyed by. `None` for a tree nobody has
    /// written to yet -- it has nothing to be keyed by, and nothing worth
    /// linking to either.
    ///
    /// The first event and **not** `Config::project_id`, which `f266`
    /// disqualified: `Store::open` silently regenerates a missing `config`,
    /// so deleting one file mints a fresh id for a tree that already has one.
    /// The log is append-only and cannot do that.
    ///
    /// Read off the fold rather than kept in a field, and the difference is
    /// not tidiness: a field is filled once at `open`, so a tree that was
    /// empty when the server started would never gain a permanent id while
    /// it ran, however many events it wrote. This answer moves with the log.
    pub fn ulid(&self) -> Option<&str> {
        self.log.first().map(|e| e.id.as_str())
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
        self.refresh_if_stale()?;
        Ok((&self.ctx, &self.log))
    }

    /// Brings the resident tree up to the log (`f599`). A log that only
    /// grew, with the last event folded still where it was, is caught up
    /// by applying just what was appended; anything else is folded whole,
    /// as it always was.
    fn refresh_if_stale(&mut self) -> Result<(), Failure> {
        let log_path = self.ctx.store.log();
        let now = store::fingerprint(&log_path);
        if now == self.seen {
            return Ok(());
        }
        // A handle keeps reading the file it opened. A log that was replaced
        // rather than grown -- a copy restored over it, a sync client, an edit
        // by hand -- would leave this reading the old one for good, and the
        // tail would come back empty every time. The two fingerprints agree
        // exactly while it is the same file, so a difference means reopen.
        let handle_is_current = self
            .log_file
            .as_ref()
            .is_some_and(|f| store::fingerprint_in(f) == now);
        if !handle_is_current {
            self.log_file = File::open(&log_path).ok();
        }
        let grew = now.0 >= self.fold_end
            && match &self.last {
                Some(last) => match &self.log_file {
                    Some(f) => crate::index::event_still_at_in(f, last),
                    None => crate::index::event_still_at(&log_path, last),
                },
                None => self.fold_end == 0,
            };
        if grew {
            let tail = self.tracked(&log_path, self.fold_end)?;
            for e in &tail.events {
                self.ctx.tree.apply(e.seq, &e.ts, &e.lane, &e.payload);
            }
            // `fold` sorts children and roots when it finishes, and applying a tail
            // event by event does not: a `num` out of order, or a node that arrived
            // before its parent and is resolved inside this tail, would leave the
            // resident tree ordered differently from a fresh one.
            self.ctx.tree.sort_nodes();
            self.committed_broken += tail.broken;
            // The unterminated tail is re-evaluated on every read, never
            // consumed, so it is added in fresh here, not accumulated the
            // way `committed_broken` is.
            self.ctx.tree.broken_lines = self.committed_broken + usize::from(tail.unterminated);
            self.log.extend(tail.events);
            self.fold_end = tail.end_offset;
            if tail.last.is_some() {
                self.last = tail.last;
            }
            self.ctx.seen = now;
        } else {
            let store = store::Store::open(self.root.clone())?;
            let store_log = store.log();
            // A whole fold is starting from scratch over the file as it
            // stands right now, so the reader does too.
            self.log_file = File::open(&store_log).ok();
            let read = self.tracked(&store_log, 0)?;
            self.committed_broken = read.broken;
            self.ctx
                .refold(store, &read.events, self.committed_broken, now);
            self.ctx.tree.broken_lines = self.committed_broken + usize::from(read.unterminated);
            self.log = read.events;
            self.fold_end = read.end_offset;
            self.last = read.last;
            #[cfg(test)]
            {
                self.full_folds += 1;
            }
        }
        self.seen = now;
        Ok(())
    }

    /// Reads from `path` at `from_offset`, through the open handle when
    /// there is one, and by path otherwise -- the open, not the read, is
    /// what the system charges for (`f599`). A handle that stopped serving
    /// is dropped and this same read falls back to path: a handle gone bad
    /// must not leave the server unable to read at all.
    fn tracked(&mut self, path: &Path, from_offset: u64) -> Result<crate::index::Tracked, Failure> {
        if let Some(f) = &self.log_file {
            match crate::index::read_tracked_in(f, path, from_offset) {
                Ok(t) => return Ok(t),
                Err(_) => self.log_file = None,
            }
        }
        crate::index::read_tracked(path, from_offset)
    }

    /// Runs a write against the tree this `Project` already keeps folded,
    /// instead of the caller building a fresh `Ctx` of its own.
    ///
    /// Stale first, same as a read: another process -- typically the CLI --
    /// may have appended since the last fold, and operating on a resident
    /// tree that has fallen behind would append at the wrong `seq` and
    /// collide with what that process just wrote. Once `f` has run, the
    /// fingerprint is taken again so the *next* call, read or write, does
    /// not pay to re-fold something this one already applied in memory --
    /// that second, avoidable fold was the actual cost `t192` measured.
    ///
    /// `d598`: held from before the refresh until after the fingerprint, so
    /// no other writer can land between what this server folded and what it
    /// appends. The window it closes is `f143`: the staleness check ran
    /// before the write, and nothing kept it true until the append.
    pub fn write<T>(
        &mut self,
        f: impl FnOnce(&mut ops::Ctx) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        self.ctx.lock_for_write()?;
        let result = self.locked_write(f);
        self.ctx.unlock();
        result
    }

    /// The body of `write`, run with the lock already held. Split out so
    /// `write` can release the lock on every path out of here -- an early
    /// return from `refresh_if_stale` or from `f` must not leave it behind
    /// (`f602`): a resident server that keeps a lock nobody is using again
    /// holds the tree for the rest of its life.
    fn locked_write<T>(
        &mut self,
        f: impl FnOnce(&mut ops::Ctx) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        self.refresh_if_stale()?;
        self.ctx.wrote = None;
        let result = f(&mut self.ctx);
        // What `f` appended is already applied to the tree (`Ctx::emit`),
        // and the append said where its lines landed, so the log is never
        // read back: reopening it right after writing it cost six
        // milliseconds a write, against a five millisecond budget (`f599`).
        if let Some(w) = self.ctx.wrote.take() {
            if w.previous_len == self.fold_end {
                if let Some(e) = w.events.last() {
                    self.last = Some(crate::index::LastEvent {
                        line_offset: w.last_line_offset,
                        id: e.id.clone(),
                        seq: e.seq,
                    });
                }
                self.fold_end = w.end_offset;
                self.log.extend(w.events);
                self.seen = store::fingerprint(&self.ctx.store.log());
            } else {
                // The log did not end where this fold stopped: something --
                // an append that died mid-write -- left bytes past it, and
                // these lines were written behind them, so they are not a
                // clean continuation. Fold the whole log next time, and let
                // the tree be exactly what is on disk.
                self.last = None;
                self.seen = (0, None);
            }
        }
        result
    }
}

/// Every root the process was asked to serve, each folded once.
pub struct Registry {
    projects: Vec<Project>,
}

impl Registry {
    /// `here` is what `store::locate` answered for the folder this process
    /// started in, paired with the tree root it belongs to (`Located.root`)
    /// -- `None` when the caller has nothing to resolve, or the starting
    /// folder is not inside any tree at all. Given to the one project among
    /// `roots` whose own root is that root, as `Whose::Resolved`, and to no
    /// other: `web` can serve several roots at once, and a folder is a lane
    /// of at most one of them. Every other project answers -- and, through
    /// `Project::write`, signs -- as `Whose::Founding`, the same as any
    /// tree nobody ran `setup` in. Getting this wrong made the resident
    /// server read `main` from a joined folder and, worse, write events
    /// signed `main` from it (`t594`, twice: the second time because
    /// `Ctx::from_events` had not caught up to `Whose` yet).
    pub fn open(
        roots: Vec<PathBuf>,
        here: Option<(PathBuf, store::Located)>,
    ) -> Result<Registry, Failure> {
        let store_dir = store::store_dir();
        Registry::open_in(roots, here, store_dir.as_deref())
    }

    /// [`Self::open`], pointed at a store directory the caller already
    /// holds instead of the one this machine resolves to. Everything above
    /// about `here` holds here too; this is where the two part ways.
    ///
    /// `None` is a machine with no registry to ask, and every project then
    /// goes by its folder. It is also what a unit test passes: `open`
    /// resolves the store the one way there is, and the one way there is
    /// refuses to run under `cfg(test)` without `VIVAC_HOME` -- on purpose,
    /// since a test that reached it would read this machine's own registry.
    pub fn open_in(
        roots: Vec<PathBuf>,
        here: Option<(PathBuf, store::Located)>,
        store_dir: Option<&Path>,
    ) -> Result<Registry, Failure> {
        if roots.is_empty() {
            return Err(Failure::usage(
                "vivac needs at least one root to serve.".to_string(),
            ));
        }
        let unique = dedup_by_target(roots);
        let pairs = assign_names_and_slugs(&unique, store_dir);
        let here_key = here
            .as_ref()
            .map(|(root, _)| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()));
        // `Located` is not `Clone` and only one project can ever match, so
        // this is taken out of `here_located` exactly once, the moment the
        // matching root is found -- `Whose::Founding` for every other.
        let (_, mut here_located) = here.map_or((None, None), |(r, l)| (Some(r), Some(l)));
        let mut projects = Vec::with_capacity(unique.len());
        for (root, (name, id)) in unique.into_iter().zip(pairs) {
            let key = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            let located_here = if here_key.as_ref() == Some(&key) {
                here_located.take()
            } else {
                None
            };
            let whose = match &located_here {
                Some(l) => ops::Whose::Resolved(l),
                None => ops::Whose::Founding,
            };
            projects.push(Project::open(root, name, id, whose)?);
        }
        Ok(Registry { projects })
    }

    /// The first root the process was given. `open` refuses an empty list, so
    /// there is always one.
    pub fn first(&mut self) -> &mut Project {
        &mut self.projects[0]
    }

    /// What a URL's `<id>` names here. The `id` is compared against what the
    /// registry already holds and never handed to the filesystem, which is
    /// what makes a `..` in a path uninteresting.
    /// Takes `&mut` for one reason: a project with no permanent id yet is
    /// one event away from having one, so it is re-read before being
    /// answered about. A project that already has one is left alone, because
    /// the log is append-only and a first event never becomes a different
    /// one. The cost is a `stat` per still-empty tree, and only until its
    /// first write.
    pub fn named(&mut self, id: &str) -> Named {
        for p in &mut self.projects {
            if p.ulid().is_none() {
                let _ = p.current();
            }
        }
        let slugs: Vec<String> = self.projects.iter().map(|p| p.slug.clone()).collect();
        let ulids: Vec<Option<String>> = self
            .projects
            .iter()
            .map(|p| p.ulid().map(str::to_string))
            .collect();
        name_or_ulid(id, &slugs, &ulids)
    }

    /// The project at a position [`named`] handed back.
    pub fn at(&mut self, i: usize) -> &mut Project {
        &mut self.projects[i]
    }

    /// Every project this process serves. `&mut` because the index page
    /// reads each one's tree, and reading re-folds when the log moved.
    pub fn all(&mut self) -> &mut [Project] {
        &mut self.projects
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

/// What a URL's `<id>` named, once the registry has been asked.
///
/// Its own type rather than an `Option` because "no project" and "more than
/// one project" are different answers and `d374` gives them different pages:
/// one is a 404, the other is a choice the reader makes.
pub enum Named {
    /// Exactly one, at this position in the registry.
    One(usize),
    /// Several, at these positions. Nothing here picks between them.
    Ambiguous(Vec<usize>),
    Unknown,
}

/// The rule `d374` decided, as a function over what the registry holds, so
/// the tests aim at it and not at a socket.
///
/// The permanent form wins first: a `<ulid>` is compared before any name, so
/// a saved link keeps opening the tree it was saved from even after another
/// project of the same name joins the registry. Then the readable form, and
/// a name held by more than one root resolves to none of them -- which is
/// what `registry::resolve` already does for `--project` on the CLI, for the
/// same reason: answering about the wrong tree while looking right is worse
/// than not answering.
///
/// Matching is exact, with no case folding. Crockford base32 is defined
/// case-insensitively, but nothing here ever emits an uppercase ULID and a
/// permanent link is copied rather than typed, so the leniency would only
/// widen what the one security-watched path accepts.
///
/// Two roots can carry the same ULID -- a copied directory carries a copied
/// log -- and that is `Ambiguous` too, deliberately: `d201` says it is
/// detected and reported, never guessed at.
fn name_or_ulid(id: &str, slugs: &[String], ulids: &[Option<String>]) -> Named {
    let by_ulid: Vec<usize> = ulids
        .iter()
        .enumerate()
        .filter(|(_, u)| u.as_deref() == Some(id))
        .map(|(i, _)| i)
        .collect();
    let hits = if by_ulid.is_empty() {
        slugs
            .iter()
            .enumerate()
            .filter(|(_, s)| *s == id)
            .map(|(i, _)| i)
            .collect()
    } else {
        by_ulid
    };
    match hits.len() {
        0 => Named::Unknown,
        1 => Named::One(hits[0]),
        _ => Named::Ambiguous(hits),
    }
}

/// The name and the slug each root goes by. Its own function because the rule
/// is the whole point of the registry, and because it is what the tests aim
/// at.
///
/// **Nothing is made unique here**, which is the change `d374` brought. The
/// slug used to gain a `-2` when two directories shared a name; `f373` showed
/// that suffix was positional, so it moved when the registry changed and a
/// saved link silently opened the other project. Uniqueness lives on the
/// ULID now, and ambiguity on the name is answered rather than papered over.
///
/// **The name is the project's, not the folder's** (`f679`). This read
/// `file_name()` and asked nothing else, so `--name` -- which every surface
/// on the CLI had read since `t640` -- reached no page at all: a product
/// checked out as `v2` was titled `v2` on its front page, in the index and
/// in its URL, while `vivac brief` in the same folder called it by its
/// name. Both halves move together on purpose: the slug is made from the
/// name, so the readable URL is the name the resolver on the CLI already
/// answers to, and the permanent one is the ULID either way.
///
/// `store_dir` is `None` for a machine with no registry to ask, and then
/// the folder is all there is -- which is exactly what this did before.
/// It is an argument rather than a second resolution for the reason
/// `registry::effective_name` gives: one resolution serves every root here.
fn assign_names_and_slugs(roots: &[PathBuf], store_dir: Option<&Path>) -> Vec<(String, String)> {
    roots
        .iter()
        .map(|r| {
            let name = store_dir
                .and_then(|d| crate::registry::effective_name(d, r))
                .or_else(|| crate::registry::folder_name(r))
                .unwrap_or_else(|| "-".into());
            let slug = sanitize(&name);
            (name, slug)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slugs_of(pairs: Vec<(String, String)>) -> Vec<String> {
        pairs.into_iter().map(|(_, slug)| slug).collect()
    }

    /// The folder-only half of the rule, which is every root below: none of
    /// these paths is a tree on this disk, so none of them has a name saved
    /// anywhere. `None` for the store keeps it that way without reading the
    /// registry of the machine running the suite -- `Registry::open_in`.
    /// The other half, a name fixed on purpose, is proved over a socket in
    /// `tests/web.rs`, since it takes a tree and a registry to have one.
    fn names_and_slugs(roots: &[PathBuf]) -> Vec<(String, String)> {
        assign_names_and_slugs(roots, None)
    }

    #[test]
    fn a_single_root_gets_its_bare_directory_name() {
        let slugs = slugs_of(names_and_slugs(&[PathBuf::from("/work/vivac")]));
        assert_eq!(slugs, vec!["vivac".to_string()]);
    }

    #[test]
    fn two_roots_with_the_same_directory_name_keep_the_same_slug() {
        // The `-2` this used to hand out was positional, and `f373` showed
        // the first root leaving the registry passed its URL to the second.
        // They collide on purpose now, and `name_or_ulid` refuses to guess.
        let slugs = slugs_of(names_and_slugs(&[
            PathBuf::from("/a/vivac"),
            PathBuf::from("/b/vivac"),
        ]));
        assert_eq!(slugs, vec!["vivac".to_string(), "vivac".to_string()]);
    }

    #[test]
    fn a_root_with_no_directory_name_falls_back_to_a_dash() {
        let slugs = slugs_of(names_and_slugs(&[PathBuf::from("/")]));
        assert_eq!(slugs, vec!["-".to_string()]);
    }

    #[test]
    fn a_name_with_characters_a_url_cannot_carry_becomes_a_slug_that_can() {
        let pairs = names_and_slugs(&[PathBuf::from("/work/my repo#1")]);
        assert_eq!(
            pairs,
            vec![("my repo#1".to_string(), "my-repo-1".to_string())]
        );
    }

    #[test]
    fn the_name_is_left_alone_however_the_slug_comes_out() {
        let pairs = names_and_slugs(&[PathBuf::from("/a/my repo"), PathBuf::from("/b/my-repo")]);
        assert_eq!(
            pairs,
            vec![
                ("my repo".to_string(), "my-repo".to_string()),
                ("my-repo".to_string(), "my-repo".to_string()),
            ]
        );
    }

    #[test]
    fn a_name_with_nothing_a_url_can_carry_still_gets_a_slug() {
        let pairs = names_and_slugs(&[PathBuf::from("/work/###")]);
        assert!(!pairs[0].1.is_empty());
    }

    fn slugs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn ulids(v: &[Option<&str>]) -> Vec<Option<String>> {
        v.iter().map(|u| u.map(|s| s.to_string())).collect()
    }

    #[test]
    fn an_unambiguous_name_names_its_project() {
        let n = name_or_ulid("ridge", &slugs(&["vivac", "ridge"]), &ulids(&[None, None]));
        assert!(matches!(n, Named::One(1)));
    }

    #[test]
    fn a_name_two_roots_share_names_them_all_and_picks_none() {
        let n = name_or_ulid(
            "vivac",
            &slugs(&["vivac", "ridge", "vivac"]),
            &ulids(&[None, None, None]),
        );
        match n {
            Named::Ambiguous(which) => assert_eq!(which, vec![0, 2]),
            _ => panic!("an ambiguous name resolved to something"),
        }
    }

    #[test]
    fn a_ulid_resolves_even_when_the_name_it_carries_is_shared() {
        // The whole point of the permanent form: this is the case where the
        // readable one cannot answer.
        let n = name_or_ulid(
            "01m1b46bb82zxqrr24twpk12rw",
            &slugs(&["vivac", "vivac"]),
            &ulids(&[
                Some("01m1zjvaj05n6tp9cw1aq1wq8h"),
                Some("01m1b46bb82zxqrr24twpk12rw"),
            ]),
        );
        assert!(matches!(n, Named::One(1)));
    }

    #[test]
    fn a_ulid_beats_a_name_that_happens_to_match_it() {
        let n = name_or_ulid(
            "01m1b46bb82zxqrr24twpk12rw",
            &slugs(&["01m1b46bb82zxqrr24twpk12rw", "other"]),
            &ulids(&[None, Some("01m1b46bb82zxqrr24twpk12rw")]),
        );
        assert!(matches!(n, Named::One(1)), "the readable form won");
    }

    #[test]
    fn two_roots_carrying_the_same_ulid_are_reported_not_guessed() {
        // A copied directory carries a copied log. `d201` says this is
        // detected and said out loud, never resolved to one of the two.
        let n = name_or_ulid(
            "01m1b46bb82zxqrr24twpk12rw",
            &slugs(&["vivac", "vivac-copy"]),
            &ulids(&[
                Some("01m1b46bb82zxqrr24twpk12rw"),
                Some("01m1b46bb82zxqrr24twpk12rw"),
            ]),
        );
        match n {
            Named::Ambiguous(which) => assert_eq!(which, vec![0, 1]),
            _ => panic!("a duplicated ULID resolved to one project"),
        }
    }

    #[test]
    fn an_id_nothing_carries_names_nothing() {
        let n = name_or_ulid("nope", &slugs(&["vivac"]), &ulids(&[Some("01m1")]));
        assert!(matches!(n, Named::Unknown));
    }

    #[test]
    fn a_tree_with_no_first_event_is_never_matched_by_an_empty_id() {
        let n = name_or_ulid("", &slugs(&["vivac"]), &ulids(&[None]));
        assert!(matches!(n, Named::Unknown));
    }

    #[test]
    fn the_same_root_given_twice_is_one_project() {
        let tmp = std::env::temp_dir().join(format!("vivac-project-t-{}", crate::id::ulid()));
        std::fs::create_dir_all(&tmp).unwrap();
        store::Store::create(&tmp).unwrap();
        let want = tmp.file_name().unwrap().to_string_lossy().into_owned();
        let mut registry = Registry::open_in(vec![tmp.clone(), tmp.clone()], None, None)
            .unwrap_or_else(|e| panic!("{}", e.message()));
        assert_eq!(registry.first().slug, want);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_tree_with_no_events_yet_has_no_permanent_id() {
        // `main.rs` calls this the late root: the command about to run is
        // often the one that writes the first event, so between `init` and
        // that write there is nothing to be keyed by. The readable form is
        // the only way in until then, and that is correct rather than
        // degraded -- there is nothing to link to yet.
        let tmp = std::env::temp_dir().join(format!("vivac-project-u-{}", crate::id::ulid()));
        std::fs::create_dir_all(&tmp).unwrap();
        store::Store::create(&tmp).unwrap();
        let mut registry = Registry::open_in(vec![tmp.clone()], None, None)
            .unwrap_or_else(|e| panic!("{}", e.message()));
        assert_eq!(registry.first().ulid(), None);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `t594`: the resident server used to sign every
    /// project `main`, hard-coded, whether or not the folder it started in
    /// was actually that project's founding lane.
    #[test]
    fn only_the_project_the_starting_folder_resolves_to_gets_its_lane() {
        let a = std::env::temp_dir().join(format!("vivac-registry-a-{}", crate::id::ulid()));
        let b = std::env::temp_dir().join(format!("vivac-registry-b-{}", crate::id::ulid()));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        store::Store::create(&a).unwrap();
        store::Store::create(&b).unwrap();

        let located_a = store::Located {
            root: a.clone(),
            lane_dir: a.clone(),
            lane: Some(crate::lane::Lane {
                version: 1,
                id: "01mLANE".to_string(),
                project: String::new(),
            }),
            worktree: None,
        };
        let mut registry = Registry::open_in(
            vec![a.clone(), b.clone()],
            Some((a.clone(), located_a)),
            None,
        )
        .unwrap_or_else(|e| panic!("{}", e.message()));

        let with_lane = registry
            .all()
            .iter()
            .find(|p| p.root == a)
            .expect("project a is in the registry");
        assert_eq!(with_lane.ctx.lane.as_deref(), Some("01mLANE"));

        // `t594`: `Whose::Founding` answers as the
        // founding lane outright now, `Some(lane::MAIN)`, rather than the
        // `None` `from_events` used to carry for "nobody said" -- the same
        // value `Whose::Resolved` reaches for a folder that resolved to
        // `main` on its own, which is what lets §6.9 tell the two apart by
        // asking the tree, not by trusting whichever one happened to
        // build this `Ctx`.
        let without_lane = registry
            .all()
            .iter()
            .find(|p| p.root == b)
            .expect("project b is in the registry");
        assert_eq!(without_lane.ctx.lane.as_deref(), Some(crate::lane::MAIN));

        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }
}
