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
const GITIGNORE_LABEL: &str = ".vivac/.gitignore";
const LANE_LABEL: &str = ".vivac/lane";

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
// The lane: `t594` §4.5, joining the tree above rather than planting a
// second one.
// ---------------------------------------------------------------------------

/// What this run has to do about the lane `roots.here` is, worked out
/// before anything is written so the plan can say it.
struct LanePlan {
    lane_id: String,
    name: String,
    repos: Vec<crate::event::Repo>,
    /// This folder does not carry `.vivac/lane` yet, so this run has to
    /// write it before it can declare (`t594` §4.5.2, case (c)). The id
    /// this points back at is minted here, since it never depends on the
    /// tree's own state; the project it points back at does, and is
    /// worked out at write time instead (`write_lane`).
    is_new: bool,
    /// Whether the config still needs `lock_lanes_in_config`: absent for
    /// a tree that does not exist yet, which always needs it once
    /// planted, and read off the existing one otherwise.
    needs_lock: bool,
    /// The tree already says exactly this (`t594` §4.5.2, case (e)):
    /// nothing to write, and running `setup` twice in a row does not
    /// leave two events behind.
    unchanged: bool,
    /// How many repositories the redaction guard kept out, and the first
    /// rule that caught one. `d600`: they are still missing from the
    /// declaration, and that is said rather than left silent, without
    /// repeating which repository it was.
    excluded: Option<(usize, &'static str)>,
}

/// `folder_name`, or what it becomes once the redaction guard rejects it
/// (`d600`, `lane::name_for`): the folder's own name never reaches the
/// log either way.
fn declared_name(id: &str, folder_name: &str) -> String {
    match crate::redact::check_field("lane name", folder_name) {
        Some(_) => crate::lane::name_for(id, folder_name),
        None => folder_name.to_string(),
    }
}

/// `scanned`, filtered through the redaction guard (`d600`): what is left
/// to declare, and the count and first rule of whatever it kept out.
/// Shared by declaring a lane's own folder and by declaring `main` on the
/// tree's own folder, whether that happens because someone asked for it
/// or because `ensure_first_event` needs to seed it -- one piece of work,
/// one place that does it.
fn filtered_repos(
    scanned: Vec<crate::event::Repo>,
) -> (Vec<crate::event::Repo>, Option<(usize, &'static str)>) {
    let mut excluded_count = 0usize;
    let mut excluded_rule: Option<&'static str> = None;
    let repos = scanned
        .into_iter()
        .filter(
            |r| match crate::redact::check_field("repository path", &r.path) {
                Some(f) => {
                    excluded_count += 1;
                    excluded_rule.get_or_insert(f.rule);
                    false
                }
                None => true,
            },
        )
        .collect();
    (
        repos,
        (excluded_count > 0).then(|| (excluded_count, excluded_rule.unwrap())),
    )
}

/// What the tree already says about `lane_id`, read without writing
/// anything: `Store::open` would fill a missing `config` in on its own,
/// and that write is one `--dry-run` must never trigger just by asking
/// what a tree is on (`t594` fix-1, finding 6). `config_version` reads
/// `ConfigVersion::One` for a tree with no config at all -- the same
/// answer `Store::open` would settle on for a tree with no lane and no
/// pillar or rule either, so `needs_lock` comes out right either way
/// without this having to know why the file is missing.
struct ExistingLane {
    config_version: crate::store::ConfigVersion,
    declared: Option<(String, Vec<crate::event::Repo>)>,
}

fn existing_lane(tree: &Path, lane_id: &str) -> Option<ExistingLane> {
    let (events, broken) =
        crate::store::read_all_from(&tree.join(crate::store::DIR).join(crate::store::LOG)).ok()?;
    let folded = crate::model::fold(&events, broken);
    Some(ExistingLane {
        config_version: crate::store::peek_config_version(tree)
            .unwrap_or(crate::store::ConfigVersion::One),
        declared: folded
            .lanes
            .get(lane_id)
            .map(|s| (s.name.clone(), s.repos.clone())),
    })
}

/// `t594` §4.5.2's five cases, decided from `roots` alone: whether there is
/// a tree above `here` at all, and whether `here` already carries its own
/// `.vivac/lane` (`Located::lane_dir == here`, rather than some ancestor's).
fn plan_lane(roots: &super::Roots) -> LanePlan {
    let (repos, excluded) = filtered_repos(crate::repos::scan(&roots.here));

    let folder_name = roots
        .here
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let here_has_its_own_vivac = roots
        .located
        .as_ref()
        .is_some_and(|l| l.lane_dir == roots.here);

    let (lane_id, name, is_new) = match &roots.located {
        None => main_lane(),
        Some(l) if here_has_its_own_vivac && l.lane.is_none() => main_lane(),
        Some(l) if here_has_its_own_vivac => {
            let id = l.lane.as_ref().unwrap().id.clone();
            let name = declared_name(&id, &folder_name);
            (id, name, false)
        }
        Some(_) => {
            let id = crate::lane::new_id();
            let name = declared_name(&id, &folder_name);
            (id, name, true)
        }
    };

    let existing = roots
        .located
        .as_ref()
        .and_then(|_| existing_lane(&roots.tree, &lane_id));
    let needs_lock = existing
        .as_ref()
        .map(|e| e.config_version != crate::store::ConfigVersion::Lanes)
        .unwrap_or(true);
    let unchanged = existing
        .and_then(|e| e.declared)
        .is_some_and(|(n, r)| n == name && r == repos);

    LanePlan {
        lane_id,
        name,
        repos,
        is_new,
        needs_lock,
        unchanged,
        excluded,
    }
}

fn main_lane() -> (String, String, bool) {
    (
        crate::lane::MAIN.to_string(),
        crate::lane::MAIN.to_string(),
        false,
    )
}

/// The tree's own first event id, seeding one when there is none: a brand
/// new lane's own `.vivac/lane` needs a stable id to point back at
/// (`resolve_lane`, `store.rs` -- it reads a tree's first line as the
/// cheap fingerprint that ties a lane to the right tree), and there is
/// nothing stable to point at in a tree that has never written anything,
/// which a tree fresh out of `init` or a bare plant still is.
///
/// The seed is the tree's own implicit `main` declaring itself for real,
/// with its own folder's actual repositories -- the same walk declaring
/// `main` by hand would do, and not a placeholder: task 8 decides with
/// this list whether a linked worktree is one of the lane's own
/// repositories or a lane apart, and an empty list would hand it the
/// wrong answer (`t594` fix-1, finding 2). Taken and released under its
/// own lock, before the new lane's own lock is taken, since a second
/// attempt to lock the same file from this same process would otherwise
/// wait on itself.
///
/// If this write succeeds and the log's first line still will not parse
/// as an id right after, that is not this call's own failure to undo --
/// it already appended a real event and already locked the config, and
/// the log only ever grows. The error says so, since the caller cannot.
fn ensure_first_event(tree: &Path) -> Result<String, Failure> {
    if let Some(id) = crate::store::first_event_id(tree) {
        return Ok(id);
    }
    let (repos, _excluded) = filtered_repos(crate::repos::scan(tree));
    let store = crate::store::Store::open(tree.to_path_buf())?;
    let mut ctx = crate::ops::Ctx::load_for_write(store, Some(crate::lane::MAIN.to_string()))?;
    ctx.lock_for_write()?;
    crate::ops::declare_lane(&mut ctx, crate::lane::MAIN.to_string(), repos)?;
    crate::store::first_event_id(tree).ok_or_else(|| {
        Failure::Io(std::io::Error::other(
            "this folder's main lane was just declared to give the tree a first \
             event, and locked its config to match, and the tree's own first \
             line is still unreadable after that -- the log only ever grows, \
             so what was just written stays either way",
        ))
    })
}

/// What this run actually does, in order: this folder's own `.vivac/lane`
/// on disk first -- only for a brand new lane, and with no lock held over
/// it at all -- and only then `declare_lane`, which takes the write lock,
/// locks the config and emits `lane.declared` together.
///
/// That is *not* `t594` §4.5.2's own order, which puts the file inside the
/// lock and after the config is closed. This one is at least as safe: if
/// the process dies between the file and the lock, the folder already
/// knows whose thread it is and the tree finds out the moment the fold
/// sees the matching event, which is exactly what dying between the file
/// and the event -- the ordering the spec itself calls safe -- already
/// leaves behind. If it dies between the file and the *config* closing
/// specifically, the tree does not have a lane event yet either, so an
/// older vivac reading it in between is not being lied to. What the file
/// must never do is land *after* the event: that is the one ordering that
/// leaves a folder signing as `main` while the tree already says
/// otherwise, and nothing here permits it.
fn write_lane(roots: &super::Roots, plan: &LanePlan) -> Result<(), Failure> {
    if plan.is_new {
        let project = ensure_first_event(&roots.tree)?;
        let lane = crate::lane::Lane {
            version: 1,
            id: plan.lane_id.clone(),
            project,
        };
        crate::lane::write(&roots.here.join(crate::store::DIR), &lane)?;
    }

    let store = crate::store::Store::open(roots.tree.clone())?;
    let mut ctx = crate::ops::Ctx::load_for_write(store, Some(plan.lane_id.clone()))?;
    ctx.lock_for_write()?;
    crate::ops::declare_lane(&mut ctx, plan.name.clone(), plan.repos.clone())
}

/// Locks the tree's config to `t594`'s own sentence without touching the
/// log: for a lane whose declaration already matches (`unchanged`), there
/// is nothing new to say, but the config can still have lost the lock
/// underneath it -- by hand, or by an older `Store::open` regenerating one
/// that went missing before it knew a lane event counts too (`t594` fix-1,
/// finding 5). `unchanged` must never decide this on its own: a folder
/// that has nothing new to declare can still be the reason the config
/// needs relocking.
fn relock_lanes(tree: &Path) -> Result<(), Failure> {
    let mut store = crate::store::Store::open(tree.to_path_buf())?;
    let lock = store.lock_for_write()?;
    store.lock_lanes_in_config(&lock)?;
    Ok(())
}

/// The clause text for a `Failure`, without doubling an `Io` variant's own
/// "Input/output error:" prefix once `failure_with_rollback` wraps it a
/// second time (`t594` fix-1, finding 3): `Failure::message` already adds
/// that prefix for `Io`, and the planting failure this mirrors uses a raw
/// `std::io::Error` -- which has no such prefix to begin with -- for the
/// exact same reason.
fn detail_of(e: &Failure) -> String {
    match e {
        Failure::Io(io) => io.to_string(),
        other => other.message(),
    }
}

/// The exit-5 text for a lane declaration or a config relock that failed,
/// after `unrestored` -- what `super::rollback` could not put back among
/// the settings/mcp/skill/gitignore pieces -- is already known.
///
/// Unlike `failure_with_rollback`, this never says every file came back:
/// by the time either call above can fail, a real event may already sit
/// in the tree's own log (`ensure_first_event`'s seed) or the config may
/// already be locked, and neither of those is a file `rollback` ever
/// touches or could undo. `t565` §7.7 accepts the same gap for planting,
/// on the same reasoning -- but planting never writes anything of
/// informational value before it can fail, and a lane's own event does,
/// so this says the log stays instead of claiming a rollback it did not
/// do and cannot do.
fn lane_failure_with_rollback(clause: String, unrestored: &[PathBuf]) -> Failure {
    let mut message = clause;
    if unrestored.is_empty() {
        message.push_str(
            ", so setup put the settings, the server entry and the skill back\n  \
             as they were. Whatever this already wrote to the tree's own log stays\n  \
             either way: the log only ever grows.",
        );
    } else {
        message.push_str(", and setup could not put these back as they were:\n");
        for p in unrestored {
            message.push_str(&format!("      {}\n", p.display()));
        }
        message.push_str(
            "  setup keeps no copy on disk, so the only other copy is whatever\n  \
             version control holds. Whatever this already wrote to the tree's own\n  \
             log stays either way: the log only ever grows.",
        );
    }
    Failure::Io(std::io::Error::other(message))
}

/// Notes `tree` in this machine's registry, the same bookkeeping every
/// ordinary command already does on its way out (`main.rs`). `setup`
/// itself never used to reach that block -- it returns before it
/// (`f277`) -- and that was harmless while every folder it touched was
/// found by walking up from itself. It stopped being harmless the moment
/// `setup` could join a folder whose only path back to its tree is the
/// registry: a linked worktree that sits beside the tree's own folder
/// rather than above it, which `resolve_lane` (`store.rs`) can only ever
/// find through here (`t594` fix-1, finding 1). Quiet when there is
/// nowhere to note or nothing to note it with yet, the same as the
/// ordinary path.
fn note_registry(tree: &Path) {
    let Some(store_dir) = crate::store::store_dir() else {
        return;
    };
    if let Some(project_id) = crate::store::first_event_id(tree) {
        crate::registry::note(&store_dir, &project_id, tree);
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
    // A tree this run plants already carries its `.gitignore`, straight out
    // of `Store::create`: only a tree from before `t594` §4.9 can lack it.
    let gitignore_missing = !vivac_missing
        && !tree
            .join(crate::store::DIR)
            .join(crate::store::GITIGNORE)
            .is_file();
    let start_missing = matches!(start_hook_state, HookState::Missing);
    let stop_missing = matches!(stop_hook_state, HookState::Missing);
    let mcp_missing = matches!(mcp_server_state, McpState::Missing);
    let skill_missing_or_replaceable = matches!(
        skill_file_state,
        SkillState::Missing | SkillState::Replaceable
    );

    let lane = plan_lane(roots);

    let nothing_to_write = !vivac_missing
        && !gitignore_missing
        && !start_missing
        && !stop_missing
        && !mcp_missing
        && !skill_missing_or_replaceable
        && lane.unchanged
        && !lane.needs_lock;

    let piece_block = render_piece_block(
        here,
        tree,
        vivac_missing,
        gitignore_missing,
        settings.exists,
        mcp.exists,
        &start_hook_state,
        &stop_hook_state,
        start_missing,
        stop_missing,
        &mcp_server_state,
        &skill_file_state,
        &lane,
    );

    // Asked once per run, and before either early exit below, so a log
    // already tracked is flagged whether this run has anything else to
    // write or not: someone already set up is exactly who never reaches
    // the branch that used to be the only one carrying this warning.
    let log_tracked = crate::anchor::in_working_tree(tree)
        && crate::anchor::tracks(tree, ".vivac/events") == Some(true);

    // Checked before `nothing_to_write`, not after: that branch notes the
    // registry (`note_registry`), and `--dry-run` promises to write
    // nothing anywhere, the machine's registry included (`t594` fix-2,
    // finding 2). An already-set-up project asking for `--dry-run` used
    // to reach the other branch first and note it anyway.
    if a.has("dry-run") {
        outln!("{piece_block}{TRAILING_PARAGRAPH}\n  Nothing written: --dry-run.");
        if log_tracked {
            print!("{TRACKED_WARNING}");
        }
        return Ok(0);
    }

    if nothing_to_write {
        // A real run, never `--dry-run`, thanks to the check above: noting
        // the registry is bookkeeping every ordinary command already does
        // on a pure read, not a write this promise is about.
        note_registry(tree);
        outln!("{piece_block}  Nothing to write: this project is already set up.");
        if log_tracked {
            print!("{TRACKED_WARNING}");
        }
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

    if gitignore_missing {
        writes.push(super::PlannedWrite::write(
            tree.join(crate::store::DIR).join(crate::store::GITIGNORE),
            "*\n".to_string(),
            None,
        ));
    }

    super::commit(&writes)?;

    // Planting is the one step this run takes after the commit above, which
    // may already have written `.vivac/.gitignore` (`gitignore_missing`) --
    // so `.vivac/` is not untouched by the time this runs. What stays true
    // is narrower: planting itself never rolls back. A failure here undoes
    // the JSON commit by hand, but whatever `Store::create` managed to
    // write in `.vivac/` before failing is left exactly as it is (`t565`
    // §7.7).
    if vivac_missing {
        if let Err(e) = crate::store::Store::create(tree) {
            let unrestored = super::rollback(&writes);
            return Err(super::failure_with_rollback(
                format!("the tree could not be planted ({e})"),
                &unrestored,
            ));
        }
    }

    // Declaring the lane, or just relocking the config, goes right after
    // planting, next to it: never rolled back on its own, only the JSON
    // commit undone by hand if it fails -- `write_lane`'s own doc explains
    // why that is still safe.
    if !lane.unchanged {
        if let Err(e) = write_lane(roots, &lane) {
            let unrestored = super::rollback(&writes);
            return Err(lane_failure_with_rollback(
                format!("the lane could not be declared ({})", detail_of(&e)),
                &unrestored,
            ));
        }
    } else if lane.needs_lock {
        // Nothing new to declare, but the config still needs the lock
        // `unchanged` must never decide on its own (`t594` fix-1, finding
        // 5): here the only write is the lock itself, so a failure has
        // nothing irreversible to own up to and the ordinary wording is
        // accurate as it stands.
        if let Err(e) = relock_lanes(tree) {
            let unrestored = super::rollback(&writes);
            return Err(super::failure_with_rollback(
                format!(
                    "the tree's config could not be relocked ({})",
                    detail_of(&e)
                ),
                &unrestored,
            ));
        }
    }

    // What *this run* actually did to the tree, for `written_text`
    // (`t594` fix-3): every one of these is independent, and `needs_lock`
    // decides `config_locked` regardless of which branch above closed
    // it -- both `write_lane`'s own `declare_lane` and `relock_lanes`
    // close the same lock, and only ever do it for real when it was
    // still open beforehand.
    let written = Written {
        connection: start_missing || stop_missing || mcp_missing,
        skill: skill_missing_or_replaceable,
        planted: vivac_missing,
        gitignore_created: gitignore_missing,
        lane_declared: !lane.unchanged,
        config_locked: lane.needs_lock,
        undoable: start_missing
            && stop_missing
            && mcp_missing
            && matches!(skill_file_state, SkillState::Missing),
    };
    note_registry(tree);
    print!("\n{}", written_text(&written));
    if log_tracked {
        print!("{TRACKED_WARNING}");
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn render_piece_block(
    here: &Path,
    tree: &Path,
    vivac_missing: bool,
    gitignore_missing: bool,
    settings_exists: bool,
    mcp_exists: bool,
    start_hook_state: &HookState,
    stop_hook_state: &HookState,
    start_missing: bool,
    stop_missing: bool,
    mcp_server_state: &McpState,
    skill_file_state: &SkillState,
    lane: &LanePlan,
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
    if gitignore_missing {
        // Two different files, in two different folders, can both need
        // this line in the same run -- the tree's own, from before `t594`
        // §4.9, and a brand new lane's own (below). Only then does the
        // tree's own copy say whose it is; on its own it reads exactly as
        // it always has (`t594` fix-1, finding 7).
        let status = if lane.is_new {
            "create: keeps the tree's .vivac/ out of version control"
        } else {
            "create: keeps .vivac/ out of version control"
        };
        s.push_str(&piece_line(GITIGNORE_LABEL, status));
    }

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

    if !lane.unchanged {
        if lane.is_new {
            s.push_str(&piece_line(
                LANE_LABEL,
                &format!(
                    "create: this folder becomes lane \"{}\" of the tree above",
                    lane.name
                ),
            ));
            s.push_str(&piece_line(
                GITIGNORE_LABEL,
                "create: keeps .vivac/ out of version control",
            ));
        } else {
            // One sentence for both: declaring `main` on the tree's own
            // folder and redeclaring a lane that already existed are the
            // same write, and neither creates a file the way a brand new
            // lane does above -- it is the log that changes.
            s.push_str(&piece_line(
                ".vivac/events",
                &format!(
                    "record: this folder is lane \"{}\", with its repositories",
                    lane.name
                ),
            ));
        }
    }
    // What the redaction guard kept out is the folder's own state, not a
    // change: it is still true on a run that declares nothing new, so it
    // is said every time rather than only on the run that first found it
    // (`t594` fix-1, finding 8).
    if let Some((count, rule)) = lane.excluded {
        let noun = if count == 1 {
            "repository"
        } else {
            "repositories"
        };
        s.push_str(&sub_line(
            "kept out",
            &format!("{count} {noun}, refused: {rule}"),
        ));
    }
    // Independent of `unchanged`: the config can need the lock even when
    // nothing about the declaration itself changed (`t594` fix-1, finding
    // 5).
    if lane.needs_lock {
        s.push_str(&piece_line(
            "config",
            "lock: from now on this tree needs vivac 0.12 or newer",
        ));
    }

    s.push('\n');
    s
}

const TRAILING_PARAGRAPH: &str = "  The hooks run a command in every session, and the server is how the\n  agent writes to the tree. Nothing outside this directory is touched,\n  and no file is copied.\n";

const NO_TERMINAL_TEXT: &str = "  setup asks before writing, and there is no terminal here to ask.\n  See what it would write:  vivac setup claude-code --dry-run\n  Then write it:            vivac setup claude-code --yes";

/// What this run wrote, which decides how it ends (`t579` §15.5): a
/// paragraph is only printed when it is true of this run, and it says
/// what that run did, no more and no less (`t594` fix-3) -- every one of
/// these is a separate thing `apply` can write to the tree or the
/// folder, and any subset of them can be true together.
struct Written {
    /// A hook or the server, which only a new session picks up.
    connection: bool,
    /// The skill, where it was missing or an earlier release's copy.
    skill: bool,
    /// The tree, planted by this run rather than found.
    planted: bool,
    /// The tree's own `.vivac/.gitignore`, on a tree from before `t594`
    /// §4.9 that never got one (`gitignore_missing`). Independent of
    /// everything else here: a tree can be missing this and have its
    /// lanes fully settled, or the other way round.
    gitignore_created: bool,
    /// This run declared this folder's lane, or redeclared an existing
    /// one: a real thread recorded in the tree's own log.
    lane_declared: bool,
    /// This run closed the lanes lock, whether that happened on its own
    /// (nothing else changed) or alongside declaring the lane above
    /// (`t594` fix-2, finding 3 first tried to treat these as mutually
    /// exclusive, which they are not: a brand new lane commonly closes
    /// the lock in the very same write that declares it).
    config_locked: bool,
    /// All four of setup's pieces, the skill among them missing before:
    /// `--undo` removes all four, so only then does it take back exactly
    /// this run.
    undoable: bool,
}

fn written_text(w: &Written) -> String {
    let mut s = String::from("  Written.\n");
    if w.connection {
        s.push_str(SESSION_PARAGRAPH);
    } else if w.skill {
        s.push_str(SKILL_PARAGRAPH);
    }
    if w.planted {
        s.push_str(MIGRATE_PARAGRAPHS);
    } else {
        s.push_str(&tree_paragraph(
            w.gitignore_created,
            w.lane_declared,
            w.config_locked,
        ));
    }
    s.push_str(FILES_PARAGRAPH);
    if w.undoable {
        s.push_str(UNDO_LINE);
    }
    s
}

/// The paragraph about the tree itself, once planting it is ruled out
/// (`MIGRATE_PARAGRAPHS` covers that): `TREE_KEPT_PARAGRAPH` when none of
/// the three actually happened, and one sentence naming exactly the ones
/// that did otherwise -- never more than what this run wrote, and never
/// silent about any of it.
///
/// The three are independent, and saying so in the type is the fix: they
/// were mutually exclusive branches before, so a run that did two of them
/// could only name one, and a run that only wrote the tree's `.gitignore`
/// had no branch at all and claimed to have changed nothing -- two lines
/// under its own plan announcing that write (`t594`).
fn tree_paragraph(gitignore_created: bool, lane_declared: bool, config_locked: bool) -> String {
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
        return TREE_KEPT_PARAGRAPH.to_string();
    }
    // Noun phrases rather than verb phrases: they share one subject, so
    // two of them join without the reader having to carry a verb across
    // the list, and none of them can be read as belonging to this run
    // rather than to the tree.
    format!(
        "\n{}",
        wrapped(&format!(
            "The tree was already there, and setup wrote in it: {}.",
            join_with_and(&clauses)
        ))
    )
}

/// `text`, wrapped to the same width every other paragraph in this file
/// already wraps to by hand, each line indented by two spaces. A plain
/// greedy word wrap is all this needs: nothing it ever wraps runs past a
/// short sentence naming one to three clauses.
fn wrapped(text: &str) -> String {
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

/// `items`, in English list form: one on its own, two joined by "and",
/// three or more comma-separated with "and" before the last.
fn join_with_and(items: &[&str]) -> String {
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

const SKILL_PARAGRAPH: &str = "\n  The vivac-migrate skill is now the one this version of vivac ships.\n  Sessions opened from now on use it.\n";

const MIGRATE_PARAGRAPHS: &str = "\n  Nothing has been brought in from anywhere yet. To bring in what this\n  project already knows, from another memory system, the harness's own\n  memory, instruction files or its documents, ask the agent:\n\n      Use the vivac-migrate skill to bring everything this project knows\n      into vivac.\n\n  It shows you a plan before writing anything, checks what it wrote, and\n  offers to retire the other maps one at a time, only if you say yes.\n\n  Until then, another memory system you use keeps talking to the agent as\n  before, and may tell it to use that system first. That is expected: the\n  skill only reads from it.\n";

const TREE_KEPT_PARAGRAPH: &str =
    "\n  The tree was already there, and setup changed nothing in it.\n";

const FILES_PARAGRAPH: &str = "\n  The hooks, the server and the skill are plain files in this project:\n  commit them if everyone who works here uses vivac, and keep them out of\n  version control if only you do. .vivac/ is never committed: it is this\n  machine's record, and a copy of it in every clone would diverge from the\n  others. Its own .gitignore keeps it out.\n";

const UNDO_LINE: &str = "\n  Undo:  vivac setup claude-code --undo\n";

const TRACKED_WARNING: &str = "\n  .vivac/events is tracked by git here. Every clone and worktree gets its\n  own copy of the log, and the copies diverge. Remove it from the index\n  (git rm -r --cached .vivac) and let .vivac/.gitignore keep it out.\n";

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
        assert_eq!(skill_fingerprint(), 0x53833e2dadbcf537);
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
