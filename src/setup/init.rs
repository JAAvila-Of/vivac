//! `vivac init`'s share of the tree side (`d723` piece A): planting is
//! `init`'s job, and configuring a harness is `setup`'s -- `init` used to
//! plant with none of the flags `setup` already gave the tree side, so
//! `--join`, `--new-tree`, `--name` and `--lane-name` stayed unreachable
//! from a folder with no interest in a harness at all (`f721`). This module
//! is the very same path `claude_code.rs` and `codex.rs` used to walk
//! through `tree.rs`, minus the three files a harness reads: the roots, the
//! plan, asking, and the tree's own writes -- planting, the lane, the
//! version lock and the `.gitignore`.
//!
//! Reached by every `init` run now, bare or flagged (`main.rs`, `f721`):
//! a bare run used to take a separate path straight through
//! `Store::create`, which called neither `tree::plan` nor the guard
//! against two trees of one product that lives inside it -- planting
//! quietly succeeded where a flagged run already refused. `main.rs` keeps
//! one guard of its own ahead of this dispatch, for a folder whose own
//! `.vivac/lane` cannot be resolved to any tree (`t594`); every other
//! case, flagged or not, is decided here.
//!
//! `tree.rs`'s own `plan`, `plan_join` and the refusal a shared repository
//! raises no longer take a `Harness` (`d723` piece B, `f717` dissolved):
//! `init` is the only caller left, and every message a plant or a join can
//! raise now proposes `vivac init`, which names no harness at all.

use super::tree;
use crate::args::Args;
use crate::failure::Failure;
use crate::output::outln;
use crate::plan::{heading, render_items, PlanItem};
use crate::style::Stream;

pub(super) fn run(cwd: &std::path::Path, a: &Args) -> Result<i32, Failure> {
    let roots = super::resolve_roots(cwd)?;
    if a.has("undo") {
        return undo(&roots, a);
    }
    // `t594`: this guard used to run ahead of `apply` in `claude_code.rs`,
    // catching the home folder and the global store before deciding
    // between planting and joining. `d723` piece B moved that decision
    // here in full, and the guard belongs wherever the decision is made --
    // left behind, a tree could be planted or a lane declared in either
    // place, and only a later `vivac setup` would have caught it, after
    // the fact. `claude_code::run` and `codex::run` still check it too, for
    // a lane that already exists there from before this guard moved.
    if let Some(refusal) = super::refuse_home_or_global_store(&roots) {
        return Err(refusal);
    }
    apply(&roots, a)
}

fn apply(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let below = tree::trees_below(&roots.here);
    let join_spec = a.opt("join");
    if !below.is_empty() {
        return Err(match join_spec {
            Some(spec) => super::claude_code::tree_below_join_refusal(&roots.here, &below, spec),
            None => tree::tree_below_refusal(&below),
        });
    }
    if let Some(spec) = join_spec {
        return match tree::plan_join(roots, spec, a.opt("lane-name"))? {
            Some((join_roots, plan)) => apply_writes(&join_roots, a, plan),
            None => Ok(0),
        };
    }
    let plan = tree::plan(roots, a)?;
    apply_writes(roots, a, plan)
}

/// `plan`'s own plan items, `tree.rs`'s (`t592` tranche 2, `d710`): the
/// same [`tree::opening_items`] and [`tree::closing_items`] every harness
/// already shows for the tree, rendered together so every column lines up
/// across both, with nothing of a harness's own between them -- `init`
/// writes none.
fn full_plan(here: &std::path::Path, plan: &tree::TreePlan) -> String {
    let mut s = heading(Stream::Out, "vivac init", here);
    // `t640`, point 10 bis: said before anything is written, the same
    // sentence `setup`'s own plan shows for the same reason --
    // `name_collision` is only ever `Some` once `--name`'s own value
    // already matches another project's effective name, and that is true
    // of `init`'s own plant exactly as it is of `setup`'s. One sentence,
    // read from `claude_code::name_collision_paragraph` rather than a
    // second copy of it (`f724`).
    if let Some(name) = &plan.name_collision {
        s.push_str(&super::claude_code::name_collision_paragraph(name));
    }
    let mut items = tree::opening_items(plan);
    items.extend(tree::closing_items(plan));
    s.push_str(&render_items(Stream::Out, &items));
    s
}

/// Every paragraph a run may still have to show past the plan itself --
/// the second-map hint, the tracked-`.vivac/events` warning, the tree-
/// above-this-one warning -- collected so every path out joins them the
/// same way (`d792`: one blank line between each, never a double one).
fn warning_paragraphs(plan: &tree::TreePlan) -> Vec<String> {
    let mut v = Vec::new();
    if !plan.unknown_product_warning.is_empty() {
        v.push(plan.unknown_product_warning.clone());
    }
    if plan.log_tracked {
        v.push(super::claude_code::tracked_git_warning());
    }
    if let Some(w) = &plan.above_warning {
        v.push(w.clone());
    }
    v
}

fn apply_writes(roots: &super::Roots, a: &Args, plan: tree::TreePlan) -> Result<i32, Failure> {
    let here = &roots.here;
    let nothing_to_write = !plan.vivac_missing
        && !plan.gitignore_missing
        && plan.lane.unchanged
        && !plan.lane.needs_lock
        && plan.lane.stale_worktrees.is_empty();

    let mut blocks = vec![full_plan(here, &plan).trim_end_matches('\n').to_string()];
    blocks.extend(warning_paragraphs(&plan));
    let plan_text = format!("{}\n\n", blocks.join("\n\n"));

    if a.has("dry-run") {
        outln!(
            "{}",
            super::claude_code::close_with(&plan_text, super::claude_code::DRY_RUN_LINE)
        );
        return Ok(0);
    }

    if nothing_to_write {
        // A real run, never `--dry-run`: noting the registry is
        // bookkeeping every ordinary command already does on a pure read,
        // not a write this promise is about (`tree::note_registry`'s own
        // doc).
        tree::note_registry(roots);
        outln!(
            "{}",
            super::claude_code::close_with(
                &plan_text,
                "Nothing to write: this project is already set up."
            )
        );
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::init_no_terminal_text(a)));
    }

    super::claude_code::print_plan(&plan_text);
    let proceed = a.has("yes") || super::ask("\nWrite it? [y/N] ");
    if !proceed {
        outln!("\nNothing written.");
        return Ok(0);
    }

    // Everything past this line is `tree.rs`'s own writes, exactly as
    // `claude_code::apply_writes` and `codex::apply_writes` already commit
    // them: the tree's own `.gitignore` first, in the one batch
    // `super::commit` writes all or nothing, and only then planting, the
    // lane and the version lock, none of which are plain file writes.
    let writes = tree::file_writes(roots, &plan);
    super::commit(&writes)?;
    tree::commit(roots, &plan, &writes)?;

    tree::note_registry(roots);
    tree::note_name(&plan);

    print!("\n{}", written_text(&plan));
    Ok(0)
}

/// What this run just wrote, in `init`'s own words, and the `Next:` block
/// every successful run ends with (`d792`): a tree that was already there
/// gets `claude_code::tree_paragraph`'s own sentence -- the one piece of
/// `claude_code`'s closing text that was never about a harness to begin
/// with. A fresh tree gets no first-node hint any more: the next step is
/// setting up the agent, and the agent is the one that writes nodes.
///
/// `f790`: the migrate nudge that used to end here moved onto `setup`,
/// the first write a fresh lane usually makes -- `init` only plants or
/// joins, and whether anything has been brought in yet is a question
/// about writing, not about that.
fn written_text(plan: &tree::TreePlan) -> String {
    let mut paragraphs = vec![crate::style::good(Stream::Out, "Written.")];
    if !plan.vivac_missing {
        let lane_declared = !plan.lane.unchanged || !plan.lane.stale_worktrees.is_empty();
        paragraphs.push(super::claude_code::tree_paragraph(
            "init",
            plan.gitignore_missing,
            lane_declared,
            plan.lane.needs_lock,
        ));
    }
    paragraphs.push(next_agent_block());
    format!("{}\n", paragraphs.join("\n\n"))
}

/// The `Next:` block a fresh plant or join always ends with: which harness
/// to set up next is the one thing this run cannot decide, and every setup
/// still shows its own migrate nudge once it runs, so `init` never has to
/// (`f790`).
fn next_agent_block() -> String {
    format!(
        "{} set up the agent you use\n\n  {}\n  {}",
        crate::style::bold(Stream::Out, "Next:"),
        crate::style::bold(Stream::Out, "vivac setup claude-code"),
        crate::style::bold(Stream::Out, "vivac setup codex")
    )
}

// ---------------------------------------------------------------------------
// `--undo`: takes off only what `init` itself wrote past planting -- this
// folder's own lane, and the `.vivac/` a join left holding nothing else
// (`f719`). Never the tree: a tree that holds events is never `init`'s to
// undo, the same promise `claude_code::undo` and `codex::undo` already keep
// for their own three files (`r515`).
// ---------------------------------------------------------------------------

fn undo(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let here = roots.here.as_path();
    let undo_lane = tree::undo_lane(roots)?;

    // `raw` is the field to gate on here, not `removable` (`d680`
    // regression, caught porting `tests/setup.rs`): a lane that exists but
    // has written to the tree is not removable either, and used to fall
    // into this same "nothing to undo" sentence as a folder with no lane
    // at all -- which is not true, and `undo_lane_lines` below already has
    // the right sentence for it.
    if undo_lane.raw.is_none() {
        // `d784`: a bare plant -- this folder's own `main`, never claimed
        // elsewhere -- writes no `.vivac/lane` file at all (`lane::read`'s
        // own "ordinary absence"), so `undo_lane` above has nothing to
        // say about it and `.vivac/` sat outside `--undo`'s reach no
        // matter how empty it still was. A join never lands here: it
        // always writes a lane file of its own, so `raw` is `Some` for
        // one.
        if crate::store::already_planted(here) {
            return undo_bare_tree(roots, a);
        }
        outln!("Nothing to undo: this folder carries no lane vivac init wrote.");
        return Ok(0);
    }

    let mut items = tree::vivac_dir_items(&undo_lane);
    items.extend(tree::undo_lane_items(&undo_lane));
    let mut s = heading(Stream::Out, "vivac init --undo", here);
    s.push_str(&render_items(Stream::Out, &items));

    if a.has("dry-run") {
        outln!(
            "{}",
            super::claude_code::close_with(&s, super::claude_code::DRY_RUN_LINE)
        );
        return Ok(0);
    }

    if !undo_lane.removable {
        // The lane stays -- `undo_lane_items` above already said why -- so
        // there is nothing left in this plan to confirm or to write.
        outln!(
            "{}",
            super::claude_code::close_with(
                &s,
                "Nothing written: this lane has written to the tree."
            )
        );
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::init_no_terminal_text(a)));
    }

    super::claude_code::print_plan(&s);
    let proceed = a.has("yes") || super::ask("\nUndo it? [y/N] ");
    if !proceed {
        outln!("\nNothing written.");
        return Ok(0);
    }

    let mut writes = vec![super::PlannedWrite::delete(
        undo_lane.path.clone(),
        undo_lane.raw.clone().unwrap_or_default(),
    )];
    // `f719`, point B: the `.gitignore` a join wrote alongside the lane,
    // gone the same commit -- absent when there never was one, the same as
    // `undo_lane.raw` above.
    if undo_lane.vivac_dir_removable {
        let gitignore = undo_lane.vivac_dir.join(crate::store::GITIGNORE);
        if let Ok(original) = std::fs::read(&gitignore) {
            writes.push(super::PlannedWrite::delete(gitignore, original));
        }
    }

    super::commit(&writes)?;

    // `f719`, point B: `.vivac/` itself, once the lane and its `.gitignore`
    // are both gone -- a join's own folder, holding nothing else, has no
    // reason left to carry one. Best-effort, and only once the commit
    // above is known to have succeeded.
    if undo_lane.vivac_dir_removable {
        super::claude_code::remove_if_empty(Some(&undo_lane.vivac_dir));
    }

    outln!("\nUndone. The tree in .vivac/ is untouched.");
    Ok(0)
}

/// `d784`: the one door `--undo` may remove a tree through -- this folder
/// holds it outright, planted by a bare `init` that wrote no lane file at
/// all, and nothing has ever written to it. Refuses instead the moment
/// the log carries even one capture event (`session::capture_count`,
/// reusing `d779`'s own definition of work): `lane.declared`,
/// `lane.claimed`, `session.started` and `where.changed` do not count,
/// since a folder can carry every one of those and still hold no work
/// anybody would miss.
fn undo_bare_tree(roots: &super::Roots, a: &Args) -> Result<i32, Failure> {
    let here = roots.here.as_path();
    let vivac_dir = here.join(crate::store::DIR);
    let store = crate::store::Store::open(roots.tree.clone())?;
    let (events, _) = store.read_all()?;
    let writes = crate::session::capture_count(&events);
    if writes > 0 {
        outln!(
            "Nothing to undo: the tree in .vivac/ already holds work ({writes} writes), and \
             init --undo never removes a tree that does."
        );
        return Ok(0);
    }

    let items = vec![
        PlanItem::new(
            "remove",
            tree::VIVAC_LABEL,
            "the tree init planted here: it holds no work yet",
        ),
        PlanItem::new("forget", "~/.vivac/projects", "this project"),
    ];
    let mut s = heading(Stream::Out, "vivac init --undo", here);
    s.push_str(&render_items(Stream::Out, &items));

    if a.has("dry-run") {
        outln!(
            "{}",
            super::claude_code::close_with(&s, super::claude_code::DRY_RUN_LINE)
        );
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::init_no_terminal_text(a)));
    }

    super::claude_code::print_plan(&s);
    let proceed = a.has("yes") || super::ask("\nUndo it? [y/N] ");
    if !proceed {
        outln!("\nNothing written.");
        return Ok(0);
    }

    // Read before removing: once `.vivac/` is gone, so is the log
    // `first_event_id` would otherwise read to find it.
    let project_id = crate::store::first_event_id(&roots.tree);
    std::fs::remove_dir_all(&vivac_dir).map_err(|e| {
        Failure::Io(std::io::Error::other(format!(
            "{} could not be removed ({e})",
            vivac_dir.display()
        )))
    })?;
    if let (Some(store_dir), Some(project_id)) = (crate::store::store_dir(), project_id) {
        crate::registry::forget(&store_dir, &project_id);
    }

    outln!("\nUndone. There is no tree here any more.");
    Ok(0)
}
