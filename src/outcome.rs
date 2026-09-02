//! What a write returns, instead of printing it.
//!
//! `t106`: the CLI is not the only caller any more, and a caller over MCP
//! cannot read stdout. Every write op is moving from printing straight to a
//! terminal to returning an `Outcome`, so a second surface can read the same
//! answer the CLI does. `main.rs` is the only place left that turns one into
//! text, in `to_text` below -- a mirror of `brief::to_text`, which already
//! proved the split works: the data is built once, and the terminal
//! rendering is a formatting step over it, not a second source of truth.
//!
//! **A variant carries the value an operation computed, never the sentence it
//! would have printed.** Where today's code interpolates a value into a
//! string -- `r.phrase()`, `kind.prefix()`, an anchor's short hash -- the
//! field here is that value, and `to_text` does the interpolating. Where
//! today's code prints a fixed sentence that names no data of its own, the
//! field is the `bool` or `Option` that decides whether it appears, and the
//! sentence itself lives in `to_text`. A model rule such as the depth
//! threshold `push` warns past, or whether a close needed `--force`, is
//! decided once, by the operation that knows the rule; `to_text` only knows
//! how to say what was already decided.

use crate::anchor::AnchorRef;
use crate::model::Counts;

/// A node closed, win or by force. Shared by `pop` and `done`, which both
/// wrap it: closing itself is one rule (`ops::close_node`) applied from two
/// places.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Closed {
    pub alias: String,
    pub title: String,
    pub force: bool,
}

/// `push`'s advice to reconsider the root, past a stack four deep. `MODEL.md`
/// §6.1: intervene, never block, so it is advice and never a refusal.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DepthAdvice {
    pub depth: usize,
    pub root_alias: String,
    pub root_title: String,
}

/// Where `pop` lands: the parent that becomes the new focus, with what is
/// still open below it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoppedTo {
    pub alias: String,
    pub title: String,
    pub counts: Counts,
}

/// The parent `add` filed a node under, when it did not land at the root.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AddedUnder {
    pub alias: String,
    pub title: String,
}

/// The parent `promote` leaves behind. Provenance does not move: the node
/// promoted is still born from here, only its rank changed (`d33`'s sibling
/// rule for goals).
#[derive(Debug, Clone, serde::Serialize)]
pub struct StillBornFrom {
    pub alias: String,
    pub title: String,
}

/// A node `abandon` saved out of the fall, still born where it was.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RescuedNode {
    pub alias: String,
    pub title: String,
}

/// The decision `decide --supersedes` retires.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SupersededNode {
    pub alias: String,
}

/// The two shapes `flag` can leave: cleared, or raised with the reason that
/// justified it. `BRIEF-SPEC.md` §10 makes the reason mandatory on the way in;
/// this is what comes back out.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "change")]
pub enum FlagChange {
    Off,
    Raised { title: String, reason: String },
}

/// A node `restore` cannot put back on the stack, and why: it closed, it was
/// abandoned, or it no longer exists at all.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LostNode {
    pub alias: String,
    pub title: String,
    /// Already resolved to a word (`n.state.word(n.kind)`, or the literal
    /// `"gone"`): there is no live node left to ask, past this point, for a
    /// node that no longer resolves at all.
    pub state: String,
}

/// One path that changed since a vivac's anchor, and how many times.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChangeLine {
    pub file_path: String,
    pub times: usize,
}

/// The three shapes `restore`'s diff can take. Which one applies is a fact
/// about the anchor and the repository, decided once by the operation; the
/// render only knows how to say each of the three.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "anchor")]
pub enum RestoreAnchor {
    /// `Null`: there was never anything to diff against (`MODEL.md` §8).
    Empty,
    NoChanges {
        anchor_short: String,
    },
    Changed {
        anchor_short: String,
        changes: Vec<ChangeLine>,
        /// The `governs` globs the stack declared at save time. Empty unless
        /// something on it declared one, and that is what decides whether
        /// the "touch what the stack governed" count is worth saying at all.
        working_set: Vec<String>,
    },
}

/// What a write handed back, instead of a line on a terminal.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "op")]
pub enum Outcome {
    Pushed {
        alias: String,
        title: String,
        blocks: bool,
        advice: Option<DepthAdvice>,
    },
    Popped {
        closed: Closed,
        parent: Option<PoppedTo>,
    },
    Done {
        closed: Closed,
    },
    Added {
        alias: String,
        title: String,
        parent: Option<AddedUnder>,
        blocks: bool,
    },
    Noted {
        alias: String,
    },
    Blocked {
        alias: String,
        blocks: bool,
        parent_alias: String,
        parent_title: String,
    },
    Promoted {
        alias: String,
        title: String,
        parent: Option<StillBornFrom>,
    },
    Abandoned {
        alias: String,
        title: String,
        cascaded: Option<usize>,
        rescued: Vec<RescuedNode>,
    },
    Parked {
        alias: String,
        title: String,
    },
    Focused {
        alias: String,
        revived: bool,
    },
    Flagged {
        alias: String,
        flag: String,
        change: FlagChange,
    },
    Decided {
        alias: String,
        title: String,
        superseded: Option<SupersededNode>,
        no_alternatives: bool,
    },
    Saved {
        num: u64,
        label: String,
        anchor: AnchorRef,
        next: String,
    },
    Restored {
        alias: String,
        kind: String,
        ts: String,
        label: String,
        next_intent: String,
        lost: Vec<LostNode>,
        anchor: RestoreAnchor,
    },
    /// The end-of-session hook's automatic stop. Silent today, the same way
    /// `auto_vivac` prints nothing: a segment nobody was asked to summarize
    /// is not a place to start inventing prose.
    AutoStopped,
    /// A session opened, for the start hook. Silent for the same reason: the
    /// brief is what the hook actually shows, and this is not it.
    SessionOpened,
}

fn closed_lines(out: &mut Vec<String>, c: &Closed) {
    out.push(format!(
        "  {}  {}  -> {}",
        c.alias,
        c.title,
        if c.force { "closed BY FORCE" } else { "closed" }
    ));
    if c.force {
        out.push("        recorded as a false close in every render".to_string());
    }
}

/// The `Outcome` as plain text, the way the CLI has always printed it.
///
/// A mirror of `brief::to_text`: the data is already final by the time it
/// gets here, so this only ever formats, never decides.
pub fn to_text(o: &Outcome) -> String {
    let mut lines: Vec<String> = Vec::new();
    match o {
        Outcome::Pushed {
            alias,
            title,
            blocks,
            advice,
        } => {
            lines.push(format!("  {alias}  {title}"));
            if *blocks {
                lines.push("        blocks its parent from closing".to_string());
            }
            if let Some(a) = advice {
                lines.push(String::new());
                lines.push(format!(
                    "  You are {} levels away from {} \"{}\".",
                    a.depth, a.root_alias, a.root_title
                ));
                lines.push("  Is this still a detour, or did the real goal move?".to_string());
                lines.push("  If it moved:  vivac promote".to_string());
            }
        }
        Outcome::Popped { closed, parent } => {
            closed_lines(&mut lines, closed);
            match parent {
                Some(p) => {
                    lines.push(format!("  back to {}  {}", p.alias, p.title));
                    let f = p.counts.phrase();
                    if !f.is_empty() {
                        lines.push(format!("        ({f} below it)"));
                    }
                }
                None => lines.push("  empty stack".to_string()),
            }
        }
        Outcome::Done { closed } => closed_lines(&mut lines, closed),
        Outcome::Added {
            alias,
            title,
            parent,
            blocks,
        } => {
            let where_at = match parent {
                Some(p) => format!(" under {}", p.alias),
                None => " (root)".to_string(),
            };
            lines.push(format!("  {alias}  {title}{where_at}"));
            if *blocks {
                lines.push("        blocks its parent from closing".to_string());
            }
        }
        Outcome::Noted { alias } => lines.push(format!("  {alias} noted")),
        Outcome::Blocked {
            alias,
            blocks,
            parent_alias,
            parent_title,
        } => {
            let verb = if *blocks {
                "blocks"
            } else {
                "no longer blocks"
            };
            lines.push(format!(
                "  {alias} {verb} the close of {parent_alias}  {parent_title}"
            ));
        }
        Outcome::Promoted {
            alias,
            title,
            parent,
        } => {
            lines.push(format!("  {alias}  {title}  -> a goal of its own"));
            if let Some(p) = parent {
                lines.push(format!("        still born from {}  {}", p.alias, p.title));
            }
        }
        Outcome::Abandoned {
            alias,
            title,
            cascaded,
            rescued,
        } => {
            lines.push(format!("  {alias}  {title}  -> abandoned"));
            if let Some(n) = cascaded {
                lines.push(format!("        and {n} descendant(s) with it"));
            }
            if !rescued.is_empty() {
                lines.push(String::new());
                lines.push(format!("  Rescued, and still born from {alias}:"));
                for r in rescued {
                    lines.push(format!("      {:<6} {}", r.alias, r.title));
                }
                lines.push(String::new());
                lines.push(
                    "  Their lineage crosses an abandoned node on purpose: where they".to_string(),
                );
                lines.push("  were born does not change because it got discarded.".to_string());
            }
        }
        Outcome::Parked { alias, title } => {
            lines.push(format!("  {alias}  {title}  -> parked"));
            lines.push("        shows up in:  vivac parked".to_string());
        }
        Outcome::Focused { alias, revived } => {
            if *revived {
                lines.push(format!("  {alias} is open again"));
            }
        }
        Outcome::Flagged {
            alias,
            flag,
            change,
        } => match change {
            FlagChange::Off => lines.push(format!("  {alias}  is no longer {flag}")),
            FlagChange::Raised { title, reason } => {
                lines.push(format!("  {alias}  {title}  -> {flag}"));
                lines.push(format!("        {reason}"));
            }
        },
        Outcome::Decided {
            alias,
            title,
            superseded,
            no_alternatives,
        } => {
            lines.push(format!("  {alias}  {title}"));
            if let Some(s) = superseded {
                lines.push(format!("        {} becomes superseded", s.alias));
            }
            if *no_alternatives {
                lines.push(
                    "        no alternatives recorded: in a month they get proposed again"
                        .to_string(),
                );
            }
        }
        Outcome::Saved {
            num,
            label,
            anchor,
            next,
        } => {
            let shown = if label.is_empty() { "no label" } else { label };
            lines.push(format!("  v{num}  {shown}"));
            if !anchor.is_empty_tree() {
                lines.push(format!("        anchored to {}", anchor.short()));
            } else {
                lines.push("        no anchor: there is no version control here".to_string());
            }
            if next.is_empty() {
                lines.push(
                    "        no --next: coming back there will be nothing to pick up".to_string(),
                );
            }
        }
        Outcome::Restored {
            alias,
            kind,
            ts,
            label,
            next_intent,
            lost,
            anchor,
        } => {
            lines.push(String::new());
            lines.push(format!(
                "  {alias} · {kind} · {}",
                crate::clock::date_of(ts)
            ));
            if !label.is_empty() {
                lines.push(format!("  {label}"));
            }
            lines.push(String::new());
            if !next_intent.is_empty() {
                lines.push(format!("  you were about to:  {next_intent}"));
                lines.push(String::new());
            }
            for p in lost {
                lines.push(format!(
                    "  no longer on the stack:  {} {} [{}]",
                    p.alias, p.title, p.state
                ));
            }
            if !lost.is_empty() {
                lines.push(String::new());
            }
            match anchor {
                RestoreAnchor::Empty => {
                    lines.push(
                        "  No anchor: there is no diff to show, only the date above.".to_string(),
                    );
                    lines.push(String::new());
                }
                RestoreAnchor::NoChanges { anchor_short } => {
                    lines.push(format!("  Nothing changed since {anchor_short}."));
                    lines.push(String::new());
                }
                RestoreAnchor::Changed {
                    anchor_short,
                    changes,
                    working_set,
                } => {
                    let touching = changes
                        .iter()
                        .filter(|c| {
                            working_set
                                .iter()
                                .any(|g| crate::glob::covers(g, &c.file_path))
                        })
                        .count();
                    let suffix = if working_set.is_empty() {
                        String::new()
                    } else {
                        format!(", {touching} of them touch what the stack governed")
                    };
                    lines.push(format!(
                        "  {} changes since {anchor_short}{suffix}",
                        changes.len()
                    ));
                    for c in changes.iter().take(6) {
                        lines.push(format!("      {:<52} ({})", c.file_path, c.times));
                    }
                    if changes.len() > 6 {
                        lines.push(format!("      ... and {} more", changes.len() - 6));
                    }
                    lines.push(String::new());
                }
            }
        }
        // Neither hook op has ever printed anything: both fire from a Claude
        // Code hook, never from a terminal, so there is nobody to tell.
        Outcome::AutoStopped | Outcome::SessionOpened => {}
    }
    // `Outcome::Focused { revived: false, .. }` and the two hook outcomes
    // just above are the shapes with nothing to say: the CLI printed no
    // line at all for them, and an empty `Vec` here has to come out as an
    // empty string and not as a single blank line.
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}
