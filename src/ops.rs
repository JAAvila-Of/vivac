//! The operations that write. Every one goes through the redaction guard.
//!
//! All but one refuse outright when it finds something. The exception is the
//! session opening: it is written by a hook, with no author present to reword
//! anything, so it replaces the field with the name of the rule that refused
//! it and writes the seam anyway. Losing the seam would cost more than the
//! field is worth, and a refusal that is recorded is not a refusal in silence.
//!
//! Capture hangs off the seams of the work, never off a judgement of
//! relevance. That is the one thing actually measured: over 170 real minutes,
//! `push`/`pop` --which cannot be skipped without leaving the work half done--
//! were called nine times, and the operation that asked "is this worth
//! keeping?" was called zero times, under a protocol declared mandatory.

use crate::anchor::{self, Anchor};
use crate::event::{Against, Arm, Body, Event, Flag, Kind, State, VivacKind};
use crate::failure::{Failure, R};
use crate::model::{fold, Node, Tree};
use crate::outcome::{self, Outcome};
use crate::params;
use crate::store::Store;
use crate::{id, redact};
use std::path::Path;

pub struct Ctx {
    pub store: Store,
    pub tree: Tree,
    pub anchor: Box<dyn Anchor>,
}

impl Ctx {
    /// For a command that only ever reads. `LOADING.md` §4: this is a read,
    /// so it is free to refresh the derived index once its tail passes the
    /// threshold -- see `index::load`.
    pub fn load(store: Store) -> Result<Ctx, Failure> {
        Ctx::load_opt(store, true)
    }

    /// For a command that may append to the log. Still free to read a warm
    /// or stale index -- applying its tail is cheap enough for the write
    /// budget -- but it must never pay to rewrite the file itself
    /// (`LOADING.md` §4 "Cuándo se reescribe").
    pub fn load_for_write(store: Store) -> Result<Ctx, Failure> {
        Ctx::load_opt(store, false)
    }

    fn load_opt(store: Store, allow_index_refresh: bool) -> Result<Ctx, Failure> {
        let tree = crate::index::load(&store, allow_index_refresh)?;
        let anchor = anchor::detect(&store.root);
        Ok(Ctx {
            store,
            tree,
            anchor,
        })
    }

    /// Same read `changes` and `why` need, handing back the events instead
    /// of dropping them. Both need the log's own fields -- `actor`, `lane`,
    /// the exact payload -- which the derived index does not carry, so this
    /// always folds the whole log rather than going through `index::load`:
    /// there is no tail to apply that would save the read those two need
    /// anyway.
    pub fn load_with_log(store: Store) -> Result<(Ctx, Vec<Event>), Failure> {
        let (events, broken) = store.read_all()?;
        let tree = fold(&events, broken);
        let anchor = anchor::detect(&store.root);
        Ok((
            Ctx {
                store,
                tree,
                anchor,
            },
            events,
        ))
    }

    /// Writes and **then applies in memory**, so that whatever gets printed
    /// next is the state after the operation and not the one before it.
    fn emit(&mut self, bodies: Vec<Body>) -> R {
        // `d444`: the one bit `Store::append`'s own write-lock needs and
        // cannot see for itself -- whether this tree already has a pillar
        // or a rule, from a write before this one.
        let already_governed = self.tree.has_governance;
        self.store
            .append(bodies.clone(), self.tree.seq, already_governed)?;
        let ts = crate::clock::now_rfc3339();
        for c in &bodies {
            let seq = self.tree.seq + 1;
            self.tree.apply(seq, &ts, c);
        }
        Ok(())
    }

    fn resolve(&self, s: &str) -> Result<&crate::model::Node, Failure> {
        self.tree
            .resolve(s)
            .ok_or_else(|| Failure::usage(format!("No such node: {s}.")))
    }
}

/// Builds a vivac out of the stack as it stands right now.
///
/// The `working_set` is **not measured**: measuring which files the pitch
/// touched would need a `post_tool` hook, which is not in Tier 0. It is
/// derived from the `governs` the stack declares, which is what there is, and
/// the `brief` says so rather than pretending it observed it.
fn vivac(
    ctx: &Ctx,
    kind: VivacKind,
    next_intent: &str,
    node_ref: Option<String>,
    label: &str,
) -> Body {
    let stack: Vec<(String, String)> = ctx
        .tree
        .stack
        .iter()
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| (n.alias(), n.title(&ctx.tree).to_string()))
        .collect();
    let mut working_set: Vec<String> = ctx
        .tree
        .stack
        .iter()
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .flat_map(|n| n.governs(&ctx.tree).into_iter().map(str::to_string))
        .collect();
    working_set.sort();
    working_set.dedup();
    Body::VivacCreated {
        vivac: id::ulid(),
        num: ctx.tree.next_vivac_num.max(1),
        kind,
        stack,
        working_set,
        next_intent: next_intent.to_string(),
        anchor: ctx.anchor.snapshot(),
        node_ref,
        label: label.to_string(),
    }
}

/// No text reaches the log without coming through here.
fn guard_text(fields: &[(&str, &str)]) -> R {
    match redact::check_fields(fields) {
        Some(h) => Err(Failure::Redaction(Box::new(h))),
        None => Ok(()),
    }
}

/// Text off an untrusted payload, made safe to store without failing the
/// write. What the guard objected to never gets in; what replaces it names
/// the rule and nothing else, so the log shows that a refusal happened
/// without repeating what caused it.
fn guarded_or_refused(field: &str, text: &str) -> String {
    match redact::check_field(field, text) {
        Some(f) => format!("refused: {}", f.rule),
        None => text.to_string(),
    }
}

fn kind_of(raw: Option<&str>, fallback: Kind) -> Result<Kind, Failure> {
    match raw {
        None => Ok(fallback),
        Some(s) => Kind::parse(s)
            .ok_or_else(|| Failure::usage(format!("Unknown type: {s}. They are: {}", Kind::ALL))),
    }
}

/// The same guard `note` is held to: one line, not empty. `d415`.
fn validate_arm_text(s: &str) -> Result<(), Failure> {
    if s.trim().is_empty() {
        return Err(Failure::usage("An arm cannot be empty."));
    }
    if s.contains('\n') {
        return Err(Failure::usage(
            "An arm is one line: write it the way it would be typed.",
        ));
    }
    Ok(())
}

/// Is this slashed-and-unnormalized folder absolute? Checked on every
/// system regardless of which one is running, per `d441`: the log travels,
/// and a path only Windows would call absolute still carries this machine's
/// layout once it lands somewhere else.
fn is_absolute_arm_dir(slashed: &str) -> bool {
    if slashed.starts_with('/') || slashed.starts_with('~') {
        return true;
    }
    let mut chars = slashed.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

/// `./vivac/` -> `vivac`; `vivac\src` (already slashed to `vivac/src`) stays
/// `vivac/src`; `.`, `./` or `.\` (slashed to `./`) -> `.`. `d441`.
fn normalize_arm_dir(slashed: &str) -> String {
    let parts: Vec<&str> = slashed
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Checks 3 through 6 of `d441`'s six, shared by `--arm-dir` (`add`, `push`)
/// and `--dir` (`arm`): once a folder is known to have been given at all --
/// check 1 or 2, which differ in wording by caller -- absolute, `..`,
/// redaction and existence are the same check regardless of which flag
/// named it.
fn validate_arm_dir(raw: &str, tree_root: &Path) -> Result<String, Failure> {
    let slashed = raw.replace('\\', "/");
    if is_absolute_arm_dir(&slashed) {
        return Err(Failure::usage(
            "An arm's folder is relative to the one that holds .vivac: an \
             absolute path would write this machine's layout into the log.",
        ));
    }
    if slashed.split('/').any(|c| c == "..") {
        return Err(Failure::usage(
            "An arm's folder has to be inside the one that holds .vivac.",
        ));
    }
    let normalized = normalize_arm_dir(&slashed);
    guard_text(&[("dir", &normalized)])?;
    if !tree_root.join(&normalized).is_dir() {
        return Err(Failure::usage(format!(
            "There is no folder {normalized} inside the one that holds .vivac."
        )));
    }
    Ok(normalized)
}

/// The wording of the missing-folder and folder-without-arm messages, in the
/// two vocabularies a flag can be named in: the CLI's own `--flag`, and
/// MCP's bare argument name. §21: "the same texts, with `arm_dir` in place
/// of `--arm-dir`, `dir` in place of `--dir` and `arm` in place of `--arm`."
fn needs_arm_dir_message(via_mcp: bool) -> String {
    let flag = if via_mcp { "arm_dir" } else { "--arm-dir" };
    format!(
        "An arm needs {flag}: the folder it runs in, relative to the one \
         that holds .vivac. Use . for that folder itself."
    )
}

fn arm_dir_without_arm_message(via_mcp: bool) -> String {
    let (dir_flag, arm_flag) = if via_mcp {
        ("arm_dir", "arm")
    } else {
        ("--arm-dir", "--arm")
    };
    format!("{dir_flag} says where an arm runs, and no {arm_flag} was given.")
}

fn needs_dir_message(via_mcp: bool) -> String {
    let flag = if via_mcp { "dir" } else { "--dir" };
    format!(
        "An arm needs {flag}: the folder it runs in, relative to the one \
         that holds .vivac. Use . for that folder itself."
    )
}

/// `--arm`/`--arm-dir`, checked against the type it is born with. `d415`:
/// only a rule may carry one, and it may carry none -- a rule with no arm is
/// judged. `d441`: whenever one or more `--arm` are given, `--arm-dir` is
/// mandatory and names the one folder every arm in this call runs in.
/// `via_mcp` only ever changes which vocabulary the missing-folder messages
/// use, never the check itself.
fn arms_of(
    ctx: &Ctx,
    raw: Vec<String>,
    dir: Option<String>,
    kind: Kind,
    via_mcp: bool,
) -> Result<Vec<Arm>, Failure> {
    if !raw.is_empty() && kind != Kind::Rule {
        return Err(Failure::usage(format!(
            "Only a rule has an arm; this would be {}.",
            kind.with_article()
        )));
    }
    if raw.is_empty() {
        if dir.is_some() {
            return Err(Failure::usage(arm_dir_without_arm_message(via_mcp)));
        }
        return Ok(vec![]);
    }
    let dir = match dir {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Err(Failure::usage(needs_arm_dir_message(via_mcp))),
    };
    let normalized = validate_arm_dir(&dir, &ctx.store.root)?;
    for (i, a) in raw.iter().enumerate() {
        validate_arm_text(a)?;
        if raw[..i].contains(a) {
            return Err(Failure::usage(format!("The same arm is given twice: {a}")));
        }
    }
    Ok(raw
        .into_iter()
        .map(|command| Arm {
            dir: normalized.clone(),
            command,
        })
        .collect())
}

/// Splits one `--against` entry on the **first** `:`: the id to its left,
/// the sentence to its right, both trimmed. `t426` §2.1: the sentence may
/// carry more colons of its own.
fn split_against_entry(raw: &str) -> Result<(&str, &str), Failure> {
    let form_error =
        || Failure::usage("--against needs an id and a sentence: --against \"r12: why it holds\"");
    let (id, why) = raw.split_once(':').ok_or_else(form_error)?;
    let (id, why) = (id.trim(), why.trim());
    if id.is_empty() || why.is_empty() {
        return Err(form_error());
    }
    Ok((id, why))
}

/// `--against`, checked against what it points at. `t426` §2.1 and §2.2:
/// shared by `decide`, `push`, `add` and `declare`, since every one of them
/// judges an entry by the same questions -- only `push` and `add` ever
/// call it with a `kind` that is not already `Kind::Decision`, since a
/// decision is the only kind that may carry one.
///
/// Every check runs **before** anything is written, in order: the form of
/// each entry, that the id exists, that it names a pillar or a rule, that
/// it still governs, and that no id repeats within this one call.
fn against_of(ctx: &Ctx, raw: Vec<String>, kind: Kind) -> Result<Vec<Against>, Failure> {
    if !raw.is_empty() && kind != Kind::Decision {
        return Err(Failure::usage(format!(
            "--against goes on a decision, and this is {}",
            kind.with_article()
        )));
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut seen: Vec<u64> = Vec::with_capacity(raw.len());
    for entry in &raw {
        let (id, why) = split_against_entry(entry)?;
        let n = ctx
            .tree
            .resolve(id)
            .ok_or_else(|| Failure::usage(format!("No such node: {id}.")))?;
        if !matches!(n.kind, Kind::Pillar | Kind::Rule) {
            return Err(Failure::usage(format!(
                "--against points at a pillar or a rule, and {} is {}",
                n.alias(),
                n.kind.with_article()
            )));
        }
        if !n.state.is_open() {
            return Err(Failure::usage(format!(
                "--against points at what still governs, and {} is {}: vivac rules lists what does",
                n.alias(),
                n.state.word(n.kind)
            )));
        }
        if seen.contains(&n.num) {
            return Err(Failure::usage(format!(
                "--against names {} twice",
                n.alias()
            )));
        }
        seen.push(n.num);
        out.push(Against {
            node: n.id.clone(),
            why: why.to_string(),
        });
    }
    Ok(out)
}

/// What it takes to create a node, named rather than positional.
///
/// `title` and `why` stay borrowed rather than owned: every caller still
/// needs its own copy afterwards (a title goes into the vivac, `add`'s
/// `where_at` reads the parent, not this), so taking a slice costs nothing
/// and asking for an owned `String` here would just make each caller clone
/// one it already had.
struct Born<'a> {
    title: &'a str,
    why: &'a str,
    kind: Kind,
    parent: Option<String>,
    refs: Vec<String>,
    governs: Vec<String>,
    blocks: bool,
    arms: Vec<Arm>,
    /// Already validated by `against_of`. Empty for every kind that is not
    /// a decision, since only a decision may carry one.
    against: Vec<Against>,
}

/// Creates a node. Returns the event, the alias number assigned, and
/// whether the `against` key was written empty -- `d445`'s `no_against`,
/// which `push`, `add` and `decide` each fold into their own `Outcome`.
///
/// Takes a `Born` already extracted rather than `&Args`: the three ops that
/// call this (`push`, `add`, `decide`) do not all read the fields the same
/// way (`add` defaults `why` with `.opt_or`, `push` demands it), so the
/// reading stays with each caller and only the shared write comes here.
fn born(ctx: &Ctx, b: Born) -> Result<(Body, u64, String, bool), Failure> {
    let mut fields: Vec<(&str, &str)> = vec![("title", b.title), ("why", b.why)];
    fields.extend(b.refs.iter().map(|r| ("ref", r.as_str())));
    fields.extend(b.governs.iter().map(|g| ("governs", g.as_str())));
    fields.extend(b.arms.iter().map(|a| ("arm", a.command.as_str())));
    fields.extend(b.against.iter().map(|a| ("against", a.why.as_str())));
    guard_text(&fields)?;

    let node = id::ulid();
    let num = ctx.tree.next_num.max(1);
    // `t426` §1.1: `Some` only for a decision born while at least one
    // pillar or rule is open -- the same predicate `vivac rules` lists
    // under -- and `Some(vec![])` when nothing was declared. Every other
    // node keeps writing exactly the bytes it always has.
    let against = (b.kind == Kind::Decision && ctx.tree.has_open_governance()).then_some(b.against);
    let no_against = against.as_ref().is_some_and(Vec::is_empty);
    Ok((
        Body::NodeCreated {
            node: node.clone(),
            num,
            kind: b.kind,
            title: b.title.to_string(),
            why: b.why.to_string(),
            parent: b.parent,
            blocks: b.blocks,
            refs: b.refs,
            governs: b.governs,
            arms: b.arms,
            against,
        },
        num,
        node,
        no_against,
    ))
}

/// `push` — open a detour. It is **the** operation: the provenance edge is
/// created here on its own, with nobody having to remember to declare it.
pub fn push(ctx: &mut Ctx, p: params::Push) -> Result<Outcome, Failure> {
    let parent = ctx.tree.focus().map(|n| n.id.clone());
    let kind = kind_of(
        p.kind.as_deref(),
        if parent.is_none() {
            Kind::Goal
        } else {
            Kind::Task
        },
    )?;
    let arms = arms_of(ctx, p.arms, p.arm_dir, kind, p.via_mcp)?;
    let against = against_of(ctx, p.against, kind)?;
    let (ev, num, node, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms,
            against,
        },
    )?;
    // The vivac goes **before** the push: it freezes the stack at the moment
    // of the fork, which is the belay where you make yourself safe before
    // setting off. The `next_intent` is the child being opened, because that
    let v = vivac(ctx, VivacKind::Push, &p.title, parent, "");
    ctx.emit(vec![v, ev, Body::Pushed { node }])?;

    // `emit` already applied the push in memory, so the stack includes the
    // new node and there is no need to add one.
    let depth_of = ctx.tree.stack_depth();
    // §6.1: intervene, never block. A deep stack is almost never lack of
    // discipline: the root goal moved and nobody re-rooted.
    let advice = if depth_of >= 4 {
        // The node named is the **bottom of this stack**, never the tree's
        // first root: the number measures the stack (`f156`), so taking the
        // number from one place and the node from another gives a true count
        // pointing at the wrong goal. It showed up as soon as there was more
        // than one root -- which is what `promote` exists to make -- and the
        // advice named whichever root was written first, closed or not
        // (`f331`).
        ctx.tree.stack_bottom().map(|root| outcome::DepthAdvice {
            depth: depth_of,
            root_alias: root.alias(),
            root_title: root.title(&ctx.tree).to_string(),
        })
    } else {
        None
    };
    Ok(Outcome::Pushed {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        blocks: p.blocks,
        advice,
        no_against,
    })
}

/// `pop` — close the focus and come back to the parent with context.
pub fn pop(ctx: &mut Ctx, p: params::Pop) -> Result<Outcome, Failure> {
    let focus = ctx
        .tree
        .focus()
        .ok_or_else(|| {
            Failure::usage(
                "The stack is empty. Open something:  vivac push \"<title>\" --why \"<reason>\"",
            )
        })?
        .clone();
    let outcome_text = p.outcome.as_str();
    let next = p.next.as_deref().unwrap_or(outcome_text);
    guard_text(&[("outcome", outcome_text), ("next", next)])?;
    let v = vivac(ctx, VivacKind::Pop, next, Some(focus.id.clone()), "");
    // Trap: two separate `emit`s in a row, not one lot like `push` -- one
    // inside `close_node`, one here for the vivac -- and the parent's counts
    // below have to be read only after both, or the number comes out wrong.
    let closed = close_node(ctx, &focus, outcome_text, p.force, true)?;
    ctx.emit(vec![v])?;
    let parent = match focus.parent.and_then(|p| ctx.tree.node_by_num(p)) {
        Some(parent) => Some(outcome::PoppedTo {
            alias: parent.alias(),
            title: parent.title(&ctx.tree).to_string(),
            counts: ctx.tree.counts(parent.num),
        }),
        None => None,
    };
    Ok(Outcome::Popped { closed, parent })
}

/// `park` — what produces DO NOT TOUCH NOW; without it that section always
/// comes out empty. The closure rule does not stop it: parking claims nothing
/// finished, and if parking cost more than ignoring, nobody would park.
/// Whether a word is shaped like the name of a node.
///
/// A bare number, or one character of type prefix and a number: `25`, `f25`.
/// Prose never looks like that, so a word that does and resolves to nothing is
/// a typo rather than a reason, and saying so beats guessing.
fn looks_like_an_id(s: &str) -> bool {
    let s = s.trim().trim_start_matches('#');
    let mut c = s.chars();
    let Some(first) = c.next() else {
        return false;
    };
    let rest = c.as_str();
    if first.is_ascii_digit() {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// The node an operation acts on where naming it is optional, and the reason
/// written beside it.
///
/// **Two words are not ambiguous**: the first is an id and it has to resolve.
/// `park f74 "<reason>"` used to fall through to the focus when `f74` named
/// nothing, parking a node nobody had written down, filing `f74` itself as the
/// reason, dropping the reason actually typed, and exiting 0. The hole was
/// `and_then`, which flattens "did not resolve" into the same `None` as "was
/// not given" (`f74`).
///
/// One word **is** ambiguous by the grammar, because a reason is as good a
/// word as an alias. So it is resolved, and only a word shaped like an id has
/// to succeed.
fn named_or_focus(
    ctx: &Ctx,
    node: Option<&str>,
    reason: Option<&str>,
    usage: &'static str,
) -> Result<(Node, String), Failure> {
    let focus = || {
        ctx.tree
            .focus()
            .cloned()
            .ok_or_else(|| Failure::usage(usage))
    };
    match (node, reason) {
        (Some(s), Some(r)) => Ok((ctx.resolve(s)?.clone(), r.to_string())),
        (Some(w), None) => match ctx.tree.resolve(w) {
            Some(n) => Ok((n.clone(), String::new())),
            None if looks_like_an_id(w) => Err(Failure::usage(format!("No such node: {w}."))),
            None => Ok((focus()?, w.to_string())),
        },
        _ => Ok((focus()?, String::new())),
    }
}

pub fn park(ctx: &mut Ctx, p: params::Park) -> Result<Outcome, Failure> {
    let (node, reason) = named_or_focus(
        ctx,
        p.node.as_deref(),
        p.reason.as_deref(),
        "usage: vivac park [<id>] [\"<reason>\"]",
    )?;
    let reason = reason.as_str();
    guard_text(&[("reason", reason)])?;
    let mut evs = vec![vivac(
        ctx,
        VivacKind::Park,
        reason,
        Some(node.id.clone()),
        "",
    )];
    evs.push(Body::StateChanged {
        node: node.id.clone(),
        state: State::Suspended,
        outcome: reason.to_string(),
        forced: false,
    });
    if ctx.tree.stack.contains(&node.num) {
        evs.push(Body::Popped {
            node: node.id.clone(),
        });
    }
    ctx.emit(evs)?;
    Ok(Outcome::Parked {
        alias: node.alias(),
        title: node.title(&ctx.tree).to_string(),
    })
}

/// The closure rule. `MODEL.md` §7, and the **only** rule in the model that
/// refuses a user operation.
///
/// It earns that privilege because the case it prevents is measured: an
/// audit marked DONE with its findings open took 26 days to be spotted.
/// Without this, the model lets the same mistake happen again.
fn close_node(
    ctx: &mut Ctx,
    n: &crate::model::Node,
    outcome: &str,
    force: bool,
    unstack: bool,
) -> Result<crate::outcome::Closed, Failure> {
    if !force {
        let pending_count = ctx.tree.open_blockers(n.num);
        if !pending_count.is_empty() {
            let mut m = format!(
                "  {} CANNOT close: {} open closure condition(s)\n",
                n.alias(),
                pending_count.len()
            );
            for c in &pending_count {
                m.push_str(&format!("\n      {:<6} {}", c.alias(), c.title(&ctx.tree)));
            }
            m.push_str(&format!(
                "\n\n  A run closes with its findings, not with its report.\n  \
                 Closing it anyway leaves a trace:  vivac done {} --force",
                n.num
            ));
            return Err(Failure::Model(m));
        }
    }
    let mut evs = vec![Body::StateChanged {
        node: n.id.clone(),
        state: State::Done,
        outcome: outcome.to_string(),
        forced: force,
    }];
    if unstack && ctx.tree.stack.contains(&n.num) {
        evs.push(Body::Popped { node: n.id.clone() });
    }
    ctx.emit(evs)?;
    Ok(crate::outcome::Closed {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        force,
    })
}

pub fn done(ctx: &mut Ctx, p: params::Done) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    guard_text(&[("outcome", &p.outcome)])?;
    let closed = close_node(ctx, &n, &p.outcome, p.force, true)?;
    Ok(Outcome::Done { closed })
}

/// `add` — a node without touching the stack. It is how a tree that already
/// existed elsewhere gets in, and how a finding hangs off something that is
pub fn add(ctx: &mut Ctx, p: params::Add) -> Result<Outcome, Failure> {
    let parent = match &p.parent {
        Some(s) => Some(ctx.resolve(s)?.id.clone()),
        None => ctx.tree.focus().map(|n| n.id.clone()),
    };
    let kind = kind_of(
        p.kind.as_deref(),
        if parent.is_none() {
            Kind::Goal
        } else {
            Kind::Task
        },
    )?;
    let arms = arms_of(ctx, p.arms, p.arm_dir, kind, p.via_mcp)?;
    let against = against_of(ctx, p.against, kind)?;
    let (ev, num, _, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms,
            against,
        },
    )?;
    ctx.emit(vec![ev])?;
    let parent_info = parent
        .and_then(|id| ctx.tree.node(&id))
        .map(|n| outcome::AddedUnder {
            alias: n.alias(),
            title: n.title(&ctx.tree).to_string(),
        });
    Ok(Outcome::Added {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        parent: parent_info,
        blocks: p.blocks,
        no_against,
    })
}

pub fn note(ctx: &mut Ctx, p: params::Note) -> Result<Outcome, Failure> {
    let (n, note) = match (p.node.as_deref(), p.note.as_deref()) {
        (Some(s), Some(t)) => (ctx.resolve(s)?.clone(), t.to_string()),
        (Some(t), None) => {
            let f = ctx
                .tree
                .focus()
                .ok_or_else(|| Failure::usage("usage: vivac note [<id>] \"<note>\""))?;
            (f.clone(), t.to_string())
        }
        _ => return Err(Failure::usage("usage: vivac note [<id>] \"<note>\"")),
    };
    guard_text(&[("note", &note)])?;
    ctx.emit(vec![Body::NodeNoted {
        node: n.id.clone(),
        note,
    }])?;
    Ok(Outcome::Noted { alias: n.alias() })
}

pub fn block(ctx: &mut Ctx, p: params::Block) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    let Some(parent) = n.parent.and_then(|p| ctx.tree.node_by_num(p)) else {
        return Err(Failure::usage(format!(
            "{} is the root: there is no parent to block.",
            n.alias()
        )));
    };
    let blocks = !p.off;
    let (pa, pt) = (parent.alias(), parent.title(&ctx.tree).to_string());
    ctx.emit(vec![Body::BlockChanged {
        node: n.id.clone(),
        blocks,
    }])?;
    Ok(Outcome::Blocked {
        alias: n.alias(),
        blocks,
        parent_alias: pa,
        parent_title: pt,
    })
}

/// `promote` — the focus becomes a goal of its own and the stack is cut there.
///
/// The provenance chain is **kept**: where it was born does not change just
/// because its rank did. Without this operation, the depth warning has no way
/// out and ends up being ignored.
pub fn promote(ctx: &mut Ctx, p: params::Promote) -> Result<Outcome, Failure> {
    let n = match p.id {
        Some(s) => ctx.resolve(&s)?.clone(),
        None => ctx
            .tree
            .focus()
            .ok_or_else(|| Failure::usage("usage: vivac promote [<id>]"))?
            .clone(),
    };
    ctx.emit(vec![Body::Promoted { node: n.id.clone() }])?;
    let parent = n
        .parent
        .and_then(|id| ctx.tree.node_by_num(id))
        .map(|parent| outcome::StillBornFrom {
            alias: parent.alias(),
            title: parent.title(&ctx.tree).to_string(),
        });
    Ok(Outcome::Promoted {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        parent,
    })
}

/// `abandon` — discard. It costs the same as `pop` on purpose: if abandoning
/// were dearer than ignoring, nobody would abandon and in three months the
/// tree would be noise.
///
/// The cascade is **not** the default. `MODEL.md` §6 wants it with a
/// confirmation and the list up front, and a non-interactive CLI cannot
/// confirm anything: it shows what would fall and asks for an explicit
///
/// **Rescue does not reparent** (`d33`). `MODEL.md` §6 said to re-parent the
/// descendant onto a living ancestor; that rewrites the birth, and invariant
/// 11 says a thing is born in one place. A rescued node stays where it was
/// born: alive, under an abandoned parent. It is the same shape as an open
/// finding under a closed batch, which the tree already knows how to show and
/// the brief already knows how to count.
pub fn abandon(ctx: &mut Ctx, p: params::Abandon) -> Result<Outcome, Failure> {
    let (n, reason) = named_or_focus(
        ctx,
        p.node.as_deref(),
        p.reason.as_deref(),
        "usage: vivac abandon [<id>] \"<reason>\"",
    )?;
    let reason = reason.as_str();
    guard_text(&[("reason", reason)])?;

    // Rescuing a node rescues its descendants. Saving the parent and letting
    // the children die would be a half rescue nobody asked for, and would
    // orphan exactly what was meant to be kept.
    let mut rescued: std::collections::HashSet<String> = Default::default();
    for s in p.rescue {
        let r = ctx
            .tree
            .resolve(&s)
            .ok_or_else(|| Failure::usage(format!("no such node: {s}")))?;
        let (rid, r_num, ralias) = (r.id.clone(), r.num, r.alias());
        if rid == n.id {
            return Err(Failure::usage(format!(
                "{ralias} is the one being abandoned; it cannot be rescued from itself"
            )));
        }
        if !ctx.tree.descendants(n.num).iter().any(|d| d.id == rid) {
            return Err(Failure::usage(format!(
                "{ralias} does not hang off {}: there is nothing to rescue it from",
                n.alias()
            )));
        }
        rescued.insert(rid.clone());
        for d in ctx.tree.descendants(r_num) {
            rescued.insert(d.id.clone());
        }
    }

    let (falling, saved): (Vec<&Node>, Vec<&Node>) = ctx
        .tree
        .descendants(n.num)
        .into_iter()
        .filter(|d| d.state.is_open())
        .partition(|d| !rescued.contains(&d.id));

    // Only what falls unnamed needs confirming. If everything was rescued,
    // there is nothing left to confirm.
    if !falling.is_empty() && !p.cascade {
        let mut m = format!(
            "  {}  {}\n  has {} open descendant(s) with no rescue:\n",
            n.alias(),
            n.title(&ctx.tree),
            falling.len()
        );
        for d in &falling {
            m.push_str(&format!("\n      {:<6} {}", d.alias(), d.title(&ctx.tree)));
        }
        m.push_str("\n\n  Abandon all of it:     vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --cascade");
        m.push_str("\n  Save some of it:       vivac abandon ");
        m.push_str(&n.num.to_string());
        m.push_str(" --rescue <id>");
        m.push_str("\n  Save it as a goal:     vivac promote <id>");
        return Err(Failure::Model(m));
    }

    let mut evs = vec![Body::StateChanged {
        node: n.id.clone(),
        state: State::Abandoned,
        outcome: reason.to_string(),
        forced: false,
    }];
    let falling_count = falling.len();
    let saved_lines: Vec<(String, String)> = saved
        .iter()
        .map(|d| (d.alias(), d.title(&ctx.tree).to_string()))
        .collect();
    for d in falling {
        evs.push(Body::StateChanged {
            node: d.id.clone(),
            state: State::Abandoned,
            outcome: format!("cascaded from {}", n.alias()),
            forced: false,
        });
    }
    // The stack is the path to the focus and cannot cross an abandoned node,
    // so everything hanging off the abandoned one leaves it --the rescued
    // included, which stays alive but stops being on the path--.
    let mut out_of_scope: Vec<(u64, String)> = vec![(n.num, n.id.clone())];
    out_of_scope.extend(
        ctx.tree
            .descendants(n.num)
            .iter()
            .map(|d| (d.num, d.id.clone())),
    );
    for (num, id) in out_of_scope {
        if ctx.tree.stack.contains(&num) {
            evs.push(Body::Popped { node: id });
        }
    }

    ctx.emit(evs)?;
    Ok(Outcome::Abandoned {
        alias: n.alias(),
        title: n.title(&ctx.tree).to_string(),
        cascaded: (falling_count > 0).then_some(falling_count),
        rescued: saved_lines
            .into_iter()
            .map(|(alias, title)| outcome::RescuedNode { alias, title })
            .collect(),
    })
}

/// `focus` — step back into a node that already exists.
///
/// Without this the stack only works inside one session: the next day the log
/// holds the whole tree and the stack is empty, and there is no way to say "I
/// am on this" without opening a new node, which is exactly the litter to be
/// avoided. The stack becomes the path from the root down to the node, which
/// is what working on it means.
pub fn focus(ctx: &mut Ctx, p: params::Focus) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();

    if !n.state.is_open() && !p.reopen {
        // Parking says "maybe I will be back", so returning is the normal
        // operation and asks no permission. Closing claims something finished:
        // undoing that has to be deliberate.
        if n.state != State::Suspended {
            return Err(Failure::Model(format!(
                "  {} is {}. Going back into it undoes that claim.\n\n  \
                 If it really was not finished:  vivac focus {} --reopen",
                n.alias(),
                n.state.word(n.kind),
                n.num
            )));
        }
    }

    let lineage: Vec<(u64, String)> = ctx
        .tree
        .ancestors(n.num)
        .iter()
        .map(|p| (p.num, p.id.clone()))
        .collect();
    let mut evs: Vec<Body> = ctx
        .tree
        .stack
        .iter()
        .filter(|num| !lineage.iter().any(|(lineage_num, _)| lineage_num == *num))
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| Body::Popped { node: n.id.clone() })
        .collect();
    if !n.state.is_open() {
        evs.push(Body::StateChanged {
            node: n.id.clone(),
            state: State::Active,
            outcome: String::new(),
            forced: false,
        });
    }
    for (num, id) in &lineage {
        if !ctx.tree.stack.contains(num) {
            evs.push(Body::Pushed { node: id.clone() });
        }
    }
    let revived = !n.state.is_open();
    ctx.emit(evs)?;
    // Trap: `render::stack` used to be called from here, reading `a` for its
    // own `--json` on its own. `main.rs` calls it separately now, after this
    // `Outcome` is printed -- `render.rs` is not touched, and the flag never
    // reached this call site from the CLI anyway (`focus` is not allowed
    // `--json` in `main.rs`'s table).
    Ok(Outcome::Focused {
        alias: n.alias(),
        revived,
    })
}

/// `flag <id> <flag> --why <reason>` — raise or clear a flag.
///
/// The reason is **mandatory** when raising it. `BRIEF-SPEC.md` §10 tests it
/// as a contract: a flag with no reason informs nobody, it only adds noise to
/// the brief, and within a week they all get ignored.
pub fn flag(ctx: &mut Ctx, p: params::Flag) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    let flag = Flag::parse(&p.flag).ok_or_else(|| {
        Failure::usage(format!("Unknown flag: {}. They are: {}", p.flag, Flag::ALL))
    })?;

    if p.off {
        ctx.emit(vec![Body::FlagCleared {
            node: n.id.clone(),
            flag,
        }])?;
        return Ok(Outcome::Flagged {
            alias: n.alias(),
            flag: flag.word().to_string(),
            change: outcome::FlagChange::Off,
        });
    }
    let reason = p.why.ok_or_else(|| {
        Failure::usage(
            "Missing --why. A flag with no reason informs nobody: in two weeks\n  \
             nobody will know what needed looking at, and they all get ignored.",
        )
    })?;
    guard_text(&[("reason", &reason)])?;
    ctx.emit(vec![Body::FlagRaised {
        node: n.id.clone(),
        flag,
        reason: reason.clone(),
    }])?;
    Ok(Outcome::Flagged {
        alias: n.alias(),
        flag: flag.word().to_string(),
        change: outcome::FlagChange::Raised {
            title: n.title(&ctx.tree).to_string(),
            reason,
        },
    })
}

/// `arm <id> "<command>" [--off]` — record or remove what verifies a rule.
///
/// Vivac never runs it: `d415`. Shaped like `flag`, and like `flag` it
/// arms or disarms a closed node. Where it parts from `flag` is the
/// repeat: a flag folds into a set, so raising it twice changes nothing,
/// but arms fold into a list, so a repeated arm would show twice and the
/// removal of an absent one would write a line that changes no answer.
/// Both are refused before anything is written.
pub fn arm(ctx: &mut Ctx, p: params::Arm) -> Result<Outcome, Failure> {
    let n = ctx.resolve(&p.id)?.clone();
    if n.kind != Kind::Rule {
        return Err(Failure::usage(format!(
            "Only a rule has an arm; {} is {}.",
            n.alias(),
            n.kind.with_article()
        )));
    }
    let dir = match &p.dir {
        Some(d) if !d.trim().is_empty() => d.clone(),
        _ => return Err(Failure::usage(needs_dir_message(p.via_mcp))),
    };
    let dir = validate_arm_dir(&dir, &ctx.store.root)?;
    validate_arm_text(&p.command)?;
    let has = n
        .arms(&ctx.tree)
        .contains(&(dir.as_str(), p.command.as_str()));
    if p.off && !has {
        return Err(Failure::usage(format!(
            "{0} has no such arm; vivac why {0} lists the ones it has.",
            n.alias()
        )));
    }
    if !p.off && has {
        return Err(Failure::usage(format!(
            "{} already has that arm.",
            n.alias()
        )));
    }
    guard_text(&[("arm", &p.command)])?;
    let body = if p.off {
        Body::ArmRemoved {
            node: n.id.clone(),
            dir: dir.clone(),
            command: p.command.clone(),
        }
    } else {
        Body::ArmAdded {
            node: n.id.clone(),
            dir: dir.clone(),
            command: p.command.clone(),
        }
    };
    ctx.emit(vec![body])?;
    Ok(Outcome::Armed {
        alias: n.alias(),
        dir,
        arm: p.command,
        change: if p.off {
            outcome::ArmChange::Removed
        } else {
            outcome::ArmChange::Added
        },
    })
}

/// `decide` — record a decision.
///
/// The discarded alternatives are optional in the schema and mandatory in
/// practice: without them, in a month the agent proposes again what you
/// already rejected.
pub fn decide(ctx: &mut Ctx, p: params::Decide) -> Result<Outcome, Failure> {
    let superseded = match &p.supersedes {
        Some(s) => Some(ctx.resolve(s)?.clone()),
        None => None,
    };

    let mut body = p.reason.clone();
    if !p.alternatives.is_empty() {
        body.push_str(&format!("  |  discarded: {}", p.alternatives.join("; ")));
    }
    let parent = match &p.parent {
        Some(s) => Some(ctx.resolve(s)?.id.clone()),
        None => ctx.tree.focus().map(|n| n.id.clone()),
    };
    let against = against_of(ctx, p.against, Kind::Decision)?;
    let (ev, num, _, no_against) = born(
        ctx,
        Born {
            title: &p.title,
            why: &body,
            kind: Kind::Decision,
            parent,
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
            arms: vec![],
            against,
        },
    )?;

    let mut evs = vec![ev];
    if let Some(v) = &superseded {
        // `supersedes` forms a chain: the old one becomes superseded, not deleted.
        evs.push(Body::StateChanged {
            node: v.id.clone(),
            state: State::Superseded,
            outcome: format!("superseded by d{num}"),
            forced: false,
        });
    }
    ctx.emit(evs)?;
    Ok(Outcome::Decided {
        alias: format!("d{num}"),
        title: p.title,
        superseded: superseded.map(|v| outcome::SupersededNode { alias: v.alias() }),
        no_alternatives: p.alternatives.is_empty(),
        no_against,
    })
}

/// `declare <decision> --against "<id>: <why>"` — record, after the fact,
/// what a decision was judged against. `t426` §2.2: unlike `--against` at
/// birth, the decision may be in any state -- it declares a fact about the
/// past, not a claim about what it still governs.
pub fn declare(ctx: &mut Ctx, p: params::Declare) -> Result<Outcome, Failure> {
    let (Some(id), false) = (p.id.as_deref(), p.against.is_empty()) else {
        return Err(Failure::usage(
            "usage: vivac declare <decision> --against \"r12: <why>\"",
        ));
    };
    let n = ctx.resolve(id)?.clone();
    if n.kind != Kind::Decision {
        return Err(Failure::usage(format!(
            "vivac declare takes a decision, and {} is {}",
            n.alias(),
            n.kind.with_article()
        )));
    }
    let entries = against_of(ctx, p.against, Kind::Decision)?;
    // What the decision already declares, at birth or later, cannot be
    // declared again.
    let mut already: Vec<u64> = n.against.iter().map(|a| a.node).collect();
    for e in &entries {
        let (num, alias) = ctx
            .tree
            .node(&e.node)
            .map(|x| (x.num, x.alias()))
            .unwrap_or((u64::MAX, e.node.clone()));
        if already.contains(&num) {
            return Err(Failure::usage(format!(
                "{} already declares {alias}",
                n.alias()
            )));
        }
        already.push(num);
    }
    guard_text(
        &entries
            .iter()
            .map(|a| ("against", a.why.as_str()))
            .collect::<Vec<_>>(),
    )?;
    ctx.emit(vec![Body::AgainstAdded {
        node: n.id.clone(),
        against: entries.clone(),
    }])?;
    Ok(Outcome::Declared {
        alias: n.alias(),
        against: entries
            .into_iter()
            .map(|a| outcome::DeclaredPair {
                node: ctx.tree.node(&a.node).map(|x| x.alias()).unwrap_or(a.node),
                why: a.why,
            })
            .collect(),
    })
}

/// `save [label]` — a safe stop on purpose.
pub fn save(ctx: &mut Ctx, p: params::Save) -> Result<Outcome, Failure> {
    guard_text(&[("label", &p.label), ("next", &p.next)])?;
    let v = vivac(ctx, VivacKind::Manual, &p.next, None, &p.label);
    let num = ctx.tree.next_vivac_num.max(1);
    ctx.emit(vec![v])?;
    // With no VCS no precision is faked: the vivac is worth the same, but
    // restoring it will only give plain age, not a diff.
    let anchor = ctx.anchor.snapshot();
    Ok(Outcome::Saved {
        num,
        label: p.label,
        anchor,
        next: p.next,
    })
}

/// `restore <v>` — go back to a vivac.
///
/// **It never touches the working tree.** Mixing context navigation with tree
/// manipulation turns a tool for attention into a branch manager worse than
/// git. It rebuilds the stack and presents the diff.
pub fn restore(ctx: &mut Ctx, p: params::Restore) -> Result<Outcome, Failure> {
    let v = ctx
        .tree
        .vivac(&p.vivac)
        .ok_or_else(|| Failure::usage(format!("No such vivac: {}.", p.vivac)))?
        .clone();

    // The vivac's stack is frozen by alias. Nodes that no longer exist or are
    // closed get skipped and named: restoring resurrects nothing.
    let mut lineage = Vec::new();
    let mut lost: Vec<outcome::LostNode> = Vec::new();
    for (alias, title) in &v.stack {
        let state = match ctx.tree.resolve(alias) {
            Some(n) if n.state.is_open() => {
                lineage.push((n.num, n.id.clone()));
                continue;
            }
            Some(n) => n.state.word(n.kind).to_string(),
            None => "gone".to_string(),
        };
        lost.push(outcome::LostNode {
            alias: alias.clone(),
            title: title.clone(),
            state,
        });
    }
    let mut evs: Vec<Body> = ctx
        .tree
        .stack
        .iter()
        .filter(|num| !lineage.iter().any(|(lineage_num, _)| lineage_num == *num))
        .filter_map(|&num| ctx.tree.node_by_num(num))
        .map(|n| Body::Popped { node: n.id.clone() })
        .collect();
    for (num, id) in &lineage {
        if !ctx.tree.stack.contains(num) {
            evs.push(Body::Pushed { node: id.clone() });
        }
    }
    let changes = ctx.anchor.changed_since(&v.anchor);
    ctx.emit(evs)?;

    let anchor = if v.anchor.is_empty_tree() {
        outcome::RestoreAnchor::Empty
    } else if changes.is_empty() {
        outcome::RestoreAnchor::NoChanges {
            anchor_short: v.anchor.short().to_string(),
        }
    } else {
        outcome::RestoreAnchor::Changed {
            anchor_short: v.anchor.short().to_string(),
            changes: changes
                .iter()
                .map(|c| outcome::ChangeLine {
                    file_path: c.file_path.clone(),
                    times: c.times,
                })
                .collect(),
            working_set: v.working_set.clone(),
        }
    };
    // Trap: `render::stack` used to be called from here too, on the same `a`
    // it read `--json` from on its own. `main.rs` calls it separately now,
    // after this `Outcome` is printed -- `restore` is allowed no flags at all
    // in `main.rs`'s table, so `--json` never reached this call site either.
    Ok(Outcome::Restored {
        alias: v.alias(),
        kind: v.kind.word().to_string(),
        ts: v.ts,
        label: v.label,
        next_intent: v.next_intent,
        lost,
        anchor,
    })
}

/// An automatic stop, for the end-of-session hook.
pub fn auto_vivac(
    ctx: &mut Ctx,
    kind: VivacKind,
    next: &str,
    label: &str,
) -> Result<Outcome, Failure> {
    guard_text(&[("next", next), ("label", label)])?;
    let v = vivac(ctx, kind, next, None, label);
    ctx.emit(vec![v])?;
    Ok(Outcome::AutoStopped)
}

/// The opening of a session, for the start hook.
///
/// It records **what the brief claimed** --the focus it named and the stop it
/// showed as the last one-- so that *was the brief followed?* can be answered
/// by comparing that against the first node touched afterwards, instead of by
/// somebody's judgement.
///
/// These are inputs and never a verdict: what counts as *following* the brief
/// lives in whoever reads, not in the log. Storing the comparison instead of
/// its terms would freeze a definition that may well turn out to be wrong.
///
/// `source` and `session` come off a payload this program did not write, so
/// they go through the guard like any other text. Unlike everywhere else, a
/// finding does not stop the write: the field is replaced by the rule that
/// refused it and the opening is recorded regardless. The rest of the guard
/// can afford to refuse because somebody is there to reword the sentence; a
/// hook has nobody, and a hook that fails is a hook that gets switched off.
pub fn session_started(
    ctx: &mut Ctx,
    source: &str,
    session: Option<String>,
) -> Result<Outcome, Failure> {
    let source = guarded_or_refused("source", source);
    let session = session.map(|s| guarded_or_refused("session", &s));
    // The focus the brief paints is the top of the stack: it walks the
    // ancestors of `stack.last()` and keeps the last of the lineage, which is
    // that same node again.
    let focus = ctx.tree.focus().map(|n| n.id.clone());
    let vivac = ctx.tree.vivacs.last().map(|v| v.id.clone());
    ctx.emit(vec![Body::SessionStarted {
        source,
        focus,
        vivac,
        session,
    }])?;
    Ok(Outcome::SessionOpened)
}
