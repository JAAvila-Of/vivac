//! Codex, the second harness `vivac setup` knows.
//!
//! `t592`, tranche 1: a clean project, none of the three files below there
//! yet. `d653` decided the shape -- the same three pieces `claude_code.rs`
//! writes, at the three places Codex reads them from inside a project:
//! the MCP server in `.codex/config.toml` (TOML, added between two marker
//! comments rather than parsed, `d654`), the two session hooks in
//! `.codex/hooks.json`, and the skill at `.agents/skills/vivac-migrate/`,
//! which is `claude_code::skill_text()` itself rather than a second copy of
//! it.
//!
//! Tranche 2's piece A (`t592`, `d710`) briefly gave this file a fourth
//! piece, the tree itself, through the module both harnesses shared
//! (`src/setup/tree.rs`, `r515`). `d723` piece B took it away again: plant,
//! join and `--name` are `init`'s alone now, so this file goes back to
//! exactly the three pieces Codex itself reads, once `super::resolve_for_setup`
//! has already said this folder is either the tree's own or one of its
//! declared lanes.
//!
//! Tranche 2's piece B (`t592` §4) is still here: each of the three files
//! merges with whatever is already there, the same three outcomes
//! `claude_code.rs` already gives its own files -- create it, add to what
//! is there, or leave it alone because it already has what this run would
//! write.
//!
//! Tranche 2's piece C (`t592` §5) is `--undo`: it takes off exactly what
//! this harness wrote and leaves everything else, the same promise
//! `claude_code::undo` already keeps for its own three files (`r515`, not
//! reinvented here). `d723` piece B: neither `--undo` here nor
//! `claude_code`'s reaches into `.vivac/lane` any more -- the lane is not
//! `setup`'s to touch, undoing included.
//!
//! Two things Codex needs that this run cannot do for it, because both live
//! outside the project (`d655`): the project has to be marked trusted in
//! the person's own Codex configuration before it reads anything under
//! `.codex/`, and each hook is approved on its own, against its hash,
//! inside Codex. The closing summary names both.

use super::claude_code::{
    append_hook, hook_state, not_object_conflict, read_json, remove_hook, skill_fingerprint_intact,
    skill_state, skill_text, unreadable_conflict, HookState, SkillState,
};
use super::json::{self, Value};
use super::tree;
use crate::args::Args;
use crate::failure::Failure;
use crate::output::outln;
use std::path::{Path, PathBuf};

const CONFIG_LABEL: &str = ".codex/config.toml";
const HOOKS_LABEL: &str = ".codex/hooks.json";
const SKILL_LABEL: &str = ".agents/skills/vivac-migrate/SKILL.md";

const SESSION_START_COMMAND: &str = "vivac session start --hook";
const SESSION_END_COMMAND: &str = "vivac session end --hook";
const SESSION_START_MATCHER: &str = "startup|resume|clear|compact";

/// `d654`: written whole, between the two marker comments a later `--undo`
/// will look for -- no TOML reader in this crate, and none needed for a
/// file this only ever writes out this way onto empty ground.
///
/// Only for that ground: once `.codex/hooks.json` exists, its state is read
/// and grown through the JSON machinery `claude_code.rs` already has
/// (`read_json`, `hook_state`, `append_hook`), not through this string.
/// `serde_json`'s own pretty-printer puts every object on its own line, so
/// its output stops matching this string byte for byte the moment there is
/// anything to merge with -- one puts `{ "type": ..., "command": ... }` on
/// one line and the other never does (`t592` tranche 2, piece B).
fn hooks_content() -> String {
    format!(
        "{{\n  \
           \"hooks\": {{\n    \
             \"SessionStart\": [\n      \
               {{\n        \
                 \"matcher\": \"{SESSION_START_MATCHER}\",\n        \
                 \"hooks\": [\n          \
                   {{ \"type\": \"command\", \"command\": \"{SESSION_START_COMMAND}\" }}\n        \
                 ]\n      \
               }}\n    \
             ],\n    \
             \"Stop\": [\n      \
               {{\n        \
                 \"hooks\": [\n          \
                   {{ \"type\": \"command\", \"command\": \"{SESSION_END_COMMAND}\" }}\n        \
                 ]\n      \
               }}\n    \
             ]\n  \
           }}\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// `.codex/config.toml`: no TOML reader in this crate (`d654`), so its state
// is read by scanning lines for the two marker comments setup's own block
// sits between, never by parsing.
// ---------------------------------------------------------------------------

const CONFIG_OPEN_MARKER: &str = "# added by vivac setup codex";
const CONFIG_CLOSE_MARKER: &str = "# end of what vivac setup codex added";

/// `d654`: written whole between the two markers above -- the block itself,
/// unchanged from tranche 1.
const CONFIG_CONTENT: &str = "# added by vivac setup codex\n\
[mcp_servers.vivac]\n\
command = \"vivac\"\n\
args = [\"mcp\"]\n\
# end of what vivac setup codex added\n";

#[derive(Debug)]
enum ConfigState {
    Create,
    Append,
    Already,
}

/// `existing`'s own state, read by scanning its lines rather than parsing
/// TOML (`d654`): both markers present is `Already`, neither is `Append`,
/// and one without the other is a conflict this returns as `Err` rather
/// than a state, because a half-written block cannot be repaired by
/// guessing where it ended. `Err` covers a second case too: neither marker
/// present, but a foreign `[mcp_servers.vivac]` table already there would
/// collide with the one this run would add.
///
/// A line search is not a TOML parse: it can false-positive on a line
/// inside a multi-line string that happens to read exactly like one of
/// these markers or that table header. A false positive here is a plain
/// refusal instead of a broken file underneath -- the side to be wrong on.
fn config_state(existing: &str) -> Result<ConfigState, String> {
    let lines: Vec<&str> = existing.lines().collect();
    let has_open = lines.contains(&CONFIG_OPEN_MARKER);
    let has_close = lines.contains(&CONFIG_CLOSE_MARKER);
    if has_open && has_close {
        return Ok(ConfigState::Already);
    }
    if has_open != has_close {
        return Err(config_marker_conflict(has_open));
    }
    if lines.iter().any(|&l| l.trim() == "[mcp_servers.vivac]") {
        return Err(config_table_conflict());
    }
    Ok(ConfigState::Append)
}

fn config_marker_conflict(has_open: bool) -> String {
    let (missing_word, missing_marker) = if has_open {
        ("closing", CONFIG_CLOSE_MARKER)
    } else {
        ("opening", CONFIG_OPEN_MARKER)
    };
    format!(
        "  {CONFIG_LABEL} has one of setup's own two markers and not the other.\n  \
         The {missing_word} one is missing:\n      {missing_marker}\n  \
         A half-written block is not repaired by guessing where it ended. Fix it\n  \
         by hand, or take out the marker that is there, then run setup again."
    )
}

fn config_table_conflict() -> String {
    format!(
        "  {CONFIG_LABEL} already has a [mcp_servers.vivac] table that setup did not\n  \
         write, and setup never rewrites an entry it did not write. Adding ours\n  \
         below it would declare that table twice, which is a file Codex cannot\n  \
         read at all. Rename or remove it, then run setup again."
    )
}

/// `existing`, with setup's own block appended at the end, preceded by a
/// blank line: `existing` itself is never touched, down to the byte, other
/// than gaining a trailing newline first when it did not already end in
/// one.
fn append_config(existing: &str) -> String {
    let mut s = existing.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push('\n');
    s.push_str(CONFIG_CONTENT);
    s
}

// ---------------------------------------------------------------------------
// `.codex/config.toml`, the `--undo` side (`t592` tranche 2, piece C): the
// same two markers `config_state` scans for, read for the opposite
// question -- not whether the block can be added, but whether it can be
// taken off. A half-written block is the one state `--undo` treats
// differently from `apply`: `config_state` refuses outright on it, but
// `--undo` never blocks on a file it is not sure it can touch -- it leaves
// that one file alone and says why, the same as a hook it does not
// recognise or a skill changed since setup wrote it.
// ---------------------------------------------------------------------------

enum ConfigUndoState {
    /// Both markers present: setup's own block is there to take off.
    Ours,
    /// Neither marker: nothing here is setup's, whether the file is
    /// missing, empty, or has content of its own that never went through
    /// `append_config`.
    NotOurs,
    /// One marker without the other -- `true` when the opening one is the
    /// one present, so the closing one is what is missing.
    HalfMarker(bool),
}

fn config_undo_state(existing: &str) -> ConfigUndoState {
    let lines: Vec<&str> = existing.lines().collect();
    let has_open = lines.contains(&CONFIG_OPEN_MARKER);
    let has_close = lines.contains(&CONFIG_CLOSE_MARKER);
    if has_open && has_close {
        ConfigUndoState::Ours
    } else if has_open != has_close {
        ConfigUndoState::HalfMarker(has_open)
    } else {
        ConfigUndoState::NotOurs
    }
}

/// `existing`, with setup's own block taken off -- the exact inverse of
/// `append_config`, byte for byte. Only ever called once `config_undo_state`
/// has already confirmed both markers are present.
///
/// The blank line right before the opening marker goes too, but only when
/// `append_config` is the one thing that could have put it there: a blank
/// line sitting right before the marker, with a line of its own before
/// that. A block written onto empty ground (`ConfigState::Create`) never
/// gained that blank line to begin with, so the marker starting at the very
/// top of the file leaves nothing extra to take off.
fn remove_config_block(existing: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let open = lines
        .iter()
        .position(|&l| l == CONFIG_OPEN_MARKER)
        .expect("config_undo_state::Ours already confirmed this marker is here");
    let close = lines
        .iter()
        .position(|&l| l == CONFIG_CLOSE_MARKER)
        .expect("config_undo_state::Ours already confirmed this marker is here");
    let start = if open > 1 && lines[open - 1].is_empty() {
        open - 1
    } else {
        open
    };
    let mut kept: Vec<&str> = Vec::new();
    kept.extend_from_slice(&lines[..start]);
    kept.extend_from_slice(&lines[close + 1..]);
    let mut s = kept.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

struct Paths {
    config: PathBuf,
    hooks: PathBuf,
    skill: PathBuf,
}

fn paths(root: &Path) -> Paths {
    Paths {
        config: root.join(".codex").join("config.toml"),
        hooks: root.join(".codex").join("hooks.json"),
        skill: root
            .join(".agents")
            .join("skills")
            .join("vivac-migrate")
            .join("SKILL.md"),
    }
}

pub fn run(cwd: &Path, a: &Args) -> Result<i32, Failure> {
    if a.has("undo") {
        return undo(cwd, a);
    }
    let roots = super::resolve_for_setup(cwd)?;
    if let Some(refusal) = super::refuse_home_or_global_store(&roots) {
        return Err(refusal);
    }
    apply(&roots, a)
}

/// Named after `t565` §7.8's own two-column plan, reused rather than
/// refixed a second time (`d653`): `piece_line` and `sub_line` are
/// `claude_code.rs`'s, and so is the paragraph beneath it -- true of
/// these hooks and this server too, and it names neither harness.
///
/// `d723` piece B took the tree's own fourth piece back out of this plan:
/// `resolve_for_setup` has already confirmed this folder is either the
/// tree's own or one of its declared lanes by the time this renders, so
/// there is nothing about the tree left for it to say.
///
/// `config_state`, the two hook states and `skill_file_state` decide which
/// of the three outcomes each piece shows (`t592` tranche 2, piece B): the
/// same "already has it" versus "add to it" versus "create it" `claude_code`
/// already draws for its own files.
#[allow(clippy::too_many_arguments)]
fn render_plan(
    here: &Path,
    config_state: &ConfigState,
    hooks_exists: bool,
    start_hook_state: &HookState,
    stop_hook_state: &HookState,
    start_missing: bool,
    stop_missing: bool,
    skill_file_state: &SkillState,
) -> String {
    use super::claude_code::{piece_line, sub_line};
    let mut s = format!("  vivac setup codex, in {}\n\n", here.display());

    let config_status = match config_state {
        ConfigState::Create => "create: the \"vivac\" server",
        ConfigState::Append => "add: the \"vivac\" server",
        ConfigState::Already => "already has the \"vivac\" server",
    };
    s.push_str(&piece_line(CONFIG_LABEL, config_status));
    if !matches!(config_state, ConfigState::Already) {
        s.push_str("        vivac mcp\n");
    }

    let hooks_status = match (hooks_exists, start_missing, stop_missing) {
        (_, false, false) => "already has both hooks",
        (_, true, false) => "add the SessionStart hook",
        (_, false, true) => "add the Stop hook",
        (false, true, true) => "create: two hooks",
        (true, true, true) => "add two hooks",
    };
    s.push_str(&piece_line(HOOKS_LABEL, hooks_status));
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

    s
}

/// `.agents/skills/vivac-migrate/SKILL.md` is already there, and either
/// setup did not write it or it was changed since -- the same rejection
/// `claude_code.rs` gives its own skill, naming this harness's own path
/// (`t592` tranche 2, piece B).
fn skill_conflict() -> String {
    format!(
        "  {SKILL_LABEL} is already there, and either setup did not write it or\n  \
         it was changed since. setup never overwrites it: move it away, then run\n  \
         setup again."
    )
}

/// `d723` piece B: no plan of the tree side joins this one any more --
/// `run` has already confirmed, through `super::resolve_for_setup`, that
/// this folder is either the tree's own or one of its declared lanes, so
/// there is nothing left here to plant, declare or lock. `t592` tranche 2,
/// piece B: each of the three files can now already be there, so this
/// reads its state first, the same shape `claude_code::apply` already
/// reads `settings.json`, `.mcp.json` and the skill in, rather than the
/// outright refusal tranche 1 gave any of the three already existing.
fn apply(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let here = &roots.here;
    let target = paths(here);
    let config_raw = std::fs::read_to_string(&target.config).ok();
    let hooks = read_json(&target.hooks);
    let skill_raw = std::fs::read_to_string(&target.skill).ok();

    let mut conflicts: Vec<String> = Vec::new();

    let config_state_result = match &config_raw {
        None => Ok(ConfigState::Create),
        Some(text) => config_state(text),
    };
    if let Err(msg) = &config_state_result {
        conflicts.push(msg.clone());
    }

    if let Some((line, col)) = hooks.parse_error {
        conflicts.push(unreadable_conflict(HOOKS_LABEL, line, col));
    } else if hooks.not_object {
        conflicts.push(not_object_conflict(HOOKS_LABEL));
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

    // Every conflict above is checked, so every `Err` branch already went
    // into `conflicts` and returned: what is left here is always `Ok`.
    let config_state = config_state_result.expect("checked above");

    let hooks_root = hooks.value.clone().unwrap_or_else(|| Value::object(vec![]));
    let start_hook_state = hook_state(&hooks_root, "SessionStart", "start", SESSION_START_COMMAND);
    let stop_hook_state = hook_state(&hooks_root, "Stop", "end", SESSION_END_COMMAND);
    let start_missing = matches!(start_hook_state, HookState::Missing);
    let stop_missing = matches!(stop_hook_state, HookState::Missing);
    let skill_missing_or_replaceable = matches!(
        skill_file_state,
        SkillState::Missing | SkillState::Replaceable
    );

    let config_needs_write = !matches!(config_state, ConfigState::Already);
    let hooks_needs_write = start_missing || stop_missing;

    let nothing_to_write =
        !config_needs_write && !hooks_needs_write && !skill_missing_or_replaceable;

    let full_plan = format!(
        "{}\n",
        render_plan(
            here,
            &config_state,
            hooks.exists,
            &start_hook_state,
            &stop_hook_state,
            start_missing,
            stop_missing,
            &skill_file_state,
        )
    );

    if a.has("dry-run") {
        outln!(
            "{full_plan}{}\n  Nothing written: --dry-run.",
            super::claude_code::TRAILING_PARAGRAPH
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
        outln!("{full_plan}  Nothing to write: this project is already set up.");
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::no_terminal_text(
            super::Harness::Codex,
            a,
        )));
    }

    print!("{full_plan}{}", super::claude_code::TRAILING_PARAGRAPH);
    let proceed = a.has("yes") || super::ask("\n  Write it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    let mut writes = Vec::new();

    match config_state {
        ConfigState::Create => {
            writes.push(super::PlannedWrite::write(
                target.config.clone(),
                CONFIG_CONTENT.to_string(),
                None,
            ));
        }
        ConfigState::Append => {
            let existing = config_raw.expect("Append only reached with a file already there");
            let rendered = append_config(&existing);
            writes.push(super::PlannedWrite::write(
                target.config.clone(),
                rendered,
                Some(existing.into_bytes()),
            ));
        }
        ConfigState::Already => {}
    }

    if hooks_needs_write {
        if hooks.exists {
            let mut new_hooks = hooks_root.clone();
            if start_missing {
                append_hook(
                    &mut new_hooks,
                    "SessionStart",
                    SESSION_START_COMMAND,
                    Some(SESSION_START_MATCHER),
                );
            }
            if stop_missing {
                append_hook(&mut new_hooks, "Stop", SESSION_END_COMMAND, None);
            }
            let rendered = json::finalize(
                &json::render(&new_hooks, &hooks.indent),
                hooks.eol,
                hooks.trailing_newline,
            );
            let before = hooks_root.clone();
            writes.push(super::PlannedWrite {
                path: target.hooks.clone(),
                action: super::Action::Write(rendered),
                original: Some(hooks.raw.clone().into_bytes()),
                preserved: Some(Box::new(move |updated| json::extends(&before, updated))),
            });
        } else {
            writes.push(super::PlannedWrite::write(
                target.hooks.clone(),
                hooks_content(),
                None,
            ));
        }
    }

    if skill_missing_or_replaceable {
        writes.push(super::PlannedWrite::write(
            target.skill.clone(),
            skill_text(),
            skill_raw.clone().map(String::into_bytes),
        ));
    }

    super::commit(&writes)?;

    // Bookkeeping, not a write this promise is about (`note_registry`'s
    // own doc): `d723` piece B, the tree itself is untouched.
    tree::note_registry(roots);

    print!("\n{}", written_text(here));
    Ok(0)
}

// ---------------------------------------------------------------------------
// `--undo` (`t592` tranche 2, piece C): takes off exactly what this harness
// wrote, and leaves everything else. `claude_code::undo` already resolves
// this whole shape for its own three files; this mirrors it rather than
// reinventing it (`r515`) -- same order of pieces in the plan, same
// vocabulary, same all-or-nothing through the same `super::commit`.
// ---------------------------------------------------------------------------

/// `d723` piece B: `--undo` takes off only the three pieces this harness
/// itself wrote. Neither the lane nor the tree is `setup`'s to touch, so
/// this reads and writes `here` alone -- no `Roots`, no
/// `resolve_for_setup`, and none of the tree's own refusals: undoing
/// whatever an earlier setup wrote is always safe, regardless of what the
/// tree above `here` is doing.
fn undo(here: &Path, a: &Args) -> Result<i32, Failure> {
    use super::claude_code::{piece_line, sub_line};

    let target = paths(here);
    let config_raw = std::fs::read_to_string(&target.config).ok();
    let hooks = read_json(&target.hooks);
    let skill_raw = std::fs::read_to_string(&target.skill).ok();

    let mut conflicts: Vec<String> = Vec::new();
    if let Some((line, col)) = hooks.parse_error {
        conflicts.push(unreadable_conflict(HOOKS_LABEL, line, col));
    } else if hooks.not_object {
        conflicts.push(not_object_conflict(HOOKS_LABEL));
    }
    if !conflicts.is_empty() {
        let mut msg = conflicts.join("\n\n");
        msg.push_str("\n\n  Nothing written.");
        return Err(Failure::Model(msg));
    }

    // `.codex/config.toml` has no such abort of its own: a half-written
    // block is never unreadable the way broken JSON is, so `--undo` leaves
    // it alone and says why instead of blocking on it (`t592` tranche 2 §5).
    let config_state = config_raw.as_deref().map(config_undo_state);
    let config_ours = matches!(config_state, Some(ConfigUndoState::Ours));

    let hooks_root = hooks.value.clone().unwrap_or_else(|| Value::object(vec![]));
    let start_hook_state = hook_state(&hooks_root, "SessionStart", "start", SESSION_START_COMMAND);
    let stop_hook_state = hook_state(&hooks_root, "Stop", "end", SESSION_END_COMMAND);
    let start_ours = matches!(start_hook_state, HookState::Exact);
    let stop_ours = matches!(stop_hook_state, HookState::Exact);
    let skill_ours = skill_raw.as_deref().is_some_and(skill_fingerprint_intact);

    let nothing_to_undo = !config_ours && !start_ours && !stop_ours && !skill_ours;
    if nothing_to_undo {
        outln!("  Nothing to undo: none of what setup writes is here.");
        return Ok(0);
    }

    let mut preview = hooks_root.clone();
    if start_ours {
        remove_hook(&mut preview, "SessionStart", SESSION_START_COMMAND);
    }
    if stop_ours {
        remove_hook(&mut preview, "Stop", SESSION_END_COMMAND);
    }
    let hooks_becomes_empty = preview
        .as_object()
        .is_some_and(|s: &[(String, Value)]| s.is_empty());

    let hooks_status: String = match (start_ours, stop_ours) {
        (true, true) if hooks_becomes_empty => {
            "remove the two hooks setup wrote; nothing else is left, so it goes".to_string()
        }
        (true, true) => "remove the two hooks setup wrote".to_string(),
        (true, false) => "remove the SessionStart hook".to_string(),
        (false, true) => "remove the Stop hook".to_string(),
        (false, false) => "left as it is".to_string(),
    };

    let mut s = format!("  vivac setup codex --undo, in {}\n\n", here.display());

    // `t592` tranche 2 §5: a half-written block is not this run's to guess
    // the end of, so the file is left alone and the line says why. A status
    // line and not the paragraph `apply` refuses with: that paragraph ends
    // by saying to run setup again, which is not what the person in front
    // of it asked for, and a plan reads as a grid.
    match &config_state {
        Some(ConfigUndoState::HalfMarker(has_open)) => {
            s.push_str(&piece_line(
                CONFIG_LABEL,
                "left as it is: its marker block is half written",
            ));
            s.push_str(&sub_line(
                "missing",
                if *has_open {
                    CONFIG_CLOSE_MARKER
                } else {
                    CONFIG_OPEN_MARKER
                },
            ));
        }
        Some(ConfigUndoState::Ours) => {
            let existing = config_raw
                .as_deref()
                .expect("ConfigUndoState::Ours only reached with a file present");
            if remove_config_block(existing).trim().is_empty() {
                s.push_str(&piece_line(
                    CONFIG_LABEL,
                    "remove the \"vivac\" server; nothing else is left, so it goes",
                ));
            } else {
                s.push_str(&piece_line(CONFIG_LABEL, "remove the \"vivac\" server"));
            }
        }
        Some(ConfigUndoState::NotOurs) | None => {
            s.push_str(&piece_line(CONFIG_LABEL, "left as it is"));
        }
    }

    s.push_str(&piece_line(HOOKS_LABEL, &hooks_status));
    if let HookState::Different(_) = &start_hook_state {
        s.push_str(&sub_line(
            "SessionStart",
            "runs vivac another way; left as it is",
        ));
    }
    if let HookState::Different(_) = &stop_hook_state {
        s.push_str(&sub_line("Stop", "runs vivac another way; left as it is"));
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

    s.push('\n');

    if a.has("dry-run") {
        outln!("{s}  Nothing written: --dry-run.");
        return Ok(0);
    }

    // `f718`, the same guard the writing path has: a run with nobody to
    // answer used to print the question anyway, remove nothing and exit 0.
    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::no_terminal_text(
            super::Harness::Codex,
            a,
        )));
    }

    print!("{s}");
    let proceed = a.has("yes") || super::ask("  Undo it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    let mut writes = Vec::new();

    if config_ours {
        let existing = config_raw.expect("config_ours only true with a file present");
        let removed = remove_config_block(&existing);
        if removed.trim().is_empty() {
            writes.push(super::PlannedWrite::delete(
                target.config.clone(),
                existing.into_bytes(),
            ));
        } else {
            writes.push(super::PlannedWrite::write(
                target.config.clone(),
                removed,
                Some(existing.into_bytes()),
            ));
        }
    }

    if start_ours || stop_ours {
        let original = hooks.raw.clone().into_bytes();
        if hooks_becomes_empty {
            writes.push(super::PlannedWrite::delete(target.hooks.clone(), original));
        } else {
            let mut new_hooks = hooks_root.clone();
            if start_ours {
                remove_hook(&mut new_hooks, "SessionStart", SESSION_START_COMMAND);
            }
            if stop_ours {
                remove_hook(&mut new_hooks, "Stop", SESSION_END_COMMAND);
            }
            let rendered = json::finalize(
                &json::render(&new_hooks, &hooks.indent),
                hooks.eol,
                hooks.trailing_newline,
            );
            let before = hooks_root.clone();
            writes.push(super::PlannedWrite {
                path: target.hooks.clone(),
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
            target.skill.clone(),
            skill_raw.clone().unwrap().into_bytes(),
        ));
    }

    super::commit(&writes)?;

    // Best-effort, and only once the commit above is known to have
    // succeeded: an empty directory left behind costs nothing to leave for
    // a later run, but is tidier gone.
    if skill_ours {
        super::claude_code::remove_if_empty(target.skill.parent());
        super::claude_code::remove_if_empty(target.skill.parent().and_then(Path::parent));
        super::claude_code::remove_if_empty(
            target
                .skill
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent),
        );
    }
    // `.codex/` itself: the one folder `claude_code.rs` has no equivalent
    // of, since `.codex/config.toml` and `.codex/hooks.json` are its only
    // two files (`t592` tranche 2 §5). Attempted unconditionally, same as
    // every `remove_if_empty` above: it costs nothing when the folder is
    // not actually empty, or is already gone.
    super::claude_code::remove_if_empty(target.config.parent());

    outln!("  Undone. The tree in .vivac/ is untouched.");
    Ok(0)
}

const FILES_PARAGRAPH: &str = "\n  The server entry and the hooks file are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do.\n";

/// `d655`: the two doors this run cannot open itself, because both live in
/// configuration this run never touches -- the person's own, for trust, and
/// Codex's own approval prompt, for the hook.
const HOOK_PARAGRAPH: &str = "\n  Each hook not already approved is approved on its own, against its hash,\n  and asked again if it changes. Inside Codex, the first time and whenever\n  a hook changes:\n\n      /hooks\n";

/// `path`, quoted the way a TOML table key can actually hold it.
///
/// A double-quoted TOML string is *basic*: a backslash inside it escapes,
/// so printing a Windows path there verbatim -- `"C:\Users\...` -- hands
/// back something Codex's own TOML reader cannot parse. A *literal* string,
/// single-quoted, takes every byte between the quotes as it is and never
/// escapes at all, which is exactly what a path wants; TOML's own rule is
/// that a literal string cannot itself carry a single quote, so a path that
/// happens to have one falls back to a basic string, escaping the one
/// character that would otherwise be read as its own closing quote and the
/// backslash that a basic string would otherwise also read as an escape.
fn quoted_path(path: &str) -> String {
    if !path.contains('\'') {
        return format!("'{path}'");
    }
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn trusted_paragraph(here: &Path) -> String {
    format!(
        "\n  Codex will not read anything under .codex/ in this project until you\n  \
         mark it trusted, which lives in your own configuration, not this\n  \
         project's. Add to ~/.codex/config.toml:\n\n      \
         [projects.{}]\n      \
         trust_level = \"trusted\"\n",
        quoted_path(&here.display().to_string())
    )
}

/// `f706`: the last word, because it decides who can do any of the three
/// things above. Under Codex's own sandbox `.codex` and `.agents` are
/// read-only once they exist as directories, which they do from the line
/// before this one, so running this again is a person's job from here on
/// and an agent that tries it only learns that it cannot.
const SANDBOX_PARAGRAPH: &str = "\n  Running setup here again is yours to do from a terminal. Now that .codex/\n  and .agents/ exist, Codex keeps both read-only inside its own sandbox, so\n  an agent working in this project cannot write to either.\n";

fn written_text(here: &Path) -> String {
    let mut s = String::from("  Written.\n");
    s.push_str(FILES_PARAGRAPH);
    s.push_str(&trusted_paragraph(here));
    s.push_str(HOOK_PARAGRAPH);
    s.push_str(SANDBOX_PARAGRAPH);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows path is the ordinary case: no single quote in it, so the
    /// literal-string form applies and every backslash survives exactly
    /// once. Doubling it would be the basic-string escape, and this is not
    /// a basic string.
    #[test]
    fn a_path_with_no_single_quote_is_wrapped_in_single_quotes_untouched() {
        let path = r"C:\Users\someone\project";
        assert_eq!(quoted_path(path), r"'C:\Users\someone\project'");
    }

    /// The one case a literal string cannot hold: TOML forbids a single
    /// quote inside one, so this falls back to a basic string, and a basic
    /// string escapes both the quote and every backslash beside it.
    #[test]
    fn a_path_with_a_single_quote_falls_back_to_an_escaped_basic_string() {
        let path = r"C:\Users\o'someone\project";
        assert_eq!(quoted_path(path), r#""C:\\Users\\o'someone\\project""#);
    }

    /// The rule that actually keeps the line valid TOML, checked against
    /// both shapes at once rather than trusted from the two cases above:
    /// whatever `quoted_path` returns, a basic (double-quoted) result never
    /// leaves a lone backslash behind once every doubled pair is accounted
    /// for.
    #[test]
    fn the_result_never_leaves_a_bare_backslash_inside_double_quotes() {
        for path in [
            r"C:\Users\someone\project",
            r"C:\Users\o'someone\project",
            "/home/someone/project",
            "/home/o'someone/project",
        ] {
            let quoted = quoted_path(path);
            let Some(inner) = quoted.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
                continue;
            };
            let stripped_of_escaped_pairs = inner.replace("\\\\", "");
            assert!(
                !stripped_of_escaped_pairs.contains('\\'),
                "{quoted:?} leaves a backslash TOML would read as a bad escape"
            );
        }
    }

    // -----------------------------------------------------------------------
    // `config_state`: the four states and the fifth rejection, decided by
    // scanning lines rather than parsing TOML (`t592` tranche 2, piece B).
    // -----------------------------------------------------------------------

    #[test]
    fn config_state_reads_both_markers_as_already() {
        let existing = format!("{CONFIG_OPEN_MARKER}\nsomething\n{CONFIG_CLOSE_MARKER}\n");
        assert!(matches!(config_state(&existing), Ok(ConfigState::Already)));
    }

    #[test]
    fn config_state_reads_neither_marker_as_append() {
        assert!(matches!(
            config_state("[other]\nkey = 1\n"),
            Ok(ConfigState::Append)
        ));
    }

    #[test]
    fn config_state_rejects_an_opening_marker_with_no_closing_one() {
        let existing = format!("{CONFIG_OPEN_MARKER}\nsomething\n");
        let err = config_state(&existing).unwrap_err();
        assert!(err.contains(CONFIG_LABEL), "{err}");
        assert!(err.contains("closing"), "{err}");
    }

    #[test]
    fn config_state_rejects_a_closing_marker_with_no_opening_one() {
        let existing = format!("something\n{CONFIG_CLOSE_MARKER}\n");
        let err = config_state(&existing).unwrap_err();
        assert!(err.contains(CONFIG_LABEL), "{err}");
        assert!(err.contains("opening"), "{err}");
    }

    #[test]
    fn config_state_rejects_a_foreign_mcp_servers_vivac_table() {
        let existing = "[mcp_servers.vivac]\ncommand = \"something-else\"\n";
        let err = config_state(existing).unwrap_err();
        assert!(err.contains(CONFIG_LABEL), "{err}");
        assert!(err.contains("[mcp_servers.vivac]"), "{err}");
    }

    #[test]
    fn append_config_adds_a_blank_line_then_the_block() {
        let existing = "# hand-written\n";
        let after = append_config(existing);
        assert_eq!(after, format!("# hand-written\n\n{CONFIG_CONTENT}"));
    }

    #[test]
    fn append_config_adds_the_missing_newline_before_the_blank_line() {
        let existing = "# hand-written, no trailing newline";
        let after = append_config(existing);
        assert_eq!(
            after,
            format!("# hand-written, no trailing newline\n\n{CONFIG_CONTENT}")
        );
    }
}
