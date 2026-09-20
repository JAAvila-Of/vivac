//! Codex, the second harness `vivac setup` knows.
//!
//! `t592`, tranche 1: a clean project, none of the three files below there
//! yet. `d653` decided the shape -- the same three pieces `claude_code.rs`
//! writes, at the three places Codex reads them from inside a project:
//! the MCP server in `.codex/config.toml` (TOML, added between two marker
//! comments rather than parsed, `d654`), the two session hooks in
//! `.codex/hooks.json`, and the skill at `.agents/skills/vivac-migrate/`,
//! which is `claude_code::skill_text()` itself rather than a second copy of
//! it. Merging with a file already there, `--undo` and running this twice
//! are tranche 2 and are not this file's job yet: a target that already
//! exists stops the run instead.
//!
//! Two things Codex needs that this run cannot do for it, because both live
//! outside the project (`d655`): the project has to be marked trusted in
//! the person's own Codex configuration before it reads anything under
//! `.codex/`, and each hook is approved on its own, against its hash,
//! inside Codex. The closing summary names both.

use crate::args::Args;
use crate::failure::Failure;
use crate::output::outln;
use std::path::{Path, PathBuf};

const CONFIG_LABEL: &str = ".codex/config.toml";
const HOOKS_LABEL: &str = ".codex/hooks.json";
const SKILL_LABEL: &str = ".agents/skills/vivac-migrate/SKILL.md";

const SESSION_START_COMMAND: &str = "vivac session start --hook";
const SESSION_END_COMMAND: &str = "vivac session end --hook";

/// `d654`: written whole, between the two marker comments a later `--undo`
/// will look for -- no TOML reader in this crate, and none needed for a
/// file this tranche only ever writes onto empty ground.
const CONFIG_CONTENT: &str = "# added by vivac setup codex\n\
[mcp_servers.vivac]\n\
command = \"vivac\"\n\
args = [\"mcp\"]\n\
# end of what vivac setup codex added\n";

/// `d653`: `SessionStart` and `Stop`, the literal translation of the TOML
/// hook shape into Codex's own JSON one -- an event name, a list of groups,
/// each with its own `hooks` list. `Stop` runs during a turn, the mirror of
/// Claude Code's own `Stop` (`d656`), rather than `SessionEnd`, which runs
/// once and only long after the work that motivated it.
fn hooks_content() -> String {
    format!(
        "{{\n  \
           \"hooks\": {{\n    \
             \"SessionStart\": [\n      \
               {{\n        \
                 \"matcher\": \"startup|resume|clear|compact\",\n        \
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

pub fn run(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    if let Some(refusal) = refuse_unsupported_flags(a) {
        return Err(refusal);
    }
    if let Some(refusal) = super::refuse_home_or_global_store(roots) {
        return Err(refusal);
    }
    apply(roots, a)
}

/// The flags `claude_code.rs` already knows and this harness does not yet
/// (`t592` tranche 2): checked first, before this run reads a single file
/// or writes one. Reading them later, inside `apply`, is exactly the shape
/// that let `--undo` on a clean project write the three files instead of
/// removing anything -- a flag nobody reads is a flag nobody obeys, the same
/// lesson `f52` already drew from a positional silently dropped rather than
/// a flag. The order among the four is arbitrary; it only has to be fixed,
/// so which one a run names never depends on how the flags happened to be
/// typed.
const UNSUPPORTED_FLAGS: &[(&str, &str)] = &[
    ("undo", "removing what it wrote"),
    ("join", "joining a tree that lives elsewhere"),
    ("new-tree", "planting here despite a tracked product"),
    ("lane-name", "naming this folder's lane"),
];

fn refuse_unsupported_flags(a: &Args) -> Option<Failure> {
    for &(flag, does) in UNSUPPORTED_FLAGS {
        if a.has(flag) {
            let sentence =
                format!("setup codex does not take --{flag} yet: {does} lands in a later release.");
            let body = super::claude_code::wrapped(&sentence);
            // `Failure::Usage` directly, not the `Failure::usage` helper:
            // that helper prepends its own two spaces for a one-line
            // message, and `wrapped` already opens every line, the first
            // included, with the same two spaces the rest of this module's
            // paragraphs use.
            return Some(Failure::Usage(format!("{body}\n  Nothing written.")));
        }
    }
    None
}

/// Named after `t565` §7.8's own two-column plan, reused rather than
/// refixed a second time (`d653`): `piece_line`, `sub_line` and
/// `wrapped_piece_line` are `claude_code.rs`'s, and so is the paragraph
/// beneath it -- true of these hooks and this server too, and it names
/// neither harness.
fn render_plan(here: &Path) -> String {
    use super::claude_code::{piece_line, sub_line, wrapped_piece_line};
    let mut s = format!("  vivac setup codex, in {}\n\n", here.display());
    s.push_str(&piece_line(CONFIG_LABEL, "create: the \"vivac\" server"));
    s.push_str("        vivac mcp\n");
    s.push_str(&piece_line(HOOKS_LABEL, "create: two hooks"));
    s.push_str(&sub_line("SessionStart", SESSION_START_COMMAND));
    s.push_str(&sub_line("Stop", SESSION_END_COMMAND));
    s.push_str(&wrapped_piece_line(
        SKILL_LABEL,
        "create: how an agent brings",
        "another memory into vivac",
    ));
    s.push('\n');
    s
}

fn existing_files_refusal(existing: &[&str]) -> Failure {
    let list = existing.join("\n      ");
    Failure::Model(format!(
        "  This project already has:\n      {list}\n\n  \
         setup codex does not merge with a file that is already there yet: that\n  \
         lands in a later release. Move it aside, then run setup again.\n\n  \
         Nothing written."
    ))
}

const NO_TERMINAL_TEXT: &str = "  setup asks before writing, and there is no terminal here to ask.\n  See what it would write:  vivac setup codex --dry-run\n  Then write it:            vivac setup codex --yes";

fn apply(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let here = &roots.here;
    let target = paths(here);

    let existing: Vec<&str> = [
        (target.config.exists(), CONFIG_LABEL),
        (target.hooks.exists(), HOOKS_LABEL),
        (target.skill.exists(), SKILL_LABEL),
    ]
    .into_iter()
    .filter_map(|(exists, label)| exists.then_some(label))
    .collect();
    if !existing.is_empty() {
        return Err(existing_files_refusal(&existing));
    }

    let plan = render_plan(here);

    if a.has("dry-run") {
        outln!(
            "{plan}{}\n  Nothing written: --dry-run.",
            super::claude_code::TRAILING_PARAGRAPH
        );
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(NO_TERMINAL_TEXT.to_string()));
    }

    print!("{plan}{}", super::claude_code::TRAILING_PARAGRAPH);
    let proceed = a.has("yes") || super::ask("\n  Write it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
        return Ok(0);
    }

    let writes = vec![
        super::PlannedWrite::write(target.config.clone(), CONFIG_CONTENT.to_string(), None),
        super::PlannedWrite::write(target.hooks.clone(), hooks_content(), None),
        super::PlannedWrite::write(target.skill.clone(), super::claude_code::skill_text(), None),
    ];
    super::commit(&writes)?;

    print!("\n{}", written_text(here));
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

fn written_text(here: &Path) -> String {
    let mut s = String::from("  Written.\n");
    s.push_str(FILES_PARAGRAPH);
    s.push_str(&trusted_paragraph(here));
    s.push_str(HOOK_PARAGRAPH);
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
}
