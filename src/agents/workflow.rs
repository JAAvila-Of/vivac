//! Reviewed native-agent transfers share one plan across human and machine callers.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRef {
    pub harness: String,
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncDestination {
    pub assignment: Assignment,
    pub path: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncItem {
    pub agent: Option<String>,
    pub source: NativeRef,
    pub destinations: Vec<SyncDestination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncSelection {
    pub why: String,
    pub items: Vec<SyncItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Comparison {
    pub references: Vec<NativeRef>,
}

pub(crate) fn inventory(cwd: &Path) -> Result<Value, Failure> {
    let (located, lane, ledger) = read_ledger(cwd)?;
    let root = &located.lane_dir;
    let mut result = status_value(root, &lane, &ledger, None, None)?;
    for row in result["agents"].as_array_mut().unwrap() {
        let agent = find_agent(&ledger, row["agent"].as_str().unwrap())?;
        row["definition"] =
            serde_json::to_value(&agent.definition).map_err(std::io::Error::other)?;
        let mut sources = Vec::new();
        for (key, binding) in &ledger.bindings {
            if key.0 == lane
                && binding.agent == agent.agent
                && read_current(root, &key.1, &key.2)?.is_some()
            {
                sources.push(adapter(&key.1)?.inspect(root, &key.2)?);
            }
        }
        if let Some(source) = &agent.definition.prompt {
            if !sources.iter().any(|candidate| {
                candidate.harness == source.harness && candidate.path == source.path
            }) && read_current(root, &source.harness, &source.path)?.is_some()
            {
                sources.push(adapter(&source.harness)?.inspect(root, &source.path)?);
            }
        }
        row["sources"] = serde_json::to_value(sources).map_err(std::io::Error::other)?;
    }
    for row in result["unmanaged"].as_array_mut().unwrap() {
        row["detached"] = json!(ledger.detached.contains(&(
            lane.clone(),
            row["harness"].as_str().unwrap().into(),
            row["path"].as_str().unwrap().into()
        )));
    }
    let mut harnesses = Vec::new();
    for native in adapters::all() {
        harnesses.push(harness_value(root, &ledger, native.as_ref())?);
    }
    result["harnesses"] = json!(harnesses);
    Ok(result)
}

pub(crate) fn harness_inventory(cwd: &Path, harness: &str) -> Result<Value, Failure> {
    let (located, _, ledger) = read_ledger(cwd)?;
    let native = adapter(harness)?;
    harness_value(&located.lane_dir, &ledger, native.as_ref())
}

fn harness_value(
    root: &Path,
    ledger: &Ledger,
    native: &dyn adapters::Adapter,
) -> Result<Value, Failure> {
    let model_catalog = native.model_catalog(root);
    let mut models = std::collections::BTreeSet::from(["inherit".to_string()]);
    models.extend(model_catalog.models.iter().map(|model| model.id.clone()));
    for agent in ledger.agents.values() {
        for assignment in &agent.definition.assignments {
            if assignment.harness == native.name() {
                models.insert(assignment.model.clone());
            }
        }
    }
    for candidate in native.discover(root)? {
        if let Some(model) = candidate.model {
            models.insert(model);
        }
    }
    let efforts: &[&str] = if native.name() == "codex" {
        &[
            "inherit", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ]
    } else {
        &["inherit", "low", "medium", "high", "xhigh", "max"]
    };
    Ok(
        json!({"harness":native.name(),"configured":crate::setup::doctor::configured(root,native.name()),
        "directory":native.directory(),"extension":native.extension(),"models":models,"efforts":efforts,"model_catalog":model_catalog,"capabilities":native.capabilities()}),
    )
}

fn reference(root: &Path, source: &NativeRef) -> Result<(Candidate, String, String), Failure> {
    safe_target(root, &source.harness, &source.path)?;
    let candidate = adapter(&source.harness)?.inspect(root, &source.path)?;
    if candidate.digest != source.digest {
        return Err(Failure::Model("Native source changed after review.".into()));
    }
    if candidate.problem.is_some() || !candidate.unsupported.is_empty() {
        return Err(Failure::usage(
            "Native source requires supported, safe metadata.",
        ));
    }
    let (body, description, digest) = adapters::read_source(root, &source.harness, &source.path)?;
    if digest != source.digest {
        return Err(Failure::Model(
            "Native source changed during review.".into(),
        ));
    }
    Ok((candidate, body, description))
}

pub(crate) fn compare(cwd: &Path, refs: &[NativeRef]) -> Result<Value, Failure> {
    if refs.is_empty() || refs.len() > 32 {
        return Err(Failure::usage("Compare takes one to 32 native references."));
    }
    let (located, _, _) = read_ledger(cwd)?;
    let mut size = 0;
    let mut sources = Vec::new();
    for source in refs {
        let (metadata, body, _) = reference(&located.lane_dir, source)?;
        size += body.len();
        if size > 4 * 1024 * 1024 {
            return Err(Failure::usage("Comparison exceeds its 4 MiB limit."));
        }
        sources.push(json!({"harness":source.harness,"path":source.path,"digest":source.digest,"body":body,"metadata":metadata}));
    }
    Ok(json!({"sources":sources,"historical_base":false}))
}

struct PreparedItem {
    definition: Definition,
    prompt: String,
    source: Candidate,
}

fn owner<'a>(ledger: &'a Ledger, key: &Target) -> Option<&'a Binding> {
    ledger
        .bindings
        .iter()
        .find(|(target, _)| {
            target.0 == key.0 && target.1 == key.1 && target.2.eq_ignore_ascii_case(&key.2)
        })
        .map(|(_, binding)| binding)
}

fn prepare(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    selection: &SyncSelection,
) -> Result<(Vec<PreparedItem>, Value), Failure> {
    checked("agent sync reason", &selection.why, 4000)?;
    if selection.why.trim().is_empty() || selection.items.is_empty() || selection.items.len() > 32 {
        return Err(Failure::usage(
            "Sync requires a reason and one to 32 selected agents.",
        ));
    }
    let mut paths = HashSet::new();
    let mut identities = HashSet::new();
    let mut prepared = Vec::new();
    let mut rows = Vec::new();
    let mut revisions = Vec::new();
    for item in &selection.items {
        let key = (
            lane.to_string(),
            item.source.harness.clone(),
            item.source.path.clone(),
        );
        let previous = item
            .agent
            .as_deref()
            .map(|id| find_agent(ledger, id))
            .transpose()?;
        if previous.is_some_and(|agent| agent.definition.retired) {
            return Err(Failure::usage("This agent is retired."));
        }
        if let Some(previous) = previous {
            if !identities.insert(previous.agent.clone()) {
                return Err(Failure::usage("Select each agent identity once."));
            }
        }
        if ledger.detached.contains(&key) {
            return Err(Failure::usage(
                "Detached agents require explicit import before synchronization.",
            ));
        }
        if owner(ledger, &key)
            .is_some_and(|binding| previous.is_none_or(|agent| binding.agent != agent.agent))
        {
            return Err(Failure::usage(
                "Native source belongs to another agent identity.",
            ));
        }
        if !paths.insert(item.source.path.to_ascii_lowercase()) {
            return Err(Failure::usage("Selected native paths overlap."));
        }
        let (source, prompt, description) = reference(root, &item.source)?;
        let mut definition = previous
            .map(|agent| agent.definition.clone())
            .unwrap_or_else(|| Definition {
                schema_version: 1,
                name: source.name.clone().unwrap(),
                retired: false,
                prompt: None,
                assignments: vec![],
                contract: types::Contract {
                    purpose: description.clone(),
                    duties: vec!["Execute the referenced native prompt.".into()],
                    limits: vec![],
                    acceptance: vec![
                        "Preserve the referenced instructions across managed destinations.".into(),
                    ],
                },
            });
        definition.contract.purpose = description;
        let original = Assignment {
            harness: source.harness.clone(),
            name: source.name.clone().unwrap(),
            model: source.model.clone().unwrap(),
            effort: source.effort.clone().unwrap(),
            settings: source.settings.clone(),
        };
        definition
            .assignments
            .retain(|assignment| assignment.harness != original.harness);
        definition.assignments.push(original);
        definition.prompt = Some(PromptSource {
            harness: source.harness.clone(),
            path: source.path.clone(),
            digest: adapters::digest(prompt.as_bytes()),
        });
        let mut destinations = Vec::new();
        let mut harnesses = HashSet::new();
        for destination in &item.destinations {
            let assignment = &destination.assignment;
            let native = adapter(&assignment.harness)?;
            native.validate(assignment)?;
            if assignment.harness == source.harness || !harnesses.insert(assignment.harness.clone())
            {
                return Err(Failure::usage(
                    "Each destination needs a distinct harness other than its source.",
                ));
            }
            if !crate::setup::doctor::configured(root, &assignment.harness) {
                return Err(Failure::usage(
                    "Set up the destination harness before synchronization.",
                ));
            }
            let target = safe_target(root, &assignment.harness, &destination.path)?;
            if !paths.insert(destination.path.to_ascii_lowercase()) {
                return Err(Failure::usage("Selected native paths overlap."));
            }
            let key = (
                lane.to_string(),
                assignment.harness.clone(),
                destination.path.clone(),
            );
            if ledger.detached.contains(&key) {
                return Err(Failure::usage("A destination is detached from custody."));
            }
            if owner(ledger, &key)
                .is_some_and(|binding| previous.is_none_or(|agent| agent.agent != binding.agent))
            {
                return Err(Failure::usage(
                    "Destination belongs to another agent identity.",
                ));
            }
            let current = read_current(root, &assignment.harness, &destination.path)?;
            if current.as_ref().map(|bytes| adapters::digest(bytes)) != destination.digest {
                return Err(Failure::Model("Destination changed after review.".into()));
            }
            if current.is_some() {
                let candidate = native.inspect(root, &destination.path)?;
                if candidate.problem.is_some() || !candidate.unsupported.is_empty() {
                    return Err(Failure::usage(
                        "Destination has unsupported metadata; preserve it before synchronization.",
                    ));
                }
                adapters::read_source(root, &assignment.harness, &destination.path)?;
            }
            definition
                .assignments
                .retain(|selected| selected.harness != assignment.harness);
            definition.assignments.push(assignment.clone());
            let _ = target;
            let before = if current.is_some() {
                Some(native.inspect(root, &destination.path)?)
            } else {
                None
            };
            destinations.push(json!({"harness":assignment.harness,"path":destination.path,"digest":destination.digest,"model":assignment.model,"effort":assignment.effort,"settings":assignment.settings,"before":before,"state":if current.is_some(){"replace_reviewed"}else{"create"}}));
        }
        definition
            .assignments
            .sort_by(|a, b| a.harness.cmp(&b.harness));
        validate_definition(&definition)?;
        // Rendering is validated before any revision or identity is allocated.
        for destination in &item.destinations {
            adapter(&destination.assignment.harness)?.render(
                "reviewed-agent",
                "reviewed-revision",
                &definition,
                &destination.assignment,
                Some(&prompt),
            )?;
        }
        revisions.push(json!({"agent":item.agent,"revision":previous.map(|agent|&agent.revision),"definition":definition}));
        rows.push(json!({"agent":item.agent,"source":item.source,"original":source,"destinations":destinations}));
        prepared.push(PreparedItem {
            definition,
            prompt,
            source,
        });
    }
    let normalized =
        serde_json::to_vec(&json!({"selection":selection,"revisions":revisions,"lane":lane}))
            .map_err(std::io::Error::other)?;
    Ok((
        prepared,
        json!({"plan_digest":adapters::digest(&normalized),"items":rows,"applied":false}),
    ))
}

pub(crate) fn plan(cwd: &Path, selection: &SyncSelection) -> Result<Value, Failure> {
    let (located, lane, ledger) = read_ledger(cwd)?;
    prepare(&located.lane_dir, &lane, &ledger, selection).map(|(_, value)| value)
}

pub(crate) fn apply(
    cwd: &Path,
    selection: &SyncSelection,
    plan_digest: &str,
) -> Result<(Value, i32), Failure> {
    let located = store::locate(cwd)?.ok_or(Failure::NoStore)?;
    let mut ctx = Ctx::load_with_log(
        Store::open(located.root.clone())?,
        ops::Whose::Resolved(&located),
    )?
    .0;
    ctx.lock_for_write()?;
    if ctx.store.read_all()?.1 != 0 {
        return Err(Failure::Model(
            "Agent transfer refuses an unreadable event log.".into(),
        ));
    }
    let ledger = replay(&ctx.tree)?;
    let lane = ctx.lane.clone().unwrap_or_else(|| crate::lane::MAIN.into());
    let (prepared, mut value) = prepare(&ctx.lane_dir, &lane, &ledger, selection)?;
    if value["plan_digest"].as_str() != Some(plan_digest) {
        return Err(Failure::Model(
            "Reviewed plan is stale; preview it again.".into(),
        ));
    }
    let mut events = Vec::new();
    let mut writes = Vec::new();
    let mut imported = Vec::new();
    let mut next_number = ctx.tree.next_num.max(1);
    for (item, prepared) in selection.items.iter().zip(prepared) {
        let previous = item
            .agent
            .as_deref()
            .map(|id| find_agent(&ledger, id))
            .transpose()?;
        let agent = previous.map_or_else(crate::id::ulid, |agent| agent.agent.clone());
        let (mut decision, revision) = ops::agent_revision(
            &ctx,
            &format!("Synchronize agent {}", prepared.definition.name),
            &selection.why,
            None,
            vec![],
        )?;
        if let Body::NodeCreated { num, .. } = &mut decision {
            *num = next_number;
        }
        next_number = next_number
            .checked_add(1)
            .ok_or_else(|| Failure::Model("Agent revision numbers are exhausted.".into()))?;
        events.push(decision);
        if let Some(previous) = previous {
            events.push(Body::StateChanged {
                node: previous.revision.clone(),
                state: State::Superseded,
                outcome: format!("Replaced by agent revision {revision}"),
                forced: false,
                until: None,
            });
        }
        events.push(Body::AgentRecorded {
            agent: agent.clone(),
            node: revision.clone(),
            definition: prepared.definition.clone(),
        });
        events.push(Body::AgentBound {
            agent: agent.clone(),
            harness: item.source.harness.clone(),
            path: item.source.path.clone(),
            baseline: Some(prepared.source.digest.clone()),
        });
        events.push(Body::AgentMaterialized {
            agent: agent.clone(),
            revision: revision.clone(),
            harness: item.source.harness.clone(),
            path: item.source.path.clone(),
            digest: prepared.source.digest,
            adapter: adapter(&item.source.harness)?.version().into(),
        });
        for destination in &item.destinations {
            let native = adapter(&destination.assignment.harness)?;
            let content = native.render(
                &agent,
                &revision,
                &prepared.definition,
                &destination.assignment,
                Some(&prepared.prompt),
            )?;
            let digest = adapters::digest(content.as_bytes());
            let original = read_current(
                &ctx.lane_dir,
                &destination.assignment.harness,
                &destination.path,
            )?;
            if original
                .as_ref()
                .map(|bytes| adapters::digest(bytes))
                .as_deref()
                != Some(&digest)
            {
                writes.push(crate::setup::PlannedWrite::write(
                    safe_target(
                        &ctx.lane_dir,
                        &destination.assignment.harness,
                        &destination.path,
                    )?,
                    content,
                    original,
                ));
            }
            events.push(Body::AgentBound {
                agent: agent.clone(),
                harness: destination.assignment.harness.clone(),
                path: destination.path.clone(),
                baseline: destination.digest.clone(),
            });
            events.push(Body::AgentMaterialized {
                agent: agent.clone(),
                revision: revision.clone(),
                harness: destination.assignment.harness.clone(),
                path: destination.path.clone(),
                digest,
                adapter: native.version().into(),
            });
        }
        imported.push(json!({"agent":agent,"revision":revision}));
    }
    // The complete batch is rechecked immediately before the first native write.
    for item in &selection.items {
        revalidate_target(
            &ctx.lane_dir,
            &item.source.harness,
            &item.source.path,
            &Some(item.source.digest.clone()),
        )?;
        for destination in &item.destinations {
            revalidate_target(
                &ctx.lane_dir,
                &destination.assignment.harness,
                &destination.path,
                &destination.digest,
            )?;
        }
    }
    crate::setup::commit(&writes)?;
    if ctx.emit(events).is_err() {
        let unrestored = crate::setup::rollback(&writes);
        return Ok((
            json!({"applied":false,"receipt_failed":true,"rollback_attempted":true,"rollback_failed":!unrestored.is_empty(),"error":"Native writes were attempted but the custody receipt failed. Inspect destinations before retrying."}),
            5,
        ));
    }
    value["applied"] = json!(true);
    value["imported"] = json!(imported);
    Ok((value, 0))
}

pub(super) fn selection<T: serde::de::DeserializeOwned>(
    cwd: &Path,
    input: &str,
) -> Result<T, Failure> {
    let raw = if input.trim_start().starts_with('{') {
        input.to_string()
    } else {
        checked("agent selection path", input, 4096)?;
        let path = cwd.join(input);
        if fs::metadata(&path)?.len() > 128 * 1024 {
            return Err(Failure::usage("Selection exceeds its 128 KiB limit."));
        }
        fs::read_to_string(path)?
    };
    if raw.len() > 128 * 1024 {
        return Err(Failure::usage("Selection exceeds its 128 KiB limit."));
    }
    serde_json::from_str(&raw)
        .map_err(|_| Failure::usage("Selection must use the declared JSON fields and types."))
}

#[cfg(test)]
mod catalog_refresh_tests {
    use super::*;

    #[test]
    fn model_choices_refresh_without_losing_configured_models() {
        const FIXTURE: &str = "VIVAC_CATALOG_REFRESH_FIXTURE";
        let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) else {
            let root =
                std::env::temp_dir().join(format!("vivac-catalog-refresh-{}", crate::id::ulid()));
            fs::create_dir_all(root.join(".codex/agents")).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "agents::workflow::catalog_refresh_tests::model_choices_refresh_without_losing_configured_models", "--nocapture"])
                .env(FIXTURE, &root)
                .env("CODEX_HOME", root.join(".codex"))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let started = std::time::Instant::now();
            let timed_out = loop {
                if child.try_wait().unwrap().is_some() {
                    break false;
                }
                if started.elapsed() > std::time::Duration::from_secs(30) {
                    child.kill().unwrap();
                    break true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let output = child.wait_with_output().unwrap();
            fs::remove_dir_all(root).unwrap();
            assert!(!timed_out, "Model refresh fixture timed out.");
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let native = adapters::get("codex").unwrap();
        let assignment = Assignment {
            harness: "codex".into(),
            name: "reviewer".into(),
            model: "used-native".into(),
            effort: "inherit".into(),
            settings: BTreeMap::new(),
        };
        let mut definition = Definition {
            schema_version: 1,
            name: "reviewer".into(),
            contract: types::Contract {
                purpose: "Review changes.".into(),
                duties: vec![],
                limits: vec![],
                acceptance: vec![],
            },
            assignments: vec![Assignment {
                model: "used-ledger".into(),
                ..assignment.clone()
            }],
            prompt: None,
            retired: false,
        };
        fs::write(
            root.join(".codex/agents/reviewer.toml"),
            native
                .render("agent", "revision", &definition, &assignment, None)
                .unwrap(),
        )
        .unwrap();
        let mut ledger = Ledger::default();
        ledger.agents.insert(
            "agent".into(),
            Agent {
                agent: "agent".into(),
                revision: "revision".into(),
                definition: definition.clone(),
            },
        );
        let cache = root.join(".codex/models_cache.json");
        fs::write(&cache, r#"{"models":[{"slug":"cached-old","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]}]}"#).unwrap();
        let before = harness_value(&root, &ledger, native.as_ref()).unwrap();
        assert_eq!(
            before["models"],
            json!(["cached-old", "inherit", "used-ledger", "used-native"])
        );
        fs::write(&cache, r#"{"models":[{"slug":"cached-new","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]}]}"#).unwrap();
        definition.assignments[0].model = "changed-ledger".into();
        ledger.agents.get_mut("agent").unwrap().definition = definition;
        let after = harness_value(&root, &ledger, native.as_ref()).unwrap();
        assert_eq!(
            after["models"],
            json!(["cached-new", "changed-ledger", "inherit", "used-native"])
        );
        assert_eq!(
            after["model_catalog"]["models"][0]["efforts"],
            json!(["high"])
        );
        assert_eq!(
            before["model_catalog"]["models"][0]["efforts"],
            json!(["low"])
        );
    }
}
