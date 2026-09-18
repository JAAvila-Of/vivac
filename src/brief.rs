//! The `brief`: a deterministic render bounded in tokens.
//!
//! `BRIEF-SPEC.md`. It answers three questions in order of importance: where
//! we are and how we got here, what governs this point, and **what is out of
//! scope right now**. The third is the one no other tool emits: every memory
//! tool dumps what is relevant, and the problem in agentic development is the
//! opposite one, bounding.
//!
//! Two rules override everything else:
//!
//! - **Same log + same `--now` + same anchor state -> same bytes.** Without
//!   `--now` determinism would be impossible, because ages are relative to
//!   the moment.
//! - **The spine is never truncated.** If it does not fit, the budget is
//!   wrong and it says so, but it comes out whole: it is the answer to
//!   question 1, and without it the brief has no reason to exist.

use crate::anchor::Anchor;
use crate::args::Args;
use crate::event::{Kind, State, WhereRepo};
use crate::failure::R;
use crate::model::{Node, Tree};
use std::collections::HashSet;
use std::path::Path;

const BUDGET: usize = 1500;
/// The whole brief is pure ASCII.
///
/// `BRIEF-SPEC.md` §7 draws the spine with box-drawing characters, but the DX
/// pillar demands it degrade without breaking "in cmd.exe as well as Windows
/// Terminal", and there any code page that is not UTF-8 turns them into
/// garbage. What is normative in §7 are the markers --that the focus be
/// visible, that a flag carry its reason, that an empty section not show--
const RULE: &str = "------------------------------------------------------------";

/// One section of the brief. The vector order is the one in §3, which is both
/// render order and priority order: truncation starts from the bottom.
struct Section {
    lines: Vec<String>,
    truncable: bool,
}

impl Section {
    fn fixed(lines: Vec<String>) -> Section {
        Section {
            lines,
            truncable: false,
        }
    }
    fn loose(lines: Vec<String>) -> Section {
        Section {
            lines,
            truncable: true,
        }
    }
}

/// Token estimator. It is an estimate and the ceiling is indicative: what
/// matters is that it be **deterministic**, so two runs of the same log
/// truncate the same way.
fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn tokens_of(sections: &[Section]) -> usize {
    sections
        .iter()
        .flat_map(|s| s.lines.iter())
        .map(|l| tokens(l) + 1)
        .sum()
}

/// Truncates a list keeping the first `n`. An item from the middle is never
/// dropped in silence.
fn trim_list(mut v: Vec<String>, n: usize, which: &str) -> Vec<String> {
    if v.len() > n {
        let left_over = v.len() - n;
        v.truncate(n);
        v.push(format!("      ... and {left_over} more (vivac {which})"));
    }
    v
}

fn heading(title: &str, body: Vec<String>) -> Vec<String> {
    // Empty sections are omitted whole, heading included: a brief with nothing
    // parked does not say "DO NOT TOUCH NOW: (empty)".
    if body.is_empty() {
        return vec![];
    }
    let mut v = vec![String::new(), format!(" {title}")];
    v.extend(body);
    v
}

/// Constraints that govern the path.
///
/// **By `spawns` only.** Inheriting through `depends_on` as well would turn
/// the computation from O(depth) into O(graph), and would lose the property
/// that inheritance is legible by looking at the stack on screen.
pub(crate) fn constraints<'a>(a: &'a Tree, lineage: &[&Node]) -> Vec<&'a Node> {
    let on_lineage: HashSet<u64> = lineage.iter().map(|n| n.num).collect();
    let mut v: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Constraint && n.state.is_open())
        .filter(|n| {
            // Project-wide, or reachable from the path. Project-wide means
            // hanging off a root **or being one**: `MODEL.md` §9.5 blesses
            // `parent: PROJECT`, and a node with no parent at all is the
            // strongest form of that, not a weaker one.
            let project_wide = n.parent.is_none()
                || n.parent
                    .and_then(|p| a.node_by_num(p))
                    .is_some_and(|p| p.parent.is_none());
            project_wide
                || a.ancestors(n.num)
                    .iter()
                    .any(|p| on_lineage.contains(&p.num))
        })
        .collect();
    // At risk first --the ones carrying a flag-- and then by alias.
    v.sort_by_key(|n| (n.flags.is_empty(), n.num));
    v
}

/// `t533` piece (c) (`f134`, `f55`): a node whose state is not open carries
/// its word, in brackets, right after the title -- the same word `why` and
/// `tree` already show (`render.rs`, `label`). Title and mark share the 44
/// columns the title alone used to have, so the two fit together; the mark
/// is never the part that gives. An open node keeps exactly the bytes it
/// always has.
fn spine_label(a: &Tree, n: &Node) -> String {
    if n.state.is_open() {
        return clip(n.title(a), 44);
    }
    let mark = format!("  [{}]", n.state.word(n.kind));
    let budget = 44usize.saturating_sub(mark.chars().count());
    format!("{}{mark}", clip(n.title(a), budget))
}

fn spine(a: &Tree, lineage: &[&Node]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, n) in lineage.iter().enumerate() {
        let first = i == 0;
        let is_last = i == lineage.len() - 1;
        // Continuation: the trunk carries on while anything is left below.
        let cont = if is_last { "        " } else { "  |     " };

        let branch = if first {
            " GOAL ".to_string()
        } else if is_last {
            "  `-- ".to_string()
        } else {
            "  |-- ".to_string()
        };
        let flags: Vec<&str> = n.flags.keys().map(|b| b.word()).collect();
        let flag = if flags.is_empty() {
            String::new()
        } else {
            format!("  ! {}", flags.join(" "))
        };
        let here_mark = if is_last { "   <== HERE" } else { "" };
        v.push(format!(
            "{branch}{:<6} {}{flag}{here_mark}",
            n.alias(),
            spine_label(a, n)
        ));
        let why = n.why(a);
        if !first && !why.is_empty() {
            v.push(format!("{cont}why: {}", clip(why, 52)));
        }
        let governs = n.governs(a);
        if !governs.is_empty() {
            v.push(format!("{cont}governs: {}", governs.join(" ")));
        }
        if !is_last {
            v.push("  |".to_string());
        }
    }
    v
}

/// Cuts on a word boundary without exceeding `n`, **counting the ellipsis**.
/// Budgeting for it matters: otherwise the cut overruns on exactly the
/// tightest lines of the brief, which are the ones being truncated.
pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let t: String = s.chars().take(n.saturating_sub(3)).collect();
    match t.rsplit_once(' ') {
        Some((a, _)) if !a.is_empty() => format!("{a}..."),
        _ => format!("{t}..."),
    }
}

/// Project level: hanging off nothing, or off a node that itself hangs off
/// nothing. `MODEL.md` §9.5 blesses `parent: PROJECT`, and a node with no
/// parent at all is the strongest form of that, not a weaker one.
/// `constraints()` above has drawn the line this way from the start; `t533`
/// piece (b) widens `standing()`'s own clause to the same shape, and the
/// no-focus path in `to_text` stands on nothing else.
fn project_wide(a: &Tree, n: &Node) -> bool {
    n.parent.is_none()
        || n.parent
            .and_then(|p| a.node_by_num(p))
            .is_some_and(|p| p.parent.is_none())
}

/// Standing decisions that reach the focus: project-level, on the path, or
/// with a `governs` overlapping the focus's own. Superseded ones never
/// appear. Always called with a real focus; with none, `to_text` reads
/// `project_wide` on its own instead, since there is neither a path nor a
/// `governs` to overlap.
pub(crate) fn standing<'a>(a: &'a Tree, focus: &Node, on_lineage: &HashSet<u64>) -> Vec<&'a Node> {
    let mut dec: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Decision && n.state.is_open())
        .filter(|n| {
            project_wide(a, n)
                || on_lineage.contains(&n.num)
                || n.parent.is_some_and(|p| on_lineage.contains(&p))
                || n.governs(a)
                    .iter()
                    .any(|g| focus.governs(a).iter().any(|f| crate::glob::covers(g, f)))
        })
        .collect();
    dec.sort_by_key(|n| n.num);
    dec
}

/// Whether a node could ever show up in `OPEN GOALS`, kind-wise: a goal, or a
/// node with no parent at all, and never a pillar, a rule, a decision or a
/// constraint -- the same governance kinds `Node::is_front` already keeps out
/// of pending work. `t533` §3.6 (`f73`, `f456`).
fn is_goal_shaped(n: &Node) -> bool {
    !matches!(
        n.kind,
        Kind::Decision | Kind::Constraint | Kind::Pillar | Kind::Rule
    ) && (n.kind == Kind::Goal || n.parent.is_none())
}

/// What `BORN FROM HERE` lists: `focus`'s own open, front children, plus how
/// many more open fronts hang further down without being listed one by one.
///
/// `f49`: a blocking question is left out here, because `BLOCKS` already
/// lists it -- showing it twice says the same thing in two places for no
/// reason. A blocking task still shows, asterisk and all: only a question is
/// also a row of its own in `BLOCKS`.
fn born_from_here(a: &Tree, focus: &Node) -> Vec<String> {
    let mut children: Vec<String> = a
        .children(focus.num)
        .into_iter()
        .filter(|c| c.is_front())
        .filter(|c| !(c.kind == Kind::Question && c.blocks))
        .map(|c| {
            format!(
                "  {} {:<6} {}",
                if c.blocks { '*' } else { ' ' },
                c.alias(),
                c.title(a)
            )
        })
        .collect();
    // Closing a parent cannot make its open children invisible. They are
    // counted and the place to look is named; listing them here would drag in
    // the whole tree, which is exactly the noise the focus exists to keep
    // out.
    let direct: std::collections::HashSet<&str> = a
        .children(focus.num)
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let deep = a
        .descendants(focus.num)
        .into_iter()
        .filter(|n| n.is_front() && !direct.contains(n.id.as_str()))
        .filter(|n| !a.children(n.num).iter().any(|c| c.is_front()))
        .count();
    if deep > 0 {
        children.push(format!(
            "    + {deep} further down, outside this level   vivac open"
        ));
    }
    children
}

/// The fixed block that takes the spine's place with no focus (`t533`
/// §3.6). Never truncated, the same as the spine.
fn no_focus_block(a: &Tree) -> Vec<String> {
    let mut v = vec![" No active focus.".to_string()];

    let mut goals: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state.is_open() && is_goal_shaped(n))
        .collect();
    goals.sort_by_key(|n| n.num);

    if !goals.is_empty() {
        v.push(String::new());
        v.push(" OPEN GOALS".to_string());
        for m in &goals {
            v.push(format!(
                "  {:<6} {:<40} {} open below",
                m.alias(),
                clip(m.title(a), 40),
                a.counts(m.num).open_count
            ));
        }
    }

    v.push(String::new());
    if a.is_empty_tree() {
        v.push(" Start with:  vivac push \"<title>\" --why \"<reason>\"".to_string());
    } else if let Some(first) = goals.first() {
        v.push(format!(" Pick up with:  vivac focus {}", first.alias()));
        v.push(" Or open another:  vivac push \"<title>\" --why \"<reason>\"".to_string());
    } else {
        // Nothing open of that shape, but something parked would qualify if
        // it were open. `f50`: the action names a real id, never `<id>`.
        let mut parked: Vec<&Node> = a
            .nodes_iter()
            .filter(|n| n.state == State::Suspended && is_goal_shaped(n))
            .collect();
        parked.sort_by_key(|n| n.num);
        match parked.first() {
            Some(p) => {
                v.push(format!(" Pick up with:  vivac focus {}", p.alias()));
                v.push(" Or open another:  vivac push \"<title>\" --why \"<reason>\"".to_string());
            }
            None => {
                v.push(" Open the next one:  vivac push \"<title>\" --why \"<reason>\"".to_string())
            }
        }
    }
    v
}

pub fn brief(
    a: &Tree,
    root: &Path,
    lane_dir: &Path,
    anchor_of: &dyn Anchor,
    args: &Args,
    project: &str,
) -> R {
    print!("{}", to_text(a, root, lane_dir, anchor_of, args, project)?);
    Ok(())
}

/// Whether `root` is a copy of a tree living somewhere else on this
/// machine, laid out for the brief's own shape (`t594` §4.7). Read only,
/// through `copy_of`, never `note`: the brief is a read and must never
/// write (`c319`).
///
/// `copy_notice`'s heading and body are the one thing shared with `check`'s
/// own block -- indentation is each caller's own, the words are not, so
/// nobody has to keep two paragraphs saying the same thing in agreement by
/// hand.
fn copy_block(root: &Path) -> Vec<String> {
    let Some(project_id) = crate::store::first_event_id(root) else {
        return vec![];
    };
    let Some(store_dir) = crate::store::store_dir() else {
        return vec![];
    };
    let (first, rest) = match crate::registry::copy_of(&store_dir, &project_id, root) {
        crate::registry::Noted::Copy { first, rest } => (first, rest),
        crate::registry::Noted::Fine => return vec![],
    };
    // `session start` reaches this before it ever writes anything
    // (`t594`): marking here, rather than after the block is
    // printed, is what lets `registry::warn_if_wrote` see that the very
    // same words already reached whoever is reading before it decides
    // whether to say them again on `stderr`.
    crate::store::mark_shown();
    let notice = crate::registry::copy_notice(first.as_deref(), &rest);
    let mut v = vec![format!(" {}", notice.heading), String::new()];
    v.extend(notice.body.lines().map(|l| format!("  {l}")));
    v.push(String::new());
    v
}

/// How one repository's checkout reads on a BRANCH MOVED line: a branch by
/// its bare name, a detached `HEAD` as `@<short sha>`, and a rebase in
/// progress as `@<branch> (rebasing)` -- the branch git is replaying onto,
/// not the tip it detached from (§5.2, the four forms). `None` when there
/// is nothing knowable at all, which BRANCH MOVED has nothing to say about.
fn head_repr(branch: Option<&str>, sha: Option<&str>, rebasing: bool) -> Option<String> {
    match branch {
        Some(b) if rebasing => Some(format!("@{b} (rebasing)")),
        Some(b) => Some(b.to_string()),
        None => sha.map(|s| format!("@{}", &s[..s.len().min(7)])),
    }
}

/// The redaction guard's own phrase for a branch name it kept out (`d600`,
/// §2.4), reused here rather than invented again: a withheld branch reads
/// the same way whether it is `why` naming where a node was born or
/// BRANCH MOVED naming where a repository moved to.
const BRANCH_WITHHELD: &str = "branch name withheld: it looked like a secret";

/// A declared repository's last known checkout, as `where.changed` wrote
/// it. A withheld branch (§2.4) reads with the guard's own phrase, since
/// what reached the log was already redacted; a repository the lane last
/// saw as gone has nothing to compare with.
fn last_known_repr(r: &WhereRepo) -> Option<String> {
    if r.missing {
        return None;
    }
    if r.withheld {
        return Some(BRANCH_WITHHELD.to_string());
    }
    head_repr(r.branch.as_deref(), r.sha.as_deref(), r.rebasing)
}

/// A repository's checkout right now, read straight off the working tree
/// and never through the log: the redaction guard has not seen this name
/// yet, so it is run past it here -- the same check `ops::snapshot_of` runs
/// before anything reaches the log at all.
fn now_repr(w: &crate::anchor::Where) -> Option<String> {
    let crate::anchor::Where::Head(h) = w else {
        return None;
    };
    if let Some(b) = &h.branch {
        if crate::redact::check_field("branch", b).is_some() {
            return Some(BRANCH_WITHHELD.to_string());
        }
    }
    head_repr(h.branch.as_deref(), h.sha.as_deref(), h.rebasing)
}

/// The branch to look a candidate up for: the checkout's own branch, only
/// when it is not withheld and no rebase is under way -- a rebase's own
/// branch is what it is replaying onto, not a place work was last focused.
fn candidate_branch(w: &crate::anchor::Where) -> Option<String> {
    let crate::anchor::Where::Head(h) = w else {
        return None;
    };
    if h.rebasing {
        return None;
    }
    let b = h.branch.as_ref()?;
    (crate::redact::check_field("branch", b).is_none()).then(|| b.clone())
}

/// One of the lane's declared repositories whose checkout no longer reads
/// the way the lane's own last `where.changed` said it did.
struct Moved {
    path: String,
    before: String,
    now: String,
    /// The branch to offer a candidate for, when there is one to look up.
    candidate_branch: Option<String>,
    root: Option<String>,
}

/// BRANCH MOVED (`t594` §5.2): shows only when today's `HEAD` of some
/// repository of the lane differs from the lane's own last `where.changed`.
/// `[]` covers every tree this never applies to -- no lane, no
/// repositories declared, or no `where.changed` yet to compare against --
/// which is every tree before `setup` ran (§2.6) and reads byte for byte
/// as it always has.
fn branch_moved_block(a: &Tree, lane_dir: &Path) -> Vec<String> {
    let lane = a.lane();
    let Some(state) = a.lanes.get(lane) else {
        return vec![];
    };
    if state.repos.is_empty() {
        return vec![];
    }
    let Some(last) = a.wheres.iter().rev().find(|w| w.lane == lane) else {
        return vec![];
    };

    let mut moved: Vec<Moved> = state
        .repos
        .iter()
        .filter_map(|r| {
            let before_repo = last.repos.iter().find(|w| w.path == r.path)?;
            let before = last_known_repr(before_repo)?;
            let now = crate::anchor::where_of(&lane_dir.join(&r.path));
            let now_line = now_repr(&now)?;
            if before == now_line {
                return None;
            }
            Some(Moved {
                path: r.path.clone(),
                before,
                now: now_line,
                candidate_branch: candidate_branch(&now),
                root: r.root.clone(),
            })
        })
        .collect();
    if moved.is_empty() {
        return vec![];
    }
    moved.sort_by(|x, y| x.path.cmp(&y.path));

    let mut lines = vec![" BRANCH MOVED since this lane last wrote".to_string()];
    for m in &moved {
        lines.push(format!("   {}   {} -> {}", m.path, m.before, m.now));
    }

    // Candidates: this lane's own last focus on the new branch, else
    // another lane's crossed by root commit, else "no earlier work" --
    // capped at three and ordered by `seq` descending (§5.2).
    struct Candidate {
        seq: u64,
        line: String,
        target: Option<String>,
    }
    let mut candidates: Vec<Candidate> = moved
        .iter()
        .filter_map(|m| {
            let branch = m.candidate_branch.as_deref()?;
            Some(
                match a.branch_candidate(lane, &m.path, m.root.as_deref(), branch) {
                    Some(c) => {
                        let node = a.node_by_num(c.node)?;
                        let who = match &c.lane {
                            Some(other) => format!(" (lane {other})"),
                            None => String::new(),
                        };
                        Candidate {
                            seq: c.seq,
                            line: format!(
                                "   last focus on {branch}{who}:   {}   {}",
                                node.alias(),
                                node.title(a)
                            ),
                            target: Some(node.alias()),
                        }
                    }
                    None => Candidate {
                        seq: 0,
                        line: format!("   no earlier work on {branch}"),
                        target: None,
                    },
                },
            )
        })
        .collect();
    candidates.sort_by_key(|x| std::cmp::Reverse(x.seq));
    candidates.truncate(3);
    for c in &candidates {
        lines.push(c.line.clone());
    }

    let mut targets: Vec<&str> = candidates
        .iter()
        .filter_map(|c| c.target.as_deref())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    if let [only] = targets[..] {
        lines.push(format!("   to resume:  vivac focus {only}"));
    }
    // A trailing blank, the same spacer `REPEATED NUMBERS` ends its own
    // block with: this section sits right after the header and relies on
    // nothing after it to open with one of its own.
    lines.push(String::new());

    lines
}

/// One lane's own thread: which lane, what it is focused on, and the
/// `seq` its last write sits at. The shared starting point of OTHER
/// LANES (`t594` §5.3) and `stack --lanes` (§5.5): the first keeps only
/// the lanes that wrote after this one's own last write and are not this
/// one; the second keeps every one of them, this lane included.
pub(crate) struct LaneFocus<'t> {
    pub(crate) id: &'t str,
    pub(crate) name: &'t str,
    pub(crate) focus: &'t Node,
    pub(crate) seq: u64,
}

/// Every lane with something on its own stack to name (`t594` §5.3 and
/// §5.5 alike).
///
/// `[]` covers a tree with one lane that has never written here itself,
/// and every lane whose only events were declaring itself or moving a
/// branch: `Tree::apply` gives *every* event a `lanes` entry, context
/// events included, so an empty `stack` is what tells a lane that
/// actually worked apart from one of those defaults (`t594` tramo 5,
/// task 2's own warning).
pub(crate) fn lanes_with_a_stack(a: &Tree) -> Vec<LaneFocus<'_>> {
    a.lanes
        .iter()
        .filter_map(|(id, s)| {
            let focus = a.node_by_num(*s.stack.last()?)?;
            Some(LaneFocus {
                id: id.as_str(),
                name: if s.name.is_empty() {
                    id.as_str()
                } else {
                    s.name.as_str()
                },
                focus,
                seq: s.seq_wrote,
            })
        })
        .collect()
}

/// Which lane wrote to this tree most recently, among the ones with
/// something on their own stack to name: the lane the web treats as
/// "the" focus once there is more than one to pick from (`t594` §5.6).
/// Sorted the same way `other_lanes` already sorts its own rows -- `seq`
/// descending, the lane id breaking a tie -- so the two agree on what
/// "most recent" means even though the log's own counter never actually
/// hands two different lanes the same `seq` to disagree over.
pub(crate) fn last_writer(a: &Tree) -> Option<LaneFocus<'_>> {
    let mut rows = lanes_with_a_stack(a);
    rows.sort_by(|x, y| y.seq.cmp(&x.seq).then_with(|| x.id.cmp(y.id)));
    rows.into_iter().next()
}

/// Which of this tree's lanes have a folder the registry no longer finds
/// on disk, checked with `exists()` right now and never written down
/// (`t594` §5.3/§5.5, decision 2 of this task): a disk that disconnects
/// and comes back changes the answer both ways, so this is read at the
/// moment of showing the list, never cached.
///
/// `None` when this tree's project id or the registry's own store
/// directory cannot be resolved -- nothing to check a folder against.
/// What that means to a caller differs by feature, so it is left to
/// decide: OTHER LANES treats it as "vouch for none of them", and
/// `stack --lanes` treats it as "mark none of them", because unlike
/// OTHER LANES, that list exists to be shown regardless.
pub(crate) fn gone_lane_ids(root: &Path) -> Option<Vec<String>> {
    let project_id = crate::store::first_event_id(root)?;
    let store_dir = crate::store::store_dir()?;
    Some(crate::registry::lanes_with_missing_folder(
        &store_dir,
        &project_id,
    ))
}

/// Every lane but this one that wrote after this lane's own last write,
/// has something on its own stack to name, and whose folder the registry
/// still finds on disk (`t594` §5.3, decisions 1, 2 and 4 of this task).
///
/// `exists()` runs at most once per lane the registry knows of, and only
/// this far: nothing reaches the registry until there is at least one
/// lane with a stack and a `seq_wrote` newer than this one's own
/// (`f623`).
fn other_lanes<'t>(a: &'t Tree, root: &Path) -> Vec<LaneFocus<'t>> {
    let here = a.lane();
    let own_seq = a.lanes.get(here).map(|s| s.seq_wrote).unwrap_or(0);
    let mut rows: Vec<LaneFocus> = lanes_with_a_stack(a)
        .into_iter()
        .filter(|r| r.id != here && r.seq > own_seq)
        .collect();
    if rows.is_empty() {
        return rows;
    }
    let Some(gone) = gone_lane_ids(root) else {
        // Nothing to check a folder against: a lane this cannot vouch for
        // as still there does not get shown as one that is.
        return Vec::new();
    };
    rows.retain(|r| !gone.iter().any(|g| g == r.id));
    rows.sort_by(|x, y| y.seq.cmp(&x.seq).then_with(|| x.id.cmp(y.id)));
    rows
}

const OTHER_LANES_TITLE: &str = "OTHER LANES since you last wrote here";

/// OTHER LANES's own rows: three spaces, the lane, three spaces, its
/// focus's alias and title, three spaces, the date that focus was opened
/// -- the same three-space separator BRANCH MOVED already writes with,
/// rather than a fixed-width table nothing in the spec asks for.
fn other_lanes_rows(a: &Tree, rows: &[LaneFocus]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            format!(
                "   {}   {}   {}   {}",
                r.name,
                r.focus.alias(),
                r.focus.title(a),
                r.focus.opened(a)
            )
        })
        .collect()
}

/// The trace OTHER LANES leaves when the budget trims its rows away
/// (`t594` §5.3): the heading stays, and one line says how many lanes
/// there were and where to read them in full. Falling silent would say
/// nothing happened here, and something did.
fn other_lanes_fallback(n: usize) -> Vec<String> {
    // One lane reaches this line as easily as several: the trace is only
    // shorter than the rows it replaces when a row is long, and a single
    // long row is the cheapest way to get here.
    let lanes = if n == 1 { "lane" } else { "lanes" };
    heading(
        OTHER_LANES_TITLE,
        vec![format!(
            "   {n} {lanes} wrote here since you did (vivac stack --lanes)"
        )],
    )
}

/// The brief as text. `session start --hook` prints it straight to stdout
/// (`f403`, `f404`): Claude Code turns plain-text stdout on `SessionStart`
/// into context the agent can see and act on, so there is nothing further to
/// wrap it in.
pub fn to_text(
    a: &Tree,
    root: &Path,
    lane_dir: &Path,
    anchor_of: &dyn Anchor,
    args: &Args,
    project: &str,
) -> Result<String, crate::failure::Failure> {
    let today = args.opt("now").unwrap_or("").to_string();
    let today = if today.is_empty() {
        crate::clock::now_rfc3339()
    } else {
        today
    };
    let date = crate::clock::date_of(&today).to_string();
    let budget: usize = args
        .opt("budget")
        .and_then(|s| s.parse().ok())
        .unwrap_or(BUDGET);

    let lineage: Vec<&Node> = match a.stack().last() {
        Some(&num) => a.ancestors(num),
        None => vec![],
    };
    let focus: Option<&Node> = lineage.last().copied();

    let mut s: Vec<Section> = Vec::new();

    // 0. The copy warning, ahead of everything else (`t594` §4.7): if this
    // folder is a copy, the lineage below may have diverged from whatever
    // this same first event looks like in the other folder, without either
    // side knowing. Fixed, like the header it sits in front of -- this is
    // the one section whose absence would make the rest of the brief a
    // silent lie.
    let block = copy_block(root);
    if !block.is_empty() {
        s.push(Section::fixed(block));
    }

    // 1. Header. 2. Spine, or -- with no focus -- the fixed block that takes
    // its place (`t533` §3.6). Neither is ever truncated. `lane_name` reads
    // `main` for the founding lane and its own declared name for any other,
    // so a tree with one lane prints the exact bytes it always has (`t594`
    // §5.1).
    s.push(Section::fixed(vec![
        format!(
            "vivac · project: {project} · lane: {} · {date}",
            a.lane_name()
        ),
        RULE.to_string(),
        String::new(),
    ]));
    // BRANCH MOVED (`t594` §5.2): right behind the header, so it is the
    // last thing the budget would ever reach. `Section::fixed` and never
    // truncated -- it is bounded by construction, one line per repository
    // moved, three candidates at most, one `to resume` (`t427`).
    let branch_moved = branch_moved_block(a, lane_dir);
    if !branch_moved.is_empty() {
        s.push(Section::fixed(branch_moved));
    }
    // `t429`'s second fix: repeated numbers are named, never hidden. One
    // line, bounded, and only when there are any.
    //
    // `repeated_nums` carries one entry per extra claimant, so a number
    // three nodes claim shows up twice: deduplicated here, in the order the
    // fold first met each one, so the five-wide cap counts distinct numbers
    // rather than claimants.
    if !a.repeated_nums.is_empty() {
        let mut seen = HashSet::new();
        let distinct_nums: Vec<u64> = a
            .repeated_nums
            .iter()
            .map(|d| d.num)
            .filter(|num| seen.insert(*num))
            .collect();
        let mut nums: Vec<String> = distinct_nums.iter().take(5).map(u64::to_string).collect();
        if distinct_nums.len() > 5 {
            nums.push(format!("+{}", distinct_nums.len() - 5));
        }
        s.push(Section::fixed(vec![
            format!(
                " REPEATED NUMBERS  {}  <- each names two nodes; vivac check",
                nums.join(", ")
            ),
            String::new(),
        ]));
    }
    s.push(Section::fixed(match focus {
        Some(_) => spine(a, &lineage),
        None => no_focus_block(a),
    }));

    // 3. Focus: what hangs off it unclosed. Standing decisions do not go in
    //    --they are not pending work and they have their own section (8)--,
    //    and whatever hangs further down is counted without being listed.
    //    Empty, and so omitted, with no focus to hang anything off.
    let born = focus.map(|f| born_from_here(a, f)).unwrap_or_default();
    s.push(Section::fixed(heading("BORN FROM HERE", born)));

    // 4. Invariants.
    let invariants: Vec<String> = constraints(a, &lineage)
        .iter()
        .map(|c| {
            let risk = if c.flags.is_empty() { "" } else { "   AT RISK" };
            format!("  {:<6} {}{risk}", c.alias(), c.title(a))
        })
        .collect();
    s.push(Section::fixed(heading("INVARIANTS", invariants)));

    // 5. Blocking questions: all of them, untruncated.
    let on_lineage: HashSet<u64> = lineage.iter().map(|n| n.num).collect();
    let questions: Vec<String> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Question && n.state.is_open() && n.blocks)
        .filter(|n| {
            a.ancestors(n.num)
                .iter()
                .any(|p| on_lineage.contains(&p.num))
        })
        .map(|n| format!("  {:<6} {}", n.alias(), n.title(a)))
        .collect();
    let mut questions = questions;
    questions.sort();
    s.push(Section::fixed(heading("BLOCKS", questions)));

    // 6. Flags on the path, or one hop off it.
    let mut flagged: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| !n.flags.is_empty())
        .filter(|n| {
            on_lineage.contains(&n.num) || n.parent.is_some_and(|p| on_lineage.contains(&p))
        })
        .collect();
    flagged.sort_by_key(|n| n.num);
    let flag_lines: Vec<String> = flagged
        .iter()
        .flat_map(|n| {
            n.flags.iter().map(move |(b, reason)| {
                format!(
                    "  {:<6} {:<10} {}",
                    n.alias(),
                    b.word(),
                    clip(a.text(*reason), 44)
                )
            })
        })
        .collect();
    s.push(Section::loose(heading(
        "FLAGGED",
        trim_list(flag_lines, 3, "stats"),
    )));

    // 7. Out of scope: every parked node of the project, regardless of the
    // focus (`d536`) -- so this section and `parked`'s own count agree
    // (`f60`). **This is the product's differentiator**, and it only has
    // content if `park` costs the same as `pop`.
    let mut parked_nodes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();
    parked_nodes.sort_by_key(|n| n.num);
    let out_of_scope: Vec<String> = parked_nodes
        .iter()
        .flat_map(|n| {
            let hangs_off = n
                .parent
                .and_then(|p| a.node_by_num(p))
                .map(|p| format!("hangs off {}", p.alias()))
                .unwrap_or_default();
            let mut v = vec![format!(
                "  {:<6} {:<40} {hangs_off}",
                n.alias(),
                clip(n.title(a), 40)
            )];
            let outcome = n.outcome(a);
            if !outcome.is_empty() {
                v.push(format!("         \"{}\"", clip(outcome, 56)));
            }
            v
        })
        .collect();
    s.push(Section::loose(heading(
        "DO NOT TOUCH NOW",
        trim_list(out_of_scope, 6, "parked"),
    )));

    // 8. Standing decisions: project-level, on the path, or with a `governs`
    // overlapping the focus's own. Superseded ones never appear. With no
    // focus, only the project-level ones reach it: there is neither a path
    // nor a `governs` of the focus's own to overlap.
    //
    // **Project-level had been missing**, and it is the case that matters
    // most: a decision that governs the whole product hangs off nothing, so
    // it was on no path and reached no brief. The invariants above had the
    // clause and the decisions did not, which was an asymmetry and not a
    // choice.
    let dec: Vec<&Node> = match focus {
        Some(f) => standing(a, f, &on_lineage),
        None => {
            let mut d: Vec<&Node> = a
                .nodes_iter()
                .filter(|n| n.kind == Kind::Decision && n.state.is_open() && project_wide(a, n))
                .collect();
            d.sort_by_key(|n| n.num);
            d
        }
    };
    let decisions: Vec<String> = dec
        .iter()
        .map(|n| format!("  {:<6} {}", n.alias(), clip(n.title(a), 52)))
        .collect();
    s.push(Section::loose(heading(
        "STANDING DECISIONS",
        trim_list(decisions, 3, "tree"),
    )));

    // 9. Last vivac. Restoring is always restore + diff: a vivac is never
    // presented without saying what changed since.
    let vv: Vec<String> = match a.last_vivac() {
        None => vec![],
        Some(v) => {
            let mut l = vec![format!(
                "  {} · {} · {}{}",
                v.alias(),
                v.kind.word(),
                crate::clock::date_of(&v.ts),
                // A single repository reads exactly as it always has --
                // the short sha of `anchor`, root or lone declared
                // repository alike. Only two or more declared repositories
                // change the line at all, and they collapse to a count
                // rather than picking one sha to stand for all of them
                // (`t594` task 4, §4.4).
                match crate::model::anchoring(&v.anchor, &v.anchors) {
                    Some(a) => format!(" · {a}"),
                    None => String::new(),
                }
            )];
            if !v.next_intent.is_empty() {
                l.push(format!(
                    "         you were about to: {}",
                    clip(&v.next_intent, 52)
                ));
            }
            // With no anchor no diff lines are invented: they are omitted, and
            // the date above stands in, which is the plain age there really is.
            if !v.anchor.is_empty_tree() {
                let changes = anchor_of.changed_since(&v.anchor);
                if !changes.is_empty() {
                    let touching = changes
                        .iter()
                        .filter(|c| {
                            v.working_set
                                .iter()
                                .any(|g| crate::glob::covers(g, &c.file_path))
                        })
                        .count();
                    l.push(format!(
                        "         {} changes since, {touching} touching what it governs",
                        changes.len()
                    ));
                }
            }
            l
        }
    };
    s.push(Section::loose(heading("LAST VIVAC", vv)));

    // 10. Freshness.
    let stale_ones: Vec<String> = lineage
        .iter()
        .filter(|n| n.flags.contains_key(&crate::event::Flag::Stale))
        .map(|n| format!("  {:<6} {}", n.alias(), n.title(a)))
        .collect();
    s.push(Section::loose(heading("UNTOUCHED FOR A WHILE", stale_ones)));

    // 11. OTHER LANES (`t594` §5.3): the last section of the brief, so the
    // budget trims it first (`emit`'s own search runs from the bottom).
    // Decided here, ahead of `emit`, rather than by that same generic
    // clearing: every other truncable section vanishes whole when the
    // budget will not have it, and this one is not allowed to -- falling
    // in silence would be worse than not being there at all.
    let other = other_lanes(a, root);
    if !other.is_empty() {
        let full = heading(OTHER_LANES_TITLE, other_lanes_rows(a, &other));
        let full_tokens: usize = full.iter().map(|l| tokens(l) + 1).sum();
        if tokens_of(&s) + full_tokens <= budget {
            s.push(Section::loose(full));
        } else {
            let short = other_lanes_fallback(other.len());
            let short_tokens: usize = short.iter().map(|l| tokens(l) + 1).sum();
            if tokens_of(&s) + short_tokens <= budget {
                s.push(Section::loose(short));
            }
        }
    }

    emit(s, budget, a)
}

/// Assembles under budget. It is a **soft ceiling**: truncatable sections are
/// dropped from the bottom up until it fits; if it still does not fit, it is
/// emitted anyway with a warning. Going over budget is a sign the tree needs
/// pruning, not that the brief should lie by silent omission.
fn emit(mut s: Vec<Section>, budget: usize, a: &Tree) -> Result<String, crate::failure::Failure> {
    let requested = tokens_of(&s);
    while tokens_of(&s) > budget {
        match s.iter().rposition(|x| x.truncable && !x.lines.is_empty()) {
            Some(i) => s[i].lines.clear(),
            None => break,
        }
    }
    let spent = tokens_of(&s);

    let mut o = String::new();
    for l in s.iter().flat_map(|x| x.lines.iter()) {
        o.push_str(l);
        o.push('\n');
    }
    let parked_nodes = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .count();
    o.push_str(&format!(
        "
{RULE}
 {spent} tokens · depth {} · {parked_nodes} parked
",
        a.stack_depth()
    ));
    if spent > budget {
        o.push_str(&format!(
            "
 ! the brief is over budget ({spent}/{budget}).
   The spine is never truncated: what is left over is tree, not render.
   What can be pruned:  vivac triage
"
        ));
    } else if requested > budget {
        o.push_str(&format!(
            "
 ! {} tokens trimmed to fit in {budget}.
",
            requested - spent
        ));
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trace OTHER LANES leaves behind is public prose, and one lane
    /// reaches it as easily as several (`tests/brief.rs`'s own budget test
    /// gets there with exactly one).
    #[test]
    fn the_trace_says_one_lane_and_never_one_lanes() {
        assert!(other_lanes_fallback(1)
            .iter()
            .any(|l| l.contains("1 lane wrote here since you did")));
        assert!(other_lanes_fallback(2)
            .iter()
            .any(|l| l.contains("2 lanes wrote here since you did")));
    }

    #[test]
    fn the_estimator_is_deterministic() {
        assert_eq!(tokens("same"), 1);
        assert_eq!(tokens("same tokens"), 3);
        assert_eq!(tokens(""), 0);
        // Same text, same number, always.
        assert_eq!(tokens("abcdefgh"), tokens("12345678"));
    }

    #[test]
    fn trimming_says_what_is_missing() {
        let v: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let r = trim_list(v, 3, "parked");
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], "l0");
        assert!(r[3].contains("7 more"), "{}", r[3]);
    }

    #[test]
    fn an_empty_section_leaves_no_heading() {
        assert!(heading("DO NOT TOUCH NOW", vec![]).is_empty());
        assert_eq!(heading("X", vec!["  a".into()]).len(), 3);
    }

    #[test]
    fn clipping_respects_words() {
        assert_eq!(clip("hello world", 20), "hello world");
        assert!(clip("a fairly long sentence that does not fit", 20).ends_with("..."));
        assert!(
            clip("a fairly long sentence that does not fit", 20)
                .chars()
                .count()
                <= 20
        );
    }
}
