//! What the maintainer reads.
//!
//! All ASCII and not one colour escape. The DX pillar is explicit: **meaning
//! is never encoded in colour alone**, and this has to degrade without
//! breaking --with no tty, over ssh, and in cmd.exe as well as Windows
//! Terminal--. `[x]`, `[~]`, `*` and `<== FALSE CLOSE` read in black and
//! white. Colour, when it lands, reinforces; it does not inform.
//!
//! Every render has its `--json` twin, which is the other half of the
//! audience: the agent needs parseable output, not a drawn tree.

use crate::anchor::AnchorRef;
use crate::args::Args;
use crate::brief::clip;
use crate::event::{Body, Event, Kind, State};
use crate::failure::{Failure, R};
use crate::model::{Aggregates, Node, Tree};
use crate::output::outln;
use serde_json::json;
use std::collections::HashMap;

pub(crate) const WIDTH: usize = 62;

/// `d330`: how much of an ancestor's why/note/outcome survives in `why`
/// without `--full`. Matches `WIDTH` on purpose -- a clipped ancestor
/// collapses to roughly one wrapped line -- and it never applies to the
/// node actually asked about, which stays whole with or without `--full`.
const ANCESTOR_CLIP: usize = WIDTH;

pub(crate) fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for p in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + p.chars().count() > width {
            lines.push(format!("{indent}{cur}"));
            cur = p.to_string();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(p);
        }
    }
    if !cur.is_empty() {
        lines.push(format!("{indent}{cur}"));
    }
    lines
}

fn label(a: &Tree, n: &Node) -> String {
    match n.state {
        State::Active => n.title(a).to_string(),
        e => format!("{}  [{}]", n.title(a), e.word(n.kind)),
    }
}

fn json_node(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let r = ag.counts(n.num);
    json!({
        "id": n.id,
        "alias": n.alias(),
        "num": n.num,
        "kind": n.kind,
        "title": n.title(a),
        "why": n.why(a),
        "state": n.state,
        "blocks": n.blocks,
        "parent": n.parent.and_then(|p| a.node_by_num(p).map(|x| x.alias())),
        "note": n.note(a),
        "outcome": n.outcome(a),
        "refs": n.refs(a),
        "governs": n.governs(a),
        "opened": n.opened(a),
        "closed": n.closed(a),
        "false_close": n.state == State::Done && ag.blockers(n.num) > 0,
        "open_below": r.open_count,
        "total_below": r.total,
    })
}

pub(crate) fn print_json(v: serde_json::Value) -> R {
    outln!(
        "{}",
        serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?
    );
    Ok(())
}

/// `why` — why we are here. It is the operation that defines the product.
///
/// It narrates the path from the root and then answers the three questions
/// that come next: what was left open in parallel, what was born here, and
/// what keeps each step of the path from closing.
///
/// `--full` adds three more, per step of the path and on `node` itself: the
/// anchor in force when that step was born, the decisions born there that
/// still stand, and the siblings that were still open at that moment. The
/// first two answer from the folded `Tree`; the third cannot, because
/// closing is folded away to the final state, and this is a question about
/// a moment in the past. `Full` answers it from the log directly.
///
/// Only the log gives a moment a `seq`: `ts` alone ties within the same day,
/// and this project has had days with 23 stops on it, so a comparison by
/// date would be wrong on exactly the days it matters.
pub(crate) struct Full {
    /// Node id -> the `seq` it was created at. The first `node.created` for
    /// an id wins, matching `Tree::apply`'s own rule for a repeated one.
    created: HashMap<String, u64>,
    /// Node id -> every `state.changed` it ever had, in log order. A node can
    /// be reopened, so this is not "the one time it closed": it is the whole
    /// history, searched for whatever it was at a given `seq`.
    state: HashMap<String, Vec<(u64, State)>>,
}

impl Full {
    pub(crate) fn from_log(log: &[Event]) -> Full {
        let mut created = HashMap::new();
        let mut state: HashMap<String, Vec<(u64, State)>> = HashMap::new();
        for e in log {
            match &e.payload {
                Body::NodeCreated { node, .. } => {
                    created.entry(node.clone()).or_insert(e.seq);
                }
                Body::StateChanged { node, state: s, .. } => {
                    state.entry(node.clone()).or_default().push((e.seq, *s));
                }
                _ => {}
            }
        }
        Full { created, state }
    }

    /// What a node's state was at `seq`, inclusive. With no `state.changed`
    /// at or before it, the node was still in the one it is born with.
    fn state_at(&self, id: &str, seq: u64) -> State {
        self.state
            .get(id)
            .into_iter()
            .flatten()
            .rfind(|(s, _)| *s <= seq)
            .map(|(_, state)| *state)
            .unwrap_or(State::Active)
    }
}

/// The anchor in force when `n` was born: the most recent stop at or before
/// the `seq` of its `node.created`, and its anchor. Empty with nothing
/// earlier to point to -- there is no version control, or the node predates
/// every stop -- and that is a value, not a failure.
pub(crate) fn anchor_of(a: &Tree, full: &Full, n: &Node) -> AnchorRef {
    let Some(&seq) = full.created.get(&n.id) else {
        return AnchorRef::default();
    };
    a.vivacs
        .iter()
        .rfind(|v| v.seq <= seq)
        .map(|v| v.anchor.clone())
        .unwrap_or_default()
}

/// The decisions born from `n` that still stand: a filter over what
/// `born_here` already lists, kept to the ones that are a decision and still
/// open. Superseding one closes it, so a superseded decision drops out on
/// its own.
pub(crate) fn standing_of<'a>(a: &'a Tree, n: &Node) -> Vec<&'a Node> {
    a.children(n.num)
        .into_iter()
        .filter(|c| c.kind == Kind::Decision && c.state.is_open())
        .collect()
}

/// The siblings of `n`, born before it by `Node::num`, that were still open
/// at the `seq` `n` was born. Not by `closed`'s date: two siblings can open
/// and close on the day `n` was born, in an order the date cannot tell
/// apart.
pub(crate) fn open_then_of<'a>(a: &'a Tree, full: &Full, n: &Node) -> Vec<&'a Node> {
    let (Some(&seq), Some(parent)) = (full.created.get(&n.id), n.parent) else {
        return vec![];
    };
    a.children(parent)
        .into_iter()
        .filter(|c| c.id != n.id && c.num < n.num)
        .filter(|c| full.state_at(&c.id, seq).is_open())
        .collect()
}

/// `json_node`, with the three `--full` fields added.
fn json_node_full(a: &Tree, ag: &Aggregates, full: &Full, n: &Node) -> serde_json::Value {
    let mut v = json_node(a, ag, n);
    v["anchor"] = json!(anchor_of(a, full, n));
    v["standing"] = json!(standing_of(a, n)
        .iter()
        .map(|c| json_node(a, ag, c))
        .collect::<Vec<_>>());
    v["open_then"] = json!(open_then_of(a, full, n)
        .iter()
        .map(|c| json_node(a, ag, c))
        .collect::<Vec<_>>());
    v
}

/// `why` as data.
///
/// The builder and the printing are two functions, the way `brief.rs` has
/// always had them: `to_text` builds and `brief` prints one line lower. It
/// matters more than tidiness here, because a second reader --the MCP server--
/// speaks JSON-RPC over the same standard output. A `println!` in its path
/// does not look untidy, it corrupts the channel.
///
/// `full` is `None` for every caller but `why --full`, `why_data`'s own
/// signature included: the MCP tool calls that one and has never asked for
/// the log, so its shape stays exactly what it has always been.
fn why_data_impl(a: &Tree, full: Option<&Full>, id: &str) -> Result<serde_json::Value, Failure> {
    let ag = &a.aggregates();
    let n = a
        .resolve(id)
        .ok_or_else(|| Failure::usage(format!("No such node: {id}.")))?;
    let lineage = a.ancestors(n.num);
    let node_json = |x: &Node| match full {
        Some(f) => json_node_full(a, ag, f, x),
        None => json_node(a, ag, x),
    };
    let siblings: Vec<_> = n
        .parent
        .map(|p| a.children(p))
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.id != n.id && c.state.is_open())
        .map(|c| json_node(a, ag, c))
        .collect();
    Ok(json!({
        "node": node_json(n),
        "path": lineage.iter().map(|x| node_json(x)).collect::<Vec<_>>(),
        "in_parallel": siblings,
        "born_here": a.children(n.num).iter().filter(|c| c.state.is_open())
            .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
        "blockers": a.open_blockers(n.num).iter()
            .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
    }))
}

pub fn why_data(a: &Tree, id: &str) -> Result<serde_json::Value, Failure> {
    why_data_impl(a, None, id)
}

/// A front, identified by its alias and where it hangs, not the node itself:
/// `why` on the alias brings the rest.
///
/// This used to be `json_node`'s eighteen keys plus `lineage`, the same
/// shape `why` needs because `why` is asked for exactly that prose. The
/// prose `open` prints was already the right shape -- alias, title, path --
/// and the data was not; `d172` made the same fix for `find` first, down to
/// dropping `matched`, which has no analogue here because there is no query.
/// Measured over the same 10,000-node tree, both numbers from the same
/// harness: the MCP payload was 1,993,053 bytes and is now 599,012, 30% of
/// what it cost before.
pub fn open_data(a: &Tree) -> serde_json::Value {
    let ag = a.aggregates();
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(n.num).iter().any(|c| c.is_front()))
        .collect();
    // `sort_by_cached_key` and not `sort_by_key`: the second calls the key
    // function O(n log n) times, and this key goes to the aggregate map every
    // time it is called. It was measured, and the difference did not come up
    // out of the noise on this machine -- so it is here for being the right
    // primitive against a key that costs a lookup, not for a number.
    leaves.sort_by_cached_key(|n| {
        (
            !n.blocks,
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    json!(leaves
        .iter()
        .map(|n| json!({
            "alias": n.alias(),
            "kind": n.kind,
            "state": n.state,
            "title": n.title(a),
            "lineage": lineage_of(a, n),
        }))
        .collect::<Vec<_>>())
}

/// The `--full` lines for one step of the path, printed the way the JSON
/// twin carries the same three fields: the anchor, the decisions still
/// standing, and the siblings still open at that moment.
fn print_full_of(a: &Tree, full: &Full, n: &Node) {
    let anchor = anchor_of(a, full, n);
    if anchor.is_empty_tree() {
        outln!("        anchor: none");
    } else {
        outln!("        anchor: {} ({})", anchor.short(), anchor.kind);
    }
    let standing = standing_of(a, n);
    if !standing.is_empty() {
        outln!(
            "        standing ({}): {}",
            standing.len(),
            standing
                .iter()
                .map(|d| d.alias())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let open_then = open_then_of(a, full, n);
    if !open_then.is_empty() {
        outln!(
            "        open then ({}): {}",
            open_then.len(),
            open_then
                .iter()
                .map(|d| d.alias())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

pub fn why(a: &Tree, log: &[Event], args: &Args) -> R {
    let ag = &a.aggregates();
    let s = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac why <id>"))?;
    let n = a
        .resolve(s)
        .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))?;
    let lineage = a.ancestors(n.num);
    let full = args.has("full").then(|| Full::from_log(log));

    if args.has("json") {
        return print_json(match &full {
            Some(f) => why_data_impl(a, Some(f), s)?,
            None => why_data(a, s)?,
        });
    }

    outln!();
    outln!("  Why we are here  ->  {}", n.alias());
    outln!("  {}", "-".repeat(66));
    outln!();
    for (i, p) in lineage.iter().enumerate() {
        let is_last = i == lineage.len() - 1;
        // The node actually asked about prints whole either way; an
        // ancestor's body only survives whole under `--full`.
        let clip_body = !is_last && full.is_none();
        let body = |text: &str| {
            if clip_body {
                clip(text, ANCESTOR_CLIP)
            } else {
                text.to_string()
            }
        };
        outln!("  {:<6}{}", p.alias(), label(a, p));
        for l in wrap(&body(p.why(a)), WIDTH, "        ") {
            outln!("{l}");
        }
        let note = p.note(a);
        for l in wrap(&format!("! {}", body(note)), WIDTH, "        ") {
            if !note.is_empty() {
                outln!("{l}");
            }
        }
        let outcome = p.outcome(a);
        for l in wrap(&format!("= {}", body(outcome)), WIDTH, "        ") {
            if !outcome.is_empty() {
                outln!("{l}");
            }
        }
        if let Some(f) = &full {
            print_full_of(a, f, p);
        }
        if !is_last {
            let f = ag.counts(p.num).phrase();
            if !f.is_empty() {
                outln!("        ({f} below)");
            }
            outln!("        |");
            outln!("        v");
        } else {
            outln!();
            outln!("        ^^^ you are here");
        }
    }
    outln!();

    // "we had ten things to review, we are on the first"
    if let Some(parent) = n.parent {
        let siblings: Vec<_> = a
            .children(parent)
            .into_iter()
            .filter(|c| c.id != n.id && c.state.is_open())
            .collect();
        if !siblings.is_empty() {
            outln!("  In parallel, still open ({}):", siblings.len());
            for c in siblings {
                outln!("      {:<6} {}", c.alias(), c.title(a));
            }
            outln!();
        }
    }

    let kids: Vec<_> = a
        .children(n.num)
        .into_iter()
        .filter(|c| c.state.is_open())
        .collect();
    if !kids.is_empty() {
        outln!("  Born here and still open ({}):", kids.len());
        for c in kids {
            outln!(
                "    {} {:<6} {}",
                if c.blocks { '*' } else { ' ' },
                c.alias(),
                c.title(a)
            );
        }
        outln!();
    }

    for p in &lineage {
        let pending_count = a.open_blockers(p.num);
        if !pending_count.is_empty() && p.state.is_open() {
            outln!(
                "  {} does not close until these close ({}):",
                p.alias(),
                pending_count.len()
            );
            for c in pending_count {
                outln!("      {:<6} {}", c.alias(), c.title(a));
            }
            outln!();
        }
    }
    Ok(())
}

fn branch(a: &Tree, ag: &Aggregates, n: &Node, prefix: &str, is_last: bool, show_all: bool) {
    let f = ag.counts(n.num).phrase();
    let mut tail = if f.is_empty() {
        String::new()
    } else {
        format!("   ({f})")
    };
    let pending_count = ag.blockers(n.num);
    if n.state == State::Done && pending_count > 0 {
        tail.push_str(&format!(
            "   <== FALSE CLOSE: {pending_count} open condition(s)"
        ));
    }
    let mark = if n.blocks { "* " } else { "" };
    outln!(
        "{prefix}{}[{}] {:<6} {mark}{}{tail}",
        if is_last { "`-- " } else { "|-- " },
        n.state.mark(),
        n.alias(),
        n.title(a)
    );
    let sig = format!("{prefix}{}", if is_last { "    " } else { "|   " });
    let children: Vec<_> = a
        .children(n.num)
        .into_iter()
        .filter(|h| show_all || h.state.is_open() || ag.counts(h.num).open_count > 0)
        .collect();
    for (i, h) in children.iter().enumerate() {
        branch(a, ag, h, &sig, i == children.len() - 1, show_all);
    }
}

fn subtree_json(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let mut v = json_node(a, ag, n);
    v["children"] = json!(a
        .children(n.num)
        .iter()
        .map(|h| subtree_json(a, ag, h))
        .collect::<Vec<_>>());
    v
}

pub fn tree(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let roots: Vec<&Node> = match args.positional(0) {
        Some(s) => vec![a
            .resolve(s)
            .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))?],
        None => a.roots(),
    };
    if args.has("json") {
        return print_json(json!(roots
            .iter()
            .map(|n| subtree_json(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if a.is_empty_tree() {
        outln!("  Empty tree.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    let show_all = args.has("all");
    outln!();
    for (i, n) in roots.iter().enumerate() {
        branch(a, ag, n, "  ", i == roots.len() - 1, show_all);
    }
    outln!();
    if !show_all {
        outln!("  (closed nodes with no open descendants hidden; --all shows them)");
        outln!();
    }
    Ok(())
}

/// Fronts printed before the list gives way to the tail line. Each front
/// costs two lines, so ten of them plus the header and the tail still fit
/// one screen with nothing to scroll -- and a list that has to scroll
/// already broke the promise of "right now".
const MAX_FRONTS_SHOWN: usize = 10;

/// `open` — what is waiting for you right now, and what has been open so
/// long you are not actually working it any more (`d383`). The order below
/// is deduced from that sentence, not chosen and explained after: a
/// blocker sorts first, because a blocker is exactly something waiting on
/// you; among the rest, whichever holds up more tree; at a tie, the newest.
pub fn open(a: &Tree, args: &Args) -> R {
    let ag = a.aggregates();
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(n.num).iter().any(|c| c.is_front()))
        .collect();
    // `sort_by_cached_key` and not `sort_by_key`: the second calls the key
    // function O(n log n) times, and this key goes to the aggregate map every
    // time it is called. It was measured, and the difference did not come up
    // out of the noise on this machine -- so it is here for being the right
    // primitive against a key that costs a lookup, not for a number.
    leaves.sort_by_cached_key(|n| {
        (
            !n.blocks,
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    let standing = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Decision && n.state.is_open())
        .count();
    if args.has("json") {
        return print_json(open_data(a));
    }
    if leaves.is_empty() && standing == 0 {
        outln!("  Nothing open.");
        return Ok(());
    }
    outln!();
    outln!(
        "  {} open front{}",
        leaves.len(),
        if leaves.len() == 1 { "" } else { "s" },
    );
    outln!();
    let show_all = args.has("all");
    let shown = if show_all {
        leaves.len()
    } else {
        leaves.len().min(MAX_FRONTS_SHOWN)
    };
    for n in &leaves[..shown] {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        let lineage = a.ancestors(n.num);
        if lineage.len() > 1 {
            let v: Vec<String> = lineage[..lineage.len() - 1]
                .iter()
                .map(|p| p.alias())
                .collect();
            outln!("         via {}", v.join(" > "));
        }
    }
    let hidden = leaves.len() - shown;
    if hidden > 0 {
        // The oldest of the ones left out, never of the whole set: a front
        // that made the cut is being worked, and its age is not the gap
        // `--all` closes.
        let oldest = leaves[shown..].iter().map(|n| n.opened(a)).min();
        let age = oldest.and_then(|d| crate::clock::days_between(d, &crate::clock::now_rfc3339()));
        // The same three arms the project index uses for a project that has
        // not moved. A count of days is the wrong shape at zero and at one,
        // and "open for 0 days" is a sentence nobody says.
        match age {
            Some(d) if d <= 0 => {
                outln!("  {hidden} more, the oldest opened today -- vivac open --all")
            }
            Some(1) => {
                outln!("  {hidden} more, the oldest open since yesterday -- vivac open --all")
            }
            Some(days) => {
                outln!("  {hidden} more, the oldest open for {days} days -- vivac open --all")
            }
            None => outln!("  {hidden} more -- vivac open --all"),
        }
    }
    // They are not fronts, but making them vanish without saying so would be
    // omitting in silence: they get counted and located.
    if standing > 0 {
        let phrase = if standing == 1 {
            "1 standing decision, which is not work".to_string()
        } else {
            format!("{standing} standing decisions, which are not work")
        };
        outln!();
        outln!("  + {phrase}   vivac brief");
    }
    outln!();
    Ok(())
}

/// `triage` — what can be pruned, and with which command.
///
/// A brief over budget **must not lie by omission** (`BRIEF-SPEC.md` §4):
/// the signal is that the graph needs pruning, and this is the view that says
/// where. `MODEL.md` §6.1 also sends it the deep nodes, because a chain that
/// long is almost never lack of discipline: it is that the goal moved and
/// nobody re-rooted.
pub fn triage(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();

    let mut parked_nodes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();

    // `MODEL.md` §6.1: from 6 on it shows up here, and it never blocks. The
    // distance is to the goal the node answers to, not to the root: `promote`
    // is the way out this section prints, and a count from the root is one
    // `promote` cannot move (`f156`).
    let mut deep: Vec<(&Node, usize)> = a
        .nodes_iter()
        .filter(|n| n.is_front())
        .map(|n| (n, a.under_goal(n.num).len()))
        .filter(|(_, d)| *d >= 6)
        .collect();

    // Alive, hanging off something discarded. `abandon`'s rescue produces
    // them, and it does **not** reparent on purpose (`d33`): the node stays
    // where it was born. That is why they need revisiting now and then, and
    // why they are here and not in `check`: it is not store corruption, it is
    // work that lost the reason it was born for.
    let mut orphaned: Vec<(&Node, &Node)> = a
        .nodes_iter()
        .filter(|n| n.is_front())
        .filter_map(|n| {
            let p = a.node_by_num(n.parent?)?;
            (p.state == State::Abandoned).then_some((n, p))
        })
        .collect();

    // Invariant 10. `check` reports them for CI; here they get acted on, and
    // with the same exemption: a **forced** close was a decision, it has its
    // trace and the tree marks it. Repeating it here every day would be asking
    // for what was already decided to be decided again. What does land here is
    // the close that turned false later, when a blocker got hung on something
    // already closed: that is the case that took 26 days to spot.
    let mut false_closes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Done && !n.forced_close && ag.blockers(n.num) > 0)
        .collect();

    parked_nodes.sort_by_key(|n| n.num);
    deep.sort_by_key(|(n, _)| n.num);
    orphaned.sort_by_key(|(n, _)| n.num);
    false_closes.sort_by_key(|n| n.num);

    if args.has("json") {
        return print_json(json!({
            "parked": parked_nodes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
            "deep": deep.iter().map(|(n, d)| {
                let mut v = json_node(a, ag, n);
                // Named for what it counts. `stats` reports a `depth` measured
                // from the root, and one key meaning two distances would be
                // read wrong exactly once.
                v["depth_from_goal"] = json!(d);
                v
            }).collect::<Vec<_>>(),
            "orphaned_by_discard": orphaned.iter().map(|(n, p)| {
                let mut v = json_node(a, ag, n);
                v["discarded"] = json!(p.alias());
                v["discarded_because"] = json!(p.outcome(a));
                v
            }).collect::<Vec<_>>(),
            "false_closes": false_closes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }

    let total = parked_nodes.len() + deep.len() + orphaned.len() + false_closes.len();
    if total == 0 {
        outln!("  Nothing to prune.");
        return Ok(());
    }
    outln!();
    outln!("  TRIAGE - {total} thing(s) to look at");

    if !parked_nodes.is_empty() {
        outln!();
        outln!(
            "  PARKED ({})                       focus <id>  |  abandon <id>",
            parked_nodes.len()
        );
        for n in &parked_nodes {
            outln!("    {:<6} {}", n.alias(), n.title(a));
            for l in wrap(n.outcome(a), WIDTH, "           ") {
                outln!("{l}");
            }
        }
    }

    if !deep.is_empty() {
        outln!();
        outln!(
            "  6 OR MORE FROM ITS GOAL ({})      promote <id>",
            deep.len()
        );
        for (n, d) in &deep {
            outln!(
                "    {:<6} {:<40} depth {d}",
                n.alias(),
                clip(n.title(a), 40)
            );
            // The lineage starts where the number does. Drawing it from the
            // root beside a distance to the goal would say two things at once.
            let v: Vec<String> = a
                .under_goal(n.num)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .map(|p| p.alias())
                .collect();
            outln!("           via {}", v.join(" > "));
        }
    }

    if !orphaned.is_empty() {
        outln!();
        outln!(
            "  SURVIVED A DISCARD ({})           abandon <id>  |  promote <id>",
            orphaned.len()
        );
        for (n, p) in &orphaned {
            outln!("    {:<6} {}", n.alias(), n.title(a));
            outln!(
                "           born from {}, discarded: {}",
                p.alias(),
                clip(p.outcome(a), 36)
            );
        }
    }

    if !false_closes.is_empty() {
        outln!();
        outln!(
            "  FALSE CLOSES ({})                 close what is left, or --force",
            false_closes.len()
        );
        for n in &false_closes {
            outln!(
                "    {:<6} {:<40} {} blocker(s)",
                n.alias(),
                clip(n.title(a), 40),
                ag.blockers(n.num)
            );
        }
    }
    outln!();
    Ok(())
}

/// `parked` — DO NOT TOUCH NOW. It is the section no other tool emits: every
/// memory tool dumps what is relevant, and the problem in agentic development
/// is the opposite one, bounding.
pub fn parked(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let mut ps: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();
    ps.sort_by_key(|n| n.num);
    if args.has("json") {
        return print_json(json!(ps
            .iter()
            .map(|n| json_node(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if ps.is_empty() {
        outln!("  Nothing parked.");
        return Ok(());
    }
    outln!();
    outln!("  DO NOT TOUCH NOW ({})", ps.len());
    outln!();
    for n in ps {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        for l in wrap(n.outcome(a), WIDTH, "         ") {
            outln!("{l}");
        }
    }
    outln!();
    Ok(())
}

/// `stack` — where you are right now, from the root to the focus.
pub fn stack(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let stack: Vec<&Node> = a
        .stack
        .iter()
        .filter_map(|&num| a.node_by_num(num))
        .collect();
    if args.has("json") {
        return print_json(json!({
            "depth": stack.len(),
            "stack": stack.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    if stack.is_empty() {
        outln!("  Empty stack.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    outln!();
    for (i, n) in stack.iter().enumerate() {
        let focus = if i == stack.len() - 1 {
            "   <- focus"
        } else {
            ""
        };
        outln!("  {}{:<6} {}{focus}", "  ".repeat(i), n.alias(), n.title(a));
    }
    outln!();
    if stack.len() >= 6 {
        outln!(
            "  Stack {} levels deep. Almost never lack of discipline: usually",
            stack.len()
        );
        outln!("  the root goal moved and nobody re-rooted.  vivac promote");
        outln!();
    }
    Ok(())
}

pub fn stats(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let mut by_state = std::collections::BTreeMap::new();
    let mut orphans = 0usize;
    let mut false_closes = Vec::new();
    for n in a.nodes_iter() {
        *by_state.entry(n.state.word(n.kind)).or_insert(0usize) += 1;
        if n.parent.is_some_and(|p| a.node_by_num(p).is_none()) {
            orphans += 1;
        }
        if n.state == State::Done && ag.blockers(n.num) > 0 {
            false_closes.push(n);
        }
    }
    let depth_of = ag.max_depth;
    false_closes.sort_by_key(|n| n.num);
    if args.has("json") {
        return print_json(json!({
            "nodes": a.total(),
            "by_state": by_state,
            "depth": depth_of,
            "roots": a.roots().len(),
            "stack": a.stack_depth(),
            "orphans": orphans,
            "broken_lines": a.broken_lines,
            "false_closes": false_closes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    outln!();
    outln!("  nodes          {}", a.total());
    for (k, v) in &by_state {
        outln!("  {k:<14} {v}");
    }
    outln!("  depth          {depth_of}");
    outln!("  roots          {}", a.roots().len());
    outln!("  stack          {}", a.stack_depth());
    if orphans > 0 {
        outln!("  ORPHANS        {orphans}  <- broken provenance");
    }
    if a.broken_lines > 0 {
        outln!("  broken lines   {}  <- in .vivac/events", a.broken_lines);
    }
    if !false_closes.is_empty() {
        outln!();
        outln!("  FALSE CLOSES ({})", false_closes.len());
        for n in false_closes {
            outln!("      {:<6} {}", n.alias(), n.title(a));
        }
    }
    outln!();
    Ok(())
}

/// `vivacs` — the safe stops, latest first.
pub fn vivacs(a: &Tree, args: &Args) -> R {
    if args.has("json") {
        return print_json(json!(a
            .vivacs
            .iter()
            .rev()
            .map(|v| json!({
                "id": v.id,
                "alias": v.alias(),
                "node_ref": v.node_ref.as_ref().and_then(|r| a.node(r).map(|n| n.alias())),
                "kind": v.kind.word(),
                "ts": v.ts,
                "label": v.label,
                "next_intent": v.next_intent,
                "anchor": v.anchor,
                "stack": v.stack.iter().map(|(al, t)| json!({"alias": al, "title": t}))
                    .collect::<Vec<_>>(),
                "working_set": v.working_set,
            }))
            .collect::<Vec<_>>()));
    }
    if a.vivacs.is_empty() {
        outln!("  No stops yet.  vivac save \"<label>\"");
        return Ok(());
    }
    outln!();
    for v in a.vivacs.iter().rev().take(20) {
        let top = v
            .stack
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "empty stack".into());
        outln!(
            "  {:<5} {:<7} {}  {}",
            v.alias(),
            v.kind.word(),
            crate::clock::date_of(&v.ts),
            top
        );
        if !v.label.is_empty() {
            outln!("           {}", v.label);
        }
        if !v.next_intent.is_empty() {
            outln!("           you were about to: {}", v.next_intent);
        }
    }
    if a.vivacs.len() > 20 {
        outln!();
        outln!("  ... and {} more", a.vivacs.len() - 20);
    }
    outln!();
    Ok(())
}

/// The fields of a node that carry meaning, in the order a reader wants them.
///
/// The title is a label; the reason, the note and the outcome are where the
/// thinking is. A search that read only titles would find the folder and miss
/// what is inside it.
fn searchable<'t>(a: &'t Tree, n: &Node) -> [(&'static str, &'t str); 4] {
    [
        ("title", n.title(a)),
        ("why", n.why(a)),
        ("note", n.note(a)),
        ("outcome", n.outcome(a)),
    ]
}

/// A window of `width` characters around the first term that hit.
///
/// The offsets come out of the lowercased copy, and lowercasing can change
/// how many bytes --and even how many characters-- a string takes, so the
/// map back to the original is built while lowercasing rather than assumed.
/// A snippet that lands two characters off is not a defect worth a wrong
/// answer.
fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let mut lower = String::with_capacity(text.len());
    let mut origin: Vec<usize> = Vec::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        for lowered_char in c.to_lowercase() {
            for _ in 0..lowered_char.len_utf8() {
                origin.push(i);
            }
            lower.push(lowered_char);
        }
    }
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .map(|b| origin[b])
        .unwrap_or(0);
    let end = (at + width * 2 / 3).clamp(width, chars.len());
    let start = end - width;
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.extend(chars[start..end].iter());
    if end < chars.len() {
        out.push_str("...");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text search over the tree.
///
/// `PILLARS.md` gives text search a ceiling of 100 ms and nothing ever
/// implemented it: a budget with no floor under it, the same class of
/// unchecked claim as the test count that lied for a day.
///
/// Two rules it does not bend. **Every term has to appear**, or a second word
/// would widen the search instead of narrowing it, which is the opposite of
/// what typing more means. And **closed nodes are searched too**: what you
/// look for months later is usually finished, and a search that stopped at
/// the open fronts would be a to-do list rather than a memory.
///
/// Order is not recency. Newest-first was the first answer, and it is the
/// wrong one for the search a memory is actually asked to do: the hits that
/// founded a subject are the oldest of them, and they were arriving last.
/// `d362` orders by what a hit is about first, by how much tree it holds up
/// second, and by recency only as the last tiebreak.
fn terms_of(query: &str) -> Result<Vec<String>, Failure> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() {
        return Err(Failure::usage("usage: vivac find \"<text>\"".to_string()));
    }
    Ok(terms)
}

/// Where a field lands in the order [`hits_for`] sorts by: title first, why
/// second, note and outcome tied for last. A function rather than the
/// position `searchable` returns the field at, because note and outcome tie
/// and a position has no room for one.
fn field_order(field: &str) -> u8 {
    match field {
        "title" => 0,
        "why" => 1,
        _ => 2,
    }
}

/// Every node that matches, best first, each with the fields it hit on.
///
/// Three keys, read in order, with no weights and no tunable constants.
///
/// **What the hit is about.** A term in the title is what the node is
/// called; a term in the reason is what the node argued; a term in a note
/// or an outcome is what happened along the way. The first of those
/// answers "where was this decided" better than the last, so the field of
/// the best hit dominates everything else. The note and the outcome tie:
/// both are what came after the argument.
///
/// **How much tree it holds up.** Among nodes that hit on the same field
/// the question is which one founded the subject, and the tree already
/// knows: the one everything else hangs off. `Aggregates` has the subtree
/// total of every node from a pass that is already linear, so this costs
/// a lookup.
///
/// **Recency**, last. It was the whole order before `d362` and it is a
/// tiebreak now: a search over a tree that has run for months is answered
/// from the end often enough to be worth keeping, and never often enough
/// to outrank what the hit is about.
///
/// What is deliberately absent is a relevance score. Term frequency, IDF
/// and length normalization are what BM25 would add, and here IDF is inert
/// -- every term has to appear, so every hit contains all of them -- while
/// length normalization is inverted: it penalizes long fields as diluted,
/// and in this corpus a long reason is the reasoning.
fn hits_for<'t>(
    a: &'t Tree,
    ag: &Aggregates,
    terms: &[String],
) -> Vec<(&'t Node, Vec<&'static str>)> {
    let mut hits: Vec<(&Node, Vec<&'static str>)> = Vec::new();
    for n in a.nodes_iter() {
        let lowered: Vec<(&'static str, String)> = searchable(a, n)
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (*k, v.to_lowercase()))
            .collect();
        if !terms
            .iter()
            .all(|t| lowered.iter().any(|(_, v)| v.contains(t.as_str())))
        {
            continue;
        }
        let matched: Vec<&'static str> = lowered
            .iter()
            .filter(|(_, v)| terms.iter().any(|t| v.contains(t.as_str())))
            .map(|(k, _)| *k)
            .collect();
        hits.push((n, matched));
    }
    hits.sort_by_key(|(n, matched)| {
        (
            field_order(matched[0]),
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    hits
}

fn lineage_of(a: &Tree, n: &Node) -> Vec<String> {
    let line = a.ancestors(n.num);
    line[..line.len().saturating_sub(1)]
        .iter()
        .map(|p| p.alias())
        .collect()
}

/// A handle to a hit, not the node itself: `why` on the alias brings the rest.
///
/// Returning the whole node paid for `why`, `note` and `outcome` in full on
/// every hit, plus twelve bookkeeping fields nobody asked for. Measured over
/// the real tree with one query, both numbers from the same run: the JSON
/// cost 8.7 times its own prose and now costs 1.7. `matched` carries the
/// fragment `snippet` would print rather than the whole field, for the same
/// reason.
///
/// The six fields a hit carries, shared with [`find_data_everywhere`] so the
/// shape stays in exactly one place: `d273` adds a `project` field beside
/// this one rather than widening it.
fn hit_json(a: &Tree, n: &Node, matched: &[&'static str], terms: &[String]) -> serde_json::Value {
    let fragments: serde_json::Map<String, serde_json::Value> = matched
        .iter()
        .map(|field| {
            let text = searchable(a, n)
                .iter()
                .find(|(k, _)| k == field)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            (field.to_string(), json!(snippet(text, terms, WIDTH)))
        })
        .collect();
    json!({
        "alias": n.alias(),
        "kind": n.kind,
        "state": n.state,
        "title": n.title(a),
        "lineage": lineage_of(a, n),
        "matched": fragments,
    })
}

pub fn find_data(a: &Tree, query: &str) -> Result<serde_json::Value, Failure> {
    let terms = terms_of(query)?;
    let ag = &a.aggregates();
    Ok(json!(hits_for(a, ag, &terms)
        .iter()
        .map(|(n, matched)| hit_json(a, n, matched, &terms))
        .collect::<Vec<_>>()))
}

pub fn find(a: &Tree, args: &Args) -> R {
    let query = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac find \"<text>\"".to_string()))?;
    let terms = terms_of(query)?;
    if args.has("json") {
        return print_json(find_data(a, query)?);
    }
    let ag = &a.aggregates();
    let hits = hits_for(a, ag, &terms);

    if hits.is_empty() {
        outln!("  Nothing matches \"{query}\".");
        return Ok(());
    }
    outln!();
    outln!(
        "  {} match{} for \"{}\"",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" },
        query,
    );
    outln!();
    for (n, matched) in hits.iter().take(20) {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        let lineage = lineage_of(a, n);
        if !lineage.is_empty() {
            outln!("         via {}", lineage.join(" > "));
        }
        // The title is already on the line above it. Repeating it as the
        // reason the hit came back would say nothing.
        for field in matched.iter().filter(|f| **f != "title") {
            let text = searchable(a, n)
                .iter()
                .find(|(k, _)| k == field)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            outln!("         {}: {}", field, snippet(text, &terms, WIDTH));
        }
    }
    if hits.len() > 20 {
        outln!();
        outln!(
            "  ... and {} more   vivac find \"...\" --json",
            hits.len() - 20
        );
    }
    outln!();
    Ok(())
}

/// The directory's own name, as `d146` defines it: never the path it sits
/// under, because an absolute path names the account and the machine it
/// runs on and the security pillar allows neither into a result.
///
/// `pub(crate)` rather than private since `d273`'s second half: `registry`
/// resolves `--project`'s value against the same bare name this hands back,
/// so a hit `find --everywhere` prints is exactly what `why --project` then
/// takes.
pub(crate) fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
}

/// The JSON twin of [`find_everywhere`]: every hit [`hit_json`] already
/// knows how to build, plus the project it came from. A separate builder
/// rather than a wider `find_data`, the same call `d172` made when `find`
/// stopped sharing `json_node`.
fn find_data_everywhere(projects: &[(String, Tree)], terms: &[String]) -> serde_json::Value {
    let mut hits = Vec::new();
    for (name, tree) in projects {
        let ag = &tree.aggregates();
        for (n, matched) in hits_for(tree, ag, terms) {
            let mut hit = hit_json(tree, n, &matched, terms);
            if let serde_json::Value::Object(fields) = &mut hit {
                fields.insert("project".to_string(), json!(name));
            }
            hits.push(hit);
        }
    }
    json!(hits)
}

/// [`find_everywhere`]'s JSON, with no `Args` to read it from: what
/// `vivac_find`'s `everywhere` argument calls through the MCP server. The
/// same read `find --everywhere --json` runs, so the two can never drift
/// apart -- `d172`'s tie, carried past one project.
pub fn find_everywhere_data(query: &str) -> Result<serde_json::Value, Failure> {
    let terms = terms_of(query)?;
    let known_roots = crate::store::store_dir()
        .map(|d| crate::registry::roots(&d))
        .unwrap_or_default();
    let mut projects: Vec<(String, Tree)> = Vec::new();
    for root in known_roots {
        let name = project_name(&root);
        if let Ok(tree) =
            crate::store::Store::open(root).and_then(|s| crate::index::load(&s, false))
        {
            projects.push((name, tree));
        }
    }
    projects.sort_by(|x, y| x.0.cmp(&y.0));
    Ok(find_data_everywhere(&projects, &terms))
}

/// `find`, fanned out over every project the registry knows about instead
/// of only the one under foot. `d273`'s first half.
///
/// Each tree loads through the local index with `allow_persist: false`:
/// searching from one project must never write inside another project's
/// `.vivac/`. A root that fails to open -- moved, deleted, unreadable -- is
/// not skipped: `d201` settled that a vanished root going quiet loses
/// exactly the answer somebody came for, so it is counted and named instead.
///
/// A bare alias means nothing across trees -- `d100` exists in three of them
/// and names three different decisions -- so text output groups hits by
/// project rather than running them together.
pub fn find_everywhere(a: &Args) -> R {
    let query = a
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac find \"<text>\"".to_string()))?;
    let terms = terms_of(query)?;

    let known_roots = crate::store::store_dir()
        .map(|d| crate::registry::roots(&d))
        .unwrap_or_default();

    let mut projects: Vec<(String, Tree)> = Vec::new();
    let mut unreachable: Vec<String> = Vec::new();
    for root in known_roots {
        let name = project_name(&root);
        match crate::store::Store::open(root).and_then(|s| crate::index::load(&s, false)) {
            Ok(tree) => projects.push((name, tree)),
            Err(_) => unreachable.push(name),
        }
    }
    projects.sort_by(|x, y| x.0.cmp(&y.0));
    unreachable.sort();

    if a.has("json") {
        return print_json(find_data_everywhere(&projects, &terms));
    }

    type ProjectHits<'t> = (&'t str, &'t Tree, Vec<(&'t Node, Vec<&'static str>)>);
    let sections: Vec<ProjectHits> = projects
        .iter()
        .filter_map(|(name, tree)| {
            let ag = &tree.aggregates();
            let hits = hits_for(tree, ag, &terms);
            (!hits.is_empty()).then_some((name.as_str(), tree, hits))
        })
        .collect();
    let total: usize = sections.iter().map(|(_, _, hits)| hits.len()).sum();

    if total == 0 {
        outln!("  Nothing matches \"{query}\".");
    } else {
        outln!();
        outln!(
            "  {} match{} for \"{}\" across {} project{}",
            total,
            if total == 1 { "" } else { "es" },
            query,
            sections.len(),
            if sections.len() == 1 { "" } else { "s" },
        );
        for (name, tree, hits) in &sections {
            outln!();
            outln!("  {name}");
            for (n, matched) in hits.iter().take(20) {
                outln!("    {:<6} {}", n.alias(), n.title(tree));
                let lineage = lineage_of(tree, n);
                if !lineage.is_empty() {
                    outln!("           via {}", lineage.join(" > "));
                }
                for field in matched.iter().filter(|f| **f != "title") {
                    let text = searchable(tree, n)
                        .iter()
                        .find(|(k, _)| k == field)
                        .map(|(_, v)| *v)
                        .unwrap_or_default();
                    outln!("           {}: {}", field, snippet(text, &terms, WIDTH));
                }
            }
            if hits.len() > 20 {
                outln!(
                    "    ... and {} more   vivac find \"...\" --everywhere --json",
                    hits.len() - 20
                );
            }
        }
        outln!();
    }

    if !unreachable.is_empty() {
        outln!(
            "  {} project{} unreachable: {}",
            unreachable.len(),
            if unreachable.len() == 1 { "" } else { "s" },
            unreachable.join(", ")
        );
        outln!();
    }

    Ok(())
}
