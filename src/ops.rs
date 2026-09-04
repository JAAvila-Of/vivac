//! The operations that write. Every one goes through the redaction guard.
//!
//! Capture hangs off the seams of the work, never off a judgement of
//! relevance. That is the one thing actually measured: over 170 real minutes,
//! `push`/`pop` --which cannot be skipped without leaving the work half done--
//! were called nine times, and the operation that asked "is this worth
//! keeping?" was called zero times, under a protocol declared mandatory.

use crate::anchor::{self, Anchor};
use crate::event::{Body, Event, Flag, Kind, State, VivacKind};
use crate::failure::{Failure, R};
use crate::model::{fold, Node, Tree};
use crate::outcome::{self, Outcome};
use crate::params;
use crate::store::Store;
use crate::{id, redact};

pub struct Ctx {
    pub store: Store,
    pub tree: Tree,
    pub anchor: Box<dyn Anchor>,
}

impl Ctx {
    pub fn load(store: Store) -> Result<Ctx, Failure> {
        Ctx::load_with_log(store).map(|(c, _)| c)
    }

    /// Same read as `load`, handing back the events instead of dropping them.
    ///
    /// One command needs the log itself and not only what it folds into
    /// (`changes`), and there are two worse ways to give it that: a field on
    /// `Ctx` that twenty-nine other commands carry and never read, or a
    /// second full read of a file this one has already read.
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
        self.store.append(bodies.clone(), self.tree.seq)?;
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
        .filter_map(|id| ctx.tree.node(id))
        .map(|n| (n.alias(), n.title.clone()))
        .collect();
    let mut working_set: Vec<String> = ctx
        .tree
        .stack
        .iter()
        .filter_map(|id| ctx.tree.node(id))
        .flat_map(|n| n.governs.iter().cloned())
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

fn kind_of(raw: Option<&str>, fallback: Kind) -> Result<Kind, Failure> {
    match raw {
        None => Ok(fallback),
        Some(s) => Kind::parse(s)
            .ok_or_else(|| Failure::usage(format!("Unknown type: {s}. They are: {}", Kind::ALL))),
    }
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
}

/// Creates a node. Returns the event and the alias number assigned.
///
/// Takes a `Born` already extracted rather than `&Args`: the three ops that
/// call this (`push`, `add`, `decide`) do not all read the fields the same
/// way (`add` defaults `why` with `.opt_or`, `push` demands it), so the
/// reading stays with each caller and only the shared write comes here.
fn born(ctx: &Ctx, b: Born) -> Result<(Body, u64, String), Failure> {
    let mut fields: Vec<(&str, &str)> = vec![("title", b.title), ("why", b.why)];
    fields.extend(b.refs.iter().map(|r| ("ref", r.as_str())));
    fields.extend(b.governs.iter().map(|g| ("governs", g.as_str())));
    guard_text(&fields)?;

    let node = id::ulid();
    let num = ctx.tree.next_num.max(1);
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
        },
        num,
        node,
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
    let (ev, num, node) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
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
        ctx.tree.roots().first().map(|root| outcome::DepthAdvice {
            depth: depth_of,
            root_alias: root.alias(),
            root_title: root.title.clone(),
        })
    } else {
        None
    };
    Ok(Outcome::Pushed {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        blocks: p.blocks,
        advice,
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
    let parent = match ctx.tree.node(focus.parent.as_deref().unwrap_or("")) {
        Some(parent) => Some(outcome::PoppedTo {
            alias: parent.alias(),
            title: parent.title.clone(),
            counts: ctx.tree.counts(&parent.id),
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
    if ctx.tree.stack.contains(&node.id) {
        evs.push(Body::Popped {
            node: node.id.clone(),
        });
    }
    ctx.emit(evs)?;
    Ok(Outcome::Parked {
        alias: node.alias(),
        title: node.title,
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
        let pending_count = ctx.tree.open_blockers(&n.id);
        if !pending_count.is_empty() {
            let mut m = format!(
                "  {} CANNOT close: {} open closure condition(s)\n",
                n.alias(),
                pending_count.len()
            );
            for c in &pending_count {
                m.push_str(&format!("\n      {:<6} {}", c.alias(), c.title));
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
    if unstack && ctx.tree.stack.contains(&n.id) {
        evs.push(Body::Popped { node: n.id.clone() });
    }
    ctx.emit(evs)?;
    Ok(crate::outcome::Closed {
        alias: n.alias(),
        title: n.title.clone(),
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
    let (ev, num, _) = born(
        ctx,
        Born {
            title: &p.title,
            why: &p.why,
            kind,
            parent: parent.clone(),
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
        },
    )?;
    ctx.emit(vec![ev])?;
    let parent_info = parent
        .and_then(|id| ctx.tree.node(&id))
        .map(|n| outcome::AddedUnder {
            alias: n.alias(),
            title: n.title.clone(),
        });
    Ok(Outcome::Added {
        alias: format!("{}{}", kind.prefix(), num),
        title: p.title,
        parent: parent_info,
        blocks: p.blocks,
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
    let Some(parent) = n.parent.as_ref().and_then(|p| ctx.tree.node(p)) else {
        return Err(Failure::usage(format!(
            "{} is the root: there is no parent to block.",
            n.alias()
        )));
    };
    let blocks = !p.off;
    let (pa, pt) = (parent.alias(), parent.title.clone());
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
        .as_ref()
        .and_then(|id| ctx.tree.node(id))
        .map(|parent| outcome::StillBornFrom {
            alias: parent.alias(),
            title: parent.title.clone(),
        });
    Ok(Outcome::Promoted {
        alias: n.alias(),
        title: n.title,
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
        let (rid, ralias) = (r.id.clone(), r.alias());
        if rid == n.id {
            return Err(Failure::usage(format!(
                "{ralias} is the one being abandoned; it cannot be rescued from itself"
            )));
        }
        if !ctx.tree.descendants(&n.id).iter().any(|d| d.id == rid) {
            return Err(Failure::usage(format!(
                "{ralias} does not hang off {}: there is nothing to rescue it from",
                n.alias()
            )));
        }
        rescued.insert(rid.clone());
        for d in ctx.tree.descendants(&rid) {
            rescued.insert(d.id.clone());
        }
    }

    let (falling, saved): (Vec<&Node>, Vec<&Node>) = ctx
        .tree
        .descendants(&n.id)
        .into_iter()
        .filter(|d| d.state.is_open())
        .partition(|d| !rescued.contains(&d.id));

    // Only what falls unnamed needs confirming. If everything was rescued,
    // there is nothing left to confirm.
    if !falling.is_empty() && !p.cascade {
        let mut m = format!(
            "  {}  {}\n  has {} open descendant(s) with no rescue:\n",
            n.alias(),
            n.title,
            falling.len()
        );
        for d in &falling {
            m.push_str(&format!("\n      {:<6} {}", d.alias(), d.title));
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
    let saved_lines: Vec<(String, String)> =
        saved.iter().map(|d| (d.alias(), d.title.clone())).collect();
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
    let mut out_of_scope: Vec<String> = vec![n.id.clone()];
    out_of_scope.extend(ctx.tree.descendants(&n.id).iter().map(|d| d.id.clone()));
    for id in out_of_scope {
        if ctx.tree.stack.contains(&id) {
            evs.push(Body::Popped { node: id });
        }
    }

    ctx.emit(evs)?;
    Ok(Outcome::Abandoned {
        alias: n.alias(),
        title: n.title,
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

    let lineage: Vec<String> = ctx
        .tree
        .ancestors(&n.id)
        .iter()
        .map(|p| p.id.clone())
        .collect();
    let mut evs: Vec<Body> = ctx
        .tree
        .stack
        .iter()
        .filter(|id| !lineage.contains(id))
        .map(|id| Body::Popped { node: id.clone() })
        .collect();
    if !n.state.is_open() {
        evs.push(Body::StateChanged {
            node: n.id.clone(),
            state: State::Active,
            outcome: String::new(),
            forced: false,
        });
    }
    for id in &lineage {
        if !ctx.tree.stack.contains(id) {
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
            title: n.title,
            reason,
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
    let (ev, num, _) = born(
        ctx,
        Born {
            title: &p.title,
            why: &body,
            kind: Kind::Decision,
            parent,
            refs: p.refs,
            governs: p.governs,
            blocks: p.blocks,
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
                lineage.push(n.id.clone());
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
        .filter(|id| !lineage.contains(id))
        .map(|id| Body::Popped { node: id.clone() })
        .collect();
    for id in &lineage {
        if !ctx.tree.stack.contains(id) {
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
pub fn session_started(
    ctx: &mut Ctx,
    source: &str,
    session: Option<String>,
) -> Result<Outcome, Failure> {
    // The focus the brief paints is the top of the stack: it walks the
    // ancestors of `stack.last()` and keeps the last of the lineage, which is
    // that same node again.
    let focus = ctx.tree.focus().map(|n| n.id.clone());
    let vivac = ctx.tree.vivacs.last().map(|v| v.id.clone());
    ctx.emit(vec![Body::SessionStarted {
        source: source.to_string(),
        focus,
        vivac,
        session,
    }])?;
    Ok(Outcome::SessionOpened)
}
