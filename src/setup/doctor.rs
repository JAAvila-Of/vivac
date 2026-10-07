//! Project configuration and observed hook execution are different evidence.
//! This command never uses the normal loader, runs a harness, or repairs files.

use crate::args::Args;
use crate::event::Body;
use crate::failure::Failure;
use crate::output::outln;
use crate::style::{self, Stream};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Serialize)]
struct Check {
    name: String,
    status: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

#[derive(Default, Serialize)]
struct Report {
    checks: Vec<Check>,
}

impl Report {
    fn add(&mut self, name: &str, status: &'static str, detail: &str, fix: Option<&str>) {
        self.checks.push(Check {
            name: name.into(),
            status,
            detail: detail.into(),
            fix: fix.map(str::to_string),
        });
    }

    fn print(&self, json: bool) -> i32 {
        let errors = self.checks.iter().filter(|c| c.status == "error").count();
        if json {
            outln!(
                "{}",
                serde_json::json!({"checks": self.checks, "errors": errors})
            );
        } else {
            outln!("  {}", style::bold(Stream::Out, "vivac doctor"));
            for c in &self.checks {
                let mark = match c.status {
                    "ok" => style::good(Stream::Out, "[ok]"),
                    "error" => style::gone(Stream::Out, "[error]"),
                    _ => style::warn(Stream::Out, "[unverified]"),
                };
                let line = format!("{}: {}", c.name, c.detail);
                let lead = format!("  {mark} ");
                let width = style::width(Stream::Out).unwrap_or(80);
                let indent = match c.status {
                    "ok" => 7,
                    "error" => 10,
                    _ => 15,
                };
                for (i, part) in style::wrap_title(indent, &line, width).iter().enumerate() {
                    outln!(
                        "{}{}",
                        if i == 0 {
                            lead.clone()
                        } else {
                            " ".repeat(indent)
                        },
                        part
                    );
                }
                if let Some(fix) = &c.fix {
                    for part in style::wrap_title(4, fix, width) {
                        outln!("    {part}");
                    }
                }
            }
            outln!();
            for part in style::wrap_title(2, &format!("{errors} configuration, read or hook error(s). Unverified checks are not proof of failure."), style::width(Stream::Out).unwrap_or(80)) {
                outln!("  {part}");
            }
        }
        i32::from(errors > 0)
    }
}

/// Read errors never include file contents or parser excerpts in the output.
fn read(path: &Path, label: &str, fix: &str, report: &mut Report) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) => {
            let detail = if e.kind() == std::io::ErrorKind::NotFound {
                "file is missing"
            } else {
                "file cannot be read as UTF-8"
            };
            report.add(label, "error", detail, Some(fix));
            None
        }
    }
}

fn read_json(path: &Path, label: &str, fix: &str, report: &mut Report) -> Option<Value> {
    let s = read(path, label, fix, report)?;
    match serde_json::from_str::<Value>(&s) {
        Ok(v) if v.is_object() => Some(v),
        _ => {
            report.add(label, "error", "expected a JSON object", Some(fix));
            None
        }
    }
}

fn hooks(root: &Value, harness: &str, report: &mut Report) {
    let fix = format!("Run vivac setup {harness} --dry-run in the lane folder; review any conflicts before applying setup.");
    if root.get("disableAllHooks").and_then(Value::as_bool) == Some(true) {
        report.add(
            harness,
            "error",
            "hooks are disabled by project configuration",
            Some("Enable hooks in the project's settings, then restart the harness."),
        );
    }
    for (event, word) in [
        ("SessionStart", "start"),
        ("UserPromptSubmit", "prompt"),
        ("Stop", "end"),
    ] {
        let name = format!("{harness} {event}");
        let expected = format!("vivac session {word} --hook");
        let entries = root
            .get("hooks")
            .and_then(|v| v.get(event))
            .and_then(Value::as_array);
        let exact = entries.into_iter().flatten().any(|entry| {
            // The default start matcher covers every supported opening. A
            // narrower or custom matcher needs a person to judge its scope.
            let scope = match entry.get("matcher") {
                None => true,
                Some(Value::String(matcher)) => {
                    matcher.is_empty()
                        || matcher == "*"
                        || (event == "SessionStart" && matcher == "startup|resume|clear|compact")
                }
                _ => false,
            };
            scope
                && entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|h| {
                        h.get("type").and_then(Value::as_str) == Some("command")
                            && h.get("command").and_then(Value::as_str) == Some(expected.as_str())
                            && matches!(h.get("async"), None | Some(Value::Bool(false)))
                    })
        });
        if exact {
            report.add(
                &name,
                "ok",
                "expected command is configured; execution is checked separately",
                None,
            );
        } else {
            // Reuse setup's recognition of an alternative vivac command,
            // without echoing that command (it may contain private data).
            let text = serde_json::to_string(root).expect("JSON value");
            let parsed = super::json::parse(&text).expect("JSON value");
            let state = super::claude_code::hook_state(&parsed, event, word, &expected);
            match state {
                super::claude_code::HookState::Missing => {
                    report.add(&name, "error", "expected hook is missing", Some(&fix))
                }
                _ => report.add(
                    &name,
                    "warning",
                    "custom command, matcher or execution mode; not verified",
                    Some(&fix),
                ),
            }
        }
    }
}

fn harness(here: &Path, name: &str, report: &mut Report) {
    let fix = format!("Run vivac setup {name} --dry-run in the lane folder; review the plan before applying setup.");
    let hook_file = if name == "codex" {
        ".codex/hooks.json"
    } else {
        ".claude/settings.json"
    };
    if let Some(v) = read_json(&here.join(hook_file), hook_file, &fix, report) {
        hooks(&v, name, report);
    }
    if name == "claude-code" {
        let local = here.join(".claude/settings.local.json");
        if local.exists() {
            if let Some(v) = read_json(&local, ".claude/settings.local.json", "Inspect the project's .claude/settings.local.json; it can override shared hook settings.", report) {
                if v.get("disableAllHooks").and_then(Value::as_bool) == Some(true) {
                    report.add("claude-code local hooks", "error", "hooks are disabled by local project settings", Some("Enable hooks in .claude/settings.local.json, then restart Claude Code."));
                }
            }
        }
        if let Some(v) = read_json(&here.join(".mcp.json"), ".mcp.json", &fix, report) {
            let parsed = super::json::parse(&v.to_string()).expect("JSON value");
            match super::claude_code::mcp_state(&parsed) {
                super::claude_code::McpState::Ours | super::claude_code::McpState::OtherName(_) => {
                    report.add(
                        "claude-code MCP",
                        "ok",
                        "vivac mcp is configured; server connectivity is not probed",
                        None,
                    )
                }
                _ => report.add(
                    "claude-code MCP",
                    "error",
                    "expected vivac mcp entry is missing or conflicting",
                    Some(&fix),
                ),
            }
        }
    } else if let Some(text) = read(
        &here.join(".codex/config.toml"),
        ".codex/config.toml",
        &fix,
        report,
    ) {
        // Do not mistake the mere presence of markers for valid TOML. This
        // verifies only the unmodified block setup writes, not the whole file.
        let normalized = text.replace("\r\n", "\n");
        let count = normalized.matches("[mcp_servers.vivac]").count();
        if count == 1 && normalized.contains(super::codex::CONFIG_CONTENT.trim_end()) {
            report.add("codex MCP", "ok", "setup's server block is present; surrounding TOML and server connectivity are not verified", None);
        } else {
            report.add(
                "codex MCP",
                "warning",
                "setup's server block is absent or modified; custom TOML is not verified",
                Some(&fix),
            );
        }
        report.add(
            "codex trust",
            "warning",
            "project trust and hook approval cannot be read from project files",
            Some(
                "Open this project in Codex, approve its project configuration and inspect /hooks.",
            ),
        );
    }
}

pub(crate) fn configured(here: &Path, name: &str) -> bool {
    let files: &[&str] = match name {
        "codex" => &[".codex/hooks.json", ".codex/config.toml"],
        "claude-code" => &[".claude/settings.json", ".mcp.json"],
        _ => return false,
    };
    for relative in files {
        let directory = relative.split('/').next().unwrap();
        let Ok(path) = crate::agents::adapters::safe_path(here, relative, directory) else {
            return false;
        };
        if !std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
        {
            return false;
        }
    }
    let local = ".claude/settings.local.json";
    if name == "claude-code" && here.join(local).exists() {
        let Ok(path) = crate::agents::adapters::safe_path(here, local, ".claude") else {
            return false;
        };
        if !std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
        {
            return false;
        }
    }
    let mut report = Report::default();
    harness(here, name, &mut report);
    report
        .checks
        .iter()
        .filter(|check| check.name != "codex trust")
        .all(|check| check.status == "ok")
}

pub(crate) fn run(cwd: &Path, args: &Args) -> Result<i32, Failure> {
    let chosen = args.positional(0);
    if !args.extra(1).is_empty() || chosen.is_some_and(|h| !matches!(h, "claude-code" | "codex")) {
        return Err(Failure::usage(
            "usage: vivac doctor [claude-code|codex] [--json]",
        ));
    }
    let mut report = Report::default();
    let located = match crate::store::locate(cwd) {
        Ok(Some(l)) => l,
        Ok(None) => {
            report.add("tree", "error", "no tree found from this folder", Some("Run vivac init in the project, or vivac init --join <project> in a folder belonging to an existing tree."));
            return Ok(report.print(args.has("json")));
        }
        Err(_) => {
            report.add("tree", "error", "tree or lane could not be resolved", Some("Inspect .vivac/config and .vivac/lane; restore missing files or the linked tree before running setup."));
            return Ok(report.print(args.has("json")));
        }
    };
    let config = located.root.join(".vivac/config");
    let mut events = None;
    if read(
        &config,
        ".vivac/config",
        "Restore the project's .vivac/config; doctor does not regenerate it.",
        &mut report,
    )
    .is_some()
    {
        match crate::store::Store::open_from_elsewhere(located.root.clone()) {
            Ok(st) => {
                if !st.log().is_file() {
                    report.add("tree", "error", "event log is missing", Some("Restore .vivac/events from a known good copy."));
                } else {
                    match st.read_all() {
                        Ok((log, broken)) => {
                            if broken == 0 {
                                report.add("tree", "ok", "config and event log are readable; use vivac check for tree invariants", None);
                            } else {
                                report.add("tree", "error", &format!("{broken} unreadable event line(s)"), Some("Run vivac check to inspect the tree; restore damaged event lines from a known good copy."));
                            }
                            events = Some(log);
                        }
                        Err(_) => report.add("tree", "error", "event log could not be read by this version", Some("Check file access and whether the tree requires a newer vivac.")),
                    }
                }
            }
            Err(_) => report.add("tree", "error", "config could not be read by this version", Some("Restore a valid .vivac/config or update vivac if the tree requires a newer version.")),
        }
    }
    let here = &located.lane_dir;
    let mut found = false;
    for name in ["claude-code", "codex"] {
        let present = if name == "codex" {
            here.join(".codex/hooks.json").exists() || here.join(".codex/config.toml").exists()
        } else {
            here.join(".claude/settings.json").exists()
                || here.join(".claude/settings.local.json").exists()
                || here.join(".mcp.json").exists()
        };
        if chosen == Some(name) || (chosen.is_none() && present) {
            found = true;
            harness(here, name, &mut report);
        }
    }
    if !found {
        report.add(
            "harness",
            "error",
            "no project configuration found in the lane folder",
            Some("Run vivac setup claude-code or vivac setup codex in the lane folder."),
        );
    }
    let lane = located
        .lane
        .as_ref()
        .map(|l| l.id.as_str())
        .unwrap_or(crate::lane::MAIN);
    if let Some(log) = events {
        match log
            .iter()
            .rev()
            .find(|e| e.lane == lane && matches!(e.payload, Body::SessionStarted { .. }))
        {
            Some(e) if crate::clock::epoch_seconds(&e.ts).is_some() => report.add(
                "observed SessionStart",
                "ok",
                &format!(
                    "brief emission recorded at {}; shared lane evidence, harness not identified",
                    e.ts
                ),
                None,
            ),
            _ => report.add(
                "observed SessionStart",
                "warning",
                "no recorded opening in this lane; configuration alone does not prove execution",
                Some("Restart the harness in this project, then run vivac doctor again."),
            ),
        }
    }
    match crate::session::close_evidence(&located) {
        Some((at, detail, status)) => report.add("observed Stop", status, &format!("{at}: {detail}; shared lane evidence on this machine, harness not identified"), if status == "ok" { None } else { Some("Run vivac session end --dry-run to inspect the close hook's result.") }),
        None => report.add("observed Stop", "warning", "no readable close-hook record for this lane on this machine", Some("Let a turn finish in the harness, then run vivac doctor again; vivac session end --dry-run explains the close hook.")),
    }
    match crate::agents::diagnosis(cwd, chosen) {
        Ok(agents) => {
            let count = |key: &str| agents[key].as_array().map_or(0, Vec::len);
            let errors = count("errors");
            let unmanaged = count("unmanaged");
            let unverified = count("unverified");
            report.add(
                "agent custody",
                if errors > 0 { "error" } else if unmanaged + unverified > 0 { "warning" } else { "ok" },
                &format!("{errors} custody error(s), {unmanaged} unmanaged agent file(s), {unverified} unverified assignment(s); native configuration does not prove runtime selection"),
                if errors + unmanaged + unverified > 0 { Some("Run vivac agents scan and vivac agents status to inspect assignments and custody.") } else { None },
            );
        }
        Err(_) => report.add(
            "agent custody",
            "error",
            "agent custody could not be inspected; file contents withheld",
            Some("Run vivac agents status to inspect the failure."),
        ),
    }
    Ok(report.print(args.has("json")))
}
