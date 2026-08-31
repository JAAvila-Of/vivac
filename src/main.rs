//! `vivac` — procedencia del trabajo.
//!
//! Un arbol donde cada nodo sabe de cual nacio, para poder contestar "¿por que
//! estamos aca?" meses despues.
//!
//! No detecta nada y no adivina nada. Esa era la tesis anterior y fallo sus
//! dos puertas de decision: lo que se detectaba no era el hilo perdido. La
//! captura es explicita y se apoya en las costuras del trabajo --se abre un
//! nodo al empezar, se cierra al terminar-- porque lo unico que se midio de
//! verdad es que una operacion que pide un juicio de relevancia no se llama
//! nunca bajo carga.

mod args;
mod check;
mod clock;
mod event;
mod fallo;
mod id;
mod import;
mod model;
mod ops;
mod redact;
mod render;
mod store;

use args::Args;
use fallo::Fallo;

const USO: &str = r#"vivac - procedencia del trabajo

  El agente escribe (la pila lleva el arbol sola)

    vivac focus <id> [--reabrir]              vuelve a entrar en un nodo
    vivac push "<titulo>" --por "<motivo>"    abre un nodo y lo apila
          [--tipo goal|task|decision|question|constraint|finding|assumption]
          [--bloquea]        su padre no cierra hasta que este cierre
          [--ref R] [--governs G]
    vivac pop ["<resultado>"]                 cierra el foco, vuelve al padre
    vivac park [<id>] ["<motivo>"]            aparca: alimenta NO TOCAR AHORA
    vivac promote [<id>]                      el foco pasa a ser meta propia
    vivac abandon [<id>] ["<motivo>"] [--cascada]

  Sin tocar la pila

    vivac add "<titulo>" [--padre N] [--por "<motivo>"] [--bloquea]
    vivac done <id> ["<resultado>"] [--forzar]
    vivac note [<id>] "<nota>"
    vivac block <id> [--off]

  El mantenedor lee            (todos aceptan --json)

    vivac why <id>                            POR QUE ESTAMOS ACA
    vivac tree [id] [--todo]                  el arbol, con cierres falsos
    vivac open                                frentes abiertos y su linaje
    vivac stack                               donde estas ahora
    vivac parked                              NO TOCAR AHORA
    vivac stats                               cifras
    vivac check                               invariantes; va en CI

  Empezar

    vivac init                                siembra .vivac/ aca
    vivac import <tree.json>                  trae un arbol del spike

  Codigos de salida
    0 bien   1 el modelo rechaza   2 uso   3 guarda de redaccion   4 sin .vivac
"#;

fn main() {
    std::process::exit(correr());
}

fn correr() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        print!("{USO}");
        return 0;
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help" | "ayuda") {
        print!("{USO}");
        return 0;
    }
    if matches!(cmd.as_str(), "-V" | "--version" | "version") {
        println!("vivac {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let a = Args::parse(argv.into_iter().skip(1));

    match despachar(&cmd, &a) {
        Ok(codigo) => codigo,
        Err(e) => {
            let c = e.codigo();
            e.imprimir();
            c
        }
    }
}

fn despachar(cmd: &str, a: &Args) -> Result<i32, Fallo> {
    let cwd = std::env::current_dir().map_err(Fallo::Io)?;

    if cmd == "init" {
        let s = store::Store::crear(&cwd)?;
        println!("  vivac sembrado en {}", cwd.display());
        println!("        proyecto {}", s.config.project_id);
        println!();
        println!("  Primer nodo:  vivac push \"<titulo>\" --por \"<motivo>\"");
        return Ok(0);
    }

    let raiz = store::buscar_raiz(&cwd).ok_or(Fallo::SinStore)?;
    let mut ctx = ops::Ctx::cargar(store::Store::abrir(raiz)?)?;

    // `check` es el unico que devuelve un codigo propio: distingue la
    // corrupcion del almacen de un hallazgo sobre el proyecto.
    if cmd == "check" {
        return check::check(&ctx.arbol, a);
    }

    // Opciones validas por comando. Una que no este aqui es un error, no un
    // silencio: ver `Args::desconocidas`.
    const COMUNES: &[&str] = &["json"];
    let permitidas: &[&str] = match cmd {
        "push" => &["por", "tipo", "bloquea", "ref", "governs"],
        "add" => &["padre", "por", "tipo", "bloquea", "ref", "governs"],
        "pop" | "done" => &["forzar"],
        "abandon" => &["cascada"],
        "focus" => &["reabrir"],
        "block" => &["off"],
        "tree" => &["todo", "all", "json"],
        "park" | "promote" | "note" | "import" => &[],
        _ => COMUNES,
    };
    let desconocidas = a.desconocidas(&[permitidas, COMUNES].concat());
    if !desconocidas.is_empty() {
        let validas = if permitidas.is_empty() {
            "ninguna".to_string()
        } else {
            permitidas
                .iter()
                .map(|o| format!("--{o}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        return Err(Fallo::uso(format!(
            "{} no acepta {}.

  Acepta: {validas}",
            cmd,
            desconocidas
                .iter()
                .map(|o| format!("--{o}"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }

    let r: fallo::R = match cmd {
        "focus" => ops::focus(&mut ctx, a),
        "push" => ops::push(&mut ctx, a),
        "pop" => ops::pop(&mut ctx, a),
        "park" => ops::park(&mut ctx, a),
        "promote" => ops::promote(&mut ctx, a),
        "abandon" => ops::abandon(&mut ctx, a),
        "add" => ops::add(&mut ctx, a),
        "done" => ops::done(&mut ctx, a),
        "note" => ops::note(&mut ctx, a),
        "block" => ops::block(&mut ctx, a),
        "import" => import::import(&mut ctx, a),
        "why" => render::why(&ctx.arbol, a),
        "tree" => render::tree(&ctx.arbol, a),
        "open" => render::open(&ctx.arbol, a),
        "stack" => render::stack(&ctx.arbol, a),
        "parked" => render::parked(&ctx.arbol, a),
        "stats" => render::stats(&ctx.arbol, a),
        otro => {
            print!("{USO}");
            return Err(Fallo::uso(format!("Comando desconocido: {otro}")));
        }
    };
    r.map(|_| 0)
}
