//! `import` — brings in the `tree.json` from the Python spike.
//!
//! Three trees were seeded with the spike and filled in by hand against real
//! projects. Redoing them would throw away the only raw material this
//! project has, so the migration is part of the port, not an extra.
//!
//! Two things are preserved on purpose: **the node number** --the design
//! documents cite `#8` and `#11`, and if the number changed those references
//! would stop resolving-- and **the original date**, written into the event's
//! `ts`. The alternative was flattening the whole timeline onto today.

use crate::args::Args;
use crate::event::{Body, Event, Kind, State};
use crate::failure::{Failure, R};
use crate::ops::Ctx;
use crate::{id, redact};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Old {
    nodes: BTreeMap<String, OldNode>,
}

#[derive(Deserialize)]
struct OldNode {
    id: u64,
    title: String,
    kind: String,
    status: String,
    parent: Option<u64>,
    #[serde(default)]
    why: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    refs: Vec<String>,
    #[serde(default)]
    blocks: bool,
    #[serde(default)]
    opened: String,
    #[serde(default)]
    closed: Option<String>,
}

fn kind_of(kind: &str) -> Kind {
    match kind {
        "goal" => Kind::Goal,
        "decision" => Kind::Decision,
        "finding" => Kind::Finding,
        // `run` and `issue` were work subtypes in the spike. The model does
        // not distinguish them: `MODEL.md` §4.2 leaves `task` as the only work
        // entity, and `finding` fits as a field, not as a state.
        _ => Kind::Task,
    }
}

fn state_of(status: &str) -> State {
    match status {
        "done" => State::Done,
        "parked" => State::Suspended,
        "superseded" => State::Superseded,
        _ => State::Active,
    }
}

fn instant(date: &str) -> String {
    if date.len() == 10 {
        format!("{date}T12:00:00Z")
    } else {
        crate::clock::now_rfc3339()
    }
}

pub fn import(ctx: &mut Ctx, args: &Args) -> R {
    let file_path = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac import <path to tree.json>"))?;
    if !ctx.tree.is_empty_tree() {
        return Err(Failure::Model(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.tree.total()
        )));
    }
    let raw = std::fs::read_to_string(file_path)?;
    let old: Old = serde_json::from_str(&raw)
        .map_err(|e| Failure::usage(format!("{file_path} is not a spike tree.json: {e}")))?;

    let mut nodes: Vec<&OldNode> = old.nodes.values().collect();
    nodes.sort_by_key(|n| n.id);

    // The redaction guard runs **before** anything is written. A tree coming
    // from outside is exactly the case where a key may have slipped in.
    for n in &nodes {
        let fields: Vec<(&str, &str)> = vec![
            ("title", &n.title),
            ("why", &n.why),
            ("outcome", &n.outcome),
            ("note", &n.note),
        ];
        if let Some(mut h) = redact::check_fields(&fields) {
            h.field = format!("node #{} ({})", n.id, h.field);
            return Err(Failure::Redaction(Box::new(h)));
        }
        if let Some(mut h) = n.refs.iter().find_map(|r| redact::check_field("ref", r)) {
            h.field = format!("node #{} (ref)", n.id);
            return Err(Failure::Redaction(Box::new(h)));
        }
    }

    let ulids: BTreeMap<u64, String> = nodes.iter().map(|n| (n.id, id::ulid())).collect();
    let mut events = Vec::new();
    let mut seq = 0u64;
    let actor = ctx.store.config.actor.clone();
    let mut push_event = |body: Body, ts: String| {
        seq += 1;
        events.push(Event {
            seq,
            id: id::ulid(),
            ts,
            actor: actor.clone(),
            lane: "main".into(),
            payload: body,
        });
    };

    for n in &nodes {
        push_event(
            Body::NodeCreated {
                node: ulids[&n.id].clone(),
                num: n.id,
                kind: kind_of(&n.kind),
                title: n.title.clone(),
                why: n.why.clone(),
                parent: n.parent.and_then(|p| ulids.get(&p).cloned()),
                blocks: n.blocks,
                refs: n.refs.clone(),
                governs: vec![],
            },
            instant(&n.opened),
        );
    }
    for n in &nodes {
        if !n.note.is_empty() {
            push_event(
                Body::NodeNoted {
                    node: ulids[&n.id].clone(),
                    note: n.note.clone(),
                },
                instant(&n.opened),
            );
        }
        let state = state_of(&n.status);
        if state != State::Active {
            push_event(
                Body::StateChanged {
                    node: ulids[&n.id].clone(),
                    state,
                    outcome: n.outcome.clone(),
                    // The spike had no closure rule, so there is no way to
                    // know whether a close was deliberate. They import
                    // unforced: the ones that turn out false have to show up
                    // in `check`, which is exactly what needs to be seen.
                    forced: false,
                },
                instant(n.closed.as_deref().unwrap_or(&n.opened)),
            );
        }
    }

    let total = nodes.len();
    ctx.store.write_raw(&events)?;
    println!("  {total} nodes imported from {file_path}");
    println!("        {} events written to .vivac/events", events.len());
    println!();
    println!("  Review what the spike could not see:  vivac check");
    Ok(())
}
