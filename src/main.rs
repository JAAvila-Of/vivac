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
mod failure;
mod glob;
mod id;
mod import;
mod mcp;
mod model;
mod ops;
mod reconcile;
mod redact;
mod render;
mod session;
mod store;

use args::Args;
use failure::Failure;

const USAGE: &str = r#"vivac - provenance of work

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
          [--type goal|task|decision|question|constraint|finding|assumption]
          [--ref R] [--governs G]
    vivac done <id> ["<outcome>"] [--force]
    vivac note [<id>] "<note>"
    vivac block <id> [--off]
    vivac decide "<title>" --reason "<r>" [--alternative X] [--supersedes d9]
    vivac flag <id> suspect|review|stale --why "<reason>"  [--off]

  Safe stops

    vivac save ["<label>"] [--next "<what you were about to do>"]
    vivac restore <v>                         rebuilds the stack, gives the diff
    vivac vivacs                              the stops, latest first

  The maintainer reads          (--json on all of them but the brief)

    vivac brief [--budget 1500] [--now <date>]
                                              where you are and what NOT to touch
    vivac why <id>                            WHY WE ARE HERE
    vivac tree [id] [--all]                   the tree, with false closes marked
    vivac open                                open fronts and their lineage
    vivac find "<text>"                       every node whose words match
    vivac stack                               where you are right now
    vivac parked                              DO NOT TOUCH NOW
    vivac triage                              what can be pruned, and with what
    vivac reconcile [--since <v>] [--all]     files that changed with nothing
                                              in the tree claiming them
    vivac stats                               numbers
    vivac check                               invariants; belongs in CI

  Session

    vivac session start [--hook]              the brief, ready to inject
    vivac session end   [--hook]              automatic stop at close
    vivac mcp                                 serve the reads over MCP
    vivac hooks                               what to paste into settings.json

  Getting started

    vivac init                                plant .vivac/ here
    vivac import <tree.json>                  bring in a tree from the spike

  Exit codes
    0 fine   1 the model refuses   2 usage   3 redaction guard   4 no .vivac
"#;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        print!("{USAGE}");
        return 0;
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        print!("{USAGE}");
        return 0;
    }
    if matches!(cmd.as_str(), "-V" | "--version" | "version") {
        println!("vivac {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let a = Args::parse(argv.into_iter().skip(1));

    match dispatch(&cmd, &a) {
        Ok(code) => code,
        Err(e) => {
            let c = e.code();
            e.print_to_stderr();
            c
        }
    }
}

fn project_name(ctx: &ops::Ctx) -> String {
    ctx.store
        .root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
}

fn dispatch(cmd: &str, a: &Args) -> Result<i32, Failure> {
    let cwd = std::env::current_dir().map_err(Failure::Io)?;

    if cmd == "init" {
        let s = store::Store::create(&cwd)?;
        println!("  vivac planted in {}", cwd.display());
        println!("        project {}", s.config.project_id);
        println!();
        println!("  First node:  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(0);
    }

    if cmd == "hooks" {
        return session::hooks().map(|_| 0);
    }

    let Some(root) = store::find_root(&cwd) else {
        // The hooks stay quiet where there is no tree. One that fails in
        // every unrelated directory gets switched off within two days, and
        // the two that matter go with it.
        if cmd == "session" && a.has("hook") {
            return Ok(0);
        }
        return Err(Failure::NoStore);
    };
    // The server outlives its calls and it is not the only writer, so it
    // loads the tree itself and reloads it when the log moves. Everything
    // below assumes one command, one process, one fold.
    if cmd == "mcp" {
        return mcp::serve(root).map(|_| 0);
    }

    let mut ctx = ops::Ctx::load(store::Store::open(root)?)?;

    // `check` is the only one with an exit code of its own: it separates
    // store corruption from a finding about the project.
    if cmd == "check" {
        return check::check(&ctx.tree, a);
    }

    // Valid options per command. One that is not here is an error and not
    // silence: see `Args::unknown`.
    //
    // `--json` is listed command by command, and it used to be common to all
    // of them. Common meant every command took it and ten did something with
    // it: `vivac brief --json` and `vivac push "x" --json` both printed text
    // and left with a 0. A flag allowed everywhere and read in ten places is
    // the very failure this table exists to prevent, one level up (`f51`).
    let allowed: &[&str] = match cmd {
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
        // The brief does not speak JSON, and that is a decision and not a
        // gap: the shape would have to be designed, it has no consumer
        // today, and the agent reads the brief as prose. `d53`.
        "brief" => &["budget", "now"],
        "session" => &["hook", "next", "budget", "now"],
        "add" => &["parent", "why", "type", "blocks", "ref", "governs"],
        "done" => &["force"],
        "abandon" => &["cascade", "rescue"],
        "focus" => &["reopen"],
        "block" => &["off"],
        "tree" => &["all", "json"],
        "reconcile" => &["since", "all", "json"],
        // The reads that speak JSON, spelled out. No shorthand: a shorthand
        // is what let the brief claim it for two releases.
        "why" | "open" | "stack" | "parked" | "triage" | "stats" | "vivacs" | "find" => &["json"],
        "park" | "promote" | "note" | "import" | "restore" => &[],
        _ => &[],
    };
    let unknown = a.unknown(allowed);
    if !unknown.is_empty() {
        let takes = if allowed.is_empty() {
            "none".to_string()
        } else {
            allowed
                .iter()
                .map(|o| format!("--{o}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        return Err(Failure::usage(format!(
            "{} does not take {}.

  It takes: {takes}",
            cmd,
            unknown
                .iter()
                .map(|o| format!("--{o}"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }

    // Words each command takes of its own. Anything past that is refused for
    // the same reason an unknown flag is: the table above only ever covered
    // half the command line, and the other half went through in silence
    // (`f52`).
    let takes: usize = match cmd {
        "park" | "abandon" | "done" | "note" | "flag" => 2,
        "focus" | "push" | "pop" | "promote" | "add" | "block" | "decide" | "save" | "restore"
        | "import" | "why" | "tree" | "session" | "find" => 1,
        _ => 0,
    };
    if let [first, ..] = a.extra(takes) {
        let room = match takes {
            0 => "no words of its own".to_string(),
            1 => "one word of its own".to_string(),
            n => format!("{n} words of its own"),
        };
        return Err(Failure::usage(format!(
            "{cmd} does not take \"{first}\".

  It takes {room}. Everything else goes behind a --flag, and a flag
  that repeats is written out again:  --governs a --governs b"
        )));
    }

    let r: failure::R = match cmd {
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
            let project = project_name(&ctx);
            brief::brief(&ctx.tree, ctx.anchor.as_ref(), a, &project)
        }
        "vivacs" => render::vivacs(&ctx.tree, a),
        "session" => {
            let project = project_name(&ctx);
            session::dispatch(&mut ctx, a, &project)
        }
        "why" => render::why(&ctx.tree, a),
        "tree" => render::tree(&ctx.tree, a),
        "open" => render::open(&ctx.tree, a),
        "find" => render::find(&ctx.tree, a),
        "stack" => render::stack(&ctx.tree, a),
        "parked" => render::parked(&ctx.tree, a),
        "triage" => render::triage(&ctx.tree, a),
        "reconcile" => reconcile::reconcile(&ctx.tree, ctx.anchor.as_ref(), a),
        "stats" => render::stats(&ctx.tree, a),
        other => {
            print!("{USAGE}");
            return Err(Failure::usage(format!("unknown command: {other}")));
        }
    };
    r.map(|_| 0)
}
