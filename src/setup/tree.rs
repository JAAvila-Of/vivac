//! The tree's own side of planting: `init`'s alone since `d723` piece B.
//!
//! `t592` tranche 2, `d710`, built this shared by both harnesses; `d723`
//! piece B took planting, joining and `--name` away from `setup` entirely,
//! so `init.rs` is the only caller left for [`plan`], [`plan_join`],
//! [`opening_lines`], [`closing_lines`], [`file_writes`] and [`commit`].
//! Nothing here proposes a harness's own command any more, `f717`
//! dissolved: every message this module raises names `vivac init`, which
//! reads the same wherever an agent is opened.
//!
//! [`undo_lane`], [`undo_lane_lines`] and [`vivac_dir_lines`] moved in for
//! `--undo`, piece C of the same tranche (`r515`), when both harnesses'
//! own `--undo` still took a joined folder's lane back with them. `d723`
//! piece B ended that too: `init --undo` is their only caller now, the
//! lane and the tree having never been `setup`'s to undo.
//!
//! What stays behind in `claude_code.rs`, `pub(super)` for `init.rs` to
//! call: `tree_below_join_refusal`, one refusal `--join`'s own preamble
//! still raises, and `piece_line`/`sub_line`, the two-column rendering
//! [`opening_lines`] and [`closing_lines`] still draw through.

use crate::args::Args;
use crate::failure::Failure;
use std::path::{Path, PathBuf};

pub(super) const VIVAC_LABEL: &str = ".vivac/";
const GITIGNORE_LABEL: &str = ".vivac/.gitignore";
pub(super) const LANE_LABEL: &str = ".vivac/lane";

// ---------------------------------------------------------------------------
// `--name`: naming the product on purpose (`t640`), rather than always
// deriving it from whichever folder holds the tree.
// ---------------------------------------------------------------------------

/// The most `--name` may be, once trimmed. Generous on purpose: this is a
/// product's own name, not a title with a budget of its own.
const NAME_MAX_LEN: usize = 100;

/// The minimal shape `--name`'s own value has to have before it ever
/// reaches the redaction guard (`t640`, point 5): not empty once
/// surrounding space is trimmed, one line -- the same rule
/// `ops::validate_arm_text` already holds an arm to -- and at most
/// [`NAME_MAX_LEN`] characters. Spaces inside are fine: the registry
/// already knows how to quote a name that has them
/// (`registry::quote_if_needed`).
fn validate_name(raw: &str) -> Result<&str, Failure> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Failure::usage("--name cannot be empty."));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(Failure::usage(
            "--name is one line: it cannot carry a newline or another control \
             character.",
        ));
    }
    if trimmed.chars().count() > NAME_MAX_LEN {
        return Err(Failure::usage(format!(
            "--name is {} characters long; the limit is {NAME_MAX_LEN}.",
            trimmed.chars().count()
        )));
    }
    Ok(trimmed)
}

/// `--name`'s own value, validated (point 5) and passed through the same
/// redaction guard `folder_name` already reads a derived name through
/// (point 4): `Ok(None)` when `--name` was not given at all, and every
/// other outcome already carries the right exit code -- a usage error
/// (2) for a shape the guard never gets to see, or `Failure::Redaction`
/// (3) for one it refuses.
fn requested_name(a: &Args) -> Result<Option<String>, Failure> {
    let Some(raw) = a.opt("name") else {
        return Ok(None);
    };
    let trimmed = validate_name(raw)?;
    match crate::redact::check_field("project name", trimmed) {
        Some(finding) => Err(Failure::Redaction(Box::new(finding))),
        None => Ok(Some(trimmed.to_string())),
    }
}

/// The product name this run's own plan shows, on the line that names a
/// lane (`t640`, point 9): `requested`'s own value where this run is
/// planting with one, or the tree's own effective name otherwise.
/// `registry::effective_name` already falls back to `tree`'s own folder
/// name once there is nothing on file for it -- exactly a fresh plant's
/// own case, since nothing can be on file yet for a tree that does not
/// exist.
fn product_name_for_plan(tree: &Path, requested: Option<&str>) -> Option<String> {
    if let Some(name) = requested {
        return Some(name.to_string());
    }
    let store_dir = crate::store::store_dir()?;
    crate::registry::effective_name(&store_dir, tree)
}

/// `name`, quoted the way every other sentence names a product, or a
/// placeholder once the redaction guard has withheld it --
/// `registry::label_for`'s own shape, for a product rather than a folder,
/// since "another folder" reads wrong beside a lane's own name.
fn product_label(name: Option<&str>) -> String {
    match name {
        Some(n) => format!("\"{n}\""),
        None => "this product".to_string(),
    }
}

/// Saves `plan`'s own requested name into the registry as its tree's own
/// (`t640`, point 6), once this run has given it a first event to be keyed
/// by. Quiet when there is nothing to key it by yet, or nowhere to save it
/// to -- the same promise `note_registry` already makes, and a name is no
/// different: the registry is a comfort a command can do without.
pub(super) fn note_name(plan: &TreePlan) {
    let Some(name) = plan.requested_name.as_deref() else {
        return;
    };
    let Some(store_dir) = crate::store::store_dir() else {
        return;
    };
    let Some(project_id) = crate::store::first_event_id(&plan.tree) else {
        return;
    };
    crate::registry::set_name(&store_dir, &project_id, name);
}

// ---------------------------------------------------------------------------
// Recognizing an existing product, before planting a second map of it:
// `t594` §4.5, case 3 -- reached only when there is no tree above `here`
// at all. Checked in this order because §4.5.1 describes a state of the
// disk that has to be fixed before either of the other two questions
// means anything: a tree below (`trees_below`), then a product this
// machine's registry already tracks (`sharing_repos`).
// ---------------------------------------------------------------------------

/// The deepest a nested tree can sit beneath the folder being set up, the
/// same two levels `repos::scan` fixes for a repository -- and for the
/// same reason: it also keeps a symlink cycle from running away with the
/// walk.
const TREE_SCAN_DEPTH: u32 = 2;

/// Every `.vivac/` holding a tree (`events` or `config`) strictly inside
/// `folder`: the same walk `repos::scan` does over `.git` -- two levels
/// down, never descending into a repository or into a `.vivac/` already
/// found -- but looking for a tree instead of a repository, and never
/// checking `folder` itself. That last part used to be unreachable rather
/// than absent: the only caller skipped calling this at all once `folder`
/// already had a tree of its own. `t594` made that call
/// reachable, and it surfaced the gap -- calling this on a folder that
/// already holds a tree used to report the folder itself as a tree
/// sitting "below" it.
pub(super) fn trees_below(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for sub in child_folders(folder) {
        walk_for_trees(&sub, 1, &mut found);
    }
    found.sort();
    found
}

/// `dir`'s own immediate subdirectories, `.vivac/` excluded, in a fixed
/// order: the one piece `trees_below` and `walk_for_trees`'s own
/// recursive step both need.
fn child_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut subdirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| p.file_name().is_some_and(|n| n != crate::store::DIR))
        .collect();
    subdirs.sort();
    subdirs
}

fn walk_for_trees(dir: &Path, depth: u32, found: &mut Vec<PathBuf>) {
    if crate::store::already_planted(dir) {
        found.push(dir.to_path_buf());
        // Never descend into a tree already found: whatever sits inside
        // it belongs to that tree, not to this walk.
        return;
    }
    if dir.join(".git").exists() {
        return;
    }
    if depth == TREE_SCAN_DEPTH {
        return;
    }
    for sub in child_folders(dir) {
        walk_for_trees(&sub, depth + 1, found);
    }
}

/// `path`'s own folder name, or `None` when the redaction guard rejects
/// it: this text reaches an agent's context (`d600`), the same rule
/// `registry::folder_name` already follows for a copy's folder.
pub(super) fn guarded_folder_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    match crate::redact::check_field("folder name", &name) {
        Some(_) => None,
        None => Some(name),
    }
}

/// §6.4: a tree already sitting inside this folder. Named, unless the
/// guard withholds a name; with two or more, the withheld ones are simply
/// left out rather than replaced one by one.
pub(super) fn tree_below_refusal(paths: &[PathBuf]) -> Failure {
    let names: Vec<Option<String>> = paths.iter().map(|p| guarded_folder_name(p)).collect();
    if let [only] = names.as_slice() {
        let label = crate::registry::label_for(only.as_deref());
        return Failure::Model(format!(
            "  There is already a tree inside this folder, in {label}.\n  \
             Planting another one here would split this project: sessions opened in\n  \
             {label} would use that one, and the rest this one.\n\n  \
             Move that tree up here, then run init again. From inside {label}:\n      \
             vivac relocate .."
        ));
    }
    let quoted: Vec<String> = names
        .iter()
        .filter_map(|n| n.as_deref())
        .map(|n| format!("\"{n}\""))
        .collect();
    let quoted_refs: Vec<&str> = quoted.iter().map(String::as_str).collect();
    let where_clause = if quoted_refs.is_empty() {
        "under names this tool will not write down".to_string()
    } else {
        format!("in {}", super::claude_code::join_with_and(&quoted_refs))
    };
    Failure::Model(format!(
        "  There are trees inside this folder, {where_clause}.\n  \
         vivac cannot merge trees: keep one per product, move it up here with\n  \
         vivac relocate, and leave the others as they are."
    ))
}

/// `t594` §4.5, case 3's own second refusal: this folder's repositories
/// already belong to a project the registry tracks. Skipped for
/// `bypass_registered` -- `--new-tree` (`t594` §4.5's own escape for two
/// forks that share a root commit) -- and skipped when there is a tree
/// above `here` at all, since with one this is an ordinary join and the
/// product question does not arise.
fn refuse_second_map(roots: &super::Roots, bypass_registered: bool) -> Result<(), Failure> {
    if roots.located.is_some() {
        return Ok(());
    }
    if bypass_registered {
        return Ok(());
    }
    let (here_repos, _excluded) = filtered_repos(crate::repos::scan(&roots.here));
    let root_commits: Vec<String> = here_repos.iter().filter_map(|r| r.root.clone()).collect();
    if root_commits.is_empty() {
        return Ok(());
    }
    let Some(store_dir) = crate::store::store_dir() else {
        return Ok(());
    };
    let best = crate::registry::sharing_repos(&store_dir, &root_commits)
        .into_iter()
        .find(|s| !crate::anchor::same_folder(&s.root, &roots.here));
    match best {
        Some(sharing) => Err(product_registered_refusal(&sharing, &here_repos)),
        None => Ok(()),
    }
}

/// §6.3: this folder's own repositories already belong to a project the
/// registry tracks. `here_repos` names the repositories printed --
/// **this** folder's own, per `repos::scan`, never the other project's.
///
/// Names three ways out, not two (`d680`): the correct one when the tree
/// belongs here rather than where it landed -- `relocate` it into place
/// first, then join -- used to go unnamed, and the two that were left,
/// joining from here or planting a second tree, cost a reader who followed
/// them the tree they meant to keep. The `relocate` line sits before
/// `--new-tree`'s own: the escape that keeps the tree, read before the one
/// that gives it up.
fn product_registered_refusal(
    sharing: &crate::registry::Sharing,
    here_repos: &[crate::event::Repo],
) -> Failure {
    let mut repo_names: Vec<&str> = here_repos
        .iter()
        .filter(|r| {
            r.root
                .as_deref()
                .is_some_and(|root| sharing.shared.iter().any(|s| s == root))
        })
        .map(|r| r.path.as_str())
        .collect();
    repo_names.sort_unstable();
    // `f677`: "." is what `Repo::relative` writes when the folder itself
    // is the repository, and printed bare it disappears into the
    // sentence's own closing period -- named here instead, the one place
    // this list turns into words a person reads.
    let repo_list = repo_names
        .iter()
        .map(|p| if *p == "." { "this folder itself" } else { p })
        .collect::<Vec<_>>()
        .join(", ");
    // `d723` piece B: proposes `vivac init`, which names no harness, rather
    // than `vivac setup <harness>` (`f717` dissolved) -- planting is
    // `init`'s question wherever this refusal is reached from.
    match &sharing.name {
        Some(name) => Failure::Model(format!(
            "  Some repositories here are already tracked by project \"{name}\":\n      \
             {repo_list}\n  \
             Planting another tree would give this product two maps.\n\n  \
             To work on {name} from this folder:\n      \
             vivac init --join {}\n  \
             If the tree should live here instead, run this in the folder that holds it:\n      \
             vivac relocate <path to this folder>\n  \
             To plant a separate tree anyway:\n      \
             vivac init --new-tree",
            crate::registry::quote_if_needed(name)
        )),
        None => Failure::Model(format!(
            "  Some repositories here are already tracked by another project on this\n  \
             machine:\n      \
             {repo_list}\n  \
             Planting another tree would give this product two maps.\n\n  \
             To work on it from this folder, give the path to its folder:\n      \
             vivac init --join <path to that folder>\n  \
             If the tree should live here instead, run this in the folder that holds it:\n      \
             vivac relocate <path to this folder>\n  \
             To plant a separate tree anyway:\n      \
             vivac init --new-tree"
        )),
    }
}

/// `f676`/`d682`: the guard above only speaks when this folder's own
/// repositories share a root commit with a project the registry already
/// tracks -- read the other way round, when the registry knows other
/// products and this folder shares a root commit with **none** of them,
/// it says nothing at all. A folder that genuinely is a new product and
/// one whose repositories the registry simply never learned about yet
/// look identical from here, and only the first is what a silent plant
/// should mean.
///
/// A warning, never a refusal: it changes nothing about what this run
/// does, so it is checked independent of `--new-tree`, which only bypasses
/// the guard above. `None` once the registry has nothing on file yet --
/// there is nothing for this folder to fail to share with.
fn second_map_hint(here: &Path) -> Option<String> {
    let store_dir = crate::store::store_dir()?;
    if crate::registry::roots(&store_dir).is_empty() {
        return None;
    }
    let (here_repos, _excluded) = filtered_repos(crate::repos::scan(here));
    let root_commits: Vec<String> = here_repos.iter().filter_map(|r| r.root.clone()).collect();
    if !crate::registry::sharing_repos(&store_dir, &root_commits).is_empty() {
        return None;
    }
    Some(format!(
        "{} Nothing here shares a repository with the projects vivac already tracks, so \
         it cannot tell whether this is one of them. If it is, stop and use {} instead.",
        crate::style::warn(crate::style::Stream::Out, "This plants a new product."),
        crate::style::bold(crate::style::Stream::Out, "--join <name>")
    ))
}

/// §6.5: this folder's own tree -- freshly planted, or the closer one it
/// just joined -- itself sits inside yet another one, found by continuing
/// the very same upward walk past it. `t594` §4.5, case 2's own extra
/// check: it never blocks anything, and it is checked for a fresh plant
/// too, where it always reads `None` -- `store::locate` already walked
/// every ancestor of `here` looking for exactly this, and found nothing,
/// or there would be a tree above to join instead of planting.
fn tree_root_above(tree_root: &Path) -> Option<PathBuf> {
    let mut d = tree_root.to_path_buf();
    while d.pop() {
        if crate::store::already_planted(&d) {
            return Some(d);
        }
    }
    None
}

fn tree_above_warning(name: Option<&str>) -> String {
    let label = crate::registry::label_for(name);
    format!(
        "This tree sits inside another one, in folder {label}. Sessions opened above \
         this folder use that one: keep one tree per product."
    )
}

// ---------------------------------------------------------------------------
// The lane: `t594` §4.5, joining the tree above rather than planting a
// second one.
// ---------------------------------------------------------------------------

/// What this run has to do about the lane `roots.here` is, worked out
/// before anything is written so the plan can say it.
pub(super) struct LanePlan {
    lane_id: String,
    pub(super) name: String,
    repos: Vec<crate::event::Repo>,
    /// This folder does not carry `.vivac/lane` yet, so this run has to
    /// write it before it can declare (`t594` §4.5.2, case (c)). The id
    /// this points back at is minted here, since it never depends on the
    /// tree's own state; the project it points back at does, and is
    /// worked out at write time instead (`write_lane`).
    pub(super) is_new: bool,
    /// Whether the config still needs `lock_lanes_in_config`: absent for
    /// a tree that does not exist yet, which always needs it once
    /// planted, and read off the existing one otherwise.
    pub(super) needs_lock: bool,
    /// The tree already says exactly this (`t594` §4.5.2, case (e)):
    /// nothing to write, and running `setup` twice in a row does not
    /// leave two events behind.
    pub(super) unchanged: bool,
    /// How many repositories the redaction guard kept out, and the first
    /// rule that caught one. `d600`: they are still missing from the
    /// declaration, and that is said rather than left silent, without
    /// repeating which repository it was.
    pub(super) excluded: Option<(usize, &'static str)>,
    /// Other lanes in this tree that joined as a worktree of one of these
    /// repositories while it still had no root commit recorded, and are
    /// still declared with none (`f609`): each one's id, its name kept as
    /// it was, and the repository it shares with this folder's own, now
    /// carrying the root commit this run just found for it.
    pub(super) stale_worktrees: Vec<(String, String, crate::event::Repo)>,
}

/// `scanned`, filtered through the redaction guard (`d600`): what is left
/// to declare, and the count and first rule of whatever it kept out.
/// Shared by declaring a lane's own folder and by declaring `main` on the
/// tree's own folder, whether that happens because someone asked for it
/// or because `ensure_first_event` needs to seed it -- one piece of work,
/// one place that does it.
fn filtered_repos(
    scanned: Vec<crate::event::Repo>,
) -> (Vec<crate::event::Repo>, Option<(usize, &'static str)>) {
    let mut excluded_count = 0usize;
    let mut excluded_rule: Option<&'static str> = None;
    let repos = scanned
        .into_iter()
        .filter(
            |r| match crate::redact::check_field("repository path", &r.path) {
                Some(f) => {
                    excluded_count += 1;
                    excluded_rule.get_or_insert(f.rule);
                    false
                }
                None => true,
            },
        )
        .collect();
    (
        repos,
        (excluded_count > 0).then(|| (excluded_count, excluded_rule.unwrap())),
    )
}

/// The tree at `tree_root`, folded once. A `.vivac/` that is empty or not
/// there at all (`f566`, or no tree yet) folds to `Tree::default`, which
/// answers every question below the same way absence always has --
/// `main_claimed: false`, nothing declared -- so callers never need to
/// know which kind of "nothing" they got. Shared by `plan_lane`'s own
/// decision and by `existing_lane`, so a `setup` run folds the tree once
/// rather than once per question asked of it.
fn fold_tree(tree_root: &Path) -> crate::model::Tree {
    let (events, broken) =
        crate::store::read_all_from(&tree_root.join(crate::store::DIR).join(crate::store::LOG))
            .unwrap_or_default();
    crate::model::fold(&events, broken)
}

/// Whether `lane_id` has changed `tree_root`'s own tree beyond declaring
/// itself: `LaneState::seq_change` already skips the context events
/// (`lane.declared`, `lane.claimed`, `where.changed`) a join writes on a
/// lane's own behalf, so a lane that only ever joined and never pushed,
/// popped or noted anything answers `false` here. `--undo`'s own use is
/// the one thing this decides: a lane that never wrote owns no history for
/// removing `.vivac/lane` to orphan (`d680`).
pub(super) fn lane_has_written(tree_root: &Path, lane_id: &str) -> bool {
    fold_tree(tree_root)
        .lanes
        .get(lane_id)
        .is_some_and(|s| s.seq_change != 0)
}

/// What the tree already says about `lane_id`, read without writing
/// anything: `Store::open` would fill a missing `config` in on its own,
/// and that write is one `--dry-run` must never trigger just by asking
/// what a tree is on (`t594`). `config_version` reads
/// `ConfigVersion::One` for a tree with no config at all -- the same
/// answer `Store::open` would settle on for a tree with no lane and no
/// pillar or rule either, so `needs_lock` comes out right either way
/// without this having to know why the file is missing.
struct ExistingLane {
    config_version: crate::store::ConfigVersion,
    declared: Option<(String, Vec<crate::event::Repo>)>,
}

fn existing_lane(tree: &Path, lane_id: &str, folded: &crate::model::Tree) -> ExistingLane {
    ExistingLane {
        config_version: crate::store::peek_config_version(tree)
            .unwrap_or(crate::store::ConfigVersion::One),
        declared: folded
            .lanes
            .get(lane_id)
            .map(|s| (s.name.clone(), s.repos.clone())),
    }
}

/// `t594` §4.5.2's five cases, decided from `roots` alone: whether there is
/// a tree above `here` at all, and whether `here` already carries its own
/// `.vivac/lane` (`Located::lane_dir == here`, rather than some ancestor's)
/// -- plus a sixth, `t594`: `here` holds the tree, has no
/// lane file, and `main` has already been claimed by another folder
/// (`main_claimed`). Declaring `main` there again would be a lie about
/// where `main` actually lives, so this mints `here` a lane of its own
/// instead, the same as any other folder that never had one.
fn plan_lane(roots: &super::Roots, lane_name: Option<&str>) -> LanePlan {
    let (repos, excluded) = filtered_repos(crate::repos::scan(&roots.here));

    let folder_name = roots
        .here
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `--lane-name` (`t594` §4.5's own `--lane-name <name>`), or this
    // folder's own name when nobody named it: the word `declared_name`
    // guards below either way, for every lane -- `main` included since
    // `d624`, which made `main_lane` fall back to this same folder name
    // instead of staying literally `main` when nobody names it. Accepting
    // `--lane-name` and silently doing nothing with it -- §2.3 names both
    // planting and joining -- would be worse than either using it or
    // refusing it outright (`t594`).
    let requested_name = lane_name.unwrap_or(&folder_name);
    let here_has_its_own_vivac = roots
        .located
        .as_ref()
        .is_some_and(|l| l.lane_dir == roots.here);
    // Folded once, ahead of the decision below, which needs to know
    // whether `main` has already been claimed elsewhere before it can
    // tell "here is main" apart from "here holds the tree, but is not
    // main any more" -- and `existing_lane`, further down, needs the
    // very same fold.
    let folded = fold_tree(&roots.tree);

    let (lane_id, name, is_new) = match &roots.located {
        None => main_lane(lane_name, &folder_name),
        Some(l) if here_has_its_own_vivac && l.lane.is_none() && !folded.main_claimed => {
            main_lane(lane_name, &folder_name)
        }
        Some(l) if here_has_its_own_vivac && l.lane.is_none() => {
            let id = crate::lane::new_id();
            let name = crate::lane::declared_name(&id, requested_name);
            (id, name, true)
        }
        Some(l) if here_has_its_own_vivac => {
            let id = l.lane.as_ref().unwrap().id.clone();
            let name = crate::lane::declared_name(&id, requested_name);
            (id, name, false)
        }
        Some(_) => {
            let id = crate::lane::new_id();
            let name = crate::lane::declared_name(&id, requested_name);
            (id, name, true)
        }
    };

    let existing = existing_lane(&roots.tree, &lane_id, &folded);
    let needs_lock = existing.config_version != crate::store::ConfigVersion::Lanes;
    let unchanged = existing
        .declared
        .is_some_and(|(n, r)| n == name && r == repos);
    let stale_worktrees = stale_worktree_roots(&roots.here, &repos, &lane_id, &folded);

    LanePlan {
        lane_id,
        name,
        repos,
        is_new,
        needs_lock,
        unchanged,
        excluded,
        stale_worktrees,
    }
}

/// The already-declared worktree lanes one of `here`'s own repositories
/// explains but never told: each one joined while its matching repository
/// here still had no root commit recorded, copied that absence forward
/// (`ops::resolve_whose`), and nothing has revisited it since -- the
/// tree's own fold has no way to tell a worktree lane's folder apart from
/// any other lane's, so this reads it straight off git's own worktree
/// bookkeeping instead of guessing at it from the fold alone (`f609`).
///
/// Skips `lane_id`: a repository whose own root just changed already gets
/// declared by the caller through the ordinary path, and finding it here
/// too would only redeclare it a second time under the same identity.
fn stale_worktree_roots(
    here: &Path,
    repos: &[crate::event::Repo],
    lane_id: &str,
    folded: &crate::model::Tree,
) -> Vec<(String, String, crate::event::Repo)> {
    let mut out = Vec::new();
    for repo in repos {
        let Some(root) = &repo.root else { continue };
        for worktree in linked_worktrees_of(&here.join(&repo.path)) {
            let Ok(Some(lane)) = crate::lane::read(&worktree.join(crate::store::DIR)) else {
                continue;
            };
            if lane.id == lane_id {
                continue;
            }
            let Some(state) = folded.lanes.get(&lane.id) else {
                continue;
            };
            let pending_shape = [crate::event::Repo {
                path: ".".to_string(),
                root: None,
            }];
            if state.repos == pending_shape {
                out.push((
                    lane.id,
                    state.name.clone(),
                    crate::event::Repo {
                        path: ".".to_string(),
                        root: Some(root.clone()),
                    },
                ));
            }
        }
    }
    out
}

/// Every worktree git still links to the repository at `repo_root`, read
/// off `.git/worktrees/*/gitdir` rather than spawning `git worktree list`:
/// one file read costs nothing beside the `git rev-list` `repos::scan`
/// already pays for this same folder, and a worktree git has pruned
/// leaves no `gitdir` file behind for this to find in the first place
/// (`f609`).
fn linked_worktrees_of(repo_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(repo_root.join(".git").join("worktrees")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("gitdir")).ok())
        .filter_map(|raw| PathBuf::from(raw.trim()).parent().map(Path::to_path_buf))
        .collect()
}

/// `main`'s id never changes. Its name falls back to this folder's own
/// name exactly like every other lane's, unless `lane_name` asks for a
/// different one (`d624`).
fn main_lane(lane_name: Option<&str>, folder_name: &str) -> (String, String, bool) {
    let requested_name = lane_name.unwrap_or(folder_name);
    let name = crate::lane::declared_name(crate::lane::MAIN, requested_name);
    (crate::lane::MAIN.to_string(), name, false)
}

/// The tree's own first event id, seeding one when there is none: a brand
/// new lane's own `.vivac/lane` needs a stable id to point back at
/// (`resolve_lane`, `store.rs` -- it reads a tree's first line as the
/// cheap fingerprint that ties a lane to the right tree), and there is
/// nothing stable to point at in a tree that has never written anything,
/// which a tree fresh out of `init` or a bare plant still is.
///
/// The seed is the tree's own implicit `main` declaring itself for real,
/// with its own folder's actual repositories -- the same walk declaring
/// `main` by hand would do, and not a placeholder: task 8 decides with
/// this list whether a linked worktree is one of the lane's own
/// repositories or a lane apart, and an empty list would hand it the
/// wrong answer (`t594`). Taken and released under its
/// own lock, before the new lane's own lock is taken, since a second
/// attempt to lock the same file from this same process would otherwise
/// wait on itself.
///
/// If this write succeeds and the log's first line still will not parse
/// as an id right after, that is not this call's own failure to undo --
/// it already appended a real event and already locked the config, and
/// the log only ever grows. The error says so, since the caller cannot.
fn ensure_first_event(tree: &Path) -> Result<String, Failure> {
    if let Some(id) = crate::store::first_event_id(tree) {
        return Ok(id);
    }
    let (repos, _excluded) = filtered_repos(crate::repos::scan(tree));
    let store = crate::store::Store::open(tree.to_path_buf())?;
    let mut ctx = crate::ops::Ctx::load_for_write(
        store,
        crate::ops::Whose::Declared(crate::lane::MAIN.to_string(), tree.to_path_buf()),
    )?;
    ctx.lock_for_write()?;
    crate::ops::declare_lane(&mut ctx, crate::lane::MAIN.to_string(), repos)?;
    crate::store::first_event_id(tree).ok_or_else(|| {
        Failure::Io(std::io::Error::other(
            "this folder's main lane was just declared to give the tree a first \
             event, and locked its config to match, and the tree's own first \
             line is still unreadable after that -- the log only ever grows, \
             so what was just written stays either way",
        ))
    })
}

/// What this run actually does, in order: this folder's own `.vivac/lane`
/// on disk first -- only for a brand new lane, and with no lock held over
/// it at all -- and only then `declare_lane`, which takes the write lock,
/// locks the config and emits `lane.declared` together.
///
/// That is *not* `t594` §4.5.2's own order, which puts the file inside the
/// lock and after the config is closed. This one is at least as safe: if
/// the process dies between the file and the lock, the folder already
/// knows whose thread it is and the tree finds out the moment the fold
/// sees the matching event, which is exactly what dying between the file
/// and the event -- the ordering the spec itself calls safe -- already
/// leaves behind. If it dies between the file and the *config* closing
/// specifically, the tree does not have a lane event yet either, so an
/// older vivac reading it in between is not being lied to. What the file
/// must never do is land *after* the event: that is the one ordering that
/// leaves a folder signing as `main` while the tree already says
/// otherwise, and nothing here permits it.
/// `write_lane_inner`'s own errors, wrapped so a fresh lane's own file
/// never survives a failure past it (`t565` §7.7, ported to the scope
/// `init` has now, `d723` piece B): `write_lane_inner` writes this
/// folder's own `.vivac/lane` before it ever reaches the target's own
/// log, and the two used to have no shared fate -- a failure in the
/// second left the first sitting on disk, claiming a lane the target's
/// log never received.
fn write_lane(roots: &super::Roots, plan: &LanePlan) -> Result<(), Failure> {
    match write_lane_inner(roots, plan) {
        Ok(()) => Ok(()),
        Err(e) => {
            if plan.is_new {
                undo_fresh_lane_file(roots);
            }
            Err(e)
        }
    }
}

/// What this run actually does, in order, once `write_lane` above has a
/// failure of its own to clean up around: this folder's own
/// `.vivac/lane` on disk first -- only for a brand new lane, and with no
/// lock held over it at all -- and only then `declare_lane`, which takes
/// the write lock, locks the config and emits `lane.declared` together.
fn write_lane_inner(roots: &super::Roots, plan: &LanePlan) -> Result<(), Failure> {
    if plan.is_new {
        let project = ensure_first_event(&roots.tree)?;
        let lane = crate::lane::Lane {
            version: 1,
            id: plan.lane_id.clone(),
            project,
        };
        crate::lane::write(&roots.here.join(crate::store::DIR), &lane)?;
    }

    let store = crate::store::Store::open(roots.tree.clone())?;
    // `Whose::Declared`, not `Whose::Resolved`: this lane is `plan`'s own
    // decision, already made from `roots` and `repos::scan` above, and
    // `t594` §2.3's own resolution -- built for a folder that has not
    // said which lane it is yet -- would ask a question this call already
    // answered, and could answer it differently for a worktree `setup`
    // is declaring by hand rather than leaving to join on its own
    // (`t594`). `roots.here`, not `roots.tree`: `plan.repos` was scanned
    // from `roots.here` too, and a redeclaration reads this folder back
    // through `where_to_write` -- a lane joined from elsewhere is not
    // sitting at the tree's own root.
    let mut ctx = crate::ops::Ctx::load_for_write(
        store,
        crate::ops::Whose::Declared(plan.lane_id.clone(), roots.here.clone()),
    )?;
    ctx.lock_for_write()?;
    crate::ops::declare_lane(&mut ctx, plan.name.clone(), plan.repos.clone())?;
    redeclare_stale_worktrees(&mut ctx, plan)
}

/// Takes back what `write_lane_inner` had already written in `roots.here`
/// before the run failed: the lane file `crate::lane::write` puts there
/// for a brand new lane, and the `.gitignore` beside it once the file is
/// the only other thing left in `.vivac/` -- the exact same shape
/// `--undo`'s own `f719` cleanup already reasons about, reused here
/// rather than a second copy of the same check.
///
/// Best-effort, like `--undo`'s own `remove_if_empty`: the run is already
/// failing, and a second failure here has nothing left to report that the
/// first one has not already said. Safe even when `write_lane_inner`
/// never got past `crate::lane::write` in the first place -- there is
/// nothing on disk yet, and removing a file that is not there is not an
/// error this ignores, it is the ordinary case.
fn undo_fresh_lane_file(roots: &super::Roots) {
    let vivac_dir = roots.here.join(crate::store::DIR);
    let dir_holds_only_the_lane = vivac_dir_holds_only_the_lane(&vivac_dir);
    let _ = std::fs::remove_file(vivac_dir.join(crate::lane::FILE));
    if dir_holds_only_the_lane {
        let _ = std::fs::remove_file(vivac_dir.join(crate::store::GITIGNORE));
        let _ = std::fs::remove_dir(&vivac_dir);
    }
}

/// Just `plan`'s stale-worktree redeclarations (`f609`), for a run whose
/// own lane has nothing new to declare -- `write_lane` above is not
/// reached at all in that case, and a worktree stuck with no root commit
/// from before this folder's own ever had one would otherwise stay stuck
/// on every such run, forever, once this folder's own declaration has
/// settled. Opens the tree's write lock on its own, the same way
/// `relock_lanes` does, since there is no other write in this run to
/// share it with.
fn redeclare_only_stale_worktrees(roots: &super::Roots, plan: &LanePlan) -> Result<(), Failure> {
    let store = crate::store::Store::open(roots.tree.clone())?;
    let mut ctx = crate::ops::Ctx::load_for_write(
        store,
        crate::ops::Whose::Declared(plan.lane_id.clone(), roots.here.clone()),
    )?;
    ctx.lock_for_write()?;
    redeclare_stale_worktrees(&mut ctx, plan)
}

/// `plan.stale_worktrees`, applied one at a time under `ctx`'s already-held
/// lock. Shared by `write_lane`, which reaches it right after declaring
/// this folder's own lane, and by `redeclare_only_stale_worktrees`, which
/// has no declaration of its own to declare first.
fn redeclare_stale_worktrees(ctx: &mut crate::ops::Ctx, plan: &LanePlan) -> Result<(), Failure> {
    for (lane, name, repo) in plan.stale_worktrees.clone() {
        redeclare_worktree_root(ctx, lane, name, repo)?;
    }
    Ok(())
}

/// Redeclares a stale worktree lane's own repository with the root commit
/// its founding lane just learned, straight through `Store::append`
/// rather than `Ctx::emit` (`f609`). `emit` would run `where_to_write`
/// against `ctx.lane_dir`, which is wherever this run is standing --
/// `roots.here`, never the worktree's own folder this call never visited
/// -- and hand that lane a location that is not its own. Writing only
/// `lane.declared` says the one thing this run actually knows: the
/// repository's root commit, and nothing about where that lane is right
/// now.
fn redeclare_worktree_root(
    ctx: &mut crate::ops::Ctx,
    lane: String,
    name: String,
    repo: crate::event::Repo,
) -> Result<(), Failure> {
    let lock = ctx
        .lock
        .as_ref()
        .ok_or_else(|| Failure::Io(std::io::Error::other("write without the tree's lock")))?;
    let appended = ctx.store.append(
        lock,
        &lane,
        vec![crate::event::Body::LaneDeclared {
            lane: lane.clone(),
            name,
            repos: vec![repo],
        }],
        ctx.tree.seq,
        ctx.tree.has_governance,
    )?;
    for e in &appended.events {
        ctx.tree.apply(e.seq, &e.ts, &e.lane, &e.payload);
    }
    Ok(())
}

/// Locks the tree's config to `t594`'s own sentence without touching the
/// log: for a lane whose declaration already matches (`unchanged`), there
/// is nothing new to say, but the config can still have lost the lock
/// underneath it -- by hand, or by an older `Store::open` regenerating one
/// that went missing before it knew a lane event counts too (`t594`).
/// `unchanged` must never decide this on its own: a folder
/// that has nothing new to declare can still be the reason the config
/// needs relocking.
fn relock_lanes(tree: &Path) -> Result<(), Failure> {
    let mut store = crate::store::Store::open(tree.to_path_buf())?;
    let lock = store.lock_for_write()?;
    store.lock_lanes_in_config(&lock)?;
    Ok(())
}

/// The clause text for a `Failure`, without doubling an `Io` variant's own
/// "Input/output error:" prefix once a caller wraps it a second time
/// (`t594`): `Failure::message` already adds that prefix for `Io`, and the
/// planting failure this mirrors uses a raw `std::io::Error` -- which has
/// no such prefix to begin with -- for the exact same reason.
///
/// `pub(super)`: `claude_code.rs` and `codex.rs` both unwrap [`commit`]'s
/// own failure this same way, before wrapping it with whatever rollback
/// their own harness files needed.
pub(super) fn detail_of(e: &Failure) -> String {
    match e {
        Failure::Io(io) => io.to_string(),
        other => other.message(),
    }
}

/// Every root commit any lane of `tree` has declared, deduplicated and
/// sorted: the same union `relocate::union_repo_roots` computes, for the
/// same reason -- `note_registry`'s own `Sighting.repos` wants every
/// repository this tree's lanes declare, not just the one this run
/// happens to be about, so a later `setup` elsewhere can tell that a
/// folder it has never seen still holds this product (`t594` §4.8,
/// `registry::Sighting.repos`'s own doc).
fn union_repo_roots(tree: &crate::model::Tree) -> Vec<String> {
    let mut roots: Vec<String> = tree
        .lanes
        .values()
        .flat_map(|state| state.repos.iter())
        .filter_map(|repo| repo.root.clone())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Notes `roots.tree` in this machine's registry, the same bookkeeping
/// every ordinary command already does on its way out (`main.rs`). `setup`
/// itself never used to reach that block -- it returns before it
/// (`f277`) -- and that was harmless while every folder it touched was
/// found by walking up from itself. It stopped being harmless the moment
/// `setup` could join a folder whose only path back to its tree is the
/// registry: a linked worktree that sits beside the tree's own folder
/// rather than above it, which `resolve_lane` (`store.rs`) can only ever
/// find through here (`t594`). Quiet when there is
/// nowhere to note or nothing to note it with yet, the same as the
/// ordinary path.
pub(super) fn note_registry(roots: &super::Roots) {
    let Some(store_dir) = crate::store::store_dir() else {
        return;
    };
    if let Some(project_id) = crate::store::first_event_id(&roots.tree) {
        let lane = roots.located.as_ref().and_then(|l| {
            l.lane
                .as_ref()
                .map(|lane| (lane.id.as_str(), l.lane_dir.as_path()))
        });
        // The tree is folded once more here, past whatever `plan_lane`
        // already folded: this call always runs after every write this
        // run makes, so it is the one place that can report the whole
        // tree's repositories as they stand once this run is done, the
        // same union `relocate` already writes on a move (`t594` §4.8).
        let repos = union_repo_roots(&fold_tree(&roots.tree));
        let noted = crate::registry::note(
            &store_dir,
            &project_id,
            crate::registry::Sighting {
                root: &roots.tree,
                lane,
                repos: Some(&repos),
            },
        );
        // Left for `registry::warn_if_wrote` to decide, once this run is
        // done and can say whether it actually wrote anything: the
        // `nothing_to_write` branch above reaches this call too, and that
        // one is a read (`t594`).
        crate::registry::set_pending(noted);
    }
}

// ---------------------------------------------------------------------------
// `--join`'s own lane plan: `claude_code::join` still owns the preamble
// that resolves the target and refuses a folder that already carries a
// lane elsewhere, but the plan itself, once a join is going ahead, is the
// same shape a plant's own is.
// ---------------------------------------------------------------------------

/// `--join`'s own lane plan (`t640`, point 11): always a brand new lane.
/// `join`'s own preamble already rules out the one case where this folder
/// carries a lane of `target` already -- the idempotent no-op
/// `say_nothing_was_done` answers with, before this is ever reached -- so
/// `here` reaching this function never already has an id of its own to
/// keep, the same as `plan_lane`'s own `Some(_)` branch for a folder that
/// merely resolves into a tree above it rather than carrying one itself.
pub(super) fn plan_join_lane(here: &Path, target: &Path, lane_name: Option<&str>) -> LanePlan {
    let (repos, excluded) = filtered_repos(crate::repos::scan(here));
    let folder_name = here
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let requested_name = lane_name.unwrap_or(&folder_name);
    let id = crate::lane::new_id();
    let name = crate::lane::declared_name(&id, requested_name);
    let folded = fold_tree(target);
    let existing = existing_lane(target, &id, &folded);
    let needs_lock = existing.config_version != crate::store::ConfigVersion::Lanes;
    // A fresh id can never already be declared, so this always reads
    // `false` -- computed the same way `plan_lane` computes it rather
    // than hardcoded, so the two never have a reason to drift apart.
    let unchanged = existing
        .declared
        .is_some_and(|(n, r)| n == name && r == repos);
    let stale_worktrees = stale_worktree_roots(here, &repos, &id, &folded);

    LanePlan {
        lane_id: id,
        name,
        repos,
        is_new: true,
        needs_lock,
        unchanged,
        excluded,
        stale_worktrees,
    }
}

/// The id of the lane `here` itself is, or `None` for a folder that merely
/// resolves up into a tree above it. The one criterion, asked in the two
/// places `join` needs it: a lane file, carried by this folder rather than
/// by some ancestor. `same_folder`, never a path compared as text -- a
/// second spelling of the same folder is the same folder (`f612`).
pub(super) fn lane_carried_by<'a>(l: &'a crate::store::Located, here: &Path) -> Option<&'a str> {
    let lane = l.lane.as_ref()?;
    crate::anchor::same_folder(&l.lane_dir, here).then_some(lane.id.as_str())
}

/// What a person learns from a `--join` that had nothing left to do: that
/// it is done already, and that this run left it alone. It reads as an
/// answer rather than as a refusal because a re-run of the provisioning a
/// team shares is the ordinary way to arrive here -- the same reason a
/// plain `setup` run twice says the tree was already there.
///
/// The second sentence is for `--lane-name` asking for a name the lane
/// does not have: the flag was read and not acted on, and a flag accepted
/// in silence leaves nothing behind to say it was ignored (`t594`).
/// Asking for the name it already carries needs no sentence --
/// nothing was left undone. The tree is folded only for that question, so
/// a run without the flag reads no log at all.
pub(super) fn say_nothing_was_done(target: &Path, lane_id: &str, lane_name: Option<&str>) {
    crate::output::outln!(
        "  This folder is already a lane of that tree, and init changed nothing in it."
    );
    let Some(requested) = lane_name else {
        return;
    };
    // `declared_name` is what the name would have become had it been
    // written, redaction guard and all (`d600`): comparing the raw request
    // instead would report a difference the write itself would have
    // collapsed.
    let requested = crate::lane::declared_name(lane_id, requested);
    let current = fold_tree(target)
        .lanes
        .get(lane_id)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    if current != requested {
        crate::output::outln!("  The lane name it already has was left as it is.");
    }
}

// ---------------------------------------------------------------------------
// The plan: `TreePlan` is everything `apply_writes` needs to know about
// the tree, worked out once, before anything is written or even asked.
// ---------------------------------------------------------------------------

/// Everything a run needs to know about the tree side of a plant or a
/// join, before it prints a plan or writes anything: the same values
/// `claude_code.rs`'s own `apply_writes` used to compute inline.
pub(super) struct TreePlan {
    pub(super) here: PathBuf,
    pub(super) tree: PathBuf,
    pub(super) vivac_missing: bool,
    pub(super) gitignore_missing: bool,
    pub(super) lane: LanePlan,
    pub(super) product_name: Option<String>,
    pub(super) requested_name: Option<String>,
    pub(super) name_collision: Option<String>,
    /// `t594` §4.5, case 2: this tree itself sits inside yet another one.
    /// Never blocks anything; printed alongside whichever exit a run
    /// actually reaches.
    pub(super) above_warning: Option<String>,
    /// `f676`/`d682`: empty unless this run plants a genuine new product
    /// the registry cannot recognise as one it already knows.
    pub(super) unknown_product_warning: String,
    /// Whether `.vivac/events` is itself tracked by git in a working tree
    /// -- asked once per run, and true regardless of what else this run
    /// has left to do.
    pub(super) log_tracked: bool,
}

/// `TreePlan`'s own fields, worked out from `roots` and an already-decided
/// `lane`/`requested` pair: shared by [`plan`], for a plant, and
/// [`plan_for_join`], for a join -- the two ways a `LanePlan` gets built,
/// converging here on the one place that turns either into the rest of
/// what a run needs to know about the tree.
fn build_plan(roots: &super::Roots, lane: LanePlan, requested: Option<String>) -> TreePlan {
    let here = roots.here.clone();
    let tree = roots.tree.clone();
    let vivac_missing = !crate::store::already_planted(&tree);
    // A tree this run plants already carries its `.gitignore`, straight
    // out of `Store::create`: only a tree from before `t594` §4.9 can
    // lack it.
    let gitignore_missing = !vivac_missing
        && !tree
            .join(crate::store::DIR)
            .join(crate::store::GITIGNORE)
            .is_file();
    let product_name = product_name_for_plan(&tree, requested.as_deref());
    // `t640`, point 10 bis: a warning, never a refusal -- a project's
    // identity is its first event's id, not its name, so two projects
    // answering to the same name break nothing but `--join <name>`'s own
    // convenience, and that is worth saying before this is written.
    let name_collision = requested
        .as_deref()
        .filter(|name| {
            crate::store::store_dir().is_some_and(|store_dir| {
                crate::registry::another_project_answers_to(&store_dir, name)
            })
        })
        .map(str::to_string);
    let above_warning =
        tree_root_above(&tree).map(|p| tree_above_warning(guarded_folder_name(&p).as_deref()));
    // `f676`/`d682`: only a genuine plant can be a product the registry
    // never learned about yet -- a join already named its tree, and a
    // tree already here is already a known one.
    let unknown_product_warning = if vivac_missing {
        second_map_hint(&here).unwrap_or_default()
    } else {
        String::new()
    };
    let log_tracked = crate::anchor::in_working_tree(&tree)
        && crate::anchor::tracks(&tree, ".vivac/events") == Some(true);
    TreePlan {
        here,
        tree,
        vivac_missing,
        gitignore_missing,
        lane,
        product_name,
        requested_name: requested,
        name_collision,
        above_warning,
        unknown_product_warning,
        log_tracked,
    }
}

/// All of the tree side's own refusals, and the plan of what a plant would
/// do: `t592` tranche 2's own seam (`d710`), shared by every harness. Does
/// not write anything.
///
/// What it checks, in order: `--name` fixes a product's own name, valid
/// only while this run is planting a fresh tree or bypassing that question
/// outright with `--new-tree` (`t640`, point 1); whether this folder's
/// repositories are already tracked by another project the registry
/// knows, unless `--new-tree` bypasses that too (`refuse_second_map`);
/// and only then the lane this folder itself would become
/// (`plan_lane`). A tree already sitting below `here` is a caller's own
/// guard, checked before this is ever called: `init::run` (`d723` piece B,
/// the one caller left) needs to know about it earlier than this, to
/// choose between a plant's own refusal and a join's.
pub(super) fn plan(roots: &super::Roots, a: &Args) -> Result<TreePlan, Failure> {
    refuse_second_map(roots, a.has("new-tree"))?;
    let tree = &roots.tree;
    let requested = requested_name(a)?;
    if requested.is_some() && crate::store::already_planted(tree) && !a.has("new-tree") {
        return Err(Failure::usage(
            "--name only names a product while init plants one: this \
             folder's tree already exists, and already has a name of its \
             own.\n\n  Nothing written.",
        ));
    }
    let lane = plan_lane(roots, a.opt("lane-name"));
    Ok(build_plan(roots, lane, requested))
}

/// [`plan`]'s own mirror for `--join`: `join`'s preamble has already
/// resolved the target tree and ruled out the cases that need no plan at
/// all, so what is left is exactly [`build_plan`] with `lane` already
/// decided and no requested name -- `--join` does not take `--name`.
pub(super) fn plan_for_join(join_roots: &super::Roots, lane: LanePlan) -> TreePlan {
    build_plan(join_roots, lane, None)
}

/// §6.4's mirror image, upward: a folder with no `.vivac/` of its own,
/// told to `--join` a tree somewhere else while the tree it already
/// resolves to sits above it.
///
/// `Failure::already_a_lane` used to answer here, and its own doc says
/// what is wrong with that: it is for "a folder that already carries
/// somebody else's `.vivac/lane`", and this folder carries none at all.
/// The refusal itself was never in doubt -- joining would split the
/// product either way -- so what changes is only the sentence, which now
/// says the thing that is true and where to go and read it.
///
/// Named, and the name withheld when the redaction guard rejects it
/// (`d600`), the same as every other folder this module names.
fn tree_above_refusal(tree_root: &Path) -> Failure {
    let label = crate::registry::label_for(guarded_folder_name(tree_root).as_deref());
    Failure::Model(format!(
        "  A tree sits above this folder, in {label}, so this folder already belongs to\n  \
         that product. Joining it to a different tree would split the two. To see\n  \
         where it belongs:  vivac brief"
    ))
}

/// Every refusal `--join` owns, and the plan it ends in. `Ok(None)` means
/// the run found nothing left to do and has already said so, so the caller
/// returns `Ok(0)` without writing or printing anything more.
pub(super) fn plan_join(
    roots: &super::Roots,
    spec: &str,
    lane_name: Option<&str>,
) -> Result<Option<(super::Roots, TreePlan)>, Failure> {
    let target = crate::registry::resolve(spec)?;
    if !crate::store::already_planted(&target) {
        return Err(Failure::Model(format!(
            "  \"{spec}\" has no tree yet, so there is nothing to join.\n  \
             Plant one there first:  vivac init"
        )));
    }
    // §4.5: refuses when this folder already is a lane of *another* tree --
    // joining the very one it already resolves to does nothing at all, since
    // there is nothing left to do. A folder that holds a tree of its own
    // gets a different text: it carries no lane to redirect, it carries
    // the tree (`t594`).
    if let Some(l) = &roots.located {
        if !crate::anchor::same_folder(&l.root, &target) {
            if crate::anchor::same_folder(&roots.here, &l.root) {
                return Err(Failure::already_has_a_tree());
            }
            // Two different folders reach this line, and only one of them
            // is a lane: the one that carries `.vivac/lane` itself.
            // Everything else here has no `.vivac/` of its own at all and
            // simply resolves up into the tree above it, which is a
            // different sentence -- `already_a_lane` names a file that
            // folder does not have.
            if lane_carried_by(l, &roots.here).is_some() {
                return Err(Failure::already_a_lane());
            }
            return Err(tree_above_refusal(&l.root));
        }
        // The same tree, and this folder already carries the lane file
        // that says so: everything below would mint a second lane id for
        // a folder that already has one, orphaning the stack, the focus
        // and the counters the first one holds. Nothing is written and
        // nothing is appended, so this returns ahead of `--dry-run` too:
        // what that flag reports is what a run would do, and this run
        // would do nothing either way.
        if let Some(id) = lane_carried_by(l, &roots.here) {
            say_nothing_was_done(&target, id, lane_name);
            return Ok(None);
        }
    }
    // Never `spec`, and never `target` either (`t594`):
    // unlike the "no tree yet" refusal above, this is the one place `join`
    // would otherwise echo a path back that a person did not necessarily
    // type themselves -- `spec` might have resolved through a project
    // name, not a path at all.
    if crate::store::first_event_id(&target).is_none() {
        return Err(Failure::Model(
            "  That tree has no events yet, so there is nothing to join: it has\n  \
             no identity yet for a lane to point back at."
                .to_string(),
        ));
    }

    // `t640`, point 11: from here, this run walks the very same path
    // `apply_writes` already walks for a plant, minus the plant --
    // `join_roots.tree` is `target`, already confirmed planted above, so
    // `apply_writes` never mints a `Store::create` for it. `located: None`
    // is safe: `write_lane`, `note_registry` and `apply_writes` itself
    // only ever read `roots.tree` and `roots.here` off this value, never
    // `located`, which is `plan_lane`'s own question and `join` answers
    // for itself with `plan_join_lane` instead.
    let join_roots = super::Roots {
        here: roots.here.clone(),
        tree: target.clone(),
        located: None,
    };
    let lane = plan_join_lane(&roots.here, &target, lane_name);
    let plan = plan_for_join(&join_roots, lane);
    Ok(Some((join_roots, plan)))
}

// ---------------------------------------------------------------------------
// Rendering: the plan lines `claude_code.rs` has always shown for the
// tree, split into the two groups every harness's plan interleaves with
// its own pieces -- `opening_lines` before them, `closing_lines` after.
// The split is the reading order and not the order things happen in: the
// ground a project stands on is named before whatever gets written onto
// it, and what the run then records about it comes last.
// ---------------------------------------------------------------------------

/// The `.vivac/` item itself, and the tree's own `.gitignore` item when a
/// tree that already exists still lacks one: the tree items `init`'s own
/// plan shows first, ahead of the lane's own (`closing_items`). `d792`:
/// data first, one `PlanItem` per row, rendered together with
/// `closing_items` so every column lines up across both halves.
pub(super) fn opening_items(plan: &TreePlan) -> Vec<super::claude_code::PlanItem> {
    use super::claude_code::PlanItem;
    let mut items = Vec::new();

    let mut vivac_item = if plan.vivac_missing {
        PlanItem::new("plant", VIVAC_LABEL, "the tree")
    } else {
        PlanItem::new("keep", VIVAC_LABEL, "already there")
    };
    // Where the tree actually lives is not prose and never wraps, so a
    // tree found above this folder gets its path on a line of its own
    // rather than riding inside the status that does.
    if !plan.vivac_missing && plan.tree != plan.here {
        vivac_item = vivac_item.with_sub("in", plan.tree.display().to_string());
    }
    items.push(vivac_item);

    if plan.gitignore_missing {
        // Two different files, in two different folders, can both need
        // this item in the same run -- the tree's own, from before `t594`
        // §4.9, and a brand new lane's own (`closing_items`). Only then
        // does the tree's own copy say whose it is; on its own it reads
        // exactly as it always has (`t594`).
        let what = if plan.lane.is_new {
            "keeps the tree's .vivac/ out of version control"
        } else {
            "keeps .vivac/ out of version control"
        };
        items.push(PlanItem::new("create", GITIGNORE_LABEL, what));
    }
    items
}

/// The lane's own items, the stale-worktree and excluded-repository ones,
/// and the version lock: the tree items `init`'s own plan shows *after*
/// the tree's own (`opening_items`).
pub(super) fn closing_items(plan: &TreePlan) -> Vec<super::claude_code::PlanItem> {
    use super::claude_code::PlanItem;
    let mut items = Vec::new();
    let lane = &plan.lane;
    if !lane.unchanged {
        // `t640`, point 9: the plan names the product on this same line,
        // in both shapes it takes -- a brand new lane and a redeclared
        // one alike.
        let product = product_label(plan.product_name.as_deref());
        if lane.is_new {
            items.push(PlanItem::new(
                "create",
                LANE_LABEL,
                format!("this folder becomes lane \"{}\" of {product}", lane.name),
            ));
            items.push(PlanItem::new(
                "create",
                GITIGNORE_LABEL,
                "keeps .vivac/ out of version control",
            ));
        } else {
            // One sentence for both: declaring `main` on the tree's own
            // folder and redeclaring a lane that already existed are the
            // same write, and neither creates a file the way a brand new
            // lane does above -- it is the log that changes.
            items.push(PlanItem::new(
                "write",
                ".vivac/events",
                format!(
                    "its log, with this folder as part \"{}\" of {product}",
                    lane.name
                ),
            ));
        }
    }
    // Independent of `unchanged` too: a worktree can be stuck with no root
    // commit from before this folder's own repositories ever had one,
    // which a run that finds nothing new of its own to declare still
    // repairs (`f609`).
    if !lane.stale_worktrees.is_empty() {
        let count = lane.stale_worktrees.len();
        let noun = if count == 1 { "lane" } else { "lanes" };
        items.push(PlanItem::new(
            "redeclare",
            ".vivac/events",
            format!("{count} worktree {noun} with the repositories this run found"),
        ));
    }
    // What the redaction guard kept out is the folder's own state, not a
    // change: it is still true on a run that declares nothing new, so it
    // is said every time rather than only on the run that first found it
    // (`t594`). Hangs off whichever item is already last in this plan, the
    // same visual attachment the flat string this replaces always gave
    // it; a run with nothing else to say about the lane still gets one of
    // its own to hang it off.
    if let Some((count, rule)) = lane.excluded {
        let noun = if count == 1 {
            "repository"
        } else {
            "repositories"
        };
        let value = format!("{count} {noun}, refused: {rule}");
        let last = items
            .pop()
            .unwrap_or_else(|| PlanItem::new("keep", ".vivac/events", ""));
        items.push(last.with_sub("kept out", value));
    }
    // Independent of `unchanged`: the config can need the lock even when
    // nothing about the declaration itself changed (`t594`).
    if lane.needs_lock {
        items.push(PlanItem::new(
            "lock",
            ".vivac/config",
            "the minimum version to open it: vivac 0.12",
        ));
    }
    items
}

/// The exit-5 text for a lane declaration or a config relock that failed,
/// after `unrestored` -- what `super::rollback` could not put back among
/// the tree's own `.gitignore` -- is already known, and after `write_lane`'s
/// own best-effort cleanup of a fresh join's lane file has already run.
///
/// Unlike `failure_with_rollback`, this never says every file came back:
/// by the time either call above can fail, a real event may already sit
/// in the tree's own log (`ensure_first_event`'s seed) or the config may
/// already be locked, and neither of those is a file `rollback` ever
/// touches or could undo. `t565` §7.7 accepts the same gap for planting,
/// on the same reasoning -- but planting never writes anything of
/// informational value before it can fail, and a lane's own event does,
/// so this says the log stays instead of claiming a rollback it did not
/// do and cannot do.
///
/// `d723` piece B: `init` is the only caller left, and there is no
/// settings file, server entry or skill for this run to have touched --
/// `write_lane` above already took its own lane file and `.gitignore`
/// back out before this is ever built.
fn lane_failure_with_rollback(clause: String, unrestored: &[PathBuf]) -> Failure {
    let mut message = clause;
    if unrestored.is_empty() {
        message.push_str(
            ", so init put back everything it had already written here and in\n  \
             the tree. Whatever this already wrote to the tree's own log stays\n  \
             either way: the log only ever grows.",
        );
    } else {
        message.push_str(", and init could not put these back as they were:\n");
        for p in unrestored {
            message.push_str(&format!("      {}\n", p.display()));
        }
        message.push_str(
            "  init keeps no copy on disk, so the only other copy is whatever\n  \
             version control holds. Whatever this already wrote to the tree's own\n  \
             log stays either way: the log only ever grows.",
        );
    }
    Failure::Io(std::io::Error::other(message))
}

// ---------------------------------------------------------------------------
// Committing: planting, the lane, and the version lock. The tree's own
// `.gitignore` is a plain file write, so it travels in [`file_writes`]
// with every harness's own settings, server entries and skill, and goes
// down in the same all-or-nothing `super::commit` -- this only ever runs
// once that commit has already succeeded, and rolls the same writes back
// by hand if planting, the lane or the lock fails where that commit
// cannot reach.
// ---------------------------------------------------------------------------

/// The tree's own plain-file writes, for the caller's own `writes` --
/// today only the tree's `.gitignore`, when an existing tree still lacks
/// one. `Vec::new()` for a tree this run plants (`Store::create` writes
/// its own) and for one that already has it.
///
/// `pub(super)`: both harnesses add this to their own `writes`, at the
/// same place a fresh plant's `.gitignore` always sat -- the last piece
/// pushed, right after the skill -- so the single `super::commit` below
/// keeps covering every file this run touches, `.gitignore` included
/// (`t565` §7.3).
pub(super) fn file_writes(roots: &super::Roots, plan: &TreePlan) -> Vec<super::PlannedWrite> {
    let mut writes = Vec::new();
    if plan.gitignore_missing {
        writes.push(super::PlannedWrite::write(
            roots
                .tree
                .join(crate::store::DIR)
                .join(crate::store::GITIGNORE),
            "*\n".to_string(),
            None,
        ));
    }
    writes
}

/// Plants the tree if `plan` says to, declares this folder's lane (or just
/// relocks the config when nothing about the declaration itself changed),
/// and redeclares any worktree lane `plan` found stuck with no root
/// commit.
///
/// `committed` is the caller's own `writes`, already committed by the
/// caller's own `super::commit` -- settings, server entry, skill and the
/// tree's own `.gitignore` among them ([`file_writes`]). None of the three
/// steps here ever rolls back on its own failure (`t565` §7.7): a failure
/// undoes `committed` by hand instead, since this function is the first
/// place that knows one of them failed. Planting never writes anything of
/// informational value before it can fail, so its own failure uses the
/// plain `super::failure_with_rollback`, the same as a relock's -- but
/// declaring the lane can already have appended a real event to the
/// tree's own log (`ensure_first_event`'s seed) by the time it fails,
/// which `rollback` cannot undo and must not be claimed fixed, so that one
/// uses [`lane_failure_with_rollback`] instead.
pub(super) fn commit(
    roots: &super::Roots,
    plan: &TreePlan,
    committed: &[super::PlannedWrite],
) -> Result<(), Failure> {
    if plan.vivac_missing {
        if let Err(e) = crate::store::Store::create(&roots.tree) {
            let unrestored = super::rollback(committed);
            return Err(super::failure_with_rollback(
                format!("the tree could not be planted ({e})"),
                &unrestored,
            ));
        }
    }

    if !plan.lane.unchanged {
        if let Err(e) = write_lane(roots, &plan.lane) {
            let unrestored = super::rollback(committed);
            return Err(lane_failure_with_rollback(
                format!("the lane could not be declared ({})", detail_of(&e)),
                &unrestored,
            ));
        }
    } else {
        if !plan.lane.stale_worktrees.is_empty() {
            if let Err(e) = redeclare_only_stale_worktrees(roots, &plan.lane) {
                let unrestored = super::rollback(committed);
                return Err(lane_failure_with_rollback(
                    format!("the lane could not be declared ({})", detail_of(&e)),
                    &unrestored,
                ));
            }
        }
        if plan.lane.needs_lock {
            if let Err(e) = relock_lanes(&roots.tree) {
                let unrestored = super::rollback(committed);
                return Err(super::failure_with_rollback(
                    format!(
                        "the tree's config could not be relocked ({})",
                        detail_of(&e)
                    ),
                    &unrestored,
                ));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `--undo`'s own piece of the tree, shared by both harnesses (`t592`
// tranche 2, piece C, `r515`): whether this folder's own lane file can go,
// and what the plan says about it either way. Out of `claude_code::undo`,
// the only place it lived until `codex::undo` needed the same door.
// ---------------------------------------------------------------------------

/// `raw` is `Some` only once a lane file was actually there to read; `Ok`
/// with `exists: false` is the ordinary shape of a folder with no lane file
/// at all, and has nothing for `--undo`'s plan to say about this piece.
pub(super) struct UndoLane {
    pub(super) path: PathBuf,
    pub(super) raw: Option<Vec<u8>>,
    exists: bool,
    pub(super) removable: bool,
    /// This folder's own `.vivac/`, so `--undo` can delete its
    /// `.gitignore` and the now-empty directory itself without rebuilding
    /// the path from `roots` a second time (`f719`, point B).
    pub(super) vivac_dir: PathBuf,
    /// Whether `.vivac/` here holds no tree of its own (`already_planted`
    /// false): the one fact that tells a joined folder's `.vivac/` apart
    /// from the folder that actually holds the tree, which `--undo` never
    /// touches. `false` whenever `exists` is too -- a folder with no lane
    /// file of its own never joined anything in the first place.
    joined: bool,
    /// Whether `--undo` may remove `.vivac/` itself once `path` is gone:
    /// every one of `f719`'s own three conditions at once -- the lane was
    /// retirable, this folder holds no tree, and nothing is left inside
    /// but the `.gitignore` this tool itself would have written.
    pub(super) vivac_dir_removable: bool,
}

/// Reads this folder's own `.vivac/lane`, without writing anything, and
/// decides whether `--undo` may take it (`d680`): `lane_has_written`
/// already skips the context events a join writes on a lane's own behalf,
/// so a lane that only ever joined and never pushed, popped or noted
/// anything owns no history for the file to orphan.
///
/// Also decides whether `.vivac/` itself may go once that lane does
/// (`f719`, point B): it was the union that wrote this folder's own
/// `.vivac/` in the first place, so taking the lane away leaves it with no
/// reason to exist, as long as this folder holds no tree of its own and
/// nothing beyond the lane and a stock `.gitignore` is left inside.
pub(super) fn undo_lane(roots: &super::Roots) -> Result<UndoLane, Failure> {
    let here = roots.here.as_path();
    let vivac_dir = here.join(crate::store::DIR);
    let path = vivac_dir.join(crate::lane::FILE);
    let raw = std::fs::read(&path).ok();
    let own_lane = crate::lane::read(&vivac_dir)?;
    let wrote = own_lane
        .as_ref()
        .is_some_and(|lane| lane_has_written(&roots.tree, &lane.id));
    let exists = own_lane.is_some();
    let removable = exists && !wrote;
    let joined = exists && !crate::store::already_planted(here);
    let vivac_dir_removable = joined && removable && vivac_dir_holds_only_the_lane(&vivac_dir);
    Ok(UndoLane {
        path,
        raw,
        exists,
        removable,
        vivac_dir,
        joined,
        vivac_dir_removable,
    })
}

/// Whether `vivac_dir` holds nothing beyond the `lane` file `--undo` is
/// about to take and, at most, the `.gitignore` `write_gitignore` itself
/// would have written (`f719`, point B, condition 3): a `.gitignore`
/// missing entirely counts the same as one that matches -- neither is
/// anything this tool did not write -- and any other name at all, or a
/// `.gitignore` whose content does not match, answers `false`: a line
/// somebody added by hand is theirs, and `write_gitignore` already
/// promises never to touch a file that already exists.
fn vivac_dir_holds_only_the_lane(vivac_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(vivac_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(crate::lane::FILE) {
            continue;
        }
        if name == std::ffi::OsStr::new(crate::store::GITIGNORE) {
            if matches!(std::fs::read_to_string(entry.path()), Ok(c) if c == "*\n") {
                continue;
            }
            return false;
        }
        return false;
    }
    true
}

/// The lane's own item in `--undo`'s plan: nothing at all when this folder
/// never had a lane file, `remove` when it can go, and `keep` with why
/// when it stays because its lane has written.
pub(super) fn undo_lane_items(lane: &UndoLane) -> Vec<super::claude_code::PlanItem> {
    use super::claude_code::PlanItem;
    if !lane.exists {
        return Vec::new();
    }
    if lane.removable {
        vec![PlanItem::new("remove", LANE_LABEL, "this folder's lane")]
    } else {
        vec![PlanItem::new(
            "keep",
            LANE_LABEL,
            "this lane has written to the tree, and removing it would orphan what it wrote",
        )]
    }
}

/// The `.vivac/` item itself, in `--undo`'s plan (`f719`, point B): the
/// same reassurance it has always given once the tree lives in this
/// folder -- unchanged, since the tree itself still stays out of `--undo`
/// entirely -- or, once it does not, the decision this folder's own
/// `.vivac/` earns for holding nothing a join did not write: gone along
/// with the lane that justified it, or left in place and said why.
pub(super) fn vivac_dir_items(lane: &UndoLane) -> Vec<super::claude_code::PlanItem> {
    use super::claude_code::PlanItem;
    if !lane.joined {
        return vec![PlanItem::new(
            "keep",
            VIVAC_LABEL,
            "the tree is not setup's",
        )];
    }
    if lane.vivac_dir_removable {
        vec![PlanItem::new(
            "remove",
            VIVAC_LABEL,
            "it holds nothing but this lane",
        )]
    } else {
        vec![PlanItem::new(
            "keep",
            VIVAC_LABEL,
            "it holds more than this lane",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same promise for the refusal's mirror image, upward: the tree
    /// above is named, and a name the redaction guard rejects is not
    /// written down at all -- the sentence still says where to go and
    /// read it.
    #[test]
    fn tree_above_refusal_with_the_name_withheld_says_so_without_naming_anyone() {
        let secret = "someone@example.com";
        assert!(
            crate::redact::check_field("folder name", secret).is_some(),
            "the guard must actually reject this name, or the test proves nothing"
        );

        let msg = tree_above_refusal(&PathBuf::from("/tmp").join(secret)).message();

        assert!(
            msg.contains("A tree sits above this folder, in another folder,"),
            "{msg}"
        );
        assert!(msg.contains("vivac brief"), "{msg}");
        assert!(!msg.contains(secret), "{msg}");
    }

    /// `tree_below_refusal`'s own fallback for two or more trees below
    /// whose names the redaction guard withholds entirely: unspecified by
    /// `t594` §1.2, which only names the plural form's shape, not what it
    /// says once nothing is nameable at all -- so it earns its keep by
    /// having a test rather than by being removed (`t594`).
    #[test]
    fn tree_below_refusal_with_every_name_withheld_says_so_without_naming_anyone() {
        let secret_a = "someone@example.com";
        let secret_b = "other@example.com";
        assert!(
            crate::redact::check_field("folder name", secret_a).is_some(),
            "the guard must actually reject this name, or the test proves nothing"
        );
        let paths = vec![
            PathBuf::from("/tmp").join(secret_a),
            PathBuf::from("/tmp").join(secret_b),
        ];

        let msg = tree_below_refusal(&paths).message();

        assert!(
            msg.contains("under names this tool will not write down"),
            "{msg}"
        );
        assert!(!msg.contains(secret_a), "{msg}");
        assert!(!msg.contains(secret_b), "{msg}");
    }

    /// `lane_failure_with_rollback` must never claim every file came back:
    /// by the time a lane declaration fails, a real event can already sit
    /// in the tree's own log, which `rollback` never touches. With
    /// nothing left `rollback` could not restore, this still says
    /// whatever `init` itself had already written came back **and** that
    /// the log stays either way -- both halves, not one instead of the
    /// other.
    #[test]
    fn lane_failure_with_rollback_with_nothing_unrestored_still_says_the_log_stays() {
        let msg =
            lane_failure_with_rollback("the lane could not be declared (boom)".to_string(), &[])
                .message();
        assert!(
            msg.contains("so init put back everything it had already written"),
            "{msg}"
        );
        assert!(msg.contains("the log only ever grows"), "{msg}");
    }

    /// The other half: something `rollback` itself could not put back is
    /// named, version control is pointed at as the only other copy, and
    /// the log is still said to stay -- the same two things true at once,
    /// not a choice between them.
    #[test]
    fn lane_failure_with_rollback_with_something_unrestored_names_it_and_still_says_the_log_stays()
    {
        let unrestored = vec![PathBuf::from("/tmp/.vivac/.gitignore")];
        let msg = lane_failure_with_rollback(
            "the lane could not be declared (boom)".to_string(),
            &unrestored,
        )
        .message();
        assert!(msg.contains("/tmp/.vivac/.gitignore"), "{msg}");
        assert!(msg.contains("init keeps no copy on disk"), "{msg}");
        assert!(msg.contains("the log only ever grows"), "{msg}");
    }
}
