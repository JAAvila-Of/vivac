//! El `brief`: render determinista y acotado en tokens.
//!
//! `BRIEF-SPEC.md`. Contesta tres preguntas en orden de importancia: donde
//! estamos y como llegamos, que gobierna este punto, y **que esta fuera de
//! alcance ahora mismo**. La tercera es la que ninguna otra herramienta emite:
//! toda herramienta de memoria vuelca lo relevante, y el problema del
//! desarrollo agentico es el contrario, acotar.
//!
//! Dos reglas mandan sobre todo lo demas:
//!
//! - **Mismo log + mismo `--now` + mismo estado del ancla -> mismos bytes.**
//!   Sin `--now` el determinismo seria imposible, porque las antiguedades son
//!   relativas al momento.
//! - **La espina nunca se trunca.** Si no cabe, el presupuesto esta mal y se
//!   avisa, pero sale entera: es la respuesta a la pregunta 1, y sin ella el
//!   brief no tiene razon de existir.

use crate::anchor::Anchor;
use crate::args::Args;
use crate::event::{Estado, Tipo};
use crate::fallo::R;
use crate::model::{Arbol, Nodo};

const PRESUPUESTO: usize = 1500;
/// Todo el brief es ASCII puro.
///
/// `BRIEF-SPEC.md` §7 dibuja la espina con caracteres de caja, pero el pilar
/// de DX exige degradar sin romperse "en cmd.exe ademas de Windows Terminal",
/// y ahi una pagina de codigos que no sea UTF-8 los convierte en basura. Lo
/// normativo de §7 son los marcadores --que el foco se vea, que una bandera
/// lleve su motivo, que una seccion vacia no salga-- no los glifos.
const RAYA: &str = "------------------------------------------------------------";

/// Una seccion del brief. El orden del vector es el de §3, que es a la vez
/// orden de renderizado y de prioridad: se trunca desde abajo.
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

/// Estimador de tokens. Es una estimacion y el techo es orientativo: lo que
/// importa es que sea **determinista**, para que dos ejecuciones del mismo log
/// trunquen igual.
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

/// Trunca una lista conservando los primeros `n`. Nunca se quita un elemento
/// del medio en silencio.
fn recortar(mut v: Vec<String>, n: usize, que: &str) -> Vec<String> {
    if v.len() > n {
        let sobran = v.len() - n;
        v.truncate(n);
        v.push(format!("      ... y {sobran} mas (vivac {que})"));
    }
    v
}

fn encabezado(titulo: &str, cuerpo: Vec<String>) -> Vec<String> {
    // Las secciones vacias se omiten enteras, incluido el encabezado: un brief
    // sin nada aparcado no dice "NO TOCAR AHORA: (vacio)".
    if cuerpo.is_empty() {
        return vec![];
    }
    let mut v = vec![String::new(), format!(" {titulo}")];
    v.extend(cuerpo);
    v
}

/// Constraints que gobiernan el camino.
///
/// **Solo por `spawns`.** Heredar tambien por `depends_on` convertiria el
/// calculo de O(profundidad) a O(grafo), y perderia que la herencia sea
/// legible mirando la pila en pantalla.
fn constraints<'a>(a: &'a Arbol, camino: &[&Nodo]) -> Vec<&'a Nodo> {
    let en_camino: std::collections::HashSet<&str> = camino.iter().map(|n| n.id.as_str()).collect();
    let mut v: Vec<&Nodo> = a
        .todos()
        .filter(|n| n.tipo == Tipo::Constraint && n.estado.abierto())
        .filter(|n| {
            // Del proyecto (cuelga de una raiz), o alcanzable desde el camino.
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
    // En riesgo primero --las que llevan bandera-- y luego por alias.
    v.sort_by_key(|n| (n.banderas.is_empty(), n.num));
    v
}

fn espina(camino: &[&Nodo]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, n) in camino.iter().enumerate() {
        let primero = i == 0;
        let ultimo = i == camino.len() - 1;
        // Continuacion: el tronco sigue mientras quede algo debajo.
        let sigue = if ultimo { "        " } else { "  |     " };

        let rama = if primero {
            " META ".to_string()
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
        let aqui = if ultimo { "   <== AQUI" } else { "" };
        v.push(format!(
            "{rama}{:<6} {}{bandera}{aqui}",
            n.alias(),
            corta(&n.titulo, 44)
        ));
        if !primero && !n.por.is_empty() {
            v.push(format!("{sigue}por: {}", corta(&n.por, 52)));
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

/// Corta por palabra sin pasarse de `n`, **contando los puntos suspensivos**.
/// Presupuestarlos importa: si no, el corte se pasa de largo justo en las
/// lineas mas apretadas del brief, que son las que se truncan.
fn corta(s: &str, n: usize) -> String {
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

/// El brief como texto. `session start --hook` lo necesita entero para
/// meterlo en el sobre: lo que quede fuera del sobre, el agente no lo ve.
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

    // 1. Cabecera. 2. Espina, que nunca se trunca.
    s.push(Seccion::fija(vec![
        format!("vivac · proyecto: {proyecto} · lane: main · {fecha}"),
        RAYA.to_string(),
        String::new(),
    ]));
    s.push(Seccion::fija(espina(&camino)));

    // 3. Foco: lo que cuelga de el sin cerrar.
    let hijos: Vec<String> = a
        .hijos(&foco.id)
        .into_iter()
        .filter(|c| c.estado.abierto())
        .map(|c| {
            format!(
                "  {} {:<6} {}",
                if c.bloquea { '*' } else { ' ' },
                c.alias(),
                c.titulo
            )
        })
        .collect();
    s.push(Seccion::fija(encabezado("NACIO DE AQUI", hijos)));

    // 4. Invariantes.
    let inv: Vec<String> = constraints(a, &camino)
        .iter()
        .map(|c| {
            let riesgo = if c.banderas.is_empty() {
                ""
            } else {
                "   EN RIESGO"
            };
            format!("  {:<6} {}{riesgo}", c.alias(), c.titulo)
        })
        .collect();
    s.push(Seccion::fija(encabezado("INVARIANTES", inv)));

    // 5. Preguntas bloqueantes: todas, sin truncar.
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
    s.push(Seccion::fija(encabezado("BLOQUEA", preg)));

    // 6. Banderas del camino, o a un salto de el.
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
        "MARCADO",
        recortar(ban, 3, "stats"),
    )));

    // 7. Fuera de alcance. **Es el diferenciador del producto**, y solo tiene
    // contenido si `park` cuesta lo mismo que `pop`.
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
                .map(|p| format!("cuelga de {}", p.alias()))
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
        "NO TOCAR AHORA",
        recortar(fuera, 6, "parked"),
    )));

    // 8. Decisiones vigentes: en el camino, o con `governs` que solapa con el
    // del foco. Las superadas no aparecen nunca.
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
        "DECISIONES VIGENTES",
        recortar(decs, 3, "tree"),
    )));

    // 9. Ultimo vivac. Restaurar es siempre restaurar + diff: nunca se
    // presenta un vivac sin decir que cambio desde entonces.
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
                l.push(format!("         ibas a: {}", corta(&v.next_intent, 52)));
            }
            // Sin ancla no se inventan lineas de diff: se omiten, y arriba
            // queda la fecha, que es la antiguedad temporal que si se tiene.
            if !v.anchor.vacio() {
                let cambios = ancla.changed_since(&v.anchor);
                if !cambios.is_empty() {
                    let tocan = cambios
                        .iter()
                        .filter(|c| v.working_set.iter().any(|g| crate::glob::cubre(g, &c.ruta)))
                        .count();
                    l.push(format!(
                        "         {} cambios desde entonces, {tocan} tocan lo que gobierna",
                        cambios.len()
                    ));
                }
            }
            l
        }
    };
    s.push(Seccion::suelta(encabezado("ULTIMO VIVAC", vv)));

    // 10. Frescura.
    let viejos: Vec<String> = camino
        .iter()
        .filter(|n| n.banderas.contains_key(&crate::event::Bandera::Stale))
        .map(|n| format!("  {:<6} {}", n.alias(), n.titulo))
        .collect();
    s.push(Seccion::suelta(encabezado("SIN TOCAR HACE TIEMPO", viejos)));

    emitir(s, presupuesto, a)
}

/// Ensambla con presupuesto. Es un **techo blando**: se van quitando secciones
/// truncables de abajo arriba hasta caber; si aun asi no cabe, se emite igual
/// con un aviso. Rebasar el presupuesto es señal de que el arbol necesita
/// poda, no de que el brief deba mentir por omision silenciosa.
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
 {gastados} tokens · profundidad {} · {aparcados} aparcados
",
        a.profundidad_pila()
    ));
    if gastados > presupuesto {
        o.push_str(&format!(
            "
 ! el brief excede el presupuesto ({gastados}/{presupuesto}).
                La espina no se trunca nunca: lo que sobra es arbol, no render.
"
        ));
    } else if pedidos > presupuesto {
        o.push_str(&format!(
            "
 ! {} tokens recortados para caber en {presupuesto}.
",
            pedidos - gastados
        ));
    }
    Ok(o)
}

/// Pila vacia. **Nunca sale vacio**: enseña los objetivos abiertos y una
/// accion concreta.
fn sin_foco(a: &Arbol, proyecto: &str, fecha: &str) -> Result<String, crate::fallo::Fallo> {
    let mut o = format!(
        "vivac · proyecto: {proyecto} · lane: main · {fecha}
{RAYA}

 Sin foco activo.
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
 OBJETIVOS ABIERTOS
",
        );
        for m in metas {
            o.push_str(&format!(
                "  {:<6} {:<40} {} abiertos por debajo
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
            " Empieza con:  vivac push \"<titulo>\" --por \"<motivo>\"
",
        );
    } else {
        o.push_str(
            " Retoma con:   vivac focus <id>
",
        );
        o.push_str(
            " O abre otro:  vivac push \"<titulo>\" --por \"<motivo>\"
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
        assert_eq!(tokens("hola"), 1);
        assert_eq!(tokens("hola mundo"), 3);
        assert_eq!(tokens(""), 0);
        // Mismo texto, misma cifra, siempre.
        assert_eq!(tokens("abcdefgh"), tokens("12345678"));
    }

    #[test]
    fn recortar_avisa_de_lo_que_falta() {
        let v: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let r = recortar(v, 3, "parked");
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], "l0");
        assert!(r[3].contains("7 mas"), "{}", r[3]);
    }

    #[test]
    fn una_seccion_vacia_no_deja_encabezado() {
        assert!(encabezado("NO TOCAR AHORA", vec![]).is_empty());
        assert_eq!(encabezado("X", vec!["  a".into()]).len(), 3);
    }

    #[test]
    fn cortar_respeta_palabras() {
        assert_eq!(corta("hola mundo", 20), "hola mundo");
        assert!(corta("una frase bastante larga que no cabe", 20).ends_with("..."));
        assert!(
            corta("una frase bastante larga que no cabe", 20)
                .chars()
                .count()
                <= 20
        );
    }
}
