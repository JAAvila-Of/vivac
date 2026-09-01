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

/// Wraps text in the envelope Claude Code injects into the context. It is a
/// single JSON line, with no external dependency and no `jq` in between: a
/// hook with a pipe is a hook that breaks on the first different machine.
fn envelope(evento: &str, text: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": evento,
            "additionalContext": text,
        }
    })
    .to_string()
}

pub fn start(ctx: &crate::ops::Ctx, a: &Args, project: &str) -> R {
    if !a.has("hook") {
        return crate::brief::brief(&ctx.tree, ctx.anchor.as_ref(), a, project);
    }
    // In hook mode the brief is captured and emitted inside the envelope. No
    // loose noise on stdout: what is not in the envelope, the agent never sees.
    let text = crate::brief::to_text(&ctx.tree, ctx.anchor.as_ref(), a, project)?;
    println!("{}", envelope("SessionStart", &text));
    Ok(())
}

pub fn end(ctx: &mut crate::ops::Ctx, a: &Args) -> R {
    // With no stack there is no thread to close, and an empty vivac is just
    // noise to be pruned later.
    if ctx.tree.stack.is_empty() {
        if !a.has("hook") {
            println!("  Empty stack: no stop worth saving.");
        }
        return Ok(());
    }
    // Nor with nothing new. Claude Code's `Stop` hook runs **on every turn**,
    // not when the session closes: there is no end-of-session event (`f35`).
    // Without this guard it would be forty identical stops a day, and a stop
    // that repeats is not a stop, it is a log.
    if ctx.tree.seq_change <= ctx.tree.seq_vivac {
        if !a.has("hook") {
            println!("  Nothing changed since the last stop.");
        }
        return Ok(());
    }
    let next = a.opt_or("next");
    let label = segment_label(&ctx.tree);
    let num = ctx.tree.next_vivac_num.max(1);
    crate::ops::auto_vivac(ctx, VivacKind::Auto, &next, &label)?;
    if !a.has("hook") {
        println!("  v{num}  automatic stop at session close");
    }
    Ok(())
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
    let mut parts = Vec::new();
    if t.seg_new > 0 {
        parts.push(format!("{} new", t.seg_new));
    }
    if t.seg_closed > 0 {
        parts.push(format!("{} closed", t.seg_closed));
    }
    if t.seg_notes == 1 {
        parts.push("1 note".to_string());
    } else if t.seg_notes > 1 {
        parts.push(format!("{} notes", t.seg_notes));
    }
    if parts.is_empty() && t.seg_events > 0 {
        parts.push(if t.seg_events == 1 {
            "1 change".to_string()
        } else {
            format!("{} changes", t.seg_events)
        });
    }
    parts.join(", ")
}

pub fn dispatch(ctx: &mut crate::ops::Ctx, a: &Args, project: &str) -> R {
    match a.positional(0) {
        Some("start") | Some("inicio") => start(ctx, a, project),
        Some("end") | Some("fin") => end(ctx, a),
        _ => Err(Failure::usage("usage: vivac session start|end [--hook]")),
    }
}

/// `vivac hooks` — prints what to paste, and touches nobody's configuration.
/// Writing to the user's settings is an action you ask for, not one that
/// happens by surprise.
pub fn hooks() -> R {
    println!(
        r#"
  Paste this into the project's .claude/settings.json:

  {{
    "hooks": {{
      "SessionStart": [
        {{ "hooks": [{{ "type": "command", "command": "vivac session start --hook" }}] }}
      ],
      "Stop": [
        {{ "hooks": [{{ "type": "command", "command": "vivac session end --hook" }}] }}
      ]
    }}
  }}

  SessionStart injects the brief into the agent's context.
  Stop leaves an automatic stop with the stack as it stood.

  Both stay quiet and exit 0 where there is no .vivac/, so they can be left
  in the global configuration without getting in the way of other projects.
"#
    );
    Ok(())
}
