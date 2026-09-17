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
use crate::output::outln;
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

    // `t594`: `lock_for_write` can reload the tree if
    // another writer landed first, and everything that read the tree
    // before this point -- `seq`, whether it still counts as empty --
    // has to be read again after, or a second writer racing this one
    // hands out the very `seq` `t594` already fixed a
    // door over. Taken before `seq`/`lane` are read, not after: numbering
    // happens under the lock, like any other write.
    ctx.lock_for_write()?;
    if !ctx.tree.is_empty_tree() {
        return Err(Failure::Model(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.tree.total()
        )));
    }

    let mut events = Vec::new();
    let mut seq = ctx.tree.seq;
    let actor = ctx.store.config.actor.clone();
    // The lane this context actually runs as, `main` only as the fallback
    // every write already uses (`emit`): `import` writes outside the
    // funnel (`write_raw`, below), so it has to decide this itself rather
    // than being signed for automatically (`t594`). A
    // worktree still pending never joins through here -- `import` never
    // calls `emit`, so it never mints a lane of its own -- and its nodes
    // land on `main` exactly as they did before this lane ever existed,
    // rather than inventing a second way to join one.
    let lane = ctx
        .lane
        .clone()
        .unwrap_or_else(|| crate::lane::MAIN.to_string());
    let mut push_event = |body: Body, ts: String| {
        seq += 1;
        events.push(Event {
            seq,
            id: id::ulid(),
            ts,
            actor: actor.clone(),
            lane: lane.clone(),
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
                arms: vec![],
                against: None,
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
    let lock = ctx
        .lock
        .as_ref()
        .ok_or_else(|| Failure::Io(std::io::Error::other("write without the tree's lock")))?;
    ctx.store.write_raw(lock, &events)?;
    outln!("  {total} nodes imported from {file_path}");
    outln!("        {} events written to .vivac/events", events.len());
    outln!();
    outln!("  Review what the spike could not see:  vivac check");
    Ok(())
}
