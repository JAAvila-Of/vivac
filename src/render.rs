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
use serde_json::json;
use std::collections::HashMap;

pub(crate) const WIDTH: usize = 62;

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

fn label(n: &Node) -> String {
    match n.state {
        State::Active => n.title.clone(),
        e => format!("{}  [{}]", n.title, e.word(n.kind)),
    }
}

fn json_node(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let r = ag.counts(&n.id);
    json!({
        "id": n.id,
        "alias": n.alias(),
        "num": n.num,
        "kind": n.kind,
        "title": n.title,
        "why": n.why,
        "state": n.state,
        "blocks": n.blocks,
        "parent": n.parent.as_ref().and_then(|p| a.node(p).map(|x| x.alias())),
        "note": n.note,
        "outcome": n.outcome,
        "refs": n.refs,
        "governs": n.governs,
        "opened": n.opened,
        "closed": n.closed,
        "false_close": n.state == State::Done && ag.blockers(&n.id) > 0,
        "open_below": r.open_count,
        "total_below": r.total,
    })
}

pub(crate) fn print_json(v: serde_json::Value) -> R {
    println!(
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
    a.children(&n.id)
        .into_iter()
        .filter(|c| c.kind == Kind::Decision && c.state.is_open())
        .collect()
}

/// The siblings of `n`, born before it by `Node::num`, that were still open
/// at the `seq` `n` was born. Not by `closed`'s date: two siblings can open
/// and close on the day `n` was born, in an order the date cannot tell
/// apart.
pub(crate) fn open_then_of<'a>(a: &'a Tree, full: &Full, n: &Node) -> Vec<&'a Node> {
    let (Some(&seq), Some(parent)) = (full.created.get(&n.id), n.parent.as_ref()) else {
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
    let lineage = a.ancestors(&n.id);
    let node_json = |x: &Node| match full {
        Some(f) => json_node_full(a, ag, f, x),
        None => json_node(a, ag, x),
    };
    let siblings: Vec<_> = n
        .parent
        .as_ref()
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
        "born_here": a.children(&n.id).iter().filter(|c| c.state.is_open())
            .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
        "blockers": a.open_blockers(&n.id).iter()
            .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
    }))
}

pub fn why_data(a: &Tree, id: &str) -> Result<serde_json::Value, Failure> {
    why_data_impl(a, None, id)
}

/// The open fronts as data.
pub fn open_data(a: &Tree) -> serde_json::Value {
    let ag = &a.aggregates();
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(&n.id).iter().any(|c| c.is_front()))
        .collect();
    leaves.sort_by_key(|n| n.num);
    json!(leaves
        .iter()
        .map(|n| {
            let mut v = json_node(a, ag, n);
            v["lineage"] = json!(a
                .ancestors(&n.id)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .map(|p| p.alias())
                .collect::<Vec<_>>());
            v
        })
        .collect::<Vec<_>>())
}

/// The `--full` lines for one step of the path, printed the way the JSON
/// twin carries the same three fields: the anchor, the decisions still
/// standing, and the siblings still open at that moment.
fn print_full_of(a: &Tree, full: &Full, n: &Node) {
    let anchor = anchor_of(a, full, n);
    if anchor.is_empty_tree() {
        println!("        anchor: none");
    } else {
        println!("        anchor: {} ({})", anchor.short(), anchor.kind);
    }
    let standing = standing_of(a, n);
    if !standing.is_empty() {
        println!(
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
        println!(
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
    let lineage = a.ancestors(&n.id);
    let full = args.has("full").then(|| Full::from_log(log));

    if args.has("json") {
        return print_json(match &full {
            Some(f) => why_data_impl(a, Some(f), s)?,
            None => why_data(a, s)?,
        });
    }

    println!();
    println!("  Why we are here  ->  {}", n.alias());
    println!("  {}", "-".repeat(66));
    println!();
    for (i, p) in lineage.iter().enumerate() {
        let is_last = i == lineage.len() - 1;
        println!("  {:<6}{}", p.alias(), label(p));
        for l in wrap(&p.why, WIDTH, "        ") {
            println!("{l}");
        }
        for l in wrap(&format!("! {}", p.note), WIDTH, "        ") {
            if !p.note.is_empty() {
                println!("{l}");
            }
        }
        for l in wrap(&format!("= {}", p.outcome), WIDTH, "        ") {
            if !p.outcome.is_empty() {
                println!("{l}");
            }
        }
        if let Some(f) = &full {
            print_full_of(a, f, p);
        }
        if !is_last {
            let f = ag.counts(&p.id).phrase();
            if !f.is_empty() {
                println!("        ({f} below)");
            }
            println!("        |");
            println!("        v");
        } else {
            println!();
            println!("        ^^^ you are here");
        }
    }
    println!();

    // "we had ten things to review, we are on the first"
    if let Some(parent) = n.parent.as_ref() {
        let siblings: Vec<_> = a
            .children(parent)
            .into_iter()
            .filter(|c| c.id != n.id && c.state.is_open())
            .collect();
        if !siblings.is_empty() {
            println!("  In parallel, still open ({}):", siblings.len());
            for c in siblings {
                println!("      {:<6} {}", c.alias(), c.title);
            }
            println!();
        }
    }

    let kids: Vec<_> = a
        .children(&n.id)
        .into_iter()
        .filter(|c| c.state.is_open())
        .collect();
    if !kids.is_empty() {
        println!("  Born here and still open ({}):", kids.len());
        for c in kids {
            println!(
                "    {} {:<6} {}",
                if c.blocks { '*' } else { ' ' },
                c.alias(),
                c.title
            );
        }
        println!();
    }

    for p in &lineage {
        let pending_count = a.open_blockers(&p.id);
        if !pending_count.is_empty() && p.state.is_open() {
            println!(
                "  {} does not close until these close ({}):",
                p.alias(),
                pending_count.len()
            );
            for c in pending_count {
                println!("      {:<6} {}", c.alias(), c.title);
            }
            println!();
        }
    }
    Ok(())
}

fn branch(a: &Tree, ag: &Aggregates, n: &Node, prefix: &str, is_last: bool, show_all: bool) {
    let f = ag.counts(&n.id).phrase();
    let mut tail = if f.is_empty() {
        String::new()
    } else {
        format!("   ({f})")
    };
    let pending_count = ag.blockers(&n.id);
    if n.state == State::Done && pending_count > 0 {
        tail.push_str(&format!(
            "   <== FALSE CLOSE: {pending_count} open condition(s)"
        ));
    }
    let mark = if n.blocks { "* " } else { "" };
    println!(
        "{prefix}{}[{}] {:<6} {mark}{}{tail}",
        if is_last { "`-- " } else { "|-- " },
        n.state.mark(),
        n.alias(),
        n.title
    );
    let sig = format!("{prefix}{}", if is_last { "    " } else { "|   " });
    let children: Vec<_> = a
        .children(&n.id)
        .into_iter()
        .filter(|h| show_all || h.state.is_open() || ag.counts(&h.id).open_count > 0)
        .collect();
    for (i, h) in children.iter().enumerate() {
        branch(a, ag, h, &sig, i == children.len() - 1, show_all);
    }
}

fn subtree_json(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let mut v = json_node(a, ag, n);
    v["children"] = json!(a
        .children(&n.id)
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
        println!("  Empty tree.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    let show_all = args.has("all");
    println!();
    for (i, n) in roots.iter().enumerate() {
        branch(a, ag, n, "  ", i == roots.len() - 1, show_all);
    }
    println!();
    if !show_all {
        println!("  (closed nodes with no open descendants hidden; --all shows them)");
        println!();
    }
    Ok(())
}

/// `open` — the open fronts, each with its lineage compressed. It is the
/// "where was I" view for the start of the day.
pub fn open(a: &Tree, args: &Args) -> R {
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(&n.id).iter().any(|c| c.is_front()))
        .collect();
    leaves.sort_by_key(|n| n.num);
    let standing = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Decision && n.state.is_open())
        .count();
    if args.has("json") {
        return print_json(open_data(a));
    }
    if leaves.is_empty() && standing == 0 {
        println!("  Nothing open.");
        return Ok(());
    }
    println!();
    println!(
        "  {} open front{}",
        leaves.len(),
        if leaves.len() == 1 { "" } else { "s" },
    );
    println!();
    for n in leaves {
        println!("  {:<6} {}", n.alias(), n.title);
        let lineage = a.ancestors(&n.id);
        if lineage.len() > 1 {
            let v: Vec<String> = lineage[..lineage.len() - 1]
                .iter()
                .map(|p| p.alias())
                .collect();
            println!("         via {}", v.join(" > "));
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
        println!();
        println!("  + {phrase}   vivac brief");
    }
    println!();
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
        .map(|n| (n, a.under_goal(&n.id).len()))
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
            let p = a.node(n.parent.as_deref()?)?;
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
        .filter(|n| n.state == State::Done && !n.forced_close && ag.blockers(&n.id) > 0)
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
                v["discarded_because"] = json!(p.outcome);
                v
            }).collect::<Vec<_>>(),
            "false_closes": false_closes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }

    let total = parked_nodes.len() + deep.len() + orphaned.len() + false_closes.len();
    if total == 0 {
        println!("  Nothing to prune.");
        return Ok(());
    }
    println!();
    println!("  TRIAGE - {total} thing(s) to look at");

    if !parked_nodes.is_empty() {
        println!();
        println!(
            "  PARKED ({})                       focus <id>  |  abandon <id>",
            parked_nodes.len()
        );
        for n in &parked_nodes {
            println!("    {:<6} {}", n.alias(), n.title);
            for l in wrap(&n.outcome, WIDTH, "           ") {
                println!("{l}");
            }
        }
    }

    if !deep.is_empty() {
        println!();
        println!(
            "  6 OR MORE FROM ITS GOAL ({})      promote <id>",
            deep.len()
        );
        for (n, d) in &deep {
            println!("    {:<6} {:<40} depth {d}", n.alias(), clip(&n.title, 40));
            // The lineage starts where the number does. Drawing it from the
            // root beside a distance to the goal would say two things at once.
            let v: Vec<String> = a
                .under_goal(&n.id)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .map(|p| p.alias())
                .collect();
            println!("           via {}", v.join(" > "));
        }
    }

    if !orphaned.is_empty() {
        println!();
        println!(
            "  SURVIVED A DISCARD ({})           abandon <id>  |  promote <id>",
            orphaned.len()
        );
        for (n, p) in &orphaned {
            println!("    {:<6} {}", n.alias(), n.title);
            println!(
                "           born from {}, discarded: {}",
                p.alias(),
                clip(&p.outcome, 36)
            );
        }
    }

    if !false_closes.is_empty() {
        println!();
        println!(
            "  FALSE CLOSES ({})                 close what is left, or --force",
            false_closes.len()
        );
        for n in &false_closes {
            println!(
                "    {:<6} {:<40} {} blocker(s)",
                n.alias(),
                clip(&n.title, 40),
                ag.blockers(&n.id)
            );
        }
    }
    println!();
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
        println!("  Nothing parked.");
        return Ok(());
    }
    println!();
    println!("  DO NOT TOUCH NOW ({})", ps.len());
    println!();
    for n in ps {
        println!("  {:<6} {}", n.alias(), n.title);
        for l in wrap(&n.outcome, WIDTH, "         ") {
            println!("{l}");
        }
    }
    println!();
    Ok(())
}

/// `stack` — where you are right now, from the root to the focus.
pub fn stack(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let stack: Vec<&Node> = a.stack.iter().filter_map(|id| a.node(id)).collect();
    if args.has("json") {
        return print_json(json!({
            "depth": stack.len(),
            "stack": stack.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    if stack.is_empty() {
        println!("  Empty stack.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    println!();
    for (i, n) in stack.iter().enumerate() {
        let focus = if i == stack.len() - 1 {
            "   <- focus"
        } else {
            ""
        };
        println!("  {}{:<6} {}{focus}", "  ".repeat(i), n.alias(), n.title);
    }
    println!();
    if stack.len() >= 6 {
        println!(
            "  Stack {} levels deep. Almost never lack of discipline: usually",
            stack.len()
        );
        println!("  the root goal moved and nobody re-rooted.  vivac promote");
        println!();
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
        if n.parent.as_ref().is_some_and(|p| a.node(p).is_none()) {
            orphans += 1;
        }
        if n.state == State::Done && ag.blockers(&n.id) > 0 {
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
    println!();
    println!("  nodes          {}", a.total());
    for (k, v) in &by_state {
        println!("  {k:<14} {v}");
    }
    println!("  depth          {depth_of}");
    println!("  roots          {}", a.roots().len());
    println!("  stack          {}", a.stack_depth());
    if orphans > 0 {
        println!("  ORPHANS        {orphans}  <- broken provenance");
    }
    if a.broken_lines > 0 {
        println!("  broken lines   {}  <- in .vivac/events", a.broken_lines);
    }
    if !false_closes.is_empty() {
        println!();
        println!("  FALSE CLOSES ({})", false_closes.len());
        for n in false_closes {
            println!("      {:<6} {}", n.alias(), n.title);
        }
    }
    println!();
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
        println!("  No stops yet.  vivac save \"<label>\"");
        return Ok(());
    }
    println!();
    for v in a.vivacs.iter().rev().take(20) {
        let top = v
            .stack
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "empty stack".into());
        println!(
            "  {:<5} {:<7} {}  {}",
            v.alias(),
            v.kind.word(),
            crate::clock::date_of(&v.ts),
            top
        );
        if !v.label.is_empty() {
            println!("           {}", v.label);
        }
        if !v.next_intent.is_empty() {
            println!("           you were about to: {}", v.next_intent);
        }
    }
    if a.vivacs.len() > 20 {
        println!();
        println!("  ... and {} more", a.vivacs.len() - 20);
    }
    println!();
    Ok(())
}

/// The fields of a node that carry meaning, in the order a reader wants them.
///
/// The title is a label; the reason, the note and the outcome are where the
/// thinking is. A search that read only titles would find the folder and miss
/// what is inside it.
fn searchable(n: &Node) -> [(&'static str, &str); 4] {
    [
        ("title", n.title.as_str()),
        ("why", n.why.as_str()),
        ("note", n.note.as_str()),
        ("outcome", n.outcome.as_str()),
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
/// Newest first, because a search over a tree that has been running for
/// months is answered from the end far more often than from the beginning.
fn terms_of(query: &str) -> Result<Vec<String>, Failure> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() {
        return Err(Failure::usage("usage: vivac find \"<text>\"".to_string()));
    }
    Ok(terms)
}

/// Every node that matches, newest first, each with the fields it hit on.
fn hits_for<'t>(a: &'t Tree, terms: &[String]) -> Vec<(&'t Node, Vec<&'static str>)> {
    let mut hits: Vec<(&Node, Vec<&'static str>)> = Vec::new();
    for n in a.nodes_iter() {
        let lowered: Vec<(&'static str, String)> = searchable(n)
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
    hits.sort_by_key(|(n, _)| std::cmp::Reverse(n.num));
    hits
}

fn lineage_of(a: &Tree, n: &Node) -> Vec<String> {
    let line = a.ancestors(&n.id);
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
pub fn find_data(a: &Tree, query: &str) -> Result<serde_json::Value, Failure> {
    let terms = terms_of(query)?;
    Ok(json!(hits_for(a, &terms)
        .iter()
        .map(|(n, matched)| {
            let fragments: serde_json::Map<String, serde_json::Value> = matched
                .iter()
                .map(|field| {
                    let text = searchable(n)
                        .iter()
                        .find(|(k, _)| k == field)
                        .map(|(_, v)| *v)
                        .unwrap_or_default();
                    (field.to_string(), json!(snippet(text, &terms, WIDTH)))
                })
                .collect();
            json!({
                "alias": n.alias(),
                "kind": n.kind,
                "state": n.state,
                "title": n.title,
                "lineage": lineage_of(a, n),
                "matched": fragments,
            })
        })
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
    let hits = hits_for(a, &terms);

    if hits.is_empty() {
        println!("  Nothing matches \"{query}\".");
        return Ok(());
    }
    println!();
    println!(
        "  {} match{} for \"{}\"",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" },
        query,
    );
    println!();
    for (n, matched) in hits.iter().take(20) {
        println!("  {:<6} {}", n.alias(), n.title);
        let lineage = lineage_of(a, n);
        if !lineage.is_empty() {
            println!("         via {}", lineage.join(" > "));
        }
        // The title is already on the line above it. Repeating it as the
        // reason the hit came back would say nothing.
        for field in matched.iter().filter(|f| **f != "title") {
            let text = searchable(n)
                .iter()
                .find(|(k, _)| k == field)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            println!("         {}: {}", field, snippet(text, &terms, WIDTH));
        }
    }
    if hits.len() > 20 {
        println!();
        println!(
            "  ... and {} more   vivac find \"...\" --json",
            hits.len() - 20
        );
    }
    println!();
    Ok(())
}
