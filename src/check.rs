//! `check` — the `MODEL.md` §9 invariants that apply to Tier 0.
//!
//! It separates two things that look alike and are not. A cycle or an orphan
//! is **store corruption**: the tool is lying. A false close is a **finding
//! about the project**: the store is fine and what is wrong is the work,
//! which was called finished without being finished. Both exit non-zero
//! --this belongs in CI-- but they are not counted together.

use crate::args::Args;
use crate::event::State;
use crate::model::Tree;

pub fn check(a: &Tree, args: &Args) -> Result<i32, crate::failure::Failure> {
    let mut store: Vec<String> = Vec::new();
    let mut project: Vec<String> = Vec::new();

    if a.broken_lines > 0 {
        store.push(format!(
            "{} unreadable line(s) in .vivac/events (skipped while reading)",
            a.broken_lines
        ));
    }

    let mut nums = std::collections::HashMap::new();
    for n in a.nodes_iter() {
        // Invariant 11: provenance is a tree. The schema already rules out two
        // parents --`spawns` travels inside the node-- so the only thing that
        // can break here is the parent not existing.
        if let Some(p) = &n.parent {
            if a.node(p).is_none() {
                store.push(format!(
                    "{} points at a parent that does not exist",
                    n.alias()
                ));
            }
        }
        // Invariant 1: acyclic. If the path to the root does not end at a node
        // with no parent, it is going in circles.
        let lineage = a.ancestors(&n.id);
        if lineage.first().is_some_and(|r| r.parent.is_some()) {
            store.push(format!("{} sits in a provenance cycle", n.alias()));
        }
        if let Some(other) = nums.insert(n.num, n.alias()) {
            store.push(format!(
                "number {} repeated: {} and {}",
                n.num,
                other,
                n.alias()
            ));
        }
        // Invariant 10: false close.
        //
        // A **forced** close does not count as a violation: `MODEL.md` §9
        // exempts it on purpose, because there are legitimate forced closes
        // --a lane being abandoned-- and what was asked was that they be a
        // decision and not an oversight. The trace is in the event and the
        // render still marks it; what it does not do is break CI every day.
        if n.state == State::Done && !n.forced_close && !a.open_blockers(&n.id).is_empty() {
            let pending_count = a.open_blockers(&n.id);
            let aliases: Vec<String> = pending_count.iter().map(|c| c.alias()).collect();
            project.push(format!(
                "{} is closed with {} open condition(s): {}",
                n.alias(),
                pending_count.len(),
                aliases.join(", ")
            ));
        }
    }
    store.sort();
    project.sort();

    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "store": store,
                "project": project,
                "ok": store.is_empty() && project.is_empty(),
            }))
            .map_err(std::io::Error::other)?
        );
    } else {
        println!();
        if store.is_empty() && project.is_empty() {
            println!("  No findings. {} nodes checked.", a.total());
            println!();
        }
        if !store.is_empty() {
            println!(
                "  STORE ({})  <- the tool is lying; it needs fixing",
                store.len()
            );
            println!();
            for m in &store {
                println!("      {m}");
            }
            println!();
        }
        if !project.is_empty() {
            println!(
                "  PROJECT ({})  <- the store is fine; the work is not",
                project.len()
            );
            println!();
            for m in &project {
                println!("      {m}");
            }
            println!();
            println!("  A false close is not repaired by editing the tree: reopen what");
            println!("  stayed open, or close it deliberately with --force.");
            println!();
        }
    }
    Ok(i32::from(!(store.is_empty() && project.is_empty())))
}
