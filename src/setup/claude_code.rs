//! Claude Code, the one harness `vivac setup` knows today.
//!
//! What is Claude Code's own: the two files it reads (`.claude/settings.json`
//! and `.mcp.json`), the skill it looks for under `.claude/skills/`, the
//! shape of a hook entry, and how a command line already there is told apart
//! from a foreign one. `t565` §9: this is the module `INTEGRATION.md` points
//! at for the level-one work of a new harness.
//!
//! `d723` piece B: this file no longer touches the tree at all -- planting,
//! joining and `--name` moved to `init` in piece A, and here that means
//! `run` never reaches `tree::plan` or `tree::plan_join` any more. What
//! stays is exactly the three pieces Claude Code itself reads, once
//! `super::resolve_for_setup` has already said this folder is either the
//! tree's own or one of its declared lanes. `tree_below_join_refusal` and
//! `join_with_and` stay here even so: `init`'s own `--join` still calls
//! them, and moving two functions nobody asked to move is not this piece's
//! job.

use super::json::{self, Value};
use super::tree;
use crate::args::Args;
use crate::failure::Failure;
use crate::output::outln;
use std::path::{Path, PathBuf};

const SETTINGS_LABEL: &str = ".claude/settings.json";
const MCP_LABEL: &str = ".mcp.json";
const SKILL_LABEL: &str = ".claude/skills/vivac-migrate/SKILL.md";

const SESSION_START_COMMAND: &str = "vivac session start --hook";
const SESSION_END_COMMAND: &str = "vivac session end --hook";
const SESSION_PROMPT_COMMAND: &str = "vivac session prompt --hook";

const FRONTMATTER: &str = include_str!("skill-frontmatter.md");
const BODY: &str = include_str!("skill-body.md");

pub fn run(cwd: &Path, a: &Args) -> Result<i32, Failure> {
    if a.has("undo") {
        return undo(cwd, a);
    }
    let roots = super::resolve_for_setup(cwd)?;
    // Still checked here, and still excluded from `--undo`: undoing
    // whatever an earlier setup wrote there is always safe, home folder or
    // global store included.
    if let Some(refusal) = super::refuse_home_or_global_store(&roots) {
        return Err(refusal);
    }
    apply(&roots, a)
}

// ---------------------------------------------------------------------------
// Shared: the vivac-command test, and reading the two JSON files.
// ---------------------------------------------------------------------------

/// Whether `word`'s first token, quotes and path stripped, is `vivac`:
/// `t565` §7.4.
fn is_vivac_command(word: &str) -> bool {
    let word = word.replace('"', "");
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word.as_str());
    let stem = if base.len() >= 4 && base[base.len() - 4..].eq_ignore_ascii_case(".exe") {
        &base[..base.len() - 4]
    } else {
        base
    };
    stem.eq_ignore_ascii_case("vivac")
}

/// `pub(super)`: `codex.rs` reads its own `.codex/hooks.json` through this
/// same struct and its fields, rather than a second reader for JSON it also
/// treats the way `d654` reserves for JSON alone (`t592` tranche 2, piece B).
pub(super) struct JsonFile {
    pub(super) exists: bool,
    pub(super) raw: String,
    pub(super) indent: String,
    pub(super) eol: &'static str,
    pub(super) trailing_newline: bool,
    /// `Some` once parsed as an object; `None` for a missing file (nothing to
    /// parse) or a conflict (unreadable, or not an object).
    pub(super) value: Option<Value>,
    /// Line and column of a parse failure, for the conflict message.
    pub(super) parse_error: Option<(usize, usize)>,
    pub(super) not_object: bool,
}

pub(super) fn read_json(path: &Path) -> JsonFile {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return JsonFile {
            exists: false,
            raw: String::new(),
            indent: "  ".to_string(),
            eol: "\n",
            trailing_newline: true,
            value: Some(Value::object(vec![])),
            parse_error: None,
            not_object: false,
        };
    };
    let indent = json::detect_indent(&raw);
    let eol = json::detect_eol(&raw);
    let trailing_newline = json::has_trailing_newline(&raw);
    match json::parse(&raw) {
        Ok(v) if v.is_object() => JsonFile {
            exists: true,
            raw,
            indent,
            eol,
            trailing_newline,
            value: Some(v),
            parse_error: None,
            not_object: false,
        },
        Ok(_) => JsonFile {
            exists: true,
            raw,
            indent,
            eol,
            trailing_newline,
            value: None,
            parse_error: None,
            not_object: true,
        },
        Err(e) => JsonFile {
            exists: true,
            raw,
            indent,
            eol,
            trailing_newline,
            value: None,
            parse_error: Some((e.line(), e.column())),
            not_object: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Hooks: SessionStart and Stop.
// ---------------------------------------------------------------------------

/// `pub(super)`: `codex.rs` reads its own two hooks through this same type
/// (`t592` tranche 2, piece B).
pub(super) enum HookState {
    Missing,
    Exact,
    Different(String),
}

fn command_first_word(cmd: &str) -> Option<&str> {
    cmd.split_whitespace().next()
}

/// Looks through `event`'s array, under the top-level `hooks` object
/// (`SessionStart` or `Stop` are never top-level keys of their own: Claude
/// Code nests every event under `hooks`), for a vivac command whose
/// arguments start with `session start` or `session end`.
///
/// `pub(super)`: Codex nests its own two events under the same top-level
/// `hooks` key, so `codex.rs` reads its own hooks.json through this same
/// function (`t592` tranche 2, piece B).
pub(super) fn hook_state(root: &Value, event: &str, session_word: &str, ours: &str) -> HookState {
    let Some(arr) = root
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
    else {
        return HookState::Missing;
    };
    for entry in arr {
        let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
            continue;
        };
        for h in hooks {
            let Some(cmd) = h.get("command").and_then(Value::as_str) else {
                continue;
            };
            let words: Vec<&str> = cmd.split_whitespace().collect();
            let Some(prog) = command_first_word(cmd) else {
                continue;
            };
            if !is_vivac_command(prog) {
                continue;
            }
            if words.get(1) == Some(&"session") && words.get(2) == Some(&session_word) {
                return if cmd == ours {
                    HookState::Exact
                } else {
                    HookState::Different(cmd.to_string())
                };
            }
        }
    }
    HookState::Missing
}

fn our_hook_entry(command: &str) -> Value {
    Value::object(vec![(
        "hooks",
        Value::Array(vec![Value::object(vec![
            ("type", Value::str("command")),
            ("command", Value::str(command)),
        ])]),
    )])
}

/// The same entry `our_hook_entry` builds, with the matcher Codex's own
/// `SessionStart` filters its sources by. Claude Code's `SessionStart`
/// takes no matcher, so this is Codex's alone rather than a parameter on
/// the shared one.
fn our_hook_entry_with_matcher(command: &str, matcher: &str) -> Value {
    Value::object(vec![
        ("matcher", Value::str(matcher)),
        (
            "hooks",
            Value::Array(vec![Value::object(vec![
                ("type", Value::str("command")),
                ("command", Value::str(command)),
            ])]),
        ),
    ])
}

/// Gets `root[key]` as an object, creating it first if it is missing.
fn get_or_insert_object<'a>(root: &'a mut Value, key: &str) -> &'a mut Value {
    if root.get(key).map(Value::is_object) != Some(true) {
        root.set(key, Value::object(vec![]));
    }
    root.as_object_mut()
        .unwrap()
        .iter_mut()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .unwrap()
}

/// `matcher`: `None` for Claude Code's own hooks, which take no matcher at
/// all. Codex's `SessionStart` passes `Some` and its `Stop` passes `None`
/// too, because Codex ignores a matcher on that event (`t592` tranche 2,
/// piece B) -- the one place the two harnesses' hook shapes differ.
///
/// `pub(super)`: `codex.rs` appends its own two hooks through this same
/// function.
pub(super) fn append_hook(root: &mut Value, event: &str, command: &str, matcher: Option<&str>) {
    let hooks = get_or_insert_object(root, "hooks");
    if hooks.get(event).map(Value::as_array).is_none() {
        hooks.set(event, Value::Array(vec![]));
    }
    let arr = hooks
        .as_object_mut()
        .unwrap()
        .iter_mut()
        .find(|(k, _)| k == event)
        .map(|(_, v)| v)
        .unwrap()
        .as_array_mut()
        .unwrap();
    let entry = match matcher {
        Some(m) => our_hook_entry_with_matcher(command, m),
        None => our_hook_entry(command),
    };
    arr.push(entry);
}

/// Removes every array entry whose sole hook is exactly `command`, then
/// drops the event key if its array is now empty, and `hooks` itself if
/// that leaves it with nothing. `t565` §7.7.
///
/// `pub(super)`: `codex.rs` takes its own two hooks off through this same
/// function (`t592` tranche 2, piece C).
pub(super) fn remove_hook(root: &mut Value, event: &str, command: &str) -> bool {
    let mut removed = false;
    let Some(hooks) = root.get("hooks").cloned() else {
        return false;
    };
    let Some(arr) = hooks.get(event).and_then(Value::as_array) else {
        return false;
    };
    let kept: Vec<Value> = arr
        .iter()
        .filter(|entry| {
            let is_ours = entry
                .get("hooks")
                .and_then(Value::as_array)
                .map(|hs| {
                    hs.len() == 1 && hs[0].get("command").and_then(Value::as_str) == Some(command)
                })
                .unwrap_or(false);
            if is_ours {
                removed = true;
            }
            !is_ours
        })
        .cloned()
        .collect();

    let mut new_hooks = hooks;
    if kept.is_empty() {
        if let Some(obj) = new_hooks.as_object_mut() {
            obj.retain(|(k, _)| k.as_str() != event);
        }
    } else {
        new_hooks.set(event, Value::Array(kept));
    }
    if new_hooks.as_object().is_some_and(|o| o.is_empty()) {
        if let Some(obj) = root.as_object_mut() {
            obj.retain(|(k, _)| k.as_str() != "hooks");
        }
    } else {
        root.set("hooks", new_hooks);
    }
    removed
}

// ---------------------------------------------------------------------------
// The MCP server entry.
// ---------------------------------------------------------------------------

enum McpState {
    Missing,
    Ours,
    OtherName(String),
    NameTaken(String),
}

fn describe_command(v: &Value) -> String {
    let cmd = v.get("command").and_then(Value::as_str).unwrap_or("");
    let args: Vec<String> = v
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if args.is_empty() {
        cmd.to_string()
    } else {
        format!("{cmd} {}", args.join(" "))
    }
}

fn is_our_mcp_entry(v: &Value) -> bool {
    let is_vivac = v
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_vivac_command);
    let args_ok = v
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|a| a.len() == 1 && a[0].as_str() == Some("mcp"));
    is_vivac && args_ok
}

fn mcp_state(root: &Value) -> McpState {
    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return McpState::Missing;
    };
    if let Some((_, v)) = servers.iter().find(|(k, _)| k == "vivac") {
        return if is_our_mcp_entry(v) {
            McpState::Ours
        } else {
            McpState::NameTaken(describe_command(v))
        };
    }
    for (name, v) in servers {
        if is_our_mcp_entry(v) {
            return McpState::OtherName(name.clone());
        }
    }
    McpState::Missing
}

fn our_mcp_entry() -> Value {
    Value::object(vec![
        ("type", Value::str("stdio")),
        ("command", Value::str("vivac")),
        ("args", Value::Array(vec![Value::str("mcp")])),
    ])
}

/// `root` with `mcpServers.vivac` removed, and `mcpServers` itself dropped
/// once that leaves it empty -- the same "an empty container does not
/// linger" rule `remove_hook` applies to `hooks`.
fn without_our_mcp_server(root: &Value) -> Value {
    let mut root = root.clone();
    let Some(mut servers) = root.get("mcpServers").cloned() else {
        return root;
    };
    if let Some(obj) = servers.as_object_mut() {
        obj.retain(|(k, _)| k != "vivac");
    }
    if servers.as_object().is_some_and(|o| o.is_empty()) {
        if let Some(obj) = root.as_object_mut() {
            obj.retain(|(k, _)| k.as_str() != "mcpServers");
        }
    } else {
        root.set("mcpServers", servers);
    }
    root
}

// ---------------------------------------------------------------------------
// The skill file.
// ---------------------------------------------------------------------------

/// `pub(super)`: `codex.rs` reads its own skill's state through this same
/// type (`t592` tranche 2, piece B).
pub(super) enum SkillState {
    Missing,
    Same,
    Replaceable,
    Conflict,
}

/// The marker names no harness, and used to name `claude-code` (`f714`):
/// this file is the same one byte for byte wherever setup writes it
/// (`d653`), so a project set up with Codex was handed a line proposing a
/// command for the other harness. What `extract_marker` reads back is the
/// prefix up to the fingerprint, which has not moved, so a file an earlier
/// vivac wrote still reads as its own and `--undo` still takes it away.
fn marker_line(fingerprint: u64) -> String {
    format!(
        "<!-- written by vivac setup; fingerprint {fingerprint:016x}; setup removes \
         it with --undo while the text is unchanged -->\n"
    )
}

fn skill_content_without_marker() -> String {
    format!("{FRONTMATTER}{BODY}")
}

fn skill_fingerprint() -> u64 {
    super::fnv1a64(skill_content_without_marker().as_bytes())
}

/// `pub(super)`: `codex.rs` writes this same file at its own path, byte for
/// byte, rather than keeping a second copy of the skill (`d653`).
pub(super) fn skill_text() -> String {
    format!("{FRONTMATTER}{}{BODY}", marker_line(skill_fingerprint()))
}

/// Splits `text` into its frontmatter, the marker's claimed fingerprint (as
/// the hex it was written with) and the content the fingerprint should have
/// been taken over -- `text` with the marker line and its newline removed.
/// `None` when there is no frontmatter or no line right after it: `t565`
/// §7.4's "any other case" for a skill with no marker at all.
fn extract_marker(text: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.first() != Some(&"---") {
        return None;
    }
    let close = lines.iter().skip(1).position(|&l| l == "---")? + 1;
    let marker_idx = close + 1;
    let marker = *lines.get(marker_idx)?;
    let fp = marker
        .strip_prefix("<!-- written by vivac setup; fingerprint ")?
        .split(';')
        .next()?
        .trim()
        .to_string();
    let mut without = lines;
    without.remove(marker_idx);
    Some((fp, without.join("\n")))
}

/// `pub(super)`: `codex.rs` calls this for its own skill file, since
/// `skill_text()` is `claude_code::skill_text()` itself rather than a
/// second copy (`d653`, `t592` tranche 2 piece B).
pub(super) fn skill_state(existing: &str) -> SkillState {
    if existing == skill_text() {
        return SkillState::Same;
    }
    match extract_marker(existing) {
        Some((fp_hex, content)) => {
            let claimed = u64::from_str_radix(&fp_hex, 16).ok();
            let actual = super::fnv1a64(content.as_bytes());
            if claimed == Some(actual) {
                SkillState::Replaceable
            } else {
                SkillState::Conflict
            }
        }
        None => SkillState::Conflict,
    }
}

/// Whether an existing skill's fingerprint is intact, regardless of whether
/// its text still matches what this version would write today. `--undo`
/// only ever removes a file it (or an earlier vivac) actually wrote.
///
/// `pub(super)`: `codex.rs` checks its own skill's fingerprint through this
/// same function before `--undo` takes it (`t592` tranche 2, piece C).
pub(super) fn skill_fingerprint_intact(existing: &str) -> bool {
    matches!(
        skill_state(existing),
        SkillState::Same | SkillState::Replaceable
    )
}

// ---------------------------------------------------------------------------
// Paths.
// ---------------------------------------------------------------------------

struct Paths {
    settings: PathBuf,
    mcp: PathBuf,
    skill: PathBuf,
}

fn paths(root: &Path) -> Paths {
    Paths {
        settings: root.join(".claude").join("settings.json"),
        mcp: root.join(".mcp.json"),
        skill: root
            .join(".claude")
            .join("skills")
            .join("vivac-migrate")
            .join("SKILL.md"),
    }
}

// ---------------------------------------------------------------------------
// Recognizing an existing product, before planting a second map of it:
// `t594` §4.5, case 3 -- reached only when there is no tree above `here`
// at all. `trees_below`, `tree_below_refusal`, `refuse_second_map` and
// `second_map_hint` moved to `tree.rs` (`t592` tranche 2, `d710`), and
// `tree_above_refusal` moved with `--join`'s own preamble (piece G,
// `f714`). What stays here is only the `--join` case:
// `tree_below_join_refusal`, `pub(super)` since `codex.rs` calls it too now.
// ---------------------------------------------------------------------------

/// `d626`: the same disk state `tree::tree_below_refusal` names for a plant,
/// met by `--join` instead. The remedy is not the same door -- nothing
/// here was about to be planted, so "move that tree up, then run setup
/// again" would have pointed at a choice nobody was making, and naming
/// the folder to run `relocate` from, rather than a destination for it,
/// is what actually matches how `relocate` works: it runs from inside
/// the tree it moves, not from above it. `spec` is printed back exactly
/// as typed and quoted, the same as every other refusal in this module
/// names something -- it is the choice being made, not a tree this call
/// went looking for and resolved.
///
/// Every tree found is named, following `tree::tree_below_refusal`'s own
/// shape for the same disk state: whoever fixes the first and hits this
/// refusal again would only be learning the same thing twice.
///
/// A route is withheld whole when any segment of it trips the redaction
/// guard (`guarded_relative`, `d600`) -- the guard covers the folder
/// name it was built to cover, and a route this refusal prints can be
/// several of those deep. With a mix of withheld and shown routes, only
/// the shown ones are listed, and how many are missing is never said:
/// the count is also something the guard would be handing over.
pub(super) fn tree_below_join_refusal(here: &Path, below: &[PathBuf], spec: &str) -> Failure {
    let routes: Vec<Option<String>> = below.iter().map(|p| guarded_relative(here, p)).collect();
    let shown: Vec<&str> = routes.iter().filter_map(|r| r.as_deref()).collect();

    if let [only] = routes.as_slice() {
        return match only {
            Some(rel) => Failure::Model(format!(
                "  There is another product's tree below this folder:\n    \
                 {rel}\n\n  \
                 This folder cannot be a lane of \"{spec}\" while that tree is there: one\n  \
                 folder answers for one product, and a lane that contains another\n  \
                 product's tree would answer for two.\n\n  \
                 If the tree below is part of \"{spec}\", move it up. From inside {rel}:\n      \
                 vivac relocate ..\n  \
                 If it is a different product, join from a folder that does not contain it."
            )),
            None => Failure::Model(format!(
                "  There is another product's tree below this folder, under a name this tool\n  \
                 will not write down.\n\n  \
                 This folder cannot be a lane of \"{spec}\" while that tree is there: one\n  \
                 folder answers for one product, and a lane that contains another\n  \
                 product's tree would answer for two.\n\n  \
                 Join from a folder that does not contain it, or move that tree up from\n  \
                 inside it:   vivac relocate .."
            )),
        };
    }

    if shown.is_empty() {
        return Failure::Model(format!(
            "  There are other products' trees below this folder, under names this tool\n  \
             will not write down.\n\n  \
             This folder cannot be a lane of \"{spec}\" while any of them is there: one\n  \
             folder answers for one product, and a lane that contains another\n  \
             product's tree would answer for two.\n\n  \
             Join from a folder that does not contain them, or move them up from\n  \
             inside each one:   vivac relocate .."
        ));
    }

    let listed: String = shown.iter().map(|r| format!("    {r}\n")).collect();
    Failure::Model(format!(
        "  There are other products' trees below this folder:\n\
         {listed}\n  \
         This folder cannot be a lane of \"{spec}\" while any of them is there: one\n  \
         folder answers for one product, and a lane that contains another\n  \
         product's tree would answer for two.\n\n  \
         Any of them that belongs to \"{spec}\" can move up, from inside it:\n      \
         vivac relocate ..\n  \
         For the rest, join from a folder that does not contain them."
    ))
}

/// `path`'s own route down from `base`, forward slashes on every
/// platform, the same convention `event::Repo::relative` already prints
/// a repository under -- or `None` when any segment of that route trips
/// the redaction guard: `guarded_folder_name` only ever checked the last
/// one, and a route `tree_below_join_refusal` prints can run several
/// folders deep, any of which might be the one that should not travel
/// (`d600`).
fn guarded_relative(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).unwrap_or(path);
    let mut parts = Vec::new();
    for c in rel.components() {
        let part = c.as_os_str().to_string_lossy().into_owned();
        match crate::redact::check_field("folder name", &part) {
            Some(_) => return None,
            None => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

// ---------------------------------------------------------------------------
// Formatting: the two-column plan lines `t565` §7.8 fixes the width of.
// ---------------------------------------------------------------------------

// `pub(super)`: `codex.rs` renders its own plan in the same two columns,
// rather than fixing the same widths a second time (`d653`).

/// The plan's own line width: the same 76 `wrapped`, below, and
/// `registry::NOTICE_WIDTH` already wrap their own prose to, and for the
/// same reason (`f720`). A status can carry a lane name or a product
/// name typed by whoever runs setup, which has no bound, so cutting it
/// by hand is wrong by construction and not by oversight.
const PLAN_WIDTH: usize = 76;

const PIECE_INDENT: usize = 4;
const PIECE_LABEL_WIDTH: usize = 41;

/// Where a status starts: the indent plus the label column's own width,
/// so a status `render::wrap` splits lands its later lines under the
/// first one instead of under the label. Derived once here rather than
/// written as `45` by hand in three places (`f720`).
const PIECE_STATUS_COLUMN: usize = PIECE_INDENT + PIECE_LABEL_WIDTH;

pub(super) fn piece_line(label: &str, status: &str) -> String {
    let indent = " ".repeat(PIECE_STATUS_COLUMN);
    let mut lines =
        crate::render::wrap(status, PLAN_WIDTH - PIECE_STATUS_COLUMN, &indent).into_iter();
    let first = lines
        .next()
        .map(|line| line.trim_start().to_string())
        .unwrap_or_default();
    let mut out = format!(
        "{:indent_width$}{label:<label_width$}{first}\n",
        "",
        indent_width = PIECE_INDENT,
        label_width = PIECE_LABEL_WIDTH
    );
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `label`'s value on its own indented line, never wrapped: `value` is a
/// command line or a path, and a command line broken across two lines is
/// not one anybody can paste -- `registry.rs` says the same of its own
/// such lines. `piece_line`'s status gained a width (`f720`); this did
/// not, on purpose.
///
/// The label column is as wide as the longest label any caller passes plus
/// two spaces. It was a fixed 15, and `UserPromptSubmit` (`d779`) is 16, so
/// its line printed with no space at all before the command.
pub(super) fn sub_line(label: &str, value: &str) -> String {
    format!("        {label:<SUB_LINE_LABEL_WIDTH$}{value}\n")
}

/// Every label [`sub_line`] is given, so the column is computed from them
/// rather than kept in step by hand: the three hook events and the `in`
/// that names where a tree lives.
const SUB_LINE_LABELS: [&str; 4] = ["SessionStart", "UserPromptSubmit", "Stop", "in"];

const SUB_LINE_LABEL_WIDTH: usize = {
    let mut widest = 0;
    let mut i = 0;
    while i < SUB_LINE_LABELS.len() {
        if SUB_LINE_LABELS[i].len() > widest {
            widest = SUB_LINE_LABELS[i].len();
        }
        i += 1;
    }
    widest + 2
};

// ---------------------------------------------------------------------------
// Applying: plan, ask, write. `d723` piece B: no plan of the tree side
// joins this one any more -- `run` has already confirmed, through
// `super::resolve_for_setup`, that this folder is either the tree's own or
// one of its declared lanes, so there is nothing left here to plant,
// declare or lock.
// ---------------------------------------------------------------------------

fn apply(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let here = &roots.here;
    let paths = paths(here);
    let settings = read_json(&paths.settings);
    let mcp = read_json(&paths.mcp);
    let skill_raw = std::fs::read_to_string(&paths.skill).ok();

    let mut conflicts: Vec<String> = Vec::new();
    if let Some((line, col)) = settings.parse_error {
        conflicts.push(unreadable_conflict(SETTINGS_LABEL, line, col));
    } else if settings.not_object {
        conflicts.push(not_object_conflict(SETTINGS_LABEL));
    }
    if let Some((line, col)) = mcp.parse_error {
        conflicts.push(unreadable_conflict(MCP_LABEL, line, col));
    } else if mcp.not_object {
        conflicts.push(not_object_conflict(MCP_LABEL));
    }

    let mcp_root = mcp.value.clone().unwrap_or_else(|| Value::object(vec![]));
    let mcp_server_state = if mcp.parse_error.is_none() && !mcp.not_object {
        mcp_state(&mcp_root)
    } else {
        McpState::Missing
    };
    if let McpState::NameTaken(cmd) = &mcp_server_state {
        conflicts.push(mcp_name_conflict(cmd));
    }

    let skill_file_state = match &skill_raw {
        None => SkillState::Missing,
        Some(text) => skill_state(text),
    };
    if matches!(skill_file_state, SkillState::Conflict) {
        conflicts.push(skill_conflict());
    }

    if !conflicts.is_empty() {
        let mut msg = conflicts.join("\n\n");
        msg.push_str("\n\n  Nothing written.");
        return Err(Failure::Model(msg));
    }

    let settings_root = settings.value.clone().unwrap();
    let start_hook_state = hook_state(
        &settings_root,
        "SessionStart",
        "start",
        SESSION_START_COMMAND,
    );
    let stop_hook_state = hook_state(&settings_root, "Stop", "end", SESSION_END_COMMAND);
    let prompt_hook_state = hook_state(
        &settings_root,
        "UserPromptSubmit",
        "prompt",
        SESSION_PROMPT_COMMAND,
    );

    let start_missing = matches!(start_hook_state, HookState::Missing);
    let stop_missing = matches!(stop_hook_state, HookState::Missing);
    let prompt_missing = matches!(prompt_hook_state, HookState::Missing);
    let mcp_missing = matches!(mcp_server_state, McpState::Missing);
    let skill_missing_or_replaceable = matches!(
        skill_file_state,
        SkillState::Missing | SkillState::Replaceable
    );

    let nothing_to_write = !start_missing
        && !stop_missing
        && !prompt_missing
        && !mcp_missing
        && !skill_missing_or_replaceable;

    let piece_block = render_piece_block(
        here,
        settings.exists,
        mcp.exists,
        &start_hook_state,
        &stop_hook_state,
        &prompt_hook_state,
        start_missing,
        stop_missing,
        prompt_missing,
        &mcp_server_state,
        &skill_file_state,
    );

    // Checked before `nothing_to_write`, not after: that branch notes the
    // registry (`note_registry`), and `--dry-run` promises to write
    // nothing anywhere, the machine's registry included (`t594`).
    // An already-set-up project asking for `--dry-run` used
    // to reach the other branch first and note it anyway.
    if a.has("dry-run") {
        outln!("{piece_block}{TRAILING_PARAGRAPH}\n  Nothing written: --dry-run.");
        return Ok(0);
    }

    if nothing_to_write {
        // A real run, never `--dry-run`, thanks to the check above: noting
        // the registry is bookkeeping every ordinary command already does
        // on a pure read, not a write this promise is about (`d723` piece
        // B: `setup` never writes to the tree itself any more, so this is
        // all `note_registry` is left doing).
        tree::note_registry(roots);
        outln!("{piece_block}  Nothing to write: this project is already set up.");
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::no_terminal_text(
            super::Harness::ClaudeCode,
            a,
        )));
    }

    print!("{piece_block}{TRAILING_PARAGRAPH}");
    let proceed = a.has("yes") || super::ask("\n  Write it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    // Build every write, then commit them together (`t565` §7.3: "se
    // pregunta una sola vez por todo y se escribe todo o nada"). `d723`
    // piece B: the tree's own `.gitignore` no longer joins this batch --
    // planting, the lane, the version lock and `.gitignore` are `init`'s
    // alone now, and this run never reaches any of them.
    let mut writes = Vec::new();
    if start_missing || stop_missing || prompt_missing {
        let mut new_settings = settings_root.clone();
        if start_missing {
            append_hook(
                &mut new_settings,
                "SessionStart",
                SESSION_START_COMMAND,
                None,
            );
        }
        if stop_missing {
            append_hook(&mut new_settings, "Stop", SESSION_END_COMMAND, None);
        }
        if prompt_missing {
            append_hook(
                &mut new_settings,
                "UserPromptSubmit",
                SESSION_PROMPT_COMMAND,
                None,
            );
        }
        let rendered = json::finalize(
            &json::render(&new_settings, &settings.indent),
            settings.eol,
            settings.trailing_newline,
        );
        let before = settings_root.clone();
        writes.push(super::PlannedWrite {
            path: paths.settings.clone(),
            action: super::Action::Write(rendered),
            original: settings.exists.then(|| settings.raw.clone().into_bytes()),
            preserved: Some(Box::new(move |updated| json::extends(&before, updated))),
        });
    }

    if mcp_missing {
        let mut new_mcp = mcp_root.clone();
        let mut servers = new_mcp
            .get("mcpServers")
            .cloned()
            .unwrap_or_else(|| Value::object(vec![]));
        servers.set("vivac", our_mcp_entry());
        new_mcp.set("mcpServers", servers);
        let rendered = json::finalize(
            &json::render(&new_mcp, &mcp.indent),
            mcp.eol,
            mcp.trailing_newline,
        );
        let before = mcp_root.clone();
        writes.push(super::PlannedWrite {
            path: paths.mcp.clone(),
            action: super::Action::Write(rendered),
            original: mcp.exists.then(|| mcp.raw.clone().into_bytes()),
            preserved: Some(Box::new(move |updated| json::extends(&before, updated))),
        });
    }

    if skill_missing_or_replaceable {
        writes.push(super::PlannedWrite::write(
            paths.skill.clone(),
            skill_text(),
            skill_raw.clone().map(String::into_bytes),
        ));
    }

    super::commit(&writes)?;

    // `d723` piece B: this run never touched the tree, so what it wrote is
    // exactly the three pieces above -- `needs_lock`, `lane_declared` and
    // the rest of what `tree::commit` used to report are `init`'s to say
    // now, not `setup`'s.
    let written = Written {
        connection: start_missing || stop_missing || prompt_missing || mcp_missing,
        // `f638`, `d641`: every tree `setup` writes into already existed
        // before this run, so the one question left is whether this run
        // is the one adding the "vivac" server to it.
        hand_registered_risk: mcp_missing,
        skill: skill_missing_or_replaceable,
        undoable: start_missing
            && stop_missing
            && prompt_missing
            && mcp_missing
            && matches!(skill_file_state, SkillState::Missing),
    };
    // Bookkeeping, not a write this promise is about (`note_registry`'s
    // own doc): the tree itself is untouched.
    tree::note_registry(roots);
    print!("\n{}", written_text(&written));
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn render_piece_block(
    here: &Path,
    settings_exists: bool,
    mcp_exists: bool,
    start_hook_state: &HookState,
    stop_hook_state: &HookState,
    prompt_hook_state: &HookState,
    start_missing: bool,
    stop_missing: bool,
    prompt_missing: bool,
    mcp_server_state: &McpState,
    skill_file_state: &SkillState,
) -> String {
    let mut s = format!("  vivac setup claude-code, in {}\n\n", here.display());

    // `t579` §4's warning: only when `here` sits inside a repository but is
    // not its root, so nobody has to guess which folder Claude Code was
    // actually opened in.
    if !here.join(".git").exists() {
        if let Some(git_root) = super::git_root_above(here) {
            s.push_str(&format!(
                "  This folder is inside the repository at {}, not at its root.\n  \
                 Claude Code reads these files only from the folder it is opened in: if\n  \
                 you open it at {}, run setup there instead.\n\n",
                git_root.display(),
                git_root.display()
            ));
        }
    }

    let settings_status = match (settings_exists, start_missing, stop_missing, prompt_missing) {
        (_, false, false, false) => "already has all three hooks",
        (_, true, false, false) => "add the SessionStart hook",
        (_, false, true, false) => "add the Stop hook",
        (_, false, false, true) => "add the UserPromptSubmit hook",
        (_, true, true, false) | (_, true, false, true) | (_, false, true, true) => "add two hooks",
        (false, true, true, true) => "create, with three hooks",
        (true, true, true, true) => "add three hooks",
    };
    s.push_str(&piece_line(SETTINGS_LABEL, settings_status));
    match start_hook_state {
        HookState::Missing => s.push_str(&sub_line("SessionStart", SESSION_START_COMMAND)),
        HookState::Different(cmd) => {
            s.push_str(&sub_line("SessionStart", &format!("already runs  {cmd}")))
        }
        HookState::Exact => {}
    }
    match stop_hook_state {
        HookState::Missing => s.push_str(&sub_line("Stop", SESSION_END_COMMAND)),
        HookState::Different(cmd) => s.push_str(&sub_line("Stop", &format!("already runs  {cmd}"))),
        HookState::Exact => {}
    }
    match prompt_hook_state {
        HookState::Missing => s.push_str(&sub_line("UserPromptSubmit", SESSION_PROMPT_COMMAND)),
        HookState::Different(cmd) => s.push_str(&sub_line(
            "UserPromptSubmit",
            &format!("already runs  {cmd}"),
        )),
        HookState::Exact => {}
    }

    let mcp_status = match mcp_server_state {
        McpState::Missing if !mcp_exists => "create, with the server \"vivac\"".to_string(),
        McpState::Missing => "add the server \"vivac\"".to_string(),
        McpState::Ours => "already has the server \"vivac\"".to_string(),
        McpState::OtherName(name) => format!("already runs vivac mcp as \"{name}\""),
        McpState::NameTaken(_) => unreachable!("a name conflict never reaches the plan"),
    };
    s.push_str(&piece_line(MCP_LABEL, &mcp_status));
    if matches!(mcp_server_state, McpState::Missing) {
        s.push_str("        vivac mcp\n");
    }

    match skill_file_state {
        SkillState::Missing => s.push_str(&piece_line(
            SKILL_LABEL,
            "create: how an agent brings another memory into vivac",
        )),
        SkillState::Replaceable => s.push_str(&piece_line(
            SKILL_LABEL,
            "replace the copy an earlier vivac wrote",
        )),
        SkillState::Same => s.push_str(&piece_line(SKILL_LABEL, "already there")),
        SkillState::Conflict => unreachable!("a skill conflict never reaches the plan"),
    }

    s.push('\n');
    s
}

// `pub(super)`: true of `codex.rs`'s own hooks and server too, and neither
// names Claude Code (`d653`).
pub(super) const TRAILING_PARAGRAPH: &str = "  The hooks run a command in every session, and the server is how the\n  agent writes to the tree. Nothing outside this directory is touched,\n  and no file is copied.\n";

/// What this run wrote, which decides how it ends (`t579` §15.5): a
/// paragraph is only printed when it is true of this run, and it says
/// what that run did, no more and no less (`t594`) -- every one of
/// these is a separate thing `apply` can write to the tree or the
/// folder, and any subset of them can be true together.
struct Written {
    /// A hook or the server, which only a new session picks up.
    connection: bool,
    /// This run added the "vivac" server to a tree that was already here
    /// before it (`f638`, `d641`): a hand-registered local-scope server
    /// from before `setup` existed can shadow the one this run just added,
    /// and nothing on screen says so. Always `false`
    /// when `connection` is, since this is never true without the server
    /// being part of what made `connection` true. `d723` piece B: every
    /// tree `setup` writes into already existed before this run, so this
    /// no longer has a plant to be conditioned on.
    hand_registered_risk: bool,
    /// The skill, where it was missing or an earlier release's copy.
    skill: bool,
    /// All four of setup's pieces, the skill among them missing before:
    /// `--undo` removes all four, so only then does it take back exactly
    /// this run.
    undoable: bool,
}

fn written_text(w: &Written) -> String {
    let mut s = String::from("  Written.\n");
    if w.connection {
        s.push_str(SESSION_PARAGRAPH);
        if w.hand_registered_risk {
            s.push_str(HAND_REGISTERED_PARAGRAPH);
        }
    } else if w.skill {
        s.push_str(SKILL_PARAGRAPH);
    }
    s.push_str(FILES_PARAGRAPH);
    if w.undoable {
        s.push_str(UNDO_LINE);
    }
    s
}

/// The paragraph about the tree itself: `tree_kept_paragraph` when none of
/// the three actually happened, and one sentence naming exactly the ones
/// that did otherwise -- never more than what this run wrote, and never
/// silent about any of it.
///
/// The three are independent, and saying so in the type is the fix: they
/// were mutually exclusive branches before, so a run that did two of them
/// could only name one, and a run that only wrote the tree's `.gitignore`
/// had no branch at all and claimed to have changed nothing -- two lines
/// under its own plan announcing that write (`t594`).
///
/// `pub(super)`: `init.rs` is this function's only caller since `d723`
/// piece B -- `setup` itself never writes to the tree any more, so `actor`
/// is always `"init"` at the one call site left. Kept as a parameter even
/// so, rather than a literal `"init"` inlined below: renaming a plain
/// helper that already says what it takes is not this piece's job.
pub(super) fn tree_paragraph(
    actor: &str,
    gitignore_created: bool,
    lane_declared: bool,
    config_locked: bool,
) -> String {
    let mut clauses = Vec::new();
    if gitignore_created {
        clauses.push("its own .gitignore");
    }
    if lane_declared {
        clauses.push("this folder's own thread");
    }
    if config_locked {
        clauses.push("the sentence that stops an older vivac from reading it");
    }
    if clauses.is_empty() {
        return tree_kept_paragraph(actor);
    }
    // Noun phrases rather than verb phrases: they share one subject, so
    // two of them join without the reader having to carry a verb across
    // the list, and none of them can be read as belonging to this run
    // rather than to the tree.
    format!(
        "\n{}",
        wrapped(&format!(
            "The tree was already there, and {actor} wrote in it: {}.",
            join_with_and(&clauses)
        ))
    )
}

/// `text`, wrapped to the same width every other paragraph in this file
/// already wraps to by hand, each line indented by two spaces. A plain
/// greedy word wrap is all this needs: nothing it ever wraps runs past a
/// short sentence naming one to three clauses.
///
/// `pub(super)`: `codex.rs` wraps its own one-sentence refusals the same
/// way, rather than hand-wrapping each one to the same width again.
pub(super) fn wrapped(text: &str) -> String {
    const WIDTH: usize = 76;
    let mut out = String::new();
    let mut line = String::from("  ");
    for word in text.split_whitespace() {
        if line.len() + word.len() + 1 > WIDTH && line.trim() != "" {
            out.push_str(line.trim_end());
            out.push('\n');
            line = String::from("  ");
        }
        line.push_str(word);
        line.push(' ');
    }
    out.push_str(line.trim_end());
    out.push('\n');
    out
}

/// `name`'s own collision paragraph (`t640`, point 10 bis): `name` already
/// names another project on this machine, wrapped through [`wrapped`]
/// rather than split by hand the way this used to be written twice, once
/// here and once in `init.rs` (`f724`). A product's own name runs up to
/// `tree::NAME_MAX_LEN` characters, none of them this run's to shorten, so
/// a break placed by hand before it was ever typed could not promise to
/// still land under the width once it was in -- measured: sixty-one fixed
/// characters ahead of the break this used to have, so any name sixteen
/// characters or longer already ran past it.
///
/// `pub(super)`: `init.rs`'s own plan reads this (`d723` piece A) -- `--name`
/// moved there with the rest of the tree side in piece B, so `init` is this
/// function's only caller left.
pub(super) fn name_collision_paragraph(name: &str) -> String {
    format!(
        "{}\n",
        wrapped(&format!(
            "\"{name}\" already names another project on this machine. With both \
             answering to it, --join will need a path instead of the name: two \
             projects that share a name give it nothing to tell them apart by."
        ))
    )
}

/// `items`, in English list form: one on its own, two joined by "and",
/// three or more comma-separated with "and" before the last.
///
/// `pub(super)`: `tree::tree_below_refusal` joins a list of folder names
/// the same way, rather than fixing the same rule a second time.
pub(super) fn join_with_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = items.split_last().expect("checked non-empty above");
            format!("{} and {last}", rest.join(", "))
        }
    }
}

const SESSION_PARAGRAPH: &str = "\n  Open a new Claude Code session in this folder. The brief arrives on its\n  own when it starts. If Claude Code asks whether to use the \"vivac\" server\n  from .mcp.json, say yes: it is what lets the agent write to the tree.\n";

/// `f638`: before `setup` existed, the README told people to run
/// `claude mcp add vivac -- vivac mcp`, which registers the server in
/// Claude Code's local scope. Claude Code connects to a same-named server
/// once, preferring local scope over the project scope `.mcp.json` holds,
/// so an entry this run adds there can go silently unused.
///
/// setup never reads a harness's personal configuration to check for a
/// hand-made registration directly: for Claude Code that file
/// (`~/.claude.json`) also holds the sign-in session, and the security
/// pillar vetoes opening it (`d641`). So the condition below is inferred
/// from the project instead -- the tree was here before this run, and
/// this run is the one adding the "vivac" server to `.mcp.json` -- rather
/// than read from the harness itself.
///
/// Each harness `setup` covers later says this in its own words and with
/// its own command, at this same point in its closing message.
const HAND_REGISTERED_PARAGRAPH: &str = "\n  The tree was here before this server was. If you once registered vivac\n  by hand with claude mcp add, Claude Code keeps using that registration\n  and not this one. To keep only this one, run from this folder:\n\n      claude mcp remove vivac -s local\n";

const SKILL_PARAGRAPH: &str = "\n  The vivac-migrate skill is now the one this version of vivac ships.\n  Sessions opened from now on use it.\n";

/// `pub(super)`: `init.rs` is this constant's only caller since `d723`
/// piece B -- planting is `init`'s alone now, and this is what a fresh
/// plant has always said about bringing in what the project already
/// knows. The text is unchanged; only the module that shows it moved,
/// with planting.
pub(super) const MIGRATE_PARAGRAPHS: &str = "\n  Nothing has been brought in from anywhere yet. To bring in what this\n  project already knows, from another memory system, the harness's own\n  memory, instruction files or its documents, ask the agent:\n\n      Use the vivac-migrate skill to bring everything this project knows\n      into vivac.\n\n  It shows you a plan before writing anything, checks what it wrote, and\n  offers to retire the other maps one at a time, only if you say yes.\n\n  Until then, another memory system you use keeps talking to the agent as\n  before, and may tell it to use that system first. That is expected: the\n  skill only reads from it.\n";

/// `f678`/`d683`: [`MIGRATE_PARAGRAPHS`]'s own argument was for the
/// **tree**, which a join finds already there and may already carry
/// content for -- true, and beside the point. This folder's own
/// instruction files, the harness's memory and its documents came with
/// the folder, not the tree, and joining a tree never reads any of that.
///
/// `pub(super)`: `init.rs` is this constant's only caller since `d723`
/// piece B -- joining is `init`'s alone now, and this is what joining an
/// existing tree has always said about this folder's own knowledge, apart
/// from the tree's. The text is unchanged; only the module that shows it
/// moved, with joining.
pub(super) const JOIN_MIGRATE_PARAGRAPHS: &str = "\n  This folder's own knowledge is not in the tree. Instruction files, the\n  harness's memory and the documents that live here came with the folder,\n  and joining a tree does not read them. To bring them in, ask the agent:\n\n      Use the vivac-migrate skill to bring everything this project knows\n      into vivac.\n\n  The tree already has content, and the skill expects that: it looks at\n  what is there before writing, and proposes a note on the node that\n  already says it rather than a duplicate.\n";

/// `tree_paragraph`'s own text for nothing changed, naming `actor` the same
/// way its other sentence does.
fn tree_kept_paragraph(actor: &str) -> String {
    format!("\n  The tree was already there, and {actor} changed nothing in it.\n")
}

const FILES_PARAGRAPH: &str = "\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n";

const UNDO_LINE: &str = "\n  Undo:  vivac setup claude-code --undo\n";

/// The words come from `anchor::EVENTS_TRACKED_WARNING` (`f619`), wrapped
/// to this file's own paragraph width: `check` reads that very same
/// constant, so the two can no longer drift the way they once did, and
/// `check`'s copy never named a worktree at all.
///
/// `pub(super)`: `codex.rs` shows this same warning after a plant too
/// (`t592` tranche 2, `d710`), rather than a copy of the wrapping.
pub(super) fn tracked_git_warning() -> String {
    format!("\n{}", wrapped(crate::anchor::EVENTS_TRACKED_WARNING))
}

/// `pub(super)`: `codex.rs` reports the same conflict for its own
/// `.codex/hooks.json`, naming its own label (`t592` tranche 2, piece B).
pub(super) fn unreadable_conflict(label: &str, line: usize, column: usize) -> String {
    format!(
        "  {label} is not JSON setup can read (line {line}, column {column}), so\n  \
         it will not touch it: a file it cannot read is a file it could only\n  \
         overwrite."
    )
}

pub(super) fn not_object_conflict(label: &str) -> String {
    format!(
        "  {label} holds JSON whose top level is not an object, so setup will\n  \
         not touch it: a file it cannot read is a file it could only overwrite."
    )
}

fn mcp_name_conflict(command_and_args: &str) -> String {
    format!(
        "  .mcp.json already has a server called \"vivac\", and it does not run\n  \
         vivac mcp:\n      {command_and_args}\n  \
         setup never rewrites an entry it did not write. Rename or remove that\n  \
         one, then run setup again."
    )
}

fn skill_conflict() -> String {
    "  .claude/skills/vivac-migrate/SKILL.md is already there, and either setup\n  \
     did not write it or it was changed since. setup never overwrites it:\n  \
     move it away, then run setup again."
        .to_string()
}

// ---------------------------------------------------------------------------
// `--undo`.
// ---------------------------------------------------------------------------

/// `d723` piece B: `--undo` takes off only the three pieces this harness
/// itself wrote. Neither the lane nor the tree is `setup`'s to touch, so
/// this reads and writes `here` alone -- no `Roots`, no `resolve_for_setup`,
/// and none of the tree's own refusals: undoing whatever an earlier setup
/// wrote is always safe, regardless of what the tree above `here` is doing.
fn undo(here: &Path, a: &Args) -> Result<i32, Failure> {
    let paths = paths(here);
    let settings = read_json(&paths.settings);
    let mcp = read_json(&paths.mcp);
    let skill_raw = std::fs::read_to_string(&paths.skill).ok();

    let mut conflicts: Vec<String> = Vec::new();
    if let Some((line, col)) = settings.parse_error {
        conflicts.push(unreadable_conflict(SETTINGS_LABEL, line, col));
    } else if settings.not_object {
        conflicts.push(not_object_conflict(SETTINGS_LABEL));
    }
    if let Some((line, col)) = mcp.parse_error {
        conflicts.push(unreadable_conflict(MCP_LABEL, line, col));
    } else if mcp.not_object {
        conflicts.push(not_object_conflict(MCP_LABEL));
    }
    if !conflicts.is_empty() {
        let mut msg = conflicts.join("\n\n");
        msg.push_str("\n\n  Nothing written.");
        return Err(Failure::Model(msg));
    }

    let settings_root = settings.value.clone().unwrap();
    let start_hook_state = hook_state(
        &settings_root,
        "SessionStart",
        "start",
        SESSION_START_COMMAND,
    );
    let stop_hook_state = hook_state(&settings_root, "Stop", "end", SESSION_END_COMMAND);
    let prompt_hook_state = hook_state(
        &settings_root,
        "UserPromptSubmit",
        "prompt",
        SESSION_PROMPT_COMMAND,
    );
    let mcp_root = mcp.value.clone().unwrap();
    let mcp_server_state = mcp_state(&mcp_root);
    let skill_ours = skill_raw.as_deref().is_some_and(skill_fingerprint_intact);

    let start_ours = matches!(start_hook_state, HookState::Exact);
    let stop_ours = matches!(stop_hook_state, HookState::Exact);
    let prompt_ours = matches!(prompt_hook_state, HookState::Exact);
    let mcp_ours = matches!(mcp_server_state, McpState::Ours);

    let nothing_to_undo = !start_ours && !stop_ours && !prompt_ours && !mcp_ours && !skill_ours;
    if nothing_to_undo {
        outln!("  Nothing to undo: none of what setup writes is here.");
        return Ok(0);
    }

    // Preview the settings.json result to know whether it empties out.
    let mut preview = settings_root.clone();
    if start_ours {
        remove_hook(&mut preview, "SessionStart", SESSION_START_COMMAND);
    }
    if stop_ours {
        remove_hook(&mut preview, "Stop", SESSION_END_COMMAND);
    }
    if prompt_ours {
        remove_hook(&mut preview, "UserPromptSubmit", SESSION_PROMPT_COMMAND);
    }
    let settings_becomes_empty = preview
        .as_object()
        .is_some_and(|s: &[(String, Value)]| s.is_empty());

    let mcp_becomes_empty = mcp_ours
        && without_our_mcp_server(&mcp_root)
            .as_object()
            .is_some_and(|s: &[(String, Value)]| s.is_empty());

    let settings_status: String = match (start_ours, stop_ours, prompt_ours) {
        (true, true, true) if settings_becomes_empty => {
            "remove the three hooks setup wrote; nothing else is left, so it goes".to_string()
        }
        (true, true, true) => "remove the three hooks setup wrote".to_string(),
        (true, true, false) => "remove the two hooks setup wrote".to_string(),
        (true, false, true) => "remove the two hooks setup wrote".to_string(),
        (false, true, true) => "remove the two hooks setup wrote".to_string(),
        (true, false, false) => "remove the SessionStart hook".to_string(),
        (false, true, false) => "remove the Stop hook".to_string(),
        (false, false, true) => "remove the UserPromptSubmit hook".to_string(),
        (false, false, false) => "left as it is".to_string(),
    };

    let mut s = format!(
        "  vivac setup claude-code --undo, in {}\n\n",
        here.display()
    );
    s.push_str(&piece_line(SETTINGS_LABEL, &settings_status));
    if let HookState::Different(_) = &start_hook_state {
        s.push_str(&sub_line(
            "SessionStart",
            "runs vivac another way; left as it is",
        ));
    }
    if let HookState::Different(_) = &stop_hook_state {
        s.push_str(&sub_line("Stop", "runs vivac another way; left as it is"));
    }
    if let HookState::Different(_) = &prompt_hook_state {
        s.push_str(&sub_line(
            "UserPromptSubmit",
            "runs vivac another way; left as it is",
        ));
    }

    s.push_str(&piece_line(
        MCP_LABEL,
        if mcp_becomes_empty {
            "remove the server \"vivac\"; nothing else is left, so it goes"
        } else if mcp_ours {
            "remove the server \"vivac\""
        } else {
            "left as it is"
        },
    ));

    s.push_str(&piece_line(
        SKILL_LABEL,
        if skill_ours {
            "remove"
        } else if skill_raw.is_some() {
            "changed since setup wrote it; left as it is"
        } else {
            "left as it is"
        },
    ));

    s.push('\n');

    if a.has("dry-run") {
        outln!("{s}  Nothing written: --dry-run.");
        return Ok(0);
    }

    // `f718`: the same guard `apply` has had all along, missing
    // here. Without it a run with nobody to answer printed the question
    // anyway, removed nothing, and exited 0 -- and 0 with nothing done is
    // what a script reads as done.
    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::no_terminal_text(
            super::Harness::ClaudeCode,
            a,
        )));
    }

    print!("{s}");
    let proceed = a.has("yes") || super::ask("  Undo it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    // Every change -- a rewrite or a removal -- is one commit, the same
    // all-or-nothing guarantee `apply` gives (`t565` §7.6).
    let mut writes = Vec::new();
    if start_ours || stop_ours || prompt_ours {
        let original = settings.raw.clone().into_bytes();
        if settings_becomes_empty {
            writes.push(super::PlannedWrite::delete(
                paths.settings.clone(),
                original,
            ));
        } else {
            let mut new_settings = settings_root.clone();
            if start_ours {
                remove_hook(&mut new_settings, "SessionStart", SESSION_START_COMMAND);
            }
            if stop_ours {
                remove_hook(&mut new_settings, "Stop", SESSION_END_COMMAND);
            }
            if prompt_ours {
                remove_hook(
                    &mut new_settings,
                    "UserPromptSubmit",
                    SESSION_PROMPT_COMMAND,
                );
            }
            let rendered = json::finalize(
                &json::render(&new_settings, &settings.indent),
                settings.eol,
                settings.trailing_newline,
            );
            let before = settings_root.clone();
            writes.push(super::PlannedWrite {
                path: paths.settings.clone(),
                action: super::Action::Write(rendered),
                original: Some(original),
                preserved: Some(Box::new(move |updated| {
                    json::contained_in(updated, &before)
                })),
            });
        }
    }
    if mcp_ours {
        let original = mcp.raw.clone().into_bytes();
        if mcp_becomes_empty {
            writes.push(super::PlannedWrite::delete(paths.mcp.clone(), original));
        } else {
            let new_mcp = without_our_mcp_server(&mcp_root);
            let rendered = json::finalize(
                &json::render(&new_mcp, &mcp.indent),
                mcp.eol,
                mcp.trailing_newline,
            );
            let before = mcp_root.clone();
            writes.push(super::PlannedWrite {
                path: paths.mcp.clone(),
                action: super::Action::Write(rendered),
                original: Some(original),
                preserved: Some(Box::new(move |updated| {
                    json::contained_in(updated, &before)
                })),
            });
        }
    }
    if skill_ours {
        writes.push(super::PlannedWrite::delete(
            paths.skill.clone(),
            skill_raw.clone().unwrap().into_bytes(),
        ));
    }

    super::commit(&writes)?;

    // Best-effort, and only once the commit above is known to have
    // succeeded: an empty directory left behind costs nothing to leave for
    // a later run, but is tidier gone.
    if skill_ours {
        remove_if_empty(paths.skill.parent());
        remove_if_empty(paths.skill.parent().and_then(Path::parent));
        remove_if_empty(
            paths
                .skill
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent),
        );
    }

    outln!("  Undone. The tree in .vivac/ is untouched.");
    Ok(0)
}

/// `pub(super)`: `codex.rs` cleans up its own empty directories with this
/// same best-effort removal after its own `--undo` commit (`t592` tranche 2,
/// piece C).
pub(super) fn remove_if_empty(dir: Option<&Path>) {
    if let Some(dir) = dir {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_vivac_command_strips_quotes_path_and_extension() {
        assert!(is_vivac_command("vivac"));
        assert!(is_vivac_command("VIVAC"));
        assert!(is_vivac_command("\"vivac\""));
        assert!(is_vivac_command("C:/tools/vivac.exe"));
        assert!(is_vivac_command("C:\\tools\\vivac.EXE"));
        assert!(is_vivac_command("/usr/local/bin/vivac"));
        assert!(!is_vivac_command("vivacx"));
        assert!(!is_vivac_command("notvivac"));
    }

    /// Every label that reaches `sub_line` keeps at least two spaces before
    /// its value. The fixed column of 15 printed `UserPromptSubmitvivac
    /// session prompt --hook` in 0.15.1.
    #[test]
    fn every_sub_line_label_keeps_two_spaces_before_its_value() {
        for label in SUB_LINE_LABELS {
            let line = sub_line(label, "VALUE");
            let gap = line.trim_start().trim_start_matches(label);
            assert!(
                gap.starts_with("  "),
                "{label} leaves {gap:?} before its value: {line:?}"
            );
        }
    }

    #[test]
    fn the_fingerprint_matches_the_known_hash_of_the_literal_text() {
        // Computed independently (Python's own FNV-1a/64) over the exact
        // frontmatter and body this file embeds. It moves whenever the
        // skill's text does, and moving it is meant to be deliberate: the
        // last time was `t767`, which made the skill check every source can
        // still be found, work in batches and close lessons as records,
        // after the migration of a real project lost three sources in four.
        assert_eq!(skill_fingerprint(), 0x0f51521030b6eafe);
    }

    #[test]
    fn extract_marker_reads_back_what_skill_text_writes() {
        let text = skill_text();
        let (fp_hex, content) = extract_marker(&text).unwrap();
        let fp = u64::from_str_radix(&fp_hex, 16).unwrap();
        assert_eq!(fp, skill_fingerprint());
        assert_eq!(content, skill_content_without_marker());
    }
}
