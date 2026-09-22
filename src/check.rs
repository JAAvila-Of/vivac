//! `check` — the `MODEL.md` §9 invariants that apply to Tier 0.
//!
//! It separates two things that look alike and are not. A cycle or an orphan
//! is **store corruption**: the tool is lying. A false close, or a decision
//! that could have declared what it was judged against and named nothing, is
//! a **finding about the project**: the store is fine and what is wrong is
//! the work. Both exit non-zero --this belongs in CI-- but they are not
//! counted together.
//!
//! `--gates` adds a third category, and it follows the same split: the store
//! is fine and **nothing is delivering it**. `d350`/`d351` settled what that
//! means for `MODEL.md` Tier 0 -- the one gate that matters is whether the
//! brief reaches the agent at all, the MCP is not a gate, and it is measured
//! off each project's own log rather than the host's configuration: the hook
//! writes `session.started` when it fires.
//!
//! A project reports here when every node it holds was written before the
//! first session was ever opened -- including a log that never opened one at
//! all. "Zero openings, ever" is not the criterion, and a real tree is why:
//! it held one opening, eight days *after* its last node. That single late
//! opening cleared a "never opened" filter while every node in the tree had
//! in fact been written with no brief in front of anybody.

use crate::args::Args;
use crate::event::{Body, Kind, State};
use crate::model::Tree;
use crate::output::outln;
use std::path::Path;

pub fn check(a: &Tree, root: &Path, args: &Args) -> Result<i32, crate::failure::Failure> {
    let mut store: Vec<String> = Vec::new();
    let mut project: Vec<String> = Vec::new();
    // Tracked apart from `project`'s own count so each footer prints only
    // for the finding that actually produced it (`t426` §4): a tree can
    // hold a false close with no undeclared decision, or the other way
    // round.
    let mut false_close_count = 0usize;
    let mut undeclared_count = 0usize;

    if a.broken_lines > 0 {
        store.push(format!(
            "{} unreadable line(s) in .vivac/events (skipped while reading)",
            a.broken_lines
        ));
    }

    // One ULID, one `num`. With `num` as `Tree`'s own storage key, only the
    // first of two claimants ever makes it into `nodes_iter` below -- the
    // fold records the second at the moment it loses, since a scan
    // afterwards has nothing left to see.
    for d in &a.repeated_nums {
        store.push(format!(
            "number {} repeated: {} and {}",
            d.num, d.first, d.second
        ));
    }

    // `f610`/`f604`: two corruptions the fold above never named, found by
    // one full pass over the raw log rather than the derived `Tree` --
    // `check` carries no budget of its own, unlike the write path this
    // pass never touches.
    let scan = crate::store::scan_log(&root.join(crate::store::DIR).join(crate::store::LOG))?;
    for r in &scan.repeated_seqs {
        store.push(format!(
            "seq {} appears twice, at line {} and line {}",
            r.seq, r.first_line, r.second_line
        ));
    }
    if let Some(line) = scan.torn_tail {
        store.push(format!(
            "line {line} does not end with a newline: whatever was appended after it was \
             swallowed and cannot be recovered from this log"
        ));
    }

    for n in a.nodes_iter() {
        // Invariant 11: provenance is a tree. The schema already rules out two
        // parents --`spawns` travels inside the node-- so the only thing that
        // can break here is the parent not existing.
        if let Some(p) = n.parent {
            if a.node_by_num(p).is_none() {
                store.push(format!(
                    "{} points at a parent that does not exist",
                    n.alias()
                ));
            }
        }
        // Invariant 1: acyclic. If the path to the root does not end at a node
        // with no parent, it is going in circles.
        let lineage = a.ancestors(n.num);
        if lineage.first().is_some_and(|r| r.parent.is_some()) {
            store.push(format!("{} sits in a provenance cycle", n.alias()));
        }
        // Invariant 10: false close.
        //
        // A **forced** close does not count as a violation: `MODEL.md` §9
        // exempts it on purpose, because there are legitimate forced closes
        // --a lane being abandoned-- and what was asked was that they be a
        // decision and not an oversight. The trace is in the event and the
        // render still marks it; what it does not do is break CI every day.
        if n.state == State::Done && !n.forced_close && !a.open_blockers(n.num).is_empty() {
            let pending_count = a.open_blockers(n.num);
            let aliases: Vec<String> = pending_count.iter().map(|c| c.alias()).collect();
            project.push(format!(
                "{} is closed with {} open condition(s): {}",
                n.alias(),
                pending_count.len(),
                aliases.join(", ")
            ));
            false_close_count += 1;
        }
        // `t426` §4: a decision that could have declared what it was judged
        // against -- its `node.created` carried the key -- and never did,
        // at birth or later. One born with no key never had the chance, and
        // does not count.
        if n.kind == Kind::Decision
            && n.state.is_open()
            && n.against_recorded
            && n.against.is_empty()
        {
            project.push(format!(
                "{0} declares nothing it was judged against: vivac declare {0} --against \"<id>: <why>\"",
                n.alias()
            ));
            undeclared_count += 1;
        }
    }

    // `t594` §4.9: a log inside a git working tree is one `git add .` away
    // from travelling to every clone, where each copy diverges.
    if crate::anchor::in_working_tree(root) {
        if !root
            .join(crate::store::DIR)
            .join(crate::store::GITIGNORE)
            .is_file()
        {
            project.push(
                ".vivac/.gitignore is missing, so git can pick up the log: vivac init writes it"
                    .to_string(),
            );
        }
        match crate::anchor::tracks(root, ".vivac/events") {
            Some(true) => project.push(crate::anchor::EVENTS_TRACKED_WARNING.to_string()),
            Some(false) => {}
            None => project.push(
                "git could not tell whether .vivac/events is tracked here: it is not on \
                 PATH, or it refuses this folder. git status shows which"
                    .to_string(),
            ),
        }
    }

    store.sort();
    project.sort();

    // `t594` §4.7: another folder on this machine may still hold a tree
    // that starts with this same event -- a copy, not a move (`d201`) --
    // and `check` is where that gets said. `copy_of` is a single, bounded
    // lookup: one registry key, plus one `first_event_id` check per entry
    // already in that project's own `copies` (typically 0 or 1, pruned on
    // the next write once a copy is deleted), never the `--gates` fan-out
    // below over every root the registry knows, so it runs on every
    // `check` rather than only that one.
    let copy: Option<(Option<String>, Vec<Option<String>>)> = crate::store::first_event_id(root)
        .and_then(|project_id| {
            let store_dir = crate::store::store_dir()?;
            match crate::registry::copy_of(&store_dir, &project_id, root) {
                crate::registry::Noted::Copy { first, rest } => Some((first, rest)),
                crate::registry::Noted::Fine => None,
            }
        });

    // The fan-out over every root the registry knows, read only when asked:
    // without `--gates` this never reads another project's log, and the
    // JSON and the exit code stay exactly what they were before this flag
    // existed. The single, bounded registry lookup above is unconditional;
    // this multi-project scan is the one `--gates` guards.
    let mut gates: Vec<String> = Vec::new();
    if args.has("gates") {
        if let Some(store_dir) = crate::store::store_dir() {
            for root in crate::registry::roots(&store_dir) {
                let Ok(project_store) = crate::store::Store::open(root.clone()) else {
                    continue;
                };
                let Ok((events, _)) = project_store.read_all() else {
                    continue;
                };
                let mut nodes = 0u64;
                let mut before_first_opening = 0u64;
                let mut opened = false;
                for e in &events {
                    match &e.payload {
                        Body::NodeCreated { .. } => {
                            nodes += 1;
                            if !opened {
                                before_first_opening += 1;
                            }
                        }
                        Body::SessionStarted { .. } => opened = true,
                        _ => {}
                    }
                }
                if nodes > 0 && before_first_opening == nodes {
                    gates.push(format!(
                        "{}: {} nodes written, and not one after a session ever opened",
                        crate::render::project_name(&root),
                        nodes
                    ));
                }
            }
        }
        gates.sort();
    }
    let ok = store.is_empty() && project.is_empty() && gates.is_empty() && copy.is_none();

    if args.has("json") {
        let mut payload = serde_json::json!({
            "store": store,
            "project": project,
            "ok": ok,
        });
        if let serde_json::Value::Object(fields) = &mut payload {
            if let Some((first, rest)) = &copy {
                let mut others = vec![first.clone()];
                others.extend(rest.clone());
                fields.insert("copy".to_string(), serde_json::json!({ "others": others }));
            }
            if args.has("gates") {
                fields.insert("gates".to_string(), serde_json::json!(gates));
            }
        }
        outln!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(std::io::Error::other)?
        );
    } else {
        outln!();
        if ok {
            outln!("  No findings. {} nodes checked.", a.total());
            outln!();
        }
        if let Some((first, rest)) = &copy {
            let notice = crate::registry::copy_notice(first.as_deref(), rest);
            outln!("  {}", notice.heading);
            outln!();
            for line in notice.body.lines() {
                outln!("      {line}");
            }
            outln!();
        }
        if !store.is_empty() {
            outln!(
                "  STORE ({})  <- the tool is lying; it needs fixing",
                store.len()
            );
            outln!();
            for m in &store {
                outln!("      {m}");
            }
            outln!();
        }
        if !project.is_empty() {
            outln!(
                "  PROJECT ({})  <- the store is fine; the work is not",
                project.len()
            );
            outln!();
            for m in &project {
                outln!("      {m}");
            }
            outln!();
            if false_close_count > 0 {
                outln!("  A false close is not repaired by editing the tree: reopen what");
                outln!("  stayed open, or close it deliberately with --force.");
                outln!();
            }
            if undeclared_count > 0 {
                outln!("  A decision that declared nothing stays as it was written: vivac declare");
                outln!("  adds what it was judged against, and why shows it as late.");
                outln!();
            }
        }
        if !gates.is_empty() {
            outln!(
                "  GATES ({})  <- the store is fine; nothing delivers it",
                gates.len()
            );
            outln!();
            for m in &gates {
                outln!("      {m}");
            }
            outln!();
            outln!("  A tree nobody opens is a tree nobody reads. Run  vivac hooks  inside");
            outln!("  that project and paste what it prints.");
            outln!();
        }
    }
    Ok(i32::from(!ok))
}
