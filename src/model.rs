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

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub num: u64,
    pub kind: Kind,
    pub title: String,
    /// Why it was born. `push` demands it: a detour with no reason is the
    /// very failure this project attacks.
    pub why: String,
    pub state: State,
    pub parent: Option<String>,
    /// The parent's closure condition. Explicit, and by default it does **not**
    /// block: forcing it leaves parents that never close. `MODEL.md` §5.
    pub blocks: bool,
    pub note: String,
    pub outcome: String,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub opened: String,
    pub closed: Option<String>,
    pub forced_close: bool,
    /// Flag -> reason. Orthogonal to state: a node can be `active` and
    /// `suspect` at the same time.
    pub flags: BTreeMap<Flag, String>,
}

/// A safe stop. Immutable: there is no event that modifies one.
#[derive(Debug, Clone)]
pub struct Vivac {
    pub id: String,
    pub num: u64,
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
    pub fn is_front(&self) -> bool {
        self.state.is_open() && self.kind != Kind::Decision
    }
}

#[derive(Debug, Default, Clone, Copy)]
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
    nodes: HashMap<String, Node>,
    children: HashMap<String, Vec<String>>,
    por_num: HashMap<u64, String>,
    pub roots: Vec<String>,
    pub stack: Vec<String>,
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
}

pub fn fold(eventos: &[Event], rotas: usize) -> Tree {
    let mut a = Tree {
        broken_lines: rotas,
        ..Default::default()
    };
    for e in eventos {
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
            } => {
                if self.nodes.contains_key(node) {
                    // Repeated creation: commutative, the first one wins.
                    return;
                }
                self.nodes.insert(
                    node.clone(),
                    Node {
                        id: node.clone(),
                        num: *num,
                        kind: *kind,
                        title: title.clone(),
                        why: why.clone(),
                        state: State::Active,
                        parent: parent.clone(),
                        blocks: *blocks,
                        note: String::new(),
                        outcome: String::new(),
                        refs: refs.clone(),
                        governs: governs.clone(),
                        opened: crate::clock::date_of(ts).to_string(),
                        closed: None,
                        forced_close: false,
                        flags: BTreeMap::new(),
                    },
                );
                self.por_num.insert(*num, node.clone());
                self.next_num = self.next_num.max(*num + 1);
                match parent {
                    Some(p) => self
                        .children
                        .entry(p.clone())
                        .or_default()
                        .push(node.clone()),
                    None => self.roots.push(node.clone()),
                }
            }
            Body::StateChanged {
                node,
                state,
                outcome,
                forced,
            } => {
                if let Some(n) = self.nodes.get_mut(node) {
                    n.state = *state;
                    if !outcome.is_empty() {
                        n.outcome = outcome.clone();
                    }
                    n.forced_close = *forced;
                    n.closed = if state.is_open() {
                        None
                    } else {
                        Some(crate::clock::date_of(ts).to_string())
                    };
                }
            }
            Body::NodeNoted { node, note } => {
                if let Some(n) = self.nodes.get_mut(node) {
                    n.note = note.clone();
                }
            }
            Body::BlockChanged { node, blocks } => {
                if let Some(n) = self.nodes.get_mut(node) {
                    n.blocks = *blocks;
                }
            }
            Body::Pushed { node } => {
                if !self.stack.contains(node) {
                    self.stack.push(node.clone());
                }
            }
            Body::Popped { node } => {
                self.stack.retain(|x| x != node);
            }
            Body::FlagRaised { node, flag, reason } => {
                if let Some(n) = self.nodes.get_mut(node) {
                    n.flags.insert(*flag, reason.clone());
                }
            }
            Body::FlagCleared { node, flag } => {
                if let Some(n) = self.nodes.get_mut(node) {
                    n.flags.remove(flag);
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
                if let Some(n) = self.nodes.get_mut(node) {
                    n.kind = Kind::Goal;
                }
                // The stack is cut at the promoted node: it becomes the root
                // of its own. The provenance chain is untouched: where it was
                // born does not change because its rank did.
                if let Some(i) = self.stack.iter().position(|x| x == node) {
                    self.stack.drain(..i);
                }
            }
            // An opening moves nothing in the tree. What it does to the
            // counters is decided above, and it is deliberate.
            Body::SessionStarted { .. } => {}
        }
    }

    /// Stable order by number: two renders of the same log are identical.
    ///
    /// Only needed while folding. Live, nodes are born with an increasing
    /// number, so appending at the end already leaves the right order.
    pub fn sort_nodes(&mut self) {
        let nums: std::collections::HashMap<String, u64> =
            self.nodes.iter().map(|(k, n)| (k.clone(), n.num)).collect();
        for v in self.children.values_mut() {
            v.sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
        }
        self.roots
            .sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
    }
}

impl Tree {
    pub fn is_empty_tree(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn total(&self) -> usize {
        self.nodes.len()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn nodes_iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Resolves whatever the user types: `7`, `t7` or the whole ULID.
    /// The bare number works on purpose --`vivac why 7`-- because forcing
    /// anyone to recall the prefix is capture cost with nothing in return.
    pub fn resolve(&self, s: &str) -> Option<&Node> {
        let limpio = s.trim().trim_start_matches('#');
        if let Ok(n) = limpio.parse::<u64>() {
            return self.por_num.get(&n).and_then(|id| self.nodes.get(id));
        }
        let sin_prefijo = &limpio[1..];
        if limpio.len() > 1 && sin_prefijo.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = sin_prefijo.parse::<u64>() {
                return self
                    .por_num
                    .get(&n)
                    .and_then(|id| self.nodes.get(id))
                    .filter(|nd| nd.kind.prefix() == limpio.chars().next().unwrap());
            }
        }
        self.nodes.get(limpio)
    }

    pub fn children(&self, id: &str) -> Vec<&Node> {
        self.children
            .get(id)
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
    pub fn ancestors(&self, id: &str) -> Vec<&Node> {
        let mut lineage = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = self.nodes.get(id);
        while let Some(n) = cur {
            if !seen.insert(&n.id) {
                break;
            }
            lineage.push(n);
            cur = n.parent.as_deref().and_then(|p| self.nodes.get(p));
        }
        lineage.reverse();
        lineage
    }

    pub fn descendants(&self, id: &str) -> Vec<&Node> {
        let mut out = Vec::new();
        let mut stack = vec![id.to_string()];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            for h in self.children.get(&cur).map(|v| v.as_slice()).unwrap_or(&[]) {
                if seen.insert(h.clone()) {
                    if let Some(n) = self.nodes.get(h) {
                        out.push(n);
                    }
                    stack.push(h.clone());
                }
            }
        }
        out.sort_by_key(|n| n.num);
        out
    }

    /// Open descendants marked as a closure condition.
    ///
    /// **Transitive on purpose**: a blocking grandchild blocks the grandparent.
    /// Without that, slipping one node in between is enough to skip the guard
    /// by accident, which is exactly how a false close gets in.
    pub fn open_blockers(&self, id: &str) -> Vec<&Node> {
        self.descendants(id)
            .into_iter()
            .filter(|n| n.blocks && n.state.is_open())
            .collect()
    }

    pub fn counts(&self, id: &str) -> Counts {
        let d = self.descendants(id);
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

    pub fn vivac(&self, s: &str) -> Option<&Vivac> {
        let n: u64 = s.trim().trim_start_matches(['#', 'v']).parse().ok()?;
        self.vivacs.iter().find(|v| v.num == n)
    }

    pub fn focus(&self) -> Option<&Node> {
        self.stack.last().and_then(|id| self.nodes.get(id))
    }

    pub fn stack_depth(&self) -> usize {
        self.stack.len()
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
    counts: HashMap<String, Counts>,
    blockers: HashMap<String, usize>,
    pub max_depth: usize,
}

impl Aggregates {
    pub fn counts(&self, id: &str) -> Counts {
        self.counts.get(id).copied().unwrap_or_default()
    }

    pub fn blockers(&self, id: &str) -> usize {
        self.blockers.get(id).copied().unwrap_or(0)
    }
}

impl Tree {
    pub fn aggregates(&self) -> Aggregates {
        let mut ag = Aggregates::default();

        // Orphans hang off no root. They get walked anyway: a broken tree has
        // to stay inspectable, which is what `check` is for.
        let mut entries: Vec<&String> = self.roots.iter().collect();
        entries.extend(
            self.nodes
                .values()
                .filter(|n| {
                    n.parent
                        .as_ref()
                        .is_some_and(|p| !self.nodes.contains_key(p))
                })
                .map(|n| &n.id),
        );

        let mut order: Vec<(&String, usize)> = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(&String, usize)> = entries.into_iter().map(|id| (id, 1)).collect();
        let mut seen = std::collections::HashSet::new();
        while let Some((id, depth_of)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            ag.max_depth = ag.max_depth.max(depth_of);
            order.push((id, depth_of));
            if let Some(hs) = self.children.get(id) {
                stack.extend(hs.iter().map(|h| (h, depth_of + 1)));
            }
        }

        // From the leaves upward: each parent sums what its children have plus
        // the children themselves.
        for (id, _) in order.iter().rev() {
            let mut r = Counts::default();
            let mut b = 0usize;
            for h in self.children.get(*id).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(child) = self.nodes.get(h) else {
                    continue;
                };
                let hr = ag.counts(h);
                r.total += hr.total + 1;
                r.open_count += hr.open_count + usize::from(child.state == State::Active);
                r.closed_count += hr.closed_count + usize::from(child.state == State::Done);
                r.parked_nodes += hr.parked_nodes + usize::from(child.state == State::Suspended);
                b += ag.blockers(h) + usize::from(child.blocks && child.state == State::Active);
            }
            ag.counts.insert((*id).clone(), r);
            ag.blockers.insert((*id).clone(), b);
        }
        ag
    }
}
