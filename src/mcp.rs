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
//! **Standard output is the protocol.** A `println!` on a path the server
//! touches is not untidy, it is a malformed frame and the client hangs up. So
//! the reads are called through their builders --`find_data`, `why_data`,
//! `open_data`, `to_text`-- which return the answer instead of printing it,
//! and the server decides what reaches the wire. `tests/mcp.rs` guards that
//! with a test that reads every line back. This used to be the one place in
//! the crate that could not print as it goes; since `output` became the only
//! owner of standard output there is no other, and `tests/no_println.rs`
//! keeps it that way for reasons of its own.
//!
//! **What this is not.** `INTEGRATION.md` §4 is blunt about it: MCP tools are
//! voluntary, and an agent under task pressure does not call them. So this
//! does not fix the capture problem, and it is not offered as a fix. What it
//! answers is `d100`: the memory store this replaces is reachable over MCP,
//! and taking its place means being reachable through the same door, in the
//! tool list, with a schema.
//!
//! **Five reads, nine writes, fourteen tools.** The reads answer `brief`,
//! `find`, `why`, `open` and `rules` -- the last one `t411`'s own pull,
//! since a rule nobody pulls on is a rule that might as well not be there.
//! The writes are `push`, `pop`, `add`, `decide`, `note`, `park`, `save`,
//! `arm` and `declare` -- the first seven `t106` already turned into
//! functions that hand back an `Outcome` instead of printing one, so this
//! is the second caller that reads the same answer the CLI does.
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
use crate::{brief, index, ops, outcome, params, registry, render, store};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// The version spoken when the client does not name one.
const PROTOCOL: &str = "2025-06-18";

/// What JSON shape an argument's value takes. Every tool used to need only
/// `string`, but `blocks` is a flag and `ref`/`governs`/`alternative` repeat,
/// so `schema` below has two more shapes to say.
#[derive(Clone, Copy)]
enum ArgKind {
    Str,
    Bool,
    List,
}

impl ArgKind {
    /// The word a mismatch message names it by. Not the JSON type `schema`
    /// writes into `inputSchema` -- `List` is `array` there -- because this
    /// reads as a sentence, not as a wire format.
    fn word(self) -> &'static str {
        match self {
            ArgKind::Str => "string",
            ArgKind::Bool => "boolean",
            ArgKind::List => "list",
        }
    }
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

/// Fourteen, and the number is a budget rather than a stage of growth: every
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
                      note or outcome contains all of the terms, best first, each with \
                      the lineage it hangs from. Ranking is not recency: a hit in the \
                      title outranks a hit in a note, a node holding up more tree \
                      outranks one holding up less, and recency is only the last \
                      tiebreak. Closed nodes are included: what you look for months \
                      later is usually finished.",
        args: &[
            Arg {
                name: "query",
                kind: ArgKind::Str,
                required: true,
                description: "Words to look for. Every one of them has to appear.",
            },
            Arg {
                name: "everywhere",
                kind: ArgKind::Bool,
                required: false,
                description: "Searches every project on the machine rather than this one.",
            },
        ],
    },
    Tool {
        name: "vivac_why",
        description: "Why a node exists: the chain from the goal down to it, what is open \
                      in parallel, what was born from it, and what blocks it from closing. \
                      This is the question the whole tool exists to answer.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The node as the tree names it: g1, t12, f74, d29.",
            },
            Arg {
                name: "project",
                kind: ArgKind::Str,
                required: false,
                description: "Opens a node that lives in another tree: a project name from \
                              `vivac_find`'s `everywhere`, since an alias only means \
                              something inside its own tree.",
            },
        ],
    },
    Tool {
        name: "vivac_open",
        description: "The open fronts of this project, each with its lineage: what is \
                      actually unfinished, rather than everything that was ever written \
                      down.",
        args: &[],
    },
    Tool {
        name: "vivac_rules",
        description: "What governs this project: every pillar, the rules under each \
                      pillar and those without one, and the invariants. A pillar's \
                      title names it and says what it rejects, in the project's own \
                      words. Each rule carries the commands that verify it, each with \
                      the folder it runs in, relative to the folder that holds .vivac, \
                      or none, which means it is judged. Run a command from its folder: \
                      from anywhere else it can pass without checking anything. Read it \
                      whenever you are asked to check work against the project's rules, \
                      whether or not they arrived when the session opened: vivac hands \
                      you the rules and the commands, and the judging is yours. If it \
                      comes back with no pillar and no rule while the project keeps its \
                      rules in files such as CLAUDE.md or AGENTS.md, propose which are \
                      pillars and which are rules, let the person decide, and write \
                      them with vivac_add.",
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
                description: "goal, task, decision, question, constraint, finding, \
                              assumption, pillar or rule. Defaults to goal at the root, \
                              task otherwise. A pillar is titled with its name and what \
                              it restricts, in the project's own words.",
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
            Arg {
                name: "arm",
                kind: ArgKind::List,
                required: false,
                description: "Commands that verify this rule, one per entry, all run in \
                              arm_dir. Only for a rule; a rule without one is judged. \
                              vivac never runs them.",
            },
            Arg {
                name: "arm_dir",
                kind: ArgKind::Str,
                required: false,
                description: "The folder every arm given here runs in, relative to the \
                              folder that holds .vivac: vivac, say, or . for that folder \
                              itself. Required with arm, refused without it. It has to \
                              exist.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "Only for a decision: a pillar or rule it was judged \
                              against and a sentence on how it holds, as one entry: \
                              \"r12: the write path stays local\". Repeat for each one.",
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
                description: "goal, task, decision, question, constraint, finding, \
                              assumption, pillar or rule. Defaults to goal at the root, \
                              task otherwise. A pillar is titled with its name and what \
                              it restricts, in the project's own words.",
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
            Arg {
                name: "arm",
                kind: ArgKind::List,
                required: false,
                description: "Commands that verify this rule, one per entry, all run in \
                              arm_dir. Only for a rule; a rule without one is judged. \
                              vivac never runs them.",
            },
            Arg {
                name: "arm_dir",
                kind: ArgKind::Str,
                required: false,
                description: "The folder every arm given here runs in, relative to the \
                              folder that holds .vivac: vivac, say, or . for that folder \
                              itself. Required with arm, refused without it. It has to \
                              exist.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "Only for a decision: a pillar or rule it was judged \
                              against and a sentence on how it holds, as one entry: \
                              \"r12: the write path stays local\". Repeat for each one.",
            },
        ],
    },
    Tool {
        name: "vivac_decide",
        description: "Record a decision, with the reason it was made and every alternative \
                      that lost. Call it the moment a choice is actually settled, not \
                      before and not long after: the alternatives are optional in the \
                      schema and not in practice, because without them the same option \
                      gets proposed again in a month by whoever was not in the room. \
                      When the project has pillars or rules, name in `against` the ones \
                      this was judged against, each with a sentence: a pillar judged \
                      in silence reads the same as one skipped.",
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
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "A pillar or rule this was judged against and a sentence on \
                              how it holds, as one entry: \"r12: the write path stays \
                              local\". Repeat for each one. vivac checks that the pillar \
                              or rule exists and still governs; the sentence is not judged.",
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
    Tool {
        name: "vivac_arm",
        description: "Record a command that verifies a rule and the folder it runs in, or \
                      with off, remove one. vivac never runs it: it hands it to whoever \
                      checks the rule. Call it the moment a test for a rule exists, \
                      because the day a rule became checkable is part of its history.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The rule, as the tree names it: r12.",
            },
            Arg {
                name: "command",
                kind: ArgKind::Str,
                required: true,
                description: "The command or test that verifies it, as someone would \
                              type it.",
            },
            Arg {
                name: "dir",
                kind: ArgKind::Str,
                required: true,
                description: "The folder the command runs in, relative to the folder \
                              that holds .vivac: vivac, say, or . for that folder \
                              itself. It has to exist.",
            },
            Arg {
                name: "off",
                kind: ArgKind::Bool,
                required: false,
                description: "Remove this command, in this folder, from the rule \
                              instead of adding it.",
            },
        ],
    },
    Tool {
        name: "vivac_declare",
        description: "Record, after the fact, the pillars or rules a decision was judged \
                      against, each with a sentence. Call it the moment the judging \
                      happens -- someone asks whether a decision holds against a rule, \
                      and it gets checked -- because when a decision was judged is part \
                      of its history, and this one shows as late.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The decision, as the tree names it: d12.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: true,
                description: "A pillar or rule it was judged against and a sentence on \
                              how it holds, as one entry: \"r12: the write path stays \
                              local\". Repeat for each one.",
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

/// The one door every argument comes through. A schema and the call that
/// reads it are two sources, and nothing else keeps them together (`f438`,
/// `f452`): a `Reader` is built for one tool, and every read it does is
/// checked against that tool's own `args`.
struct Reader<'a> {
    tool: &'static Tool,
    arguments: &'a Value,
}

impl<'a> Reader<'a> {
    fn new(tool: &'static Tool, params: &'a Value) -> Reader<'a> {
        Reader {
            tool,
            arguments: &params["arguments"],
        }
    }

    /// Debug builds only, and cheap enough there to run on every read: an
    /// argument the tool's schema does not list, or one read as a shape
    /// other than the one declared for it, stops the call on the spot.
    #[cfg(debug_assertions)]
    fn checked(&self, name: &str, kind: ArgKind) {
        match self.tool.args.iter().find(|a| a.name == name) {
            None => panic!(
                "{tool} reads {name}, which its schema does not declare",
                tool = self.tool.name
            ),
            Some(declared) if declared.kind.word() != kind.word() => panic!(
                "{tool} reads {name} as a {read}, and its schema declares a {is}",
                tool = self.tool.name,
                read = kind.word(),
                is = declared.kind.word()
            ),
            Some(_) => {}
        }
    }

    #[cfg(not(debug_assertions))]
    fn checked(&self, _name: &str, _kind: ArgKind) {}

    fn str(&self, name: &str) -> Option<&'a str> {
        self.checked(name, ArgKind::Str);
        self.arguments[name].as_str()
    }

    fn bool(&self, name: &str) -> bool {
        self.checked(name, ArgKind::Bool);
        self.arguments[name].as_bool().unwrap_or(false)
    }

    fn list(&self, name: &str) -> Vec<String> {
        self.checked(name, ArgKind::List);
        self.arguments[name]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The two panics `checked` raises, pinned to their exact wording. Only
/// meaningful in a debug build, the only build where `checked` does
/// anything.
#[cfg(all(test, debug_assertions))]
mod reader_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "vivac_why reads arm_dir, which its schema does not declare")]
    fn reading_an_undeclared_argument_panics() {
        let tool = TOOLS.iter().find(|t| t.name == "vivac_why").unwrap();
        let params = json!({ "arguments": {} });
        Reader::new(tool, &params).str("arm_dir");
    }

    #[test]
    #[should_panic(expected = "vivac_push reads arm as a string, and its schema declares a list")]
    fn reading_a_declared_argument_with_the_wrong_shape_panics() {
        let tool = TOOLS.iter().find(|t| t.name == "vivac_push").unwrap();
        let params = json!({ "arguments": {} });
        Reader::new(tool, &params).str("arm");
    }
}

/// Serialised the same way the three reads that speak JSON already are:
/// `pretty` over a `Value`, so the model gets back data it can parse rather
/// than the sentence `outcome::to_text` writes for a terminal.
fn outcome_text(o: outcome::Outcome) -> Result<String, Failure> {
    pretty(serde_json::to_value(&o).map_err(|e| Failure::Io(std::io::Error::other(e)))?)
}

fn call(project: &mut Project, params: &Value) -> Result<String, Failure> {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return Err(Failure::usage(format!(
            "no such tool: {name}. This server has: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )));
    };
    let a = Reader::new(tool, params);
    let missing = |what: &str| Failure::usage(format!("{name} needs a {what}."));
    match name {
        "vivac_brief" => {
            let empty = Args::default();
            let name = project.name.clone();
            let ctx = project.current()?;
            brief::to_text(&ctx.tree, ctx.anchor.as_ref(), &empty, &name)
        }
        "vivac_find" => {
            let query = a.str("query").ok_or_else(|| missing("query"))?.to_string();
            if a.bool("everywhere") {
                pretty(render::find_everywhere_data(&query)?)
            } else {
                pretty(render::find_data(&project.current()?.tree, &query)?)
            }
        }
        "vivac_why" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            match a.str("project") {
                Some(spec) => {
                    let foreign_root = registry::resolve(spec)?;
                    let tree = index::load(&store::Store::open(foreign_root)?, false)?;
                    pretty(render::why_data(&tree, &id)?)
                }
                None => pretty(render::why_data(&project.current()?.tree, &id)?),
            }
        }
        "vivac_open" => pretty(render::open_data(&project.current()?.tree)),
        "vivac_rules" => pretty(render::rules_data(&project.current()?.tree)),
        "vivac_push" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let why = a.str("why").ok_or_else(|| missing("why"))?.to_string();
            let p = params::Push {
                title,
                why,
                kind: a.str("type").map(str::to_string),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                arms: a.list("arm"),
                arm_dir: a.str("arm_dir").map(str::to_string),
                against: a.list("against"),
                via_mcp: true,
            };
            outcome_text(project.write(|ctx| ops::push(ctx, p))?)
        }
        "vivac_pop" => {
            let p = params::Pop {
                outcome: a.str("outcome").unwrap_or("").to_string(),
                next: a.str("next").map(str::to_string),
                force: a.bool("force"),
            };
            outcome_text(project.write(|ctx| ops::pop(ctx, p))?)
        }
        "vivac_add" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let p = params::Add {
                title,
                parent: a.str("parent").map(str::to_string),
                kind: a.str("type").map(str::to_string),
                why: a.str("why").unwrap_or("").to_string(),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                arms: a.list("arm"),
                arm_dir: a.str("arm_dir").map(str::to_string),
                against: a.list("against"),
                via_mcp: true,
            };
            outcome_text(project.write(|ctx| ops::add(ctx, p))?)
        }
        "vivac_decide" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let reason = a
                .str("reason")
                .ok_or_else(|| missing("reason"))?
                .to_string();
            let p = params::Decide {
                title,
                parent: a.str("parent").map(str::to_string),
                reason,
                alternatives: a.list("alternative"),
                supersedes: a.str("supersedes").map(str::to_string),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                against: a.list("against"),
            };
            outcome_text(project.write(|ctx| ops::decide(ctx, p))?)
        }
        // `id` given: the two words are unambiguous, the way `vivac note <id>
        // "<note>"` is. `id` left out: the note text takes the place a lone
        // positional would on the CLI, so `ops::note` attaches it to the
        // focus the same way `vivac note "<note>"` does.
        "vivac_note" => {
            let text = a.str("note").ok_or_else(|| missing("note"))?.to_string();
            let p = match a.str("id") {
                Some(id) => params::Note {
                    node: Some(id.to_string()),
                    note: Some(text),
                },
                None => params::Note {
                    node: Some(text),
                    note: None,
                },
            };
            outcome_text(project.write(|ctx| ops::note(ctx, p))?)
        }
        // Same shape as `vivac_note`: with no `id`, `reason` takes the place
        // of the single word `vivac park "<reason>"` would pass, and
        // `named_or_focus` is what resolves it against the focus.
        "vivac_park" => {
            let p = match (a.str("id"), a.str("reason")) {
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
            outcome_text(project.write(|ctx| ops::park(ctx, p))?)
        }
        "vivac_arm" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let command = a
                .str("command")
                .ok_or_else(|| missing("command"))?
                .to_string();
            let p = params::Arm {
                id,
                command,
                dir: a.str("dir").map(str::to_string),
                off: a.bool("off"),
                via_mcp: true,
            };
            outcome_text(project.write(|ctx| ops::arm(ctx, p))?)
        }
        "vivac_declare" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let p = params::Declare {
                id: Some(id),
                against: a.list("against"),
            };
            outcome_text(project.write(|ctx| ops::declare(ctx, p))?)
        }
        "vivac_save" => {
            let p = params::Save {
                label: a.str("label").unwrap_or("").to_string(),
                next: a.str("next").unwrap_or("").to_string(),
            };
            outcome_text(project.write(|ctx| ops::save(ctx, p))?)
        }
        other => unreachable!("{other} passed the tool lookup but no arm here handles it"),
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

/// `t192`: a write used to build a brand new `Ctx` -- store reopened, index
/// reloaded, anchor re-walked -- on every single call, throwing away the
/// fold `Project` already keeps warm. These tests are the other half of
/// that fix: proof that operating on the resident tree instead of a fresh
/// one never leaves it holding something a fresh fold would not.
#[cfg(test)]
mod resident_write_tests {
    use super::*;
    use crate::model::{Node, Tree};
    use crate::store::Store;
    use std::path::Path;

    fn temp_project(name: &str) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!(
            "vivac-mcp-resident-{name}-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Store::create(&root).unwrap();
        let project = Project::open(root.clone(), "t".into(), "t".into())
            .unwrap_or_else(|e| panic!("{}", e.message()));
        (root, project)
    }

    fn cleanup(root: &Path) {
        std::fs::remove_dir_all(root).ok();
    }

    fn call_tool(project: &mut Project, name: &str, arguments: Value) -> Value {
        let params = json!({ "name": name, "arguments": arguments });
        let text = call(project, &params).unwrap_or_else(|e| panic!("{name}: {}", e.message()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} did not reply JSON: {e}"))
    }

    /// A deterministic, order-independent rendering of a whole `Tree`:
    /// `nodes_sorted` fixes the node order and every field is resolved
    /// through the tree that owns it, so two trees folded from the same
    /// log compare equal here even when the `HashMap`s backing them were
    /// built in a different order.
    fn dump_node(tree: &Tree, n: &Node) -> String {
        format!(
            "node num={} id={} kind={:?} state={:?} parent={:?} blocks={} \
             forced_close={} title={:?} why={:?} note={:?} outcome={:?} \
             opened={:?} closed={:?} refs={:?} governs={:?} flags={:?}\n",
            n.num,
            n.id,
            n.kind,
            n.state,
            n.parent,
            n.blocks,
            n.forced_close,
            n.title(tree),
            n.why(tree),
            n.note(tree),
            n.outcome(tree),
            n.opened(tree),
            n.closed(tree),
            n.refs(tree),
            n.governs(tree),
            n.flags,
        )
    }

    fn dump_tree(tree: &Tree) -> String {
        let mut out = format!(
            "roots={:?} stack={:?} seq={} seq_change={} seq_vivac={} next_num={} \
             next_vivac_num={} broken={}\n",
            tree.roots,
            tree.stack,
            tree.seq,
            tree.seq_change,
            tree.seq_vivac,
            tree.next_num,
            tree.next_vivac_num,
            tree.broken_lines,
        );
        for n in tree.nodes_sorted() {
            out.push_str(&dump_node(tree, n));
        }
        for v in &tree.vivacs {
            out.push_str(&format!(
                "vivac num={} id={} seq={} kind={:?} stack={:?} working_set={:?} \
                 next_intent={:?} anchor={:?} node_ref={:?} label={:?} ts={:?}\n",
                v.num,
                v.id,
                v.seq,
                v.kind,
                v.stack,
                v.working_set,
                v.next_intent,
                v.anchor,
                v.node_ref,
                v.label,
                v.ts,
            ));
        }
        out
    }

    /// The property `t192` exists for: whatever the resident tree holds
    /// after the call, folding the log from scratch has to hold the exact
    /// same thing.
    fn assert_resident_matches_fresh_fold(root: &Path, project: &mut Project) {
        let resident = dump_tree(
            &project
                .current()
                .unwrap_or_else(|e| panic!("{}", e.message()))
                .tree,
        );
        let fresh = dump_tree(
            &ops::Ctx::load(Store::open(root.to_path_buf()).unwrap())
                .unwrap_or_else(|e| panic!("{}", e.message()))
                .tree,
        );
        assert_eq!(
            resident, fresh,
            "the resident tree diverged from a fresh fold of the same log"
        );
    }

    #[test]
    fn push_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("push");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn pop_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("pop");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(&mut project, "vivac_pop", json!({"outcome": "it shipped"}));
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn add_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("add");
        call_tool(
            &mut project,
            "vivac_add",
            json!({"title": "A finding", "why": "noticed in passing"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn decide_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("decide");
        call_tool(
            &mut project,
            "vivac_decide",
            json!({"title": "Rotate keys", "reason": "the old ones leaked"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn note_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("note");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_note",
            json!({"note": "the rollback plan is untested"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn park_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("park");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_park",
            json!({"reason": "waiting on the security review"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn save_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("save");
        call_tool(
            &mut project,
            "vivac_save",
            json!({"label": "before the migration", "next": "run the reconcile"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn declare_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("declare");
        call_tool(
            &mut project,
            "vivac_add",
            json!({"title": "Security", "type": "pillar", "why": "vetoes on the spot"}),
        );
        call_tool(
            &mut project,
            "vivac_add",
            json!({
                "title": "Keep the write path local",
                "parent": "1",
                "type": "rule",
                "why": "guard",
            }),
        );
        call_tool(
            &mut project,
            "vivac_decide",
            json!({"title": "Keep it local", "reason": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_declare",
            json!({
                "id": "d3",
                "against": ["r2: nothing on the write path calls the network"],
            }),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// The other half of `LOADING.md` §4's rule: `load_for_write` exists so
    /// that a write never pays to rewrite the derived index, and the
    /// resident path replacing it must not quietly start doing that.
    #[test]
    fn a_resident_write_never_persists_the_index() {
        let (root, mut project) = temp_project("index");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        assert!(
            !Store::open(root.clone()).unwrap().index_path().exists(),
            "a write through the resident Ctx must never persist the index"
        );
        cleanup(&root);
    }
}
