//! Lo que lee el mantenedor.
//!
//! Todo en ASCII y sin una sola secuencia de color. El pilar de DX es
//! explicito: **el significado nunca se codifica solo en color**, y esto tiene
//! que degradar sin romperse --sin tty, por ssh, y en cmd.exe ademas de
//! Windows Terminal--. `[x]`, `[~]`, `*` y `<== CIERRE FALSO` se leen en
//! blanco y negro. El color, cuando llegue, refuerza; no informa.
//!
//! Cada render tiene su gemelo en `--json`, que es la otra mitad de la
//! audiencia: el agente necesita salida parseable, no un arbol dibujado.

use crate::args::Args;
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
        "tipo": n.tipo,
        "titulo": n.titulo,
        "por": n.por,
        "estado": n.estado,
        "bloquea": n.bloquea,
        "padre": n.padre.as_ref().and_then(|p| a.nodo(p).map(|x| x.alias())),
        "nota": n.nota,
        "resultado": n.resultado,
        "refs": n.refs,
        "governs": n.governs,
        "abierto": n.abierto,
        "cerrado": n.cerrado,
        "cierre_falso": n.estado == Estado::Done && ag.bloqueantes(&n.id) > 0,
        "abiertos_debajo": r.abiertos,
        "total_debajo": r.total,
    })
}

fn imprimir_json(v: serde_json::Value) -> R {
    println!(
        "{}",
        serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?
    );
    Ok(())
}

/// `why` — por que estamos aca. Es la operacion que define el producto.
///
/// Narra el camino desde la raiz y despues contesta las tres preguntas que
/// vienen detras: que quedo en paralelo sin cerrar, que nacio de aca, y que
/// impide cerrar cada escalon del camino.
pub fn why(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let s = args
        .libre(0)
        .ok_or_else(|| Fallo::uso("uso: vivac why <id>"))?;
    let n = a
        .resolver(s)
        .ok_or_else(|| Fallo::uso(format!("No existe el nodo {s}.")))?;
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
            "nodo": json_nodo(a, ag, n),
            "camino": camino.iter().map(|x| json_nodo(a, ag, x)).collect::<Vec<_>>(),
            "en_paralelo": hermanos,
            "nacio_de_aca": a.hijos(&n.id).iter().filter(|c| c.estado.abierto())
                .map(|c| json_nodo(a, ag, c)).collect::<Vec<_>>(),
            "bloqueantes": a.bloqueantes_abiertos(&n.id).iter()
                .map(|c| json_nodo(a, ag, c)).collect::<Vec<_>>(),
        }));
    }

    println!();
    println!("  Por que estamos aca  ->  {}", n.alias());
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
                println!("        ({f} por debajo)");
            }
            println!("        |");
            println!("        v");
        } else {
            println!();
            println!("        ^^^ estas aca");
        }
    }
    println!();

    // "teniamos que revisar estas diez cosas, vamos por la primera"
    if let Some(padre) = n.padre.as_ref() {
        let hermanos: Vec<_> = a
            .hijos(padre)
            .into_iter()
            .filter(|c| c.id != n.id && c.estado.abierto())
            .collect();
        if !hermanos.is_empty() {
            println!("  En paralelo, sin cerrar ({}):", hermanos.len());
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
        println!("  Nacio de aca y sigue abierto ({}):", kids.len());
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
                "  {} no cierra hasta que cierren ({}):",
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
            "   <== CIERRE FALSO: {pend} condicion(es) abierta(s)"
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
    v["hijos"] = json!(a
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
            .ok_or_else(|| Fallo::uso(format!("No existe el nodo {s}.")))?],
        None => a.raices(),
    };
    if args.tiene("json") {
        return imprimir_json(json!(raices
            .iter()
            .map(|n| subarbol_json(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if a.vacio() {
        println!("  Arbol vacio.  vivac push \"<titulo>\" --por \"<motivo>\"");
        return Ok(());
    }
    let todo = args.tiene("todo") || args.tiene("all");
    println!();
    for (i, n) in raices.iter().enumerate() {
        rama(a, ag, n, "  ", i == raices.len() - 1, todo);
    }
    println!();
    if !todo {
        println!("  (cerrados sin descendencia abierta ocultos; --todo los muestra)");
        println!();
    }
    Ok(())
}

/// `open` — los frentes abiertos, cada uno con su linaje comprimido. Es la
/// vista de "por donde iba" al empezar el dia.
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
                v["linaje"] = json!(a
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
        println!("  Nada abierto.");
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
    // No son frentes, pero desaparecerlas de aqui sin decirlo seria omitir
    // en silencio: se cuentan y se dice donde estan.
    if vigentes > 0 {
        let frase = if vigentes == 1 {
            "1 decision vigente, que no se hace".to_string()
        } else {
            format!("{vigentes} decisiones vigentes, que no se hacen")
        };
        println!();
        println!("  + {frase}   vivac brief");
    }
    println!();
    Ok(())
}

/// `parked` — NO TOCAR AHORA. Es la seccion que ninguna otra herramienta
/// emite: toda herramienta de memoria vuelca lo relevante, y el problema del
/// desarrollo agentico es el contrario, acotar.
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
        println!("  Nada aparcado.");
        return Ok(());
    }
    println!();
    println!("  NO TOCAR AHORA ({})", ps.len());
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

/// `stack` — donde estas ahora mismo, de la raiz al foco.
pub fn stack(a: &Arbol, args: &Args) -> R {
    let ag = &a.agregados();
    let pila: Vec<&Nodo> = a.pila.iter().filter_map(|id| a.nodo(id)).collect();
    if args.tiene("json") {
        return imprimir_json(json!({
            "profundidad": pila.len(),
            "pila": pila.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    if pila.is_empty() {
        println!("  Pila vacia.  vivac push \"<titulo>\" --por \"<motivo>\"");
        return Ok(());
    }
    println!();
    for (i, n) in pila.iter().enumerate() {
        let foco = if i == pila.len() - 1 {
            "   <- foco"
        } else {
            ""
        };
        println!("  {}{:<6} {}{foco}", "  ".repeat(i), n.alias(), n.titulo);
    }
    println!();
    if pila.len() >= 6 {
        println!(
            "  Pila a {} niveles. Casi nunca es indisciplina: suele ser que",
            pila.len()
        );
        println!("  el objetivo raiz cambio y nadie volvio a enraizar.  vivac promote");
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
            "nodos": a.total(),
            "por_estado": por_estado,
            "profundidad": hondo,
            "raices": a.raices().len(),
            "pila": a.profundidad_pila(),
            "huerfanos": huerfanos,
            "lineas_rotas": a.lineas_rotas,
            "cierres_falsos": falsos.iter().map(|n| json_nodo(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    println!();
    println!("  nodos          {}", a.total());
    for (k, v) in &por_estado {
        println!("  {k:<14} {v}");
    }
    println!("  profundidad    {hondo}");
    println!("  raices         {}", a.raices().len());
    println!("  pila           {}", a.profundidad_pila());
    if huerfanos > 0 {
        println!("  HUERFANOS      {huerfanos}  <- procedencia rota");
    }
    if a.lineas_rotas > 0 {
        println!("  lineas rotas   {}  <- en .vivac/events", a.lineas_rotas);
    }
    if !falsos.is_empty() {
        println!();
        println!("  CIERRES FALSOS ({})", falsos.len());
        for n in falsos {
            println!("      {:<6} {}", n.alias(), n.titulo);
        }
    }
    println!();
    Ok(())
}

/// `vivacs` — las paradas seguras, de la ultima hacia atras.
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
                "etiqueta": v.etiqueta,
                "next_intent": v.next_intent,
                "anchor": v.anchor,
                "pila": v.pila.iter().map(|(al, t)| json!({"alias": al, "titulo": t}))
                    .collect::<Vec<_>>(),
                "working_set": v.working_set,
            }))
            .collect::<Vec<_>>()));
    }
    if a.vivacs.is_empty() {
        println!("  Ninguna parada todavia.  vivac save \"<etiqueta>\"");
        return Ok(());
    }
    println!();
    for v in a.vivacs.iter().rev().take(20) {
        let cima = v
            .pila
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "pila vacia".into());
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
            println!("           ibas a: {}", v.next_intent);
        }
    }
    if a.vivacs.len() > 20 {
        println!();
        println!("  ... y {} mas", a.vivacs.len() - 20);
    }
    println!();
    Ok(())
}
