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
use crate::brief::corta;
use crate::event::{Estado, Tipo};
use crate::fallo::{Fallo, R};
use crate::model::{Agregados, Arbol, Nodo};
use serde_json::json;

const ANCHO: usize = 62;

fn envolver(texto: &str, ancho: usize, sangria: &str) -> Vec<String> {
    if texto.trim().is_empty() {
        return vec![];
    }
    let mut lineas = Vec::new();
    let mut cur = String::new();
    for p in texto.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + p.chars().count() > ancho {
            lineas.push(format!("{sangria}{cur}"));
            cur = p.to_string();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(p);
        }
    }
    if !cur.is_empty() {
        lineas.push(format!("{sangria}{cur}"));
    }
    lineas
}

fn etiqueta(n: &Nodo) -> String {
    match n.estado {
        Estado::Active => n.titulo.clone(),
        e => format!("{}  [{}]", n.titulo, e.palabra(n.tipo)),
    }
}

fn json_nodo(a: &Arbol, ag: &Agregados, n: &Nodo) -> serde_json::Value {
    let r = ag.recuento(&n.id);
    json!({
        "id": n.id,
        "alias": n.alias(),
        "num": n.num,
        "type": n.tipo,
        "title": n.titulo,
        "why": n.por,
        "state": n.estado,
        "blocks": n.bloquea,
        "parent": n.padre.as_ref().and_then(|p| a.nodo(p).map(|x| x.alias())),
        "note": n.nota,
        "outcome": n.resultado,
        "refs": n.refs,
        "governs": n.governs,
        "opened": n.abierto,
        "closed": n.cerrado,
        "false_close": n.estado == Estado::Done && ag.bloqueantes(&n.id) > 0,
        "open_below": r.abiertos,
        "total_below": r.total,
    })
}

fn imprimir_json(v: serde_json::Value) -> R {
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
pub fn why(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let s = args
        .libre(0)
        .ok_or_else(|| Fallo::uso("usage: vivac why <id>"))?;
    let n = a
        .resolver(s)
        .ok_or_else(|| Fallo::uso(format!("No such node: {s}.")))?;
    let camino = a.ancestros(&n.id);

    if args.tiene("json") {
        let hermanos: Vec<_> = n
            .padre
            .as_ref()
            .map(|p| a.hijos(p))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.id != n.id && c.estado.abierto())
            .map(|c| json_nodo(a, ag, c))
            .collect();
        return imprimir_json(json!({
            "node": json_nodo(a, ag, n),
            "path": camino.iter().map(|x| json_nodo(a, ag, x)).collect::<Vec<_>>(),
            "en_paralelo": hermanos,
            "born_here": a.hijos(&n.id).iter().filter(|c| c.estado.abierto())
                .map(|c| json_nodo(a, ag, c)).collect::<Vec<_>>(),
            "blockers": a.bloqueantes_abiertos(&n.id).iter()
                .map(|c| json_nodo(a, ag, c)).collect::<Vec<_>>(),
        }));
    }

    println!();
    println!("  Why we are here  ->  {}", n.alias());
    println!("  {}", "-".repeat(66));
    println!();
    for (i, p) in camino.iter().enumerate() {
        let ultimo = i == camino.len() - 1;
        println!("  {:<6}{}", p.alias(), etiqueta(p));
        for l in envolver(&p.por, ANCHO, "        ") {
            println!("{l}");
        }
        for l in envolver(&format!("! {}", p.nota), ANCHO, "        ") {
            if !p.nota.is_empty() {
                println!("{l}");
            }
        }
        for l in envolver(&format!("= {}", p.resultado), ANCHO, "        ") {
            if !p.resultado.is_empty() {
                println!("{l}");
            }
        }
        if !ultimo {
            let f = ag.recuento(&p.id).frase();
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
    if let Some(padre) = n.padre.as_ref() {
        let hermanos: Vec<_> = a
            .hijos(padre)
            .into_iter()
            .filter(|c| c.id != n.id && c.estado.abierto())
            .collect();
        if !hermanos.is_empty() {
            println!("  In parallel, still open ({}):", hermanos.len());
            for c in hermanos {
                println!("      {:<6} {}", c.alias(), c.titulo);
            }
            println!();
        }
    }

    let kids: Vec<_> = a
        .hijos(&n.id)
        .into_iter()
        .filter(|c| c.estado.abierto())
        .collect();
    if !kids.is_empty() {
        println!("  Born here and still open ({}):", kids.len());
        for c in kids {
            println!(
                "    {} {:<6} {}",
                if c.bloquea { '*' } else { ' ' },
                c.alias(),
                c.titulo
            );
        }
        println!();
    }

    for p in &camino {
        let pend = a.bloqueantes_abiertos(&p.id);
        if !pend.is_empty() && p.estado.abierto() {
            println!(
                "  {} does not close until these close ({}):",
                p.alias(),
                pend.len()
            );
            for c in pend {
                println!("      {:<6} {}", c.alias(), c.titulo);
            }
            println!();
        }
    }
    Ok(())
}

fn rama(a: &Arbol, ag: &Agregados, n: &Nodo, prefijo: &str, ultimo: bool, todo: bool) {
    let f = ag.recuento(&n.id).frase();
    let mut cola = if f.is_empty() {
        String::new()
    } else {
        format!("   ({f})")
    };
    let pend = ag.bloqueantes(&n.id);
    if n.estado == Estado::Done && pend > 0 {
        cola.push_str(&format!(
            "   <== FALSE CLOSE: {pend} open condition(s)"
        ));
    }
    let marca = if n.bloquea { "* " } else { "" };
    println!(
        "{prefijo}{}[{}] {:<6} {marca}{}{cola}",
        if ultimo { "`-- " } else { "|-- " },
        n.estado.marca(),
        n.alias(),
        n.titulo
    );
    let sig = format!("{prefijo}{}", if ultimo { "    " } else { "|   " });
    let hijos: Vec<_> = a
        .hijos(&n.id)
        .into_iter()
        .filter(|h| todo || h.estado.abierto() || ag.recuento(&h.id).abiertos > 0)
        .collect();
    for (i, h) in hijos.iter().enumerate() {
        rama(a, ag, h, &sig, i == hijos.len() - 1, todo);
    }
}

fn subarbol_json(a: &Arbol, ag: &Agregados, n: &Nodo) -> serde_json::Value {
    let mut v = json_nodo(a, ag, n);
    v["children"] = json!(a
        .hijos(&n.id)
        .iter()
        .map(|h| subarbol_json(a, ag, h))
        .collect::<Vec<_>>());
    v
}

pub fn tree(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let raices: Vec<&Nodo> = match args.libre(0) {
        Some(s) => vec![a
            .resolver(s)
            .ok_or_else(|| Fallo::uso(format!("No such node: {s}.")))?],
        None => a.raices(),
    };
    if args.tiene("json") {
        return imprimir_json(json!(raices
            .iter()
            .map(|n| subarbol_json(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if a.vacio() {
        println!("  Empty tree.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    let todo = args.tiene("all") || args.tiene("all");
    println!();
    for (i, n) in raices.iter().enumerate() {
        rama(a, ag, n, "  ", i == raices.len() - 1, todo);
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
pub fn open(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let mut hojas: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.es_frente() && !a.hijos(&n.id).iter().any(|c| c.es_frente()))
        .collect();
    hojas.sort_by_key(|n| n.num);
    let vigentes = a
        .todos()
        .filter(|n| n.tipo == Tipo::Decision && n.estado.abierto())
        .count();
    if args.tiene("json") {
        return imprimir_json(json!(hojas
            .iter()
            .map(|n| {
                let mut v = json_nodo(a, ag, n);
                v["lineage"] = json!(a
                    .ancestros(&n.id)
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
    if hojas.is_empty() && vigentes == 0 {
        println!("  Nothing open.");
        return Ok(());
    }
    println!();
    println!(
        "  {} frente{} abierto{}",
        hojas.len(),
        if hojas.len() == 1 { "" } else { "s" },
        if hojas.len() == 1 { "" } else { "s" }
    );
    println!();
    for n in hojas {
        println!("  {:<6} {}", n.alias(), n.titulo);
        let camino = a.ancestros(&n.id);
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
    if vigentes > 0 {
        let frase = if vigentes == 1 {
            "1 standing decision, which is not work".to_string()
        } else {
            format!("{vigentes} standing decisions, which are not work")
        };
        println!();
        println!("  + {frase}   vivac brief");
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
pub fn triage(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();

    let mut aparcados: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.estado == Estado::Suspended)
        .collect();

    // `MODEL.md` §6.1: from 6 on it shows up here, and it never blocks.
    let mut hondos: Vec<(&Nodo, usize)> = a
        .todos()
        .filter(|n| n.es_frente())
        .map(|n| (n, a.ancestros(&n.id).len()))
        .filter(|(_, d)| *d >= 6)
        .collect();

    // Alive, hanging off something discarded. `abandon`'s rescue produces
    // them, and it does **not** reparent on purpose (`d33`): the node stays
    // where it was born. That is why they need revisiting now and then, and
    // why they are here and not in `check`: it is not store corruption, it is
    // work that lost the reason it was born for.
    let mut descolgados: Vec<(&Nodo, &Nodo)> = a
        .todos()
        .filter(|n| n.es_frente())
        .filter_map(|n| {
            let p = a.nodo(n.padre.as_deref()?)?;
            (p.estado == Estado::Abandoned).then_some((n, p))
        })
        .collect();

    // Invariant 10. `check` reports them for CI; here they get acted on, and
    // with the same exemption: a **forced** close was a decision, it has its
    // trace and the tree marks it. Repeating it here every day would be asking
    // for what was already decided to be decided again. What does land here is
    // the close that turned false later, when a blocker got hung on something
    // already closed: that is the case that took 26 days to spot.
    let mut falsos: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.estado == Estado::Done && !n.cierre_forzado && ag.bloqueantes(&n.id) > 0)
        .collect();

    aparcados.sort_by_key(|n| n.num);
    hondos.sort_by_key(|(n, _)| n.num);
    descolgados.sort_by_key(|(n, _)| n.num);
    falsos.sort_by_key(|n| n.num);

    if args.tiene("json") {
        return imprimir_json(json!({
            "parked": aparcados.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
            "deep": hondos.iter().map(|(n, d)| {
                let mut v = json_nodo(a, ag, n);
                v["depth"] = json!(d);
                v
            }).collect::<Vec<_>>(),
            "orphaned_by_discard": descolgados.iter().map(|(n, p)| {
                let mut v = json_nodo(a, ag, n);
                v["discarded"] = json!(p.alias());
                v["discarded_because"] = json!(p.resultado);
                v
            }).collect::<Vec<_>>(),
            "false_closes": falsos.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
        }));
    }

    let total = aparcados.len() + hondos.len() + descolgados.len() + falsos.len();
    if total == 0 {
        println!("  Nothing to prune.");
        return Ok(());
    }
    println!();
    println!("  TRIAGE - {total} thing(s) to look at");

    if !aparcados.is_empty() {
        println!();
        println!(
            "  PARKED ({})                       focus <id>  |  abandon <id>",
            aparcados.len()
        );
        for n in &aparcados {
            println!("    {:<6} {}", n.alias(), n.titulo);
            for l in envolver(&n.resultado, ANCHO, "           ") {
                println!("{l}");
            }
        }
    }

    if !hondos.is_empty() {
        println!();
        println!(
            "  6 OR MORE FROM THE ROOT ({})      promote <id>",
            hondos.len()
        );
        for (n, d) in &hondos {
            println!(
                "    {:<6} {:<40} depth {d}",
                n.alias(),
                corta(&n.titulo, 40)
            );
            let v: Vec<String> = a
                .ancestros(&n.id)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .map(|p| p.alias())
                .collect();
            println!("           via {}", v.join(" > "));
        }
    }

    if !descolgados.is_empty() {
        println!();
        println!(
            "  SURVIVED A DISCARD ({})           abandon <id>  |  promote <id>",
            descolgados.len()
        );
        for (n, p) in &descolgados {
            println!("    {:<6} {}", n.alias(), n.titulo);
            println!(
                "           born from {}, discarded: {}",
                p.alias(),
                corta(&p.resultado, 36)
            );
        }
    }

    if !falsos.is_empty() {
        println!();
        println!(
            "  FALSE CLOSES ({})                 close what is left, or --force",
            falsos.len()
        );
        for n in &falsos {
            println!(
                "    {:<6} {:<40} {} blocker(s)",
                n.alias(),
                corta(&n.titulo, 40),
                ag.bloqueantes(&n.id)
            );
        }
    }
    println!();
    Ok(())
}

/// `parked` — DO NOT TOUCH NOW. It is the section no other tool emits: every
/// memory tool dumps what is relevant, and the problem in agentic development
/// is the opposite one, bounding.
pub fn parked(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let mut ps: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.estado == Estado::Suspended)
        .collect();
    ps.sort_by_key(|n| n.num);
    if args.tiene("json") {
        return imprimir_json(json!(ps
            .iter()
            .map(|n| json_nodo(a, ag, n))
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
        println!("  {:<6} {}", n.alias(), n.titulo);
        for l in envolver(&n.resultado, ANCHO, "         ") {
            println!("{l}");
        }
    }
    println!();
    Ok(())
}

/// `stack` — where you are right now, from the root to the focus.
pub fn stack(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let pila: Vec<&Nodo> = a.pila.iter().filter_map(|id| a.nodo(id)).collect();
    if args.tiene("json") {
        return imprimir_json(json!({
            "depth": pila.len(),
            "stack": pila.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    if pila.is_empty() {
        println!("  Empty stack.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    println!();
    for (i, n) in pila.iter().enumerate() {
        let foco = if i == pila.len() - 1 {
            "   <- focus"
        } else {
            ""
        };
        println!("  {}{:<6} {}{foco}", "  ".repeat(i), n.alias(), n.titulo);
    }
    println!();
    if pila.len() >= 6 {
        println!(
            "  Stack {} levels deep. Almost never lack of discipline: usually",
            pila.len()
        );
        println!("  the root goal moved and nobody re-rooted.  vivac promote");
        println!();
    }
    Ok(())
}

pub fn stats(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let mut por_estado = std::collections::BTreeMap::new();
    let mut huerfanos = 0usize;
    let mut falsos = Vec::new();
    for n in a.todos() {
        *por_estado.entry(n.estado.palabra(n.tipo)).or_insert(0usize) += 1;
        if n.padre.as_ref().is_some_and(|p| a.nodo(p).is_none()) {
            huerfanos += 1;
        }
        if n.estado == Estado::Done && ag.bloqueantes(&n.id) > 0 {
            falsos.push(n);
        }
    }
    let hondo = ag.profundidad_max;
    falsos.sort_by_key(|n| n.num);
    if args.tiene("json") {
        return imprimir_json(json!({
            "nodes": a.total(),
            "by_state": por_estado,
            "depth": hondo,
            "roots": a.raices().len(),
            "stack": a.profundidad_pila(),
            "orphans": huerfanos,
            "broken_lines": a.lineas_rotas,
            "false_closes": falsos.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    println!();
    println!("  nodes          {}", a.total());
    for (k, v) in &por_estado {
        println!("  {k:<14} {v}");
    }
    println!("  depth          {hondo}");
    println!("  roots          {}", a.raices().len());
    println!("  stack          {}", a.profundidad_pila());
    if huerfanos > 0 {
        println!("  ORPHANS        {huerfanos}  <- broken provenance");
    }
    if a.lineas_rotas > 0 {
        println!("  broken lines   {}  <- in .vivac/events", a.lineas_rotas);
    }
    if !falsos.is_empty() {
        println!();
        println!("  FALSE CLOSES ({})", falsos.len());
        for n in falsos {
            println!("      {:<6} {}", n.alias(), n.titulo);
        }
    }
    println!();
    Ok(())
}

/// `vivacs` — the safe stops, latest first.
pub fn vivacs(a: &Arbol, args: &Args) -> R {
    if args.tiene("json") {
        return imprimir_json(json!(a
            .vivacs
            .iter()
            .rev()
            .map(|v| json!({
                "id": v.id,
                "alias": v.alias(),
                "node_ref": v.node_ref.as_ref().and_then(|r| a.nodo(r).map(|n| n.alias())),
                "kind": v.kind.palabra(),
                "ts": v.ts,
                "label": v.etiqueta,
                "next_intent": v.next_intent,
                "anchor": v.anchor,
                "stack": v.pila.iter().map(|(al, t)| json!({"alias": al, "title": t}))
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
            .pila
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "empty stack".into());
        println!(
            "  {:<5} {:<7} {}  {}",
            v.alias(),
            v.kind.palabra(),
            crate::clock::date_of(&v.ts),
            cima
        );
        if !v.etiqueta.is_empty() {
            println!("           {}", v.etiqueta);
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
