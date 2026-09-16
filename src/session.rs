//! The two session hooks. `ROADMAP.md` §4.
//!
//! `session start` injects the brief and `session end` leaves an automatic
//! stop. They are the **seams of the session**, the way `push`/`pop` are the
//! seams of the work: they ask for no judgement of relevance, they just happen.
//!
//! Both exit 0 and say nothing when there is no `.vivac/`. A hook that fails
//! in every directory without a tree gets switched off within two days.

use crate::args::Args;
use crate::event::VivacKind;
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
        return crate::brief::brief(&ctx.tree, ctx.anchor.as_ref(), a, project);
    }
    // In hook mode the brief goes straight to stdout, in plain text: Claude
    // Code's own hook reference says plain-text stdout on `SessionStart`
    // becomes context the agent can see and act on (`f403`, `f404`), so there
    // is no envelope to build and no format only this one hook understands.
    let text = crate::brief::to_text(&ctx.tree, ctx.anchor.as_ref(), a, project)?;
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
    let shown_vivac = ctx.tree.vivacs.last().map(|v| v.id.clone());
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
        _ => Err(Failure::usage("usage: vivac session start|end [--hook]")),
    }
}
