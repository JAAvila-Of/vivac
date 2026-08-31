//! Los dos hooks de sesion. `ROADMAP.md` §4.
//!
//! `session start` inyecta el brief y `session end` deja una parada
//! automatica. Son las **costuras de la sesion**, igual que `push`/`pop` son
//! las del trabajo: no piden un juicio de relevancia, ocurren solas.
//!
//! Los dos salen con 0 y sin decir nada cuando no hay `.vivac/`. Un hook que
//! falla en cada directorio sin arbol se desactiva a los dos dias.

use crate::args::Args;
use crate::event::VivacKind;
use crate::fallo::{Fallo, R};

/// Envuelve texto en el sobre que Claude Code inyecta en el contexto. Es una
/// linea de JSON, sin dependencias externas ni `jq` en medio: un hook con una
/// tuberia es un hook que se rompe en la primera maquina distinta.
fn sobre(evento: &str, texto: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": evento,
            "additionalContext": texto,
        }
    })
    .to_string()
}

pub fn start(ctx: &crate::ops::Ctx, a: &Args, proyecto: &str) -> R {
    if !a.tiene("hook") {
        return crate::brief::brief(&ctx.arbol, ctx.anchor.as_ref(), a, proyecto);
    }
    // En modo hook el brief se captura y se emite dentro del sobre. Nada de
    // ruido suelto en stdout: lo que no va en el sobre, el agente no lo ve.
    let texto = crate::brief::a_texto(&ctx.arbol, ctx.anchor.as_ref(), a, proyecto)?;
    println!("{}", sobre("SessionStart", &texto));
    Ok(())
}

pub fn end(ctx: &mut crate::ops::Ctx, a: &Args) -> R {
    // Sin pila no hay largo que cerrar, y un vivac vacio solo es ruido que
    // luego hay que podar.
    if ctx.arbol.pila.is_empty() {
        if !a.tiene("hook") {
            println!("  Pila vacia: no hay parada que guardar.");
        }
        return Ok(());
    }
    let luego = a.opt_o("luego");
    let num = ctx.arbol.siguiente_vivac.max(1);
    crate::ops::vivac_auto(ctx, VivacKind::Auto, &luego)?;
    if !a.tiene("hook") {
        println!("  v{num}  parada automatica al cerrar la sesion");
    }
    Ok(())
}

pub fn despachar(ctx: &mut crate::ops::Ctx, a: &Args, proyecto: &str) -> R {
    match a.libre(0) {
        Some("start") | Some("inicio") => start(ctx, a, proyecto),
        Some("end") | Some("fin") => end(ctx, a),
        _ => Err(Fallo::uso("uso: vivac session start|end [--hook]")),
    }
}

/// `vivac hooks` — imprime lo que hay que pegar, y no toca la configuracion
/// de nadie. Escribir en los ajustes del usuario es una accion que se pide,
/// no una que se hace por sorpresa.
pub fn hooks() -> R {
    println!(
        r#"
  Pega esto en .claude/settings.json del proyecto:

  {{
    "hooks": {{
      "SessionStart": [
        {{ "hooks": [{{ "type": "command", "command": "vivac session start --hook" }}] }}
      ],
      "Stop": [
        {{ "hooks": [{{ "type": "command", "command": "vivac session end --hook" }}] }}
      ]
    }}
  }}

  SessionStart inyecta el brief en el contexto del agente.
  Stop deja una parada automatica con la pila del momento.

  Los dos callan y salen con 0 donde no hay .vivac/, asi que se pueden dejar
  puestos en la configuracion global sin que molesten en otros proyectos.
"#
    );
    Ok(())
}
