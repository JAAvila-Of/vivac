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
use crate::style::{self, Stream};
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
// Formatting (`d792`, `t789`): a plan is data first -- one `PlanItem` per
// row, its verb and its "what" already apart -- and rendered second, so
// every column's width comes from the items actually in this plan rather
// than a fixed guess (`t565` §7.8's old 41 named no file at all once a
// label like `.vivac/config` grew past it). `codex.rs` and `tree.rs` build
// the same `PlanItem`s and render them through the same two functions,
// rather than fixing the columns a second time (`d653`).
// ---------------------------------------------------------------------------

/// One row of a plan: a verb, the path or label it acts on, what it does
/// in plain words, and the value lines (a hook's command, a tree's own
/// path) that sit under it, dim and indented to the path column.
pub(super) struct PlanItem {
    pub(super) verb: &'static str,
    pub(super) path: String,
    pub(super) what: String,
    pub(super) sub: Vec<(String, String)>,
}

impl PlanItem {
    pub(super) fn new(
        verb: &'static str,
        path: impl Into<String>,
        what: impl Into<String>,
    ) -> PlanItem {
        PlanItem {
            verb,
            path: path.into(),
            what: what.into(),
            sub: Vec::new(),
        }
    }

    pub(super) fn with_sub(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
    ) -> PlanItem {
        self.sub.push((label.into(), value.into()));
        self
    }
}

/// The line every plan opens with: the command in bold, the folder it acts
/// on dim, never a verb of its own -- each row below names its own.
pub(super) fn heading(stream: Stream, cmd: &str, here: &std::path::Path) -> String {
    format!(
        "{} will, in {}:\n\n",
        style::bold(stream, cmd),
        style::dim(stream, &here.display().to_string())
    )
}

/// `items`, rendered: every column padded to the widest plain text in that
/// column across the whole plan, two spaces of gap after each, so a run
/// with one long path never drags every other row's own width up with it
/// column by column but *does* keep its own row's columns lined up with
/// the rest. Never wrapped (`d792`): a plan item is one line, and the
/// terminal is what wraps it if it has to.
pub(super) fn render_items(stream: Stream, items: &[PlanItem]) -> String {
    let verb_width = items.iter().map(|i| i.verb.len()).max().unwrap_or(0);
    let path_width = items.iter().map(|i| i.path.len()).max().unwrap_or(0);
    let sub_label_width = items
        .iter()
        .flat_map(|i| i.sub.iter().map(|(label, _)| label.len()))
        .max()
        .unwrap_or(0);
    let sub_indent = " ".repeat(2 + verb_width + 2);
    let mut s = String::new();
    for item in items {
        s.push_str("  ");
        s.push_str(&style::verb(stream, item.verb));
        s.push_str(&" ".repeat(verb_width - item.verb.len() + 2));
        s.push_str(&style::path(stream, &item.path));
        s.push_str(&" ".repeat(path_width - item.path.len() + 2));
        s.push_str(&item.what);
        s.push('\n');
        for (label, value) in &item.sub {
            let line = format!(
                "{label}{}  {value}",
                " ".repeat(sub_label_width - label.len())
            );
            s.push_str(&sub_indent);
            s.push_str(&style::dim(stream, &line));
            s.push('\n');
        }
    }
    s
}

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

    let mut plan_block = heading(Stream::Out, "vivac setup claude-code", here);
    if let Some(warning) = git_root_warning(here) {
        plan_block.push_str(&warning);
        plan_block.push_str("\n\n");
    }
    plan_block.push_str(&render_items(
        Stream::Out,
        &plan_items(
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
        ),
    ));

    // Checked before `nothing_to_write`, not after: that branch notes the
    // registry (`note_registry`), and `--dry-run` promises to write
    // nothing anywhere, the machine's registry included (`t594`).
    // An already-set-up project asking for `--dry-run` used
    // to reach the other branch first and note it anyway.
    if a.has("dry-run") {
        outln!(
            "{}",
            close_with(&format!("{plan_block}\n{TRAILING_PARAGRAPH}"), DRY_RUN_LINE)
        );
        return Ok(0);
    }

    if nothing_to_write {
        // A real run, never `--dry-run`, thanks to the check above: noting
        // the registry is bookkeeping every ordinary command already does
        // on a pure read, not a write this promise is about (`d723` piece
        // B: `setup` never writes to the tree itself any more, so this is
        // all `note_registry` is left doing).
        tree::note_registry(roots);
        outln!(
            "{}",
            close_with(
                &plan_block,
                "Nothing to write: this project is already set up."
            )
        );
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::no_terminal_text(
            super::Harness::ClaudeCode,
            a,
        )));
    }

    print_plan(&format!("{plan_block}\n{TRAILING_PARAGRAPH}"));
    let proceed = a.has("yes") || super::ask("\nWrite it? [y/N] ");
    if !proceed {
        outln!("\nNothing written.");
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
        // `t789`: since `init` plants on its own, every tree predates this
        // server, so "the tree was here first" no longer tells a hand
        // registration apart from a fresh plant. A hand registration is
        // something done to a tree in use, so a tree with no work in it
        // yet does not get the warning.
        hand_registered_risk: mcp_missing && tree_has_work(roots),
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
    // `f790`: the migrate advice moved off `init` and onto the first
    // successful `setup` of a lane that has not brought anything in yet.
    print!("{}", migrate_advice(roots));
    Ok(0)
}

/// `t579` §4's warning, reflowed to one line: only when `here` sits inside
/// a repository but is not its root, so nobody has to guess which folder
/// Claude Code was actually opened in.
pub(super) fn git_root_warning(here: &Path) -> Option<String> {
    if here.join(".git").exists() {
        return None;
    }
    let git_root = super::git_root_above(here)?;
    Some(format!(
        "This folder is inside the repository at {git_root}, not at its root. \
         Claude Code reads these files only from the folder it is opened in: if you \
         open it at {git_root}, run setup there instead.",
        git_root = git_root.display()
    ))
}

#[allow(clippy::too_many_arguments)]
fn plan_items(
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
) -> Vec<PlanItem> {
    let mut items = Vec::new();

    let (settings_verb, settings_what) =
        match (settings_exists, start_missing, stop_missing, prompt_missing) {
            (_, false, false, false) => ("keep", "already has all three hooks"),
            (_, true, false, false) => ("add", "the SessionStart hook"),
            (_, false, true, false) => ("add", "the Stop hook"),
            (_, false, false, true) => ("add", "the UserPromptSubmit hook"),
            (_, true, true, false) | (_, true, false, true) | (_, false, true, true) => {
                ("add", "two hooks")
            }
            (false, true, true, true) => ("create", "three hooks"),
            (true, true, true, true) => ("add", "three hooks"),
        };
    let mut settings_item = PlanItem::new(settings_verb, SETTINGS_LABEL, settings_what);
    match start_hook_state {
        HookState::Missing => {
            settings_item = settings_item.with_sub("SessionStart", SESSION_START_COMMAND)
        }
        HookState::Different(cmd) => {
            settings_item = settings_item.with_sub("SessionStart", format!("already runs  {cmd}"))
        }
        HookState::Exact => {}
    }
    match stop_hook_state {
        HookState::Missing => settings_item = settings_item.with_sub("Stop", SESSION_END_COMMAND),
        HookState::Different(cmd) => {
            settings_item = settings_item.with_sub("Stop", format!("already runs  {cmd}"))
        }
        HookState::Exact => {}
    }
    match prompt_hook_state {
        HookState::Missing => {
            settings_item = settings_item.with_sub("UserPromptSubmit", SESSION_PROMPT_COMMAND)
        }
        HookState::Different(cmd) => {
            settings_item =
                settings_item.with_sub("UserPromptSubmit", format!("already runs  {cmd}"))
        }
        HookState::Exact => {}
    }
    items.push(settings_item);

    let (mcp_verb, mcp_what): (&'static str, String) = match mcp_server_state {
        McpState::Missing if !mcp_exists => ("create", "the \"vivac\" server".to_string()),
        McpState::Missing => ("add", "the \"vivac\" server".to_string()),
        McpState::Ours => ("keep", "already has the \"vivac\" server".to_string()),
        McpState::OtherName(name) => ("keep", format!("already runs vivac mcp as \"{name}\"")),
        McpState::NameTaken(_) => unreachable!("a name conflict never reaches the plan"),
    };
    let mut mcp_item = PlanItem::new(mcp_verb, MCP_LABEL, mcp_what);
    if matches!(mcp_server_state, McpState::Missing) {
        mcp_item = mcp_item.with_sub("run", "vivac mcp");
    }
    items.push(mcp_item);

    let (skill_verb, skill_what) = match skill_file_state {
        SkillState::Missing => ("create", "how an agent brings another memory into vivac"),
        SkillState::Replaceable => ("replace", "the copy an earlier vivac wrote"),
        SkillState::Same => ("keep", "already there"),
        SkillState::Conflict => unreachable!("a skill conflict never reaches the plan"),
    };
    items.push(PlanItem::new(skill_verb, SKILL_LABEL, skill_what));

    items
}

// `pub(super)`: true of `codex.rs`'s own hooks and server too, and neither
// names Claude Code (`d653`).
pub(super) const TRAILING_PARAGRAPH: &str = "The hooks run a command in every session, and the server is how the agent writes to the tree. Nothing outside this directory is touched, and no file is copied.";

/// The plain sentence a plan to write always ends with, once every
/// optional warning ahead of it has had its own paragraph.
pub(super) const DRY_RUN_LINE: &str = "Nothing written: --dry-run.";

/// `body`, trimmed of its own trailing blank lines, then exactly one blank
/// line, then `tail`: every plan this module prints ends this way,
/// regardless of which optional paragraphs `body` happened to include
/// (`d792` -- never the double blank a paragraph's own trailing blank line
/// and this join both leaving one used to add up to).
pub(super) fn close_with(body: &str, tail: &str) -> String {
    format!("{}\n\n{tail}", body.trim_end_matches('\n'))
}

/// Prints `body` trimmed of its own trailing newlines, followed by
/// exactly one -- ahead of the prompt this always precedes. Built as a
/// variable and printed through `"{line}"` rather than `print!("{}\n",
/// ...)` on purpose: a format string ending in `\n` reads to `clippy` as a
/// plain `println!`, which `no_println` bans under `src/` (`outln!` is
/// its replacement, and a prompt has no line of its own yet to buffer).
pub(super) fn print_plan(body: &str) {
    let line = format!("{}\n", body.trim_end_matches('\n'));
    print!("{line}");
}

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
    let mut paragraphs = vec![style::good(Stream::Out, "Written.")];
    if w.connection {
        paragraphs.push(SESSION_PARAGRAPH.to_string());
        if w.hand_registered_risk {
            paragraphs.push(hand_registered_paragraph());
        }
    } else if w.skill {
        paragraphs.push(SKILL_PARAGRAPH.to_string());
    }
    paragraphs.push(FILES_PARAGRAPH.to_string());
    if w.undoable {
        paragraphs.push(undo_line());
    }
    format!("{}\n", paragraphs.join("\n\n"))
}

/// `Next:` at the end of a run this lane had never captured anything
/// before -- the whole tree's own text -- or had captured, but not from
/// this lane -- the narrower one. `f790`: this used to be `init`'s own
/// text, shown on the plant or the join itself; it moved here because the
/// question it answers ("has anything of mine landed in this tree yet?")
/// is not settled by planting or joining, only by writing, and `setup` is
/// the first write a fresh lane usually makes. Empty once this lane
/// already has a capture of its own, or once there is nowhere to read the
/// log from at all -- never blocks a run that could not check.
/// Whether the tree `roots` names has had any work land on it, in any
/// lane. Unreadable reads as no work.
fn tree_has_work(roots: &super::Roots) -> bool {
    crate::store::Store::open(roots.tree.clone())
        .and_then(|store| store.read_all())
        .map(|(events, _)| crate::session::capture_count(&events) > 0)
        .unwrap_or(false)
}

pub(super) fn migrate_advice(roots: &super::Roots) -> String {
    let Some(located) = &roots.located else {
        return String::new();
    };
    let Ok(store) = crate::store::Store::open(roots.tree.clone()) else {
        return String::new();
    };
    let Ok((events, _)) = store.read_all() else {
        return String::new();
    };
    let lane_id = located
        .lane
        .as_ref()
        .map(|l| l.id.as_str())
        .unwrap_or(crate::lane::MAIN);
    if crate::session::capture_count(&events) == 0 {
        format!("\n{}", migrate_next_block())
    } else if crate::session::lane_capture_count(&events, lane_id) == 0 {
        format!("\n{}", join_migrate_next_block())
    } else {
        String::new()
    }
}

/// The heading every `Next:` block after a migrate nudge shares, and the
/// bold command line under it: `f790` moved both off `init` and reused
/// them for whichever of the two paragraphs below actually applies.
fn migrate_next_heading() -> String {
    format!(
        "{} bring in what this project already knows. Ask the agent:\n\n  {}",
        style::bold(Stream::Out, "Next:"),
        style::bold(
            Stream::Out,
            "Use the vivac-migrate skill to bring everything this project knows into vivac."
        )
    )
}

/// Nothing in the whole tree has captured anything yet.
fn migrate_next_block() -> String {
    format!(
        "{}\n\n{}\n\n{}\n",
        migrate_next_heading(),
        "It shows you a plan before writing anything, checks what it wrote, and offers \
         to retire the other maps one at a time, only if you say yes.",
        "Until then, another memory system you use keeps talking to the agent as before, \
         and may tell it to use that system first. That is expected: the skill only reads \
         from it."
    )
}

/// The tree has captures, none of them this lane's own: `f678`/`d683`'s own
/// argument was for the tree, which a join finds already there and may
/// already carry content for -- true, and beside the point. This folder's
/// own instruction files, the harness's memory and its documents came
/// with the folder, not the tree, and joining a tree never reads any of
/// that.
fn join_migrate_next_block() -> String {
    format!(
        "{}\n\n{}\n\n{}\n",
        migrate_next_heading(),
        "This folder's own knowledge is not in the tree. Instruction files, the harness's \
         memory and the documents that live here came with the folder, and joining a tree \
         does not read them.",
        "The tree already has content, and the skill expects that: it looks at what is \
         there before writing, and proposes a note on the node that already says it \
         rather than a duplicate."
    )
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
        "The tree was already there, and {actor} wrote in it: {}.",
        join_with_and(&clauses)
    )
}

/// `name`'s own collision paragraph (`t640`, point 10 bis): `name` already
/// names another project on this machine. A product's own name runs up to
/// `tree::NAME_MAX_LEN` characters, none of them this run's to shorten, so
/// this is never wrapped by hand (`d792`): the terminal wraps it if it has
/// to.
///
/// `pub(super)`: `init.rs`'s own plan reads this (`d723` piece A) -- `--name`
/// moved there with the rest of the tree side in piece B, so `init` is this
/// function's only caller left.
pub(super) fn name_collision_paragraph(name: &str) -> String {
    format!(
        "\"{name}\" already names another project on this machine. With both answering \
         to it, {} will need a path instead of the name: two projects that share a name \
         give it nothing to tell them apart by.\n\n",
        style::bold(Stream::Out, "--join")
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

const SESSION_PARAGRAPH: &str = "Open a new Claude Code session in this folder. The brief arrives on its own when it starts. If Claude Code asks whether to use the \"vivac\" server from .mcp.json, say yes: it is what lets the agent write to the tree.";

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
/// A function rather than a constant: the command it names is code-ish
/// prose (`d792`), on its own bold line rather than folded into the
/// sentence around it.
fn hand_registered_paragraph() -> String {
    format!(
        "The tree was here before this server was. If you once registered vivac by hand \
         with claude mcp add, Claude Code keeps using that registration and not this one. \
         To keep only this one, run from this folder:\n\n  {}",
        style::bold(Stream::Out, "claude mcp remove vivac -s local")
    )
}

const SKILL_PARAGRAPH: &str =
    "The vivac-migrate skill is now the one this version of vivac ships. Sessions opened \
     from now on use it.";

/// `tree_paragraph`'s own text for nothing changed, naming `actor` the same
/// way its other sentence does.
fn tree_kept_paragraph(actor: &str) -> String {
    format!("The tree was already there, and {actor} changed nothing in it.")
}

const FILES_PARAGRAPH: &str =
    "The hooks, the server and the skill are plain files in this project: commit them if \
     everyone who works here uses vivac, and keep them out of version control if only you \
     do. .vivac/ is never committed: it is this machine's record, and a copy of it in \
     every clone would diverge from the others. Its own .gitignore keeps it out.";

fn undo_line() -> String {
    format!(
        "{} {}",
        style::bold(Stream::Out, "Undo:"),
        style::bold(Stream::Out, "vivac setup claude-code --undo")
    )
}

/// The words come from `anchor::EVENTS_TRACKED_WARNING` (`f619`): `check`
/// reads that very same constant, so the two can no longer drift the way
/// they once did, and `check`'s copy never named a worktree at all. Bare,
/// with no newline of its own (`d792`): `init` joins it with whatever
/// other paragraphs a run has to show, one blank line between each.
pub(super) fn tracked_git_warning() -> String {
    crate::anchor::EVENTS_TRACKED_WARNING.to_string()
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
        outln!("Nothing to undo: none of what setup writes is here.");
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

    let (settings_verb, settings_what): (&'static str, &'static str) =
        match (start_ours, stop_ours, prompt_ours) {
            (true, true, true) if settings_becomes_empty => (
                "remove",
                "the three hooks setup wrote; nothing else is left, so it goes",
            ),
            (true, true, true) => ("remove", "the three hooks setup wrote"),
            (true, true, false) | (true, false, true) | (false, true, true) => {
                ("remove", "the two hooks setup wrote")
            }
            (true, false, false) => ("remove", "the SessionStart hook"),
            (false, true, false) => ("remove", "the Stop hook"),
            (false, false, true) => ("remove", "the UserPromptSubmit hook"),
            (false, false, false) => ("keep", "left as it is"),
        };
    let mut settings_item = PlanItem::new(settings_verb, SETTINGS_LABEL, settings_what);
    if let HookState::Different(_) = &start_hook_state {
        settings_item =
            settings_item.with_sub("SessionStart", "runs vivac another way; left as it is");
    }
    if let HookState::Different(_) = &stop_hook_state {
        settings_item = settings_item.with_sub("Stop", "runs vivac another way; left as it is");
    }
    if let HookState::Different(_) = &prompt_hook_state {
        settings_item =
            settings_item.with_sub("UserPromptSubmit", "runs vivac another way; left as it is");
    }

    let (mcp_verb, mcp_what) = if mcp_becomes_empty {
        (
            "remove",
            "the \"vivac\" server; nothing else is left, so it goes",
        )
    } else if mcp_ours {
        ("remove", "the \"vivac\" server")
    } else {
        ("keep", "left as it is")
    };

    let (skill_verb, skill_what) = if skill_ours {
        ("remove", "the skill setup wrote")
    } else if skill_raw.is_some() {
        ("keep", "changed since setup wrote it; left as it is")
    } else {
        ("keep", "left as it is")
    };

    let items = vec![
        settings_item,
        PlanItem::new(mcp_verb, MCP_LABEL, mcp_what),
        PlanItem::new(skill_verb, SKILL_LABEL, skill_what),
    ];
    let mut s = heading(Stream::Out, "vivac setup claude-code --undo", here);
    s.push_str(&render_items(Stream::Out, &items));

    if a.has("dry-run") {
        outln!("{}", close_with(&s, DRY_RUN_LINE));
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

    print_plan(&s);
    let proceed = a.has("yes") || super::ask("\nUndo it? [y/N] ");
    if !proceed {
        outln!("\nNothing written.");
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
    // a later run, but is tidier gone. `d784`: only `vivac-migrate` itself
    // -- the one folder this tool's own name marks as its to take back --
    // not `skills` or `.claude` above it, which may have existed before
    // setup ever ran and are never setup's to remove for being empty.
    if skill_ours {
        remove_if_empty(paths.skill.parent());
    }

    outln!("\nUndone. The tree in .vivac/ is untouched.");
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

    /// Every sub label keeps at least two spaces before its value, however
    /// wide the widest one in the same plan is. The fixed column of 15
    /// printed `UserPromptSubmitvivac session prompt --hook` in 0.15.1.
    #[test]
    fn every_sub_label_keeps_two_spaces_before_its_value() {
        for label in ["SessionStart", "UserPromptSubmit", "Stop", "in"] {
            let item = PlanItem::new("add", "path", "what").with_sub(label, "VALUE");
            let rendered = render_items(Stream::Out, &[item]);
            let sub_line = rendered.lines().nth(1).unwrap();
            let gap = sub_line.trim_start().trim_start_matches(label);
            assert!(
                gap.starts_with("  "),
                "{label} leaves {gap:?} before its value: {sub_line:?}"
            );
        }
    }

    #[test]
    fn the_fingerprint_matches_the_known_hash_of_the_literal_text() {
        // Computed independently (Python's own FNV-1a/64) over the exact
        // frontmatter and body this file embeds. It moves whenever the
        // skill's text does, and moving it is meant to be deliberate: the
        // last time was `f793`/`f794`, after an unguided migration wrote a
        // batch on a yes to its sources, never showed the nodes, and said
        // nothing about pillars.
        assert_eq!(skill_fingerprint(), 0x1d86b0aa9ecb020d);
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
