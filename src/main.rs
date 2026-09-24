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
mod index;
mod lane;
mod mcp;
mod model;
mod ops;
mod outcome;
mod output;
mod params;
mod project;
mod reconcile;
mod redact;
mod registry;
mod relocate;
mod render;
mod repos;
mod session;
mod setup;
mod store;
mod style;
mod web;

use args::Args;
use failure::Failure;
use output::outln;

const USAGE: &str = r#"vivac - provenance of work

  The agent writes (the stack carries the tree on its own)

    vivac focus <id> [--reopen]               step back into a node
    vivac push "<title>" --why "<reason>"     open a node and stack it
          [--type goal|task|decision|question|constraint|finding|assumption
                  |pillar|rule]
          [--blocks]         its parent cannot close until this one closes
          [--parent N]       under N and not the focus; the stack goes to N
          [--root]           born at the root; the stack keeps only it
          [--ref R] [--governs G]
          [--arm "<command>"]  what verifies a rule; vivac never runs it
          [--arm-dir <dir>]    where it runs, relative to where .vivac lives
          [--against "r12: <why>"]  on a decision: what it was judged against
    vivac pop ["<outcome>"] [--next "<...>"]  close the focus, back to the parent
    vivac park [<id>] ["<reason>"]            park it: feeds DO NOT TOUCH NOW
    vivac promote [<id>]                      the focus becomes a goal of its own
    vivac abandon [<id>] ["<reason>"] [--cascade]
          [--rescue <id>]    saves it and its own; it still hangs where it
                             was born, nothing is reparented

  Without touching the stack

    vivac add "<title>" [--parent N | --root] [--why "<reason>"] [--blocks]
          [--type goal|task|decision|question|constraint|finding|assumption
                  |pillar|rule]
          [--ref R] [--governs G]
          [--arm "<command>"]  what verifies a rule; vivac never runs it
          [--arm-dir <dir>]    where it runs, relative to where .vivac lives
          [--against "r12: <why>"]  on a decision: what it was judged against
    vivac done <id> ["<outcome>"] [--force]
    vivac note [<id>] "<note>"
    vivac block <id> [--off]
    vivac arm <rule> "<command>" --dir <dir> [--off]
    vivac declare <decision> --against "r12: <why>"
    vivac decide "<title>" --reason "<r>" [--parent N | --root]
          [--alternative X] [--supersedes d9] [--blocks]
          [--ref R] [--governs G]
          [--against "r12: <why>"]  what it was judged against; repeat it
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
    vivac open [--all]                        open fronts and their lineage
    vivac find "<text>" [--everywhere]        every node whose words match
                                              --everywhere: every project
                                              the registry knows
    vivac stack [--lanes]                     where you are right now
                                              --lanes: every folder of this
                                              product, and what it is on
    vivac parked                              DO NOT TOUCH NOW
    vivac rules                               the pillars, rules and invariants
                                              that govern this project
    vivac triage                              what can be pruned, and with what
    vivac reconcile [--since <v>] [--all]     files that changed with nothing
                                              in the tree claiming them
    vivac changes [--since <v>|manual]        what moved since a stop, or
                                              since the last one you made
    vivac stats                               numbers
    vivac check [--gates]                     invariants; belongs in CI
          --gates    also every tree on this machine that nobody opens

  Session

    vivac session start [--hook]              the brief, ready to inject
    vivac session end   [--hook]              automatic stop at close
    vivac session prompt [--hook]             a nudge when nothing was written
    vivac mcp                                 serve the tree over MCP
    vivac web [--port N] [--no-open]          the tree in a browser, and
          [--project P]                       nowhere but this machine

  Getting started

    vivac init [--dry-run] [--yes] [--undo] [--lane-name <name>]
                                              plant .vivac/ here, or answer
                                              which tree this folder belongs to
          [--name <name>]    the product's name, instead of the folder's;
                             only when init plants a tree
    vivac init --join <name|path> [--lane-name <name>]
                                              join this folder to a tree that
                                              lives somewhere else
    vivac init --new-tree                     plant here even if this
                                              folder's repositories already
                                              belong to a tracked product
    vivac setup claude-code [--dry-run] [--yes] [--undo]
                                              write what Claude Code needs here:
                                              hooks, the MCP server, a skill
    vivac setup codex [--dry-run] [--yes] [--undo]
                                              write what Codex needs here:
                                              hooks, the MCP server, a skill.
                                              Every flag above means the same
    vivac relocate <destination> [--lane-name <name>]
                                              move the tree there, run from the
                                              folder that holds it; this one
                                              stays a lane of it, with its own
                                              thread
    vivac import <tree.json>                  bring in a tree from the spike

  Exit codes
    0 fine   1 the model refuses   2 usage   3 redaction guard   4 no .vivac
    5 input/output error, a tree written by a newer vivac, or a tree
      another process kept locked
"#;

/// Every command the CLI actually dispatches, exactly as `USAGE` names
/// them: `commands_and_usage_stay_in_sync` keeps the two from drifting
/// apart. `unknown_command` walks this before anything else runs (`d772`),
/// so a command left off it would read as unknown no matter what
/// `dispatch` does with it further down.
///
/// `hooks` is deliberately not here: it is gone (`d557`) and answers with
/// its own tombstone, which `unknown_command` is never allowed to shadow --
/// see the `cmd != "hooks"` guard where this is read.
const COMMANDS: &[&str] = &[
    "focus",
    "push",
    "pop",
    "park",
    "promote",
    "abandon",
    "add",
    "done",
    "note",
    "block",
    "arm",
    "declare",
    "decide",
    "flag",
    "save",
    "restore",
    "vivacs",
    "brief",
    "why",
    "tree",
    "open",
    "find",
    "stack",
    "parked",
    "rules",
    "triage",
    "reconcile",
    "changes",
    "stats",
    "check",
    "session",
    "mcp",
    "web",
    "init",
    "setup",
    "relocate",
    "import",
];

/// A synonym typed for a command that does exist, paired with the second
/// line `unknown_command` gives instead of guessing which flags it takes:
/// the word for a thing an agent already knows how to ask for, spelled
/// differently (`d772`, `f770`).
const COMMAND_HINTS: &[(&[&str], &str)] = &[
    (
        &[
            "show", "get", "read", "view", "cat", "info", "describe", "inspect",
        ],
        "  To read a node and why it exists:  vivac why <id>",
    ),
    (&["list", "ls"], "  What is still open:  vivac open"),
    (
        &["search", "grep", "query"],
        "  To search the tree:  vivac find \"<words>\"",
    ),
    (
        &["status", "where", "context"],
        "  Where you are and what not to touch:  vivac brief",
    ),
    (
        &["close", "finish", "complete", "resolve"],
        "  To close a node:  vivac done <id> \"<outcome>\", or the focus:  vivac pop",
    ),
    (
        &["new", "create", "start"],
        "  To open a node:  vivac push \"<title>\" --why \"<why>\"",
    ),
    (
        &["log", "history", "diff"],
        "  What moved since a stop:  vivac changes",
    ),
];

/// Levenshtein edit distance: insert, delete and substitute each cost one.
/// Hand-rolled rather than pulled in, the same call `args.rs`'s own module
/// doc makes about the parser itself -- a command name is a handful of
/// characters, so the O(n*m) table this walks is never a cost worth a
/// dependency.
fn edit_distance(x: &str, y: &str) -> usize {
    let x: Vec<char> = x.chars().collect();
    let y: Vec<char> = y.chars().collect();
    let mut row: Vec<usize> = (0..=y.len()).collect();
    for (i, &p) in x.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &q) in y.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if p == q {
                prev
            } else {
                1 + prev.min(row[j]).min(above)
            };
            prev = above;
        }
    }
    row[y.len()]
}

/// The second line `unknown_command` gives, when it has one to give: a
/// known synonym's own hint first, and only then the closest real command
/// within two edits, ties broken alphabetically (`d772`).
fn unknown_command_hint(cmd: &str) -> Option<String> {
    for (words, hint) in COMMAND_HINTS {
        if words.contains(&cmd) {
            return Some(hint.to_string());
        }
    }
    let mut best: Option<(&str, usize)> = None;
    for &candidate in COMMANDS {
        let distance = edit_distance(cmd, candidate);
        if distance > 2 {
            continue;
        }
        best = match best {
            Some((word, current))
                if current < distance || (current == distance && word < candidate) =>
            {
                Some((word, current))
            }
            _ => Some((candidate, distance)),
        };
    }
    best.map(|(candidate, _)| format!("  Closest:  vivac {candidate}"))
}

/// `d772`: a command that is not one of `COMMANDS` -- and is not the
/// `hooks` tombstone below, which answers on its own -- is refused before
/// any flag, positional or tree-lookup check runs, so a typo never gets
/// treated as if the command it named actually existed.
fn unknown_command(cmd: &str) -> Failure {
    let mut msg = format!("\"{cmd}\" is not a vivac command.");
    if let Some(hint) = unknown_command_hint(cmd) {
        msg.push('\n');
        msg.push_str(&hint);
    }
    msg.push_str("\n  Every command:  vivac --help");
    Failure::usage(msg)
}

/// A root that reached the registry with no identity to be keyed by, and
/// the lane it was seen under (its id and folder, when the folder carries
/// one), kept so the attempt can be made again once the command has run.
/// See `note_late`.
static LATE_SIGHTING: std::sync::OnceLock<(
    std::path::PathBuf,
    Option<(String, std::path::PathBuf)>,
)> = std::sync::OnceLock::new();

fn main() {
    let code = run();
    // `std::process::exit` skips `Drop`, so a line still sitting in
    // `output`'s buffer would be lost rather than reach the reader.
    //
    // Flushed *before* the registry is written, not after: writing it can
    // wait on another process holding the registry lock, and the answer
    // this command already produced must not sit in a buffer behind a wait
    // for a side effect nobody asked about (`f603`).
    output::flush();
    note_late();
    // The single seat for the copy warning (`t594`): by
    // now the command has either written or it has not, so this is the one
    // place left in the whole process that can answer honestly.
    registry::warn_if_wrote();
    std::process::exit(code);
}

/// The second and last attempt to register a project whose log was empty when
/// the command started. An empty tree has no identity under `d201` and must not
/// get one; a tree whose first node was planted a moment ago does, and this is
/// where it exists. Still unable to fail anything: the command has already
/// produced its answer by the time this runs, and the one thing this can still
/// add is leaving `noted` for `warn_if_wrote`, below, to decide about.
fn note_late() {
    let (Some((root, lane)), Some(store_dir)) = (LATE_SIGHTING.get(), store::store_dir()) else {
        return;
    };
    if let Some(project_id) = store::first_event_id(root) {
        let noted = registry::note(
            &store_dir,
            &project_id,
            registry::Sighting {
                root,
                lane: lane.as_ref().map(|(id, dir)| (id.as_str(), dir.as_path())),
                repos: None,
            },
        );
        registry::set_pending(noted);
    }
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
        outln!("vivac {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let a = match Args::parse(argv.into_iter().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            let c = e.code();
            // Whatever ran before the refusal has to reach stdout before it
            // reaches stderr, or the two streams interleave out of order
            // once a terminal merges them. Nothing has printed yet here, but
            // `dispatch`'s own error arm below needs the same flush, and the
            // two are kept identical rather than one of them drifting.
            output::flush();
            e.print_to_stderr();
            return c;
        }
    };

    match dispatch(&cmd, &a) {
        Ok(code) => code,
        Err(e) => {
            let c = e.code();
            // Whatever `dispatch` already buffered has to reach stdout
            // before the refusal reaches stderr, or the two streams
            // interleave out of order once a terminal merges them.
            output::flush();
            e.print_to_stderr();
            c
        }
    }
}

/// The brief's own header, and the session hooks that feed it: `t640`,
/// point 10 -- the same effective name every other surface that used to
/// derive one from a folder now shows, `render::project_name` reads it
/// from rather than a second copy of the same derivation.
fn project_name(ctx: &ops::Ctx) -> String {
    render::project_name(&ctx.store.root)
}

fn dispatch(cmd: &str, a: &Args) -> Result<i32, Failure> {
    // `d772`: ahead of every other check below -- flags, positionals, the
    // tree underfoot -- because none of those mean anything for a command
    // that does not exist. `hooks` answers with its own tombstone further
    // down and must not be shadowed by this.
    if !COMMANDS.contains(&cmd) && cmd != "hooks" {
        return Err(unknown_command(cmd));
    }
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
        "push" => &[
            "why", "type", "blocks", "root", "parent", "ref", "governs", "arm", "arm-dir",
            "against",
        ],
        "pop" => &["force", "next"],
        "decide" => &[
            "parent",
            "reason",
            "alternative",
            "supersedes",
            "ref",
            "governs",
            "blocks",
            "against",
            "root",
        ],
        "flag" => &["why", "off"],
        "save" => &["next"],
        // The brief does not speak JSON, and that is a decision and not a
        // gap: the shape would have to be designed, it has no consumer
        // today, and the agent reads the brief as prose. `d53`.
        "brief" => &["budget", "now"],
        "session" => &["hook", "next", "budget", "now"],
        "add" => &[
            "parent", "why", "type", "blocks", "ref", "governs", "arm", "arm-dir", "against",
            "root",
        ],
        "done" => &["force"],
        "abandon" => &["cascade", "rescue"],
        "focus" => &["reopen"],
        "block" => &["off"],
        "arm" => &["dir", "off"],
        "declare" => &["against"],
        "tree" => &["all", "json"],
        "reconcile" => &["since", "all", "json"],
        "changes" => &["since", "json"],
        "web" => &["port", "no-open", "project"],
        "hooks" | "mcp" => &[],
        // `d723` piece A: the same six `setup` already took, with the same
        // meaning -- planting is `init`'s job now, and these are what a
        // plant needs to say. A bare `init` still takes none of them, the
        // same as before this piece: see the guard below, ahead of the
        // block these six actually reach.
        "init" | "setup" => &[
            "dry-run",
            "yes",
            "undo",
            "join",
            "new-tree",
            "lane-name",
            "name",
        ],
        "relocate" => &["lane-name"],
        // The reads that speak JSON, spelled out. No shorthand: a shorthand
        // is what let the brief claim it for two releases.
        // `open` also takes `--all`, the same escape hatch `tree` gives the
        // list it caps (`d383`): the cap must never cost access to the rest.
        "open" => &["json", "all"],
        // `--lanes` is `stack`'s own, `t594` §5.5: every lane's own
        // stack, not only this folder's, is nothing the other reads in
        // this arm take.
        "stack" => &["json", "lanes"],
        "parked" | "triage" | "stats" | "vivacs" | "rules" => &["json"],
        // `--gates` is its own on top of `--json`: the machine-wide scan
        // `d351` adds is nothing the other reads in this arm take.
        "check" => &["json", "gates"],
        // `--full` is its own on top of `--json`, so `why` cannot share the
        // arm above without granting every other read a flag it does not
        // read. `--project` is `d273`'s second half: it answers from
        // another tree entirely, the same fan-out `find --everywhere`
        // already reads the registry for.
        "why" => &["json", "full", "project"],
        // `--everywhere` is `find`'s own for the same reason: the registry
        // fan-out (`d273`) is not something any other read takes.
        "find" => &["json", "everywhere"],
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
        // `d723` piece A / `f721`: every run, flagged or bare, walks the
        // same tree path `setup` already walked through `tree.rs`
        // (`setup::init`). A bare run used to take an old, separate path
        // straight through `Store::create`, which never called
        // `tree::plan` and so never ran the guard against two trees of one
        // product either -- planting quietly succeeded where the flagged
        // path already refused. One path only, so which flags a run
        // carries can no longer decide whether that guard exists.
        //
        // `t594` keeps one guard of its own ahead of this dispatch: a
        // folder whose own `.vivac/lane` cannot be resolved to any tree --
        // exactly what `relocate` leaves the origin holding -- reaches
        // `store::locate` inside `resolve_roots` and fails there with
        // `TreeNotFound`'s generic sentence and exit code, shared with
        // every other command that loses a lane's tree. `init` alone kept
        // a sharper answer and its own exit 1 before this piece, and
        // still does: a lane that resolves fine, to its own tree or to
        // another one, carries on into `setup::init` unchanged, which is
        // where both of those are decided now.
        if let Some(lane) = lane::read(&cwd.join(store::DIR))? {
            let is_own_tree = store::already_planted(&cwd)
                && store::first_event_id(&cwd).as_deref() == Some(lane.project.as_str());
            if !is_own_tree && matches!(store::locate(&cwd), Err(Failure::TreeNotFound(_))) {
                return Err(Failure::already_a_lane());
            }
        }
        return setup::init(&cwd, a);
    }

    // A tombstone, not a plain unknown command: `vivac hooks` is gone
    // (`d557`), and whoever has it written down in a note somewhere still
    // gets sent to where it went, instead of just "unknown command".
    if cmd == "hooks" {
        return Err(Failure::usage(
            "vivac hooks is gone: vivac setup claude-code writes the hooks itself,\n  \
             after showing them.",
        ));
    }

    // `setup` may need to plant the tree, the same reason `init` returns up
    // here: there may be no root at all yet, and its own root search knows
    // to fall back to the current directory (`t565` §7.2).
    if cmd == "setup" {
        return setup::dispatch(&cwd, a);
    }

    // `--everywhere` reads the registry instead of the tree underfoot, so
    // it has to work with no root at all -- the same reason `init` and
    // `setup` return up here rather than past the check below. `d273`,
    // first half.
    if cmd == "find" && a.has("everywhere") {
        return render::find_everywhere(a).map(|_| 0);
    }

    // `web` opens from any directory (`d199`), so it returns up here for the
    // same reason `find --everywhere` does: its roots come from the registry
    // rather than from the tree underfoot, and there may be no tree underfoot
    // at all. What the working directory still decides is where `/` lands.
    //
    // Same reason as `mcp` for building its own registry, over one or more
    // roots instead of one: the server outlives this call, so it cannot take
    // the `ctx` below.
    if cmd == "web" {
        let explicit: Vec<std::path::PathBuf> = a
            .list("project")
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let located_here = store::locate(&cwd)?;
        let cwd_root = located_here.as_ref().map(|l| l.root.clone());
        let roots = if explicit.is_empty() {
            // The registry is where "every project on this machine" is
            // written down. The one underfoot can still be missing from it --
            // a tree whose first use since the registry existed is this very
            // command -- so it goes in unconditionally; `Registry::open`
            // collapses the duplicate by canonical path.
            let mut from_registry = store::store_dir()
                .map(|d| registry::roots(&d))
                .unwrap_or_default();
            from_registry.extend(cwd_root.clone());
            from_registry
        } else {
            explicit
        };
        let port =
            match a.opt("port") {
                None => None,
                Some(p) => Some(p.parse::<u16>().map_err(|_| {
                    Failure::usage(format!("--port needs a port number, not \"{p}\""))
                })?),
            };
        return web::serve(roots, located_here, port, !a.has("no-open")).map(|_| 0);
    }

    // `session prompt` (`d779`) is intercepted here, ahead of `store::locate`'s
    // own `?`: it runs on every message a person sends, and it must never
    // fail that turn. No tree, a `locate` that itself errors, a log this
    // version cannot read -- everything downstream of this line reads the
    // same as "nothing to say", so none of it is allowed to become a
    // non-zero exit the way it would for every other command.
    // Every hook drains its input first, whatever it does next: exiting
    // with the harness's payload unread leaves the harness writing into a
    // closed pipe.
    if cmd == "session" && a.has("hook") {
        session::hook_stdin();
    }
    if cmd == "session" && a.positional(0) == Some("prompt") {
        session::prompt(&cwd, a);
        return Ok(0);
    }

    let Some(located) = store::locate(&cwd)? else {
        // The hooks stay quiet where there is no tree. One that fails in
        // every unrelated directory gets switched off within two days, and
        // the two that matter go with it.
        if cmd == "session" && a.has("hook") {
            return Ok(0);
        }
        return Err(Failure::NoStore);
    };
    let root = located.root.clone();
    // A side effect of using a project, not a step of any one command: every
    // command past this point runs once per process, so this is where the
    // registry learns where the project lives. It never fails the command
    // that triggered it -- `registry::note` swallows its own errors.
    //
    // A tree that was just planted has no first event, so nothing to be keyed
    // by, and this used to end there. It cannot: the command about to run is
    // often the one that writes that first event, so the project stayed out of
    // the registry -- and out of `find --everywhere` -- until whatever came
    // next, without saying so. `f277`. So the root is kept and tried again on
    // the way out, where the event exists.
    if let Some(store_dir) = store::store_dir() {
        let lane = located
            .lane
            .as_ref()
            .map(|l| (l.id.clone(), located.lane_dir.clone()));
        match store::first_event_id(&root) {
            Some(project_id) => {
                let noted = registry::note(
                    &store_dir,
                    &project_id,
                    registry::Sighting {
                        root: &root,
                        lane: lane.as_ref().map(|(id, dir)| (id.as_str(), dir.as_path())),
                        repos: None,
                    },
                );
                // Left for `warn_if_wrote` to decide, once this command is
                // done running and can say whether it actually wrote
                // anything (`t594`) -- never here, where
                // nothing has written yet no matter which verb this is.
                registry::set_pending(noted);
            }
            None => {
                let _ = LATE_SIGHTING.set((root.clone(), lane));
            }
        }
    }
    // `relocate` moves data outright rather than updating or undoing a
    // step, the shapes the rest of this dispatch is built around, so it
    // manages its own store and its own lock instead of going through
    // `Ctx`. That is also why it stays out of MCP (`t594` §4.6): every
    // tool there either reads or undoes one step, and this does neither.
    if cmd == "relocate" {
        if let [first, ..] = a.extra(1) {
            return Err(Failure::usage(format!(
                "{cmd} does not take \"{first}\".

  It takes one word of its own. Everything else goes behind a --flag, and a flag
  that repeats is written out again:  --governs a --governs b"
            )));
        }
        let destination = match a.positional(0) {
            Some(d) if !d.is_empty() => d,
            _ => {
                return Err(Failure::usage(
                    "relocate needs a destination: vivac relocate <destination>",
                ))
            }
        };
        return relocate::run(
            &located,
            std::path::Path::new(destination),
            a.opt("lane-name"),
            &cwd,
        );
    }
    // The server outlives its calls and it is not the only writer, so it
    // loads the tree itself and reloads it when the log moves. Everything
    // below assumes one command, one process, one fold.
    if cmd == "mcp" {
        return mcp::serve(root, Some(located)).map(|_| 0);
    }

    // Its own load, ahead of the generic one below, for the same reason as
    // `mcp` and `web`: the generic one folds the log and drops the events,
    // and `changes` is the one command that needs them back. Reading here
    // and again below would read the log twice for nothing.
    if cmd == "changes" {
        let (ctx, log) =
            ops::Ctx::load_with_log(store::Store::open(root)?, ops::Whose::Resolved(&located))?;
        return changes::changes(&ctx.tree, &log, a);
    }

    // `why`'s plain read goes through the derived index again, the same as
    // every other read: `t594` §5.4's "born in lane" line reads straight off
    // the node it is asked about now (`Node::born_seq`, `Node::born_lane`),
    // and only `--full`'s own three fields -- `anchor`, `standing`,
    // `open_then` -- still need the whole log folded (`Full::from_log`), for
    // `open_then`'s question about a moment in the past that closing has
    // already folded away. `changes` above is unrelated: it asks about the
    // events themselves, not about the tree they fold into, so it always
    // reads the log regardless. The "one word of its own" limit is checked
    // here rather than left to the generic path below, so `vivac why t1
    // extra` still refuses it exactly as it always has -- after the load, so
    // a store that cannot be opened is still reported before a usage error
    // that was already true beforehand.
    if cmd == "why" {
        let extra_word = |a: &Args| -> Result<(), Failure> {
            if let [first, ..] = a.extra(1) {
                return Err(Failure::usage(format!(
                    "{cmd} does not take \"{first}\".

  It takes one word of its own. Everything else goes behind a --flag, and a flag
  that repeats is written out again:  --governs a --governs b"
                )));
            }
            Ok(())
        };
        // `--project` answers from another tree entirely: `d273`'s second
        // half. It resolves against the registry the same way
        // `find --everywhere` reads it, and the tree loads through the
        // local index with `allow_persist: false` for the same reason --
        // opening another project's node must never write inside that
        // project's `.vivac/`. `--full` needs the raw log (`Full::from_log`)
        // and a foreign log is never read that way, so the two refuse each
        // other instead of `--full` silently answering half its question.
        if let Some(spec) = a.opt("project") {
            let foreign_root = registry::resolve(spec)?;
            // No `for_lane`: this folder is not a lane of the foreign
            // tree, so it reads that tree's founding lane, the same as
            // any tree nobody ran `setup` in. Defensible and not a lie
            // today; it stops being one the day a foreign tree has a
            // second lane (`t594`).
            let tree = index::load(&store::Store::open(foreign_root)?, false)?;
            extra_word(a)?;
            if a.has("full") {
                return Err(Failure::usage(
                    "why --project does not take --full: --full reads the whole log, \
                     and a foreign project's log is never read that way.

  Drop --full or drop --project."
                        .to_string(),
                ));
            }
            return render::why(&tree, &[], a).map(|_| 0);
        }
        if a.has("full") {
            let (ctx, log) =
                ops::Ctx::load_with_log(store::Store::open(root)?, ops::Whose::Resolved(&located))?;
            extra_word(a)?;
            return render::why(&ctx.tree, &log, a).map(|_| 0);
        }
        let ctx = ops::Ctx::load(store::Store::open(root)?, ops::Whose::Resolved(&located))?;
        extra_word(a)?;
        return render::why(&ctx.tree, &[], a).map(|_| 0);
    }

    // `may_append` is checked here, once, rather than passed down: it is
    // exactly the set `write_op` below already dispatches, plus `session`
    // and `import`, which append through a path of their own. `LOADING.md`
    // §4, on when the index is rewritten: a command that might write never pays
    // to rewrite the derived index, even though reading a warm or stale one
    // stays free either way.
    let mut ctx = if may_append(cmd) {
        ops::Ctx::load_for_write(store::Store::open(root)?, ops::Whose::Resolved(&located))?
    } else {
        ops::Ctx::load(store::Store::open(root)?, ops::Whose::Resolved(&located))?
    };

    // `check` is the only one with an exit code of its own: it separates
    // store corruption from a finding about the project.
    if cmd == "check" {
        return check::check(&ctx.tree, &ctx.store.root, a);
    }

    // Words each command takes of its own. Anything past that is refused for
    // the same reason an unknown flag is: the table above only ever covered
    // half the command line, and the other half went through in silence
    // (`f52`).
    let takes: usize = match cmd {
        "park" | "abandon" | "done" | "note" | "flag" | "arm" => 2,
        "focus" | "push" | "pop" | "promote" | "add" | "block" | "decide" | "save" | "restore"
        | "import" | "tree" | "session" | "find" | "declare" => 1,
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

    // `d598`: a command that may append holds the tree's write lock from
    // here until it returns, and writes against the tree as it is once the
    // lock is held. It is taken after the command line is checked, so a
    // usage error never waits on another writer. `session` and `restore`
    // take their own: session once the brief is out, so a held lock never
    // costs the agent its brief, and restore once git has answered, so a
    // slow git never holds every other writer.
    if may_append(cmd) && !matches!(cmd, "session" | "restore") {
        ctx.lock_for_write()?;
    }

    if let Some(o) = write_op(cmd, &mut ctx, a)? {
        print!("{}", outcome::to_text(&o));
        // Trap: `focus` and `restore` used to end by delegating to
        // `render::stack`, which reads `--json` off `a` on its own. Neither
        // is allowed `--json` in the table above, so that branch was never
        // reachable from either call site; the call just moves here, right
        // after the `Outcome` each one now returns is printed.
        if matches!(cmd, "focus" | "restore") {
            render::stack(&ctx.tree, &ctx.store.root, a)?;
        }
        return Ok(0);
    }

    let r: failure::R = match cmd {
        "import" => import::import(&mut ctx, a),
        "brief" => {
            let project = project_name(&ctx);
            brief::brief(&ctx.tree, &ctx.store.root, &ctx.lane_dir, a, &project)
        }
        "vivacs" => render::vivacs(&ctx.tree, a),
        "session" => {
            let project = project_name(&ctx);
            session::dispatch(&mut ctx, a, &project, &located)
        }
        "tree" => render::tree(&ctx.tree, a),
        "open" => render::open(&ctx.tree, a),
        "rules" => render::rules(&ctx.tree, a),
        "find" => render::find(&ctx.tree, a),
        "stack" => render::stack(&ctx.tree, &ctx.store.root, a),
        "parked" => render::parked(&ctx.tree, a),
        "triage" => render::triage(&ctx.tree, a),
        "reconcile" => reconcile::reconcile(&ctx.tree, &ctx.store.root, &ctx.lane_dir, a),
        "stats" => render::stats(&ctx.tree, a),
        other => {
            print!("{USAGE}");
            return Err(Failure::usage(format!("unknown command: {other}")));
        }
    };
    r.map(|_| 0)
}

/// Whether `cmd` might append to the log this run: the fifteen names
/// `write_op` below matches, plus `session` (a hook can write an opening or
/// an automatic stop) and `import` (writes the events it brings in). Every
/// other command only ever reads.
fn may_append(cmd: &str) -> bool {
    matches!(
        cmd,
        "push"
            | "pop"
            | "done"
            | "park"
            | "add"
            | "note"
            | "block"
            | "arm"
            | "promote"
            | "abandon"
            | "focus"
            | "flag"
            | "decide"
            | "declare"
            | "save"
            | "restore"
            | "session"
            | "import"
    )
}

/// The fifteen write operations, matched once so that printing an `Outcome`
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
        "arm" => ops::arm(ctx, params::Arm::from_args(a)?)?,
        "promote" => ops::promote(ctx, params::Promote::from_args(a)?)?,
        "abandon" => ops::abandon(ctx, params::Abandon::from_args(a)?)?,
        "focus" => ops::focus(ctx, params::Focus::from_args(a)?)?,
        "flag" => ops::flag(ctx, params::Flag::from_args(a)?)?,
        "decide" => ops::decide(ctx, params::Decide::from_args(a)?)?,
        "declare" => ops::declare(ctx, params::Declare::from_args(a)?)?,
        "save" => ops::save(ctx, params::Save::from_args(a)?)?,
        "restore" => ops::restore(ctx, params::Restore::from_args(a)?)?,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, USAGE};
    use crate::failure::Failure;
    use crate::redact;
    use std::collections::BTreeSet;

    /// Every command `USAGE` actually shows, parsed rather than
    /// hand-copied: a line whose trimmed text starts with `vivac ` names one
    /// in the word right after it. Lines that only mention `vivac` in
    /// passing -- "vivac never runs it", the exit-code sentence about "a
    /// tree written by a newer vivac" -- do not start that way, and drop
    /// out on their own.
    fn commands_in_usage(help: &str) -> BTreeSet<&str> {
        help.lines()
            // The title line, `vivac - provenance of work`, sits at column
            // zero and would otherwise read as a command named `-`; every
            // real example is indented under one of the sections below it.
            .filter(|l| l.starts_with(' '))
            .filter_map(|l| l.trim_start().strip_prefix("vivac "))
            .filter_map(|rest| rest.split_whitespace().next())
            .collect()
    }

    /// `d772`: `COMMANDS` is what `unknown_command` checks a typed command
    /// against, and `USAGE` is what a person reads to learn the real ones.
    /// The two have to agree in both directions, or a command could be
    /// refused as unknown while `USAGE` still lists it, or dispatched while
    /// nobody reading `USAGE` would know it exists.
    #[test]
    fn commands_and_usage_stay_in_sync() {
        let listed = commands_in_usage(USAGE);
        let declared: BTreeSet<&str> = COMMANDS.iter().copied().collect();
        assert_eq!(
            listed, declared,
            "COMMANDS and USAGE's own `vivac <command>` lines have drifted."
        );
    }

    /// `d772`: a hint is one line of an answer an agent reads in a terminal,
    /// so it keeps to the 76 columns every other line of help does. The
    /// `close` hint first came in at 83.
    #[test]
    fn every_unknown_command_hint_fits_76_columns() {
        for (_, hint) in super::COMMAND_HINTS {
            assert!(
                hint.chars().count() <= 76,
                "a hint is {} columns wide: {hint:?}",
                hint.chars().count()
            );
        }
    }

    /// Every number in the `Exit codes` block of the help text: the codes
    /// this binary claims to be able to return. Parsed rather than
    /// hand-copied, so the check below breaks the moment the block and the
    /// code drift apart, not the moment somebody happens to read both and
    /// disagree.
    fn exit_codes_in_usage(help: &str) -> BTreeSet<i32> {
        let block = help
            .split_once("Exit codes")
            .expect("USAGE lost its `Exit codes` section")
            .1;
        block
            .split_whitespace()
            .filter_map(|word| word.parse().ok())
            .collect()
    }

    /// One instance of every `Failure` variant, and the code each one
    /// actually returns. The instances are matched with no catch-all arm on
    /// purpose: a variant added to `Failure` and left out of this array
    /// still compiles the array itself, but the match right below refuses to
    /// build until the new variant is given a line here too. `Redaction`'s
    /// sample comes from `redact::check_field` rather than being built by
    /// hand, so it is a real `Finding` and not a guess at the shape of one.
    fn exit_codes_failure_can_return() -> BTreeSet<i32> {
        let finding = redact::check_field("token", "ghp_16C7e42F292c6912E7710c838347Ae178B4a")
            .expect("a known credential prefix, refused by redact.rs's own tests too");
        let variants = [
            Failure::Model("the parent still has an open blocker".into()),
            Failure::usage("unknown command: bogus"),
            Failure::Redaction(Box::new(finding)),
            Failure::NoStore,
            Failure::SetupNoTree,
            Failure::Io(std::io::Error::other("disk full")),
            Failure::newer_vivac("this log holds an event this version does not know"),
            Failure::busy(std::time::Duration::from_secs(5)),
            Failure::not_a_lane(),
            Failure::tree_not_found(),
        ];
        variants
            .into_iter()
            .map(|f| match &f {
                Failure::Model(_) => f.code(),
                Failure::Usage(_) => f.code(),
                Failure::Redaction(_) => f.code(),
                Failure::NoStore => f.code(),
                Failure::SetupNoTree => f.code(),
                Failure::Io(_) => f.code(),
                Failure::NewerVivac(_) => f.code(),
                Failure::Busy(_) => f.code(),
                Failure::NotALane(_) => f.code(),
                Failure::TreeNotFound(_) => f.code(),
            })
            .collect()
    }

    /// The two have to agree in both directions: no code `Failure::code` can
    /// return left off the help, and no code on the help that nothing
    /// returns. `0` is added by hand rather than through the match above: it
    /// is the exit code of success, and success is not a `Failure` variant.
    /// `f462`: `Io` has returned 5 since it existed, and the help never said
    /// so.
    #[test]
    fn usage_lists_every_exit_code_failure_can_return() {
        let listed = exit_codes_in_usage(USAGE);
        let mut real = exit_codes_failure_can_return();
        real.insert(0);
        assert_eq!(
            listed, real,
            "USAGE's `Exit codes` block and what `Failure::code` can return \
             (plus 0, for success) have drifted."
        );
    }
}
