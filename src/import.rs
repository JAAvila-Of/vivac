//! `import` — brings in the `tree.json` from the Python spike.
//!
//! Three trees were seeded with the spike and filled in by hand against real
//! projects. Redoing them would throw away the only raw material this
//! project has, so the migration is part of the port, not an extra.
//!
//! Two things are preserved on purpose: **the node number** --the design
//! documents cite `#8` and `#11`, and if the number changed those references
//! would stop resolving-- and **the original date**, carried into the
//! event's `ts` rather than flattened onto today. A bare `YYYY-MM-DD` lands
//! at noon UTC; a full RFC 3339 stamp keeps its own instant, only reshaped
//! to the log's own `YYYY-MM-DDTHH:MM:SSZ`. Anything else refuses the whole
//! import, before a single event is written (`d866`) -- the date is never
//! silently replaced by whatever "now" happened to be when `import` ran.

use crate::args::Args;
use crate::event::{Body, Event, Kind, State};
use crate::failure::{Failure, R};
use crate::ops::Ctx;
use crate::output::outln;
use crate::{id, redact};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Old {
    nodes: BTreeMap<String, OldNode>,
}

#[derive(Deserialize)]
struct OldNode {
    id: u64,
    title: String,
    kind: String,
    status: String,
    parent: Option<u64>,
    #[serde(default)]
    why: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    refs: Vec<String>,
    #[serde(default)]
    blocks: bool,
    #[serde(default)]
    opened: String,
    #[serde(default)]
    closed: Option<String>,
}

fn kind_of(kind: &str) -> Kind {
    match kind {
        "goal" => Kind::Goal,
        "decision" => Kind::Decision,
        "finding" => Kind::Finding,
        // `run` and `issue` were work subtypes in the spike. The model does
        // not distinguish them: `MODEL.md` §4.2 leaves `task` as the only work
        // entity, and `finding` fits as a field, not as a state.
        _ => Kind::Task,
    }
}

fn state_of(status: &str) -> State {
    match status {
        "done" => State::Done,
        "parked" => State::Suspended,
        "superseded" => State::Superseded,
        _ => State::Active,
    }
}

/// Whether every byte in each of `ranges` is an ASCII digit. Integer
/// parsing alone is not that check: it takes a leading `+`, so `+026` would
/// read as the year 26.
fn digits(b: &[u8], ranges: &[std::ops::Range<usize>]) -> bool {
    ranges.iter().all(|r| {
        b.get(r.clone())
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
    })
}

/// `s`, a ten-byte `YYYY-MM-DD`, as `(year, month, day)` if it is a real
/// Gregorian date. `crate::clock::is_valid_civil_date` does the calendar
/// check, leap years included; this only checks that the punctuation and
/// the digits are there to check at all.
fn parse_calendar(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !digits(b, &[0..4, 5..7, 8..10]) {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    crate::clock::is_valid_civil_date(y, m, d).then_some((y, m, d))
}

/// The date or date-time `date` names, as the log's own `ts` shape --
/// `YYYY-MM-DDTHH:MM:SSZ`, UTC, to the second -- or `None` if `date` is
/// neither of the two shapes this reads.
///
/// A bare `YYYY-MM-DD` (a real calendar date, checked above) lands at noon
/// UTC, same as always. A full RFC 3339 stamp -- a `T` separator, optional
/// fractional seconds (truncated, never rounded), and a `Z` or
/// `+HH:MM`/`-HH:MM` offset -- keeps its own instant, converted to UTC,
/// which can move the calendar date across a day boundary (`d866`).
///
/// Anything else is `None`. `import`'s pre-pass turns every `None` into a
/// refusal before this is ever asked for a `ts` it did not already check,
/// so every call here after the pre-pass is expected to succeed.
fn instant(date: &str) -> Option<String> {
    if date.len() == 10 {
        parse_calendar(date)?;
        return Some(format!("{date}T12:00:00Z"));
    }
    // Every slice below is by byte, which only lands on character
    // boundaries if every character is one byte: a stamp with anything
    // wider in it is not one this reads, and has to be refused before a
    // slice can land inside that character and panic.
    let b = date.as_bytes();
    if b.len() < 20 || !date.is_ascii() {
        return None;
    }
    let (y, m, d) = parse_calendar(&date[0..10])?;
    if b[10] != b'T' || b[13] != b':' || b[16] != b':' || !digits(b, &[11..13, 14..16, 17..19]) {
        return None;
    }
    let hh: i64 = date[11..13].parse().ok()?;
    let mm: i64 = date[14..16].parse().ok()?;
    let ss: i64 = date[17..19].parse().ok()?;
    if hh >= 24 || mm >= 60 || ss >= 60 {
        return None;
    }

    // Optional fractional seconds: consumed, never read -- truncating is
    // not rounding, and the event log has never carried sub-second
    // precision.
    let mut idx = 19;
    if b.get(idx).copied() == Some(b'.') {
        idx += 1;
        let start = idx;
        while b.get(idx).copied().is_some_and(|c| c.is_ascii_digit()) {
            idx += 1;
        }
        if idx == start {
            return None;
        }
    }

    let offset_secs: i64 = match b.get(idx).copied() {
        Some(b'Z') if idx + 1 == b.len() => 0,
        Some(b'+') | Some(b'-') => {
            let sign = if b[idx] == b'-' { -1 } else { 1 };
            let rest = &date[idx + 1..];
            if rest.len() != 5
                || rest.as_bytes()[2] != b':'
                || !digits(rest.as_bytes(), &[0..2, 3..5])
            {
                return None;
            }
            let offset_hh: i64 = rest[0..2].parse().ok()?;
            let offset_mm: i64 = rest[3..5].parse().ok()?;
            if offset_hh > 23 || offset_mm > 59 {
                return None;
            }
            sign * (offset_hh * 3_600 + offset_mm * 60)
        }
        _ => return None,
    };

    let local_secs = crate::clock::days_from_civil((y, m, d)) * 86_400 + hh * 3_600 + mm * 60 + ss;
    let utc_secs = local_secs - offset_secs;
    let (utc_year, utc_month, utc_day) = crate::clock::civil_from_days(utc_secs.div_euclid(86_400));
    let rem = utc_secs.rem_euclid(86_400);
    Some(format!(
        "{utc_year:04}-{utc_month:02}-{utc_day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    ))
}

pub fn import(ctx: &mut Ctx, args: &Args) -> R {
    let file_path = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac import <path to tree.json>"))?;
    if !ctx.tree.is_empty_tree() {
        return Err(Failure::Model(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.tree.total()
        )));
    }
    let raw = std::fs::read_to_string(file_path)?;
    let old: Old = serde_json::from_str(&raw)
        .map_err(|e| Failure::usage(format!("{file_path} is not a spike tree.json: {e}")))?;

    let mut nodes: Vec<&OldNode> = old.nodes.values().collect();
    nodes.sort_by_key(|n| n.id);

    // The redaction guard runs **before** anything is written. A tree coming
    // from outside is exactly the case where a key may have slipped in.
    for n in &nodes {
        let fields: Vec<(&str, &str)> = vec![
            ("title", &n.title),
            ("why", &n.why),
            ("outcome", &n.outcome),
            ("note", &n.note),
        ];
        if let Some(mut h) = redact::check_fields(&fields) {
            h.field = format!("node #{} ({})", n.id, h.field);
            return Err(Failure::Redaction(Box::new(h)));
        }
        if let Some(mut h) = n.refs.iter().find_map(|r| redact::check_field("ref", r)) {
            h.field = format!("node #{} (ref)", n.id);
            return Err(Failure::Redaction(Box::new(h)));
        }
    }

    // `d866`: every date on every node has to resolve before anything is
    // written, exactly like the redaction guard just above -- an import
    // that writes some events and then refuses partway through is worse
    // than one that never started. Every offending date is named, not just
    // the first: a person fixing a spike tree by hand wants the whole list
    // in one pass, not one refusal per re-run.
    let mut bad_dates: Vec<String> = Vec::new();
    for n in &nodes {
        if instant(&n.opened).is_none() {
            bad_dates.push(format!("#{} opened {:?}", n.id, n.opened));
        }
        if let Some(closed) = &n.closed {
            if instant(closed).is_none() {
                bad_dates.push(format!("#{} closed {:?}", n.id, closed));
            }
        }
    }
    if !bad_dates.is_empty() {
        return Err(Failure::usage(format!(
            "import cannot place these dates on the timeline, so nothing was\n  \
             written:\n\n  {}\n\n  \
             Accepted: a real YYYY-MM-DD date, or a full RFC 3339 date-time -- a T\n  \
             separator, optional fractional seconds, and a Z or +HH:MM/-HH:MM offset.",
            bad_dates.join("\n  ")
        )));
    }

    let ulids: BTreeMap<u64, String> = nodes.iter().map(|n| (n.id, id::ulid())).collect();

    // `t594`: `lock_for_write` can reload the tree if
    // another writer landed first, and everything that read the tree
    // before this point -- `seq`, whether it still counts as empty --
    // has to be read again after, or a second writer racing this one
    // hands out the very `seq` `t594` already fixed a
    // door over. Taken before `seq`/`lane` are read, not after: numbering
    // happens under the lock, like any other write.
    ctx.lock_for_write()?;
    if !ctx.tree.is_empty_tree() {
        return Err(Failure::Model(format!(
            "  The tree already has {} nodes. Importing on top would duplicate numbers.\n\n  \
             Import into a freshly created .vivac/.",
            ctx.tree.total()
        )));
    }

    let mut events = Vec::new();
    let mut seq = ctx.tree.seq;
    let actor = ctx.store.config.actor.clone();
    // The lane this context actually runs as, `main` only as the fallback
    // every write already uses (`emit`): `import` writes outside the
    // funnel (`write_raw`, below), so it has to decide this itself rather
    // than being signed for automatically (`t594`). A
    // worktree still pending never joins through here -- `import` never
    // calls `emit`, so it never mints a lane of its own -- and its nodes
    // land on `main` exactly as they did before this lane ever existed,
    // rather than inventing a second way to join one.
    let lane = ctx
        .lane
        .clone()
        .unwrap_or_else(|| crate::lane::MAIN.to_string());
    let mut push_event = |body: Body, ts: String| {
        seq += 1;
        events.push(Event {
            seq,
            id: id::ulid(),
            ts,
            actor: actor.clone(),
            lane: lane.clone(),
            payload: body,
        });
    };

    for n in &nodes {
        push_event(
            Body::NodeCreated {
                node: ulids[&n.id].clone(),
                num: n.id,
                kind: kind_of(&n.kind),
                title: n.title.clone(),
                why: n.why.clone(),
                parent: n.parent.and_then(|p| ulids.get(&p).cloned()),
                blocks: n.blocks,
                refs: n.refs.clone(),
                governs: vec![],
                arms: vec![],
                against: None,
            },
            instant(&n.opened).expect("validated in the pre-pass above"),
        );
    }
    for n in &nodes {
        if !n.note.is_empty() {
            push_event(
                Body::NodeNoted {
                    node: ulids[&n.id].clone(),
                    note: n.note.clone(),
                },
                instant(&n.opened).expect("validated in the pre-pass above"),
            );
        }
        let state = state_of(&n.status);
        if state != State::Active {
            push_event(
                Body::StateChanged {
                    node: ulids[&n.id].clone(),
                    state,
                    outcome: n.outcome.clone(),
                    // The spike had no closure rule, so there is no way to
                    // know whether a close was deliberate. They import
                    // unforced: the ones that turn out false have to show up
                    // in `check`, which is exactly what needs to be seen.
                    forced: false,
                    // The spike predates `--until` entirely; nothing it
                    // wrote can carry a return date (`d899`).
                    until: None,
                },
                instant(n.closed.as_deref().unwrap_or(&n.opened))
                    .expect("validated in the pre-pass above"),
            );
        }
    }

    let total = nodes.len();
    let lock = ctx
        .lock
        .as_ref()
        .ok_or_else(|| Failure::Io(std::io::Error::other("write without the tree's lock")))?;
    ctx.store.write_raw(lock, &events)?;
    outln!("  {total} nodes imported from {file_path}");
    outln!("        {} events written to .vivac/events", events.len());
    outln!();
    outln!("  Review what the spike could not see:  vivac check");
    Ok(())
}
