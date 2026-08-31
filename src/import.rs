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
use crate::event::{Body, State, Event, Kind};
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

fn tipo_de(kind: &str) -> Kind {
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

fn estado_de(status: &str) -> State {
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
    let path_arg = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac import <path to tree.json>"))?;
    if !ctx.tree.is_empty_tree() {
        return Err(Failure::Model(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.tree.total()
        )));
    }
    let crudo = std::fs::read_to_string(path_arg)?;
    let old: Old = serde_json::from_str(&crudo)
        .map_err(|e| Failure::usage(format!("{path_arg} is not a spike tree.json: {e}")))?;

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
    let mut eventos = Vec::new();
    let mut seq = 0u64;
    let actor = ctx.store.config.actor.clone();
    let mut empujar = |cuerpo: Body, ts: String| {
        seq += 1;
        eventos.push(Event {
            seq,
            id: id::ulid(),
            ts,
            actor: actor.clone(),
            lane: "main".into(),
            payload: cuerpo,
        });
    };

    for n in &nodes {
        empujar(
            Body::NodeCreated {
                node: ulids[&n.id].clone(),
                num: n.id,
                kind: tipo_de(&n.kind),
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
            empujar(
                Body::NodeNoted {
                    node: ulids[&n.id].clone(),
                    note: n.note.clone(),
                },
                instant(&n.opened),
            );
        }
        let state = estado_de(&n.status);
        if state != State::Active {
            empujar(
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
    ctx.store.write_raw(&eventos)?;
    println!("  {total} nodes imported from {path_arg}");
    println!(
        "        {} events written to .vivac/events",
        eventos.len()
    );
    println!();
    println!("  Review what the spike could not see:  vivac check");
    Ok(())
}
