//! `check` — las invariantes de `MODEL.md` §9 que aplican a Tier 0.
//!
//! Separa dos cosas que se parecen y no lo son. Un ciclo o un huerfano es
//! **corrupcion del almacen**: la herramienta miente. Un cierre falso es un
//! **hallazgo sobre el proyecto**: el almacen esta bien y lo que esta mal es
//! el trabajo, que se dio por terminado sin estarlo. Las dos salen con codigo
//! distinto de cero --esto va en CI-- pero no se cuentan juntas.

use crate::args::Args;
use crate::event::Estado;
use crate::model::Arbol;

pub fn check(a: &Arbol, args: &Args) -> Result<i32, crate::fallo::Fallo> {
    let mut almacen: Vec<String> = Vec::new();
    let mut proyecto: Vec<String> = Vec::new();

    if a.lineas_rotas > 0 {
        almacen.push(format!(
            "{} linea(s) ilegibles en .vivac/events (se saltaron al leer)",
            a.lineas_rotas
        ));
    }

    let mut nums = std::collections::HashMap::new();
    for n in a.todos() {
        // Invariante 11: la procedencia es un arbol. El esquema ya impide dos
        // padres --el `spawns` viaja dentro del nodo-- asi que lo unico que
        // puede romperse aqui es que el padre no exista.
        if let Some(p) = &n.padre {
            if a.nodo(p).is_none() {
                almacen.push(format!("{} apunta a un padre que no existe", n.alias()));
            }
        }
        // Invariante 1: aciclica. Si el camino a la raiz no termina en un nodo
        // sin padre, es que da vueltas.
        let camino = a.ancestros(&n.id);
        if camino.first().is_some_and(|r| r.padre.is_some()) {
            almacen.push(format!("{} esta en un ciclo de procedencia", n.alias()));
        }
        if let Some(otro) = nums.insert(n.num, n.alias()) {
            almacen.push(format!(
                "numero {} repetido: {} y {}",
                n.num,
                otro,
                n.alias()
            ));
        }
        // Invariante 10: cierre falso.
        //
        // Un cierre **forzado** no cuenta como violacion: `MODEL.md` §9 lo
        // exime a proposito porque hay cierres legitimos a la fuerza --un
        // carril que se abandona-- y lo que se pedia era que fuesen una
        // decision y no un descuido. El rastro esta en el evento y el render
        // lo sigue marcando; lo que no hace es romper CI todos los dias.
        if n.estado == Estado::Done
            && !n.cierre_forzado
            && !a.bloqueantes_abiertos(&n.id).is_empty()
        {
            let pend = a.bloqueantes_abiertos(&n.id);
            let quienes: Vec<String> = pend.iter().map(|c| c.alias()).collect();
            proyecto.push(format!(
                "{} esta cerrado con {} condicion(es) abierta(s): {}",
                n.alias(),
                pend.len(),
                quienes.join(", ")
            ));
        }
    }
    almacen.sort();
    proyecto.sort();

    if args.tiene("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "almacen": almacen,
                "proyecto": proyecto,
                "ok": almacen.is_empty() && proyecto.is_empty(),
            }))
            .map_err(std::io::Error::other)?
        );
    } else {
        println!();
        if almacen.is_empty() && proyecto.is_empty() {
            println!("  Sin hallazgos. {} nodos revisados.", a.total());
            println!();
        }
        if !almacen.is_empty() {
            println!(
                "  ALMACEN ({})  <- la herramienta miente, hay que arreglarla",
                almacen.len()
            );
            println!();
            for m in &almacen {
                println!("      {m}");
            }
            println!();
        }
        if !proyecto.is_empty() {
            println!(
                "  PROYECTO ({})  <- el almacen esta bien; el trabajo no",
                proyecto.len()
            );
            println!();
            for m in &proyecto {
                println!("      {m}");
            }
            println!();
            println!("  Un cierre falso no se repara editando el arbol: se reabre lo");
            println!("  que quedo abierto, o se cierra a conciencia con --forzar.");
            println!();
        }
    }
    Ok(i32::from(!(almacen.is_empty() && proyecto.is_empty())))
}
