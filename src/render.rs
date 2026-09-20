//! What the maintainer reads.
//!
//! All ASCII and not one colour escape. The DX pillar is explicit: **meaning
//! is never encoded in colour alone**, and this has to degrade without
//! breaking --with no tty, over ssh, and in cmd.exe as well as Windows
//! Terminal--. `[x]`, `[~]`, `*` and `<== FALSE CLOSE` read in black and
//! white. Colour, when it lands, reinforces; it does not inform.
//!
//! Every render has its `--json` twin, which is the other half of the
//! audience: the agent needs parseable output, not a drawn tree.

use crate::anchor::AnchorRef;
use crate::args::Args;
use crate::brief::clip;
use crate::event::{Body, Event, Kind, State, WhereRepo};
use crate::failure::{Failure, R};
use crate::model::{Aggregates, Node, Tree, Vivac, Where};
use crate::output::outln;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;

pub(crate) const WIDTH: usize = 62;

/// `d330`: how much of an ancestor's why/note/outcome survives in `why`
/// without `--full`. Matches `WIDTH` on purpose -- a clipped ancestor
/// collapses to roughly one wrapped line -- and it never applies to the
/// node actually asked about, which stays whole with or without `--full`.
const ANCESTOR_CLIP: usize = WIDTH;

pub(crate) fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for p in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + p.chars().count() > width {
            lines.push(format!("{indent}{cur}"));
            cur = p.to_string();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(p);
        }
    }
    if !cur.is_empty() {
        lines.push(format!("{indent}{cur}"));
    }
    lines
}

fn label(a: &Tree, n: &Node) -> String {
    match n.state {
        State::Active => n.title(a).to_string(),
        e => format!("{}  [{}]", n.title(a), e.word(n.kind)),
    }
}

fn json_node(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let r = ag.counts(n.num);
    let mut v = json!({
        "id": n.id,
        "alias": n.alias(),
        "num": n.num,
        "kind": n.kind,
        "title": n.title(a),
        "why": n.why(a),
        "state": n.state,
        "blocks": n.blocks,
        "parent": n.parent.and_then(|p| a.node_by_num(p).map(|x| x.alias())),
        "note": n.note(a),
        "notes": n.notes(a)
            .iter()
            .map(|(at, text)| json!({"at": at, "note": text}))
            .collect::<Vec<_>>(),
        "outcome": n.outcome(a),
        "refs": n.refs(a),
        "governs": n.governs(a),
        "opened": n.opened(a),
        "closed": n.closed(a),
        "false_close": n.state == State::Done && ag.blockers(n.num) > 0,
        "open_below": r.open_count,
        "total_below": r.total,
    });
    // `t411`: a rule gains `arms`, and no other kind gains anything -- the
    // JSON of every other kind stays byte for byte what it already was.
    // `arms` is **always** present on a rule, empty or not, so a reader can
    // tell "judged" apart from "not a rule" without a second lookup.
    if n.kind == Kind::Rule {
        v["arms"] = arms_json(a, n);
    }
    // `t426` §3.2: a decision gains `against` only when its `node.created`
    // carried the key, or a late declaration was folded into a birth that
    // never did. Everything else -- every other kind, and a decision with
    // neither -- stays byte for byte what it already was.
    if n.kind == Kind::Decision && (n.against_recorded || !n.against.is_empty()) {
        v["against"] = against_json(a, n);
    }
    v
}

pub(crate) fn print_json(v: serde_json::Value) -> R {
    outln!(
        "{}",
        serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?
    );
    Ok(())
}

/// `why` — why we are here. It is the operation that defines the product.
///
/// It narrates the path from the root and then answers the three questions
/// that come next: what was left open in parallel, what was born here, and
/// what keeps each step of the path from closing.
///
/// `--full` adds three more, per step of the path and on `node` itself: the
/// anchor in force when that step was born, the decisions born there that
/// still stand, and the siblings that were still open at that moment. The
/// first two answer from `node`'s own birth (`Node::born_seq`,
/// `Node::born_lane`); the third cannot, because closing is folded away to
/// the final state, and this is a question about a moment in the past.
/// `Full` answers it from the log directly.
///
/// Only the log gives a moment a `seq`: `ts` alone ties within the same day,
/// and this project has had days with 23 stops on it, so a comparison by
/// date would be wrong on exactly the days it matters.
pub(crate) struct Full {
    /// Node id -> every `state.changed` it ever had, in log order. A node can
    /// be reopened, so this is not "the one time it closed": it is the whole
    /// history, searched for whatever it was at a given `seq`. This is the
    /// one thing a folded `Tree` cannot answer on its own -- closing folds
    /// away to the final state -- so it is the only thing left here
    /// (`t594` tramo 7: `created` and `born_lane` moved onto `Node` itself).
    state: HashMap<String, Vec<(u64, State)>>,
}

impl Full {
    pub(crate) fn from_log(log: &[Event]) -> Full {
        let mut state: HashMap<String, Vec<(u64, State)>> = HashMap::new();
        for e in log {
            if let Body::StateChanged { node, state: s, .. } = &e.payload {
                state.entry(node.clone()).or_default().push((e.seq, *s));
            }
        }
        Full { state }
    }

    /// What a node's state was at `seq`, inclusive. With no `state.changed`
    /// at or before it, the node was still in the one it is born with.
    fn state_at(&self, id: &str, seq: u64) -> State {
        self.state
            .get(id)
            .into_iter()
            .flatten()
            .rfind(|(s, _)| *s <= seq)
            .map(|(_, state)| *state)
            .unwrap_or(State::Active)
    }
}

/// The anchor in force when `n` was born: the most recent stop at or before
/// the `seq` of its `node.created`, and its anchor. Empty with nothing
/// earlier to point to -- there is no version control, or the node predates
/// every stop -- and that is a value, not a failure.
///
/// **Deliberately not filtered by lane**, unlike `reconcile::reference`
/// (`last_vivac`, `t594` task 6): `reconcile` compares *this folder's own*
/// git against the anchor of a stop, so that stop has to be this lane's;
/// asking any other lane's would compare against a commit this checkout
/// may not even have. `anchor_of` answers a different question -- what
/// commit was in `HEAD` when `n` itself was born, a property of the node,
/// not of whoever is asking -- and a node born in a lane other than the
/// reader's stays answered from that lane's own history. The answer can
/// name a commit this checkout does not have; that is honest, since the
/// node was born somewhere else, not a bug to filter away.
///
/// [`born_where`] asks the same question with a branch attached, and falls
/// back to this answer for a tree with no `where.changed` of its own
/// (`t594` §5.4): that is what keeps every stop written before this tranche
/// reading exactly as it did.
pub(crate) fn anchor_of(a: &Tree, n: &Node) -> AnchorRef {
    a.vivacs
        .iter()
        .rfind(|v| v.seq <= n.born_seq)
        .map(|v| v.anchor.clone())
        .unwrap_or_default()
}

/// Where `n` was born: the last `where.changed` of **its own lane** at or
/// before the `seq` of its `node.created`. With none -- a tree from before
/// lanes, or a lane with no repositories -- the answer falls back to the
/// anchor of the last stop, which is what [`anchor_of`] has always given.
/// Same mechanism, one question deeper.
pub(crate) fn born_where<'a>(a: &'a Tree, n: &Node) -> Option<&'a Where> {
    let lane = n.born_lane(a);
    a.wheres
        .iter()
        .rfind(|w| w.lane == lane && w.seq <= n.born_seq)
}

/// The decisions born from `n` that still stand: a filter over what
/// `born_here` already lists, kept to the ones that are a decision and still
/// open. Superseding one closes it, so a superseded decision drops out on
/// its own.
pub(crate) fn standing_of<'a>(a: &'a Tree, n: &Node) -> Vec<&'a Node> {
    a.children(n.num)
        .into_iter()
        .filter(|c| c.kind == Kind::Decision && c.state.is_open())
        .collect()
}

/// What `n` is waiting on: its open blockers, and only while `n` is itself
/// open. A closed node with open blockers is not a debt, it is a false
/// close, and `tree` reports that in those words instead.
///
/// It lives here rather than in either caller because both `why` and the
/// lineage page draw it, and `WEB.md` §2 is the reason: a page picks no
/// nodes of its own. Two implementations of this filter could disagree
/// about a node's debts, and nothing would catch it (`f380`).
pub(crate) fn blocking_of<'a>(a: &'a Tree, n: &Node) -> Vec<&'a Node> {
    if n.state.is_open() {
        a.open_blockers(n.num)
    } else {
        Vec::new()
    }
}

/// The siblings of `n`, born before it by `Node::num`, that were still open
/// at the `seq` `n` was born. Not by `closed`'s date: two siblings can open
/// and close on the day `n` was born, in an order the date cannot tell
/// apart.
pub(crate) fn open_then_of<'a>(a: &'a Tree, full: &Full, n: &Node) -> Vec<&'a Node> {
    let Some(parent) = n.parent else {
        return vec![];
    };
    a.children(parent)
        .into_iter()
        .filter(|c| c.id != n.id && c.num < n.num)
        .filter(|c| full.state_at(&c.id, n.born_seq).is_open())
        .collect()
}

/// A handle: the four fields a reader needs to recognise a node and go ask
/// `why` about it, nothing more. `open_data` prints the same four (plus
/// `lineage`, which why has no use for: a path's siblings already share a
/// parent, and whatever is born here already hangs off the node itself).
/// `t465` put this everywhere `why`'s JSON used to hand back a whole
/// [`json_node`] for something the prose only ever names -- a sibling, a
/// child, a blocker, a standing decision.
fn handle_json(a: &Tree, n: &Node) -> serde_json::Value {
    json!({
        "alias": n.alias(),
        "kind": n.kind,
        "state": n.state,
        "title": n.title(a),
    })
}

/// Adds `lane` and `where` when [`born_where`] has an answer for `n` --
/// shared by [`json_node_full`] and [`path_step_json`]'s own `--full` half,
/// so the whole node and every step of the path gain the same two fields
/// the same way (`t594` §5.4). Absent, not `null`, with none: a tree with no
/// `where.changed` gains neither field, which is what keeps its JSON byte
/// for byte what it already was.
fn add_born_where(a: &Tree, n: &Node, v: &mut serde_json::Value) {
    if let Some(w) = born_where(a, n) {
        v["lane"] = json!(w.lane);
        v["where"] = json!(w.repos);
    }
}

/// `json_node`, with the three `--full` fields added -- `standing` and
/// `open_then` as handles now rather than whole nodes (`t465`): the prose
/// `print_full_of` prints only their aliases, and the JSON used to carry the
/// rest of each one for nothing.
fn json_node_full(a: &Tree, ag: &Aggregates, full: &Full, n: &Node) -> serde_json::Value {
    let mut v = json_node(a, ag, n);
    v["anchor"] = json!(anchor_of(a, n));
    v["standing"] = json!(standing_of(a, n)
        .iter()
        .map(|c| handle_json(a, c))
        .collect::<Vec<_>>());
    v["open_then"] = json!(open_then_of(a, full, n)
        .iter()
        .map(|c| handle_json(a, c))
        .collect::<Vec<_>>());
    add_born_where(a, n, &mut v);
    v
}

/// One step of `path`: an ancestor's handle plus the body the prose actually
/// reads out loud, clipped the same way and by the same `ANCESTOR_CLIP` the
/// text render of `why` uses, and whole under `--full`. Unlike a handle,
/// `notes` carries every one of them rather than only the latest, because
/// the prose does too -- but there is no `note` field here: it would only be
/// the last of `notes` again, and `f440` found most of a path's weight was
/// one note carried twice exactly that way.
///
/// `below` is `ag.counts`, the same three fields the prose folds into one
/// phrase after every step but the last: open, closed and parked, always all
/// three, because a zero here is an answer and not an absence.
///
/// What it deliberately drops: `id`, `num`, `blocks`, `parent`, `refs`,
/// `governs`, `opened`, `closed`, `false_close`, `total_below`. The prose
/// never prints any of them for an ancestor, and whoever wants them can ask
/// `why` about that alias directly.
///
/// Two fields depend on the kind of the step, and each follows the prose. A
/// rule carries `arms` with or without `--full`, because `why` prints a
/// rule's arms on every step of the path (`f549`). A decision carries
/// `against` only under `--full`, because that is the only time the prose
/// prints an ancestor's declarations (`d330`, `d469`). Both are built by
/// the same functions [`json_node`] uses, so a step and a node cannot read
/// either one differently.
///
/// `lane` and `where` answer from `p`'s own birth with or without `--full`
/// (`t594` §5.4); `full` only ever backs `open_then`, the one question a
/// folded `Tree` cannot answer on its own. `full_extra` is `--full` itself,
/// gating its own three fields -- `anchor`, `standing`, `open_then` -- and
/// whether a body prints whole or clipped.
fn path_step_json(
    a: &Tree,
    ag: &Aggregates,
    full: &Full,
    full_extra: bool,
    p: &Node,
) -> serde_json::Value {
    let body = |text: &str| {
        if full_extra {
            text.to_string()
        } else {
            clip(text, ANCESTOR_CLIP)
        }
    };
    let below = ag.counts(p.num);
    let mut v = json!({
        "alias": p.alias(),
        "kind": p.kind,
        "state": p.state,
        "title": p.title(a),
        "why": body(p.why(a)),
        "notes": p.notes(a)
            .iter()
            .map(|(at, text)| json!({"at": at, "note": body(text)}))
            .collect::<Vec<_>>(),
        "outcome": body(p.outcome(a)),
        "below": {
            "open": below.open_count,
            "closed": below.closed_count,
            "parked": below.parked_nodes,
        },
    });
    if p.kind == Kind::Rule {
        v["arms"] = arms_json(a, p);
    }
    if full_extra && p.kind == Kind::Decision && (p.against_recorded || !p.against.is_empty()) {
        v["against"] = against_json(a, p);
    }
    add_born_where(a, p, &mut v);
    if full_extra {
        v["anchor"] = json!(anchor_of(a, p));
        v["standing"] = json!(standing_of(a, p)
            .iter()
            .map(|c| handle_json(a, c))
            .collect::<Vec<_>>());
        v["open_then"] = json!(open_then_of(a, full, p)
            .iter()
            .map(|c| handle_json(a, c))
            .collect::<Vec<_>>());
    }
    v
}

/// `why` as data.
///
/// The builder and the printing are two functions, the way `brief.rs` has
/// always had them: `to_text` builds and `brief` prints one line lower. It
/// matters more than tidiness here, because a second reader --the MCP server--
/// speaks JSON-RPC over the same standard output. A `println!` in its path
/// does not look untidy, it corrupts the channel.
///
/// `full` is the whole log folded, always -- `t594` §5.4: `lane` and
/// `where` answer for the node in view whether or not `--full` was given,
/// the same as the prose. `full_extra` is `--full` itself, gating only its
/// own three fields: `anchor`, `standing`, `open_then`. `why_data` hands
/// this an empty [`Full`] and `full_extra: false` for a caller with no log
/// to give it, such as a foreign project's tree -- `lane` and `where` are
/// then absent too, since there is nothing to answer them from.
///
/// `node` is the one whole [`json_node`], the reason anybody asked. Every
/// other field the prose only ever names, so `t465` cut each down to match:
/// `path` to a clipped [`path_step_json`] per ancestor, `in_parallel` and
/// `born_here` to a [`handle_json`] each, and `blockers` to one entry per
/// path step -- node included -- naming what `blocking_of` says still keeps
/// it from closing. `f440` measured what the old shape cost on a real tree:
/// 86,894 bytes for one `why --json`, 89% of it `in_parallel` alone, against
/// 3,685 for the prose answering the same question. Over a copy of the same
/// tree, both numbers from the same harness, it is 7,139 now.
fn why_data_impl(
    a: &Tree,
    full: &Full,
    full_extra: bool,
    id: &str,
) -> Result<serde_json::Value, Failure> {
    let ag = &a.aggregates();
    // `id` can also name a stop, not only a node: `why` is the verb that
    // opens whatever an alias names, and the brief prints a stop's alias in
    // the same shape as a node's (`f547`). A stop that resolves stands on
    // its own, so it short-circuits here rather than falling through the
    // node-shaped body below.
    let n = match a.resolve(id) {
        Some(n) => n,
        None => {
            return a
                .vivac(id)
                .map(|v| vivac_json(a, v))
                .ok_or_else(|| Failure::usage(format!("No such node: {id}.")));
        }
    };
    let lineage = a.ancestors(n.num);
    let mut node_json = if full_extra {
        json_node_full(a, ag, full, n)
    } else {
        let mut v = json_node(a, ag, n);
        add_born_where(a, n, &mut v);
        v
    };
    // `t429`'s second fix: the JSON names the hidden claimants too, and
    // `t594` widens `hidden` to a list, since a hand-edited log can hand the
    // same `num` to more than two -- the same claimants the prose names, in
    // the same order.
    let hidden: Vec<&str> = a
        .repeated_nums
        .iter()
        .filter(|d| d.num == n.num)
        .map(|d| d.second.as_str())
        .collect();
    if !hidden.is_empty() {
        node_json["repeated"] = json!({"num": n.num, "hidden": hidden});
    }
    let siblings: Vec<_> = n
        .parent
        .map(|p| a.children(p))
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.id != n.id && c.state.is_open())
        .map(|c| handle_json(a, c))
        .collect();
    let born_here: Vec<_> = a
        .children(n.num)
        .iter()
        .filter(|c| c.state.is_open())
        .map(|c| {
            let mut v = handle_json(a, c);
            v["blocks"] = json!(c.blocks);
            v
        })
        .collect();
    // Every step of the path, node included -- the same walk the prose's own
    // "does not close until" loop makes -- kept to the ones `blocking_of`
    // answers non-empty. A closed step drops out on its own: it is a false
    // close, not a debt, and `tree` and `triage` report it as one instead.
    let blockers: Vec<_> = lineage
        .iter()
        .filter_map(|p| {
            let until = blocking_of(a, p);
            (!until.is_empty()).then(|| {
                json!({
                    "blocked": p.alias(),
                    "until": until.iter().map(|c| handle_json(a, c)).collect::<Vec<_>>(),
                })
            })
        })
        .collect();
    Ok(json!({
        "node": node_json,
        "path": lineage[..lineage.len().saturating_sub(1)]
            .iter()
            .map(|p| path_step_json(a, ag, full, full_extra, p))
            .collect::<Vec<_>>(),
        "in_parallel": siblings,
        "born_here": born_here,
        "blockers": blockers,
    }))
}

/// The plain read, over whatever log the caller has: the MCP tool's local
/// path folds its resident one (`t594` §5.4, `lane` and `where`); its
/// foreign-project path hands back `&[]`, the same as `why --project`
/// always has, since a foreign log is never read that way.
pub fn why_data(a: &Tree, log: &[Event], id: &str) -> Result<serde_json::Value, Failure> {
    why_data_impl(a, &Full::from_log(log), false, id)
}

/// A front, identified by its alias and where it hangs, not the node itself:
/// `why` on the alias brings the rest.
///
/// This used to be `json_node`'s eighteen keys plus `lineage`: the shape
/// `why` gives the one node it is asked about, and until `t465` gave every
/// other node it named as well. The prose `open` prints was already the
/// right shape -- alias, title, path -- and the data was not; `d172` made the
/// same fix for `find` first, down to dropping `matched`, which has no
/// analogue here because there is no query.
/// Measured over the same 10,000-node tree, both numbers from the same
/// harness: the MCP payload was 1,993,053 bytes and is now 599,012, 30% of
/// what it cost before.
pub fn open_data(a: &Tree) -> serde_json::Value {
    let ag = a.aggregates();
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(n.num).iter().any(|c| c.is_front()))
        .collect();
    // `sort_by_cached_key` and not `sort_by_key`: the second calls the key
    // function O(n log n) times, and this key goes to the aggregate map every
    // time it is called. It was measured, and the difference did not come up
    // out of the noise on this machine -- so it is here for being the right
    // primitive against a key that costs a lookup, not for a number.
    leaves.sort_by_cached_key(|n| {
        (
            !n.blocks,
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    json!(leaves
        .iter()
        .map(|n| json!({
            "alias": n.alias(),
            "kind": n.kind,
            "state": n.state,
            "title": n.title(a),
            "lineage": lineage_of(a, n),
        }))
        .collect::<Vec<_>>())
}

/// What a person reads for a lane that is not necessarily the one in view:
/// its own declared name when it has one, the lane's own id otherwise. The
/// same fallback `Tree::lane_name` uses for the lane currently in view,
/// generalised to any lane -- `born_where` can name one nobody is reading
/// from right now.
fn lane_display<'a>(a: &'a Tree, id: &'a str) -> &'a str {
    match a.lanes.get(id) {
        Some(s) if !s.name.is_empty() => s.name.as_str(),
        _ => id,
    }
}

/// One repository's piece of a "born in lane" line: `path@branch`, falling
/// back to the sha when there is no branch to name and to the bare path
/// when neither survived. A branch the redaction guard withheld reads with
/// the phrase `d600` and §2.4 give it.
fn describe_repo(r: &WhereRepo) -> String {
    if r.withheld {
        return format!("{} (branch name withheld: it looked like a secret)", r.path);
    }
    match (&r.branch, &r.sha) {
        (Some(b), _) => format!("{}@{b}", r.path),
        (None, Some(sha)) => format!("{}@{}", r.path, &sha[..sha.len().min(7)]),
        (None, None) => r.path.clone(),
    }
}

/// The one branch every repository shares, if there is one -- what collapses
/// several repositories into "N repos on `<branch>`" rather than naming each.
fn same_branch(repos: &[WhereRepo]) -> Option<&str> {
    let first = repos.first()?.branch.as_deref()?;
    repos
        .iter()
        .all(|r| r.branch.as_deref() == Some(first))
        .then_some(first)
}

/// How a lane's repositories read on one line: one repository names itself;
/// several on the same branch collapse to a count; several on different
/// branches name up to three and count the rest, the same truncation
/// `brief.rs` already uses for a long list. The three shapes `t594` §5.4
/// gives.
fn describe_repos(repos: &[WhereRepo]) -> String {
    if let [one] = repos {
        return describe_repo(one);
    }
    if let Some(branch) = same_branch(repos) {
        return format!("{} repos on {branch}", repos.len());
    }
    let mut pieces: Vec<String> = repos.iter().take(3).map(describe_repo).collect();
    if repos.len() > 3 {
        pieces.push(format!("and {} more", repos.len() - 3));
    }
    pieces.join(", ")
}

/// The label `d596` asks for: what was decided on a branch the reader's own
/// lane has since left behind is not hidden, only marked. Compared only
/// within the born lane's own history -- never against another lane's,
/// which needs a repository's identity matched across lanes and is what
/// `t594`'s own task 6 builds.
fn branch_moved_since_birth(a: &Tree, born: &Where) -> bool {
    if born.lane != a.lane() {
        return false;
    }
    let Some(latest) = a.wheres.iter().rfind(|w| w.lane == born.lane) else {
        return false;
    };
    born.repos.iter().any(|b| {
        latest
            .repos
            .iter()
            .find(|l| l.path == b.path)
            .is_some_and(|l| l.branch != b.branch)
    })
}

/// The "born in lane" line §5.4 adds ahead of `anchor:` below, or nothing at
/// all for a tree with no `where.changed` -- `anchor_of` alone answers
/// those, exactly as it always has.
fn born_line(a: &Tree, n: &Node) -> Option<String> {
    let w = born_where(a, n)?;
    let mut line = format!(
        "born in lane {} · {}",
        lane_display(a, &w.lane),
        describe_repos(&w.repos)
    );
    if branch_moved_since_birth(a, w) {
        line.push_str(" (not the branch you are on)");
    }
    Some(line)
}

/// The `--full` lines for one step of the path, printed the way the JSON
/// twin carries the same three fields: the anchor, the decisions still
/// standing, and the siblings still open at that moment. The "born in lane"
/// line is not one of them: `t594` §5.4 has it print with or without
/// `--full`, so the caller prints it on its own, ahead of these.
fn print_full_of(a: &Tree, full: &Full, n: &Node) {
    let anchor = anchor_of(a, n);
    if anchor.is_empty_tree() {
        outln!("        anchor: none");
    } else {
        outln!("        anchor: {} ({})", anchor.short(), anchor.kind);
    }
    let standing = standing_of(a, n);
    if !standing.is_empty() {
        outln!(
            "        standing ({}): {}",
            standing.len(),
            standing
                .iter()
                .map(|d| d.alias())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let open_then = open_then_of(a, full, n);
    if !open_then.is_empty() {
        outln!(
            "        open then ({}): {}",
            open_then.len(),
            open_then
                .iter()
                .map(|d| d.alias())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// `why` on a stop's own alias: the whole stop, prose-shaped the way `why`
/// shapes a node -- same header, same rule below it, `Safe stop` where a
/// node says `Why we are here` (`f547`, `d651`). Unlike the brief's own
/// `next_intent`, nothing here is clipped: the clip is exactly what sends a
/// reader here to begin with.
fn safe_stop(a: &Tree, v: &Vivac, args: &Args) -> R {
    if args.has("json") {
        return print_json(vivac_json(a, v));
    }
    outln!();
    outln!("  Safe stop  ->  {}", v.alias());
    outln!("  {}", "-".repeat(66));
    outln!();
    let mut header = format!(
        "  {} · {} · {}",
        v.alias(),
        v.kind.word(),
        crate::clock::date_of(&v.ts)
    );
    if let Some(anchor) = crate::model::anchoring(&v.anchor, &v.anchors) {
        header.push_str(&format!(" · {anchor}"));
    }
    outln!("{header}");
    if let Some(node) = v.node_ref.as_deref().and_then(|r| a.node(r)) {
        outln!("         written at {:<6}{}", node.alias(), node.title(a));
    }
    if !v.label.is_empty() {
        outln!("         \"{}\"", v.label);
    }
    if !v.next_intent.is_empty() {
        outln!("         you were about to: {}", v.next_intent);
    }
    outln!();
    outln!("  The stack it carried");
    if v.stack.is_empty() {
        outln!("    empty stack");
    } else {
        for (alias, title) in &v.stack {
            outln!("    {:<6} {}", alias, title);
        }
    }
    if !v.working_set.is_empty() {
        outln!();
        outln!("  Working set");
        for w in &v.working_set {
            outln!("    {w}");
        }
    }
    outln!();
    outln!("  vivac restore {}  rebuilds this stack", v.alias());
    outln!();
    Ok(())
}

pub fn why(a: &Tree, log: &[Event], args: &Args) -> R {
    let ag = &a.aggregates();
    let s = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac why <id>"))?;
    // `id` can also name a stop the brief printed (`f547`): `why` is the
    // verb that opens whatever an alias names, and a stop's alias reads the
    // same shape as a node's. A stop wins whenever a node does not resolve;
    // it never shares a prefix with any `Kind`, so the two cannot collide.
    let n = match a.resolve(s) {
        Some(n) => n,
        None => {
            return match a.vivac(s) {
                Some(v) => safe_stop(a, v, args),
                None => Err(Failure::usage(format!("No such node: {s}."))),
            };
        }
    };
    let lineage = a.ancestors(n.num);
    // `t594` §5.4: "born in lane" answers for the node in view whether or
    // not `--full` was given, straight off its own birth -- `log` is only
    // for `Full`'s own `state`, which nothing here touches unless
    // `full_extra` asks `open_then_of` for it, so `log` is empty exactly
    // when the caller had no reason to read past the derived index
    // (`t594` tramo 7). `full_extra` is `--full` itself, gating only its
    // own three fields per step.
    let full_data = Full::from_log(log);
    let full_extra = args.has("full");

    if args.has("json") {
        return print_json(why_data_impl(a, &full_data, full_extra, s)?);
    }

    outln!();
    outln!("  Why we are here  ->  {}", n.alias());
    outln!("  {}", "-".repeat(66));
    // A hand-edited log can hand the same `num` to more than two claimants;
    // every one but the first is hidden the same way, so all of them are
    // named here, not just whichever the fold met second.
    let hidden: Vec<&str> = a
        .repeated_nums
        .iter()
        .filter(|d| d.num == n.num)
        .map(|d| d.second.as_str())
        .collect();
    if !hidden.is_empty() {
        let (noun, pronoun) = if hidden.len() == 1 {
            ("another node", "it")
        } else {
            ("other nodes", "them")
        };
        outln!(
            "  {} also names {noun}, {}, which this tree cannot show. vivac check lists {pronoun}.",
            n.alias(),
            hidden.join(", ")
        );
    }
    outln!();
    for (i, p) in lineage.iter().enumerate() {
        let is_last = i == lineage.len() - 1;
        // The node actually asked about prints whole either way; an
        // ancestor's body only survives whole under `--full`.
        let clip_body = !is_last && !full_extra;
        let body = |text: &str| {
            if clip_body {
                clip(text, ANCESTOR_CLIP)
            } else {
                text.to_string()
            }
        };
        outln!("  {:<6}{}", p.alias(), label(a, p));
        // `t411` §6: a rule shows its arms, in the same words `rules` prints
        // them with.
        if p.kind == Kind::Rule {
            print_arms(a, p, "        ", true);
        }
        // `t426` §3.1: a decision shows its declarations right where a rule
        // shows its arms -- behind the alias line, ahead of the body.
        // `d330`'s own rule: they show for the node actually asked about,
        // and for an ancestor only under `--full`.
        if p.kind == Kind::Decision && (is_last || full_extra) {
            print_against(a, p, "        ");
        }
        for l in wrap(&body(p.why(a)), WIDTH, "        ") {
            outln!("{l}");
        }
        let notes = p.notes(a);
        if notes.len() > 1 {
            // Two or more: each one gets its own line and its own date, or
            // there would be no way to tell which correction landed when.
            // With exactly one, a date says nothing a lone note does not
            // already say by being there -- `f186`'s own argument for
            // dropping the lineage's empty anchor.
            for (at, text) in &notes {
                let date = crate::clock::date_of(at);
                for l in wrap(&format!("! [{date}] {}", body(text)), WIDTH, "        ") {
                    outln!("{l}");
                }
            }
        } else {
            let note = p.note(a);
            for l in wrap(&format!("! {}", body(note)), WIDTH, "        ") {
                if !note.is_empty() {
                    outln!("{l}");
                }
            }
        }
        let outcome = p.outcome(a);
        for l in wrap(&format!("= {}", body(outcome)), WIDTH, "        ") {
            if !outcome.is_empty() {
                outln!("{l}");
            }
        }
        // `t594` §5.4: prints with or without `--full`, unlike the rest of
        // `print_full_of` below it.
        if let Some(line) = born_line(a, p) {
            outln!("        {line}");
        }
        if full_extra {
            print_full_of(a, &full_data, p);
        }
        if !is_last {
            let f = ag.counts(p.num).phrase();
            if !f.is_empty() {
                outln!("        ({f} below)");
            }
            outln!("        |");
            outln!("        v");
        } else {
            outln!();
            outln!("        ^^^ you are here");
        }
    }
    outln!();

    // "we had ten things to review, we are on the first"
    if let Some(parent) = n.parent {
        let siblings: Vec<_> = a
            .children(parent)
            .into_iter()
            .filter(|c| c.id != n.id && c.state.is_open())
            .collect();
        if !siblings.is_empty() {
            outln!("  In parallel, still open ({}):", siblings.len());
            for c in siblings {
                outln!("      {:<6} {}", c.alias(), c.title(a));
            }
            outln!();
        }
    }

    let kids: Vec<_> = a
        .children(n.num)
        .into_iter()
        .filter(|c| c.state.is_open())
        .collect();
    if !kids.is_empty() {
        outln!("  Born here and still open ({}):", kids.len());
        for c in kids {
            outln!(
                "    {} {:<6} {}",
                if c.blocks { '*' } else { ' ' },
                c.alias(),
                c.title(a)
            );
        }
        outln!();
    }

    for p in &lineage {
        let pending_count = blocking_of(a, p);
        if !pending_count.is_empty() {
            outln!(
                "  {} does not close until these close ({}):",
                p.alias(),
                pending_count.len()
            );
            for c in pending_count {
                outln!("      {:<6} {}", c.alias(), c.title(a));
            }
            outln!();
        }
    }
    Ok(())
}

fn branch(a: &Tree, ag: &Aggregates, n: &Node, prefix: &str, is_last: bool, show_all: bool) {
    let f = ag.counts(n.num).phrase();
    let mut tail = if f.is_empty() {
        String::new()
    } else {
        format!("   ({f})")
    };
    let pending_count = ag.blockers(n.num);
    if n.state == State::Done && pending_count > 0 {
        tail.push_str(&format!(
            "   <== FALSE CLOSE: {pending_count} open condition(s)"
        ));
    }
    let mark = if n.blocks { "* " } else { "" };
    outln!(
        "{prefix}{}[{}] {:<6} {mark}{}{tail}",
        if is_last { "`-- " } else { "|-- " },
        n.state.mark(),
        n.alias(),
        n.title(a)
    );
    let sig = format!("{prefix}{}", if is_last { "    " } else { "|   " });
    let children: Vec<_> = a
        .children(n.num)
        .into_iter()
        .filter(|h| show_all || h.state.is_open() || ag.counts(h.num).open_count > 0)
        .collect();
    for (i, h) in children.iter().enumerate() {
        branch(a, ag, h, &sig, i == children.len() - 1, show_all);
    }
}

/// `t429`'s second fix, by `d423`'s rule: a number two nodes share is
/// something this tree can only show one half of, so it says which half.
fn repeated_lines(a: &Tree) -> Vec<String> {
    a.repeated_nums
        .iter()
        .map(|d| {
            format!(
                "  {} is repeated: {} is shown and {} is not. vivac check lists every one.",
                d.num, d.first, d.second
            )
        })
        .collect()
}

fn subtree_json(a: &Tree, ag: &Aggregates, n: &Node) -> serde_json::Value {
    let mut v = json_node(a, ag, n);
    v["children"] = json!(a
        .children(n.num)
        .iter()
        .map(|h| subtree_json(a, ag, h))
        .collect::<Vec<_>>());
    v
}

pub fn tree(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let roots: Vec<&Node> = match args.positional(0) {
        Some(s) => vec![a
            .resolve(s)
            .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))?],
        None => a.roots(),
    };
    if args.has("json") {
        return print_json(json!(roots
            .iter()
            .map(|n| subtree_json(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if a.is_empty_tree() {
        outln!("  Empty tree.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    let show_all = args.has("all");
    outln!();
    for (i, n) in roots.iter().enumerate() {
        branch(a, ag, n, "  ", i == roots.len() - 1, show_all);
    }
    outln!();
    let repeated = repeated_lines(a);
    if !repeated.is_empty() {
        for l in &repeated {
            outln!("{l}");
        }
        outln!();
    }
    if !show_all {
        outln!("  (closed nodes with no open descendants hidden; --all shows them)");
        outln!();
    }
    Ok(())
}

/// Fronts printed before the list gives way to the tail line. Each front
/// costs two lines, so ten of them plus the header and the tail still fit
/// one screen with nothing to scroll -- and a list that has to scroll
/// already broke the promise of "right now".
const MAX_FRONTS_SHOWN: usize = 10;

/// `open` — what is waiting for you right now, and what has been open so
/// long you are not actually working it any more (`d383`). The order below
/// is deduced from that sentence, not chosen and explained after: a
/// blocker sorts first, because a blocker is exactly something waiting on
/// you; among the rest, whichever holds up more tree; at a tie, the newest.
pub fn open(a: &Tree, args: &Args) -> R {
    let ag = a.aggregates();
    let mut leaves: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.is_front() && !a.children(n.num).iter().any(|c| c.is_front()))
        .collect();
    // `sort_by_cached_key` and not `sort_by_key`: the second calls the key
    // function O(n log n) times, and this key goes to the aggregate map every
    // time it is called. It was measured, and the difference did not come up
    // out of the noise on this machine -- so it is here for being the right
    // primitive against a key that costs a lookup, not for a number.
    leaves.sort_by_cached_key(|n| {
        (
            !n.blocks,
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    let standing = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Decision && n.state.is_open())
        .count();
    if args.has("json") {
        return print_json(open_data(a));
    }
    if leaves.is_empty() && standing == 0 {
        outln!("  Nothing open.");
        return Ok(());
    }
    outln!();
    outln!(
        "  {} open front{}",
        leaves.len(),
        if leaves.len() == 1 { "" } else { "s" },
    );
    outln!();
    let show_all = args.has("all");
    let shown = if show_all {
        leaves.len()
    } else {
        leaves.len().min(MAX_FRONTS_SHOWN)
    };
    for n in &leaves[..shown] {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        let lineage = a.ancestors(n.num);
        if lineage.len() > 1 {
            let v: Vec<String> = lineage[..lineage.len() - 1]
                .iter()
                .map(|p| p.alias())
                .collect();
            outln!("         via {}", v.join(" > "));
        }
    }
    let hidden = leaves.len() - shown;
    if hidden > 0 {
        // The oldest of the ones left out, never of the whole set: a front
        // that made the cut is being worked, and its age is not the gap
        // `--all` closes.
        let oldest = leaves[shown..].iter().map(|n| n.opened(a)).min();
        let age = oldest.and_then(|d| crate::clock::days_between(d, &crate::clock::now_rfc3339()));
        // The same three arms the project index uses for a project that has
        // not moved. A count of days is the wrong shape at zero and at one,
        // and "open for 0 days" is a sentence nobody says.
        match age {
            Some(d) if d <= 0 => {
                outln!("  {hidden} more, the oldest opened today -- vivac open --all")
            }
            Some(1) => {
                outln!("  {hidden} more, the oldest open since yesterday -- vivac open --all")
            }
            Some(days) => {
                outln!("  {hidden} more, the oldest open for {days} days -- vivac open --all")
            }
            None => outln!("  {hidden} more -- vivac open --all"),
        }
    }
    // They are not fronts, but making them vanish without saying so would be
    // omitting in silence: they get counted and located.
    if standing > 0 {
        let phrase = if standing == 1 {
            "1 standing decision, which is not work".to_string()
        } else {
            format!("{standing} standing decisions, which are not work")
        };
        outln!();
        outln!("  + {phrase}   vivac brief");
    }
    outln!();
    Ok(())
}

/// The nearest ancestor of kind `Pillar`, climbing one parent at a time and
/// stopping at the first match. `None` when nothing above `n` is a pillar --
/// the climb still ends, at the root.
///
/// `rules`'s own performance budget (§5): a pass over the nodes plus this
/// climb for each rule, never a walk of the whole tree per rule.
fn nearest_pillar<'a>(a: &'a Tree, n: &Node) -> Option<&'a Node> {
    let mut cur = n.parent;
    while let Some(p) = cur {
        let node = a.node_by_num(p)?;
        if node.kind == Kind::Pillar {
            return Some(node);
        }
        cur = node.parent;
    }
    None
}

/// One pillar with the open rules that answer to it.
struct PillarSection<'a> {
    pillar: &'a Node,
    rules: Vec<&'a Node>,
}

/// The pull's own shape (`t411` §5): every pillar that still governs --
/// open, or closed with an open rule still hanging off it -- each with its
/// own open rules; the open rules that answer to no pillar; and every open
/// invariant. Built in one pass over the nodes plus the parent climb of
/// each rule, never a second walk of the tree.
struct RulesView<'a> {
    pillars: Vec<PillarSection<'a>>,
    orphan_rules: Vec<&'a Node>,
    invariants: Vec<&'a Node>,
}

fn rules_view(a: &Tree) -> RulesView<'_> {
    let mut under: HashMap<u64, Vec<&Node>> = HashMap::new();
    let mut orphan_rules: Vec<&Node> = Vec::new();
    for n in a.nodes_iter() {
        if n.kind == Kind::Rule && n.state.is_open() {
            match nearest_pillar(a, n) {
                Some(p) => under.entry(p.num).or_default().push(n),
                None => orphan_rules.push(n),
            }
        }
    }
    for v in under.values_mut() {
        v.sort_by_key(|n| n.num);
    }
    orphan_rules.sort_by_key(|n| n.num);

    let mut pillars: Vec<PillarSection> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Pillar)
        .filter(|n| n.state.is_open() || under.get(&n.num).is_some_and(|v| !v.is_empty()))
        .map(|n| PillarSection {
            pillar: n,
            rules: under.get(&n.num).cloned().unwrap_or_default(),
        })
        .collect();
    pillars.sort_by_key(|s| s.pillar.num);

    let mut invariants: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Constraint && n.state.is_open())
        .collect();
    invariants.sort_by_key(|n| n.num);

    RulesView {
        pillars,
        orphan_rules,
        invariants,
    }
}

/// `rules --json` and `vivac_rules`'s own payload: the same builder, so the
/// two can never drift apart.
pub fn rules_data(a: &Tree) -> serde_json::Value {
    let ag = &a.aggregates();
    let view = rules_view(a);
    json!({
        "pillars": view.pillars.iter().map(|s| {
            let mut v = json_node(a, ag, s.pillar);
            v["rules"] = json!(s.rules.iter().map(|r| json_node(a, ag, r)).collect::<Vec<_>>());
            v
        }).collect::<Vec<_>>(),
        "rules": view.orphan_rules.iter().map(|r| json_node(a, ag, r)).collect::<Vec<_>>(),
        "invariants": view.invariants.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
    })
}

/// A rule's own arms, one per line. `indent` is whatever column the rule's
/// own title started at, so the line under it lines up. `show_judged`
/// prints `judged: no command verifies it` for a rule with none; `rules`
/// (`d421`) passes `false`, because there the line only repeats what the
/// rule's absent `armed:` lines already say by not being there, and `why`
/// (`t411` §6) passes `true`, because there it is the only line and it does
/// inform.
fn print_arms(a: &Tree, r: &Node, indent: &str, show_judged: bool) {
    let arms = r.arms(a);
    if arms.is_empty() {
        if show_judged {
            outln!("{indent}judged: no command verifies it");
        }
    } else {
        for (dir, command) in arms {
            outln!("{indent}armed in {dir}/: {command}");
        }
    }
}

/// The JSON for a rule's arms: the folder and the command of each one, in
/// the same order [`print_arms`] prints them, present even when empty so a
/// reader can tell a judged rule apart from a step that is not a rule at
/// all. Shared by [`json_node`] and [`path_step_json`] so a rule's arms read
/// the same value wherever `why` carries them (`f549`).
fn arms_json(a: &Tree, r: &Node) -> serde_json::Value {
    json!(r
        .arms(a)
        .into_iter()
        .map(|(dir, command)| json!({"dir": dir, "command": command}))
        .collect::<Vec<_>>())
}

/// A decision's own declarations, one per line, wrapped the same way its
/// body is: `judged against <alias>: <why>`, with `(declared <date>)`
/// appended for a late one. `t426` §3.1.
///
/// A pillar or rule that is no longer open is marked right after its alias
/// with the word `label()` puts behind a title, `[abandoned]` or `[closed]`,
/// so a reader does not have to go and look whether what was named still
/// governs (`d551`). An open one, and a dangling reference, carry no mark.
fn print_against(a: &Tree, n: &Node, indent: &str) {
    for e in n.against(a) {
        let mark = match e.target {
            Some((kind, state)) if !state.is_open() => format!(" [{}]", state.word(kind)),
            _ => String::new(),
        };
        let suffix = match e.declared {
            Some(date) => format!("  (declared {date})"),
            None => String::new(),
        };
        let line = format!("judged against {}{mark}: {}{suffix}", e.alias, e.why);
        for l in wrap(&line, WIDTH, indent) {
            outln!("{l}");
        }
    }
}

/// A decision's declarations as JSON, one entry for each one
/// [`print_against`] prints and in the same order. `state` is there on every
/// entry, open or not, serialized the way a node's own `state` is and `null`
/// for a dangling reference: what the data carries cannot depend on what the
/// prose leaves unsaid (`d551`). Shared by [`json_node`] and
/// [`path_step_json`], so the two cannot read a declaration differently.
fn against_json(a: &Tree, n: &Node) -> serde_json::Value {
    json!(n
        .against(a)
        .into_iter()
        .map(|e| json!({
            "node": e.alias,
            "state": e.target.map(|(_, state)| state),
            "why": e.why,
            "declared": e.declared,
        }))
        .collect::<Vec<_>>())
}

/// `d422`: nobody hunting for what governs this project should have to
/// guess that a second, unread map exists. Printed once, after whichever of
/// `rules`'s two shapes just ran, and only when there was no open pillar and
/// no open rule for it to find.
fn print_second_map_hint() {
    outln!("  Rules kept in CLAUDE.md, AGENTS.md or a memory file are a second map, and");
    outln!("  vivac never reads them: bring them in with vivac add --type pillar|rule.");
}

/// `rules` — the pull: everything that governs this project, read whether
/// or not the push ever carried it into a brief. `t411` §5.
pub fn rules(a: &Tree, args: &Args) -> R {
    if args.has("json") {
        return print_json(rules_data(a));
    }
    let view = rules_view(a);
    let total_rules: usize =
        view.pillars.iter().map(|s| s.rules.len()).sum::<usize>() + view.orphan_rules.len();
    let armed_rules = view
        .pillars
        .iter()
        .flat_map(|s| &s.rules)
        .chain(&view.orphan_rules)
        .filter(|r| !r.arms.is_empty())
        .count();
    let judged_rules = total_rules - armed_rules;
    // `d422`: true whenever there is no open pillar and no open rule for
    // this read to find, whether or not an invariant is still around.
    let nothing_governs = view.pillars.is_empty() && total_rules == 0;

    if nothing_governs && view.invariants.is_empty() {
        outln!("  Nothing governs this project yet: no pillars, rules or invariants.");
        outln!();
        print_second_map_hint();
        return Ok(());
    }

    outln!();
    if !view.pillars.is_empty() {
        outln!("  PILLARS");
        for s in &view.pillars {
            outln!("  {:<6}{}", s.pillar.alias(), label(a, s.pillar));
            for r in &s.rules {
                outln!("    {:<6}{}", r.alias(), r.title(a));
                print_arms(a, r, "          ", false);
            }
        }
    }
    if !view.orphan_rules.is_empty() {
        outln!();
        outln!("  RULES WITHOUT A PILLAR");
        for r in &view.orphan_rules {
            outln!("  {:<6}{}", r.alias(), r.title(a));
            print_arms(a, r, "        ", false);
        }
    }
    if !view.invariants.is_empty() {
        outln!();
        outln!("  INVARIANTS");
        for n in &view.invariants {
            outln!("  {:<6}{}", n.alias(), n.title(a));
        }
    }
    outln!();
    outln!(
        "  {} pillar{} \u{b7} {} rule{}: {} armed, {} judged \u{b7} {} invariant{}",
        view.pillars.len(),
        if view.pillars.len() == 1 { "" } else { "s" },
        total_rules,
        if total_rules == 1 { "" } else { "s" },
        armed_rules,
        judged_rules,
        view.invariants.len(),
        if view.invariants.len() == 1 { "" } else { "s" },
    );
    outln!();
    if nothing_governs {
        print_second_map_hint();
    }
    Ok(())
}

/// `triage` — what can be pruned, and with which command.
///
/// A brief over budget **must not lie by omission** (`BRIEF-SPEC.md` §4):
/// the signal is that the graph needs pruning, and this is the view that says
/// where. `MODEL.md` §6.1 also sends it the deep nodes, because a chain that
/// long is almost never lack of discipline: it is that the goal moved and
/// nobody re-rooted.
pub fn triage(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();

    let mut parked_nodes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();

    // `MODEL.md` §6.1: from 6 on it shows up here, and it never blocks. The
    // distance is to the goal the node answers to, not to the root: `promote`
    // is the way out this section prints, and a count from the root is one
    // `promote` cannot move (`f156`).
    let mut deep: Vec<(&Node, usize)> = a
        .nodes_iter()
        .filter(|n| n.is_front())
        .map(|n| (n, a.under_goal(n.num).len()))
        .filter(|(_, d)| *d >= 6)
        .collect();

    // Alive, hanging off something discarded. `abandon`'s rescue produces
    // them, and it does **not** reparent on purpose (`d33`): the node stays
    // where it was born. That is why they need revisiting now and then, and
    // why they are here and not in `check`: it is not store corruption, it is
    // work that lost the reason it was born for.
    let mut orphaned: Vec<(&Node, &Node)> = a
        .nodes_iter()
        .filter(|n| n.is_front())
        .filter_map(|n| {
            let p = a.node_by_num(n.parent?)?;
            (p.state == State::Abandoned).then_some((n, p))
        })
        .collect();

    // Invariant 10. `check` reports them for CI; here they get acted on, and
    // with the same exemption: a **forced** close was a decision, it has its
    // trace and the tree marks it. Repeating it here every day would be asking
    // for what was already decided to be decided again. What does land here is
    // the close that turned false later, when a blocker got hung on something
    // already closed: that is the case that took 26 days to spot.
    let mut false_closes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Done && !n.forced_close && ag.blockers(n.num) > 0)
        .collect();

    parked_nodes.sort_by_key(|n| n.num);
    deep.sort_by_key(|(n, _)| n.num);
    orphaned.sort_by_key(|(n, _)| n.num);
    false_closes.sort_by_key(|n| n.num);

    if args.has("json") {
        return print_json(json!({
            "parked": parked_nodes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
            "deep": deep.iter().map(|(n, d)| {
                let mut v = json_node(a, ag, n);
                // Named for what it counts. `stats` reports a `depth` measured
                // from the root, and one key meaning two distances would be
                // read wrong exactly once.
                v["depth_from_goal"] = json!(d);
                v
            }).collect::<Vec<_>>(),
            "orphaned_by_discard": orphaned.iter().map(|(n, p)| {
                let mut v = json_node(a, ag, n);
                v["discarded"] = json!(p.alias());
                v["discarded_because"] = json!(p.outcome(a));
                v
            }).collect::<Vec<_>>(),
            "false_closes": false_closes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }

    let total = parked_nodes.len() + deep.len() + orphaned.len() + false_closes.len();
    if total == 0 {
        outln!("  Nothing to prune.");
        return Ok(());
    }
    outln!();
    outln!("  TRIAGE - {total} thing(s) to look at");

    if !parked_nodes.is_empty() {
        outln!();
        outln!(
            "  PARKED ({})                       focus <id>  |  abandon <id>",
            parked_nodes.len()
        );
        for n in &parked_nodes {
            outln!("    {:<6} {}", n.alias(), n.title(a));
            for l in wrap(n.outcome(a), WIDTH, "           ") {
                outln!("{l}");
            }
        }
    }

    if !deep.is_empty() {
        outln!();
        outln!(
            "  6 OR MORE FROM ITS GOAL ({})      promote <id>",
            deep.len()
        );
        for (n, d) in &deep {
            outln!(
                "    {:<6} {:<40} depth {d}",
                n.alias(),
                clip(n.title(a), 40)
            );
            // The lineage starts where the number does. Drawing it from the
            // root beside a distance to the goal would say two things at once.
            let v: Vec<String> = a
                .under_goal(n.num)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .map(|p| p.alias())
                .collect();
            outln!("           via {}", v.join(" > "));
        }
    }

    if !orphaned.is_empty() {
        outln!();
        outln!(
            "  SURVIVED A DISCARD ({})           abandon <id>  |  promote <id>",
            orphaned.len()
        );
        for (n, p) in &orphaned {
            outln!("    {:<6} {}", n.alias(), n.title(a));
            outln!(
                "           born from {}, discarded: {}",
                p.alias(),
                clip(p.outcome(a), 36)
            );
        }
    }

    if !false_closes.is_empty() {
        outln!();
        outln!(
            "  FALSE CLOSES ({})                 close what is left, or --force",
            false_closes.len()
        );
        for n in &false_closes {
            outln!(
                "    {:<6} {:<40} {} blocker(s)",
                n.alias(),
                clip(n.title(a), 40),
                ag.blockers(n.num)
            );
        }
    }
    outln!();
    Ok(())
}

/// `parked` — DO NOT TOUCH NOW. It is the section no other tool emits: every
/// memory tool dumps what is relevant, and the problem in agentic development
/// is the opposite one, bounding.
pub fn parked(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let mut ps: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();
    ps.sort_by_key(|n| n.num);
    if args.has("json") {
        return print_json(json!(ps
            .iter()
            .map(|n| json_node(a, ag, n))
            .collect::<Vec<_>>()));
    }
    if ps.is_empty() {
        outln!("  Nothing parked.");
        return Ok(());
    }
    outln!();
    outln!("  DO NOT TOUCH NOW ({})", ps.len());
    outln!();
    for n in ps {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        for l in wrap(n.outcome(a), WIDTH, "         ") {
            outln!("{l}");
        }
    }
    outln!();
    Ok(())
}

/// `stack` — where you are right now, from the root to the focus. With
/// `--lanes` (`t594` §5.5), every lane's own stack instead of only this
/// folder's.
pub fn stack(a: &Tree, root: &Path, args: &Args) -> R {
    let ag = &a.aggregates();
    if args.has("lanes") {
        return stack_lanes(a, root, args, ag);
    }
    let stack: Vec<&Node> = a
        .stack()
        .iter()
        .filter_map(|&num| a.node_by_num(num))
        .collect();
    if args.has("json") {
        return print_json(json!({
            "depth": stack.len(),
            "stack": stack.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    if stack.is_empty() {
        outln!("  Empty stack.  vivac push \"<title>\" --why \"<reason>\"");
        return Ok(());
    }
    outln!();
    for (i, n) in stack.iter().enumerate() {
        let focus = if i == stack.len() - 1 {
            "   <- focus"
        } else {
            ""
        };
        outln!("  {}{:<6} {}{focus}", "  ".repeat(i), n.alias(), n.title(a));
    }
    outln!();
    if stack.len() >= 6 {
        outln!(
            "  Stack {} levels deep. Almost never lack of discipline: usually",
            stack.len()
        );
        outln!("  the root goal moved and nobody re-rooted.  vivac promote");
        outln!();
    }
    Ok(())
}

/// `stack --lanes`'s own rows: every lane the tree knows of, this
/// folder's included, whether it has a front of its own or not (`f668`,
/// `brief::all_lanes`). The ones with a front sort first, the same way
/// OTHER LANES orders its own -- the most recent write first, `id`
/// breaking a tie -- and the ones without follow, sorted by name; a lane
/// that has never pushed has no `seq` of its own to sort by. Marked
/// `(folder gone)` rather than dropped: unlike OTHER LANES, this list
/// exists to name every lane, not only the ones still reachable
/// (decision 2 of `t594` §5.5).
///
/// `exists()` runs at most once per lane the registry knows of for this
/// project, and only when there is at least one row to check it against
/// (`f623`); without `--lanes`, `stack` never reaches this function at
/// all.
fn stack_lanes(a: &Tree, root: &Path, args: &Args, ag: &Aggregates) -> R {
    let mut rows = crate::brief::all_lanes(a);
    rows.sort_by(|x, y| {
        // A lane with a front sorts before one without, regardless of
        // `seq` or name: `bool`'s own order puts `false` (has a front)
        // ahead of `true` (does not).
        x.focus
            .is_none()
            .cmp(&y.focus.is_none())
            .then_with(|| match (x.focus, y.focus) {
                (Some(_), Some(_)) => y.seq.cmp(&x.seq).then_with(|| x.id.cmp(y.id)),
                _ => x.name.cmp(y.name),
            })
    });
    let gone = if rows.is_empty() {
        Vec::new()
    } else {
        crate::brief::gone_lane_ids(root).unwrap_or_default()
    };
    if args.has("json") {
        return print_json(json!({
            "lanes": rows
                .iter()
                .map(|r| json!({
                    "id": r.id,
                    "name": r.name,
                    "focus": match r.focus {
                        Some(focus) => json_node(a, ag, focus),
                        None => serde_json::Value::Null,
                    },
                    "folder_gone": gone.iter().any(|g| g == r.id),
                }))
                .collect::<Vec<_>>(),
        }));
    }
    if rows.is_empty() {
        outln!("  No lanes yet.  vivac setup claude-code plants one.");
        return Ok(());
    }
    outln!();
    for r in &rows {
        let tail = if gone.iter().any(|g| g == r.id) {
            "  (folder gone)"
        } else {
            ""
        };
        match r.focus {
            Some(focus) => outln!(
                "  {:<11} {:<6} {:<45} {}{tail}",
                r.name,
                focus.alias(),
                focus.title(a),
                focus.opened(a)
            ),
            None => outln!("  {:<11} (nothing pushed yet){tail}", r.name),
        }
    }
    outln!();
    Ok(())
}

pub fn stats(a: &Tree, args: &Args) -> R {
    let ag = &a.aggregates();
    let mut by_state = std::collections::BTreeMap::new();
    let mut orphans = 0usize;
    let mut false_closes = Vec::new();
    for n in a.nodes_iter() {
        *by_state.entry(n.state.word(n.kind)).or_insert(0usize) += 1;
        if n.parent.is_some_and(|p| a.node_by_num(p).is_none()) {
            orphans += 1;
        }
        if n.state == State::Done && ag.blockers(n.num) > 0 {
            false_closes.push(n);
        }
    }
    let depth_of = ag.max_depth;
    false_closes.sort_by_key(|n| n.num);
    if args.has("json") {
        return print_json(json!({
            "nodes": a.total(),
            "by_state": by_state,
            "depth": depth_of,
            "roots": a.roots().len(),
            "stack": a.stack_depth(),
            "orphans": orphans,
            "broken_lines": a.broken_lines,
            "false_closes": false_closes.iter().map(|n| json_node(a, ag, n)).collect::<Vec<_>>(),
        }));
    }
    outln!();
    outln!("  nodes          {}", a.total());
    for (k, v) in &by_state {
        outln!("  {k:<14} {v}");
    }
    outln!("  depth          {depth_of}");
    outln!("  roots          {}", a.roots().len());
    outln!("  stack          {}", a.stack_depth());
    if orphans > 0 {
        outln!("  ORPHANS        {orphans}  <- broken provenance");
    }
    if a.broken_lines > 0 {
        outln!("  broken lines   {}  <- in .vivac/events", a.broken_lines);
    }
    if !false_closes.is_empty() {
        outln!();
        outln!("  FALSE CLOSES ({})", false_closes.len());
        for n in false_closes {
            outln!("      {:<6} {}", n.alias(), n.title(a));
        }
    }
    outln!();
    Ok(())
}

/// One stop's own JSON shape: what `vivacs --json` gives per entry, and what
/// `why` on a stop's own alias gives loose, since the two answer the same
/// question about the same stop and cannot be let drift apart from one
/// another.
fn vivac_json(a: &Tree, v: &Vivac) -> serde_json::Value {
    json!({
        "id": v.id,
        "alias": v.alias(),
        "node_ref": v.node_ref.as_ref().and_then(|r| a.node(r).map(|n| n.alias())),
        "kind": v.kind.word(),
        "ts": v.ts,
        "label": v.label,
        "next_intent": v.next_intent,
        "anchor": v.anchor,
        "anchors": v.anchors,
        "stack": v.stack.iter().map(|(al, t)| json!({"alias": al, "title": t}))
            .collect::<Vec<_>>(),
        "working_set": v.working_set,
    })
}

/// `vivacs` — the safe stops, latest first.
pub fn vivacs(a: &Tree, args: &Args) -> R {
    if args.has("json") {
        return print_json(json!(a
            .vivacs
            .iter()
            .rev()
            .map(|v| vivac_json(a, v))
            .collect::<Vec<_>>()));
    }
    if a.vivacs.is_empty() {
        outln!("  No stops yet.  vivac save \"<label>\"");
        return Ok(());
    }
    outln!();
    // The whole tree's catalogue, on purpose: `restore`/`--since` accept
    // any vivac by `num` (`model.rs`'s own `Tree::vivac`), not only the
    // lane's own, and filtering this list would hide a stop those commands
    // still take. With more than one lane, an active neighbour can still
    // push a lane's own stops out of the last twenty before it gets here,
    // and no row says which lane a stop belongs to -- both are `t594` §5,
    // not fixed here, only written down so it is not forgotten by omission
    // (`t594`).
    for v in a.vivacs.iter().rev().take(20) {
        let top = v
            .stack
            .last()
            .map(|(al, t)| format!("{al}  {t}"))
            .unwrap_or_else(|| "empty stack".into());
        outln!(
            "  {:<5} {:<7} {}  {}",
            v.alias(),
            v.kind.word(),
            crate::clock::date_of(&v.ts),
            top
        );
        if !v.label.is_empty() {
            outln!("           {}", v.label);
        }
        if !v.next_intent.is_empty() {
            outln!("           you were about to: {}", v.next_intent);
        }
    }
    if a.vivacs.len() > 20 {
        outln!();
        outln!("  ... and {} more", a.vivacs.len() - 20);
    }
    outln!();
    Ok(())
}

/// The fields of a node that carry meaning, in the order a reader wants them.
///
/// The title is a label; the reason, the notes and the outcome are where the
/// thinking is. A search that read only titles would find the folder and miss
/// what is inside it.
///
/// `f389`: every note lives here, not only the latest, or the search that
/// reads this misses the same 37 percent `why` used to.
fn searchable<'t>(a: &'t Tree, n: &Node) -> Vec<(&'static str, &'t str)> {
    let mut fields = vec![("title", n.title(a)), ("why", n.why(a))];
    fields.extend(n.notes(a).into_iter().map(|(_, text)| ("note", text)));
    fields.push(("outcome", n.outcome(a)));
    fields
}

/// The five Unicode blocks of combining diacritical marks: what [`fold`]
/// drops once `.nfd()` has split every precomposed letter into its base and
/// its marks. `ñ` folds to `n` and `ç` folds to `c` this way. A mark from a
/// script where it is not a diacritic -- a Hebrew point, a Devanagari matra
/// -- carries meaning of its own rather than decorating a Latin letter, sits
/// outside all five blocks, and stays.
fn is_diacritic(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

/// Folds text so search stops caring about case or accent: `dueno` finds
/// `dueño`, `arbol` finds `árbol`, and a decomposed `e` + acute finds a
/// precomposed `é`.
///
/// Lower cases first -- `İ` (U+0130) lower cases to `i` followed by a
/// combining dot above, and that dot has to fall out with the rest of the
/// marks, not survive as a leftover -- then decomposes canonically and drops
/// every [`is_diacritic`] mark. `terms_of` and `hits_for` fold the query and
/// the fields it searches through this one function, so the two sides of a
/// `contains` check can never fold differently.
///
/// [`fold_with_origin`] is the same recipe with a map back to the original
/// text alongside it, for `snippet`, which needs to point at a byte of this
/// output and say which character of the source it came from. Both are
/// [`fold_into`], so they cannot disagree either.
pub(crate) fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    fold_into(text, &mut folded, None);
    folded
}

/// [`fold`], plus a map from each byte of the folded string to the char
/// index of `text` it descends from.
pub(crate) fn fold_with_origin(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    fold_into(text, &mut folded, Some(&mut origin));
    (folded, origin)
}

/// The one implementation of [`fold`], one segment at a time.
///
/// Not built by decomposing one character at a time: NFD's canonical
/// reordering can move a mark past another mark, but only within the run it
/// belongs to, and that run is anchored by the nearest starter before it (a
/// character of combining class zero) -- never further back and never past
/// the next one. Decomposing a character in isolation cannot reorder it
/// against its neighbours at all, so the two can disagree the moment a
/// source already carries two marks in a non-canonical order. Segmenting the
/// lower-cased text at each starter first, and folding one segment at a
/// time, reorders exactly the characters whole-string NFD would have
/// reordered, because canonical reordering never crosses a starter either.
/// A leading run of marks with no starter before it -- text that opens on a
/// combining character -- is a run with nothing to anchor it and is folded
/// as its own segment, the same as `.nfd()` on the whole string would treat
/// it.
///
/// Segments are also what makes this cheap, because most of what a tree
/// holds is ASCII. An ASCII character is a starter that decomposes to
/// itself, so a run of them is copied and lower cased in one go, the way
/// `str::to_lowercase` treats it, and only what is not ASCII is segmented
/// and pays for the tables. A mark that follows a run of ASCII opens a
/// segment of its own: the letter before it is never reordered, so the
/// marks after it reorder among themselves exactly as they would with it.
/// Measured on 10 000 nodes, sending every character through the tables
/// made `find` over MCP five times slower than lower casing had been, and
/// a loop that still went one character at a time left it twice as slow.
fn fold_into(text: &str, folded: &mut String, mut origin: Option<&mut Vec<usize>>) {
    let mut segment = String::new();
    let mut segment_at = 0;
    // The char index of `rest`'s first char in `text`.
    let mut at = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let ascii = rest
            .bytes()
            .position(|b| !b.is_ascii())
            .unwrap_or(rest.len());
        if ascii > 0 {
            if !segment.is_empty() {
                fold_segment(&segment, segment_at, folded, origin.as_deref_mut());
                segment.clear();
            }
            let (run, tail) = rest.split_at(ascii);
            let start = folded.len();
            folded.push_str(run);
            folded[start..].make_ascii_lowercase();
            if let Some(origin) = origin.as_deref_mut() {
                origin.extend(at..at + ascii);
            }
            at += ascii;
            rest = tail;
            continue;
        }
        let c = rest.chars().next().expect("rest is not empty");
        // Lower casing one char can produce more than one -- `İ` becomes
        // two -- and both descend from that char's index.
        for lc in c.to_lowercase() {
            if canonical_combining_class(lc) == 0 && !segment.is_empty() {
                fold_segment(&segment, segment_at, folded, origin.as_deref_mut());
                segment.clear();
            }
            if segment.is_empty() {
                segment_at = at;
            }
            segment.push(lc);
        }
        at += 1;
        rest = &rest[c.len_utf8()..];
    }
    if !segment.is_empty() {
        fold_segment(&segment, segment_at, folded, origin);
    }
}

/// One segment of [`fold_into`], every byte it emits mapped to `at`.
fn fold_segment(
    segment: &str,
    at: usize,
    folded: &mut String,
    mut origin: Option<&mut Vec<usize>>,
) {
    // One byte is one ASCII character, already lower cased, and NFD leaves
    // it as it is: the KELVIN SIGN, say, which lower cases to `k`.
    if segment.len() == 1 {
        folded.push_str(segment);
        if let Some(origin) = origin {
            origin.push(at);
        }
        return;
    }
    for c in segment.nfd().filter(|c| !is_diacritic(*c)) {
        if let Some(origin) = origin.as_deref_mut() {
            origin.extend(std::iter::repeat_n(at, c.len_utf8()));
        }
        folded.push(c);
    }
}

/// A window of `width` characters around the first term that hit.
///
/// The offsets come out of the folded copy `fold_with_origin` builds, and
/// folding can change how many bytes --and even how many characters-- a
/// string takes, so the map back to the original travels with it rather
/// than being assumed. A snippet that lands two characters off is not a
/// defect worth a wrong answer.
fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let (lower, origin) = fold_with_origin(text);
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .map(|b| origin[b])
        .unwrap_or(0);
    let end = (at + width * 2 / 3).clamp(width, chars.len());
    let start = end - width;
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.extend(chars[start..end].iter());
    if end < chars.len() {
        out.push_str("...");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text search over the tree.
///
/// `PILLARS.md` gives text search a ceiling of 100 ms and nothing ever
/// implemented it: a budget with no floor under it, the same class of
/// unchecked claim as the test count that lied for a day.
///
/// Two rules it does not bend. **Every term has to appear**, or a second word
/// would widen the search instead of narrowing it, which is the opposite of
/// what typing more means. And **closed nodes are searched too**: what you
/// look for months later is usually finished, and a search that stopped at
/// the open fronts would be a to-do list rather than a memory.
///
/// Order is not recency. Newest-first was the first answer, and it is the
/// wrong one for the search a memory is actually asked to do: the hits that
/// founded a subject are the oldest of them, and they were arriving last.
/// `d362` orders by what a hit is about first, by how much tree it holds up
/// second, and by recency only as the last tiebreak.
fn terms_of(query: &str) -> Result<Vec<String>, Failure> {
    let terms: Vec<String> = query.split_whitespace().map(fold).collect();
    if terms.is_empty() {
        return Err(Failure::usage("usage: vivac find \"<text>\"".to_string()));
    }
    Ok(terms)
}

/// Where a field lands in the order [`hits_for`] sorts by: title first, why
/// second, note and outcome tied for last. A function rather than the
/// position `searchable` returns the field at, because note and outcome tie
/// and a position has no room for one.
fn field_order(field: &str) -> u8 {
    match field {
        "title" => 0,
        "why" => 1,
        _ => 2,
    }
}

/// Every node that matches, best first, each with the fields it hit on.
///
/// Three keys, read in order, with no weights and no tunable constants.
///
/// **What the hit is about.** A term in the title is what the node is
/// called; a term in the reason is what the node argued; a term in a note
/// or an outcome is what happened along the way. The first of those
/// answers "where was this decided" better than the last, so the field of
/// the best hit dominates everything else. The note and the outcome tie:
/// both are what came after the argument.
///
/// **How much tree it holds up.** Among nodes that hit on the same field
/// the question is which one founded the subject, and the tree already
/// knows: the one everything else hangs off. `Aggregates` has the subtree
/// total of every node from a pass that is already linear, so this costs
/// a lookup.
///
/// **Recency**, last. It was the whole order before `d362` and it is a
/// tiebreak now: a search over a tree that has run for months is answered
/// from the end often enough to be worth keeping, and never often enough
/// to outrank what the hit is about.
///
/// What is deliberately absent is a relevance score. Term frequency, IDF
/// and length normalization are what BM25 would add, and here IDF is inert
/// -- every term has to appear, so every hit contains all of them -- while
/// length normalization is inverted: it penalizes long fields as diluted,
/// and in this corpus a long reason is the reasoning.
fn hits_for<'t>(
    a: &'t Tree,
    ag: &Aggregates,
    terms: &[String],
) -> Vec<(&'t Node, Vec<&'static str>)> {
    let mut hits: Vec<(&Node, Vec<&'static str>)> = Vec::new();
    for n in a.nodes_iter() {
        let lowered: Vec<(&'static str, String)> = searchable(a, n)
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (*k, fold(v)))
            .collect();
        if !terms
            .iter()
            .all(|t| lowered.iter().any(|(_, v)| v.contains(t.as_str())))
        {
            continue;
        }
        // `d390`: `note` can now appear more than once in `lowered`, one
        // entry per note. Deduped here, or a term two notes both carry would
        // print the same `note:` line once per note that hit rather than
        // once per field, the way `field_order` already assumes a hit names
        // each field at most once.
        let mut seen_fields = std::collections::HashSet::new();
        let matched: Vec<&'static str> = lowered
            .iter()
            .filter(|(_, v)| terms.iter().any(|t| v.contains(t.as_str())))
            .map(|(k, _)| *k)
            .filter(|k| seen_fields.insert(*k))
            .collect();
        hits.push((n, matched));
    }
    hits.sort_by_key(|(n, matched)| {
        (
            field_order(matched[0]),
            std::cmp::Reverse(ag.counts(n.num).total),
            std::cmp::Reverse(n.num),
        )
    });
    hits
}

fn lineage_of(a: &Tree, n: &Node) -> Vec<String> {
    let line = a.ancestors(n.num);
    line[..line.len().saturating_sub(1)]
        .iter()
        .map(|p| p.alias())
        .collect()
}

/// A handle to a hit, not the node itself: `why` on the alias brings the rest.
///
/// Returning the whole node paid for `why`, `note` and `outcome` in full on
/// every hit, plus twelve bookkeeping fields nobody asked for. Measured over
/// the real tree with one query, both numbers from the same run: the JSON
/// cost 8.7 times its own prose and now costs 1.7. `matched` carries the
/// fragment `snippet` would print rather than the whole field, for the same
/// reason.
///
/// The six fields a hit carries, shared with [`find_data_everywhere`] so the
/// shape stays in exactly one place: `d273` adds a `project` field beside
/// this one rather than widening it.
fn hit_json(a: &Tree, n: &Node, matched: &[&'static str], terms: &[String]) -> serde_json::Value {
    let fragments: serde_json::Map<String, serde_json::Value> = matched
        .iter()
        .map(|field| {
            let text = searchable(a, n)
                .iter()
                .find(|(k, _)| k == field)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            (field.to_string(), json!(snippet(text, terms, WIDTH)))
        })
        .collect();
    json!({
        "alias": n.alias(),
        "kind": n.kind,
        "state": n.state,
        "title": n.title(a),
        "lineage": lineage_of(a, n),
        "matched": fragments,
    })
}

pub fn find_data(a: &Tree, query: &str) -> Result<serde_json::Value, Failure> {
    let terms = terms_of(query)?;
    let ag = &a.aggregates();
    Ok(json!(hits_for(a, ag, &terms)
        .iter()
        .map(|(n, matched)| hit_json(a, n, matched, &terms))
        .collect::<Vec<_>>()))
}

pub fn find(a: &Tree, args: &Args) -> R {
    let query = args
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac find \"<text>\"".to_string()))?;
    let terms = terms_of(query)?;
    if args.has("json") {
        return print_json(find_data(a, query)?);
    }
    let ag = &a.aggregates();
    let hits = hits_for(a, ag, &terms);

    if hits.is_empty() {
        outln!("  Nothing matches \"{query}\".");
        return Ok(());
    }
    outln!();
    outln!(
        "  {} match{} for \"{}\"",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" },
        query,
    );
    outln!();
    for (n, matched) in hits.iter().take(20) {
        outln!("  {:<6} {}", n.alias(), n.title(a));
        let lineage = lineage_of(a, n);
        if !lineage.is_empty() {
            outln!("         via {}", lineage.join(" > "));
        }
        // The title is already on the line above it. Repeating it as the
        // reason the hit came back would say nothing.
        for field in matched.iter().filter(|f| **f != "title") {
            let text = searchable(a, n)
                .iter()
                .find(|(k, _)| k == field)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            outln!("         {}: {}", field, snippet(text, &terms, WIDTH));
        }
    }
    if hits.len() > 20 {
        outln!();
        outln!(
            "  ... and {} more   vivac find \"...\" --json",
            hits.len() - 20
        );
    }
    outln!();
    Ok(())
}

/// The directory's own name, as `d146` defines it: never the path it sits
/// under, because an absolute path names the account and the machine it
/// runs on and the security pillar allows neither into a result.
///
/// `pub(crate)` rather than private since `d273`'s second half: `registry`
/// resolves `--project`'s value against the same bare name this hands back,
/// so a hit `find --everywhere` prints is exactly what `why --project` then
/// takes.
pub(crate) fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
}

/// The JSON twin of [`find_everywhere`]: every hit [`hit_json`] already
/// knows how to build, plus the project it came from. A separate builder
/// rather than a wider `find_data`, the same call `d172` made when `find`
/// stopped sharing `json_node`.
fn find_data_everywhere(projects: &[(String, Tree)], terms: &[String]) -> serde_json::Value {
    let mut hits = Vec::new();
    for (name, tree) in projects {
        let ag = &tree.aggregates();
        for (n, matched) in hits_for(tree, ag, terms) {
            let mut hit = hit_json(tree, n, &matched, terms);
            if let serde_json::Value::Object(fields) = &mut hit {
                fields.insert("project".to_string(), json!(name));
            }
            hits.push(hit);
        }
    }
    json!(hits)
}

/// [`find_everywhere`]'s JSON, with no `Args` to read it from: what
/// `vivac_find`'s `everywhere` argument calls through the MCP server. The
/// same read `find --everywhere --json` runs, so the two can never drift
/// apart -- `d172`'s tie, carried past one project.
pub fn find_everywhere_data(query: &str) -> Result<serde_json::Value, Failure> {
    let terms = terms_of(query)?;
    let known_roots = crate::store::store_dir()
        .map(|d| crate::registry::roots(&d))
        .unwrap_or_default();
    let mut projects: Vec<(String, Tree)> = Vec::new();
    for root in known_roots {
        let name = project_name(&root);
        if let Ok(tree) =
            crate::store::Store::open(root).and_then(|s| crate::index::load(&s, false))
        {
            projects.push((name, tree));
        }
    }
    projects.sort_by(|x, y| x.0.cmp(&y.0));
    Ok(find_data_everywhere(&projects, &terms))
}

/// `find`, fanned out over every project the registry knows about instead
/// of only the one under foot. `d273`'s first half.
///
/// Each tree loads through the local index with `allow_persist: false`:
/// searching from one project must never write inside another project's
/// `.vivac/`. A root that fails to open -- moved, deleted, unreadable -- is
/// not skipped: `d201` settled that a vanished root going quiet loses
/// exactly the answer somebody came for, so it is counted and named instead.
///
/// A bare alias means nothing across trees -- `d100` exists in three of them
/// and names three different decisions -- so text output groups hits by
/// project rather than running them together.
pub fn find_everywhere(a: &Args) -> R {
    let query = a
        .positional(0)
        .ok_or_else(|| Failure::usage("usage: vivac find \"<text>\"".to_string()))?;
    let terms = terms_of(query)?;

    let known_roots = crate::store::store_dir()
        .map(|d| crate::registry::roots(&d))
        .unwrap_or_default();

    let mut projects: Vec<(String, Tree)> = Vec::new();
    let mut unreachable: Vec<String> = Vec::new();
    for root in known_roots {
        let name = project_name(&root);
        match crate::store::Store::open(root).and_then(|s| crate::index::load(&s, false)) {
            Ok(tree) => projects.push((name, tree)),
            Err(_) => unreachable.push(name),
        }
    }
    projects.sort_by(|x, y| x.0.cmp(&y.0));
    unreachable.sort();

    if a.has("json") {
        return print_json(find_data_everywhere(&projects, &terms));
    }

    type ProjectHits<'t> = (&'t str, &'t Tree, Vec<(&'t Node, Vec<&'static str>)>);
    let sections: Vec<ProjectHits> = projects
        .iter()
        .filter_map(|(name, tree)| {
            let ag = &tree.aggregates();
            let hits = hits_for(tree, ag, &terms);
            (!hits.is_empty()).then_some((name.as_str(), tree, hits))
        })
        .collect();
    let total: usize = sections.iter().map(|(_, _, hits)| hits.len()).sum();

    if total == 0 {
        outln!("  Nothing matches \"{query}\".");
    } else {
        outln!();
        outln!(
            "  {} match{} for \"{}\" across {} project{}",
            total,
            if total == 1 { "" } else { "es" },
            query,
            sections.len(),
            if sections.len() == 1 { "" } else { "s" },
        );
        for (name, tree, hits) in &sections {
            outln!();
            outln!("  {name}");
            for (n, matched) in hits.iter().take(20) {
                outln!("    {:<6} {}", n.alias(), n.title(tree));
                let lineage = lineage_of(tree, n);
                if !lineage.is_empty() {
                    outln!("           via {}", lineage.join(" > "));
                }
                for field in matched.iter().filter(|f| **f != "title") {
                    let text = searchable(tree, n)
                        .iter()
                        .find(|(k, _)| k == field)
                        .map(|(_, v)| *v)
                        .unwrap_or_default();
                    outln!("           {}: {}", field, snippet(text, &terms, WIDTH));
                }
            }
            if hits.len() > 20 {
                outln!(
                    "    ... and {} more   vivac find \"...\" --everywhere --json",
                    hits.len() - 20
                );
            }
        }
        outln!();
    }

    if !unreachable.is_empty() {
        outln!(
            "  {} project{} unreachable: {}",
            unreachable.len(),
            if unreachable.len() == 1 { "" } else { "s" },
            unreachable.join(", ")
        );
        outln!();
    }

    Ok(())
}

#[cfg(test)]
mod fold_tests {
    use super::{fold, fold_with_origin};
    use unicode_normalization::UnicodeNormalization;

    #[test]
    fn folds_spanish_diacritics_away() {
        assert_eq!(fold("dueño"), fold("dueno"));
        assert_eq!(fold("árbol"), fold("arbol"));
        assert_eq!(fold("ÁRBOL"), fold("arbol"));
    }

    #[test]
    fn folds_decomposed_and_precomposed_the_same_way() {
        let decomposed = "e\u{0301}"; // e + combining acute accent
        assert_eq!(fold(decomposed), fold("é"));
    }

    #[test]
    fn a_lower_cased_combining_mark_still_drops() {
        // U+0130, LATIN CAPITAL LETTER I WITH DOT ABOVE, lower cases to
        // `i` followed by U+0307, COMBINING DOT ABOVE.
        assert_eq!(fold("\u{0130}"), fold("i"));
    }

    /// What [`fold`] has to equal, written the plain way: lower case the
    /// whole text, run the whole of it through NFD, drop the marks. Slower,
    /// and independent of the segmenting [`super::fold_into`] does, which is
    /// the point: a test that compared the two public functions would be
    /// comparing one implementation with itself.
    fn reference(text: &str) -> String {
        text.chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .nfd()
            .filter(|c| !super::is_diacritic(*c))
            .collect()
    }

    /// A fixed, varied corpus: Spanish and French accents, Vietnamese with
    /// stacked marks, Hebrew points, Devanagari, marks in non-canonical
    /// order, a string that opens on a combining mark, an emoji and CJK.
    /// For each one, both [`fold`] and the folded half of
    /// [`fold_with_origin`] have to equal [`reference`], the map has to hold
    /// one entry per byte, and every entry has to name a real char index of
    /// the source.
    #[test]
    fn the_fold_agrees_with_whole_string_nfd() {
        let cases = [
            "dueño",
            "café",
            "garçon",
            "\u{1ec7}", // Vietnamese ệ, e with circumflex and dot below
            "Vi\u{1ec7}t Nam",
            "\u{5e9}\u{5b8}\u{5dc}\u{5d5}\u{5b9}\u{5dd}", // Hebrew, with points
            "\u{928}\u{940}\u{932}",                      // Devanagari
            "e\u{0301}\u{0323}", // acute (230) before dot below (220): not canonical
            // Shin, dagesh (21), qamats (18): marks that stay, in an order
            // NFD has to swap.
            "\u{5e9}\u{5bc}\u{5b8}",
            "\u{0301}bc", // opens on a combining acute
            // ASCII, then marks that stay, out of canonical order: the run
            // of ASCII is copied whole and the marks open their own segment.
            "sha\u{5bc}\u{5b8}lom",
            "Ab\u{0301}\u{0323}C \u{212a}elvin", // stacked marks after ASCII; KELVIN SIGN
            "ÁRBOL \u{0130}stanbul",
            "🌳 tree",
            "\u{6a39}\u{6728}", // CJK: tree, wood
        ];
        for text in cases {
            let expected = reference(text);
            assert_eq!(fold(text), expected, "fold disagrees on {text:?}");
            let (mapped, origin) = fold_with_origin(text);
            assert_eq!(mapped, expected, "fold_with_origin disagrees on {text:?}");
            assert_eq!(
                origin.len(),
                mapped.len(),
                "one origin per byte of {text:?}"
            );
            let char_count = text.chars().count();
            for (byte, idx) in origin.iter().enumerate() {
                assert!(
                    *idx < char_count,
                    "byte {byte} of {text:?} maps to char index {idx}, past its {char_count} chars"
                );
            }
        }
    }
}
