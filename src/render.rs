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

use crate::args::Args;
use crate::brief::clip;
use crate::event::{State, Kind};
use crate::failure::{Failure, R};
use crate::model::{Aggregates, Tree, Node};
use serde_json::json;

const WIDTH: usize = 62;

fn wrap(text: &str, ancho: usize, sangria: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for p in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + p.chars().count() > ancho {
            lines.push(format!("{sangria}{cur}"));
            cur = p.to_string();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(p);
        }
    }
    if !cur.is_empty() {
        lines.push(format!("{sangria}{cur}"));
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

fn print_json(v: serde_json::Value) -> R {
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
pub fn why(a: &Tree, args: &Args) -> R {
    let ag = &a.agregados();
    let s = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac why <id>"))?;
    let n = a
        .resolve(s)
        .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))?;
    let camino = a.ancestors(&n.id);

    if args.has("json") {
        let siblings: Vec<_> = n
            .parent
            .as_ref()
            .map(|p| a.children(p))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.id != n.id && c.state.is_open())
            .map(|c| json_node(a, ag, c))
            .collect();
        return print_json(json!({
            "node": json_node(a, ag, n),
            "path": camino.iter().map(|x| json_node(a, ag, x)).collect::<Vec<_>>(),
            "en_paralelo": siblings,
            "born_here": a.children(&n.id).iter().filter(|c| c.state.is_open())
                .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
            "blockers": a.open_blockers(&n.id).iter()
                .map(|c| json_node(a, ag, c)).collect::<Vec<_>>(),
        }));
    }

    println!();
    println!("  Why we are here  ->  {}", n.alias());
    println!("  {}", "-".repeat(66));
    println!();
    for (i, p) in camino.iter().enumerate() {
        let is_last = i == camino.len() - 1;
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

    for p in &camino {
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

fn branch(a: &Tree, ag: &Aggregates, n: &Node, prefix: &str, is_last: bool, todo: bool) {
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
        .filter(|h| todo || h.state.is_open() || ag.counts(&h.id).open_count > 0)
        .collect();
    for (i, h) in children.iter().enumerate() {
        branch(a, ag, h, &sig, i == children.len() - 1, todo);
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
    let ag = &a.agregados();
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
    let todo = args.has("all") || args.has("all");
    println!();
    for (i, n) in roots.iter().enumerate() {
        branch(a, ag, n, "  ", i == roots.len() - 1, todo);
    }
    println!();
    if !todo {
        println!("  (closed nodes with no open descendants hidden; --all shows them)");
        println!();
    }
    Ok(())
}

/// `open` — the open fronts, each with its lineage compressed. It is the
/// "where was I" view for the start of the day.
pub fn open(a: &Tree, args: &Args) -> R {
    let ag = &a.agregados();
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
        return print_json(json!(leaves
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
            .collect::<Vec<_>>()));
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
        let camino = a.ancestors(&n.id);
        if camino.len() > 1 {
            let v: Vec<String> = camino[..camino.len() - 1]
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
/// where. `MODEL.md` §6.1 also sends it the deep nodes, because a deep stack
/// is almost never lack of discipline: it is that the root goal moved and
/// nobody re-rooted.
pub fn triage(a: &Tree, args: &Args) -> R {
    let ag = &a.agregados();

    let mut parked_nodes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();

    // `MODEL.md` §6.1: from 6 on it shows up here, and it never blocks.
    let mut deep: Vec<(&Node, usize)> = a
        .nodes_iter()
        .filter(|n| n.is_front())
        .map(|n| (n, a.ancestors(&n.id).len()))
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
        .filter(|n| n.state == State::Done && !n.cierre_forzado && ag.blockers(&n.id) > 0)
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
                v["depth"] = json!(d);
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
            "  6 OR MORE FROM THE ROOT ({})      promote <id>",
            deep.len()
        );
        for (n, d) in &deep {
            println!(
                "    {:<6} {:<40} depth {d}",
                n.alias(),
                clip(&n.title, 40)
            );
            let v: Vec<String> = a
                .ancestors(&n.id)
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
    let ag = &a.agregados();
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
    let ag = &a.agregados();
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
    let ag = &a.agregados();
    let mut por_estado = std::collections::BTreeMap::new();
    let mut orphans = 0usize;
    let mut false_closes = Vec::new();
    for n in a.nodes_iter() {
        *por_estado.entry(n.state.word(n.kind)).or_insert(0usize) += 1;
        if n.parent.as_ref().is_some_and(|p| a.node(p).is_none()) {
            orphans += 1;
        }
        if n.state == State::Done && ag.blockers(&n.id) > 0 {
            false_closes.push(n);
        }
    }
    let depth_of = ag.profundidad_max;
    false_closes.sort_by_key(|n| n.num);
    if args.has("json") {
        return print_json(json!({
            "nodes": a.total(),
            "by_state": por_estado,
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
    for (k, v) in &por_estado {
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
        let cima = v
            .stack
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "empty stack".into());
        println!(
            "  {:<5} {:<7} {}  {}",
            v.alias(),
            v.kind.word(),
            crate::clock::date_of(&v.ts),
            cima
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
