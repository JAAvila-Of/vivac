//! The fold: from the list of events to the tree.
//!
//! The child index is built **here**, during the fold, not looked up by
//! walking every node on each query. With the Python spike it made no
//! difference; under the performance pillar's budget --`why` and `tree` over
//! ten thousand nodes below 50 ms-- a linear `children()` turns a render
//! quadratic. Indexes are thought out from the model, not bolted on when
//! they start to hurt.

use crate::anchor::AnchorRef;
use crate::event::{Body, Event, Flag, Kind, State, VivacKind};
use std::collections::BTreeMap;
use std::collections::HashMap;

/// A range of bytes inside `Tree`'s text arena.
///
/// Two integers rather than a borrowed `&str`, so a `Node` stays `Clone` and
/// needs no lifetime of its own: the arena only ever grows, so a span handed
/// out earlier keeps naming the same bytes for the life of the tree.
#[derive(Debug, Default, Clone, Copy)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

/// Resolves a span against an arena passed in directly, rather than through
/// `Tree::text`, which takes the whole `Tree` and so cannot be called while
/// another field of it is mutably borrowed.
fn span_text(arena: &str, s: Span) -> &str {
    &arena[s.start as usize..(s.start + s.len) as usize]
}

/// One note and when it was written. A note is the only thing a node can
/// receive after it is born, so the log keeps every one of them; this is
/// what the projection used to throw away (`f389`, `d390`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Note {
    pub at: Span,
    pub text: Span,
}

/// A rule's arm, resolved to spans into `Tree`'s own text arena: the folder
/// it runs in and the command itself. `d441`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArmSpan {
    pub dir: Span,
    pub command: Span,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub num: u64,
    pub kind: Kind,
    pub title: Span,
    /// Why it was born. `push` demands it: a detour with no reason is the
    /// very failure this project attacks.
    pub why: Span,
    pub state: State,
    /// The parent's `num`, not its ULID: the edge is an integer, the same as
    /// `children`, `roots` and `stack`. When the parent's ULID has no node
    /// yet -- a hand edit whose child's line landed first, or a broken line
    /// that swallowed the parent's own -- this holds a value nothing else
    /// ever answers to, minted by `Tree::resolve_pending`. If the parent
    /// does show up later, `Tree::apply_pending` rewrites it to the real
    /// `num`; if it never does, this keeps meaning exactly what it means
    /// today: no node here.
    pub parent: Option<u64>,
    /// The parent's closure condition. Explicit, and by default it does **not**
    /// block: forcing it leaves parents that never close. `MODEL.md` §5.
    pub blocks: bool,
    /// Every note this node was ever given, oldest first. The log is
    /// append-only and `apply` pushes rather than assigns, so the second
    /// note never erases the first the way this used to (`f389`, `d390`).
    pub notes: Vec<Note>,
    pub outcome: Span,
    /// A span of spans: the range, inside `Tree`'s own arena of spans, of the
    /// individual entries. Resolved with `Tree::text_list`.
    pub refs: Span,
    /// Same shape as `refs`, into the same arena.
    pub governs: Span,
    pub opened: Span,
    pub closed: Option<Span>,
    pub forced_close: bool,
    /// Flag -> reason. Orthogonal to state: a node can be `active` and
    /// `suspect` at the same time.
    pub flags: BTreeMap<Flag, Span>,
    /// A rule's arms, oldest first: the commands or tests that verify it,
    /// each with the folder it runs in (`d441`). Vivac never runs one, only
    /// stores and hands it back. Empty for a rule with none -- which means
    /// it is judged -- and for every kind that is not a rule. `d415`.
    pub arms: Vec<ArmSpan>,
}

impl Node {
    /// The fields below all read a span against the `Tree` that owns the
    /// arena it points into -- **not necessarily** the `Tree` a clone of this
    /// `Node` was taken from, though in every call site of this crate it is
    /// the same tree, since the arena is append-only and a span stays valid
    /// for its whole life.
    pub fn title<'t>(&self, tree: &'t Tree) -> &'t str {
        tree.text(self.title)
    }
    pub fn why<'t>(&self, tree: &'t Tree) -> &'t str {
        tree.text(self.why)
    }
    /// The latest note. `brief`, `tree`, `open` and the compact steps of a
    /// lineage want exactly one line here, and the newest is the one that
    /// corrects the others (`d390`).
    pub fn note<'t>(&self, tree: &'t Tree) -> &'t str {
        self.notes.last().map(|n| tree.text(n.text)).unwrap_or("")
    }
    /// Every note, oldest first, each with the date it was written.
    pub fn notes<'t>(&self, tree: &'t Tree) -> Vec<(&'t str, &'t str)> {
        self.notes
            .iter()
            .map(|n| (tree.text(n.at), tree.text(n.text)))
            .collect()
    }
    pub fn outcome<'t>(&self, tree: &'t Tree) -> &'t str {
        tree.text(self.outcome)
    }
    pub fn opened<'t>(&self, tree: &'t Tree) -> &'t str {
        tree.text(self.opened)
    }
    pub fn closed<'t>(&self, tree: &'t Tree) -> Option<&'t str> {
        self.closed.map(|s| tree.text(s))
    }
    pub fn refs<'t>(&self, tree: &'t Tree) -> Vec<&'t str> {
        tree.text_list(self.refs)
    }
    pub fn governs<'t>(&self, tree: &'t Tree) -> Vec<&'t str> {
        tree.text_list(self.governs)
    }
    /// A rule's arms, resolved to text, oldest first: the folder and the
    /// command of each one, in that order. `d441`.
    pub fn arms<'t>(&self, tree: &'t Tree) -> Vec<(&'t str, &'t str)> {
        self.arms
            .iter()
            .map(|a| (tree.text(a.dir), tree.text(a.command)))
            .collect()
    }
}

/// A safe stop. Immutable: there is no event that modifies one.
#[derive(Debug, Clone)]
pub struct Vivac {
    pub id: String,
    pub num: u64,
    /// The seq it was born at. `changes` measures a stretch from this: `ts`
    /// alone ties within the same second, and a stop cannot anchor a
    /// boundary with a number it does not remember.
    pub seq: u64,
    pub kind: VivacKind,
    pub stack: Vec<(String, String)>,
    pub working_set: Vec<String>,
    pub next_intent: String,
    pub anchor: AnchorRef,
    pub node_ref: Option<String>,
    pub label: String,
    pub ts: String,
}

impl Vivac {
    pub fn alias(&self) -> String {
        format!("v{}", self.num)
    }
}

impl Node {
    pub fn alias(&self) -> String {
        format!("{}{}", self.kind.prefix(), self.num)
    }

    /// A front is open work somebody can sit down and do.
    ///
    /// A standing decision is open and is **not** a front: you do not execute
    /// it, it governs, and it closes itself when another supersedes it.
    /// Listing it beside pending work fills the brief with things not to do,
    /// which is exactly the opposite of what it exists for.
    ///
    /// `Constraint`, `Pillar` and `Rule` are excluded for the same reason
    /// (`d414`, which keeps the first change of `d336` and widens it): a
    /// standing rule is not executed and does not close on its own either --
    /// `rules` is where it is read, not the list of what is left to do.
    pub fn is_front(&self) -> bool {
        self.state.is_open()
            && !matches!(
                self.kind,
                Kind::Decision | Kind::Constraint | Kind::Pillar | Kind::Rule
            )
    }
}

/// A `num` two different ULIDs both claimed -- a hand edit, since the log
/// itself only ever hands one out once. Recorded rather than silently
/// resolved: once `num` is the key `nodes` is stored under, only one of the
/// two can ever live there, so a scan over what survives cannot see the one
/// that lost. `check` used to find this by scanning; now it reads this.
#[derive(Debug, Clone)]
pub struct RepeatedNum {
    pub num: u64,
    /// The alias of the node that kept the number.
    pub first: String,
    /// The alias the second claimant would have had.
    pub second: String,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct Counts {
    pub total: usize,
    pub open_count: usize,
    pub closed_count: usize,
    pub parked_nodes: usize,
}

impl Counts {
    pub fn phrase(&self) -> String {
        let mut p = Vec::new();
        if self.open_count > 0 {
            p.push(format!("{} open", self.open_count));
        }
        if self.closed_count > 0 {
            p.push(format!("{} closed", self.closed_count));
        }
        if self.parked_nodes > 0 {
            p.push(format!("{} parked", self.parked_nodes));
        }
        p.join(" / ")
    }
}

#[derive(Debug, Default)]
pub struct Tree {
    /// Every node's text, appended once and never rewritten: a `Span` handed
    /// out to a `Node` stays valid for as long as the tree does.
    text: String,
    /// The arena `Node::refs` and `Node::governs` point into: each entry is
    /// itself a `Span` into `text`, so a node's own span here names a
    /// contiguous run of them -- a span of spans.
    spans: Vec<Span>,
    /// Keyed by `num`, not by the 26-byte ULID: measured, that is 7.51 ms
    /// median to build over 10 000 nodes against 1.58 with `num`, and the
    /// write budget is 5. `by_num` does not survive next to it -- with `num`
    /// as the key it would only be `nodes` again, one hop further away.
    nodes: HashMap<u64, Node>,
    children: HashMap<u64, Vec<u64>>,
    /// A ULID resolves here to the `num` a node was actually created under.
    /// Built during the fold, one entry per `NodeCreated` applied -- never
    /// for a reference that only ever names a node, such as a dangling
    /// `parent`. That is what lets a lookup that finds nothing mean "this
    /// ULID has no node" rather than requiring a second pass once the fold
    /// is done: `apply` is also what the live write path calls, one event at
    /// a time, and there is no "done" to wait for there.
    ulid_index: HashMap<String, u64>,
    /// A ULID named as a `parent` or `stack.pushed` before its own
    /// `node.created` arrived (if it ever does), mapped to the `num`
    /// `resolve_pending` minted for it while waiting. Empty on a
    /// well-formed log: nothing here is on any hot path a real write takes.
    pending: HashMap<String, u64>,
    pub roots: Vec<u64>,
    pub stack: Vec<u64>,
    pub vivacs: Vec<Vivac>,
    pub next_vivac_num: u64,
    pub seq: u64,
    /// Seq of the last event that **changed something**, and of the last
    /// vivac. Together they tell whether anything happened since the previous
    /// stop, which is what separates a useful stop from forty identical ones:
    /// Claude Code's `Stop` hook runs every turn, not at session close (`f35`).
    pub seq_change: u64,
    pub seq_vivac: u64,
    /// How many nodes were born since the last stop. An automatic stop has no
    /// declared intent --nobody was asked for one-- so what it can honestly
    /// carry is what its segment contained (`f59`).
    pub seg_new: u64,
    /// How many were settled in it.
    pub seg_closed: u64,
    /// And how much was written down against the ones already there.
    pub seg_notes: u64,
    /// Everything the segment held, births and closes and notes included. It
    /// is what the label falls back to when the segment moved the tree in some
    /// other way --a flag, a park, a bare push-- so that a stop is never blank.
    pub seg_events: u64,
    pub next_num: u64,
    pub broken_lines: usize,
    /// Every `num` a hand edit handed to two different ULIDs, in the order
    /// the fold met the second claimant. Empty on a well-formed log.
    pub repeated_nums: Vec<RepeatedNum>,
    /// Whether a pillar or a rule has ever been created, in any state.
    /// `d444`: the config's write-lock checks this once per write rather
    /// than scanning every node, and it only ever turns true -- a pillar or
    /// a rule superseded or abandoned still counts, since the config it
    /// locked stays locked.
    pub has_governance: bool,
}

pub fn fold(events: &[Event], broken: usize) -> Tree {
    let mut a = Tree {
        broken_lines: broken,
        ..Default::default()
    };
    for e in events {
        a.apply(e.seq, &e.ts, &e.payload);
    }
    a.sort_nodes();
    a
}

impl Tree {
    /// Applies one event.
    ///
    /// The fold uses it at startup and so does `emit`, right after writing.
    /// If the in-memory tree did not follow the log, every operation would
    /// print the count from **before** doing it --"back to the parent, 1 open
    /// below" for the node you just closed-- which is the kind of small lie
    /// that makes you stop trusting the rest.
    pub fn apply(&mut self, seq: u64, ts: &str, body: &Body) {
        self.seq = self.seq.max(seq);
        if matches!(body, Body::VivacCreated { .. }) {
            self.seq_vivac = self.seq_vivac.max(seq);
            self.seg_new = 0;
            self.seg_closed = 0;
            self.seg_notes = 0;
            self.seg_events = 0;
        } else if matches!(body, Body::SessionStarted { .. }) {
            // Neither a change nor a stop. Opening a session says something
            // about the session and nothing about the tree: counted as a
            // change it would arm an automatic stop for a session that did
            // nothing, and counted as a stop it would swallow the next real
            // one.
        } else {
            self.seq_change = self.seq_change.max(seq);
            self.seg_events += 1;
            match body {
                Body::NodeCreated { .. } => self.seg_new += 1,
                Body::StateChanged { state, .. } if *state == State::Done => self.seg_closed += 1,
                Body::NodeNoted { .. } => self.seg_notes += 1,
                _ => {}
            }
        }
        match body {
            Body::NodeCreated {
                node,
                num,
                kind,
                title,
                why,
                parent,
                blocks,
                refs,
                governs,
                arms,
            } => {
                if self.ulid_index.contains_key(node) {
                    // Repeated creation: commutative, the first one wins.
                    return;
                }
                if let Some(current) = self.nodes.get(num) {
                    // Two different ULIDs claiming the same `num` -- a hand
                    // edit, since `next_num` never repeats one on its own.
                    // With `num` as the key, the second one cannot be kept
                    // beside the first the way two different ULIDs used to
                    // sit side by side: one of them has to give way, and the
                    // same rule as above decides which -- the first stands.
                    // What used to be findable by scanning `nodes` afterwards
                    // is recorded here instead, since the losing side never
                    // makes it into that scan.
                    self.repeated_nums.push(RepeatedNum {
                        num: *num,
                        first: current.alias(),
                        second: format!("{}{}", kind.prefix(), num),
                    });
                    return;
                }
                if matches!(kind, Kind::Pillar | Kind::Rule) {
                    self.has_governance = true;
                }
                let title_span = self.intern(title);
                let why_span = self.intern(why);
                let refs_span = self.intern_list(refs);
                let governs_span = self.intern_list(governs);
                let arm_spans: Vec<ArmSpan> = arms
                    .iter()
                    .map(|a| ArmSpan {
                        dir: self.intern(&a.dir),
                        command: self.intern(&a.command),
                    })
                    .collect();
                let opened_span = self.intern(crate::clock::date_of(ts));
                let parent_num = parent.as_deref().map(|p| self.resolve_pending(p));
                self.nodes.insert(
                    *num,
                    Node {
                        id: node.clone(),
                        num: *num,
                        kind: *kind,
                        title: title_span,
                        why: why_span,
                        state: State::Active,
                        parent: parent_num,
                        blocks: *blocks,
                        notes: Vec::new(),
                        outcome: Span::default(),
                        refs: refs_span,
                        governs: governs_span,
                        opened: opened_span,
                        closed: None,
                        forced_close: false,
                        flags: BTreeMap::new(),
                        arms: arm_spans,
                    },
                );
                self.ulid_index.insert(node.clone(), *num);
                self.next_num = self.next_num.max(*num + 1);
                match parent_num {
                    Some(p) => self.children.entry(p).or_default().push(*num),
                    None => self.roots.push(*num),
                }
                // Whatever named this ULID before it existed -- a child's
                // `parent`, a `stack.pushed` -- gets fixed up now.
                self.apply_pending(node, *num);
            }
            Body::StateChanged {
                node,
                state,
                outcome,
                forced,
            } => {
                // Interned **before** the mutable borrow of `self.nodes`
                // below, so the two never overlap: `intern` needs the whole
                // `self`, and the borrow checker cannot see that it only
                // touches `self.text`.
                let outcome_span = (!outcome.is_empty()).then(|| self.intern(outcome));
                let closed_span =
                    (!state.is_open()).then(|| self.intern(crate::clock::date_of(ts)));
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.state = *state;
                    if let Some(s) = outcome_span {
                        n.outcome = s;
                    }
                    n.forced_close = *forced;
                    n.closed = closed_span;
                }
            }
            Body::NodeNoted { node, note } => {
                // Pushed, never assigned: the second note is a second entry
                // in the log, not a correction the tree makes in place
                // (`f389`, `d390`).
                let at = self.intern(ts);
                let text = self.intern(note);
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.notes.push(Note { at, text });
                }
            }
            Body::BlockChanged { node, blocks } => {
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.blocks = *blocks;
                }
            }
            Body::Pushed { node } => {
                // Unlike the lookups below, this persists: a `num` landing
                // on `stack` outlives the event that put it there, so a
                // node pushed before its own `node.created` needs the same
                // fix-up-on-arrival treatment as a forward-referenced
                // `parent` gets.
                let num = self.resolve_pending(node);
                if !self.stack.contains(&num) {
                    self.stack.push(num);
                }
            }
            Body::Popped { node } => {
                let num = self.resolve_ulid(node);
                self.stack.retain(|&x| x != num);
            }
            Body::FlagRaised { node, flag, reason } => {
                let reason_span = self.intern(reason);
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.flags.insert(*flag, reason_span);
                }
            }
            Body::FlagCleared { node, flag } => {
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.flags.remove(flag);
                }
            }
            Body::ArmAdded { node, dir, command } => {
                let span = ArmSpan {
                    dir: self.intern(dir),
                    command: self.intern(command),
                };
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.arms.push(span);
                }
            }
            Body::ArmRemoved { node, dir, command } => {
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    // The first arm whose pair matches, not the spans: two
                    // arms with the same text intern to two different spans,
                    // and what `arm --off` names is the pair, not which copy.
                    // `self.text` is read as a field, not through `Tree::text`
                    // -- that method takes the whole `self`, which the
                    // mutable borrow of `self.nodes` through `n` rules out.
                    if let Some(pos) = n.arms.iter().position(|a| {
                        span_text(&self.text, a.dir) == dir.as_str()
                            && span_text(&self.text, a.command) == command.as_str()
                    }) {
                        n.arms.remove(pos);
                    }
                }
            }
            Body::VivacCreated {
                vivac,
                num,
                kind,
                stack,
                working_set,
                next_intent,
                anchor,
                node_ref,
                label,
            } => {
                self.next_vivac_num = self.next_vivac_num.max(*num + 1);
                self.vivacs.push(Vivac {
                    id: vivac.clone(),
                    num: *num,
                    seq,
                    kind: *kind,
                    stack: stack.clone(),
                    working_set: working_set.clone(),
                    next_intent: next_intent.clone(),
                    anchor: anchor.clone(),
                    node_ref: node_ref.clone(),
                    label: label.clone(),
                    ts: ts.to_string(),
                });
            }
            Body::Promoted { node } => {
                let num = self.resolve_ulid(node);
                if let Some(n) = self.nodes.get_mut(&num) {
                    n.kind = Kind::Goal;
                }
                // The stack is cut at the promoted node: it becomes the root
                // of its own. The provenance chain is untouched: where it was
                // born does not change because its rank did.
                if let Some(i) = self.stack.iter().position(|&x| x == num) {
                    self.stack.drain(..i);
                }
            }
            // An opening moves nothing in the tree. What it does to the
            // counters is decided above, and it is deliberate.
            Body::SessionStarted { .. } => {}
        }
    }

    /// The `num` a ULID lives under, or `u64::MAX` when nothing has been
    /// created under it yet. Read-only, and that is enough for every event
    /// that only ever *acts* on a node -- `state.changed`, `flag.raised`,
    /// `stack.popped`, and the rest: if the ULID names nothing right now,
    /// there is nothing for a later `node.created` to complete, because
    /// none of these leave anything behind for it to find. `u64::MAX` is
    /// never a real `num`, so a lookup against it always comes back empty,
    /// the same answer a ULID that resolves to nothing gives today.
    ///
    /// `parent` and `stack.pushed` are different: both persist the ULID as
    /// a `num` that outlives this event, so a miss there has to be
    /// completable later. That is `resolve_pending`, below.
    fn resolve_ulid(&self, ulid: &str) -> u64 {
        self.ulid_index.get(ulid).copied().unwrap_or(u64::MAX)
    }

    /// The `num` a ULID names, minting one the first time it is asked for
    /// one it cannot yet answer: a hand-edited log where a child's line
    /// landed before its parent's, or a `stack.pushed` naming a node not
    /// created yet. The minted `num` is never a real one -- those only ever
    /// grow from one -- so a reference that never resolves just keeps
    /// pointing at a `num` nothing answers to, which is exactly what "does
    /// not exist" already means. If it does resolve, `apply_pending`
    /// rewrites every place this landed to the real thing.
    fn resolve_pending(&mut self, ulid: &str) -> u64 {
        if let Some(&num) = self.ulid_index.get(ulid) {
            return num;
        }
        if let Some(&num) = self.pending.get(ulid) {
            return num;
        }
        let num = u64::MAX - self.pending.len() as u64;
        self.pending.insert(ulid.to_string(), num);
        num
    }

    /// Rewrites every reference `ulid` was minted a `num` for, now that its
    /// own `node.created` has arrived under `num`: another node's `parent`,
    /// the `children` bucket it was filed under while pending, and any
    /// matching slot on `stack`. A ULID nothing was waiting on leaves this
    /// a no-op, which is the common case -- a well-formed log never has
    /// anything here to fix.
    fn apply_pending(&mut self, ulid: &str, num: u64) {
        let Some(was) = self.pending.remove(ulid) else {
            return;
        };
        for other in self.nodes.values_mut() {
            if other.parent == Some(was) {
                other.parent = Some(num);
            }
        }
        if let Some(kids) = self.children.remove(&was) {
            self.children.entry(num).or_default().extend(kids);
        }
        for slot in self.stack.iter_mut() {
            if *slot == was {
                *slot = num;
            }
        }
    }

    /// Appends `s` to the text arena and hands back the span that names it.
    /// Append-only: nothing already interned ever moves, so a span handed
    /// out earlier keeps pointing at the same bytes.
    fn intern(&mut self, s: &str) -> Span {
        let start = self.text.len() as u32;
        self.text.push_str(s);
        Span {
            start,
            len: s.len() as u32,
        }
    }

    /// Interns every one of `items` and hands back a span of spans: the
    /// range, inside `self.spans`, of the individual entries just written.
    fn intern_list(&mut self, items: &[String]) -> Span {
        let start = self.spans.len() as u32;
        for it in items {
            let span = self.intern(it);
            self.spans.push(span);
        }
        Span {
            start,
            len: items.len() as u32,
        }
    }

    /// Stable order by number: two renders of the same log are identical.
    ///
    /// Only needed while folding. Live, nodes are born with an increasing
    /// number, so appending at the end already leaves the right order.
    ///
    /// A plain sort of the entries themselves, now that they are the number:
    /// looking one up in a side table -- what this did while `children` held
    /// ULIDs -- would be sorting by the same value through an extra hop.
    pub fn sort_nodes(&mut self) {
        for v in self.children.values_mut() {
            v.sort();
        }
        self.roots.sort();
    }
}

impl Tree {
    /// Resolves a span handed out by a `Node` to the text it names.
    pub fn text(&self, span: Span) -> &str {
        &self.text[span.start as usize..(span.start + span.len) as usize]
    }

    /// Resolves a span of spans -- `Node::refs`, `Node::governs` -- to the
    /// strings it names, in the order they were written.
    pub fn text_list(&self, span: Span) -> Vec<&str> {
        self.spans[span.start as usize..(span.start + span.len) as usize]
            .iter()
            .map(|s| self.text(*s))
            .collect()
    }

    pub fn is_empty_tree(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn total(&self) -> usize {
        self.nodes.len()
    }

    /// Looks a node up by the ULID an event names it with. This is the
    /// boundary between the two: everywhere inside `Tree` an edge is a
    /// `num`, and an event is the one place a ULID still arrives from
    /// outside and has to be translated.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.ulid_index.get(id).and_then(|num| self.nodes.get(num))
    }

    /// Looks a node up by the `num` another node's own field already holds --
    /// `parent`, `roots`, `stack` -- with no ULID in between.
    pub fn node_by_num(&self, num: u64) -> Option<&Node> {
        self.nodes.get(&num)
    }

    pub fn nodes_iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Resolves whatever the user types: `7`, `t7` or the whole ULID.
    /// The bare number works on purpose --`vivac why 7`-- because forcing
    /// anyone to recall the prefix is capture cost with nothing in return.
    pub fn resolve(&self, s: &str) -> Option<&Node> {
        let clean = s.trim().trim_start_matches('#');
        if let Ok(n) = clean.parse::<u64>() {
            return self.nodes.get(&n);
        }
        // By character, not by byte. `&clean[1..]` aborts the whole process
        // when the first letter is multibyte --and the tree these ids live in
        // is written in Spanish, so a word starting with `ultima` spelled
        // properly is the ordinary case-- and again on the empty string. `f75`.
        let mut rest = clean.chars();
        let prefix = rest.next()?;
        let rest = rest.as_str();
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = rest.parse::<u64>() {
                return self.nodes.get(&n).filter(|nd| nd.kind.prefix() == prefix);
            }
        }
        self.node(clean)
    }

    pub fn children(&self, num: u64) -> Vec<&Node> {
        self.children
            .get(&num)
            .map(|v| v.iter().filter_map(|i| self.nodes.get(i)).collect())
            .unwrap_or_default()
    }

    pub fn roots(&self) -> Vec<&Node> {
        self.roots
            .iter()
            .filter_map(|i| self.nodes.get(i))
            .collect()
    }

    /// Node to root, reversed: root first. This is the path `why` walks.
    /// The `seen` set is not paranoia: a hand-edited log can hold a cycle,
    /// and hanging would be worse than giving a short path.
    pub fn ancestors(&self, num: u64) -> Vec<&Node> {
        let mut lineage = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = self.nodes.get(&num);
        while let Some(n) = cur {
            if !seen.insert(n.num) {
                break;
            }
            lineage.push(n);
            cur = n.parent.and_then(|p| self.nodes.get(&p));
        }
        lineage.reverse();
        lineage
    }

    /// The lineage from the goal a node answers to down to the node itself --
    /// the nearest goal at or above it, which may be the node. Falls back to
    /// the whole lineage when nothing above it is a goal.
    ///
    /// `ancestors` counts from the root instead, and that is a number
    /// `promote` can never move: promoting makes a node a goal without
    /// reparenting it (`d33`), so the birth chain stays exactly as long as it
    /// was. What promoting does change is which goal the nodes below answer
    /// to, and this is where that shows up (`f156`).
    pub fn under_goal(&self, num: u64) -> Vec<&Node> {
        let lineage = self.ancestors(num);
        let cut = lineage
            .iter()
            .rposition(|n| n.kind == Kind::Goal)
            .unwrap_or(0);
        lineage[cut..].to_vec()
    }

    pub fn descendants(&self, num: u64) -> Vec<&Node> {
        let mut out = Vec::new();
        let mut stack = vec![num];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            for &h in self.children.get(&cur).map(|v| v.as_slice()).unwrap_or(&[]) {
                if seen.insert(h) {
                    if let Some(n) = self.nodes.get(&h) {
                        out.push(n);
                    }
                    stack.push(h);
                }
            }
        }
        out.sort_by_key(|n| n.num);
        out
    }

    /// Open blockers of `num`: the nodes that keep it from closing.
    ///
    /// `blocks` says *this node keeps its own parent from closing*, not *any
    /// ancestor*. So `X` only counts as a blocker of `num` if there is a
    /// chain `X -> ... -> num` in which **every** link has `blocks == true`
    /// -- each one forwards the block to its own parent in turn. A child
    /// with `blocks == false` cuts the chain there: nothing beneath it can
    /// reach `num`, no matter what `blocks` says further down (`f237`).
    ///
    /// The chain descends through a blocking child regardless of that
    /// child's own state, open or closed -- only `blocks` cuts it, never
    /// `state`. A forced close on one link does not hide what is still open
    /// beneath it; it is only excluded from the result once it is itself
    /// closed.
    pub fn open_blockers(&self, num: u64) -> Vec<&Node> {
        let mut out = Vec::new();
        let mut stack = vec![num];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            for &h in self.children.get(&cur).map(|v| v.as_slice()).unwrap_or(&[]) {
                if seen.insert(h) {
                    if let Some(n) = self.nodes.get(&h) {
                        if n.blocks {
                            if n.state.is_open() {
                                out.push(n);
                            }
                            stack.push(h);
                        }
                    }
                }
            }
        }
        out.sort_by_key(|n| n.num);
        out
    }

    pub fn counts(&self, num: u64) -> Counts {
        let d = self.descendants(num);
        Counts {
            total: d.len(),
            open_count: d.iter().filter(|n| n.state == State::Active).count(),
            closed_count: d.iter().filter(|n| n.state == State::Done).count(),
            parked_nodes: d.iter().filter(|n| n.state == State::Suspended).count(),
        }
    }

    /// The most recent vivac. Vivacs are appended in event order, so the last
    /// one in the vector is the last one in time.
    pub fn last_vivac(&self) -> Option<&Vivac> {
        self.vivacs.last()
    }

    /// The last stop somebody made. `Auto` is what the `Stop` hook writes on
    /// every turn that moves the tree, and `Push`, `Pop` and `Park` ride along
    /// with the operation that caused them: `Manual` is the only kind a person
    /// sat down and wrote, which is what makes it a boundary rather than a
    /// heartbeat.
    pub fn last_manual_vivac(&self) -> Option<&Vivac> {
        self.vivacs
            .iter()
            .rev()
            .find(|v| v.kind == VivacKind::Manual)
    }

    pub fn vivac(&self, s: &str) -> Option<&Vivac> {
        let n: u64 = s.trim().trim_start_matches(['#', 'v']).parse().ok()?;
        self.vivacs.iter().find(|v| v.num == n)
    }

    pub fn focus(&self) -> Option<&Node> {
        self.stack.last().and_then(|&num| self.nodes.get(&num))
    }

    /// `focus`'s counterpart at the other end: the node this stack was opened
    /// from. The distance up to it is exactly what `stack_depth` counts, which
    /// is why the depth advice has to name this one and not the tree's first
    /// root (`f156`, `f331`).
    ///
    /// Usually a root goal, and not by invariant. `push` with nothing open has
    /// no parent and is forced to a goal, and `focus` lays the whole lineage
    /// down so the bottom is that lineage's root -- but a root reached by
    /// `promote` is whatever kind it already was, and `restore` leaves out a
    /// saved entry whose node is gone, the bottom included. Nothing here reads
    /// the kind, and nothing should start.
    pub fn stack_bottom(&self) -> Option<&Node> {
        self.stack.first().and_then(|&num| self.nodes.get(&num))
    }

    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }
}

/// Everything a fresh `Tree` needs that is not already public on it -- the
/// arena and the map `index.rs` rebuilds `ulid_index` and `children` from.
/// A constructor rather than public fields, so the arena's append-only
/// invariant stays enforced by `intern`/`intern_list` alone.
pub(crate) struct RawParts {
    pub text: String,
    pub spans: Vec<Span>,
    pub nodes: Vec<Node>,
    pub roots: Vec<u64>,
    pub stack: Vec<u64>,
    pub vivacs: Vec<Vivac>,
    pub next_vivac_num: u64,
    pub seq: u64,
    pub seq_change: u64,
    pub seq_vivac: u64,
    pub seg_new: u64,
    pub seg_closed: u64,
    pub seg_notes: u64,
    pub seg_events: u64,
    pub next_num: u64,
    pub broken_lines: usize,
}

impl Tree {
    /// Rebuilds a `Tree` from the derived index's own sections, without
    /// folding a single event. `nodes` is sorted by `num` first, so a
    /// `parent` seen earlier than its own child never happens and `children`
    /// comes out in the same ascending order `sort_nodes` leaves it in.
    ///
    /// `pending` and `repeated_nums` start empty on purpose: the index is
    /// never written while either is non-empty (`has_pending`, below, and
    /// `LOADING.md` §4 "Un log con anomalías no lleva índice"), so a tree
    /// loaded this way never had either to begin with.
    pub(crate) fn from_parts(mut p: RawParts) -> Tree {
        p.nodes.sort_by_key(|n| n.num);
        let mut nodes = HashMap::with_capacity(p.nodes.len());
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut ulid_index = HashMap::with_capacity(p.nodes.len());
        // `d444`: not carried in the index format itself -- it is cheaper to
        // re-derive over the same pass this loop already makes than to grow
        // the on-disk shape for one bit an index load can recompute for free.
        let mut has_governance = false;
        for n in p.nodes {
            ulid_index.insert(n.id.clone(), n.num);
            if let Some(parent) = n.parent {
                children.entry(parent).or_default().push(n.num);
            }
            if matches!(n.kind, Kind::Pillar | Kind::Rule) {
                has_governance = true;
            }
            nodes.insert(n.num, n);
        }
        Tree {
            text: p.text,
            spans: p.spans,
            nodes,
            children,
            ulid_index,
            pending: HashMap::new(),
            roots: p.roots,
            stack: p.stack,
            vivacs: p.vivacs,
            next_vivac_num: p.next_vivac_num,
            seq: p.seq,
            seq_change: p.seq_change,
            seq_vivac: p.seq_vivac,
            seg_new: p.seg_new,
            seg_closed: p.seg_closed,
            seg_notes: p.seg_notes,
            seg_events: p.seg_events,
            next_num: p.next_num,
            broken_lines: p.broken_lines,
            repeated_nums: Vec::new(),
            has_governance,
        }
    }

    /// The arena verbatim, for the derived index to write out. A span handed
    /// out by any `Node` in this tree stays valid against these exact bytes.
    pub(crate) fn raw_text(&self) -> &str {
        &self.text
    }

    /// The spans arena `Node::refs` and `Node::governs` point into, verbatim.
    pub(crate) fn raw_spans(&self) -> &[Span] {
        &self.spans
    }

    /// Every node, ascending by `num` -- the order the derived index stores
    /// its own table in, so loading it back never has to sort.
    pub(crate) fn nodes_sorted(&self) -> Vec<&Node> {
        let mut v: Vec<&Node> = self.nodes.values().collect();
        v.sort_by_key(|n| n.num);
        v
    }

    /// A forward reference still waiting on a node that has not arrived.
    /// The derived index is never written while this is true: loading from
    /// it skips the fold that would otherwise fix the reference up once the
    /// node does arrive, so a persisted `pending` would stay wrong forever.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Subtree counts for every node, computed in one go.
///
/// Asking each node for its own count walks its whole subtree, and doing
/// that for the whole tree makes it quadratic: measured, `tree` over ten
/// thousand nodes went from 79 ms on a subtree to 242 ms on the full tree,
/// and the 163 ms of difference were this, not the log.
///
/// A single post-order pass gets the same answer in linear time. It is the
/// kind of index the performance pillar demands be thought out from the
/// model instead of bolted on when it hurts.
#[derive(Debug, Default)]
pub struct Aggregates {
    counts: HashMap<u64, Counts>,
    blockers: HashMap<u64, usize>,
    pub max_depth: usize,
}

impl Aggregates {
    pub fn counts(&self, num: u64) -> Counts {
        self.counts.get(&num).copied().unwrap_or_default()
    }

    pub fn blockers(&self, num: u64) -> usize {
        self.blockers.get(&num).copied().unwrap_or(0)
    }
}

impl Tree {
    pub fn aggregates(&self) -> Aggregates {
        let mut ag = Aggregates::default();

        // Orphans hang off no root. They get walked anyway: a broken tree has
        // to stay inspectable, which is what `check` is for.
        let mut entries: Vec<u64> = self.roots.clone();
        entries.extend(
            self.nodes
                .values()
                .filter(|n| n.parent.is_some_and(|p| !self.nodes.contains_key(&p)))
                .map(|n| n.num),
        );

        let mut order: Vec<(u64, usize)> = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(u64, usize)> = entries.into_iter().map(|id| (id, 1)).collect();
        let mut seen = std::collections::HashSet::new();
        while let Some((id, depth_of)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            ag.max_depth = ag.max_depth.max(depth_of);
            order.push((id, depth_of));
            if let Some(hs) = self.children.get(&id) {
                stack.extend(hs.iter().map(|&h| (h, depth_of + 1)));
            }
        }

        // From the leaves upward: each parent sums what its children have plus
        // the children themselves.
        for (id, _) in order.iter().rev() {
            let mut r = Counts::default();
            let mut b = 0usize;
            for &h in self.children.get(id).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(child) = self.nodes.get(&h) else {
                    continue;
                };
                let hr = ag.counts(h);
                r.total += hr.total + 1;
                r.open_count += hr.open_count + usize::from(child.state == State::Active);
                r.closed_count += hr.closed_count + usize::from(child.state == State::Done);
                r.parked_nodes += hr.parked_nodes + usize::from(child.state == State::Suspended);
                // Same chain rule as `open_blockers` (`f237`): a count only
                // crosses `child` into `b` when `child` itself blocks. A
                // non-blocking child cuts the chain here exactly as it does
                // there, so `blockers(num)` and `open_blockers(num).len()`
                // stay two views of the one definition rather than two
                // definitions that can drift apart.
                if child.blocks {
                    b += ag.blockers(h) + usize::from(child.state == State::Active);
                }
            }
            ag.counts.insert(*id, r);
            ag.blockers.insert(*id, b);
        }
        ag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(seq: u64) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: "n1".to_string(),
                num: 1,
                kind: Kind::Goal,
                title: "Root".to_string(),
                why: "it is needed".to_string(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        }
    }

    fn node(seq: u64, num: u64, kind: Kind, parent: Option<&str>) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
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
            },
        }
    }

    fn stop(seq: u64) -> Event {
        stop_of_kind(seq, VivacKind::Manual)
    }

    fn stop_of_kind(seq: u64, kind: VivacKind) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:05:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::VivacCreated {
                vivac: format!("v{seq}"),
                num: seq,
                kind,
                stack: vec![],
                working_set: vec![],
                next_intent: String::new(),
                anchor: AnchorRef::default(),
                node_ref: None,
                label: String::new(),
            },
        }
    }

    /// The distance `triage` warns on is to the goal a node answers to, and a
    /// goal partway down the chain is the one that counts: it is the whole
    /// reason `promote` can quiet the warning at all (`f156`).
    #[test]
    fn the_lineage_under_a_goal_starts_at_the_nearest_one() {
        let events = vec![
            node(1, 1, Kind::Goal, None),
            node(2, 2, Kind::Task, Some("n1")),
            node(3, 3, Kind::Goal, Some("n2")),
            node(4, 4, Kind::Task, Some("n3")),
        ];
        let tree = fold(&events, 0);
        assert_eq!(tree.ancestors(4).len(), 4);
        let under: Vec<u64> = tree.under_goal(4).iter().map(|n| n.num).collect();
        assert_eq!(under, vec![3, 4]);
    }

    /// A goal answers to itself, which is why promoting a node takes it out of
    /// the warning along with everything below it.
    #[test]
    fn a_goal_is_its_own_goal() {
        let events = vec![
            node(1, 1, Kind::Goal, None),
            node(2, 2, Kind::Task, Some("n1")),
            node(3, 3, Kind::Goal, Some("n2")),
        ];
        let tree = fold(&events, 0);
        assert_eq!(tree.under_goal(3).len(), 1);
    }

    /// With no goal anywhere above it there is nothing nearer to count from,
    /// so the whole lineage stands in -- which is what the number meant before
    /// goals partway down existed.
    #[test]
    fn with_no_goal_above_it_the_whole_lineage_stands_in() {
        let events = vec![
            node(1, 1, Kind::Task, None),
            node(2, 2, Kind::Task, Some("n1")),
            node(3, 3, Kind::Task, Some("n2")),
        ];
        let tree = fold(&events, 0);
        assert_eq!(tree.under_goal(3).len(), 3);
    }

    /// `changes` measures a stretch from a vivac's own seq. Without it, the
    /// only boundary left to compare against would be `ts`, which ties within
    /// the same second.
    #[test]
    fn a_vivac_remembers_the_seq_it_was_created_at() {
        let events = vec![created(1), stop(2), created(3)];
        let tree = fold(&events, 0);
        assert_eq!(tree.vivacs[0].seq, 2);
    }

    /// The last stop and the last stop somebody made are different stops, and
    /// on a real tree they are usually far apart: the `Stop` hook writes one
    /// on every turn that moves the tree.
    #[test]
    fn the_last_stop_made_by_hand_skips_the_ones_the_hook_wrote() {
        let events = vec![
            stop_of_kind(1, VivacKind::Manual),
            stop_of_kind(2, VivacKind::Auto),
            stop_of_kind(3, VivacKind::Pop),
        ];
        let tree = fold(&events, 0);
        assert_eq!(tree.last_vivac().expect("a stop").num, 3);
        assert_eq!(
            tree.last_manual_vivac().expect("a stop made by hand").num,
            1
        );
    }

    /// A tree whose every stop came from the hook has none made by hand, and
    /// says so rather than handing back the nearest thing.
    #[test]
    fn a_tree_with_no_stop_made_by_hand_has_none() {
        let events = vec![stop_of_kind(1, VivacKind::Auto)];
        let tree = fold(&events, 0);
        assert!(tree.last_manual_vivac().is_none());
    }

    /// Unlike `node`, the ULID is given rather than derived from `num`: the
    /// one test that wants two different ULIDs to claim the same `num`
    /// cannot ask for that through a helper that ties the two together.
    fn node_with_id(seq: u64, ulid: &str, num: u64, kind: Kind, parent: Option<&str>) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: ulid.to_string(),
                num,
                kind,
                title: format!("Node {num}"),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        }
    }

    fn pushed(seq: u64, ulid: &str) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::Pushed {
                node: ulid.to_string(),
            },
        }
    }

    /// A hand-edited log can put a child's line before its parent's -- the
    /// parent's own line moved, or was appended out of order. Resolving the
    /// reference once, at the moment the child is folded, would leave it
    /// pointing at nothing forever even though the parent arrives two lines
    /// later: that used to be true of a `stack.pushed` naming the same not-
    /// yet-created node too, since `stack` now holds a `num` rather than the
    /// ULID that would have resolved itself once looked up fresh. Both have
    /// to land where resolving in order already does.
    #[test]
    fn a_parent_and_a_push_named_before_the_node_exists_still_resolve() {
        let events = vec![
            node(1, 2, Kind::Task, Some("n1")), // "n1" does not exist yet
            pushed(2, "n1"),                    // nor here
            node(3, 1, Kind::Goal, None),       // created last
        ];
        let tree = fold(&events, 0);
        let child = tree.node_by_num(2).expect("the child was created");
        assert_eq!(child.parent, Some(1), "the parent resolves once it exists");
        assert_eq!(tree.stack, vec![1], "the push resolves the same way");
        assert!(
            !tree.roots.contains(&2),
            "a resolved parent is not the same as none"
        );
        let siblings: Vec<u64> = tree.children(1).iter().map(|n| n.num).collect();
        assert_eq!(siblings, vec![2], "the edge lands under the real parent");
    }

    /// A hand edit can also hand two different ULIDs the same `num`. With
    /// `num` as `nodes`' own key only the first can live there, so `check`
    /// can no longer find the second by scanning survivors -- it has to be
    /// recorded at the moment it loses.
    #[test]
    fn a_repeated_number_stays_with_the_first_and_records_the_second() {
        let events = vec![
            node_with_id(1, "n1", 1, Kind::Task, None),
            node_with_id(2, "n2", 1, Kind::Finding, None), // also claims num 1
        ];
        let tree = fold(&events, 0);
        assert_eq!(tree.total(), 1, "the second claimant never lives here");
        let current = tree.node_by_num(1).expect("the first keeps the slot");
        assert_eq!(current.id, "n1");
        assert!(
            tree.node("n2").is_none(),
            "the loser is not reachable by its own ULID either"
        );
        assert_eq!(tree.repeated_nums.len(), 1);
        let repeated = &tree.repeated_nums[0];
        assert_eq!(repeated.num, 1);
        assert_eq!(repeated.first, "t1");
        assert_eq!(repeated.second, "f1");
    }

    /// Like `node`, but `blocks` is true rather than always false, so a
    /// scenario can name which links in a chain actually forward a block.
    fn node_that_blocks(seq: u64, num: u64, kind: Kind, parent: Option<&str>) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeCreated {
                node: format!("n{num}"),
                num,
                kind,
                title: format!("Node {num}"),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: true,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        }
    }

    /// Moves the node minted by `node` or `node_that_blocks` under `num` to
    /// `State::Done`.
    fn closed(seq: u64, num: u64) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:05:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::StateChanged {
                node: format!("n{num}"),
                state: State::Done,
                outcome: "done".to_string(),
                forced: false,
            },
        }
    }

    /// `f237`: `open_blockers` used to walk every descendant and filter by
    /// `blocks`, no matter what sat between it and the ancestor. Here `T`
    /// does not block `P` (`blocks = false`), so `C` underneath it -- open,
    /// with `blocks = true` -- must not count either: the link that would
    /// carry it up to `P` does not forward a block. `P` is closed, mirroring
    /// the real case this defect was found from.
    #[test]
    fn a_child_that_does_not_block_stops_the_chain() {
        let events = vec![
            node(1, 1, Kind::Goal, None),                      // P
            node(2, 2, Kind::Task, Some("n1")),                // T, blocks = false
            node_that_blocks(3, 3, Kind::Finding, Some("n2")), // C, blocks = true, open
            closed(4, 1),                                      // P closes
        ];
        let tree = fold(&events, 0);
        assert!(
            tree.open_blockers(1).is_empty(),
            "T does not block P, so nothing under T can block P either"
        );
    }

    /// The chain that does carry a block all the way up: every link between
    /// `C` and `P` has `blocks = true`. `T` blocks `P` on its own already;
    /// `C` blocks `P` too, but only because `T`'s own link forwards it --
    /// which is exactly what the chain rule requires and what `f237`'s
    /// buggy version got right for the wrong reason (it never checked `T`
    /// at all).
    #[test]
    fn a_chain_where_every_link_blocks_reaches_the_top() {
        let events = vec![
            node(1, 1, Kind::Goal, None),                      // P
            node_that_blocks(2, 2, Kind::Task, Some("n1")),    // T, blocks = true, open
            node_that_blocks(3, 3, Kind::Finding, Some("n2")), // C, blocks = true, open
        ];
        let tree = fold(&events, 0);
        let nums: Vec<u64> = tree.open_blockers(1).iter().map(|n| n.num).collect();
        assert_eq!(
            nums,
            vec![2, 3],
            "both T (direct) and C (through T's own block) reach P"
        );
    }

    /// A link that is itself closed still forwards what is open beneath it:
    /// forcing `T` shut does not erase what `C` still owes `P`.
    #[test]
    fn a_closed_link_does_not_stop_what_blocks_under_it() {
        let events = vec![
            node(1, 1, Kind::Goal, None),                      // P
            node_that_blocks(2, 2, Kind::Task, Some("n1")),    // T, blocks = true
            node_that_blocks(3, 3, Kind::Finding, Some("n2")), // C, blocks = true, open
            closed(4, 2),                                      // T closes
        ];
        let tree = fold(&events, 0);
        let nums: Vec<u64> = tree.open_blockers(1).iter().map(|n| n.num).collect();
        assert_eq!(nums, vec![3], "T closing does not stop C from blocking P");
    }

    /// The output stays sorted by `num` across more than one blocking chain,
    /// same as before the fix.
    #[test]
    fn open_blockers_from_two_chains_come_back_sorted() {
        let events = vec![
            node(1, 1, Kind::Goal, None),                      // P
            node_that_blocks(2, 4, Kind::Finding, Some("n1")), // second branch, minted first
            node_that_blocks(3, 2, Kind::Finding, Some("n1")), // first branch, minted second
        ];
        let tree = fold(&events, 0);
        let nums: Vec<u64> = tree.open_blockers(1).iter().map(|n| n.num).collect();
        assert_eq!(nums, vec![2, 4]);
    }

    /// `Aggregates::blockers` and `Tree::open_blockers` answer the same
    /// question -- how many open blockers does this node have -- from two
    /// different passes over the tree, one a count and one a list. `f237`
    /// showed what happens when only one of the two gets the chain rule: the
    /// binary starts disagreeing with itself. A tree mixing a cut-off branch
    /// (`blocks = false`), a chain that reaches the top, and a closed link
    /// that still forwards what is under it is exactly the shape that would
    /// tell the two implementations apart if only one of them had the fix.
    #[test]
    fn the_blockers_count_agrees_with_open_blockers_on_every_node() {
        let events = vec![
            node(1, 1, Kind::Goal, None),                      // P
            node(2, 2, Kind::Task, Some("n1")),                // T1, blocks = false: cut off
            node_that_blocks(3, 3, Kind::Finding, Some("n2")), // C1, under the cut branch
            node_that_blocks(4, 4, Kind::Task, Some("n1")),    // T2, blocks = true, open
            node_that_blocks(5, 5, Kind::Finding, Some("n4")), // C2, blocks = true, open
            node_that_blocks(6, 6, Kind::Finding, Some("n4")), // C3, blocks = true, closes below
            node_that_blocks(7, 7, Kind::Finding, Some("n6")), // C4, under the closed C3
            node_that_blocks(8, 8, Kind::Task, Some("n1")),    // T3, blocks = true, closes below
            node_that_blocks(9, 9, Kind::Finding, Some("n8")), // C5, under the closed T3
            closed(10, 6),                                     // C3 closes
            closed(11, 8),                                     // T3 closes
        ];
        let tree = fold(&events, 0);
        let ag = tree.aggregates();
        for n in tree.nodes_iter() {
            assert_eq!(
                ag.blockers(n.num),
                tree.open_blockers(n.num).len(),
                "node {} disagrees between the aggregate count and the list",
                n.num
            );
        }
    }

    fn noted(seq: u64, ts: &str, num: u64, note: &str) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: ts.to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload: Body::NodeNoted {
                node: format!("n{num}"),
                note: note.to_string(),
            },
        }
    }

    /// `f389`: a second note used to overwrite the first everywhere a node's
    /// note was read. It has to survive instead, in the order it was
    /// written, each one carrying the moment it was written.
    #[test]
    fn a_second_note_does_not_erase_the_first() {
        let events = vec![
            node(1, 1, Kind::Task, None),
            noted(2, "2026-09-01T00:00:00Z", 1, "first note"),
            noted(3, "2026-09-02T00:00:00Z", 1, "second note"),
        ];
        let tree = fold(&events, 0);
        let n = tree.node_by_num(1).unwrap();
        assert_eq!(
            n.notes(&tree),
            vec![
                ("2026-09-01T00:00:00Z", "first note"),
                ("2026-09-02T00:00:00Z", "second note"),
            ],
            "both notes survive, oldest first"
        );
        assert_eq!(n.note(&tree), "second note", "note() still reads the last");
    }
}
