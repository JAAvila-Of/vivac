//! `changes` — what a stretch of work moved: opened, closed, flagged, moved.
//!
//! `t148`: the log already has every event and no read hands them back
//! grouped by what happened, so answering "what did this stretch move" meant
//! opening `events` by hand. The boundary is a stop and never a raw
//! timestamp, because a timestamp ties within the same second and a stop's
//! `seq` does not.
//!
//! `t155`: the log's last stop is not the boundary anybody wants. The `Stop`
//! hook writes one on every turn that moves the tree -- 23 in a single day on
//! this project's own tree, 19 of them automatic -- so "since the last stop"
//! answers "since my last action" where the question was "since the last time
//! I looked". `--since manual` measures from the last stop somebody sat down
//! and made, which is the only one of the five kinds a person chooses to
//! write.

use crate::args::Args;
use crate::event::{Body, Event, Flag, State};
use crate::failure::Failure;
use crate::model::{Node, Tree, Vivac};
use crate::output::outln;
use crate::render::{print_json, wrap, WIDTH};
use serde_json::json;

/// Where a stretch is measured from, and how that place was chosen. Both live
/// in one type because the sentence the header prints depends on the two: the
/// same stop reads differently when it is merely the log's most recent one and
/// when it is the last one somebody sat down and made.
pub enum Boundary<'a> {
    Stop {
        vivac: &'a Vivac,
        /// Picked by `--since manual`, rather than being the log's last stop
        /// or one named outright.
        manual: bool,
    },
    /// The whole log. `asked_for_manual` tells the two ways of arriving here
    /// apart: a log with no stops at all, and `--since manual` on a log where
    /// every stop was written by the hook. Same stretch, different sentence.
    Beginning { asked_for_manual: bool },
}

pub struct Changed<'a> {
    /// The boundary the stretch is measured from.
    pub since: Boundary<'a>,
    pub opened: Vec<Opened<'a>>,
    pub closed: Vec<Closed<'a>>,
    pub flagged: Vec<Flagged<'a>>,
    pub moved: Vec<Moved<'a>>,
    pub tail: Tail,
}

/// A node born in this stretch, and the lane whose event bore it: `changes`
/// stays the whole tree's own reading, and `lane` is what lets a line from
/// another lane carry its name (`t594` §4.3).
pub struct Opened<'a> {
    pub node: &'a Node,
    pub lane: String,
}

pub struct Closed<'a> {
    pub node: &'a Node,
    pub outcome: String,
    pub forced: bool,
    pub lane: String,
}

pub struct Flagged<'a> {
    pub node: &'a Node,
    pub flag: Flag,
    pub reason: String,
    pub lane: String,
}

pub struct Moved<'a> {
    pub node: &'a Node,
    /// The word this state goes by for this kind: `State::word` already
    /// knows it, and a second spelling here would drift from the first.
    pub state: State,
    pub lane: String,
}

/// What moved the tree without naming a node movement. It is counted and
/// never dropped: a stretch that only moved the focus must not read as a
/// stretch where nothing happened.
#[derive(Default)]
pub struct Tail {
    pub focus_moves: usize,
    pub notes: usize,
    pub flags_cleared: usize,
    pub edges: usize,
    /// A rule armed or disarmed: `arm.added` or `arm.removed`.
    pub arms: usize,
    /// A `declare` call: one `against.added`, however many declarations it
    /// carried. `t426` §3.3.
    pub declarations: usize,
    /// Stops inside the stretch. Not printed in the tail: it goes in the
    /// header, where it says how far back the boundary is, which is the one
    /// place the number means something.
    pub stops: usize,
    /// Events naming a node this log cannot read. `vivac check` is the
    /// surface that explains why; dropping them without a count would be
    /// omitting in silence.
    pub unreadable: usize,
}

/// Everything in the log after `since_seq`, grouped by what happened.
///
/// Ordered by the log within each group, not by node number: a stretch reads
/// as a story, not as a lookup, and the log already comes in the order it
/// was written.
pub fn collect<'a>(tree: &'a Tree, log: &[Event], since_seq: u64) -> Changed<'a> {
    let mut result = Changed {
        since: Boundary::Beginning {
            asked_for_manual: false,
        },
        opened: Vec::new(),
        closed: Vec::new(),
        flagged: Vec::new(),
        moved: Vec::new(),
        tail: Tail::default(),
    };

    for e in log {
        if e.seq <= since_seq {
            continue;
        }
        match &e.payload {
            // Opening a session says something about the session and
            // nothing about the tree; `Tree::apply` already treats it that
            // way.
            Body::SessionStarted { .. } => {}
            Body::NodeCreated { node, .. } => match tree.node(node) {
                Some(n) => result.opened.push(Opened {
                    node: n,
                    lane: e.lane.clone(),
                }),
                None => result.tail.unreadable += 1,
            },
            Body::StateChanged {
                node,
                state,
                outcome,
                forced,
            } => match tree.node(node) {
                Some(n) if *state == State::Done => result.closed.push(Closed {
                    node: n,
                    outcome: outcome.clone(),
                    forced: *forced,
                    lane: e.lane.clone(),
                }),
                Some(n) => result.moved.push(Moved {
                    node: n,
                    state: *state,
                    lane: e.lane.clone(),
                }),
                None => result.tail.unreadable += 1,
            },
            Body::FlagRaised { node, flag, reason } => match tree.node(node) {
                Some(n) => result.flagged.push(Flagged {
                    node: n,
                    flag: *flag,
                    reason: reason.clone(),
                    lane: e.lane.clone(),
                }),
                None => result.tail.unreadable += 1,
            },
            Body::FlagCleared { node, .. } => match tree.node(node) {
                Some(_) => result.tail.flags_cleared += 1,
                None => result.tail.unreadable += 1,
            },
            Body::NodeNoted { node, .. } => match tree.node(node) {
                Some(_) => result.tail.notes += 1,
                None => result.tail.unreadable += 1,
            },
            Body::BlockChanged { node, .. } => match tree.node(node) {
                Some(_) => result.tail.edges += 1,
                None => result.tail.unreadable += 1,
            },
            Body::Pushed { node } | Body::Popped { node } | Body::Promoted { node } => {
                match tree.node(node) {
                    Some(_) => result.tail.focus_moves += 1,
                    None => result.tail.unreadable += 1,
                }
            }
            Body::ArmAdded { node, .. } | Body::ArmRemoved { node, .. } => match tree.node(node) {
                Some(_) => result.tail.arms += 1,
                None => result.tail.unreadable += 1,
            },
            Body::AgainstAdded { node, .. } => match tree.node(node) {
                Some(_) => result.tail.declarations += 1,
                None => result.tail.unreadable += 1,
            },
            // Not naming a node: nothing to check against the tree.
            Body::VivacCreated { .. } => result.tail.stops += 1,
            // Neither names a node, and `Tree::apply` already keeps either
            // one from counting as work: joining or being claimed is not a
            // change, so there is nothing here for it to add.
            Body::LaneDeclared { .. } | Body::LaneClaimed { .. } => {}
        }
    }

    result
}

impl Boundary<'_> {
    /// The seq a stretch starts after. Exclusive: the stop itself belongs to
    /// the stretch before it.
    pub(crate) fn seq(&self) -> u64 {
        match self {
            Boundary::Stop { vivac, .. } => vivac.seq,
            Boundary::Beginning { .. } => 0,
        }
    }
}

/// The boundary `--since manual` measures from: the last stop somebody sat
/// down and made, or the whole log if there has never been one.
///
/// Its own function so a caller that is not the CLI -- `t160`'s Today page,
/// which owes its "what changed" block this same limit -- asks the tree for
/// it directly instead of re-deriving it, and the two can never disagree.
pub(crate) fn manual_boundary(tree: &Tree) -> Boundary<'_> {
    match tree.last_manual_vivac() {
        Some(v) => Boundary::Stop {
            vivac: v,
            manual: true,
        },
        None => Boundary::Beginning {
            asked_for_manual: true,
        },
    }
}

/// `changes` — what a stretch of work moved, printed with `triage`'s style.
///
/// Always exits `0` when the command itself was well formed: this is a
/// reading, not a check, and an empty stretch is answered with a sentence
/// rather than a non-zero code.
pub fn changes(tree: &Tree, log: &[Event], args: &Args) -> Result<i32, Failure> {
    let boundary = match args.opt("since") {
        Some("manual") => manual_boundary(tree),
        Some(s) => Boundary::Stop {
            vivac: tree.vivac(s).ok_or_else(|| {
                Failure::usage(format!(
                    "No such vivac: {s}. Give a stop's alias, or `manual` for the last stop you made."
                ))
            })?,
            manual: false,
        },
        None => match tree.last_vivac() {
            Some(v) => Boundary::Stop {
                vivac: v,
                manual: false,
            },
            None => Boundary::Beginning {
                asked_for_manual: false,
            },
        },
    };
    let mut result = collect(tree, log, boundary.seq());
    result.since = boundary;

    if args.has("json") {
        return print_json(as_json(tree, &result)).map(|_| 0);
    }
    print_text(tree, &result);
    Ok(0)
}

/// A trailing "s" where the count calls for one, and none where it does not.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `CHANGES SINCE ...`, in one of six forms and never a seventh: the log's
/// last stop, an older one with how many stops lie between it and now, the
/// last stop somebody made with how many the hook wrote after it, or the
/// beginning of the log, which itself splits in two depending on whether a
/// stop made by hand was asked for and not found. It carries no clock time on
/// purpose: `now_rfc3339` writes in UTC, and a bare hour would read as local
/// to whoever is looking at it.
fn header(since: &Boundary, stops_since: usize) -> String {
    match since {
        Boundary::Beginning {
            asked_for_manual: false,
        } => "  CHANGES SINCE THE BEGINNING - no stops yet".to_string(),
        Boundary::Beginning {
            asked_for_manual: true,
        } => "  CHANGES SINCE THE BEGINNING - no stop here was made by hand".to_string(),
        Boundary::Stop { vivac, manual } => {
            let date = crate::clock::date_of(&vivac.ts);
            match (manual, stops_since) {
                (false, 0) => format!("  CHANGES SINCE {}, the last stop - {date}", vivac.alias()),
                (false, n) => format!(
                    "  CHANGES SINCE {} - {date}, {n} stop{} ago",
                    vivac.alias(),
                    plural(n)
                ),
                (true, 0) => format!(
                    "  CHANGES SINCE {}, the last stop you made - {date}",
                    vivac.alias()
                ),
                (true, n) => format!(
                    "  CHANGES SINCE {}, the last stop you made - {date}, {n} stop{} since",
                    vivac.alias(),
                    plural(n)
                ),
            }
        }
    }
}

/// The tail line, naming only what is not zero, in a fixed order. `stops`
/// never appears here: it already spoke in the header.
pub(crate) fn tail_phrase(tail: &Tail) -> Option<String> {
    let mut parts = Vec::new();
    if tail.focus_moves > 0 {
        parts.push(format!(
            "{} focus move{}",
            tail.focus_moves,
            plural(tail.focus_moves)
        ));
    }
    if tail.notes > 0 {
        parts.push(format!("{} note{}", tail.notes, plural(tail.notes)));
    }
    if tail.flags_cleared > 0 {
        parts.push(format!(
            "{} flag{} cleared",
            tail.flags_cleared,
            plural(tail.flags_cleared)
        ));
    }
    if tail.edges > 0 {
        parts.push(format!("{} edge change{}", tail.edges, plural(tail.edges)));
    }
    if tail.arms > 0 {
        parts.push(format!("{} arm change{}", tail.arms, plural(tail.arms)));
    }
    if tail.declarations > 0 {
        parts.push(format!(
            "{} late declaration{}",
            tail.declarations,
            plural(tail.declarations)
        ));
    }
    if tail.unreadable > 0 {
        parts.push(format!(
            "{} unreadable event{}",
            tail.unreadable,
            plural(tail.unreadable)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The name to show for `lane`: its own declared name when it has one, and
/// the bare lane otherwise -- the same rule `Tree::lane_name` uses for the
/// lane a tree is looked at from, applied here to a lane that only ever
/// wrote one line of this stretch.
fn lane_label(tree: &Tree, lane: &str) -> String {
    match tree.lanes.get(lane) {
        Some(s) if !s.name.is_empty() => s.name.clone(),
        _ => lane.to_string(),
    }
}

/// What a line signed by `lane` carries in front of it: nothing when it is
/// the lane this stretch is being read from, and the other lane's own name
/// otherwise. Empty on a single-lane tree, because `lane` never differs
/// from `tree.lane()` there -- which is what keeps this stretch printing
/// the exact bytes it always has (`t594` §4.3).
fn foreign_mark(tree: &Tree, lane: &str) -> String {
    if lane == tree.lane() {
        String::new()
    } else {
        format!("[{}] ", lane_label(tree, lane))
    }
}

fn print_text(tree: &Tree, result: &Changed) {
    outln!();
    outln!("{}", header(&result.since, result.tail.stops));

    let mut said_something = false;

    if !result.opened.is_empty() {
        said_something = true;
        outln!();
        outln!("  OPENED ({})", result.opened.len());
        for o in &result.opened {
            outln!(
                "    {}{:<6} {}",
                foreign_mark(tree, &o.lane),
                o.node.alias(),
                o.node.title(tree)
            );
        }
    }

    if !result.closed.is_empty() {
        said_something = true;
        outln!();
        outln!("  CLOSED ({})", result.closed.len());
        for c in &result.closed {
            outln!(
                "    {}{:<6} {}",
                foreign_mark(tree, &c.lane),
                c.node.alias(),
                c.node.title(tree)
            );
            let line = if c.forced {
                if c.outcome.is_empty() {
                    "forced".to_string()
                } else {
                    format!("forced: {}", c.outcome)
                }
            } else {
                c.outcome.clone()
            };
            for l in wrap(&line, WIDTH, "           ") {
                outln!("{l}");
            }
        }
    }

    if !result.flagged.is_empty() {
        said_something = true;
        outln!();
        outln!("  FLAGGED ({})", result.flagged.len());
        for f in &result.flagged {
            outln!(
                "    {}{:<6} {}",
                foreign_mark(tree, &f.lane),
                f.node.alias(),
                f.node.title(tree)
            );
            for l in wrap(
                &format!("{}: {}", f.flag.word(), f.reason),
                WIDTH,
                "           ",
            ) {
                outln!("{l}");
            }
        }
    }

    if !result.moved.is_empty() {
        said_something = true;
        outln!();
        outln!("  MOVED ({})", result.moved.len());
        for m in &result.moved {
            outln!(
                "    {}{:<6} {}",
                foreign_mark(tree, &m.lane),
                m.node.alias(),
                m.node.title(tree)
            );
            let word = m.state.word(m.node.kind);
            for l in wrap(
                &format!("{word}: {}", m.node.outcome(tree)),
                WIDTH,
                "           ",
            ) {
                outln!("{l}");
            }
        }
    }

    if let Some(t) = tail_phrase(&result.tail) {
        said_something = true;
        outln!();
        outln!("  + {t}");
    }

    if !said_something {
        outln!();
        match &result.since {
            Boundary::Stop { vivac, .. } => {
                outln!("  Nothing has moved since {}.", vivac.alias())
            }
            Boundary::Beginning { .. } => outln!("  Nothing has moved."),
        }
    }
    outln!();
}

fn as_json(tree: &Tree, result: &Changed) -> serde_json::Value {
    json!({
        // `kind` says how the boundary was chosen, not what kind of stop it
        // is: `--since v122` on a stop somebody made still reads `stop`,
        // because naming it outright is not the same question as asking for
        // the last one made by hand.
        "since": match &result.since {
            Boundary::Stop { vivac, manual } => json!({
                "kind": if *manual { "manual" } else { "stop" },
                "alias": vivac.alias(),
                "ts": vivac.ts,
                "stops_since": result.tail.stops,
            }),
            Boundary::Beginning { .. } => serde_json::Value::Null,
        },
        "opened": result.opened.iter().map(|o| json!({
            "alias": o.node.alias(),
            "title": o.node.title(tree),
            "kind": o.node.kind,
        })).collect::<Vec<_>>(),
        "closed": result.closed.iter().map(|c| json!({
            "alias": c.node.alias(),
            "title": c.node.title(tree),
            "outcome": c.outcome,
            "forced": c.forced,
        })).collect::<Vec<_>>(),
        "flagged": result.flagged.iter().map(|f| json!({
            "alias": f.node.alias(),
            "title": f.node.title(tree),
            "flag": f.flag.word(),
            "reason": f.reason,
        })).collect::<Vec<_>>(),
        "moved": result.moved.iter().map(|m| json!({
            "alias": m.node.alias(),
            "title": m.node.title(tree),
            "state": m.state.word(m.node.kind),
        })).collect::<Vec<_>>(),
        "tail": {
            "focus_moves": result.tail.focus_moves,
            "notes": result.tail.notes,
            "flags_cleared": result.tail.flags_cleared,
            "edge_changes": result.tail.edges,
            "arm_changes": result.tail.arms,
            "late_declarations": result.tail.declarations,
            "unreadable": result.tail.unreadable,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Kind;
    use crate::model::fold;

    fn ev(seq: u64, payload: Body) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload,
        }
    }

    fn node_created(seq: u64, node: &str, num: u64, title: &str) -> Event {
        ev(
            seq,
            Body::NodeCreated {
                node: node.to_string(),
                num,
                kind: Kind::Task,
                title: title.to_string(),
                why: "it is needed".to_string(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        )
    }

    fn state_changed(seq: u64, node: &str, state: State, outcome: &str, forced: bool) -> Event {
        ev(
            seq,
            Body::StateChanged {
                node: node.to_string(),
                state,
                outcome: outcome.to_string(),
                forced,
            },
        )
    }

    fn session_started(seq: u64) -> Event {
        ev(
            seq,
            Body::SessionStarted {
                source: "test".to_string(),
                focus: None,
                vivac: None,
                session: None,
            },
        )
    }

    fn flag_cleared(seq: u64, node: &str, flag: Flag) -> Event {
        ev(
            seq,
            Body::FlagCleared {
                node: node.to_string(),
                flag,
            },
        )
    }

    fn flag_raised(seq: u64, node: &str, flag: Flag, reason: &str) -> Event {
        ev(
            seq,
            Body::FlagRaised {
                node: node.to_string(),
                flag,
                reason: reason.to_string(),
            },
        )
    }

    fn pushed(seq: u64, node: &str) -> Event {
        ev(
            seq,
            Body::Pushed {
                node: node.to_string(),
            },
        )
    }

    /// The limit is exclusive: the event at `since_seq` itself is the stop
    /// being measured from, not part of the stretch.
    #[test]
    fn the_limit_is_exclusive() {
        let events = vec![
            node_created(1, "n1", 1, "Old"),
            node_created(2, "n2", 2, "New"),
        ];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 1);
        assert_eq!(result.opened.len(), 1);
        assert_eq!(result.opened[0].node.id, "n2");
    }

    /// `SessionStarted` says something about the session and nothing about
    /// the tree: it must not land in any group, nor in the tail.
    #[test]
    fn a_session_start_counts_for_nothing() {
        let events = vec![node_created(1, "n1", 1, "Node"), session_started(2)];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 0);
        assert_eq!(result.opened.len(), 1);
        assert_eq!(result.tail.focus_moves, 0);
        assert_eq!(result.tail.notes, 0);
        assert_eq!(result.tail.flags_cleared, 0);
        assert_eq!(result.tail.edges, 0);
        assert_eq!(result.tail.stops, 0);
        assert_eq!(result.tail.unreadable, 0);
    }

    /// A node born and closed in the same stretch is what happened: it comes
    /// out in both groups, not deduplicated into one.
    #[test]
    fn a_node_born_and_closed_in_the_same_stretch_appears_in_both_groups() {
        let events = vec![
            node_created(1, "n1", 1, "Fixed fast"),
            state_changed(2, "n1", State::Done, "shipped", false),
        ];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 0);
        assert_eq!(result.opened.len(), 1);
        assert_eq!(result.closed.len(), 1);
        assert_eq!(result.closed[0].node.id, "n1");
        assert_eq!(result.closed[0].outcome, "shipped");
        assert!(!result.closed[0].forced);
    }

    /// `FlagCleared` is a tail count, not a group of its own: clearing a flag
    /// is not the same kind of event as raising one.
    #[test]
    fn a_cleared_flag_goes_to_the_tail_and_not_to_a_group() {
        let events = vec![
            node_created(1, "n1", 1, "Node"),
            flag_cleared(2, "n1", Flag::Stale),
        ];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 0);
        assert!(result.flagged.is_empty());
        assert_eq!(result.tail.flags_cleared, 1);
    }

    /// A stretch that only moved the focus has to say so: `tail.focus_moves`
    /// climbs and the four groups stay empty, so it does not read as if
    /// nothing happened.
    #[test]
    fn a_stretch_that_only_moved_the_focus_has_no_group_entries() {
        let events = vec![node_created(1, "n1", 1, "Node"), pushed(2, "n1")];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 1);
        assert_eq!(result.tail.focus_moves, 1);
        assert!(result.opened.is_empty());
        assert!(result.closed.is_empty());
        assert!(result.flagged.is_empty());
        assert!(result.moved.is_empty());
    }

    /// An event naming a node the tree does not have sums in `unreadable`
    /// rather than being dropped in silence.
    #[test]
    fn an_event_naming_an_unknown_node_counts_as_unreadable() {
        let events = vec![
            node_created(1, "n1", 1, "Node"),
            flag_raised(2, "ghost", Flag::Suspect, "reason"),
        ];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 0);
        assert!(result.flagged.is_empty());
        assert_eq!(result.tail.unreadable, 1);
    }

    /// Like `ev`, signed by a lane other than `main`: the fixture the
    /// marking tests below need, since every other helper in this module
    /// hardcodes the single lane `changes` used to be the whole of.
    fn ev_lane(seq: u64, lane: &str, payload: Body) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: lane.to_string(),
            payload,
        }
    }

    /// `t594` §4.3: a line born from another lane carries that lane, so a
    /// renderer can mark it -- and one born from this stretch's own lane
    /// carries that one, indistinguishable from every line `changes` has
    /// always shown.
    #[test]
    fn each_group_carries_the_lane_its_event_was_signed_with() {
        let events = vec![
            node_created(1, "n1", 1, "Mine"),
            ev_lane(
                2,
                "b",
                Body::NodeCreated {
                    node: "n2".to_string(),
                    num: 2,
                    kind: Kind::Task,
                    title: "B's own".to_string(),
                    why: "it is needed".to_string(),
                    parent: None,
                    blocks: false,
                    refs: vec![],
                    governs: vec![],
                    arms: vec![],
                    against: None,
                },
            ),
            ev_lane(
                3,
                "b",
                Body::StateChanged {
                    node: "n2".to_string(),
                    state: State::Done,
                    outcome: "shipped".to_string(),
                    forced: false,
                },
            ),
            ev_lane(
                5,
                "b",
                Body::FlagRaised {
                    node: "n1".to_string(),
                    flag: Flag::Suspect,
                    reason: "from over there".to_string(),
                },
            ),
        ];
        let tree = fold(&events, 0);
        let result = collect(&tree, &events, 0);
        assert_eq!(result.opened[0].lane, "main");
        assert_eq!(result.opened[1].lane, "b");
        assert_eq!(result.closed[0].lane, "b");
        assert_eq!(result.flagged[0].lane, "b");
    }

    /// A single-lane tree marks nothing: every entry's `lane` equals the
    /// lane the tree is looked at from, which is the byte-for-byte
    /// guarantee `foreign_mark` exists to keep.
    #[test]
    fn a_single_lane_carries_no_foreign_mark() {
        let events = vec![node_created(1, "n1", 1, "Node")];
        let tree = fold(&events, 0);
        assert_eq!(foreign_mark(&tree, "main"), "");
    }

    /// A line from another lane is marked with that lane's own name, and
    /// falls back to the bare lane when it was never declared.
    #[test]
    fn a_foreign_lane_is_marked_with_its_name() {
        let events = vec![node_created(1, "n1", 1, "Node")];
        let tree = fold(&events, 0);
        assert_eq!(foreign_mark(&tree, "b"), "[b] ");
    }
}
