//! The fold: from the list of events to the tree.
//!
//! The child index is built **here**, during the fold, not looked up by
//! walking every node on each query. With the Python spike it made no
//! difference; under the performance pillar's budget --`why` and `tree` over
//! ten thousand nodes below 50 ms-- a linear `hijos()` turns a render
//! quadratic. Indexes are thought out from the model, not bolted on when
//! they start to hurt.

use crate::anchor::AnchorRef;
use crate::event::{Bandera, Cuerpo, Estado, Evento, Tipo, VivacKind};
use std::collections::BTreeMap;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Nodo {
    pub id: String,
    pub num: u64,
    pub tipo: Tipo,
    pub titulo: String,
    /// Why it was born. `push` demands it: a detour with no reason is the
    /// very failure this project attacks.
    pub por: String,
    pub estado: Estado,
    pub padre: Option<String>,
    /// The parent's closure condition. Explicit, and by default it does **not**
    /// block: forcing it leaves parents that never close. `MODEL.md` §5.
    pub bloquea: bool,
    pub nota: String,
    pub resultado: String,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub abierto: String,
    pub cerrado: Option<String>,
    pub cierre_forzado: bool,
    /// Flag -> reason. Orthogonal to state: a node can be `active` and
    /// `suspect` at the same time.
    pub banderas: BTreeMap<Bandera, String>,
}

/// A safe stop. Immutable: there is no event that modifies one.
#[derive(Debug, Clone)]
pub struct Vivac {
    pub id: String,
    pub num: u64,
    pub kind: VivacKind,
    pub pila: Vec<(String, String)>,
    pub working_set: Vec<String>,
    pub next_intent: String,
    pub anchor: AnchorRef,
    pub node_ref: Option<String>,
    pub etiqueta: String,
    pub ts: String,
}

impl Vivac {
    pub fn alias(&self) -> String {
        format!("v{}", self.num)
    }
}

impl Nodo {
    pub fn alias(&self) -> String {
        format!("{}{}", self.tipo.prefijo(), self.num)
    }

    /// A front is open work somebody can sit down and do.
    ///
    /// A standing decision is open and is **not** a front: you do not execute
    /// it, it governs, and it closes itself when another supersedes it.
    /// Listing it beside pending work fills the brief with things not to do,
    /// which is exactly the opposite of what it exists for.
    pub fn es_frente(&self) -> bool {
        self.estado.abierto() && self.tipo != Tipo::Decision
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Recuento {
    pub total: usize,
    pub abiertos: usize,
    pub cerrados: usize,
    pub aparcados: usize,
}

impl Recuento {
    pub fn frase(&self) -> String {
        let mut p = Vec::new();
        if self.abiertos > 0 {
            p.push(format!("{} abiertos", self.abiertos));
        }
        if self.cerrados > 0 {
            p.push(format!("{} cerrados", self.cerrados));
        }
        if self.aparcados > 0 {
            p.push(format!("{} aparcados", self.aparcados));
        }
        p.join(" / ")
    }
}

#[derive(Debug, Default)]
pub struct Arbol {
    nodos: HashMap<String, Nodo>,
    hijos: HashMap<String, Vec<String>>,
    por_num: HashMap<u64, String>,
    pub raices: Vec<String>,
    pub pila: Vec<String>,
    pub vivacs: Vec<Vivac>,
    pub siguiente_vivac: u64,
    pub seq: u64,
    /// Seq of the last event that **changed something**, and of the last
    /// vivac. Together they tell whether anything happened since the previous
    /// stop, which is what separates a useful stop from forty identical ones:
    /// Claude Code's `Stop` hook runs every turn, not at session close (`f35`).
    pub seq_cambio: u64,
    pub seq_vivac: u64,
    pub siguiente_num: u64,
    pub lineas_rotas: usize,
}

pub fn plegar(eventos: &[Evento], rotas: usize) -> Arbol {
    let mut a = Arbol {
        lineas_rotas: rotas,
        ..Default::default()
    };
    for e in eventos {
        a.aplicar(e.seq, &e.ts, &e.payload);
    }
    a.ordenar();
    a
}

impl Arbol {
    /// Applies one event.
    ///
    /// The fold uses it at startup and so does `emitir`, right after writing.
    /// If the in-memory tree did not follow the log, every operation would
    /// print the count from **before** doing it --"back to the parent, 1 open
    /// below" for the node you just closed-- which is the kind of small lie
    /// that makes you stop trusting the rest.
    pub fn aplicar(&mut self, seq: u64, ts: &str, cuerpo: &Cuerpo) {
        self.seq = self.seq.max(seq);
        if matches!(cuerpo, Cuerpo::VivacCreado { .. }) {
            self.seq_vivac = self.seq_vivac.max(seq);
        } else {
            self.seq_cambio = self.seq_cambio.max(seq);
        }
        match cuerpo {
            Cuerpo::NodoCreado {
                nodo,
                num,
                tipo,
                titulo,
                por,
                padre,
                bloquea,
                refs,
                governs,
            } => {
                if self.nodos.contains_key(nodo) {
                    // Repeated creation: commutative, the first one wins.
                    return;
                }
                self.nodos.insert(
                    nodo.clone(),
                    Nodo {
                        id: nodo.clone(),
                        num: *num,
                        tipo: *tipo,
                        titulo: titulo.clone(),
                        por: por.clone(),
                        estado: Estado::Active,
                        padre: padre.clone(),
                        bloquea: *bloquea,
                        nota: String::new(),
                        resultado: String::new(),
                        refs: refs.clone(),
                        governs: governs.clone(),
                        abierto: crate::clock::date_of(ts).to_string(),
                        cerrado: None,
                        cierre_forzado: false,
                        banderas: BTreeMap::new(),
                    },
                );
                self.por_num.insert(*num, nodo.clone());
                self.siguiente_num = self.siguiente_num.max(*num + 1);
                match padre {
                    Some(p) => self.hijos.entry(p.clone()).or_default().push(nodo.clone()),
                    None => self.raices.push(nodo.clone()),
                }
            }
            Cuerpo::EstadoCambiado {
                nodo,
                estado,
                resultado,
                forzado,
            } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.estado = *estado;
                    if !resultado.is_empty() {
                        n.resultado = resultado.clone();
                    }
                    n.cierre_forzado = *forzado;
                    n.cerrado = if estado.abierto() {
                        None
                    } else {
                        Some(crate::clock::date_of(ts).to_string())
                    };
                }
            }
            Cuerpo::NodoAnotado { nodo, nota } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.nota = nota.clone();
                }
            }
            Cuerpo::BloqueoCambiado { nodo, bloquea } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.bloquea = *bloquea;
                }
            }
            Cuerpo::Apilado { nodo } => {
                if !self.pila.contains(nodo) {
                    self.pila.push(nodo.clone());
                }
            }
            Cuerpo::Desapilado { nodo } => {
                self.pila.retain(|x| x != nodo);
            }
            Cuerpo::BanderaAlzada {
                nodo,
                bandera,
                motivo,
            } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.banderas.insert(*bandera, motivo.clone());
                }
            }
            Cuerpo::BanderaBajada { nodo, bandera } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.banderas.remove(bandera);
                }
            }
            Cuerpo::VivacCreado {
                vivac,
                num,
                kind,
                pila,
                working_set,
                next_intent,
                anchor,
                node_ref,
                etiqueta,
            } => {
                self.siguiente_vivac = self.siguiente_vivac.max(*num + 1);
                self.vivacs.push(Vivac {
                    id: vivac.clone(),
                    num: *num,
                    kind: *kind,
                    pila: pila.clone(),
                    working_set: working_set.clone(),
                    next_intent: next_intent.clone(),
                    anchor: anchor.clone(),
                    node_ref: node_ref.clone(),
                    etiqueta: etiqueta.clone(),
                    ts: ts.to_string(),
                });
            }
            Cuerpo::Promovido { nodo } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.tipo = Tipo::Goal;
                }
                // The stack is cut at the promoted node: it becomes the root
                // of its own. The provenance chain is untouched: where it was
                // born does not change because its rank did.
                if let Some(i) = self.pila.iter().position(|x| x == nodo) {
                    self.pila.drain(..i);
                }
            }
        }
    }

    /// Stable order by number: two renders of the same log are identical.
    ///
    /// Only needed while folding. Live, nodes are born with an increasing
    /// number, so appending at the end already leaves the right order.
    pub fn ordenar(&mut self) {
        let nums: std::collections::HashMap<String, u64> =
            self.nodos.iter().map(|(k, n)| (k.clone(), n.num)).collect();
        for v in self.hijos.values_mut() {
            v.sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
        }
        self.raices
            .sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
    }
}

impl Arbol {
    pub fn vacio(&self) -> bool {
        self.nodos.is_empty()
    }

    pub fn total(&self) -> usize {
        self.nodos.len()
    }

    pub fn nodo(&self, id: &str) -> Option<&Nodo> {
        self.nodos.get(id)
    }

    pub fn todos(&self) -> impl Iterator<Item = &Nodo> {
        self.nodos.values()
    }

    /// Resolves whatever the user types: `7`, `t7` or the whole ULID.
    /// The bare number works on purpose --`vivac why 7`-- because forcing
    /// anyone to recall the prefix is capture cost with nothing in return.
    pub fn resolver(&self, s: &str) -> Option<&Nodo> {
        let limpio = s.trim().trim_start_matches('#');
        if let Ok(n) = limpio.parse::<u64>() {
            return self.por_num.get(&n).and_then(|id| self.nodos.get(id));
        }
        let sin_prefijo = &limpio[1..];
        if limpio.len() > 1 && sin_prefijo.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = sin_prefijo.parse::<u64>() {
                return self
                    .por_num
                    .get(&n)
                    .and_then(|id| self.nodos.get(id))
                    .filter(|nd| nd.tipo.prefijo() == limpio.chars().next().unwrap());
            }
        }
        self.nodos.get(limpio)
    }

    pub fn hijos(&self, id: &str) -> Vec<&Nodo> {
        self.hijos
            .get(id)
            .map(|v| v.iter().filter_map(|i| self.nodos.get(i)).collect())
            .unwrap_or_default()
    }

    pub fn raices(&self) -> Vec<&Nodo> {
        self.raices
            .iter()
            .filter_map(|i| self.nodos.get(i))
            .collect()
    }

    /// Node to root, reversed: root first. This is the path `why` walks.
    /// The `visto` set is not paranoia: a hand-edited log can hold a cycle,
    /// and hanging would be worse than giving a short path.
    pub fn ancestros(&self, id: &str) -> Vec<&Nodo> {
        let mut camino = Vec::new();
        let mut visto = std::collections::HashSet::new();
        let mut cur = self.nodos.get(id);
        while let Some(n) = cur {
            if !visto.insert(&n.id) {
                break;
            }
            camino.push(n);
            cur = n.padre.as_deref().and_then(|p| self.nodos.get(p));
        }
        camino.reverse();
        camino
    }

    pub fn descendientes(&self, id: &str) -> Vec<&Nodo> {
        let mut out = Vec::new();
        let mut pila = vec![id.to_string()];
        let mut visto = std::collections::HashSet::new();
        while let Some(cur) = pila.pop() {
            for h in self.hijos.get(&cur).map(|v| v.as_slice()).unwrap_or(&[]) {
                if visto.insert(h.clone()) {
                    if let Some(n) = self.nodos.get(h) {
                        out.push(n);
                    }
                    pila.push(h.clone());
                }
            }
        }
        out.sort_by_key(|n| n.num);
        out
    }

    /// Open descendants marked as a closure condition.
    ///
    /// **Transitive on purpose**: a blocking grandchild blocks the grandparent.
    /// Without that, slipping one node in between is enough to skip the guard
    /// by accident, which is exactly how a false close gets in.
    pub fn bloqueantes_abiertos(&self, id: &str) -> Vec<&Nodo> {
        self.descendientes(id)
            .into_iter()
            .filter(|n| n.bloquea && n.estado.abierto())
            .collect()
    }

    pub fn recuento(&self, id: &str) -> Recuento {
        let d = self.descendientes(id);
        Recuento {
            total: d.len(),
            abiertos: d.iter().filter(|n| n.estado == Estado::Active).count(),
            cerrados: d.iter().filter(|n| n.estado == Estado::Done).count(),
            aparcados: d.iter().filter(|n| n.estado == Estado::Suspended).count(),
        }
    }

    /// The most recent vivac. Vivacs are appended in event order, so the last
    /// one in the vector is the last one in time.
    pub fn ultimo_vivac(&self) -> Option<&Vivac> {
        self.vivacs.last()
    }

    pub fn vivac(&self, s: &str) -> Option<&Vivac> {
        let n: u64 = s.trim().trim_start_matches(['#', 'v']).parse().ok()?;
        self.vivacs.iter().find(|v| v.num == n)
    }

    pub fn foco(&self) -> Option<&Nodo> {
        self.pila.last().and_then(|id| self.nodos.get(id))
    }

    pub fn profundidad_pila(&self) -> usize {
        self.pila.len()
    }
}

/// Subtree counts for every node, computed in one go.
///
/// Asking each node for its own count walks its whole subtree, and doing
/// that for the whole tree makes it quadratic: measured, `tree` over ten
/// thousand nodes went from 79 ms on a subtree to 242 ms on the full tree,
/// and the 163 ms of difference were this, not the log.
///
/// A single post-order pass gets the same answer in linear time. It is the
/// kind of index the performance pillar demands be thought out from the
/// model instead of bolted on when it hurts.
#[derive(Debug, Default)]
pub struct Agregados {
    recuento: HashMap<String, Recuento>,
    bloqueantes: HashMap<String, usize>,
    pub profundidad_max: usize,
}

impl Agregados {
    pub fn recuento(&self, id: &str) -> Recuento {
        self.recuento.get(id).copied().unwrap_or_default()
    }

    pub fn bloqueantes(&self, id: &str) -> usize {
        self.bloqueantes.get(id).copied().unwrap_or(0)
    }
}

impl Arbol {
    pub fn agregados(&self) -> Agregados {
        let mut ag = Agregados::default();

        // Orphans hang off no root. They get walked anyway: a broken tree has
        // to stay inspectable, which is what `check` is for.
        let mut entradas: Vec<&String> = self.raices.iter().collect();
        entradas.extend(
            self.nodos
                .values()
                .filter(|n| {
                    n.padre
                        .as_ref()
                        .is_some_and(|p| !self.nodos.contains_key(p))
                })
                .map(|n| &n.id),
        );

        let mut orden: Vec<(&String, usize)> = Vec::with_capacity(self.nodos.len());
        let mut pila: Vec<(&String, usize)> = entradas.into_iter().map(|id| (id, 1)).collect();
        let mut visto = std::collections::HashSet::new();
        while let Some((id, hondo)) = pila.pop() {
            if !visto.insert(id) {
                continue;
            }
            ag.profundidad_max = ag.profundidad_max.max(hondo);
            orden.push((id, hondo));
            if let Some(hs) = self.hijos.get(id) {
                pila.extend(hs.iter().map(|h| (h, hondo + 1)));
            }
        }

        // From the leaves upward: each parent sums what its children have plus
        // the children themselves.
        for (id, _) in orden.iter().rev() {
            let mut r = Recuento::default();
            let mut b = 0usize;
            for h in self.hijos.get(*id).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(hijo) = self.nodos.get(h) else {
                    continue;
                };
                let hr = ag.recuento(h);
                r.total += hr.total + 1;
                r.abiertos += hr.abiertos + usize::from(hijo.estado == Estado::Active);
                r.cerrados += hr.cerrados + usize::from(hijo.estado == Estado::Done);
                r.aparcados += hr.aparcados + usize::from(hijo.estado == Estado::Suspended);
                b += ag.bloqueantes(h) + usize::from(hijo.bloquea && hijo.estado == Estado::Active);
            }
            ag.recuento.insert((*id).clone(), r);
            ag.bloqueantes.insert((*id).clone(), b);
        }
        ag
    }
}
