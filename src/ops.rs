//! The operations that write. Every one goes through the redaction guard.
//!
//! Capture hangs off the seams of the work, never off a judgement of
//! relevance. That is the one thing actually measured: over 170 real minutes,
//! `push`/`pop` --which cannot be skipped without leaving the work half done--
//! were called nine times, and the operation that asked "is this worth
//! keeping?" was called zero times, under a protocol declared mandatory.

use crate::anchor::{self, Anchor};
use crate::args::Args;
use crate::event::{Bandera, Cuerpo, Estado, Tipo, VivacKind};
use crate::fallo::{Fallo, R};
use crate::model::{plegar, Arbol, Nodo};
use crate::store::Store;
use crate::{id, redact};

pub struct Ctx {
    pub store: Store,
    pub arbol: Arbol,
    pub anchor: Box<dyn Anchor>,
}

impl Ctx {
    pub fn cargar(store: Store) -> Result<Ctx, Fallo> {
        let (eventos, rotas) = store.leer()?;
        let arbol = plegar(&eventos, rotas);
        let anchor = anchor::detectar(&store.raiz);
        Ok(Ctx {
            store,
            arbol,
            anchor,
        })
    }

    /// Writes and **then applies in memory**, so that whatever gets printed
    /// next is the state after the operation and not the one before it.
    fn emitir(&mut self, cuerpos: Vec<Cuerpo>) -> R {
        self.store.escribir(cuerpos.clone(), self.arbol.seq)?;
        let ts = crate::clock::now_rfc3339();
        for c in &cuerpos {
            let seq = self.arbol.seq + 1;
            self.arbol.aplicar(seq, &ts, c);
        }
        Ok(())
    }

    fn resolver(&self, s: &str) -> Result<&crate::model::Nodo, Fallo> {
        self.arbol
            .resolver(s)
            .ok_or_else(|| Fallo::uso(format!("No such node: {s}.")))
    }
}

/// Builds a vivac out of the stack as it stands right now.
///
/// The `working_set` is **not measured**: measuring which files the pitch
/// touched would need a `post_tool` hook, which is not in Tier 0. It is
/// derived from the `governs` the stack declares, which is what there is, and
/// the `brief` says so rather than pretending it observed it.
fn vivac(
    ctx: &Ctx,
    kind: VivacKind,
    next_intent: &str,
    node_ref: Option<String>,
    etiqueta: &str,
) -> Cuerpo {
    let pila: Vec<(String, String)> = ctx
        .arbol
        .pila
        .iter()
        .filter_map(|id| ctx.arbol.nodo(id))
        .map(|n| (n.alias(), n.titulo.clone()))
        .collect();
    let mut working_set: Vec<String> = ctx
        .arbol
        .pila
        .iter()
        .filter_map(|id| ctx.arbol.nodo(id))
        .flat_map(|n| n.governs.iter().cloned())
        .collect();
    working_set.sort();
    working_set.dedup();
    Cuerpo::VivacCreado {
        vivac: id::ulid(),
        num: ctx.arbol.siguiente_vivac.max(1),
        kind,
        pila,
        working_set,
        next_intent: next_intent.to_string(),
        anchor: ctx.anchor.snapshot(),
        node_ref,
        etiqueta: etiqueta.to_string(),
    }
}

/// No text reaches the log without coming through here.
fn guardar_texto(campos: &[(&str, &str)]) -> R {
    match redact::revisar_campos(campos) {
        Some(h) => Err(Fallo::Redaccion(Box::new(h))),
        None => Ok(()),
    }
}

fn tipo_de(a: &Args, por_defecto: Tipo) -> Result<Tipo, Fallo> {
    match a.opt("type") {
        None => Ok(por_defecto),
        Some(s) => Tipo::desde(s)
            .ok_or_else(|| Fallo::uso(format!("Unknown type: {s}. They are: {}", Tipo::TODOS))),
    }
}

/// Creates a node. Returns the event and the alias number assigned.
fn nacer(
    ctx: &Ctx,
    titulo: &str,
    por: &str,
    tipo: Tipo,
    padre: Option<String>,
    a: &Args,
) -> Result<(Cuerpo, u64, String), Fallo> {
    let refs = a.lista("ref");
    let governs = a.lista("governs");
    let mut campos: Vec<(&str, &str)> = vec![("titulo", titulo), ("por", por)];
    campos.extend(refs.iter().map(|r| ("ref", r.as_str())));
    campos.extend(governs.iter().map(|g| ("governs", g.as_str())));
    guardar_texto(&campos)?;

    let nodo = id::ulid();
    let num = ctx.arbol.siguiente_num.max(1);
    Ok((
        Cuerpo::NodoCreado {
            nodo: nodo.clone(),
            num,
            tipo,
            titulo: titulo.to_string(),
            por: por.to_string(),
            padre,
            bloquea: a.tiene("blocks"),
            refs,
            governs,
        },
        num,
        nodo,
    ))
}

/// `push` — open a detour. It is **the** operation: the provenance edge is
/// created here on its own, with nobody having to remember to declare it.
pub fn push(ctx: &mut Ctx, a: &Args) -> R {
    let titulo = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac push \"<title>\" --why \"<reason>\""))?;
    let por = a.opt("why").ok_or_else(|| {
        Fallo::uso(
            "Missing --why. A detour with no reason is exactly the failure this\n  \
             exists to attack: in a month nobody will know why.",
        )
    })?;

    let padre = ctx.arbol.foco().map(|n| n.id.clone());
    let tipo = tipo_de(
        a,
        if padre.is_none() {
            Tipo::Goal
        } else {
            Tipo::Task
        },
    )?;
    let (ev, num, nodo) = nacer(ctx, titulo, por, tipo, padre.clone(), a)?;
    // The vivac goes **before** the push: it freezes the stack at the moment
    // of the fork, which is the belay where you make yourself safe before
    // setting off. The `next_intent` is the child being opened, because that
    let v = vivac(ctx, VivacKind::Push, titulo, padre, "");
    ctx.emitir(vec![v, ev, Cuerpo::Apilado { nodo }])?;

    // `emitir` already applied the push in memory, so the stack includes the
    // new node and there is no need to add one.
    let hondo = ctx.arbol.profundidad_pila();
    println!("  {}{}  {}", tipo.prefijo(), num, titulo);
    if a.tiene("blocks") {
        println!("        blocks its parent from closing");
    }
    // §6.1: intervene, never block. A deep stack is almost never lack of
    // discipline: the root goal moved and nobody re-rooted.
    if hondo >= 4 {
        if let Some(raiz) = ctx.arbol.raices().first() {
            println!();
            println!(
                "  You are {hondo} levels away from {} \"{}\".",
                raiz.alias(),
                raiz.titulo
            );
            println!("  Is this still a detour, or did the real goal move?");
            println!("  If it moved:  vivac promote");
        }
    }
    Ok(())
}

/// `pop` — close the focus and come back to the parent with context.
pub fn pop(ctx: &mut Ctx, a: &Args) -> R {
    let foco = ctx
        .arbol
        .foco()
        .ok_or_else(|| {
            Fallo::uso(
                "The stack is empty. Open something:  vivac push \"<title>\" --why \"<reason>\"",
            )
        })?
        .clone();
    let resultado = a.libre(0).unwrap_or("");
    let luego = a.opt("next").unwrap_or(resultado);
    guardar_texto(&[("outcome", resultado), ("next", luego)])?;
    let v = vivac(ctx, VivacKind::Pop, luego, Some(foco.id.clone()), "");
    cerrar(ctx, &foco, resultado, a.tiene("force"), true)?;
    ctx.emitir(vec![v])?;
    match ctx.arbol.nodo(foco.padre.as_deref().unwrap_or("")) {
        Some(p) => {
            let r = ctx.arbol.recuento(&p.id);
            println!("  back to {}  {}", p.alias(), p.titulo);
            let f = r.frase();
            if !f.is_empty() {
                println!("        ({f} below it)");
            }
        }
        None => println!("  empty stack"),
    }
    Ok(())
}

/// `park` — what produces DO NOT TOUCH NOW; without it that section always
/// comes out empty. The closure rule does not stop it: parking claims nothing
/// finished, and if parking cost more than ignoring, nobody would park.
pub fn park(ctx: &mut Ctx, a: &Args) -> R {
    let (nodo, motivo) = match a.libre(0).and_then(|s| ctx.arbol.resolver(s)) {
        Some(n) => (n.clone(), a.libre(1).unwrap_or("")),
        None => {
            let f = ctx
                .arbol
                .foco()
                .ok_or_else(|| Fallo::uso("usage: vivac park [<id>] [\"<reason>\"]"))?
                .clone();
            (f, a.libre(0).unwrap_or(""))
        }
    };
    guardar_texto(&[("reason", motivo)])?;
    let mut evs = vec![vivac(
        ctx,
        VivacKind::Park,
        motivo,
        Some(nodo.id.clone()),
        "",
    )];
    evs.push(Cuerpo::EstadoCambiado {
        nodo: nodo.id.clone(),
        estado: Estado::Suspended,
        resultado: motivo.to_string(),
        forzado: false,
    });
    if ctx.arbol.pila.contains(&nodo.id) {
        evs.push(Cuerpo::Desapilado {
            nodo: nodo.id.clone(),
        });
    }
    ctx.emitir(evs)?;
    println!("  {}  {}  -> parked", nodo.alias(), nodo.titulo);
    println!("        shows up in:  vivac parked");
    Ok(())
}

/// The closure rule. `MODEL.md` §7, and the **only** rule in the model that
/// refuses a user operation.
///
/// It earns that privilege because the case it prevents is measured: an
/// audit marked DONE with its findings open took 26 days to be spotted.
/// Without this, the model lets the same mistake happen again.
fn cerrar(
    ctx: &mut Ctx,
    n: &crate::model::Nodo,
    resultado: &str,
    forzar: bool,
    desapilar: bool,
) -> R {
    if !forzar {
        let pend = ctx.arbol.bloqueantes_abiertos(&n.id);
        if !pend.is_empty() {
            let mut m = format!(
                "  {} CANNOT close: {} open closure condition(s)\n",
                n.alias(),
                pend.len()
            );
            for c in &pend {
                m.push_str(&format!("\n      {:<6} {}", c.alias(), c.titulo));
            }
            m.push_str(&format!(
                "\n\n  A run closes with its findings, not with its report.\n  \
                 Closing it anyway leaves a trace:  vivac done {} --force",
                n.num
            ));
            return Err(Fallo::Modelo(m));
        }
    }
    let mut evs = vec![Cuerpo::EstadoCambiado {
        nodo: n.id.clone(),
        estado: Estado::Done,
        resultado: resultado.to_string(),
        forzado: forzar,
    }];
    if desapilar && ctx.arbol.pila.contains(&n.id) {
        evs.push(Cuerpo::Desapilado { nodo: n.id.clone() });
    }
    ctx.emitir(evs)?;
    println!(
        "  {}  {}  -> {}",
        n.alias(),
        n.titulo,
        if forzar {
            "closed BY FORCE"
        } else {
            "closed"
        }
    );
    if forzar {
        println!("        recorded as a false close in every render");
    }
    Ok(())
}

pub fn done(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac done <id> [\"<outcome>\"] [--force]"))?;
    let n = ctx.resolver(s)?.clone();
    let resultado = a.libre(1).unwrap_or("");
    guardar_texto(&[("outcome", resultado)])?;
    cerrar(ctx, &n, resultado, a.tiene("force"), true)
}

/// `add` — a node without touching the stack. It is how a tree that already
/// existed elsewhere gets in, and how a finding hangs off something that is
pub fn add(ctx: &mut Ctx, a: &Args) -> R {
    let titulo = a.libre(0).ok_or_else(|| {
        Fallo::uso("usage: vivac add \"<title>\" [--parent N] [--why \"<reason>\"]")
    })?;
    let padre = match a.opt("parent") {
        Some(p) => Some(ctx.resolver(p)?.id.clone()),
        None => ctx.arbol.foco().map(|n| n.id.clone()),
    };
    let tipo = tipo_de(
        a,
        if padre.is_none() {
            Tipo::Goal
        } else {
            Tipo::Task
        },
    )?;
    let (ev, num, _) = nacer(ctx, titulo, &a.opt_o("why"), tipo, padre.clone(), a)?;
    ctx.emitir(vec![ev])?;
    let donde = match padre.and_then(|p| ctx.arbol.nodo(&p).map(|n| n.alias())) {
        Some(al) => format!(" under {al}"),
        None => " (root)".into(),
    };
    println!("  {}{}  {}{}", tipo.prefijo(), num, titulo, donde);
    if a.tiene("blocks") {
        println!("        blocks its parent from closing");
    }
    Ok(())
}

pub fn note(ctx: &mut Ctx, a: &Args) -> R {
    let (n, nota) = match (a.libre(0), a.libre(1)) {
        (Some(s), Some(t)) => (ctx.resolver(s)?.clone(), t),
        (Some(t), None) => {
            let f = ctx
                .arbol
                .foco()
                .ok_or_else(|| Fallo::uso("usage: vivac note [<id>] \"<note>\""))?;
            (f.clone(), t)
        }
        _ => return Err(Fallo::uso("usage: vivac note [<id>] \"<note>\"")),
    };
    guardar_texto(&[("note", nota)])?;
    ctx.emitir(vec![Cuerpo::NodoAnotado {
        nodo: n.id.clone(),
        nota: nota.to_string(),
    }])?;
    println!("  {} noted", n.alias());
    Ok(())
}

pub fn block(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac block <id> [--off]"))?;
    let n = ctx.resolver(s)?.clone();
    let Some(padre) = n.padre.as_ref().and_then(|p| ctx.arbol.nodo(p)) else {
        return Err(Fallo::uso(format!(
            "{} is the root: there is no parent to block.",
            n.alias()
        )));
    };
    let bloquea = !a.tiene("off");
    let (pa, pt) = (padre.alias(), padre.titulo.clone());
    ctx.emitir(vec![Cuerpo::BloqueoCambiado {
        nodo: n.id.clone(),
        bloquea,
    }])?;
    let verbo = if bloquea { "bloquea" } else { "ya no bloquea" };
    println!("  {} {} the close of {pa}  {pt}", n.alias(), verbo);
    Ok(())
}

/// `promote` — the focus becomes a goal of its own and the stack is cut there.
///
/// The provenance chain is **kept**: where it was born does not change just
/// because its rank did. Without this operation, the depth warning has no way
/// out and ends up being ignored.
pub fn promote(ctx: &mut Ctx, a: &Args) -> R {
    let n = match a.libre(0) {
        Some(s) => ctx.resolver(s)?.clone(),
        None => ctx
            .arbol
            .foco()
            .ok_or_else(|| Fallo::uso("usage: vivac promote [<id>]"))?
            .clone(),
    };
    ctx.emitir(vec![Cuerpo::Promovido { nodo: n.id.clone() }])?;
    println!("  {}  {}  -> a goal of its own", n.alias(), n.titulo);
    if let Some(p) = n.padre.as_ref().and_then(|p| ctx.arbol.nodo(p)) {
        println!("        still born from {}  {}", p.alias(), p.titulo);
    }
    Ok(())
}

/// `abandon` — discard. It costs the same as `pop` on purpose: if abandoning
/// were dearer than ignoring, nobody would abandon and in three months the
/// tree would be noise.
///
/// The cascade is **not** the default. `MODEL.md` §6 wants it with a
/// confirmation and the list up front, and a non-interactive CLI cannot
/// confirm anything: it shows what would fall and asks for an explicit
///
/// **Rescue does not reparent** (`d33`). `MODEL.md` §6 said to re-parent the
/// descendant onto a living ancestor; that rewrites the birth, and invariant
/// 11 says a thing is born in one place. A rescued node stays where it was
/// born: alive, under an abandoned parent. It is the same shape as an open
/// finding under a closed batch, which the tree already knows how to show and
/// the brief already knows how to count.
pub fn abandon(ctx: &mut Ctx, a: &Args) -> R {
    let n = match a.libre(0).and_then(|s| ctx.arbol.resolver(s)) {
        Some(n) => n.clone(),
        None => ctx
            .arbol
            .foco()
            .ok_or_else(|| Fallo::uso("usage: vivac abandon [<id>] \"<reason>\""))?
            .clone(),
    };
    let motivo = a
        .libres
        .iter()
        .rev()
        .find(|s| ctx.arbol.resolver(s).is_none());
    let motivo = motivo.map(|s| s.as_str()).unwrap_or("");
    guardar_texto(&[("reason", motivo)])?;

    // Rescuing a node rescues its descendants. Saving the parent and letting
    // the children die would be a half rescue nobody asked for, and would
    // orphan exactly what was meant to be kept.
    let mut rescatados: std::collections::HashSet<String> = Default::default();
    for s in a.lista("rescue") {
        let r = ctx
            .arbol
            .resolver(&s)
            .ok_or_else(|| Fallo::uso(format!("no such node: {s}")))?;
        let (rid, ralias) = (r.id.clone(), r.alias());
        if rid == n.id {
            return Err(Fallo::uso(format!(
                "{ralias} is the one being abandoned; it cannot be rescued from itself"
            )));
        }
        if !ctx.arbol.descendientes(&n.id).iter().any(|d| d.id == rid) {
            return Err(Fallo::uso(format!(
                "{ralias} does not hang off {}: there is nothing to rescue it from",
                n.alias()
            )));
        }
        rescatados.insert(rid.clone());
        for d in ctx.arbol.descendientes(&rid) {
            rescatados.insert(d.id.clone());
        }
    }

    let (caen, salvados): (Vec<&Nodo>, Vec<&Nodo>) = ctx
        .arbol
        .descendientes(&n.id)
        .into_iter()
        .filter(|d| d.estado.abierto())
        .partition(|d| !rescatados.contains(&d.id));

    // Only what falls unnamed needs confirming. If everything was rescued,
    // there is nothing left to confirm.
    if !caen.is_empty() && !a.tiene("cascade") {
        let mut m = format!(
            "  {}  {}\n  has {} open descendant(s) with no rescue:\n",
            n.alias(),
            n.titulo,
            caen.len()
        );
        for d in &caen {
            m.push_str(&format!("\n      {:<6} {}", d.alias(), d.titulo));
        }
        m.push_str("\n\n  Abandon all of it:     vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --cascade");
        m.push_str("\n  Save some of it:       vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --rescue <id>");
        m.push_str("\n  Save it as a goal:     vivac promote <id>");
        return Err(Fallo::Modelo(m));
    }

    let mut evs = vec![Cuerpo::EstadoCambiado {
        nodo: n.id.clone(),
        estado: Estado::Abandoned,
        resultado: motivo.to_string(),
        forzado: false,
    }];
    let cuantos_caen = caen.len();
    let salvados_dice: Vec<(String, String)> = salvados
        .iter()
        .map(|d| (d.alias(), d.titulo.clone()))
        .collect();
    for d in caen {
        evs.push(Cuerpo::EstadoCambiado {
            nodo: d.id.clone(),
            estado: Estado::Abandoned,
            resultado: format!("cascaded from {}", n.alias()),
            forzado: false,
        });
    }
    // The stack is the path to the focus and cannot cross an abandoned node,
    // so everything hanging off the abandoned one leaves it --the rescued
    // included, which stays alive but stops being on the path--.
    let mut fuera: Vec<String> = vec![n.id.clone()];
    fuera.extend(ctx.arbol.descendientes(&n.id).iter().map(|d| d.id.clone()));
    for id in fuera {
        if ctx.arbol.pila.contains(&id) {
            evs.push(Cuerpo::Desapilado { nodo: id });
        }
    }

    ctx.emitir(evs)?;
    println!("  {}  {}  -> abandoned", n.alias(), n.titulo);
    if cuantos_caen > 0 {
        println!("        and {cuantos_caen} descendant(s) with it");
    }
    if !salvados_dice.is_empty() {
        println!();
        println!("  Rescued, and still born from {}:", n.alias());
        for (alias, titulo) in &salvados_dice {
            println!("      {alias:<6} {titulo}");
        }
        println!();
        println!("  Their lineage crosses an abandoned node on purpose: where they");
        println!("  were born does not change because it got discarded.");
    }
    Ok(())
}

/// `focus` — step back into a node that already exists.
///
/// Without this the stack only works inside one session: the next day the log
/// holds the whole tree and the stack is empty, and there is no way to say "I
/// am on this" without opening a new node, which is exactly the litter to be
/// avoided. The stack becomes the path from the root down to the node, which
/// is what working on it means.
pub fn focus(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac focus <id> [--reopen]"))?;
    let n = ctx.resolver(s)?.clone();

    if !n.estado.abierto() && !a.tiene("reopen") {
        // Parking says "maybe I will be back", so returning is the normal
        // operation and asks no permission. Closing claims something finished:
        // undoing that has to be deliberate.
        if n.estado != Estado::Suspended {
            return Err(Fallo::Modelo(format!(
                "  {} is {}. Going back into it undoes that claim.\n\n  \
                 If it really was not finished:  vivac focus {} --reopen",
                n.alias(),
                n.estado.palabra(n.tipo),
                n.num
            )));
        }
    }

    let camino: Vec<String> = ctx
        .arbol
        .ancestros(&n.id)
        .iter()
        .map(|p| p.id.clone())
        .collect();
    let mut evs: Vec<Cuerpo> = ctx
        .arbol
        .pila
        .iter()
        .filter(|id| !camino.contains(id))
        .map(|id| Cuerpo::Desapilado { nodo: id.clone() })
        .collect();
    if !n.estado.abierto() {
        evs.push(Cuerpo::EstadoCambiado {
            nodo: n.id.clone(),
            estado: Estado::Active,
            resultado: String::new(),
            forzado: false,
        });
    }
    for id in &camino {
        if !ctx.arbol.pila.contains(id) {
            evs.push(Cuerpo::Apilado { nodo: id.clone() });
        }
    }
    let revivido = !n.estado.abierto();
    ctx.emitir(evs)?;
    if revivido {
        println!("  {} is open again", n.alias());
    }
    crate::render::stack(&ctx.arbol, a)
}

/// `flag <id> <flag> --why <reason>` — raise or clear a flag.
///
/// The reason is **mandatory** when raising it. `BRIEF-SPEC.md` §10 tests it
/// as a contract: a flag with no reason informs nobody, it only adds noise to
/// the brief, and within a week they all get ignored.
pub fn flag(ctx: &mut Ctx, a: &Args) -> R {
    let (Some(sid), Some(sb)) = (a.libre(0), a.libre(1)) else {
        return Err(Fallo::uso(
            "usage: vivac flag <id> <flag> --why \"<reason>\"  |  --off\n\n  \
             Flags: suspect, review, stale",
        ));
    };
    let n = ctx.resolver(sid)?.clone();
    let bandera = Bandera::desde(sb).ok_or_else(|| {
        Fallo::uso(format!(
            "Unknown flag: {sb}. They are: {}",
            Bandera::TODAS
        ))
    })?;

    if a.tiene("off") {
        ctx.emitir(vec![Cuerpo::BanderaBajada {
            nodo: n.id.clone(),
            bandera,
        }])?;
        println!("  {}  is no longer {}", n.alias(), bandera.palabra());
        return Ok(());
    }
    let motivo = a.opt("why").ok_or_else(|| {
        Fallo::uso(
            "Missing --why. A flag with no reason informs nobody: in two weeks\n  \
             nobody will know what needed looking at, and they all get ignored.",
        )
    })?;
    guardar_texto(&[("reason", motivo)])?;
    ctx.emitir(vec![Cuerpo::BanderaAlzada {
        nodo: n.id.clone(),
        bandera,
        motivo: motivo.to_string(),
    }])?;
    println!("  {}  {}  -> {}", n.alias(), n.titulo, bandera.palabra());
    println!("        {motivo}");
    Ok(())
}

/// `decide` — record a decision.
///
/// The discarded alternatives are optional in the schema and mandatory in
/// practice: without them, in a month the agent proposes again what you
/// already rejected.
pub fn decide(ctx: &mut Ctx, a: &Args) -> R {
    let titulo = a.libre(0).ok_or_else(|| {
        Fallo::uso(
            "usage: vivac decide \"<title>\" --reason \"<r>\" [--alternative X] [--supersedes d9]",
        )
    })?;
    let razon = a.opt("reason").ok_or_else(|| {
        Fallo::uso("Missing --reason. A decision with no reason is a datum, not a decision.")
    })?;
    let alternativas = a.lista("alternative");
    let superada = match a.opt("supersedes") {
        Some(s) => Some(ctx.resolver(s)?.clone()),
        None => None,
    };

    let mut cuerpo = razon.to_string();
    if !alternativas.is_empty() {
        cuerpo.push_str(&format!("  |  discarded: {}", alternativas.join("; ")));
    }
    let padre = ctx.arbol.foco().map(|n| n.id.clone());
    let (ev, num, _) = nacer(ctx, titulo, &cuerpo, Tipo::Decision, padre, a)?;

    let mut evs = vec![ev];
    if let Some(v) = &superada {
        // `supersedes` forms a chain: the old one becomes superseded, not deleted.
        evs.push(Cuerpo::EstadoCambiado {
            nodo: v.id.clone(),
            estado: Estado::Superseded,
            resultado: format!("superseded by d{num}"),
            forzado: false,
        });
    }
    ctx.emitir(evs)?;
    println!("  d{num}  {titulo}");
    if let Some(v) = superada {
        println!("        {} becomes superseded", v.alias());
    }
    if alternativas.is_empty() {
        println!("        no alternatives recorded: in a month they get proposed again");
    }
    Ok(())
}

/// `save [label]` — a safe stop on purpose.
pub fn save(ctx: &mut Ctx, a: &Args) -> R {
    let etiqueta = a.libre(0).unwrap_or("");
    let luego = a.opt_o("next");
    guardar_texto(&[("label", etiqueta), ("next", &luego)])?;
    let v = vivac(ctx, VivacKind::Manual, &luego, None, etiqueta);
    let num = ctx.arbol.siguiente_vivac.max(1);
    ctx.emitir(vec![v])?;
    let anclaje = ctx.anchor.snapshot();
    println!(
        "  v{num}  {}",
        if etiqueta.is_empty() {
            "no label"
        } else {
            etiqueta
        }
    );
    if !anclaje.vacio() {
        println!("        anchored to {}", anclaje.corto());
    } else {
        // With no VCS no precision is faked: the vivac is worth the same, but
        // restoring it will only give plain age, not a diff.
        println!("        no anchor: there is no version control here");
    }
    if luego.is_empty() {
        println!("        no --next: coming back there will be nothing to pick up");
    }
    Ok(())
}

/// `restore <v>` — go back to a vivac.
///
/// **It never touches the working tree.** Mixing context navigation with tree
/// manipulation turns a tool for attention into a branch manager worse than
/// git. It rebuilds the stack and presents the diff.
pub fn restore(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac restore <v>"))?;
    let v = ctx
        .arbol
        .vivac(s)
        .ok_or_else(|| Fallo::uso(format!("No such vivac: {s}.")))?
        .clone();

    // The vivac's stack is frozen by alias. Nodes that no longer exist or are
    // closed get skipped and named: restoring resurrects nothing.
    let mut camino = Vec::new();
    let mut perdidos = Vec::new();
    for (alias, titulo) in &v.pila {
        match ctx.arbol.resolver(alias) {
            Some(n) if n.estado.abierto() => camino.push(n.id.clone()),
            Some(n) => perdidos.push(format!("{alias} {titulo} [{}]", n.estado.palabra(n.tipo))),
            None => perdidos.push(format!("{alias} {titulo} [gone]")),
        }
    }
    let mut evs: Vec<Cuerpo> = ctx
        .arbol
        .pila
        .iter()
        .filter(|id| !camino.contains(id))
        .map(|id| Cuerpo::Desapilado { nodo: id.clone() })
        .collect();
    for id in &camino {
        if !ctx.arbol.pila.contains(id) {
            evs.push(Cuerpo::Apilado { nodo: id.clone() });
        }
    }
    let cambios = ctx.anchor.changed_since(&v.anchor);
    ctx.emitir(evs)?;

    println!();
    println!(
        "  {} · {} · {}",
        v.alias(),
        v.kind.palabra(),
        crate::clock::date_of(&v.ts)
    );
    if !v.etiqueta.is_empty() {
        println!("  {}", v.etiqueta);
    }
    println!();
    if !v.next_intent.is_empty() {
        println!("  you were about to:  {}", v.next_intent);
        println!();
    }
    for p in &perdidos {
        println!("  no longer on the stack:  {p}");
    }
    if !perdidos.is_empty() {
        println!();
    }
    if v.anchor.vacio() {
        println!("  No anchor: there is no diff to show, only the date above.");
        println!();
    } else if cambios.is_empty() {
        println!("  Nothing changed since {}.", v.anchor.corto());
        println!();
    } else {
        let tocan: Vec<&crate::anchor::Cambio> = cambios
            .iter()
            .filter(|c| v.working_set.iter().any(|g| crate::glob::cubre(g, &c.ruta)))
            .collect();
        println!(
            "  {} changes since {}{}",
            cambios.len(),
            v.anchor.corto(),
            if v.working_set.is_empty() {
                String::new()
            } else {
                format!(", {} of them touch what the stack governed", tocan.len())
            }
        );
        for c in cambios.iter().take(6) {
            println!("      {:<52} ({})", c.ruta, c.veces);
        }
        if cambios.len() > 6 {
            println!("      ... and {} more", cambios.len() - 6);
        }
        println!();
    }
    crate::render::stack(&ctx.arbol, a)
}

/// An automatic stop, for the end-of-session hook.
pub fn vivac_auto(ctx: &mut Ctx, kind: VivacKind, luego: &str) -> R {
    guardar_texto(&[("next", luego)])?;
    let v = vivac(ctx, kind, luego, None, "");
    ctx.emitir(vec![v])
}
