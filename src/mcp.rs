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
//! answers is `d100`: engram is an MCP server, and taking its place means
//! being reachable through the same door, in the tool list, with a schema.
//!
//! **Reads only, for now.** The write operations return `()` and print, so
//! the server would have nothing to report back. They keep going through the
//! CLI until they return what they did rather than say it.

use crate::args::Args;
use crate::failure::{Failure, R};
use crate::project::{Project, Registry};
use crate::{brief, render};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// The version spoken when the client does not name one.
const PROTOCOL: &str = "2025-06-18";

struct Arg {
    name: &'static str,
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

/// Four, and the number is a budget rather than a stage of growth: every tool
/// here costs context in every session the agent ever opens.
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
];

fn schema(t: &Tool) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<&str> = Vec::new();
    for a in t.args {
        properties.insert(
            a.name.to_string(),
            json!({ "type": "string", "description": a.description }),
        );
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
