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
mod changes;
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
mod outcome;
mod params;
mod project;
mod reconcile;
mod redact;
mod render;
mod session;
mod store;
mod web;

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
    vivac decide "<title>" --reason "<r>" [--parent N] [--alternative X]
          [--supersedes d9] [--blocks] [--ref R] [--governs G]
    vivac flag <id> suspect|review|stale --why "<reason>"  [--off]

  Safe stops

    vivac save ["<label>"] [--next "<what you were about to do>"]
    vivac restore <v>                         rebuilds the stack, gives the diff
    vivac vivacs                              the stops, latest first

  The maintainer reads          (--json on all of them but the brief)

    vivac brief [--budget 1500] [--now <date>]
                                              where you are and what NOT to touch
    vivac why <id> [--full]                   WHY WE ARE HERE
                                              --full: anchor, standing
                                              decisions and open siblings,
                                              per step of the path
    vivac tree [id] [--all]                   the tree, with false closes marked
    vivac open                                open fronts and their lineage
    vivac find "<text>"                       every node whose words match
    vivac stack                               where you are right now
    vivac parked                              DO NOT TOUCH NOW
    vivac triage                              what can be pruned, and with what
    vivac reconcile [--since <v>] [--all]     files that changed with nothing
                                              in the tree claiming them
    vivac changes [--since <v>|manual]        what moved since a stop, or
                                              since the last one you made
    vivac stats                               numbers
    vivac check                               invariants; belongs in CI

  Session

    vivac session start [--hook]              the brief, ready to inject
    vivac session end   [--hook]              automatic stop at close
    vivac mcp                                 serve the reads over MCP
    vivac web [--port N] [--no-open]          the tree in a browser, and
          [--project P]                       nowhere but this machine
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

    // Valid options per command. One that is not here is an error and not
    // silence: see `Args::unknown`.
    //
    // `--json` is listed command by command, and it used to be common to all
    // of them. Common meant every command took it and ten did something with
    // it: `vivac brief --json` and `vivac push "x" --json` both printed text
    // and left with a 0. A flag allowed everywhere and read in ten places is
    // the very failure this table exists to prevent, one level up (`f51`).
    //
    // This runs before every command below, including the ones that return
    // before ever touching a store: a command that does its job and along
    // the way ignores what it did not understand is exactly what `f51`
    // describes.
    let allowed: &[&str] = match cmd {
        "push" => &["why", "type", "blocks", "ref", "governs"],
        "pop" => &["force", "next"],
        "decide" => &[
            "parent",
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
        "changes" => &["since", "json"],
        "web" => &["port", "no-open", "project"],
        "init" | "hooks" | "mcp" => &[],
        // The reads that speak JSON, spelled out. No shorthand: a shorthand
        // is what let the brief claim it for two releases.
        "open" | "stack" | "parked" | "triage" | "stats" | "vivacs" | "find" | "check" => &["json"],
        // `--full` is its own on top of `--json`, so `why` cannot share the
        // arm above without granting every other read a flag it does not
        // read.
        "why" => &["json", "full"],
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

    // Same reason as `mcp`, over one or more roots instead of one: the
    // server keeps running after this call returns, so it builds its own
    // registry rather than taking the `ctx` below.
    if cmd == "web" {
        let mut roots: Vec<std::path::PathBuf> = a
            .list("project")
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        if roots.is_empty() {
            roots.push(root);
        }
        let port =
            match a.opt("port") {
                None => None,
                Some(p) => Some(p.parse::<u16>().map_err(|_| {
                    Failure::usage(format!("--port needs a port number, not \"{p}\""))
                })?),
            };
        return web::serve(roots, port, !a.has("no-open")).map(|_| 0);
    }

    // Its own load, ahead of the generic one below, for the same reason as
    // `mcp` and `web`: the generic one folds the log and drops the events,
    // and `changes` is the one command that needs them back. Reading here
    // and again below would read the log twice for nothing.
    if cmd == "changes" {
        let (ctx, log) = ops::Ctx::load_with_log(store::Store::open(root)?)?;
        return changes::changes(&ctx.tree, &log, a);
    }

    // Its own load too, for the same reason: `--full` answers "who was
    // still open when this was born", and the folded `Tree` has already
    // forgotten that once a node closes. The "one word of its own" limit
    // is checked here rather than left to the generic path below, so
    // `vivac why t1 extra` still refuses it exactly as it always has.
    if cmd == "why" {
        let (ctx, log) = ops::Ctx::load_with_log(store::Store::open(root)?)?;
        if let [first, ..] = a.extra(1) {
            return Err(Failure::usage(format!(
                "{cmd} does not take \"{first}\".

  It takes one word of its own. Everything else goes behind a --flag, and a flag
  that repeats is written out again:  --governs a --governs b"
            )));
        }
        return render::why(&ctx.tree, &log, a).map(|_| 0);
    }

    let mut ctx = ops::Ctx::load(store::Store::open(root)?)?;

    // `check` is the only one with an exit code of its own: it separates
    // store corruption from a finding about the project.
    if cmd == "check" {
        return check::check(&ctx.tree, a);
    }

    // Words each command takes of its own. Anything past that is refused for
    // the same reason an unknown flag is: the table above only ever covered
    // half the command line, and the other half went through in silence
    // (`f52`).
    let takes: usize = match cmd {
        "park" | "abandon" | "done" | "note" | "flag" => 2,
        "focus" | "push" | "pop" | "promote" | "add" | "block" | "decide" | "save" | "restore"
        | "import" | "tree" | "session" | "find" => 1,
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

    if let Some(o) = write_op(cmd, &mut ctx, a)? {
        print!("{}", outcome::to_text(&o));
        // Trap: `focus` and `restore` used to end by delegating to
        // `render::stack`, which reads `--json` off `a` on its own. Neither
        // is allowed `--json` in the table above, so that branch was never
        // reachable from either call site; the call just moves here, right
        // after the `Outcome` each one now returns is printed.
        if matches!(cmd, "focus" | "restore") {
            render::stack(&ctx.tree, a)?;
        }
        return Ok(0);
    }

    let r: failure::R = match cmd {
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

/// The fourteen write operations, matched once so that printing an `Outcome`
/// lives in exactly one place in `dispatch` below. `None` means `cmd` names
/// one of the reads instead, which go on printing for themselves --
/// `render.rs` and `brief.rs` are not part of this: they are not writes.
fn write_op(cmd: &str, ctx: &mut ops::Ctx, a: &Args) -> Result<Option<outcome::Outcome>, Failure> {
    Ok(Some(match cmd {
        "push" => ops::push(ctx, params::Push::from_args(a)?)?,
        "pop" => ops::pop(ctx, params::Pop::from_args(a)?)?,
        "done" => ops::done(ctx, params::Done::from_args(a)?)?,
        "park" => ops::park(ctx, params::Park::from_args(a)?)?,
        "add" => ops::add(ctx, params::Add::from_args(a)?)?,
        "note" => ops::note(ctx, params::Note::from_args(a)?)?,
        "block" => ops::block(ctx, params::Block::from_args(a)?)?,
        "promote" => ops::promote(ctx, params::Promote::from_args(a)?)?,
        "abandon" => ops::abandon(ctx, params::Abandon::from_args(a)?)?,
        "focus" => ops::focus(ctx, params::Focus::from_args(a)?)?,
        "flag" => ops::flag(ctx, params::Flag::from_args(a)?)?,
        "decide" => ops::decide(ctx, params::Decide::from_args(a)?)?,
        "save" => ops::save(ctx, params::Save::from_args(a)?)?,
        "restore" => ops::restore(ctx, params::Restore::from_args(a)?)?,
        _ => return Ok(None),
    }))
}
