//! Claude Code, the one harness `vivac setup` knows today.
//!
//! What is Claude Code's own: the two files it reads (`.claude/settings.json`
//! and `.mcp.json`), the skill it looks for under `.claude/skills/`, the
//! shape of a hook entry, and how a command line already there is told apart
//! from a foreign one. `t565` §9: this is the module `INTEGRATION.md` points
//! at for the level-one work of a new harness.

use super::json::{self, Value};
use crate::args::Args;
use crate::failure::Failure;
use crate::output::outln;
use std::path::{Path, PathBuf};

const SETTINGS_LABEL: &str = ".claude/settings.json";
const MCP_LABEL: &str = ".mcp.json";
const SKILL_LABEL: &str = ".claude/skills/vivac-migrate/SKILL.md";
const VIVAC_LABEL: &str = ".vivac/";

const SESSION_START_COMMAND: &str = "vivac session start --hook";
const SESSION_END_COMMAND: &str = "vivac session end --hook";

const FRONTMATTER: &str = include_str!("skill-frontmatter.md");
const BODY: &str = include_str!("skill-body.md");

pub fn run(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    if a.has("undo") {
        return undo(&roots.here, a);
    }
    apply(roots, a)
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

struct JsonFile {
    exists: bool,
    raw: String,
    indent: String,
    eol: &'static str,
    trailing_newline: bool,
    /// `Some` once parsed as an object; `None` for a missing file (nothing to
    /// parse) or a conflict (unreadable, or not an object).
    value: Option<Value>,
    /// Line and column of a parse failure, for the conflict message.
    parse_error: Option<(usize, usize)>,
    not_object: bool,
}

fn read_json(path: &Path) -> JsonFile {
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

enum HookState {
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
fn hook_state(root: &Value, event: &str, session_word: &str, ours: &str) -> HookState {
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

fn append_hook(root: &mut Value, event: &str, command: &str) {
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
    arr.push(our_hook_entry(command));
}

/// Removes every array entry whose sole hook is exactly `command`, then
/// drops the event key if its array is now empty, and `hooks` itself if
/// that leaves it with nothing. `t565` §7.7.
fn remove_hook(root: &mut Value, event: &str, command: &str) -> bool {
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

enum SkillState {
    Missing,
    Same,
    Replaceable,
    Conflict,
}

fn marker_line(fingerprint: u64) -> String {
    format!(
        "<!-- written by vivac setup; fingerprint {fingerprint:016x}; vivac setup \
         claude-code --undo removes it while the text is unchanged -->\n"
    )
}

fn skill_content_without_marker() -> String {
    format!("{FRONTMATTER}{BODY}")
}

fn skill_fingerprint() -> u64 {
    super::fnv1a64(skill_content_without_marker().as_bytes())
}

fn skill_text() -> String {
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

fn skill_state(existing: &str) -> SkillState {
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
fn skill_fingerprint_intact(existing: &str) -> bool {
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
// Formatting: the two-column plan lines `t565` §7.8 fixes the width of.
// ---------------------------------------------------------------------------

fn piece_line(label: &str, status: &str) -> String {
    format!("    {label:<41}{status}\n")
}

fn sub_line(label: &str, value: &str) -> String {
    format!("        {label:<15}{value}\n")
}

fn wrapped_piece_line(label: &str, first: &str, second: &str) -> String {
    format!("    {label:<41}{first}\n{:45}{second}\n", "")
}

// ---------------------------------------------------------------------------
// Applying: plan, ask, write.
// ---------------------------------------------------------------------------

fn apply(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    if let Some(refusal) = super::refuse_home_or_global_store(roots) {
        return Err(refusal);
    }

    let here = &roots.here;
    let tree = &roots.tree;
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

    let vivac_missing = !crate::store::already_planted(tree);
    let start_missing = matches!(start_hook_state, HookState::Missing);
    let stop_missing = matches!(stop_hook_state, HookState::Missing);
    let mcp_missing = matches!(mcp_server_state, McpState::Missing);
    let skill_missing_or_replaceable = matches!(
        skill_file_state,
        SkillState::Missing | SkillState::Replaceable
    );

    let nothing_to_write = !vivac_missing
        && !start_missing
        && !stop_missing
        && !mcp_missing
        && !skill_missing_or_replaceable;

    let piece_block = render_piece_block(
        here,
        tree,
        vivac_missing,
        settings.exists,
        mcp.exists,
        &start_hook_state,
        &stop_hook_state,
        start_missing,
        stop_missing,
        &mcp_server_state,
        &skill_file_state,
    );

    if nothing_to_write {
        outln!("{piece_block}  Nothing to write: this project is already set up.");
        return Ok(0);
    }

    if a.has("dry-run") {
        outln!("{piece_block}{TRAILING_PARAGRAPH}\n  Nothing written: --dry-run.");
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(NO_TERMINAL_TEXT.to_string()));
    }

    print!("{piece_block}{TRAILING_PARAGRAPH}");
    let proceed = a.has("yes") || super::ask("\n  Write it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    // Build every write, then commit them together (`t565` §7.3: "se
    // pregunta una sola vez por todo y se escribe todo o nada").
    let mut writes = Vec::new();
    if start_missing || stop_missing {
        let mut new_settings = settings_root.clone();
        if start_missing {
            append_hook(&mut new_settings, "SessionStart", SESSION_START_COMMAND);
        }
        if stop_missing {
            append_hook(&mut new_settings, "Stop", SESSION_END_COMMAND);
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

    // `.vivac/` is planted only once the JSON commit above has already
    // landed, and a failure here undoes that commit by hand: `.vivac/`
    // itself is never touched, planted or rolled back (`t565` §7.7).
    if vivac_missing {
        if let Err(e) = crate::store::Store::create(tree) {
            let unrestored = super::rollback(&writes);
            return Err(super::failure_with_rollback(
                format!("the tree could not be planted ({e})"),
                &unrestored,
            ));
        }
    }

    print!("\n{WRITTEN_TEXT}");
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn render_piece_block(
    here: &Path,
    tree: &Path,
    vivac_missing: bool,
    settings_exists: bool,
    mcp_exists: bool,
    start_hook_state: &HookState,
    stop_hook_state: &HookState,
    start_missing: bool,
    stop_missing: bool,
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

    let vivac_status = if vivac_missing {
        "plant the tree".to_string()
    } else if tree == here {
        "already there".to_string()
    } else {
        format!("already there, in {}", tree.display())
    };
    s.push_str(&piece_line(VIVAC_LABEL, &vivac_status));

    let settings_status = match (settings_exists, start_missing, stop_missing) {
        (_, false, false) => "already has both hooks",
        (_, true, false) => "add the SessionStart hook",
        (_, false, true) => "add the Stop hook",
        (false, true, true) => "create, with two hooks",
        (true, true, true) => "add two hooks",
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
        SkillState::Missing => s.push_str(&wrapped_piece_line(
            SKILL_LABEL,
            "create: how an agent brings",
            "another memory into vivac",
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

const TRAILING_PARAGRAPH: &str = "  The hooks run a command in every session, and the server is how the\n  agent writes to the tree. Nothing outside this directory is touched,\n  and no file is copied.\n";

const NO_TERMINAL_TEXT: &str = "  setup asks before writing, and there is no terminal here to ask.\n  See what it would write:  vivac setup claude-code --dry-run\n  Then write it:            vivac setup claude-code --yes";

const WRITTEN_TEXT: &str = "  Written.\n\n  Open a new Claude Code session here. The brief arrives on its own when\n  it starts, and Claude Code asks once whether to use the \"vivac\" server\n  from .mcp.json: saying yes is what lets the agent write to the tree.\n\n  Nothing has been brought in from anywhere. If this project already lives\n  in another memory system, or keeps what it knows in CLAUDE.md, AGENTS.md,\n  MEMORY.md or its own documents, none of that is in vivac yet. Ask the\n  agent to bring it in: the vivac-migrate skill tells it how, and how to\n  check. https://github.com/JAAvila-Of/vivac#migrating-to-vivac\n\n  These are plain files in this project: commit them if everyone who works\n  here uses vivac, and keep them out of version control if only you do.\n\n  Undo:  vivac setup claude-code --undo\n";

fn unreadable_conflict(label: &str, line: usize, column: usize) -> String {
    format!(
        "  {label} is not JSON setup can read (line {line}, column {column}), so\n  \
         it will not touch it: a file it cannot read is a file it could only\n  \
         overwrite."
    )
}

fn not_object_conflict(label: &str) -> String {
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

fn undo(root: &Path, a: &Args) -> Result<i32, Failure> {
    let paths = paths(root);
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
    let mcp_root = mcp.value.clone().unwrap();
    let mcp_server_state = mcp_state(&mcp_root);
    let skill_ours = skill_raw.as_deref().is_some_and(skill_fingerprint_intact);

    let start_ours = matches!(start_hook_state, HookState::Exact);
    let stop_ours = matches!(stop_hook_state, HookState::Exact);
    let mcp_ours = matches!(mcp_server_state, McpState::Ours);

    let nothing_to_undo = !start_ours && !stop_ours && !mcp_ours && !skill_ours;
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
    let settings_becomes_empty = preview
        .as_object()
        .is_some_and(|s: &[(String, Value)]| s.is_empty());

    let mcp_becomes_empty = mcp_ours
        && without_our_mcp_server(&mcp_root)
            .as_object()
            .is_some_and(|s: &[(String, Value)]| s.is_empty());

    let settings_status: String = match (start_ours, stop_ours) {
        (true, true) if settings_becomes_empty => {
            "remove the two hooks setup wrote;\nNOTHING_ELSE".to_string()
        }
        (true, true) => "remove the two hooks setup wrote".to_string(),
        (true, false) => "remove the SessionStart hook".to_string(),
        (false, true) => "remove the Stop hook".to_string(),
        (false, false) => "left as it is".to_string(),
    };

    let mut s = format!(
        "  vivac setup claude-code --undo, in {}\n\n",
        root.display()
    );
    if settings_status.contains("NOTHING_ELSE") {
        s.push_str(&wrapped_piece_line(
            SETTINGS_LABEL,
            "remove the two hooks setup wrote;",
            "nothing else is left, so it goes",
        ));
    } else {
        s.push_str(&piece_line(SETTINGS_LABEL, &settings_status));
    }
    if let HookState::Different(_) = &start_hook_state {
        s.push_str(&sub_line(
            "SessionStart",
            "runs vivac another way; left as it is",
        ));
    }
    if let HookState::Different(_) = &stop_hook_state {
        s.push_str(&sub_line("Stop", "runs vivac another way; left as it is"));
    }

    if mcp_becomes_empty {
        s.push_str(&wrapped_piece_line(
            MCP_LABEL,
            "remove the server \"vivac\";",
            "nothing else is left, so it goes",
        ));
    } else {
        s.push_str(&piece_line(
            MCP_LABEL,
            if mcp_ours {
                "remove the server \"vivac\""
            } else {
                "left as it is"
            },
        ));
    }

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

    s.push_str(&piece_line(VIVAC_LABEL, "kept: the tree is not setup's"));
    s.push('\n');

    if a.has("dry-run") {
        outln!("{s}  Nothing written: --dry-run.");
        return Ok(0);
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
    if start_ours || stop_ours {
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

fn remove_if_empty(dir: Option<&Path>) {
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

    #[test]
    fn the_fingerprint_matches_the_known_hash_of_the_literal_text() {
        // Computed independently (Python's own FNV-1a/64) over the exact
        // frontmatter and body this file embeds.
        assert_eq!(skill_fingerprint(), 0x9c94fb0b6caffd64);
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
