//! The three session hooks. `ROADMAP.md` §4.
//!
//! `session start` injects the brief and `session end` leaves an automatic
//! stop. They are the **seams of the session**, the way `push`/`pop` are the
//! seams of the work: they ask for no judgement of relevance, they just happen.
//!
//! `session prompt` (`d779`) is the third: a nudge, on every message, for the
//! long stretch between those two seams where the thread can still go cold.
//! It reads the log and never writes to it, and it never fails the turn --
//! see [`prompt`]'s own doc for the whole of that promise.
//!
//! `start` and `end` exit 0 and say nothing when there is no `.vivac/`. A
//! hook that fails in every directory without a tree gets switched off
//! within two days.

use crate::args::Args;
use crate::event::{Body, VivacKind};
use crate::failure::{Failure, R};
use crate::output::outln;

/// What the hook is handed on stdin.
///
/// Two fields are read and the rest is left where it is. `transcript_path`
/// travels in this same payload and it is the tempting one --it is what really
/// links the tree to the conversation-- but it carries the user's home
/// directory, and the security pillar vetoes that without negotiation. An
/// opaque identifier yes; a path into somebody's filesystem no.
struct HookInput {
    source: String,
    session: Option<String>,
}

impl HookInput {
    fn read() -> HookInput {
        use std::io::IsTerminal;
        let mut raw = String::new();
        // A terminal has no payload to give, and reading one would hang the
        // hook waiting for an EOF that never comes.
        if !std::io::stdin().is_terminal() {
            use std::io::Read;
            std::io::stdin().read_to_string(&mut raw).ok();
        }
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        HookInput {
            // `unknown` and not an empty string, so that reading it later tells
            // "it did not say" apart from "we did not look".
            source: v
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string(),
            session: v
                .get("session_id")
                .and_then(|s| s.as_str())
                .map(str::to_string),
        }
    }
}

pub fn start(ctx: &mut crate::ops::Ctx, a: &Args, project: &str) -> R {
    if !a.has("hook") {
        return crate::brief::brief(&ctx.tree, &ctx.store.root, &ctx.lane_dir, a, project);
    }
    // In hook mode the brief goes straight to stdout, in plain text: Claude
    // Code's own hook reference says plain-text stdout on `SessionStart`
    // becomes context the agent can see and act on (`f403`, `f404`), so there
    // is no envelope to build and no format only this one hook understands.
    let text = crate::brief::to_text(&ctx.tree, &ctx.store.root, &ctx.lane_dir, a, project, true)?;
    print!("{text}");
    // The brief goes out **first**, and the write cannot take it down. A
    // failure that left the agent with no brief would turn a hole in the
    // instrument into blindness in the product, which is a far worse trade: a
    // log missing an opening shows up on reading, an agent missing its brief
    // does not show up until the thread is already lost.
    let hook = HookInput::read();
    // The focus the brief paints is the top of the stack: it walks the
    // ancestors of `stack.last()` and keeps the last of the lineage, which is
    // that same node again.
    //
    // What the brief painted, taken before the lock: a writer that lands
    // while this one waits must not rewrite what the agent was shown.
    let shown_focus = ctx.tree.focus().map(|n| n.id.clone());
    // By lane, matching what `brief`/`to_text` just painted (`brief.rs`'s
    // own resume line reads `last_vivac()` too): the tree-wide
    // `vivacs.last()` used to record a stop this session never saw,
    // whenever another lane's stop happened to sit last in the log
    // (`t594`).
    let shown_vivac = ctx.tree.last_vivac().map(|v| v.id.clone());
    match ctx.lock_for_write() {
        Ok(mine) => {
            crate::ops::session_started(ctx, &hook.source, hook.session, shown_focus, shown_vivac)
                .ok();
            // The write is done; nothing after this needs the lock, and the
            // process outlives it. Only released if this call is the one
            // that took it (`f602`).
            if mine {
                ctx.unlock();
            }
        }
        // The brief is already out. A session another writer kept from
        // being recorded is a small hole in the log; say so where the
        // agent reads, and never let it cost the brief (`d598`).
        Err(Failure::Busy(_)) => {
            outln!(
                "  Session not recorded: another vivac process held the tree for {} seconds.",
                crate::store::LOCK_DEADLINE.as_secs()
            );
        }
        Err(_) => {}
    }
    Ok(())
}

pub fn end(ctx: &mut crate::ops::Ctx, a: &Args) -> R {
    // The cheap checks go first, against whatever this process already
    // loaded, so a turn with nothing to stop never asks for the lock at
    // all: a read-only tree or a filesystem with no lock support would
    // otherwise fail this hook on every ordinary turn instead of only on
    // the one that actually has something to close.
    if nothing_to_stop(&ctx.tree, a) {
        return Ok(());
    }
    // The decision whether anything changed has to be made on the tree on
    // disk. In hook mode any failure to take the lock -- busy, or the lock
    // itself unsupported -- is swallowed the same way: the change that
    // armed this stop is still there for the next turn, and nothing is
    // lost. Without a hook it is still reported, as before.
    let mine = match ctx.lock_for_write() {
        Ok(mine) => mine,
        Err(_) if a.has("hook") => return Ok(()),
        Err(e) => return Err(e),
    };
    // The lock may have reloaded the tree from disk, so the same cheap
    // checks are repeated here against what is actually there now.
    if nothing_to_stop(&ctx.tree, a) {
        return Ok(());
    }
    let next = a.opt_or("next");
    let label = segment_label(&ctx.tree);
    let num = ctx.tree.next_vivac_num.max(1);
    crate::ops::auto_vivac(ctx, VivacKind::Auto, &next, &label)?;
    // The write is done; nothing after this needs the lock. Only released
    // if this call is the one that took it (`f602`).
    if mine {
        ctx.unlock();
    }
    if !a.has("hook") {
        outln!("  v{num}  automatic stop at session close");
    }
    Ok(())
}

/// Whether `t` has nothing worth an automatic stop: an empty stack, or
/// nothing new since the last one.
fn nothing_to_stop(t: &crate::model::Tree, a: &Args) -> bool {
    // With no stack there is no thread to close, and an empty vivac is just
    // noise to be pruned later.
    if t.stack().is_empty() {
        if !a.has("hook") {
            outln!("  Empty stack: no stop worth saving.");
        }
        return true;
    }
    // Nor with nothing new. Claude Code does have a `SessionEnd` event, but
    // the automatic stop hangs off `Stop` instead: `Stop` fires on every
    // turn, so the last stop never depends on the session closing cleanly
    // (`f568`). Without this guard it would be forty identical stops a day,
    // and a stop that repeats is not a stop, it is a log.
    if t.state().seq_change <= t.state().seq_vivac {
        if !a.has("hook") {
            outln!("  Nothing changed since the last stop.");
        }
        return true;
    }
    false
}

/// What the segment being closed contained, counted off the seams.
///
/// The other four kinds of stop are written by somebody who knows what they
/// were doing, and they all carry a `next_intent`. The automatic one is
/// written by a hook that was never told: asking the agent for the intent is
/// the judgement of relevance `DX` already measured at zero uses. So it
/// carries what it can know without asking --how much the segment held-- and
/// leaves `next_intent` honestly empty (`f59`).
fn segment_label(t: &crate::model::Tree) -> String {
    let s = t.state();
    let mut parts = Vec::new();
    if s.seg_new > 0 {
        parts.push(format!("{} new", s.seg_new));
    }
    if s.seg_closed > 0 {
        parts.push(format!("{} closed", s.seg_closed));
    }
    if s.seg_notes == 1 {
        parts.push("1 note".to_string());
    } else if s.seg_notes > 1 {
        parts.push(format!("{} notes", s.seg_notes));
    }
    if parts.is_empty() && s.seg_events > 0 {
        parts.push(if s.seg_events == 1 {
            "1 change".to_string()
        } else {
            format!("{} changes", s.seg_events)
        });
    }
    parts.join(", ")
}

pub fn dispatch(ctx: &mut crate::ops::Ctx, a: &Args, project: &str) -> R {
    match a.positional(0) {
        Some("start") => start(ctx, a, project),
        Some("end") => end(ctx, a),
        // `prompt` is intercepted in `main.rs`, ahead of every tree lookup
        // this dispatch would otherwise make: `d779`'s whole point is that
        // it never fails the turn, and a `Ctx` that failed to load would
        // have already turned into a non-zero exit before reaching here.
        _ => Err(Failure::usage(
            "usage: vivac session start|end|prompt [--hook]",
        )),
    }
}

/// `d779`: the session has to have been open a while, and the thread quiet
/// for a while within it, before the nudge is worth the tokens.
const PROMPT_SESSION_MIN: i64 = 5;
const PROMPT_QUIET_MIN: i64 = 10;
const PROMPT_COOLDOWN_MIN: i64 = 10;

/// Whether `body` is one of the seams `brief.rs`'s own capture-seams block
/// names -- a fact this lane wrote about the *work*, not about the session
/// or the machinery around it.
///
/// `VivacCreated` counts only when it is `Manual`, a `save` a person sat
/// down and wrote: `Auto` is the `Stop` hook's own heartbeat, and `Push`,
/// `Pop` and `Park` ride along with an operation that already counts on its
/// own account. Counting any of those four here would let a session that
/// never wrote anything keep resetting its own clock by closing a turn.
fn is_capture(body: &Body) -> bool {
    match body {
        Body::NodeCreated { .. }
        | Body::StateChanged { .. }
        | Body::NodeNoted { .. }
        | Body::BlockChanged { .. }
        | Body::Pushed { .. }
        | Body::Popped { .. }
        | Body::Promoted { .. }
        | Body::FlagRaised { .. }
        | Body::FlagCleared { .. }
        | Body::ArmAdded { .. }
        | Body::ArmRemoved { .. }
        | Body::AgainstAdded { .. } => true,
        Body::VivacCreated { kind, .. } => *kind == VivacKind::Manual,
        Body::SessionStarted { .. }
        | Body::LaneDeclared { .. }
        | Body::LaneClaimed { .. }
        | Body::WhereChanged { .. } => false,
    }
}

/// The `ts` of the last event of this lane that `matches`, log order being
/// what `Store::read_all` already hands back: the last match in the vector
/// is the last one in time.
fn last_matching_ts<'a>(
    events: &'a [crate::event::Event],
    lane: &str,
    matches: impl Fn(&Body) -> bool,
) -> Option<&'a str> {
    events
        .iter()
        .rev()
        .find(|e| e.lane == lane && matches(&e.payload))
        .map(|e| e.ts.as_str())
}

/// Where the cooldown for `key` lives: a file under `std::env::temp_dir()`,
/// named from a hash rather than the key itself -- the key can carry a
/// session identifier, and the security pillar keeps that out of a
/// filename as much as out of the log.
fn cooldown_path(key: &str) -> std::path::PathBuf {
    let hash = crate::setup::fnv1a64(key.as_bytes());
    std::env::temp_dir()
        .join("vivac")
        .join(format!("prompt-{hash:016x}"))
}

/// Whether the last nudge this same key saw was less than
/// `PROMPT_COOLDOWN_MIN` ago. Unreadable, missing or garbled reads back as
/// "never warned" rather than failing: `d779` -- a hook with an opinion
/// about its own bookkeeping is a hook that can block on it.
fn cooled_down(path: &std::path::Path, now_secs: i64) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(last) = text.trim().parse::<i64>() else {
        return true;
    };
    now_secs - last >= PROMPT_COOLDOWN_MIN * 60
}

/// Records that the nudge just spoke, for `cooled_down` to read back next
/// time. Best effort: a failure here costs one extra nudge sooner than
/// `PROMPT_COOLDOWN_MIN` would otherwise allow, never the turn itself.
fn record_spoke(path: &std::path::Path, now_secs: i64) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(path, now_secs.to_string()).ok();
}

/// The text `prompt` prints when it decides to speak, `n` being the whole
/// minutes since the reference point -- session start, or a capture since,
/// whichever is more recent.
fn prompt_text(n: i64) -> String {
    format!(
        "vivac: nothing written to the tree in the last {n} min of this session. If a seam\n\
         passed since (a new line of work, a choice, a finding you told, a \"not now\", \
         work done, a change outside the repo), write it now, before you answer.\n"
    )
}

/// `session prompt --hook` (`d779`): a nudge for the stretch between the two
/// boundaries `start` and `end` already cover, run on every message a person
/// sends. **Always exits 0 and never writes to the log.** Anything that
/// keeps this from answering cleanly -- no tree, a log this version cannot
/// read, garbage on stdin, a `.vivac/lane` this process cannot resolve --
/// reads exactly like nothing worth saying: empty stdout, exit 0. A hook
/// that can fail the turn it rides on is worse than one that occasionally
/// stays quiet when it had something to say.
///
/// The two seams already write a trace of their own kind -- `session
/// started`, an automatic stop -- so this is the one hook of the three that
/// is pure: it reads what the other two, and every ordinary write, already
/// left behind, and decides without touching any of it.
pub fn prompt(cwd: &std::path::Path, a: &Args) {
    let Some(text) = prompt_text_for(cwd, a) else {
        return;
    };
    print!("{text}");
}

/// [`prompt`]'s own decision, factored out so every early exit is a plain
/// `?` rather than a chain of nested matches: any `None` here is "nothing
/// to say", never a reason to report failure upward.
fn prompt_text_for(cwd: &std::path::Path, a: &Args) -> Option<String> {
    let located = crate::store::locate(cwd).ok()??;
    let store = crate::store::Store::open(located.root.clone()).ok()?;
    let (events, _broken) = store.read_all().ok()?;
    let lane = located
        .lane
        .as_ref()
        .map(|l| l.id.clone())
        .unwrap_or_else(|| crate::lane::MAIN.to_string());

    let session_start =
        last_matching_ts(&events, &lane, |b| matches!(b, Body::SessionStarted { .. }))?;
    let last_capture = last_matching_ts(&events, &lane, is_capture);

    let now = a
        .opt("now")
        .map(str::to_string)
        .unwrap_or_else(crate::clock::now_rfc3339);
    let now_secs = crate::clock::epoch_seconds(&now)?;
    let session_start_secs = crate::clock::epoch_seconds(session_start)?;

    let reference_secs = match last_capture.and_then(crate::clock::epoch_seconds) {
        Some(c_secs) if c_secs > session_start_secs => c_secs,
        _ => session_start_secs,
    };

    let session_elapsed_min = (now_secs - session_start_secs) / 60;
    let reference_elapsed_min = (now_secs - reference_secs) / 60;
    if session_elapsed_min < PROMPT_SESSION_MIN || reference_elapsed_min < PROMPT_QUIET_MIN {
        return None;
    }

    // The cooldown key never carries the project's own path: `first_event_id`
    // is the log's own opaque first identifier, the same one the registry
    // keys projects by.
    let project_id = crate::store::first_event_id(&located.root).unwrap_or_default();
    let session_id = HookInput::read().session;
    let key = match &session_id {
        Some(s) => format!("{project_id}\u{0}{lane}\u{0}{s}"),
        None => format!("{project_id}\u{0}{lane}"),
    };
    let path = cooldown_path(&key);
    if !cooled_down(&path, now_secs) {
        return None;
    }
    record_spoke(&path, now_secs);

    Some(prompt_text(reference_elapsed_min))
}
