//! The coordinator runs locally, independently of a harness model.
use super::*;

fn policy(ledger: &Ledger, lane: &str) -> Value {
    let selected = ledger.automation.get(lane);
    json!({"mode": if selected.is_some_and(|(enabled, _)| *enabled) {"automatic"} else {"manual"},
        "node": selected.map(|(_, node)| node)})
}

fn automatic(ledger: &Ledger, lane: &str) -> bool {
    ledger
        .automation
        .get(lane)
        .is_some_and(|(enabled, _)| *enabled)
}

fn selected(args: &Args, agent: &str, harness: &str) -> bool {
    args.positional(1).is_none_or(|id| id == agent)
        && args.opt("harness").is_none_or(|chosen| chosen == harness)
}

fn prepared(
    root: &Path,
    lane: &str,
    ledger: &Ledger,
    args: &Args,
) -> Result<(Plan, Value), Failure> {
    if let Some(id) = args.positional(1) {
        find_agent(ledger, id)?;
    }
    if let Some(harness) = args.opt("harness") {
        adapter(harness)?;
    }
    let mut combined = Plan {
        rows: vec![],
        writes: vec![],
        receipts: vec![],
        checks: vec![],
        prompt_checks: vec![],
        changed: false,
        refused: false,
    };
    let mut blocked = Vec::new();
    for (key, binding) in &ledger.bindings {
        if key.0 != lane || !selected(args, &binding.agent, &key.1) {
            continue;
        }
        match plan_selected(root, lane, ledger, args, Some(key)) {
            Ok(mut target) => {
                if target.refused {
                    blocked.extend(target.rows.clone());
                } else {
                    combined.writes.append(&mut target.writes);
                    combined.receipts.append(&mut target.receipts);
                    combined.checks.append(&mut target.checks);
                    combined.prompt_checks.append(&mut target.prompt_checks);
                    combined.changed |= target.changed;
                }
                combined.rows.append(&mut target.rows);
            }
            Err(error) => blocked.push(
                json!({"agent": binding.agent, "harness":key.1, "path":key.2,
                "state":"blocked", "reason":response(Err(error)).0["error"]}),
            ),
        }
    }
    let status =
        match custody_inventory(root, lane, ledger, args.positional(1), args.opt("harness")) {
            Ok(status) => status,
            Err(error) => {
                blocked.push(
                json!({"state":"discovery_unavailable","reason":response(Err(error)).0["error"]}),
            );
                json!({"unbound":[],"unmanaged":[],"unverified":[]})
            }
        };
    for mapping in status["unbound"].as_array().unwrap() {
        blocked.push(json!({"agent":mapping["agent"],"harness":mapping["harness"],"state":"unbound","reason":"Declare a destination explicitly."}));
    }
    let mut needs_review = Vec::new();
    let mut excluded = Vec::new();
    for candidate in status["unmanaged"].as_array().unwrap() {
        let key = (
            lane.to_string(),
            candidate["harness"].as_str().unwrap().to_string(),
            candidate["path"].as_str().unwrap().to_string(),
        );
        if ledger.detached.contains(&key) {
            excluded.push(candidate.clone());
        } else {
            needs_review
                .push(json!({"candidate":candidate,"reason":"Native agent is not yet imported."}));
        }
    }
    Ok((
        combined,
        json!({"policy":policy(ledger,lane),"needs_review":needs_review,"blocked":blocked,
        "excluded":excluded,"unverified":status["unverified"],"imported":[]}),
    ))
}

fn finish(mut result: Value, plan: &Plan, applied: bool, imported: &[Value]) -> (Value, i32) {
    result["plan"] = json!(plan.rows);
    result["applied"] = json!(applied);
    result["imported"] = json!(imported);
    let pending = !result["needs_review"].as_array().unwrap().is_empty()
        || !result["blocked"].as_array().unwrap().is_empty()
        || (!applied && plan.changed);
    (result, i32::from(pending))
}

pub(super) fn execute(cwd: &Path, args: &Args, hook: bool) -> Result<(Value, i32), Failure> {
    let (located, lane, ledger) = read_ledger(cwd)?;
    let write = args.has("yes") || (hook && automatic(&ledger, &lane));
    if !write {
        let (plan, result) = prepared(&located.lane_dir, &lane, &ledger, args)?;
        return Ok(finish(result, &plan, false, &[]));
    }
    if args.positional(0) == Some("reconcile") && !args.has("mode") {
        let (preview, result) = prepared(&located.lane_dir, &lane, &ledger, args)?;
        let importable = automatic(&ledger, &lane)
            && args.positional(1).is_none()
            && result["needs_review"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| {
                    entry["candidate"]["problem"].is_null()
                        && entry["candidate"]["unsupported"]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                });
        if !preview.changed && !importable {
            return Ok(finish(result, &preview, false, &[]));
        }
    }
    let mut ctx = Ctx::load_with_log(
        Store::open(located.root.clone())?,
        ops::Whose::Resolved(&located),
    )?
    .0;
    ctx.lock_for_write()?;
    if ctx.store.read_all()?.1 != 0 {
        return Err(Failure::Model(
            "Reconciliation refuses an unreadable event log.".into(),
        ));
    }
    let mut ledger = replay(&ctx.tree)?;
    if args.positional(0) == Some("import") {
        return import(&mut ctx, &ledger, args);
    }
    if let Some(mode) = args.opt("mode") {
        let enabled = mode == "automatic";
        if automatic(&ledger, &lane) != enabled {
            let (event, node) = revision_event(
                &ctx,
                args,
                if enabled {
                    "Authorize automatic agent reconciliation"
                } else {
                    "Disable automatic agent reconciliation"
                },
            )?;
            ctx.emit(vec![
                event,
                Body::AgentAutomationConfigured { node, enabled },
            ])?;
            ledger = replay(&ctx.tree)?;
        }
    }
    // Revocation is checked again under the write lock.
    let apply = (!hook || automatic(&ledger, &lane)) && args.opt("mode") != Some("manual");
    let mut imported = Vec::new();
    let mut refusals = Vec::new();
    if apply && automatic(&ledger, &lane) && args.positional(1).is_none() {
        let candidates = unmanaged_candidates(&ctx.lane_dir, &lane, &ledger, args.opt("harness"))
            .unwrap_or_default();
        for candidate in candidates {
            let candidate = serde_json::to_value(candidate).map_err(std::io::Error::other)?;
            let harness = candidate["harness"].as_str().unwrap();
            let path = candidate["path"].as_str().unwrap();
            if ledger
                .detached
                .contains(&(lane.clone(), harness.into(), path.into()))
            {
                continue;
            }
            let input = Args::parse(["import".into(),format!("--harness={harness}"),format!("--path={path}"),
                "--why=Preserve a discovered native agent under the authorized continuous custody policy".into(),"--yes".into()])?;
            match import(&mut ctx,&ledger,&input) {
                Ok((value,_)) => { imported.push(value); ledger=replay(&ctx.tree)?; }
                Err(_) => refusals.push(json!({"harness":harness,"path":path,"state":"blocked",
                    "reason":"Native import requires supported metadata and a safe prompt; contents withheld."})),
            }
        }
    }
    let (plan, mut result) = prepared(&ctx.lane_dir, &lane, &ledger, args)?;
    result["blocked"].as_array_mut().unwrap().extend(refusals);
    if apply {
        let (applied, code) = apply_plan(&mut ctx, &plan)?;
        if code != 0 {
            return Ok((applied, code));
        }
    }
    Ok(finish(
        result,
        &plan,
        apply && (plan.changed || !imported.is_empty()),
        &imported,
    ))
}

fn import(ctx: &mut Ctx, ledger: &Ledger, args: &Args) -> Result<(Value, i32), Failure> {
    let harness = required(args, "harness")?;
    let path = required(args, "path")?;
    let native = adapter(harness)?;
    let lane = ctx.lane.clone().unwrap_or_else(|| crate::lane::MAIN.into());
    let key = (lane, harness.into(), path.into());
    let candidate = native.inspect(&ctx.lane_dir, path)?;
    if candidate.problem.is_some() || !candidate.unsupported.is_empty() {
        return Err(Failure::usage("Native import requires supported metadata."));
    }
    let (body, description, source_digest) = adapters::read_source(&ctx.lane_dir, harness, path)?;
    if source_digest != candidate.digest {
        return Err(Failure::Model(
            "Native source changed during import.".into(),
        ));
    }
    let assignment = Assignment {
        harness: harness.into(),
        name: candidate.name.clone().unwrap(),
        model: candidate.model.clone().unwrap(),
        effort: candidate.effort.clone().unwrap(),
        settings: candidate.settings.clone(),
    };
    let previous = args
        .positional(1)
        .map(|id| find_agent(ledger, id))
        .transpose()?;
    if ledger
        .bindings
        .get(&key)
        .is_some_and(|binding| previous.is_none_or(|agent| agent.agent != binding.agent))
    {
        return Err(Failure::usage(
            "This destination already belongs to a different identity.",
        ));
    }
    if previous.is_some_and(|agent| agent.definition.retired) {
        return Err(Failure::usage("This agent is retired."));
    }
    let agent = previous.map_or_else(crate::id::ulid, |agent| agent.agent.clone());
    let mut definition = previous
        .map(|agent| agent.definition.clone())
        .unwrap_or_else(|| Definition {
            schema_version: 1,
            name: candidate.name.clone().unwrap(),
            retired: false,
            prompt: None,
            contract: types::Contract {
                purpose: description.clone(),
                duties: vec!["Execute the referenced native prompt.".into()],
                limits: vec![],
                acceptance: vec![
                    "Preserve the referenced instructions across managed destinations.".into(),
                ],
            },
            assignments: vec![],
        });
    definition.contract.purpose = description;
    definition
        .assignments
        .retain(|selected| selected.harness != harness);
    definition.assignments.push(assignment);
    definition.prompt = Some(PromptSource {
        harness: harness.into(),
        path: path.into(),
        digest: adapters::digest(body.as_bytes()),
    });
    validate_definition(&definition)?;
    revalidate_target(
        &ctx.lane_dir,
        harness,
        path,
        &Some(candidate.digest.clone()),
    )?;
    if previous.is_some_and(|old| old.definition == definition)
        && ledger
            .receipts
            .get(&owned(&key, &agent))
            .map(|receipt| &receipt.digest)
            .or_else(|| {
                ledger
                    .bindings
                    .get(&key)
                    .and_then(|binding| binding.baseline.as_ref())
            })
            == Some(&candidate.digest)
    {
        return Ok((
            json!({"agent":agent,"revision":previous.unwrap().revision,"unchanged":true}),
            0,
        ));
    }
    let (event, node) = revision_event(
        ctx,
        args,
        &format!("Import native agent {}", candidate.name.unwrap()),
    )?;
    ctx.emit(vec![
        event,
        Body::AgentRecorded {
            agent: agent.clone(),
            node: node.clone(),
            definition,
        },
        Body::AgentBound {
            agent: agent.clone(),
            harness: harness.into(),
            path: path.into(),
            baseline: Some(candidate.digest),
        },
    ])?;
    Ok((
        json!({"agent":agent,"revision":node,"prompt_referenced":true}),
        0,
    ))
}

pub(crate) fn hook(cwd: &Path) -> (Value, i32) {
    let args = Args::parse(["reconcile".into()]).unwrap();
    response(execute(cwd, &args, true))
}
