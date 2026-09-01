//! `reconcile` — the diff between the tree and the anchor's history.
//!
//! `ROADMAP.md` §7 names the project's principal risk plainly: **that the
//! graph goes stale and starts to lie**. Nothing in the tool contradicts the
//! tree, so a tree that drifts drifts in silence, and a `brief` built on it
//! does not go quiet -- it keeps answering, wrongly, into every session it is
//! injected into. That is the failure mode worth a command of its own.
//!
//! It asks one question: **what changed since the tree last looked, and which
//! of it does no node claim?** The claim is `governs`, the globs a node
//! declares over the files it owns (`MODEL.md` §10).
//!
//! **The interactive menu is not here.** `INTEGRATION.md` §9 draws this as a
//! prompt -- `[t12] [t7] [new] [ignore]` -- and the DX pillar had already
//! settled that one: the CLI comes first, without exception, because the agent
//! writes through it and a feature that lives only in an interactive interface
//! leaves half the users out. So this prints the finding and the command that
//! acts on it, the way `triage` does. The menu can come later, in the TUI,
//! over exactly this data.
//!
//! It never writes. Reconciling is a judgement about what the work meant, and
//! the tool does not have it: it can say *nobody claims `src/util/retry.rs`*,
//! and it cannot say which thread that file belongs to.

use crate::anchor::Anchor;
use crate::args::Args;
use crate::brief::clip;
use crate::failure::R;
use crate::glob;
use crate::model::{Node, Tree, Vivac};
use crate::render::print_json;
use serde_json::json;

/// How many files a section prints before it stops and says how many are left.
/// `--json` is never truncated.
const SHOWN: usize = 20;

/// A changed file, and what the tree has to say about it.
struct Verdict<'a> {
    file: String,
    times: usize,
    /// Nodes whose `governs` covers the file, open ones first.
    claimed_by: Vec<&'a Node>,
}

impl Verdict<'_> {
    fn claimed_and_open(&self) -> bool {
        self.claimed_by.iter().any(|n| n.state.is_open())
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Which stop to measure from: `--since <v>`, or the last one.
fn reference<'a>(a: &'a Tree, args: &Args) -> Result<Option<&'a Vivac>, crate::failure::Failure> {
    match args.opt("since") {
        Some(s) => a
            .vivac(s)
            .map(Some)
            .ok_or_else(|| crate::failure::Failure::usage(format!("No such vivac: {s}."))),
        None => Ok(a.vivacs.last()),
    }
}

pub fn reconcile(a: &Tree, anchor: &dyn Anchor, args: &Args) -> R {
    let Some(since) = reference(a, args)? else {
        println!();
        println!("  No stop to measure from: this tree has no vivacs yet.");
        println!();
        println!("      vivac save \"<label>\"");
        println!();
        return Ok(());
    };

    if since.anchor.is_empty_tree() {
        println!();
        println!(
            "  {} has no anchor, so there is no history to read.",
            since.alias()
        );
        println!("  Without version control the tree cannot be contradicted; that is");
        println!("  the floor of the product and not a failure.");
        println!();
        return Ok(());
    }

    // The tool's own store is not work. Without this, every reconcile reports
    // the log it just wrote to.
    let changes: Vec<crate::anchor::Change> = anchor
        .changed_since(&since.anchor)
        .into_iter()
        .filter(|c| !c.file_path.replace('\\', "/").starts_with(".vivac/"))
        .collect();

    let governing: Vec<&Node> = a.nodes_iter().filter(|n| !n.governs.is_empty()).collect();

    let mut verdicts: Vec<Verdict> = changes
        .iter()
        .map(|c| {
            let mut claimed_by: Vec<&Node> = governing
                .iter()
                .filter(|n| n.governs.iter().any(|g| glob::covers(g, &c.file_path)))
                .copied()
                .collect();
            claimed_by.sort_by_key(|n| (!n.state.is_open(), n.num));
            Verdict {
                file: c.file_path.clone(),
                times: c.times,
                claimed_by,
            }
        })
        .collect();
    verdicts.sort_by(|x, y| y.times.cmp(&x.times).then_with(|| x.file.cmp(&y.file)));

    let unclaimed: Vec<&Verdict> = verdicts
        .iter()
        .filter(|v| v.claimed_by.is_empty())
        .collect();
    let stale: Vec<&Verdict> = verdicts
        .iter()
        .filter(|v| !v.claimed_by.is_empty() && !v.claimed_and_open())
        .collect();
    let live: Vec<&Verdict> = verdicts.iter().filter(|v| v.claimed_and_open()).collect();

    if args.has("json") {
        let one = |v: &Verdict| {
            json!({
                "file": v.file,
                "changes": v.times,
                "claimed_by": v.claimed_by.iter().map(|n| json!({
                    "alias": n.alias(),
                    "title": n.title,
                    "state": n.state,
                })).collect::<Vec<_>>(),
            })
        };
        return print_json(json!({
            "since": since.alias(),
            "since_ts": since.ts,
            "anchor": since.anchor.short(),
            "governing_nodes": governing.len(),
            "changed": verdicts.len(),
            "unclaimed": unclaimed.iter().map(|v| one(v)).collect::<Vec<_>>(),
            "claimed_by_closed_work": stale.iter().map(|v| one(v)).collect::<Vec<_>>(),
            "claimed_and_open": live.iter().map(|v| one(v)).collect::<Vec<_>>(),
        }));
    }

    println!();
    println!(
        "  RECONCILE - since {} {}, {}",
        since.alias(),
        since.anchor.short(),
        plural(verdicts.len(), "file changed", "files changed")
    );

    if verdicts.is_empty() {
        println!();
        println!("  Nothing changed. The tree and the work agree.");
        println!();
        return Ok(());
    }

    // The case that will be true of most trees on the first run, and the one
    // where a list of every file is the least useful thing to print. Say the
    // real problem once instead of repeating a symptom per line.
    if governing.is_empty() {
        println!();
        println!("  No node declares what it governs, so nothing here can be claimed.");
        println!("  Until some node says which files it owns, this command has nothing");
        println!("  to compare the work against.");
        println!();
        println!("      vivac push \"<title>\" --why \"<reason>\" --governs \"src/auth/**\"");
        println!();
        return Ok(());
    }

    section(
        "NOBODY CLAIMS THESE",
        "push \"<title>\" --governs <path>",
        &unclaimed,
        |_| String::new(),
    );
    section(
        "CLAIMED ONLY BY CLOSED WORK",
        "focus <id> --reopen  |  block <id>",
        &stale,
        |v| {
            v.claimed_by
                .iter()
                .map(|n| format!("{} [{}]", n.alias(), n.state.word(n.kind)))
                .collect::<Vec<_>>()
                .join(" ")
        },
    );

    if args.has("all") {
        section("CLAIMED, AND THE WORK IS OPEN", "", &live, |v| {
            v.claimed_by
                .iter()
                .filter(|n| n.state.is_open())
                .map(|n| n.alias())
                .collect::<Vec<_>>()
                .join(" ")
        });
    } else if !live.is_empty() {
        println!();
        println!(
            "  {} under work that is open, which is what is supposed to happen.  --all",
            plural(live.len(), "file", "files")
        );
    }

    if unclaimed.is_empty() && stale.is_empty() {
        println!();
        println!("  Nothing to reconcile.");
    }
    println!();
    Ok(())
}

fn section(title: &str, action: &str, rows: &[&Verdict], note: impl Fn(&Verdict) -> String) {
    if rows.is_empty() {
        return;
    }
    println!();
    println!(
        "{}",
        format!(
            "  {} ({}){}{}",
            title,
            rows.len(),
            " ".repeat(38usize.saturating_sub(title.len() + 4)),
            action
        )
        .trim_end()
    );
    for v in rows.iter().take(SHOWN) {
        // Trimmed, because a row with no note would otherwise carry the
        // column padding out to the edge of the line.
        println!(
            "{}",
            format!("    {:<44} {:>3}  {}", clip(&v.file, 44), v.times, note(v)).trim_end()
        );
    }
    if rows.len() > SHOWN {
        println!("    + {} more   --json", rows.len() - SHOWN);
    }
}
