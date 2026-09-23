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

/// `plan`'s own two-column lines, `tree.rs`'s (`t592` tranche 2, `d710`):
/// the same [`tree::opening_lines`] and [`tree::closing_lines`] every
/// harness already shows for the tree, with nothing of a harness's own
/// between them -- `init` writes none.
fn full_plan(here: &std::path::Path, plan: &tree::TreePlan) -> String {
    let mut s = format!("  vivac init, in {}\n\n", here.display());
    // `t640`, point 10 bis: said before anything is written, the same
    // sentence `setup`'s own plan shows for the same reason --
    // `name_collision` is only ever `Some` once `--name`'s own value
    // already matches another project's effective name, and that is true
    // of `init`'s own plant exactly as it is of `setup`'s. One sentence,
    // read from `claude_code::name_collision_paragraph` rather than a
    // second copy of it (`f724`): two copies of a hand-split line is how
    // one of them keeps the break a long name outgrows.
    if let Some(name) = &plan.name_collision {
        s.push_str(&super::claude_code::name_collision_paragraph(name));
    }
    s.push_str(&tree::opening_lines(plan));
    s.push_str(&tree::closing_lines(plan));
    s
}

fn apply_writes(roots: &super::Roots, a: &Args, plan: tree::TreePlan) -> Result<i32, Failure> {
    let here = &roots.here;
    let nothing_to_write = !plan.vivac_missing
        && !plan.gitignore_missing
        && plan.lane.unchanged
        && !plan.lane.needs_lock
        && plan.lane.stale_worktrees.is_empty();

    let plan_text = full_plan(here, &plan);

    if a.has("dry-run") {
        outln!(
            "{plan_text}{}\n  Nothing written: --dry-run.",
            plan.unknown_product_warning
        );
        if plan.log_tracked {
            print!("{}", super::claude_code::tracked_git_warning());
        }
        if let Some(w) = &plan.above_warning {
            print!("{w}");
        }
        return Ok(0);
    }

    if nothing_to_write {
        // A real run, never `--dry-run`: noting the registry is
        // bookkeeping every ordinary command already does on a pure read,
        // not a write this promise is about (`tree::note_registry`'s own
        // doc).
        tree::note_registry(roots);
        outln!("{plan_text}  Nothing to write: this project is already set up.");
        if plan.log_tracked {
            print!("{}", super::claude_code::tracked_git_warning());
        }
        if let Some(w) = &plan.above_warning {
            print!("{w}");
        }
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::init_no_terminal_text(a)));
    }

    print!("{plan_text}{}", plan.unknown_product_warning);
    let proceed = a.has("yes") || super::ask("\n  Write it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
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
    if plan.log_tracked {
        print!("{}", super::claude_code::tracked_git_warning());
    }
    if let Some(w) = &plan.above_warning {
        print!("{w}");
    }
    Ok(0)
}

/// What this run just wrote, in `init`'s own words: a fresh tree gets the
/// same first-node hint a bare `vivac init` already gives, and a tree that
/// was already there gets `claude_code::tree_paragraph`'s own sentence --
/// the one piece of `claude_code`'s closing text that was never about a
/// harness to begin with.
///
/// `d723` piece B carried the migration nudge here too, not just the tree
/// side: planting and joining are what `MIGRATE_PARAGRAPHS` and
/// `JOIN_MIGRATE_PARAGRAPHS` were always keyed on, and both are `init`'s
/// alone now. The text is unchanged; only which module shows it moved,
/// with the writes it was always describing.
fn written_text(plan: &tree::TreePlan) -> String {
    let mut s = String::from("  Written.\n");
    if plan.vivac_missing {
        s.push_str("\n  First node:  vivac push \"<title>\" --why \"<reason>\"\n");
        s.push_str(super::claude_code::MIGRATE_PARAGRAPHS);
    } else {
        let lane_declared = !plan.lane.unchanged || !plan.lane.stale_worktrees.is_empty();
        s.push_str(&super::claude_code::tree_paragraph(
            "init",
            plan.gitignore_missing,
            lane_declared,
            plan.lane.needs_lock,
        ));
        // `f678`/`d683`: the argument for staying quiet here was the
        // **tree**'s, which a join finds already there and may already
        // hold content for. It says nothing about the folder, which
        // arrives with its own instruction files, its own harness memory
        // and its own documents, and joining a tree never reads any of
        // that.
        if plan.lane.is_new {
            s.push_str(super::claude_code::JOIN_MIGRATE_PARAGRAPHS);
        }
    }
    s
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
        outln!("  Nothing to undo: this folder carries no lane vivac init wrote.");
        return Ok(0);
    }

    let mut s = format!("  vivac init --undo, in {}\n\n", here.display());
    s.push_str(&tree::vivac_dir_lines(&undo_lane));
    s.push_str(&tree::undo_lane_lines(&undo_lane));
    s.push('\n');

    if a.has("dry-run") {
        outln!("{s}  Nothing written: --dry-run.");
        return Ok(0);
    }

    if !undo_lane.removable {
        // The lane stays -- `undo_lane_lines` above already said why -- so
        // there is nothing left in this plan to confirm or to write.
        outln!("{s}  Nothing written: this lane has written to the tree.");
        return Ok(0);
    }

    if !a.has("yes") && !super::stdin_is_terminal() {
        return Err(Failure::Model(super::init_no_terminal_text(a)));
    }

    print!("{s}");
    let proceed = a.has("yes") || super::ask("  Undo it? [y/N] ");
    if !proceed {
        outln!("\n  Nothing written.");
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

    outln!("  Undone. The tree in .vivac/ is untouched.");
    Ok(0)
}
