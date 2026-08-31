//! `import` — trae el `tree.json` del spike en Python.
//!
//! Existen tres arboles sembrados con el spike y llenos a mano contra
//! proyectos reales. Rehacerlos seria tirar la unica materia prima que tiene
//! este proyecto, asi que la migracion es parte del port, no un extra.
//!
//! Dos cosas se conservan a proposito: **el numero del nodo** --los documentos
//! de diseño citan `#8` y `#11`, y si el numero cambiara las referencias
//! dejarian de resolver-- y **la fecha original**, que se escribe en el `ts`
//! del evento. La alternativa era aplastar toda la linea de tiempo a hoy.

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
        // `run` e `issue` eran subtipos de trabajo en el spike. El modelo no
        // los distingue: `MODEL.md` §4.2 deja `task` como unica entidad de
        // trabajo, y `finding` cabe como campo, no como estado.
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
        .ok_or_else(|| Fallo::uso("uso: vivac import <ruta a tree.json>"))?;
    if !ctx.arbol.vacio() {
        return Err(Fallo::Modelo(format!(
            "  El arbol ya tiene {} nodos. Importar encima duplicaria numeros.\n\n  \
             Importa en un .vivac/ recien creado.",
            ctx.arbol.total()
        )));
    }
    let crudo = std::fs::read_to_string(ruta)?;
    let viejo: Viejo = serde_json::from_str(&crudo)
        .map_err(|e| Fallo::uso(format!("{ruta} no es un tree.json del spike: {e}")))?;

    let mut nodos: Vec<&NodoViejo> = viejo.nodes.values().collect();
    nodos.sort_by_key(|n| n.id);

    // La guarda de redaccion corre **antes** de escribir nada. Un arbol que
    // viene de fuera es justo el caso en que puede haberse colado una clave.
    for n in &nodos {
        let campos: Vec<(&str, &str)> = vec![
            ("titulo", &n.title),
            ("por", &n.why),
            ("resultado", &n.outcome),
            ("nota", &n.note),
        ];
        if let Some(mut h) = redact::revisar_campos(&campos) {
            h.campo = format!("nodo #{} ({})", n.id, h.campo);
            return Err(Fallo::Redaccion(Box::new(h)));
        }
        if let Some(mut h) = n.refs.iter().find_map(|r| redact::revisar("ref", r)) {
            h.campo = format!("nodo #{} (ref)", n.id);
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
                    // El spike no tenia regla de cierre, asi que no puede
                    // saberse si un cierre fue deliberado. Se importan sin
                    // forzar: los que resulten falsos tienen que salir en
                    // `check`, que es exactamente lo que hay que ver.
                    forzado: false,
                },
                instante(n.closed.as_deref().unwrap_or(&n.opened)),
            );
        }
    }

    let total = nodos.len();
    ctx.store.escribir_crudo(&eventos)?;
    println!("  {total} nodos importados desde {ruta}");
    println!(
        "        {} eventos escritos en .vivac/events",
        eventos.len()
    );
    println!();
    println!("  Revisa lo que el spike no podia ver:  vivac check");
    Ok(())
}
