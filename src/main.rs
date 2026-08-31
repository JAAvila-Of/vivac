//! `vivac` — provenance of work.
//!
//! A tree where every node knows which node it was born from, so that "why
//! are we here?" can still be answered months later.
//!
//! It detects nothing and guesses nothing. That was the earlier thesis and it
//! failed both of its decision gates: what got detected was not the lost
//! thread. Capture is explicit and hangs off the seams of the work --a node
//! is opened when you start, closed when you finish-- because the one thing
//! actually measured is that an operation asking for a judgement of relevance
//! never gets called under load.

mod anchor;
mod args;
mod brief;
mod check;
mod clock;
mod event;
mod fallo;
mod glob;
mod id;
mod import;
mod model;
mod ops;
mod redact;
mod render;
mod session;
mod store;

use args::Args;
use fallo::Fallo;

const USO: &str = r#"vivac - provenance of work

  The agent writes (the stack carries the tree on its own)

    vivac focus <id> [--reopen]               step back into a node
    vivac push "<title>" --why "<reason>"     open a node and stack it
          [--type goal|task|decision|question|constraint|finding|assumption]
          [--blocks]         its parent cannot close until this one closes
          [--ref R] [--governs G]
    vivac pop ["<outcome>"] [--next "<...>"]  close the focus, back to the parent
    vivac park [<id>] ["<reason>"]            park it: feeds DO NOT TOUCH NOW
    vivac promote [<id>]                      the focus becomes a goal of its own
    vivac abandon [<id>] ["<reason>"] [--cascade]
          [--rescue <id>]    saves it and its own; it still hangs where it
                             was born, nothing is reparented

  Without touching the stack

    vivac add "<title>" [--parent N] [--why "<reason>"] [--blocks]
    vivac done <id> ["<outcome>"] [--force]
    vivac note [<id>] "<note>"
    vivac block <id> [--off]
    vivac decide "<title>" --reason "<r>" [--alternative X] [--supersedes d9]
    vivac flag <id> suspect|review|stale --why "<reason>"  [--off]

  Safe stops

    vivac save ["<label>"] [--next "<what you were about to do>"]
    vivac restore <v>                         rebuilds the stack, gives the diff
    vivac vivacs                              the stops, latest first

  The maintainer reads          (all of them accept --json)

    vivac brief [--budget 1500] [--now <date>]
                                              where you are and what NOT to touch
    vivac why <id>                            WHY WE ARE HERE
    vivac tree [id] [--all]                   the tree, with false closes marked
    vivac open                                open fronts and their lineage
    vivac stack                               where you are right now
    vivac parked                              DO NOT TOUCH NOW
    vivac triage                              what can be pruned, and with what
    vivac stats                               numbers
    vivac check                               invariants; belongs in CI

  Session

    vivac session start [--hook]              the brief, ready to inject
    vivac session end   [--hook]              automatic stop at close
    vivac hooks                               what to paste into settings.json

  Getting started

    vivac init                                plant .vivac/ here
    vivac import <tree.json>                  bring in a tree from the spike

  Exit codes
    0 fine   1 the model refuses   2 usage   3 redaction guard   4 no .vivac
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

fn nombre_proyecto(ctx: &ops::Ctx) -> String {
    ctx.store
        .raiz
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
}

fn despachar(cmd: &str, a: &Args) -> Result<i32, Fallo> {
    let cwd = std::env::current_dir().map_err(Fallo::Io)?;

    if cmd == "init" {
        let s = store::Store::crear(&cwd)?;
        println!("  vivac planted in {}", cwd.display());
        println!("        project {}", s.config.project_id);
        println!();
        println!("  First node:  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(0);
    }

    if cmd == "hooks" {
        return session::hooks().map(|_| 0);
    }

    let Some(raiz) = store::buscar_raiz(&cwd) else {
        // The hooks stay quiet where there is no tree. One that fails in
        // every unrelated directory gets switched off within two days, and
        // the two that matter go with it.
        if cmd == "session" && a.tiene("hook") {
            return Ok(0);
        }
        return Err(Fallo::SinStore);
    };
    let mut ctx = ops::Ctx::cargar(store::Store::abrir(raiz)?)?;

    // `check` is the only one with an exit code of its own: it separates
    // store corruption from a finding about the project.
    if cmd == "check" {
        return check::check(&ctx.arbol, a);
    }

    // Valid options per command. One that is not here is an error, not
    // silencio: ver `Args::desconocidas`.
    const COMUNES: &[&str] = &["json"];
    let permitidas: &[&str] = match cmd {
        "push" => &["why", "type", "blocks", "ref", "governs"],
        "pop" => &["force", "next"],
        "decide" => &[
            "reason",
            "alternative",
            "supersedes",
            "ref",
            "governs",
            "blocks",
        ],
        "flag" => &["why", "off"],
        "save" => &["next"],
        "brief" => &["budget", "now", "json"],
        "session" => &["hook", "next", "budget", "now"],
        "add" => &["parent", "why", "type", "blocks", "ref", "governs"],
        "done" => &["force"],
        "abandon" => &["cascade", "rescue"],
        "focus" => &["reopen"],
        "block" => &["off"],
        "tree" => &["all", "json"],
        "park" | "promote" | "note" | "import" | "restore" | "vivacs" => &[],
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
        "decide" => ops::decide(&mut ctx, a),
        "flag" => ops::flag(&mut ctx, a),
        "save" => ops::save(&mut ctx, a),
        "restore" => ops::restore(&mut ctx, a),
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
        "brief" => {
            let proyecto = nombre_proyecto(&ctx);
            brief::brief(&ctx.arbol, ctx.anchor.as_ref(), a, &proyecto)
        }
        "vivacs" => render::vivacs(&ctx.arbol, a),
        "session" => {
            let proyecto = nombre_proyecto(&ctx);
            session::despachar(&mut ctx, a, &proyecto)
        }
        "why" => render::why(&ctx.arbol, a),
        "tree" => render::tree(&ctx.arbol, a),
        "open" => render::open(&ctx.arbol, a),
        "stack" => render::stack(&ctx.arbol, a),
        "parked" => render::parked(&ctx.arbol, a),
        "triage" => render::triage(&ctx.arbol, a),
        "stats" => render::stats(&ctx.arbol, a),
        otro => {
            print!("{USO}");
            return Err(Fallo::uso(format!("Comando desconocido: {otro}")));
        }
    };
    r.map(|_| 0)
}
