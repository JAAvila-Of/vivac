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
use crate::event::{Cuerpo, Estado, Evento, Tipo};
use crate::fallo::{Fallo, R};
use crate::ops::Ctx;
use crate::{id, redact};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Viejo {
    nodes: BTreeMap<String, NodoViejo>,
}

#[derive(Deserialize)]
struct NodoViejo {
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

fn tipo_de(kind: &str) -> Tipo {
    match kind {
        "goal" => Tipo::Goal,
        "decision" => Tipo::Decision,
        "finding" => Tipo::Finding,
        // `run` and `issue` were work subtypes in the spike. The model does
        // not distinguish them: `MODEL.md` §4.2 leaves `task` as the only work
        // entity, and `finding` fits as a field, not as a state.
        _ => Tipo::Task,
    }
}

fn estado_de(status: &str) -> Estado {
    match status {
        "done" => Estado::Done,
        "parked" => Estado::Suspended,
        "superseded" => Estado::Superseded,
        _ => Estado::Active,
    }
}

fn instante(fecha: &str) -> String {
    if fecha.len() == 10 {
        format!("{fecha}T12:00:00Z")
    } else {
        crate::clock::now_rfc3339()
    }
}

pub fn import(ctx: &mut Ctx, args: &Args) -> R {
    let ruta = args
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac import <path to tree.json>"))?;
    if !ctx.arbol.vacio() {
        return Err(Fallo::Modelo(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.arbol.total()
        )));
    }
    let crudo = std::fs::read_to_string(ruta)?;
    let viejo: Viejo = serde_json::from_str(&crudo)
        .map_err(|e| Fallo::uso(format!("{ruta} is not a spike tree.json: {e}")))?;

    let mut nodos: Vec<&NodoViejo> = viejo.nodes.values().collect();
    nodos.sort_by_key(|n| n.id);

    // The redaction guard runs **before** anything is written. A tree coming
    // from outside is exactly the case where a key may have slipped in.
    for n in &nodos {
        let campos: Vec<(&str, &str)> = vec![
            ("title", &n.title),
            ("why", &n.why),
            ("outcome", &n.outcome),
            ("note", &n.note),
        ];
        if let Some(mut h) = redact::revisar_campos(&campos) {
            h.campo = format!("node #{} ({})", n.id, h.campo);
            return Err(Fallo::Redaccion(Box::new(h)));
        }
        if let Some(mut h) = n.refs.iter().find_map(|r| redact::revisar("ref", r)) {
            h.campo = format!("node #{} (ref)", n.id);
            return Err(Fallo::Redaccion(Box::new(h)));
        }
    }

    let ulids: BTreeMap<u64, String> = nodos.iter().map(|n| (n.id, id::ulid())).collect();
    let mut eventos = Vec::new();
    let mut seq = 0u64;
    let actor = ctx.store.config.actor.clone();
    let mut empujar = |cuerpo: Cuerpo, ts: String| {
        seq += 1;
        eventos.push(Evento {
            seq,
            id: id::ulid(),
            ts,
            actor: actor.clone(),
            lane: "main".into(),
            payload: cuerpo,
        });
    };

    for n in &nodos {
        empujar(
            Cuerpo::NodoCreado {
                nodo: ulids[&n.id].clone(),
                num: n.id,
                tipo: tipo_de(&n.kind),
                titulo: n.title.clone(),
                por: n.why.clone(),
                padre: n.parent.and_then(|p| ulids.get(&p).cloned()),
                bloquea: n.blocks,
                refs: n.refs.clone(),
                governs: vec![],
            },
            instante(&n.opened),
        );
    }
    for n in &nodos {
        if !n.note.is_empty() {
            empujar(
                Cuerpo::NodoAnotado {
                    nodo: ulids[&n.id].clone(),
                    nota: n.note.clone(),
                },
                instante(&n.opened),
            );
        }
        let estado = estado_de(&n.status);
        if estado != Estado::Active {
            empujar(
                Cuerpo::EstadoCambiado {
                    nodo: ulids[&n.id].clone(),
                    estado,
                    resultado: n.outcome.clone(),
                    // The spike had no closure rule, so there is no way to
                    // know whether a close was deliberate. They import
                    // unforced: the ones that turn out false have to show up
                    // in `check`, which is exactly what needs to be seen.
                    forzado: false,
                },
                instante(n.closed.as_deref().unwrap_or(&n.opened)),
            );
        }
    }

    let total = nodos.len();
    ctx.store.escribir_crudo(&eventos)?;
    println!("  {total} nodes imported from {ruta}");
    println!(
        "        {} events written to .vivac/events",
        eventos.len()
    );
    println!();
    println!("  Review what the spike could not see:  vivac check");
    Ok(())
}
