//! `changes` — what a stretch of work moved: opened, closed, flagged, moved.
//!
//! `t148`: the log already has every event and no read hands them back
//! grouped by what happened, so answering "what did this stretch move" meant
//! opening `events` by hand. The boundary is a vivac and never a raw
//! timestamp, because a timestamp ties within the same second and a vivac's
//! `seq` does not.

use crate::args::Args;
use crate::event::{Body, Event, Flag, State};
use crate::failure::Failure;
use crate::model::{Node, Tree, Vivac};
use crate::render::{print_json, wrap, WIDTH};
use serde_json::json;

pub struct Changed<'a> {
    /// The stop the stretch is measured from. `None` means the log has no
    /// stops at all, so the stretch is everything.
    pub since: Option<&'a Vivac>,
    pub opened: Vec<&'a Node>,
    pub closed: Vec<Closed<'a>>,
    pub flagged: Vec<Flagged<'a>>,
    pub moved: Vec<Moved<'a>>,
    pub tail: Tail,
}

pub struct Closed<'a> {
    pub node: &'a Node,
    pub outcome: String,
    pub forced: bool,
}

pub struct Flagged<'a> {
    pub node: &'a Node,
    pub flag: Flag,
    pub reason: String,
}

pub struct Moved<'a> {
    pub node: &'a Node,
    /// The word this state goes by for this kind: `State::word` already
    /// knows it, and a second spelling here would drift from the first.
    pub state: State,
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
        since: None,
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
                Some(n) => result.opened.push(n),
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
                }),
                Some(n) => result.moved.push(Moved {
                    node: n,
                    state: *state,
                }),
                None => result.tail.unreadable += 1,
            },
            Body::FlagRaised { node, flag, reason } => match tree.node(node) {
                Some(n) => result.flagged.push(Flagged {
                    node: n,
                    flag: *flag,
                    reason: reason.clone(),
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
            // Not naming a node: nothing to check against the tree.
            Body::VivacCreated { .. } => result.tail.stops += 1,
        }
    }

    result
}

/// `changes` — what a stretch of work moved, printed with `triage`'s style.
///
/// Always exits `0` when the command itself was well formed: this is a
/// reading, not a check, and an empty stretch is answered with a sentence
/// rather than a non-zero code.
pub fn changes(tree: &Tree, log: &[Event], args: &Args) -> Result<i32, Failure> {
    let since = match args.opt("since") {
        Some(s) => Some(
            tree.vivac(s)
                .ok_or_else(|| Failure::usage(format!("No such vivac: {s}.")))?,
        ),
        None => tree.last_vivac(),
    };
    let since_seq = since.map(|v| v.seq).unwrap_or(0);
    let mut result = collect(tree, log, since_seq);
    result.since = since;

    if args.has("json") {
        return print_json(as_json(&result)).map(|_| 0);
    }
    print_text(&result);
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

/// `CHANGES SINCE ...`, in one of three forms and never a fourth: the last
/// stop, an older one with how many stops lie between it and now, or no stop
/// at all. It carries no clock time on purpose: `now_rfc3339` writes in UTC,
/// and a bare hour would read as local to whoever is looking at it.
fn header(since: Option<&Vivac>, stops_since: usize) -> String {
    match since {
        None => "  CHANGES SINCE THE BEGINNING - no stops yet".to_string(),
        Some(v) => {
            let date = crate::clock::date_of(&v.ts);
            if stops_since == 0 {
                format!("  CHANGES SINCE {}, the last stop - {date}", v.alias())
            } else {
                format!(
                    "  CHANGES SINCE {} - {date}, {stops_since} stop{} ago",
                    v.alias(),
                    plural(stops_since)
                )
            }
        }
    }
}

/// The tail line, naming only what is not zero, in a fixed order. `stops`
/// never appears here: it already spoke in the header.
fn tail_phrase(tail: &Tail) -> Option<String> {
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
    if tail.unreadable > 0 {
        parts.push(format!(
            "{} unreadable event{}",
            tail.unreadable,
            plural(tail.unreadable)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn print_text(result: &Changed) {
    println!();
    println!("{}", header(result.since, result.tail.stops));

    let mut said_something = false;

    if !result.opened.is_empty() {
        said_something = true;
        println!();
        println!("  OPENED ({})", result.opened.len());
        for n in &result.opened {
            println!("    {:<6} {}", n.alias(), n.title);
        }
    }

    if !result.closed.is_empty() {
        said_something = true;
        println!();
        println!("  CLOSED ({})", result.closed.len());
        for c in &result.closed {
            println!("    {:<6} {}", c.node.alias(), c.node.title);
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
                println!("{l}");
            }
        }
    }

    if !result.flagged.is_empty() {
        said_something = true;
        println!();
        println!("  FLAGGED ({})", result.flagged.len());
        for f in &result.flagged {
            println!("    {:<6} {}", f.node.alias(), f.node.title);
            for l in wrap(
                &format!("{}: {}", f.flag.word(), f.reason),
                WIDTH,
                "           ",
            ) {
                println!("{l}");
            }
        }
    }

    if !result.moved.is_empty() {
        said_something = true;
        println!();
        println!("  MOVED ({})", result.moved.len());
        for m in &result.moved {
            println!("    {:<6} {}", m.node.alias(), m.node.title);
            let word = m.state.word(m.node.kind);
            for l in wrap(&format!("{word}: {}", m.node.outcome), WIDTH, "           ") {
                println!("{l}");
            }
        }
    }

    if let Some(t) = tail_phrase(&result.tail) {
        said_something = true;
        println!();
        println!("  + {t}");
    }

    if !said_something {
        println!();
        match result.since {
            Some(v) => println!("  Nothing has moved since {}.", v.alias()),
            None => println!("  Nothing has moved."),
        }
    }
    println!();
}

fn as_json(result: &Changed) -> serde_json::Value {
    json!({
        "since": result.since.map(|v| json!({
            "alias": v.alias(),
            "ts": v.ts,
            "stops_since": result.tail.stops,
        })),
        "opened": result.opened.iter().map(|n| json!({
            "alias": n.alias(),
            "title": n.title,
            "kind": n.kind,
        })).collect::<Vec<_>>(),
        "closed": result.closed.iter().map(|c| json!({
            "alias": c.node.alias(),
            "title": c.node.title,
            "outcome": c.outcome,
            "forced": c.forced,
        })).collect::<Vec<_>>(),
        "flagged": result.flagged.iter().map(|f| json!({
            "alias": f.node.alias(),
            "title": f.node.title,
            "flag": f.flag.word(),
            "reason": f.reason,
        })).collect::<Vec<_>>(),
        "moved": result.moved.iter().map(|m| json!({
            "alias": m.node.alias(),
            "title": m.node.title,
            "state": m.state.word(m.node.kind),
        })).collect::<Vec<_>>(),
        "tail": {
            "focus_moves": result.tail.focus_moves,
            "notes": result.tail.notes,
            "flags_cleared": result.tail.flags_cleared,
            "edge_changes": result.tail.edges,
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
        assert_eq!(result.opened[0].id, "n2");
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
}
