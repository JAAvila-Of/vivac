//! The tree as tools an agent can call: an MCP server over standard input.
//!
//! **No new dependency.** MCP over stdio is JSON-RPC 2.0 in newline delimited
//! JSON, and `serde_json` was already here. No runtime, no HTTP, no second
//! crate: the server lives in the binary that is already installed.
//!
//! ```text
//! claude mcp add vivac -- vivac mcp
//! ```
//!
//! **Standard output is the protocol.** Every other command in this crate
//! prints as it goes; this one cannot. A `println!` on a path the server
//! touches is not untidy, it is a malformed frame and the client hangs up. So
//! the reads are called through their builders --`find_data`, `why_data`,
//! `open_data`, `to_text`-- which return the answer instead of printing it.
//! `tests/mcp.rs` guards that with a test that reads every line back.
//!
//! **What this is not.** `INTEGRATION.md` §4 is blunt about it: MCP tools are
//! voluntary, and an agent under task pressure does not call them. So this
//! does not fix the capture problem, and it is not offered as a fix. What it
//! answers is `d100`: the memory store this replaces is reachable over MCP,
//! and taking its place means being reachable through the same door, in the
//! tool list, with a schema.
//!
//! **Four reads, seven writes, eleven tools.** The reads answer `brief`,
//! `find`, `why` and `open`. The writes are `push`, `pop`, `add`, `decide`,
//! `note`, `park` and `save` -- the seven `t106` already turned into
//! functions that hand back an `Outcome` instead of printing one, so this is
//! the second caller that reads the same answer the CLI does.
//!
//! **What stays out, and why.** `abandon` discards a node and every
//! descendant it has; reachable from a tool call, that would happen with
//! nobody watching a terminal, and the security pillar vetoes it outright.
//! `restore` rewrites the stack and sits on the same side of that line.
//! `done`, `block`, `flag`, `promote` and `focus` are maintainer surgery, and
//! the maintainer has a terminal.

use crate::args::Args;
use crate::failure::{Failure, R};
use crate::project::{Project, Registry};
use crate::store::Store;
use crate::{brief, ops, outcome, params, render};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The version spoken when the client does not name one.
const PROTOCOL: &str = "2025-06-18";

/// What JSON shape an argument's value takes. Every tool used to need only
/// `string`, but `blocks` is a flag and `ref`/`governs`/`alternative` repeat,
/// so `schema` below has two more shapes to say.
enum ArgKind {
    Str,
    Bool,
    List,
}

struct Arg {
    name: &'static str,
    kind: ArgKind,
    required: bool,
    description: &'static str,
}

struct Tool {
    /// `vivac_<command>`, and the suffix is not decoration: it is the CLI
    /// command this mirrors. `INTEGRATION.md` §8 listed five tools --`ask`,
    /// `answer`, `assume`, `verify`, `refute`-- that no command implements,
    /// and a function reachable from one surface only leaves half the
    /// audience outside it, which the DX pillar refuses by name. The test
    /// `every_tool_is_a_command_the_cli_already_has` keeps the two honest.
    name: &'static str,
    description: &'static str,
    args: &'static [Arg],
}

/// Eleven, and the number is a budget rather than a stage of growth: every
/// tool here costs context in every session the agent ever opens. The other
/// seven write ops -- `done`, `block`, `promote`, `abandon`, `focus`, `flag`,
/// `restore` -- stay off this list on purpose; see the module doc.
const TOOLS: &[Tool] = &[
    Tool {
        name: "vivac_brief",
        description: "Where you are in this project and what NOT to touch right now: \
                      the focus with its lineage, the parked nodes with the reason each \
                      was parked for, the decisions that still govern, and the last safe \
                      point with what you were about to do. Read it before anything else \
                      when a session opens.",
        args: &[],
    },
    Tool {
        name: "vivac_find",
        description: "Search the provenance tree. Returns every node whose title, reason, \
                      note or outcome contains all of the terms, newest first, each with \
                      the lineage it hangs from. Closed nodes are included: what you look \
                      for months later is usually finished.",
        args: &[Arg {
            name: "query",
            kind: ArgKind::Str,
            required: true,
            description: "Words to look for. Every one of them has to appear.",
        }],
    },
    Tool {
        name: "vivac_why",
        description: "Why a node exists: the chain from the goal down to it, what is open \
                      in parallel, what was born from it, and what blocks it from closing. \
                      This is the question the whole tool exists to answer.",
        args: &[Arg {
            name: "id",
            kind: ArgKind::Str,
            required: true,
            description: "The node as the tree names it: g1, t12, f74, d29.",
        }],
    },
    Tool {
        name: "vivac_open",
        description: "The open fronts of this project, each with its lineage: what is \
                      actually unfinished, rather than everything that was ever written \
                      down.",
        args: &[],
    },
    Tool {
        name: "vivac_push",
        description: "Open a node and step into it: it becomes the focus, and everything \
                      captured next hangs from it until a matching pop. Call it the moment \
                      work forks away from the current line -- a question that has to be \
                      settled before continuing, a detour worth its own trace -- never \
                      after the fact, once the reason for taking it has already faded. \
                      `why` is mandatory for exactly that reason: a detour with no reason \
                      recorded is the failure this tree exists to catch.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "What this node is, in a few words.",
            },
            Arg {
                name: "why",
                kind: ArgKind::Str,
                required: true,
                description: "Why this is happening now. A detour with no reason is what \
                              this field exists to prevent.",
            },
            Arg {
                name: "type",
                kind: ArgKind::Str,
                required: false,
                description: "goal, task, decision, question, constraint, finding or \
                              assumption. Defaults to goal at the root, task otherwise.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this node is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this node's work is expected to touch.",
            },
        ],
    },
    Tool {
        name: "vivac_pop",
        description: "Close the current focus and step back to its parent, recording what \
                      came of it. Call it once the work `vivac_push` opened is actually \
                      finished, not on a whim to clear the stack: a node with open closure \
                      conditions refuses to close on its own, because a run that closes \
                      with its findings still open is exactly the mistake that refusal \
                      exists to catch.",
        args: &[
            Arg {
                name: "outcome",
                kind: ArgKind::Str,
                required: false,
                description: "What happened. Read back later, so leaving it out costs \
                              the next reader the point of the node.",
            },
            Arg {
                name: "next",
                kind: ArgKind::Str,
                required: false,
                description: "What comes after, when it differs from the outcome.",
            },
            Arg {
                name: "force",
                kind: ArgKind::Bool,
                required: false,
                description: "Close anyway, over open closure conditions. Leaves a trace \
                              that it happened.",
            },
        ],
    },
    Tool {
        name: "vivac_add",
        description: "File a node without touching the stack: the focus stays exactly \
                      where it was. Use it for something that belongs in the tree but is \
                      not the next thing about to happen -- a finding surfaced while \
                      working on something else, a sibling task filed for later, a piece \
                      of an existing structure being brought in. `vivac_push` is for what \
                      comes next; this is for what was just noticed.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "What this node is, in a few words.",
            },
            Arg {
                name: "parent",
                kind: ArgKind::Str,
                required: false,
                description: "The node it hangs from. Defaults to the current focus, or \
                              the root if there is none.",
            },
            Arg {
                name: "why",
                kind: ArgKind::Str,
                required: false,
                description: "Why this matters.",
            },
            Arg {
                name: "type",
                kind: ArgKind::Str,
                required: false,
                description: "goal, task, decision, question, constraint, finding or \
                              assumption. Defaults to goal at the root, task otherwise.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this node is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this node's work is expected to touch.",
            },
        ],
    },
    Tool {
        name: "vivac_decide",
        description: "Record a decision, with the reason it was made and every alternative \
                      that lost. Call it the moment a choice is actually settled, not \
                      before and not long after: the alternatives are optional in the \
                      schema and not in practice, because without them the same option \
                      gets proposed again in a month by whoever was not in the room.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "The decision, in a few words.",
            },
            Arg {
                name: "reason",
                kind: ArgKind::Str,
                required: true,
                description: "Why this and not something else. A decision with no reason \
                              is a datum, not a decision.",
            },
            Arg {
                name: "parent",
                kind: ArgKind::Str,
                required: false,
                description: "The node it hangs from. Defaults to the current focus, or \
                              the root if there is none.",
            },
            Arg {
                name: "alternative",
                kind: ArgKind::List,
                required: false,
                description: "An option that was ruled out. Repeat for each one.",
            },
            Arg {
                name: "supersedes",
                kind: ArgKind::Str,
                required: false,
                description: "An earlier decision this one retires.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this decision is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this decision's work is expected to touch.",
            },
        ],
    },
    Tool {
        name: "vivac_note",
        description: "Attach a fact to a node without changing its state or the stack: \
                      something worth keeping that is not itself a new node. Call it \
                      beside `vivac_push` and `vivac_pop` for anything that would otherwise \
                      only live in a chat transcript nobody rereads.",
        args: &[
            Arg {
                name: "note",
                kind: ArgKind::Str,
                required: true,
                description: "The fact to attach.",
            },
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: false,
                description: "The node to attach it to. Defaults to the current focus.",
            },
        ],
    },
    Tool {
        name: "vivac_park",
        description: "Suspend a node without abandoning it: it drops off the stack and \
                      becomes something a later session is told not to touch until \
                      whatever parked it is resolved. Call it when work is genuinely \
                      stuck on something outside this session, not as a substitute for \
                      `vivac_pop` on something that is simply finished.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: false,
                description: "The node to park. Defaults to the current focus.",
            },
            Arg {
                name: "reason",
                kind: ArgKind::Str,
                required: false,
                description: "Why it is stuck. Read back verbatim under DO NOT TOUCH NOW.",
            },
        ],
    },
    Tool {
        name: "vivac_save",
        description: "A deliberate safe stop: a label for this point and what was about to \
                      happen next, so a session that picks the thread back up -- this one \
                      later, or someone else's -- starts exactly where this one left off \
                      instead of guessing from the log.",
        args: &[
            Arg {
                name: "label",
                kind: ArgKind::Str,
                required: false,
                description: "A short name for this stop.",
            },
            Arg {
                name: "next",
                kind: ArgKind::Str,
                required: false,
                description: "What was about to happen next.",
            },
        ],
    },
];

fn schema(t: &Tool) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<&str> = Vec::new();
    for a in t.args {
        let mut entry = serde_json::Map::new();
        match a.kind {
            ArgKind::Str => {
                entry.insert("type".to_string(), json!("string"));
            }
            ArgKind::Bool => {
                entry.insert("type".to_string(), json!("boolean"));
            }
            ArgKind::List => {
                entry.insert("type".to_string(), json!("array"));
                entry.insert("items".to_string(), json!({ "type": "string" }));
            }
        }
        entry.insert("description".to_string(), json!(a.description));
        properties.insert(a.name.to_string(), Value::Object(entry));
        if a.required {
            required.push(a.name);
        }
    }
    json!({
        "name": t.name,
        "description": t.description,
        "inputSchema": {
            "type": "object",
            "properties": Value::Object(properties),
            "required": required,
        },
    })
}

fn ok(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn rpc_error(id: &Value, code: i32, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A refusal the model can read and act on.
///
/// It is a successful frame with `isError` raised, not a JSON-RPC error: a
/// protocol error is for the client and the model never sees it, and "no such
/// node: t999" is exactly the kind of thing the model has to see to fix its
/// own next call.
fn tool_error(id: &Value, message: String) -> String {
    ok(
        id,
        json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
    )
}

fn tool_ok(id: &Value, text: String) -> String {
    ok(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
    )
}

fn pretty(v: Value) -> Result<String, Failure> {
    serde_json::to_string_pretty(&v).map_err(|e| Failure::Io(std::io::Error::other(e)))
}

fn argument<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params["arguments"][name].as_str()
}

fn bool_argument(params: &Value, name: &str) -> bool {
    params["arguments"][name].as_bool().unwrap_or(false)
}

fn list_argument(params: &Value, name: &str) -> Vec<String> {
    params["arguments"][name]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Fresh for every write call, on its own store handle rather than
/// `Project`'s cached fold: `Ctx::load_for_write` is the one a write is
/// allowed to pay for, because unlike `Ctx::load` it never refreshes the
/// derived index (`LOADING.md` §4 -- a write must never pay to rewrite it).
/// `Project`'s cached tree picks the change up on its own next read, the same
/// way it already does for a write that lands through the CLI while the
/// server keeps running.
fn write_ctx(root: &Path) -> Result<ops::Ctx, Failure> {
    ops::Ctx::load_for_write(Store::open(root.to_path_buf())?)
}

/// Serialised the same way the three reads that speak JSON already are:
/// `pretty` over a `Value`, so the model gets back data it can parse rather
/// than the sentence `outcome::to_text` writes for a terminal.
fn outcome_text(o: outcome::Outcome) -> Result<String, Failure> {
    pretty(serde_json::to_value(&o).map_err(|e| Failure::Io(std::io::Error::other(e)))?)
}

fn call(project: &mut Project, params: &Value) -> Result<String, Failure> {
    let name = params["name"].as_str().unwrap_or_default();
    let missing = |what: &str| Failure::usage(format!("{name} needs a {what}."));
    match name {
        "vivac_brief" => {
            let empty = Args::default();
            let name = project.name.clone();
            let ctx = project.current()?;
            brief::to_text(&ctx.tree, ctx.anchor.as_ref(), &empty, &name)
        }
        "vivac_find" => {
            let query = argument(params, "query")
                .ok_or_else(|| missing("query"))?
                .to_string();
            pretty(render::find_data(&project.current()?.tree, &query)?)
        }
        "vivac_why" => {
            let id = argument(params, "id")
                .ok_or_else(|| missing("id"))?
                .to_string();
            pretty(render::why_data(&project.current()?.tree, &id)?)
        }
        "vivac_open" => pretty(render::open_data(&project.current()?.tree)),
        "vivac_push" => {
            let title = argument(params, "title")
                .ok_or_else(|| missing("title"))?
                .to_string();
            let why = argument(params, "why")
                .ok_or_else(|| missing("why"))?
                .to_string();
            let p = params::Push {
                title,
                why,
                kind: argument(params, "type").map(str::to_string),
                refs: list_argument(params, "ref"),
                governs: list_argument(params, "governs"),
                blocks: bool_argument(params, "blocks"),
            };
            outcome_text(ops::push(&mut write_ctx(&project.root)?, p)?)
        }
        "vivac_pop" => {
            let p = params::Pop {
                outcome: argument(params, "outcome").unwrap_or("").to_string(),
                next: argument(params, "next").map(str::to_string),
                force: bool_argument(params, "force"),
            };
            outcome_text(ops::pop(&mut write_ctx(&project.root)?, p)?)
        }
        "vivac_add" => {
            let title = argument(params, "title")
                .ok_or_else(|| missing("title"))?
                .to_string();
            let p = params::Add {
                title,
                parent: argument(params, "parent").map(str::to_string),
                kind: argument(params, "type").map(str::to_string),
                why: argument(params, "why").unwrap_or("").to_string(),
                refs: list_argument(params, "ref"),
                governs: list_argument(params, "governs"),
                blocks: bool_argument(params, "blocks"),
            };
            outcome_text(ops::add(&mut write_ctx(&project.root)?, p)?)
        }
        "vivac_decide" => {
            let title = argument(params, "title")
                .ok_or_else(|| missing("title"))?
                .to_string();
            let reason = argument(params, "reason")
                .ok_or_else(|| missing("reason"))?
                .to_string();
            let p = params::Decide {
                title,
                parent: argument(params, "parent").map(str::to_string),
                reason,
                alternatives: list_argument(params, "alternative"),
                supersedes: argument(params, "supersedes").map(str::to_string),
                refs: list_argument(params, "ref"),
                governs: list_argument(params, "governs"),
                blocks: bool_argument(params, "blocks"),
            };
            outcome_text(ops::decide(&mut write_ctx(&project.root)?, p)?)
        }
        // `id` given: the two words are unambiguous, the way `vivac note <id>
        // "<note>"` is. `id` left out: the note text takes the place a lone
        // positional would on the CLI, so `ops::note` attaches it to the
        // focus the same way `vivac note "<note>"` does.
        "vivac_note" => {
            let text = argument(params, "note")
                .ok_or_else(|| missing("note"))?
                .to_string();
            let p = match argument(params, "id") {
                Some(id) => params::Note {
                    node: Some(id.to_string()),
                    note: Some(text),
                },
                None => params::Note {
                    node: Some(text),
                    note: None,
                },
            };
            outcome_text(ops::note(&mut write_ctx(&project.root)?, p)?)
        }
        // Same shape as `vivac_note`: with no `id`, `reason` takes the place
        // of the single word `vivac park "<reason>"` would pass, and
        // `named_or_focus` is what resolves it against the focus.
        "vivac_park" => {
            let p = match (argument(params, "id"), argument(params, "reason")) {
                (Some(id), Some(reason)) => params::Park {
                    node: Some(id.to_string()),
                    reason: Some(reason.to_string()),
                },
                (Some(id), None) => params::Park {
                    node: Some(id.to_string()),
                    reason: None,
                },
                (None, Some(reason)) => params::Park {
                    node: Some(reason.to_string()),
                    reason: None,
                },
                (None, None) => params::Park {
                    node: None,
                    reason: None,
                },
            };
            outcome_text(ops::park(&mut write_ctx(&project.root)?, p)?)
        }
        "vivac_save" => {
            let p = params::Save {
                label: argument(params, "label").unwrap_or("").to_string(),
                next: argument(params, "next").unwrap_or("").to_string(),
            };
            outcome_text(ops::save(&mut write_ctx(&project.root)?, p)?)
        }
        other => Err(Failure::usage(format!(
            "no such tool: {other}. This server has: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// One line in, at most one line out. `None` is a notification, which by
/// definition is not answered: a reply nobody is waiting for would be read as
/// the answer to whatever comes next.
fn handle(project: &mut Project, line: &str) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(
                &Value::Null,
                -32700,
                &format!("that line is not JSON: {e}"),
            ))
        }
    };
    let id = message.get("id").cloned()?;
    let method = message["method"].as_str().unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => {
            let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            Some(ok(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "vivac", "version": env!("CARGO_PKG_VERSION") },
                }),
            ))
        }
        "ping" => Some(ok(&id, json!({}))),
        "tools/list" => Some(ok(
            &id,
            json!({ "tools": TOOLS.iter().map(schema).collect::<Vec<_>>() }),
        )),
        "tools/call" => Some(match call(project, &params) {
            Ok(text) => tool_ok(&id, text),
            Err(e) => tool_error(&id, e.message()),
        }),
        other => Some(rpc_error(
            &id,
            -32601,
            &format!("this server does not do {other}"),
        )),
    }
}

pub fn serve(root: PathBuf) -> R {
    let mut registry = Registry::open(vec![root])?;
    let project = registry.first();
    let input = std::io::stdin();
    let mut output = std::io::stdout();
    for line in input.lock().lines() {
        let line = line.map_err(Failure::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(project, &line) {
            writeln!(output, "{reply}").map_err(Failure::Io)?;
            output.flush().map_err(Failure::Io)?;
        }
    }
    Ok(())
}
