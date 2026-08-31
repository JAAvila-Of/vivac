//! The `brief`: a deterministic render bounded in tokens.
//!
//! `BRIEF-SPEC.md`. It answers three questions in order of importance: where
//! we are and how we got here, what governs this point, and **what is out of
//! scope right now**. The third is the one no other tool emits: every memory
//! tool dumps what is relevant, and the problem in agentic development is the
//! opposite one, bounding.
//!
//! Two rules override everything else:
//!
//! - **Same log + same `--now` + same anchor state -> same bytes.** Without
//!   `--now` determinism would be impossible, because ages are relative to
//!   the moment.
//! - **The spine is never truncated.** If it does not fit, the budget is
//!   wrong and it says so, but it comes out whole: it is the answer to
//!   question 1, and without it the brief has no reason to exist.

use crate::anchor::Anchor;
use crate::args::Args;
use crate::event::{Estado, Tipo};
use crate::fallo::R;
use crate::model::{Arbol, Nodo};

const PRESUPUESTO: usize = 1500;
/// The whole brief is pure ASCII.
///
/// `BRIEF-SPEC.md` §7 draws the spine with box-drawing characters, but the DX
/// pillar demands it degrade without breaking "in cmd.exe as well as Windows
/// Terminal", and there any code page that is not UTF-8 turns them into
/// garbage. What is normative in §7 are the markers --that the focus be
/// visible, that a flag carry its reason, that an empty section not show--
const RAYA: &str = "------------------------------------------------------------";

/// One section of the brief. The vector order is the one in §3, which is both
/// render order and priority order: truncation starts from the bottom.
struct Seccion {
    lineas: Vec<String>,
    truncable: bool,
}

impl Seccion {
    fn fija(lineas: Vec<String>) -> Seccion {
        Seccion {
            lineas,
            truncable: false,
        }
    }
    fn suelta(lineas: Vec<String>) -> Seccion {
        Seccion {
            lineas,
            truncable: true,
        }
    }
}

/// Token estimator. It is an estimate and the ceiling is indicative: what
/// matters is that it be **deterministic**, so two runs of the same log
/// truncate the same way.
fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn tokens_de(secciones: &[Seccion]) -> usize {
    secciones
        .iter()
        .flat_map(|s| s.lineas.iter())
        .map(|l| tokens(l) + 1)
        .sum()
}

/// Truncates a list keeping the first `n`. An item from the middle is never
/// dropped in silence.
fn recortar(mut v: Vec<String>, n: usize, que: &str) -> Vec<String> {
    if v.len() > n {
        let sobran = v.len() - n;
        v.truncate(n);
        v.push(format!("      ... and {sobran} more (vivac {que})"));
    }
    v
}

fn encabezado(titulo: &str, cuerpo: Vec<String>) -> Vec<String> {
    // Empty sections are omitted whole, heading included: a brief with nothing
    // parked does not say "DO NOT TOUCH NOW: (empty)".
    if cuerpo.is_empty() {
        return vec![];
    }
    let mut v = vec![String::new(), format!(" {titulo}")];
    v.extend(cuerpo);
    v
}

/// Constraints that govern the path.
///
/// **By `spawns` only.** Inheriting through `depends_on` as well would turn
/// the computation from O(depth) into O(graph), and would lose the property
/// that inheritance is legible by looking at the stack on screen.
fn constraints<'a>(a: &'a Arbol, camino: &[&Nodo]) -> Vec<&'a Nodo> {
    let en_camino: std::collections::HashSet<&str> = camino.iter().map(|n| n.id.as_str()).collect();
    let mut v: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.tipo == Tipo::Constraint && n.estado.abierto())
        .filter(|n| {
            // Project-wide (hangs off a root), or reachable from the path.
            let del_proyecto = n
                .padre
                .as_ref()
                .and_then(|p| a.nodo(p))
                .is_some_and(|p| p.padre.is_none());
            del_proyecto
                || a.ancestros(&n.id)
                    .iter()
                    .any(|p| en_camino.contains(p.id.as_str()))
        })
        .collect();
    // At risk first --the ones carrying a flag-- and then by alias.
    v.sort_by_key(|n| (n.banderas.is_empty(), n.num));
    v
}

fn espina(camino: &[&Nodo]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, n) in camino.iter().enumerate() {
        let primero = i == 0;
        let ultimo = i == camino.len() - 1;
        // Continuation: the trunk carries on while anything is left below.
        let sigue = if ultimo { "        " } else { "  |     " };

        let rama = if primero {
            " GOAL ".to_string()
        } else if ultimo {
            "  `-- ".to_string()
        } else {
            "  |-- ".to_string()
        };
        let banderas: Vec<&str> = n.banderas.keys().map(|b| b.palabra()).collect();
        let bandera = if banderas.is_empty() {
            String::new()
        } else {
            format!("  ! {}", banderas.join(" "))
        };
        let aqui = if ultimo { "   <== HERE" } else { "" };
        v.push(format!(
            "{rama}{:<6} {}{bandera}{aqui}",
            n.alias(),
            corta(&n.titulo, 44)
        ));
        if !primero && !n.por.is_empty() {
            v.push(format!("{sigue}why: {}", corta(&n.por, 52)));
        }
        if !n.governs.is_empty() {
            v.push(format!("{sigue}governs: {}", n.governs.join(" ")));
        }
        if !ultimo {
            v.push("  |".to_string());
        }
    }
    v
}

/// Cuts on a word boundary without exceeding `n`, **counting the ellipsis**.
/// Budgeting for it matters: otherwise the cut overruns on exactly the
/// tightest lines of the brief, which are the ones being truncated.
pub(crate) fn corta(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let t: String = s.chars().take(n.saturating_sub(3)).collect();
    match t.rsplit_once(' ') {
        Some((a, _)) if !a.is_empty() => format!("{a}..."),
        _ => format!("{t}..."),
    }
}

pub fn brief(a: &Arbol, ancla: &dyn Anchor, args: &Args, proyecto: &str) -> R {
    print!("{}", a_texto(a, ancla, args, proyecto)?);
    Ok(())
}

/// The brief as text. `session start --hook` needs it whole to put in the
/// envelope: whatever falls outside the envelope, the agent never sees.
pub fn a_texto(
    a: &Arbol,
    ancla: &dyn Anchor,
    args: &Args,
    proyecto: &str,
) -> Result<String, crate::fallo::Fallo> {
    let hoy = args.opt("now").unwrap_or("").to_string();
    let hoy = if hoy.is_empty() {
        crate::clock::now_rfc3339()
    } else {
        hoy
    };
    let fecha = crate::clock::date_of(&hoy).to_string();
    let presupuesto: usize = args
        .opt("budget")
        .and_then(|s| s.parse().ok())
        .unwrap_or(PRESUPUESTO);

    let camino: Vec<&Nodo> = match a.pila.last() {
        Some(id) => a.ancestros(id),
        None => vec![],
    };

    if camino.is_empty() {
        return sin_foco(a, proyecto, &fecha);
    }
    let foco = camino[camino.len() - 1];

    let mut s: Vec<Seccion> = Vec::new();

    // 1. Header. 2. Spine, which is never truncated.
    s.push(Seccion::fija(vec![
        format!("vivac · project: {proyecto} · lane: main · {fecha}"),
        RAYA.to_string(),
        String::new(),
    ]));
    s.push(Seccion::fija(espina(&camino)));

    // 3. Focus: what hangs off it unclosed. Standing decisions do not go in
    //    --they are not pending work and they have their own section (8)--,
    //    and whatever hangs further down is counted without being listed.
    let mut hijos: Vec<String> = a
        .hijos(&foco.id)
        .into_iter()
        .filter(|c| c.es_frente())
        .map(|c| {
            format!(
                "  {} {:<6} {}",
                if c.bloquea { '*' } else { ' ' },
                c.alias(),
                c.titulo
            )
        })
        .collect();
    // Closing a parent cannot make its open children invisible. They are
    // counted and the place to look is named; listing them here would drag in
    // the whole tree, which is exactly the noise the focus exists to keep
    // out.
    let directos: std::collections::HashSet<&str> =
        a.hijos(&foco.id).iter().map(|c| c.id.as_str()).collect();
    let hondos = a
        .descendientes(&foco.id)
        .into_iter()
        .filter(|n| n.es_frente() && !directos.contains(n.id.as_str()))
        .filter(|n| !a.hijos(&n.id).iter().any(|c| c.es_frente()))
        .count();
    if hondos > 0 {
        hijos.push(format!(
            "    + {hondos} further down, outside this level   vivac open"
        ));
    }
    s.push(Seccion::fija(encabezado("BORN FROM HERE", hijos)));

    // 4. Invariants.
    let inv: Vec<String> = constraints(a, &camino)
        .iter()
        .map(|c| {
            let riesgo = if c.banderas.is_empty() {
                ""
            } else {
                "   AT RISK"
            };
            format!("  {:<6} {}{riesgo}", c.alias(), c.titulo)
        })
        .collect();
    s.push(Seccion::fija(encabezado("INVARIANTS", inv)));

    // 5. Blocking questions: all of them, untruncated.
    let en_camino: std::collections::HashSet<&str> = camino.iter().map(|n| n.id.as_str()).collect();
    let preg: Vec<String> = a
        .todos()
        .filter(|n| n.tipo == Tipo::Question && n.estado.abierto() && n.bloquea)
        .filter(|n| {
            a.ancestros(&n.id)
                .iter()
                .any(|p| en_camino.contains(p.id.as_str()))
        })
        .map(|n| format!("  {:<6} {}", n.alias(), n.titulo))
        .collect();
    let mut preg = preg;
    preg.sort();
    s.push(Seccion::fija(encabezado("BLOCKS", preg)));

    // 6. Flags on the path, or one hop off it.
    let mut marcados: Vec<&Nodo> = a
        .todos()
        .filter(|n| !n.banderas.is_empty())
        .filter(|n| {
            en_camino.contains(n.id.as_str())
                || n.padre
                    .as_ref()
                    .is_some_and(|p| en_camino.contains(p.as_str()))
        })
        .collect();
    marcados.sort_by_key(|n| n.num);
    let ban: Vec<String> = marcados
        .iter()
        .flat_map(|n| {
            n.banderas.iter().map(move |(b, motivo)| {
                format!(
                    "  {:<6} {:<10} {}",
                    n.alias(),
                    b.palabra(),
                    corta(motivo, 44)
                )
            })
        })
        .collect();
    s.push(Seccion::suelta(encabezado(
        "FLAGGED",
        recortar(ban, 3, "stats"),
    )));

    // 7. Out of scope. **This is the product's differentiator**, and it only
    // has content if `park` costs the same as `pop`.
    let mut aparcados: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.estado == Estado::Suspended)
        .filter(|n| {
            a.ancestros(&n.id)
                .iter()
                .rev()
                .skip(1)
                .any(|p| en_camino.contains(p.id.as_str()))
        })
        .collect();
    aparcados.sort_by_key(|n| n.num);
    let fuera: Vec<String> = aparcados
        .iter()
        .flat_map(|n| {
            let colgado = n
                .padre
                .as_ref()
                .and_then(|p| a.nodo(p))
                .map(|p| format!("hangs off {}", p.alias()))
                .unwrap_or_default();
            let mut v = vec![format!(
                "  {:<6} {:<40} {colgado}",
                n.alias(),
                corta(&n.titulo, 40)
            )];
            if !n.resultado.is_empty() {
                v.push(format!("         \"{}\"", corta(&n.resultado, 56)));
            }
            v
        })
        .collect();
    s.push(Seccion::suelta(encabezado(
        "DO NOT TOUCH NOW",
        recortar(fuera, 6, "parked"),
    )));

    // 8. Standing decisions: on the path, or with a `governs` overlapping the
    // focus's own. Superseded ones never appear.
    let mut dec: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.tipo == Tipo::Decision && n.estado.abierto())
        .filter(|n| {
            en_camino.contains(n.id.as_str())
                || n.padre
                    .as_ref()
                    .is_some_and(|p| en_camino.contains(p.as_str()))
                || n.governs
                    .iter()
                    .any(|g| foco.governs.iter().any(|f| crate::glob::cubre(g, f)))
        })
        .collect();
    dec.sort_by_key(|n| n.num);
    let decs: Vec<String> = dec
        .iter()
        .map(|n| format!("  {:<6} {}", n.alias(), corta(&n.titulo, 52)))
        .collect();
    s.push(Seccion::suelta(encabezado(
        "STANDING DECISIONS",
        recortar(decs, 3, "tree"),
    )));

    // 9. Last vivac. Restoring is always restore + diff: a vivac is never
    // presented without saying what changed since.
    let vv: Vec<String> = match a.ultimo_vivac() {
        None => vec![],
        Some(v) => {
            let mut l = vec![format!(
                "  {} · {} · {}{}",
                v.alias(),
                v.kind.palabra(),
                crate::clock::date_of(&v.ts),
                if v.anchor.vacio() {
                    String::new()
                } else {
                    format!(" · {}", v.anchor.corto())
                }
            )];
            if !v.next_intent.is_empty() {
                l.push(format!("         you were about to: {}", corta(&v.next_intent, 52)));
            }
            // With no anchor no diff lines are invented: they are omitted, and
            // the date above stands in, which is the plain age there really is.
            if !v.anchor.vacio() {
                let cambios = ancla.changed_since(&v.anchor);
                if !cambios.is_empty() {
                    let tocan = cambios
                        .iter()
                        .filter(|c| v.working_set.iter().any(|g| crate::glob::cubre(g, &c.ruta)))
                        .count();
                    l.push(format!(
                        "         {} changes since, {tocan} touching what it governs",
                        cambios.len()
                    ));
                }
            }
            l
        }
    };
    s.push(Seccion::suelta(encabezado("LAST VIVAC", vv)));

    // 10. Freshness.
    let viejos: Vec<String> = camino
        .iter()
        .filter(|n| n.banderas.contains_key(&crate::event::Bandera::Stale))
        .map(|n| format!("  {:<6} {}", n.alias(), n.titulo))
        .collect();
    s.push(Seccion::suelta(encabezado("UNTOUCHED FOR A WHILE", viejos)));

    emitir(s, presupuesto, a)
}

/// Assembles under budget. It is a **soft ceiling**: truncatable sections are
/// dropped from the bottom up until it fits; if it still does not fit, it is
/// emitted anyway with a warning. Going over budget is a sign the tree needs
/// pruning, not that the brief should lie by silent omission.
fn emitir(
    mut s: Vec<Seccion>,
    presupuesto: usize,
    a: &Arbol,
) -> Result<String, crate::fallo::Fallo> {
    let pedidos = tokens_de(&s);
    while tokens_de(&s) > presupuesto {
        match s.iter().rposition(|x| x.truncable && !x.lineas.is_empty()) {
            Some(i) => s[i].lineas.clear(),
            None => break,
        }
    }
    let gastados = tokens_de(&s);

    let mut o = String::new();
    for l in s.iter().flat_map(|x| x.lineas.iter()) {
        o.push_str(l);
        o.push('\n');
    }
    let aparcados = a.todos().filter(|n| n.estado == Estado::Suspended).count();
    o.push_str(&format!(
        "
{RAYA}
 {gastados} tokens · depth {} · {aparcados} parked
",
        a.profundidad_pila()
    ));
    if gastados > presupuesto {
        o.push_str(&format!(
            "
 ! the brief is over budget ({gastados}/{presupuesto}).
   The spine is never truncated: what is left over is tree, not render.
   What can be pruned:  vivac triage
"
        ));
    } else if pedidos > presupuesto {
        o.push_str(&format!(
            "
 ! {} tokens trimmed to fit in {presupuesto}.
",
            pedidos - gastados
        ));
    }
    Ok(o)
}

/// Empty stack. **It never comes out empty**: it shows the open goals and one
/// concrete action.
fn sin_foco(a: &Arbol, proyecto: &str, fecha: &str) -> Result<String, crate::fallo::Fallo> {
    let mut o = format!(
        "vivac · project: {proyecto} · lane: main · {fecha}
{RAYA}

 No active focus.
"
    );
    let mut metas: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.estado.abierto() && (n.tipo == Tipo::Goal || n.padre.is_none()))
        .collect();
    metas.sort_by_key(|n| n.num);
    if !metas.is_empty() {
        o.push_str(
            "
 OPEN GOALS
",
        );
        for m in metas {
            o.push_str(&format!(
                "  {:<6} {:<40} {} open below
",
                m.alias(),
                corta(&m.titulo, 40),
                a.recuento(&m.id).abiertos
            ));
        }
    }
    o.push('\n');
    if a.vacio() {
        o.push_str(
            " Start with:  vivac push \"<title>\" --why \"<reason>\"
",
        );
    } else {
        o.push_str(
            " Pick up with:  vivac focus <id>
",
        );
        o.push_str(
            " Or open another:  vivac push \"<title>\" --why \"<reason>\"
",
        );
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_estimador_es_determinista() {
        assert_eq!(tokens("same"), 1);
        assert_eq!(tokens("same tokens"), 3);
        assert_eq!(tokens(""), 0);
        // Same text, same number, always.
        assert_eq!(tokens("abcdefgh"), tokens("12345678"));
    }

    #[test]
    fn recortar_avisa_de_lo_que_falta() {
        let v: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let r = recortar(v, 3, "parked");
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], "l0");
        assert!(r[3].contains("7 more"), "{}", r[3]);
    }

    #[test]
    fn una_seccion_vacia_no_deja_encabezado() {
        assert!(encabezado("DO NOT TOUCH NOW", vec![]).is_empty());
        assert_eq!(encabezado("X", vec!["  a".into()]).len(), 3);
    }

    #[test]
    fn cortar_respeta_palabras() {
        assert_eq!(corta("hello world", 20), "hello world");
        assert!(corta("a fairly long sentence that does not fit", 20).ends_with("..."));
        assert!(
            corta("a fairly long sentence that does not fit", 20)
                .chars()
                .count()
                <= 20
        );
    }
}
