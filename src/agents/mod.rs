//! Agent custody is rebuilt from events; native files are projections.
pub mod adapters;
mod human;
mod reconcile;
mod terminal;
pub mod types;
pub(crate) mod workflow;

use crate::args::Args;
use crate::event::{Body, Kind, State};
use crate::failure::Failure;
use crate::ops::{self, Ctx};
use crate::store::{self, Located, Store};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use types::{Assignment, Candidate, Definition, PromptSource};

type Target = (String, String, String);
type OwnedTarget = (String, String, String, String);

#[derive(Clone, Serialize)]
struct Agent {
    agent: String,
    revision: String,
    definition: Definition,
}
#[derive(Clone)]
struct Binding {
    agent: String,
    baseline: Option<String>,
}
#[derive(Clone)]
struct Receipt {
    revision: String,
    digest: String,
    adapter: String,
}
#[derive(Clone, Serialize)]
struct Observation {
    revision: String,
    model: String,
    effort: String,
    evidence: String,
}
#[derive(Clone, Default)]
struct Ledger {
    agents: BTreeMap<String, Agent>,
    history: BTreeMap<String, Definition>,
    bindings: BTreeMap<Target, Binding>,
    receipts: BTreeMap<OwnedTarget, Receipt>,
    observations: BTreeMap<OwnedTarget, Observation>,
    automation: BTreeMap<String, (bool, String)>,
    detached: HashSet<Target>,
}

fn owned(key: &Target, agent: &str) -> OwnedTarget {
    (
        key.0.clone(),
        key.1.clone(),
        key.2.clone(),
        agent.to_string(),
    )
}

fn equivalent_receipt(
    ledger: &Ledger,
    receipt: &Receipt,
    agent: &Agent,
    harness: &str,
    version: &str,
) -> bool {
    receipt.adapter == version
        && ledger
            .history
            .get(&receipt.revision)
            .is_some_and(|previous| {
                previous.contract == agent.definition.contract
                    && previous.prompt == agent.definition.prompt
                    && previous
                        .assignments
                        .iter()
                        .find(|assignment| assignment.harness == harness)
                        == agent
                            .definition
                            .assignments
                            .iter()
                            .find(|assignment| assignment.harness == harness)
            })
}

fn replay(tree: &crate::model::Tree) -> Result<Ledger, Failure> {
    let mut ledger = Ledger::default();
    for (lane, body) in &tree.agent_history {
        if matches!(
            body,
            Body::AgentRecorded { .. }
                | Body::AgentBound { .. }
                | Body::AgentDetached { .. }
                | Body::AgentMaterialized { .. }
                | Body::AgentObserved { .. }
                | Body::AgentAutomationConfigured { .. }
        ) {
            adapters::check_value(&serde_json::to_value(body).map_err(std::io::Error::other)?)?;
        }
        match body {
            Body::AgentAutomationConfigured { node, enabled } => {
                tree.resolve(node)
                    .filter(|n| n.id == *node && n.kind == Kind::Decision)
                    .ok_or_else(|| {
                        Failure::Model("Agent automation has no source decision.".into())
                    })?;
                ledger
                    .automation
                    .insert(lane.clone(), (*enabled, node.clone()));
            }
            Body::AgentRecorded {
                agent,
                node,
                definition,
            } => {
                let source = tree
                    .resolve(node)
                    .filter(|source| source.id == *node && source.kind == Kind::Decision)
                    .ok_or_else(|| {
                        Failure::Model("An agent revision has no source decision.".into())
                    })?;
                validate_definition(definition)?;
                ledger.history.insert(node.clone(), definition.clone());
                ledger.agents.insert(
                    agent.clone(),
                    Agent {
                        agent: agent.clone(),
                        revision: source.id.clone(),
                        definition: definition.clone(),
                    },
                );
            }
            Body::AgentBound {
                agent,
                harness,
                path,
                baseline,
            } => {
                let key = (lane.clone(), harness.clone(), path.clone());
                ledger.detached.remove(&key);
                ledger.receipts.remove(&owned(&key, agent));
                ledger.observations.remove(&owned(&key, agent));
                if ledger
                    .bindings
                    .get(&key)
                    .is_some_and(|binding| binding.agent != *agent)
                {
                    return Err(Failure::Model("Two agents claim one native target.".into()));
                }
                ledger.bindings.insert(
                    key,
                    Binding {
                        agent: agent.clone(),
                        baseline: baseline.clone(),
                    },
                );
            }
            Body::AgentDetached {
                agent,
                harness,
                path,
            } => {
                let key = (lane.clone(), harness.clone(), path.clone());
                ledger.detached.insert(key.clone());
                if ledger
                    .bindings
                    .get(&key)
                    .is_some_and(|binding| binding.agent == *agent)
                {
                    ledger.bindings.remove(&key);
                }
            }
            Body::AgentMaterialized {
                agent,
                revision,
                harness,
                path,
                digest,
                adapter,
            } => {
                ledger.receipts.insert(
                    (lane.clone(), harness.clone(), path.clone(), agent.clone()),
                    Receipt {
                        revision: revision.clone(),
                        digest: digest.clone(),
                        adapter: adapter.clone(),
                    },
                );
            }
            Body::AgentObserved {
                agent,
                revision,
                harness,
                path,
                model,
                effort,
                evidence,
            } => {
                ledger.observations.insert(
                    (lane.clone(), harness.clone(), path.clone(), agent.clone()),
                    Observation {
                        revision: revision.clone(),
                        model: model.clone(),
                        effort: effort.clone(),
                        evidence: evidence.clone(),
                    },
                );
            }
            _ => {}
        }
    }
    if ledger
        .bindings
        .values()
        .any(|binding| !ledger.agents.contains_key(&binding.agent))
    {
        return Err(Failure::Model(
            "An agent binding has no declared contract.".into(),
        ));
    }
    Ok(ledger)
}

fn checked(field: &str, value: &str, limit: usize) -> Result<(), Failure> {
    if let Some(finding) = crate::redact::check_field(field, value) {
        return Err(Failure::Redaction(Box::new(finding)));
    }
    if value.trim().is_empty()
        || value.len() > limit
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Failure::usage(
            "Agent text must be nonempty, bounded prose.",
        ));
    }
    Ok(())
}

fn validate_definition(definition: &Definition) -> Result<(), Failure> {
    adapters::check_value(&serde_json::to_value(definition).map_err(std::io::Error::other)?)?;
    if definition.schema_version != 1
        || definition.assignments.is_empty()
        || definition.assignments.len() > adapters::all().len()
    {
        return Err(Failure::usage(
            "Agent definitions need schema_version 1 and explicit harness assignments.",
        ));
    }
    checked("agent name", &definition.name, 100)?;
    if !definition
        .name
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(Failure::usage("Agent names must be plain identifiers."));
    }
    checked("agent purpose", &definition.contract.purpose, 2000)?;
    if let Some(source) = &definition.prompt {
        let native = adapter(&source.harness)?;
        if !source.path.starts_with(&format!("{}/", native.directory()))
            || Path::new(&source.path)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || source.digest.len() != 64
            || !source.digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Failure::usage(
                "Prompt references need an adapter path and SHA-256 digest.",
            ));
        }
    }
    if definition.contract.duties.is_empty() || definition.contract.acceptance.is_empty() {
        return Err(Failure::usage(
            "An agent contract needs duties and acceptance criteria.",
        ));
    }
    for entries in [
        &definition.contract.duties,
        &definition.contract.limits,
        &definition.contract.acceptance,
    ] {
        if entries.len() > 100 {
            return Err(Failure::usage("An agent contract has too many entries."));
        }
        for entry in entries {
            checked("agent contract", entry, 4000)?;
        }
    }
    let mut harnesses = HashSet::new();
    for assignment in &definition.assignments {
        if !harnesses.insert(&assignment.harness) {
            return Err(Failure::usage(
                "Each harness needs exactly one explicit assignment.",
            ));
        }
        adapter(&assignment.harness)?.validate(assignment)?;
    }
    Ok(())
}

fn adapter(harness: &str) -> Result<Box<dyn adapters::Adapter>, Failure> {
    adapters::get(harness).ok_or_else(|| Failure::usage("Unsupported agent harness."))
}
fn required<'a>(args: &'a Args, key: &str) -> Result<&'a str, Failure> {
    args.opt(key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Failure::usage(format!("Agent operation requires --{key}.")))
}
fn agent_id(args: &Args) -> Result<&str, Failure> {
    args.positional(1)
        .ok_or_else(|| Failure::usage("Agent operation requires a stable agent id."))
}
fn find_agent<'a>(ledger: &'a Ledger, id: &str) -> Result<&'a Agent, Failure> {
    ledger.agents.get(id).ok_or_else(|| {
        Failure::usage("No agent has this stable id; identity is never inferred from a name.")
    })
}
fn assignment<'a>(agent: &'a Agent, harness: &str) -> Result<&'a Assignment, Failure> {
    agent
        .definition
        .assignments
        .iter()
        .find(|assignment| assignment.harness == harness)
        .ok_or_else(|| Failure::Model("The contract has no assignment for this harness.".into()))
}

fn safe_target(root: &Path, harness: &str, relative: &str) -> Result<PathBuf, Failure> {
    let adapter = adapter(harness)?;
    let target = adapters::safe_path(root, relative, adapter.directory())?;
    if Path::new(relative)
        .extension()
        .and_then(|extension| extension.to_str())
        != Some(adapter.extension())
    {
        return Err(Failure::usage(
            "Agent target extension must match its harness.",
        ));
    }
    safe_root(root)?;
    let normalized = fs::canonicalize(root)?.join(relative);
    if global_store_path()
        .and_then(|home| fs::canonicalize(home).ok())
        .is_some_and(|home| normalized.starts_with(home))
    {
        return Err(Failure::usage(
            "Agent targets must not be inside the global store.",
        ));
    }
    Ok(target)
}

fn safe_root(root: &Path) -> Result<(), Failure> {
    let root = fs::canonicalize(root)?;
    for variable in ["USERPROFILE", "HOME"] {
        if std::env::var_os(variable)
            .and_then(|home| fs::canonicalize(home).ok())
            .is_some_and(|home| home == root)
        {
            return Err(Failure::usage(
                "Agent custody targets a project lane, not the user's home directory.",
            ));
        }
    }
    if let Some(home) = global_store_path().and_then(|home| fs::canonicalize(home).ok()) {
        if root.starts_with(home) {
            return Err(Failure::usage(
                "Agent targets must not be inside the global store.",
            ));
        }
    }
    Ok(())
}

fn global_store_path() -> Option<PathBuf> {
    store::resolve_store_dir(
        std::env::var_os("VIVAC_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
}
fn read_current(root: &Path, harness: &str, relative: &str) -> Result<Option<Vec<u8>>, Failure> {
    let path = safe_target(root, harness, relative)?;
    match fs::metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.len() > 1024 * 1024 => Err(Failure::usage(
            "An agent target must be a regular file of at most 1 MiB.",
        )),
        Ok(_) => {
            let bytes = fs::read(path)?;
            if bytes.len() > 1024 * 1024 {
                return Err(Failure::usage(
                    "The native agent file grew past its size limit.",
                ));
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn revalidate_target(
    root: &Path,
    harness: &str,
    relative: &str,
    baseline: &Option<String>,
) -> Result<(), Failure> {
    let current = read_current(root, harness, relative)?.map(|bytes| adapters::digest(&bytes));
    if current != *baseline {
        return Err(Failure::Model(
            "The native agent target changed before its binding was recorded.".into(),
        ));
    }
    Ok(())
}
fn candidate_matches(candidate: &Candidate, assignment: &Assignment) -> bool {
    candidate.problem.is_none()
        && candidate.unsupported.is_empty()
        && candidate.name.as_deref() == Some(&assignment.name)
        && candidate.model.as_deref() == Some(&assignment.model)
        && candidate.effort.as_deref() == Some(&assignment.effort)
        && candidate.settings == assignment.settings
}

fn read_ledger(cwd: &Path) -> Result<(Located, String, Ledger), Failure> {
    let located = store::locate(cwd)?.ok_or(Failure::NoStore)?;
    safe_root(&located.lane_dir)?;
    let store = Store::open_strict(located.root.clone())?;
    let tree = crate::index::load(&store, false)?;
    if tree.broken_lines != 0 {
        return Err(Failure::Io(std::io::Error::other(
            "Agent custody refuses an unreadable event log.",
        )));
    }
    let lane = located
        .lane
        .as_ref()
        .map(|lane| lane.id.clone())
        .unwrap_or_else(|| crate::lane::MAIN.to_string());
    let ledger = replay(&tree)?;
    Ok((located, lane, ledger))
}

fn observed_state(
    agent: &Agent,
    selected: Option<&Assignment>,
    observation: Option<&Observation>,
) -> &'static str {
    match observation {
        None => "unverified",
        Some(observation) if observation.revision != agent.revision => "stale",
        Some(observation)
            if selected.is_some_and(|assignment| {
                assignment.model == observation.model && assignment.effort == observation.effort
            }) =>
        {
            "reported_match"
        }
        Some(_) => "reported_mismatch",
    }
}

fn unmanaged_candidates(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    harness: Option<&str>,
) -> Result<Vec<Candidate>, Failure> {
    let mut unmanaged = Vec::new();
    for native in adapters::all()
        .into_iter()
        .filter(|native| harness.is_none_or(|harness| native.name() == harness))
    {
        for path in native.paths(root)? {
            if !ledger.bindings.contains_key(&(
                lane.to_string(),
                native.name().into(),
                path.clone(),
            )) {
                unmanaged.push(native.inspect(root, &path)?);
            }
        }
    }
    Ok(unmanaged)
}

fn custody_inventory(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    id: Option<&str>,
    harness: Option<&str>,
) -> Result<Value, Failure> {
    let mut unbound = Vec::new();
    let mut unverified = Vec::new();
    for agent in ledger
        .agents
        .values()
        .filter(|agent| id.is_none_or(|id| agent.agent == id))
    {
        for (key, _) in ledger.bindings.iter().filter(|(key, binding)| {
            key.0 == lane
                && binding.agent == agent.agent
                && harness.is_none_or(|harness| key.1 == harness)
        }) {
            let observed = observed_state(
                agent,
                assignment(agent, &key.1).ok(),
                ledger.observations.get(&owned(key, &agent.agent)),
            );
            if observed != "reported_match" && !agent.definition.retired {
                unverified.push(
                    json!({"agent":agent.agent,"harness":key.1,"path":key.2,"state":observed}),
                );
            }
        }
        for selected in agent
            .definition
            .assignments
            .iter()
            .filter(|selected| harness.is_none_or(|harness| selected.harness == harness))
        {
            let bound = ledger.bindings.iter().any(|(key, binding)| {
                key.0 == lane && key.1 == selected.harness && binding.agent == agent.agent
            });
            if !bound {
                unbound.push(json!({"agent":agent.agent,"harness":selected.harness}));
                if !agent.definition.retired {
                    unverified.push(json!({"agent":agent.agent,"harness":selected.harness,"path":null,"state":"unbound"}));
                }
            }
        }
    }
    Ok(json!({"unbound":unbound,"unverified":unverified,
        "unmanaged":unmanaged_candidates(root,lane,ledger,harness)?}))
}

fn status_value(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    id: Option<&str>,
    harness: Option<&str>,
) -> Result<Value, Failure> {
    if let Some(id) = id {
        find_agent(ledger, id)?;
    }
    if let Some(harness) = harness {
        adapter(harness)?;
    }
    let mut agents = Vec::new();
    let mut errors = Vec::new();
    let mut unverified = Vec::new();
    for agent in ledger
        .agents
        .values()
        .filter(|agent| id.is_none_or(|id| agent.agent == id))
    {
        let mut mappings = Vec::new();
        for (key, binding) in ledger.bindings.iter().filter(|(key, binding)| {
            key.0 == lane
                && binding.agent == agent.agent
                && harness.is_none_or(|harness| key.1 == harness)
        }) {
            let native = adapter(&key.1)?;
            let current = read_current(root, &key.1, &key.2)?;
            let receipt = ledger.receipts.get(&owned(key, &agent.agent));
            let selected = assignment(agent, &key.1).ok();
            let configured = if agent.definition.retired {
                "retired"
            } else if selected.is_none() {
                "no_assignment"
            } else if current.is_none() {
                "missing"
            } else if receipt.is_some_and(|receipt| {
                receipt.digest == adapters::digest(current.as_ref().unwrap())
                    && equivalent_receipt(ledger, receipt, agent, &key.1, native.version())
            }) {
                "current"
            } else if receipt
                .map(|receipt| &receipt.digest)
                .or(binding.baseline.as_ref())
                .is_some_and(|baseline| *baseline == adapters::digest(current.as_ref().unwrap()))
            {
                "pending"
            } else {
                "diverged"
            };
            let observation = ledger.observations.get(&owned(key, &agent.agent));
            let observed = observed_state(agent, selected, observation);
            if configured != "current" && configured != "retired" {
                errors.push(json!({"agent": agent.agent, "harness": key.1, "path": key.2, "state": configured}));
            }
            if observed != "reported_match" && !agent.definition.retired {
                unverified.push(json!({"agent": agent.agent, "harness": key.1, "path": key.2, "state": observed}));
            }
            mappings.push(json!({"harness": key.1, "path": key.2, "configured": configured, "observed": {"state": observed, "reported": observation}}));
        }
        for selected in agent
            .definition
            .assignments
            .iter()
            .filter(|selected| harness.is_none_or(|harness| selected.harness == harness))
        {
            let bound = ledger.bindings.iter().any(|(key, binding)| {
                key.0 == lane && key.1 == selected.harness && binding.agent == agent.agent
            });
            if !bound {
                mappings.push(json!({"harness": selected.harness, "path": null, "configured": "unbound", "observed": {"state": "unbound", "reported": null}}));
                if !agent.definition.retired {
                    unverified.push(json!({"agent": agent.agent, "harness": selected.harness, "path": null, "state": "unbound"}));
                }
            }
        }
        agents.push(json!({"agent": agent.agent, "revision": agent.revision, "name": agent.definition.name, "retired": agent.definition.retired, "mappings": mappings}));
    }
    let unmanaged = unmanaged_candidates(root, lane, ledger, harness)?;
    let adapters: Vec<Value> = adapters::all().into_iter().filter(|native| harness.is_none_or(|harness| native.name() == harness))
        .map(|native| json!({"harness": native.name(), "version": native.version(), "precedence": native.precedence(), "capabilities": native.capabilities()})).collect();
    Ok(
        json!({"adapters": adapters, "agents": agents, "unmanaged": unmanaged, "errors": errors, "unverified": unverified}),
    )
}
pub(crate) fn diagnosis(cwd: &Path, chosen: Option<&str>) -> Result<Value, Failure> {
    let (located, lane, ledger) = read_ledger(cwd)?;
    status_value(&located.lane_dir, &lane, &ledger, None, chosen)
}

fn validate_args(args: &Args) -> Result<&str, Failure> {
    let command = args
        .positional(0)
        .ok_or_else(|| Failure::usage("Agent operation required."))?;
    let (takes, allowed): (usize, &[&str]) = match command {
        "inventory" => (1, &["json"]),
        "plan" | "compare" => (1, &["selection", "json"]),
        "apply" => (1, &["selection", "plan-digest", "yes", "json"]),
        "scan" => (1, &["harness", "json"]),
        "status" | "diff" => (2, &["harness", "json"]),
        "show" => (2, &["json"]),
        "add" => (1, &["definition", "why", "parent", "against", "json"]),
        "set" => (2, &["definition", "why", "parent", "against", "json"]),
        "retire" => (2, &["why", "parent", "against", "json"]),
        "bind" | "detach" => (2, &["harness", "path", "json"]),
        "adopt" => (
            2,
            &[
                "harness", "path", "digest", "why", "parent", "against", "json",
            ],
        ),
        "observe" => (
            2,
            &[
                "harness", "path", "revision", "model", "effort", "evidence", "json",
            ],
        ),
        "sync" => (2, &["harness", "yes", "dry-run", "accept-digest", "json"]),
        "reconcile" => (
            2,
            &[
                "harness", "yes", "dry-run", "mode", "why", "parent", "against", "json",
            ],
        ),
        "import" => (
            2,
            &["harness", "path", "why", "parent", "against", "yes", "json"],
        ),
        _ => return Err(Failure::usage("Unknown agent operation.")),
    };
    if !args.unknown(allowed).is_empty() || !args.extra(takes).is_empty() {
        return Err(Failure::usage(
            "Unknown option or extra positional for this agent operation.",
        ));
    }
    for option in allowed
        .iter()
        .filter(|option| !["yes", "dry-run", "json"].contains(option))
    {
        if args.has(option) && args.list(option).is_empty() {
            return Err(Failure::usage("Agent option requires a value."));
        }
        if *option != "against" && args.list(option).len() > 1 {
            return Err(Failure::usage("Agent option may only appear once."));
        }
    }
    if command == "sync" && args.has("accept-digest") && (!args.has("yes") || args.has("dry-run")) {
        return Err(Failure::usage(
            "Accepting a manual digest requires --yes and cannot be a dry run.",
        ));
    }
    if command == "reconcile" {
        if args.has("dry-run") && args.has("yes") {
            return Err(Failure::usage(
                "Reconcile takes --yes or --dry-run, never both.",
            ));
        }
        if let Some(mode) = args.opt("mode") {
            if !matches!(mode, "automatic" | "manual")
                || !args.has("yes")
                || args.has("dry-run")
                || args.positional(1).is_some()
                || args.has("harness")
            {
                return Err(Failure::usage("A lane policy requires --mode automatic|manual --yes and no destination filter."));
            }
            required(args, "why")?;
        } else if ["why", "parent", "against"].iter().any(|key| args.has(key)) {
            return Err(Failure::usage("Policy provenance requires --mode."));
        }
    }
    if command == "import" {
        required(args, "harness")?;
        required(args, "path")?;
        required(args, "why")?;
        if !args.has("yes") {
            return Err(Failure::usage("Native import requires --yes."));
        }
    }
    if matches!(command, "plan" | "apply" | "compare") {
        required(args, "selection")?;
    }
    if command == "apply" {
        required(args, "plan-digest")?;
        if !args.has("yes") {
            return Err(Failure::usage(
                "Applying a reviewed agent plan requires --yes.",
            ));
        }
    }
    if matches!(
        command,
        "show" | "diff" | "set" | "retire" | "bind" | "detach" | "adopt" | "observe"
    ) {
        agent_id(args)?;
    }
    Ok(command)
}

pub(crate) fn response(result: Result<(Value, i32), Failure>) -> (Value, i32) {
    match result {
        Ok(result) => result,
        Err(error) => {
            let code = error.code();
            let message = match error {
                Failure::Usage(message)
                | Failure::Model(message)
                | Failure::Busy(message)
                | Failure::NotALane(message) => message,
                Failure::NewerVivac(_) => "The tree's config requires a newer vivac; update vivac before managing agents. Configuration values are withheld.".into(),
                Failure::Redaction(_) => {
                    "The redaction guard refused agent data; nothing was recorded.".into()
                }
                _ => {
                    "Agent custody could not complete the operation; inspect tree and file access."
                        .into()
                }
            };
            (json!({"error": message, "code": code}), code)
        }
    }
}

pub(crate) fn run_value(cwd: &Path, args: &Args) -> (Value, i32) {
    response(execute(cwd, args))
}

pub(crate) fn reconcile_for_hook(cwd: &Path) -> (Value, i32) {
    reconcile::hook(cwd)
}

pub fn run(cwd: &Path, args: &Args) -> Result<i32, Failure> {
    use std::io::IsTerminal;
    if args.positional(0).is_none() {
        if !args.unknown(&["json"]).is_empty() {
            return Err(Failure::usage("Unknown inventory option."));
        }
        if !args.has("json") {
            return human::run(cwd, args);
        }
        let (value, code) = response(workflow::inventory(cwd).map(|value| (value, 0)));
        crate::output::outln!(
            "{}",
            serde_json::to_string(&value).map_err(std::io::Error::other)?
        );
        return Ok(code);
    }
    if args.positional(0) == Some("sync")
        && args.positional(1).is_none()
        && !args.has("harness")
        && !args.has("accept-digest")
        && !args.has("json")
        && !args.has("yes")
        && !args.has("dry-run")
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
    {
        validate_args(args)?;
        return human::sync(cwd, args);
    }
    let (value, code) = run_value(cwd, args);
    crate::output::outln!(
        "{}",
        serde_json::to_string(&value).map_err(std::io::Error::other)?
    );
    Ok(code)
}

fn execute(cwd: &Path, args: &Args) -> Result<(Value, i32), Failure> {
    let command = validate_args(args)?;
    match command {
        "inventory" => return workflow::inventory(cwd).map(|value| (value, 0)),
        "plan" => {
            return workflow::plan(
                cwd,
                &workflow::selection(cwd, required(args, "selection")?)?,
            )
            .map(|value| (value, 0))
        }
        "apply" => {
            return workflow::apply(
                cwd,
                &workflow::selection(cwd, required(args, "selection")?)?,
                required(args, "plan-digest")?,
            )
        }
        "compare" => {
            let comparison: workflow::Comparison =
                workflow::selection(cwd, required(args, "selection")?)?;
            return workflow::compare(cwd, &comparison.references).map(|value| (value, 0));
        }
        _ => {}
    }
    if matches!(command, "reconcile" | "import") {
        return reconcile::execute(cwd, args, false);
    }
    if matches!(command, "scan" | "status" | "show" | "diff")
        || (command == "sync" && (!args.has("yes") || args.has("dry-run")))
    {
        let (located, lane, ledger) = read_ledger(cwd)?;
        if command == "show" {
            return Ok((
                serde_json::to_value(find_agent(&ledger, agent_id(args)?)?)
                    .map_err(std::io::Error::other)?,
                0,
            ));
        }
        if matches!(command, "diff" | "sync") {
            let plan = plan(&located.lane_dir, &lane, &ledger, args)?;
            let code = i32::from(plan.changed);
            return Ok((json!({"applied": false, "plan": plan.rows}), code));
        }
        let status = status_value(
            &located.lane_dir,
            &lane,
            &ledger,
            args.positional(1),
            args.opt("harness"),
        )?;
        let code = if command == "scan" {
            0
        } else {
            i32::from(
                !status["errors"].as_array().unwrap().is_empty()
                    || !status["unverified"].as_array().unwrap().is_empty(),
            )
        };
        return Ok((status, code));
    }
    let definition = if matches!(command, "add" | "set") {
        let source = required(args, "definition")?;
        let raw = if source.trim_start().starts_with('{') {
            source.to_string()
        } else {
            checked("agent definition path", source, 4096)?;
            let path = cwd.join(source);
            if fs::metadata(&path)?.len() > 1024 * 1024 {
                return Err(Failure::usage(
                    "An agent definition exceeds its size limit.",
                ));
            }
            fs::read_to_string(path)?
        };
        if raw.len() > 1024 * 1024 {
            return Err(Failure::usage(
                "An agent definition exceeds its size limit.",
            ));
        }
        let definition: Definition = serde_json::from_str(&raw).map_err(|_| {
            Failure::usage("Agent definition must be authored JSON with known schema fields.")
        })?;
        validate_definition(&definition)?;
        if definition.retired {
            return Err(Failure::usage("Use retire to retire a contract."));
        }
        Some(definition)
    } else {
        None
    };
    let located = store::locate(cwd)?.ok_or(Failure::NoStore)?;
    let mut ctx = Ctx::load_with_log(
        Store::open(located.root.clone())?,
        ops::Whose::Resolved(&located),
    )?
    .0;
    ctx.lock_for_write()?;
    let (_events, broken) = ctx.store.read_all()?;
    if broken != 0 {
        return Err(Failure::Io(std::io::Error::other(
            "Agent custody refuses an unreadable event log.",
        )));
    }
    let ledger = replay(&ctx.tree)?;
    match command {
        "add" | "set" | "retire" => record_revision(&mut ctx, &ledger, args, command, definition),
        "bind" | "adopt" | "detach" => bind(&mut ctx, &ledger, args, command),
        "observe" => observe(&mut ctx, &ledger, args),
        "sync" => sync(&mut ctx, &ledger, args),
        _ => Err(Failure::usage("Unsupported agent mutation.")),
    }
}

fn revision_event(ctx: &Ctx, args: &Args, title: &str) -> Result<(Body, String), Failure> {
    ops::agent_revision(
        ctx,
        title,
        required(args, "why")?,
        args.opt("parent"),
        args.list("against"),
    )
}

fn record_revision(
    ctx: &mut Ctx,
    ledger: &Ledger,
    args: &Args,
    command: &str,
    definition: Option<Definition>,
) -> Result<(Value, i32), Failure> {
    let previous = if command == "add" {
        None
    } else {
        Some(find_agent(ledger, agent_id(args)?)?)
    };
    if previous.is_some_and(|agent| agent.definition.retired) {
        return Err(Failure::Model("This contract is retired.".into()));
    }
    let definition = match definition {
        Some(definition) => definition,
        None => {
            let mut definition = previous.unwrap().definition.clone();
            definition.retired = true;
            definition
        }
    };
    let agent = previous.map_or_else(crate::id::ulid, |agent| agent.agent.clone());
    let title = format!(
        "{} agent contract {}",
        if command == "retire" {
            "Retire"
        } else {
            "Record"
        },
        definition.name
    );
    let (decision, revision) = revision_event(ctx, args, &title)?;
    let mut bodies = vec![decision];
    if let Some(previous) = previous {
        bodies.push(Body::StateChanged {
            node: previous.revision.clone(),
            state: State::Superseded,
            outcome: format!("Replaced by agent revision {revision}"),
            forced: false,
            until: None,
        });
    }
    bodies.push(Body::AgentRecorded {
        agent: agent.clone(),
        node: revision.clone(),
        definition,
    });
    ctx.emit(bodies)?;
    Ok((json!({"agent": agent, "revision": revision}), 0))
}

fn mutation_target(ctx: &Ctx, ledger: &Ledger, args: &Args) -> Result<(String, Target), Failure> {
    let agent = find_agent(ledger, agent_id(args)?)?;
    let harness = required(args, "harness")?;
    let path = required(args, "path")?;
    safe_target(&ctx.lane_dir, harness, path)?;
    let key = (
        ctx.lane.clone().unwrap_or_else(|| crate::lane::MAIN.into()),
        harness.to_string(),
        path.to_string(),
    );
    if ledger
        .bindings
        .get(&key)
        .is_some_and(|binding| binding.agent != agent.agent)
    {
        return Err(Failure::Model(
            "This native target is bound to another agent.".into(),
        ));
    }
    Ok((agent.agent.clone(), key))
}

fn bind(
    ctx: &mut Ctx,
    ledger: &Ledger,
    args: &Args,
    command: &str,
) -> Result<(Value, i32), Failure> {
    let (id, key) = mutation_target(ctx, ledger, args)?;
    let agent = find_agent(ledger, &id)?;
    if command == "detach" {
        if !ledger.bindings.contains_key(&key) {
            return Ok((json!({"agent": id, "detached": false}), 0));
        }
        ctx.emit(vec![Body::AgentDetached {
            agent: id.clone(),
            harness: key.1.clone(),
            path: key.2.clone(),
        }])?;
        return Ok((
            json!({"agent": id, "detached": true, "file_preserved": true}),
            0,
        ));
    }
    if agent.definition.retired {
        return Err(Failure::Model(
            "Retired contracts cannot acquire targets.".into(),
        ));
    }
    let selected = assignment(agent, &key.1)?;
    let current = read_current(&ctx.lane_dir, &key.1, &key.2)?;
    let baseline = current.as_ref().map(|bytes| adapters::digest(bytes));
    let mut bodies = Vec::new();
    if command == "adopt" {
        let expected = required(args, "digest")?;
        if baseline.as_deref() != Some(expected) {
            return Err(Failure::Model(
                "The adoption fingerprint does not match the current file.".into(),
            ));
        }
        let candidate = adapter(&key.1)?.inspect(&ctx.lane_dir, &key.2)?;
        if Some(candidate.digest.as_str()) != baseline.as_deref() {
            return Err(Failure::Model(
                "The native agent changed during adoption inspection.".into(),
            ));
        }
        if !candidate_matches(&candidate, selected) {
            return Err(Failure::Model("Native metadata does not match the reviewed assignment, or has unsupported fields.".into()));
        }
        let (decision, _) = revision_event(
            ctx,
            args,
            &format!("Adopt agent target {}", agent.definition.name),
        )?;
        bodies.push(decision);
    } else if let Some(digest) = &baseline {
        if ledger
            .receipts
            .get(&owned(&key, &id))
            .is_none_or(|receipt| receipt.digest != *digest)
        {
            return Err(Failure::Model(
                "An existing target needs explicit adoption with its reviewed fingerprint.".into(),
            ));
        }
    }
    if command == "bind" && ledger.bindings.contains_key(&key) {
        return Ok((json!({"agent": id, "bound": true, "changed": false}), 0));
    }
    revalidate_target(&ctx.lane_dir, &key.1, &key.2, &baseline)?;
    bodies.push(Body::AgentBound {
        agent: id.clone(),
        harness: key.1.clone(),
        path: key.2.clone(),
        baseline,
    });
    ctx.emit(bodies)?;
    Ok((json!({"agent": id, "bound": true, "changed": true}), 0))
}

fn observe(ctx: &mut Ctx, ledger: &Ledger, args: &Args) -> Result<(Value, i32), Failure> {
    let (id, key) = mutation_target(ctx, ledger, args)?;
    let agent = find_agent(ledger, &id)?;
    if agent.definition.retired || !ledger.bindings.contains_key(&key) {
        return Err(Failure::Model(
            "Observations need an active bound contract.".into(),
        ));
    }
    let revision = required(args, "revision")?;
    if revision != agent.revision {
        return Err(Failure::Model(
            "An observation must name the current declared revision.".into(),
        ));
    }
    let model = required(args, "model")?;
    let effort = required(args, "effort")?;
    let evidence = required(args, "evidence")?;
    checked("observed model", model, 200)?;
    checked("observed effort", effort, 100)?;
    checked("observation evidence", evidence, 4000)?;
    let selected = assignment(agent, &key.1)?;
    let code = i32::from(selected.model != model || selected.effort != effort);
    ctx.emit(vec![Body::AgentObserved {
        agent: id.clone(),
        revision: revision.to_string(),
        harness: key.1,
        path: key.2,
        model: model.to_string(),
        effort: effort.to_string(),
        evidence: evidence.to_string(),
    }])?;
    Ok((
        json!({"agent": id, "revision": revision, "state": if code == 0 { "reported_match" } else { "reported_mismatch" }, "runtime_verified": false}),
        code,
    ))
}

struct Plan {
    rows: Vec<Value>,
    writes: Vec<crate::setup::PlannedWrite>,
    receipts: Vec<Body>,
    checks: Vec<(Target, Option<String>)>,
    prompt_checks: Vec<PromptSource>,
    changed: bool,
    refused: bool,
}

fn plan(root: &Path, lane: &str, ledger: &Ledger, args: &Args) -> Result<Plan, Failure> {
    plan_selected(root, lane, ledger, args, None)
}

fn plan_selected(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    args: &Args,
    chosen_target: Option<&Target>,
) -> Result<Plan, Failure> {
    if let Some(id) = args.positional(1) {
        find_agent(ledger, id)?;
    }
    if let Some(harness) = args.opt("harness") {
        adapter(harness)?;
    }
    let mut plan = Plan {
        rows: vec![],
        writes: vec![],
        receipts: vec![],
        checks: vec![],
        prompt_checks: vec![],
        changed: false,
        refused: false,
    };
    for (key, binding) in ledger.bindings.iter().filter(|(key, binding)| {
        key.0 == lane
            && chosen_target.is_none_or(|target| *key == target)
            && args.positional(1).is_none_or(|id| binding.agent == id)
            && args.opt("harness").is_none_or(|harness| key.1 == harness)
    }) {
        let agent = find_agent(ledger, &binding.agent)?;
        if agent.definition.retired {
            plan.rows.push(json!({"agent": agent.agent, "harness": key.1, "path": key.2, "state": "retired", "file_preserved": true}));
            continue;
        }
        let native = adapter(&key.1)?;
        let selected = match assignment(agent, &key.1) {
            Ok(selected) => selected,
            Err(_) => {
                plan.changed = true;
                plan.refused = true;
                plan.rows.push(json!({"agent": agent.agent, "harness": key.1, "path": key.2, "state": "no_assignment"}));
                continue;
            }
        };
        let current = read_current(root, &key.1, &key.2)?;
        let current_digest = current.as_ref().map(|bytes| adapters::digest(bytes));
        let before = if current.is_some() {
            Some(native.inspect(root, &key.2)?)
        } else {
            None
        };
        let unsupported = before.as_ref().is_some_and(|candidate| {
            candidate.problem.is_some() || !candidate.unsupported.is_empty()
        });
        let receipt = ledger.receipts.get(&owned(key, &agent.agent));
        let baseline = receipt
            .map(|receipt| &receipt.digest)
            .or(binding.baseline.as_ref());
        let prompt = agent
            .definition
            .prompt
            .as_ref()
            .map(|source| {
                let body = adapters::read_prompt(root, &source.harness, &source.path)?;
                if adapters::digest(body.as_bytes()) != source.digest {
                    return Err(Failure::Model(
                        "Prompt source changed; import its reviewed revision before synchronizing."
                            .into(),
                    ));
                }
                Ok(body)
            })
            .transpose()?;
        let desired = native.render(
            &agent.agent,
            &agent.revision,
            &agent.definition,
            selected,
            prompt.as_deref(),
        )?;
        let desired_digest = adapters::digest(desired.as_bytes());
        let known = current_digest
            .as_ref()
            .is_none_or(|digest| baseline == Some(digest));
        let accepted = args.has("yes")
            && !args.has("dry-run")
            && current_digest
                .as_deref()
                .is_some_and(|digest| args.opt("accept-digest") == Some(digest));
        let complete = receipt.is_some_and(|receipt| {
            current_digest.as_deref() == Some(&receipt.digest)
                && equivalent_receipt(ledger, receipt, agent, &key.1, native.version())
        });
        let state = if unsupported {
            "unsupported"
        } else if !known && !accepted {
            "diverged"
        } else if current.is_none() {
            "missing"
        } else if complete {
            "current"
        } else {
            "pending"
        };
        plan.rows.push(json!({"agent": agent.agent, "revision": agent.revision, "harness": key.1, "path": key.2, "state": state, "before": before, "after": selected, "desired_digest": desired_digest, "manual_digest_accepted": accepted}));
        plan.changed |= !complete;
        if unsupported || (!known && !accepted) {
            plan.refused = true;
            continue;
        }
        if complete {
            continue;
        }
        let target = safe_target(root, &key.1, &key.2)?;
        if current_digest.as_deref() != Some(&desired_digest) {
            plan.writes
                .push(crate::setup::PlannedWrite::write(target, desired, current));
        }
        plan.checks.push((key.clone(), current_digest));
        if let Some(source) = &agent.definition.prompt {
            plan.prompt_checks.push(source.clone());
        }
        plan.receipts.push(Body::AgentMaterialized {
            agent: agent.agent.clone(),
            revision: agent.revision.clone(),
            harness: key.1.clone(),
            path: key.2.clone(),
            digest: desired_digest,
            adapter: native.version().to_string(),
        });
    }
    Ok(plan)
}

fn sync(ctx: &mut Ctx, ledger: &Ledger, args: &Args) -> Result<(Value, i32), Failure> {
    let lane = ctx.lane.clone().unwrap_or_else(|| crate::lane::MAIN.into());
    let plan = plan(&ctx.lane_dir, &lane, ledger, args)?;
    if plan.refused {
        return Ok((
            json!({"applied": false, "plan": plan.rows, "error": "Manual divergence or unsupported assignment requires explicit resolution."}),
            1,
        ));
    }
    apply_plan(ctx, &plan)
}

fn apply_plan(ctx: &mut Ctx, plan: &Plan) -> Result<(Value, i32), Failure> {
    for source in &plan.prompt_checks {
        let prompt = adapters::read_prompt(&ctx.lane_dir, &source.harness, &source.path)?;
        if adapters::digest(prompt.as_bytes()) != source.digest {
            return Err(Failure::Model(
                "Prompt source changed after the materialization plan.".into(),
            ));
        }
    }
    for (key, expected) in &plan.checks {
        let current =
            read_current(&ctx.lane_dir, &key.1, &key.2)?.map(|bytes| adapters::digest(&bytes));
        if current != *expected {
            return Err(Failure::Model(
                "An agent target changed after the materialization plan.".into(),
            ));
        }
    }
    crate::setup::commit(&plan.writes)?;
    if !plan.receipts.is_empty() && ctx.emit(plan.receipts.clone()).is_err() {
        let unrestored = crate::setup::rollback(&plan.writes);
        return Ok((
            json!({"applied": false, "receipt_failed": true, "rollback_attempted": true, "rollback_failed": !unrestored.is_empty(), "error": "Native writes were attempted but the custody receipt failed. Inspect every destination before retrying."}),
            5,
        ));
    }
    Ok((json!({"applied": plan.changed, "plan": plan.rows}), 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_prompt_changed_after_planning_cannot_be_materialized() {
        let root = std::env::temp_dir().join(format!("vivac-prompt-race-{}", crate::id::ulid()));
        fs::create_dir_all(root.join(".claude/agents")).unwrap();
        let source = ".claude/agents/reviewer.md";
        let original = "Inspect the scope.\n";
        let native = |body: &str| {
            format!("---\nname: reviewer\ndescription: Inspect the scope.\n---\n{body}")
        };
        fs::write(root.join(source), native(original)).unwrap();
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_with_log(store, ops::Whose::Founding).unwrap().0;
        ctx.lock_for_write().unwrap();
        let target = root.join(".codex/agents/reviewer.toml");
        let planned = Plan {
            rows: vec![],
            receipts: vec![],
            checks: vec![],
            changed: true,
            refused: false,
            writes: vec![crate::setup::PlannedWrite::write(
                target.clone(),
                "stale projection".into(),
                None,
            )],
            prompt_checks: vec![PromptSource {
                harness: "claude-code".into(),
                path: source.into(),
                digest: adapters::digest(original.as_bytes()),
            }],
        };
        let before = fs::read(root.join(".vivac/events")).unwrap();
        fs::write(root.join(source), native("Inspect the revised scope.\n")).unwrap();
        assert_eq!(apply_plan(&mut ctx, &planned).unwrap_err().code(), 1);
        assert!(!target.exists());
        assert_eq!(before, fs::read(root.join(".vivac/events")).unwrap());
        ctx.unlock();
        fs::remove_dir_all(root).unwrap();
    }

    // Enter the same operations with an isolated context, without resolving
    // this machine's registry or changing process-global environment variables.
    fn local(ctx: &mut Ctx, args: &Args) -> Result<(Value, i32), Failure> {
        let (_events, broken) = ctx.store.read_all()?;
        assert_eq!(broken, 0);
        let ledger = replay(&ctx.tree)?;
        match args.positional(0).unwrap() {
            command @ ("add" | "set") => {
                let definition = serde_json::from_str(required(args, "definition")?).unwrap();
                validate_definition(&definition)?;
                record_revision(ctx, &ledger, args, command, Some(definition))
            }
            command @ ("bind" | "adopt") => bind(ctx, &ledger, args, command),
            "sync" if args.has("yes") => sync(ctx, &ledger, args),
            "sync" | "diff" => {
                let plan = plan(&ctx.lane_dir, crate::lane::MAIN, &ledger, args)?;
                Ok((json!({"plan": plan.rows}), i32::from(plan.changed)))
            }
            "status" => Ok((
                status_value(
                    &ctx.lane_dir,
                    crate::lane::MAIN,
                    &ledger,
                    args.positional(1),
                    None,
                )?,
                0,
            )),
            _ => panic!("Unsupported fixture operation"),
        }
    }

    fn args(words: &[&str]) -> Args {
        Args::parse(words.iter().map(|word| word.to_string())).unwrap()
    }

    #[test]
    fn missing_target_binding_rejects_a_file_created_after_the_snapshot() {
        let root = std::env::temp_dir().join(format!("vivac-bind-race-{}", crate::id::ulid()));
        fs::create_dir_all(&root).unwrap();
        let relative = ".example/agents/reviewer.json";
        let baseline = read_current(&root, "example", relative)
            .unwrap()
            .map(|bytes| adapters::digest(&bytes));
        assert!(baseline.is_none());
        fs::create_dir_all(root.join(".example/agents")).unwrap();
        let external = json!({"name":"reviewer","model":"inherit","effort":"high","instructions":"An externally created file."}).to_string();
        fs::write(root.join(relative), &external).unwrap();
        let refusal = revalidate_target(&root, "example", relative, &baseline).unwrap_err();
        assert_eq!(refusal.code(), 1);
        assert_eq!(fs::read_to_string(root.join(relative)).unwrap(), external);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adoption_rejects_a_file_changed_during_native_inspection() {
        let root = std::env::temp_dir().join(format!("vivac-adopt-race-{}", crate::id::ulid()));
        fs::create_dir_all(&root).unwrap();
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_with_log(store, ops::Whose::Founding).unwrap().0;
        ctx.lock_for_write().unwrap();
        let definition = json!({"schema_version":1,"name":"reviewer","contract":{"purpose":"Review the scoped change.","duties":["Return evidence."],"limits":["Do not publish."],"acceptance":["Cite changed lines."]},"assignments":[{"harness":"example","name":"reviewer","model":"inherit","effort":"high","settings":{}}]}).to_string();
        let (created, code) = local(
            &mut ctx,
            &args(&[
                "add",
                "--definition",
                &definition,
                "--why",
                "Declare the reviewed assignment",
            ]),
        )
        .unwrap();
        assert_eq!(code, 0);
        let agent = created["agent"].as_str().unwrap();
        let relative = ".example/agents/reviewer.json";
        fs::create_dir_all(root.join(".example/agents")).unwrap();
        let original = json!({"name":"reviewer","model":"inherit","effort":"high","instructions":"Existing native body."}).to_string().into_bytes();
        fs::write(root.join(relative), &original).unwrap();
        let digest = adapters::digest(&original);
        let authority = fs::read(root.join(".vivac/events")).unwrap();
        fs::write(root.join(".test-race"), "simulate an external writer").unwrap();
        let result = local(
            &mut ctx,
            &args(&[
                "adopt",
                agent,
                "--harness",
                "example",
                "--path",
                relative,
                "--digest",
                &digest,
                "--why",
                "Adopt only the reviewed bytes",
            ]),
        );
        assert_eq!(result.unwrap_err().code(), 1);
        let mut external = original;
        external.push(b'\n');
        assert_eq!(fs::read(root.join(relative)).unwrap(), external);
        assert_eq!(fs::read(root.join(".vivac/events")).unwrap(), authority);
        ctx.unlock();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_registered_third_adapter_uses_the_unchanged_custody_cycle() {
        let root = std::env::temp_dir().join(format!("vivac-example-cycle-{}", crate::id::ulid()));
        fs::create_dir_all(&root).unwrap();
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_with_log(store, ops::Whose::Founding).unwrap().0;
        ctx.lock_for_write().unwrap();
        let definition = json!({"schema_version":1,"name":"reviewer","contract":{"purpose":"Review the scoped change.","duties":["Return evidence."],"limits":["Do not publish."],"acceptance":["Cite changed lines."]},"assignments":[{"harness":"example","name":"reviewer","model":"inherit","effort":"high","settings":{}}]}).to_string();
        let (created, code) = local(
            &mut ctx,
            &args(&[
                "add",
                "--definition",
                &definition,
                "--why",
                "Declare the reviewed example assignment",
            ]),
        )
        .unwrap();
        assert_eq!(code, 0);
        let agent = created["agent"].as_str().unwrap();
        let relative = ".example/agents/reviewer.json";
        assert_eq!(
            local(
                &mut ctx,
                &args(&["bind", agent, "--harness", "example", "--path", relative])
            )
            .unwrap()
            .1,
            0
        );
        let before = fs::read(root.join(".vivac/events")).unwrap();
        let (preview, code) = local(&mut ctx, &args(&["sync", agent])).unwrap();
        assert_eq!(code, 1);
        assert_eq!(preview["plan"][0]["state"], "missing");
        assert!(!root.join(relative).exists());
        assert_eq!(fs::read(root.join(".vivac/events")).unwrap(), before);
        assert_eq!(
            local(&mut ctx, &args(&["sync", agent, "--yes"])).unwrap().1,
            0
        );
        let original = fs::read(root.join(relative)).unwrap();
        let after = fs::read(root.join(".vivac/events")).unwrap();
        assert_eq!(
            local(&mut ctx, &args(&["sync", agent, "--yes"])).unwrap().1,
            0
        );
        assert_eq!(fs::read(root.join(".vivac/events")).unwrap(), after);
        let (status, _) = local(&mut ctx, &args(&["status", agent])).unwrap();
        assert_eq!(status["agents"][0]["mappings"][0]["configured"], "current");
        let mut manual: Value = serde_json::from_slice(&original).unwrap();
        manual["instructions"] = json!("Manual reviewed difference.");
        let manual = serde_json::to_vec(&manual).unwrap();
        fs::write(root.join(relative), &manual).unwrap();
        let (preview, code) = local(&mut ctx, &args(&["diff", agent])).unwrap();
        assert_eq!(code, 1);
        assert_eq!(preview["plan"][0]["state"], "diverged");
        assert_eq!(
            local(&mut ctx, &args(&["sync", agent, "--yes"])).unwrap().1,
            1
        );
        assert_eq!(fs::read(root.join(relative)).unwrap(), manual);
        let digest = adapters::digest(&manual);
        assert_eq!(
            local(
                &mut ctx,
                &args(&["sync", agent, "--yes", "--accept-digest", &digest])
            )
            .unwrap()
            .1,
            0
        );
        assert_eq!(fs::read(root.join(relative)).unwrap(), original);
        let mut revised: Value = serde_json::from_str(&definition).unwrap();
        revised["assignments"][0]["model"] = json!("explicit-model");
        assert_eq!(
            local(
                &mut ctx,
                &args(&[
                    "set",
                    agent,
                    "--definition",
                    &revised.to_string(),
                    "--why",
                    "Change the declared example model"
                ])
            )
            .unwrap()
            .1,
            0
        );
        let authority = fs::read(root.join(".vivac/events")).unwrap();
        fs::write(root.join(".test-race"), "simulate an external writer").unwrap();
        let result = local(&mut ctx, &args(&["sync", agent, "--yes"]));
        assert_eq!(result.unwrap_err().code(), 1);
        let mut external = original;
        external.push(b'\n');
        assert_eq!(fs::read(root.join(relative)).unwrap(), external);
        assert_eq!(fs::read(root.join(".vivac/events")).unwrap(), authority);
        fs::remove_dir_all(root).unwrap();
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    agent: Option<String>,
    definition: Option<Value>,
    harness: Option<String>,
    path: Option<String>,
    digest: Option<String>,
    why: Option<String>,
    parent: Option<String>,
    against: Option<Vec<String>>,
    revision: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    evidence: Option<String>,
    yes: Option<bool>,
    dry_run: Option<bool>,
    accept_digest: Option<String>,
    mode: Option<String>,
    selection: Option<Value>,
    plan_digest: Option<String>,
}

pub(crate) fn request_args(value: &Value) -> Result<Args, Failure> {
    if value
        .as_object()
        .is_some_and(|fields| fields.values().any(Value::is_null))
    {
        return Err(Failure::usage(
            "Agent arguments must use their declared types; null is not an option value.",
        ));
    }
    let request: Request = serde_json::from_value(value.clone()).map_err(|_| {
        Failure::usage("Agent arguments must use known fields and their declared types.")
    })?;
    let mut flags = Vec::new();
    if let Some(definition) = request.definition {
        if !definition.is_object() {
            return Err(Failure::usage(
                "Agent definition must be an authored JSON object.",
            ));
        }
        flags.push(format!("--definition={definition}"));
    }
    if let Some(selection) = request.selection {
        if !selection.is_object() {
            return Err(Failure::usage("Agent selection must be a JSON object."));
        }
        flags.push(format!("--selection={selection}"));
    }
    for (key, value) in [
        ("harness", request.harness),
        ("path", request.path),
        ("digest", request.digest),
        ("why", request.why),
        ("parent", request.parent),
        ("revision", request.revision),
        ("model", request.model),
        ("effort", request.effort),
        ("evidence", request.evidence),
        ("accept-digest", request.accept_digest),
        ("mode", request.mode),
        ("plan-digest", request.plan_digest),
    ] {
        if let Some(value) = value {
            flags.push(format!("--{key}={value}"));
        }
    }
    if let Some(values) = request.against {
        if values.is_empty()
            && !matches!(
                request.operation.as_str(),
                "add" | "set" | "retire" | "adopt" | "import" | "reconcile"
            )
        {
            flags.push("--against".into());
        }
        for value in values {
            flags.push(format!("--against={value}"));
        }
    }
    for (key, value) in [("yes", request.yes), ("dry-run", request.dry_run)] {
        if let Some(value) = value {
            if !matches!(
                request.operation.as_str(),
                "sync" | "reconcile" | "import" | "apply"
            ) || (request.operation == "import" && key == "dry-run")
                || value
            {
                flags.push(format!("--{key}"));
            }
        }
    }
    let mut args = Args::parse(flags)?;
    args.positionals.push(request.operation);
    args.positionals.extend(request.agent);
    validate_args(&args)?;
    Ok(args)
}
