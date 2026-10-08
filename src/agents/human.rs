//! Human inventory and a short, reviewed synchronization flow.

use super::terminal::{clean, line, select, single};
use super::types::{Assignment, Candidate};
use super::workflow::{self, NativeRef, SyncDestination, SyncItem, SyncSelection};
use crate::args::Args;
use crate::failure::Failure;
use crate::style::{self, Stream::Out};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

macro_rules! say {
    ($($arg:tt)*) => { writeln!(io::stdout(), $($arg)*)? };
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn entries(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn candidate(value: &Value) -> Result<Candidate, Failure> {
    serde_json::from_value(value.clone())
        .map_err(|_| Failure::usage("Cannot read native agent metadata."))
}

fn reference(source: &Candidate) -> NativeRef {
    NativeRef {
        harness: source.harness.clone(),
        path: source.path.clone(),
        digest: source.digest.clone(),
    }
}

fn metadata(source: &Candidate) -> String {
    clean(&format!(
        "{}: {} | model {} | effort {}",
        source.harness,
        source.path,
        source.model.as_deref().unwrap_or("inherit"),
        source.effort.as_deref().unwrap_or("inherit")
    ))
}

struct Context {
    project: String,
    name: String,
    index: usize,
    total: usize,
}

impl Context {
    fn heading(&self, source: &str, destination: &str, step: usize) -> String {
        clean(&format!(
            "{} | Agent {}/{}: {} | {} -> {} | STEP {}/3: {}",
            self.project,
            self.index,
            self.total,
            self.name,
            source,
            destination,
            step,
            match step {
                1 => "Source & destinations",
                2 => "Assignments",
                _ => "Reviewed changes",
            }
        ))
    }

    fn show(&self, source: &str, destination: &str, step: usize) -> Result<(), Failure> {
        say!(
            "\n{}",
            style::bold(
                Out,
                &style::path(Out, &self.heading(source, destination, step))
            )
        );
        Ok(())
    }

    fn direction(&self, source: &str, destination: &str) -> String {
        clean(&format!("{} | {} -> {}", self.name, source, destination))
    }
}

pub(super) fn run(cwd: &Path, _args: &Args) -> Result<i32, Failure> {
    render_inventory(&workflow::inventory(cwd)?)?;
    Ok(0)
}

fn render_inventory(inventory: &Value) -> Result<(), Failure> {
    say!("{}", style::bold(Out, "Agents"));
    say!(
        "{}",
        style::dim(
            Out,
            "Configuration status; runtime evidence is shown separately"
        )
    );
    let mut managed: Vec<_> = entries(&inventory["agents"]).iter().collect();
    managed.sort_by_key(|agent| text(agent, "name"));
    for agent in managed {
        say!(
            "\n{} [{}] managed by vivac{}",
            style::bold(Out, &clean(text(agent, "name"))),
            clean(text(agent, "agent")),
            if agent["retired"] == true {
                " (retired)"
            } else {
                ""
            }
        );
        say!("  Custody: managed by vivac");
        for mapping in entries(&agent["mappings"]) {
            let state = text(mapping, "configured");
            let assignment = entries(&agent["definition"]["assignments"])
                .iter()
                .find(|assignment| text(assignment, "harness") == text(mapping, "harness"));
            let row = clean(&format!(
                "  {}: {} | Assigned model {} | effort {} | Configuration: {} | Runtime: {}",
                text(mapping, "harness"),
                mapping["path"].as_str().unwrap_or("not bound"),
                assignment
                    .map(|assignment| text(assignment, "model"))
                    .unwrap_or("not assigned"),
                assignment
                    .map(|assignment| text(assignment, "effort"))
                    .unwrap_or("not assigned"),
                state,
                mapping["observed"]["state"]
                    .as_str()
                    .unwrap_or("unverified")
            ));
            say!(
                "{}",
                match state {
                    "current" => style::good(Out, &row),
                    "retired" => style::dim(Out, &row),
                    "missing" => style::warn(Out, &row),
                    _ => style::gone(Out, &row),
                }
            );
        }
        for source in entries(&agent["sources"]) {
            say!("  Native: {}", metadata(&candidate(source)?));
        }
        for harness in entries(&inventory["harnesses"])
            .iter()
            .filter(|h| h["configured"] == true)
        {
            if !entries(&agent["mappings"])
                .iter()
                .any(|m| text(m, "harness") == text(harness, "harness"))
            {
                say!(
                    "{}",
                    style::warn(
                        Out,
                        &format!(
                            "  {}: missing; not configured for this agent",
                            clean(text(harness, "harness"))
                        )
                    )
                );
            }
        }
    }
    let mut unmanaged: Vec<_> = entries(&inventory["unmanaged"]).iter().collect();
    unmanaged.sort_by_key(|source| text(source, "name"));
    for source in unmanaged {
        let native = candidate(source)?;
        say!(
            "\n{} | not managed{}",
            style::bold(Out, &clean(native.name.as_deref().unwrap_or("unnamed"))),
            if source["detached"] == true {
                " (detached; excluded from sync)"
            } else {
                ""
            }
        );
        say!("  {}", metadata(&native));
        say!("  Custody: not managed | Configuration: native | Runtime: unverified");
        for harness in entries(&inventory["harnesses"])
            .iter()
            .filter(|h| h["configured"] == true && text(h, "harness") != native.harness)
        {
            say!(
                "{}",
                style::warn(
                    Out,
                    &format!(
                        "  {}: missing; not configured for this agent",
                        clean(text(harness, "harness"))
                    )
                )
            );
        }
        if let Some(problem) = &native.problem {
            say!("  blocked: {}", clean(problem));
        }
        if !native.unsupported.is_empty() {
            say!("  unsupported: {}", clean(&native.unsupported.join(", ")));
        }
    }
    say!("\nHarnesses:");
    for harness in entries(&inventory["harnesses"]) {
        say!(
            "  {}: {}",
            clean(text(harness, "harness")),
            if harness["configured"] == true {
                "configured"
            } else {
                "not configured"
            }
        );
    }
    say!("\nNext: vivac agents sync to choose sources, destinations and assignments, then review changes before applying.");
    io::stdout().flush()?;
    Ok(())
}

fn confirm(question: &str) -> Result<bool, Failure> {
    Ok(line(question)?.is_some_and(|answer| answer == "yes"))
}

fn cancel() -> Result<i32, Failure> {
    say!("Cancelled. No agent changes were applied.");
    Ok(0)
}

fn compare(cwd: &Path, sources: &[Candidate]) -> Result<(), Failure> {
    let refs: Vec<_> = sources.iter().map(reference).collect();
    let result = workflow::compare(cwd, &refs)?;
    for source in entries(&result["sources"]) {
        say!(
            "\n--- {} {} ---",
            clean(text(source, "harness")),
            clean(text(source, "path"))
        );
        say!("{}", metadata(&candidate(&source["metadata"])?));
        if let Some(settings) = source["metadata"]["settings"].as_object() {
            for (key, value) in settings {
                say!("  {}: {}", clean(key), clean(&value.to_string()));
            }
        }
        for row in text(source, "body").lines() {
            say!("{}", clean(row));
        }
    }
    io::stdout().flush()?;
    Ok(())
}

fn choose_source(cwd: &Path, sources: &[Candidate]) -> Result<Option<Candidate>, Failure> {
    if sources.is_empty() {
        return Ok(None);
    }
    if sources.len() == 1 {
        return Ok(Some(sources[0].clone()));
    }
    say!("Choose the native version to preserve. There is no historical merge base.");
    for (index, source) in sources.iter().enumerate() {
        say!("{}. {}", index + 1, metadata(source));
    }
    loop {
        let Some(answer) = line("Source number; d shows current versions; q cancels:")? else {
            return Ok(None);
        };
        if answer == "d" {
            compare(cwd, sources)?;
            continue;
        }
        if let Some(source) = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| sources.get(n))
        {
            return Ok(Some(source.clone()));
        }
        say!("Choose a source number.");
    }
}

fn choice(question: &str, options: &[Value], default: &str) -> Result<Option<String>, Failure> {
    let mut options: Vec<String> = options
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if options.is_empty() {
        return Ok(
            line(&format!("{question} [{default}] (q cancels):"))?.map(|answer| {
                if answer.is_empty() {
                    default.into()
                } else {
                    answer
                }
            }),
        );
    }
    let selected = if let Some(index) = options.iter().position(|option| option == default) {
        index
    } else if question == "Model" {
        options.push(default.to_owned());
        options.len() - 1
    } else {
        options
            .iter()
            .position(|option| option == "inherit")
            .unwrap_or(0)
    };
    single(question, &options, selected, question == "Model")
}

fn effort_options(harness: &Value, model: &str) -> (Vec<Value>, bool) {
    if let Some(entry) = entries(&harness["model_catalog"]["models"])
        .iter()
        .find(|entry| text(entry, "id") == model)
    {
        let efforts: Vec<_> = entries(&entry["efforts"])
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if !efforts.is_empty() {
            let mut options = vec![Value::String("inherit".into())];
            for effort in efforts {
                let option = Value::String(effort.into());
                if !options.contains(&option) {
                    options.push(option);
                }
            }
            return (options, true);
        }
    }
    (entries(&harness["efforts"]).to_vec(), false)
}

fn show_catalog(harness: &Value) -> Result<(), Failure> {
    say!("{}", style::dim(Out, &catalog_provenance(harness)));
    let catalog = &harness["model_catalog"];
    if !text(catalog, "note").is_empty() {
        say!("{}", style::warn(Out, &clean(text(catalog, "note"))));
    }
    say!("Models are local identifiers; account access is not verified.");
    Ok(())
}

fn catalog_provenance(harness: &Value) -> String {
    let catalog = &harness["model_catalog"];
    let source = catalog["source"].as_str().unwrap_or("unavailable");
    let status = catalog["status"].as_str().unwrap_or("unavailable");
    let mut line = format!("Model catalog: {} | {}", clean(source), clean(status));
    if !text(catalog, "fetched_at").is_empty() {
        line.push_str(&format!(
            " | fetched at {}",
            clean(text(catalog, "fetched_at"))
        ));
    }
    line
}

fn show_original(source: &Candidate) -> Result<(), Failure> {
    say!(
        "{}",
        style::bold(
            Out,
            &format!(
                "Original model: {} | effort {}",
                style::path(Out, &clean(source.model.as_deref().unwrap_or("inherit"))),
                style::path(Out, &clean(source.effort.as_deref().unwrap_or("inherit")))
            )
        )
    );
    Ok(())
}

fn target_sources(inventory: &Value) -> Result<Vec<Candidate>, Failure> {
    let mut sources = Vec::new();
    for agent in entries(&inventory["agents"]) {
        for source in entries(&agent["sources"]) {
            sources.push(candidate(source)?);
        }
    }
    for source in entries(&inventory["unmanaged"]) {
        sources.push(candidate(source)?);
    }
    Ok(sources)
}

fn settings(
    harness: &Value,
    source: &Candidate,
    previous: Option<&Value>,
) -> Result<Option<BTreeMap<String, Value>>, Failure> {
    let capabilities: Vec<_> = entries(&harness["capabilities"])
        .iter()
        .filter_map(Value::as_str)
        .filter(|key| *key != "model" && *key != "effort")
        .collect();
    let mut settings: BTreeMap<String, Value> = previous
        .and_then(|a| serde_json::from_value(a["settings"].clone()).ok())
        .unwrap_or_else(|| {
            source
                .settings
                .iter()
                .filter(|(key, _)| capabilities.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        });
    for (key, value) in &source.settings {
        say!(
            "Original setting {}: {}",
            clean(key),
            clean(&value.to_string())
        );
    }
    let unsupported: Vec<_> = source
        .settings
        .keys()
        .filter(|key| !capabilities.contains(&key.as_str()))
        .collect();
    if !unsupported.is_empty() {
        say!("{} cannot represent these source settings: {}. They remain at the source; destination settings have their own meaning.",
            clean(text(harness, "harness")), clean(&unsupported.iter().map(|key| key.as_str()).collect::<Vec<_>>().join(", ")));
        if !confirm("Continue with that explicit difference? Type yes:")? {
            return Ok(None);
        }
    }
    for (key, value) in &settings {
        say!(
            "Destination setting {}: {}",
            clean(key),
            clean(&value.to_string())
        );
    }
    let Some(edit) = line("Edit destination execution settings? [no] (yes; q cancels):")? else {
        return Ok(None);
    };
    if edit != "yes" {
        return Ok(Some(settings));
    }
    let labels: Vec<_> = capabilities.iter().map(|key| (*key).to_owned()).collect();
    let defaults: Vec<_> = capabilities
        .iter()
        .map(|key| settings.contains_key(*key))
        .collect();
    let Some(selected) = select(
        "Destination settings (unchecked settings use harness defaults)",
        &labels,
        &defaults,
    )?
    else {
        return Ok(None);
    };
    for (key, selected) in capabilities.into_iter().zip(selected) {
        if !selected {
            settings.remove(key);
            continue;
        }
        let presets: &[&str] = match key {
            "sandbox_mode" => &["read-only", "workspace-write", "danger-full-access"],
            "permissionMode" => &[
                "default",
                "acceptEdits",
                "auto",
                "dontAsk",
                "bypassPermissions",
                "plan",
                "manual",
            ],
            "background" | "omitClaudeMd" => &["false", "true"],
            "isolation" => &["worktree"],
            _ => &[],
        };
        let options: Vec<Value> = presets.iter().map(|s| Value::String((*s).into())).collect();
        let current = settings
            .get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_else(|| presets.first().copied().unwrap_or("").into());
        let Some(answer) = choice(key, &options, &current)? else {
            return Ok(None);
        };
        let value = match key {
            "background" | "omitClaudeMd" => Value::Bool(
                answer
                    .parse::<bool>()
                    .map_err(|_| Failure::usage("Choose true or false."))?,
            ),
            "maxTurns" => Value::from(
                answer
                    .parse::<u64>()
                    .map_err(|_| Failure::usage("Choose a positive turn limit."))?,
            ),
            _ => Value::String(answer),
        };
        settings.insert(key.into(), value);
    }
    Ok(Some(settings))
}

pub(super) fn sync(cwd: &Path, _args: &Args) -> Result<i32, Failure> {
    let inventory = workflow::inventory(cwd)?;
    let located =
        crate::store::locate(cwd)?.ok_or_else(|| Failure::usage("No vivac tree found."))?;
    let project = crate::render::project_name(&located.root);
    let project = if project == "-" {
        located
            .lane_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    } else {
        project
    };
    let harnesses: Vec<_> = entries(&inventory["harnesses"])
        .iter()
        .filter(|h| h["configured"] == true)
        .collect();
    let mut pending: Vec<(Option<String>, String, Vec<Candidate>, Value)> = Vec::new();
    for agent in entries(&inventory["agents"]) {
        if agent["retired"] == true {
            continue;
        }
        let mappings = entries(&agent["mappings"]);
        let needs_work = mappings.iter().any(|m| text(m, "configured") != "current")
            || harnesses.iter().any(|h| {
                !mappings
                    .iter()
                    .any(|m| text(m, "harness") == text(h, "harness"))
            });
        if needs_work {
            let sources = entries(&agent["sources"])
                .iter()
                .map(candidate)
                .collect::<Result<Vec<_>, _>>()?;
            pending.push((
                Some(text(agent, "agent").into()),
                text(agent, "name").into(),
                sources,
                agent["definition"].clone(),
            ));
        }
    }
    for source in entries(&inventory["unmanaged"]) {
        if source["detached"] == true {
            continue;
        }
        let native = candidate(source)?;
        if native.problem.is_some() || !native.unsupported.is_empty() {
            continue;
        }
        pending.push((
            None,
            native.name.clone().unwrap_or_default(),
            vec![native],
            Value::Null,
        ));
    }
    if pending.is_empty() {
        say!("No agent configuration differences to synchronize.");
        return Ok(0);
    }
    let labels: Vec<_> = pending
        .iter()
        .map(|(id, name, _, _)| {
            clean(&format!(
                "{name}: {}",
                if id.is_some() {
                    "configuration differs or target is missing"
                } else {
                    "not managed"
                }
            ))
        })
        .collect();
    let Some(selected) = select(
        "Agents needing attention",
        &labels,
        &vec![true; labels.len()],
    )?
    else {
        return cancel();
    };
    let total = selected.iter().filter(|chosen| **chosen).count();
    let targets = target_sources(&inventory)?;
    let mut items = Vec::new();
    let mut current = 0;
    for ((agent, name, sources, definition), chosen) in pending.into_iter().zip(selected) {
        if !chosen {
            continue;
        }
        current += 1;
        let context = Context {
            project: project.clone(),
            name: name.clone(),
            index: current,
            total,
        };
        context.show("native versions", "choose destinations", 1)?;
        let Some(source) = choose_source(cwd, &sources)? else {
            return cancel();
        };
        say!(
            "Source: {} {}",
            clean(&source.harness),
            style::path(Out, &clean(&source.path))
        );
        say!("The source harness remains configured with its original assignment.");
        let destinations_available: Vec<_> = harnesses
            .iter()
            .copied()
            .filter(|h| text(h, "harness") != source.harness)
            .collect();
        let labels: Vec<_> = destinations_available
            .iter()
            .map(|h| clean(text(h, "harness")))
            .collect();
        let defaults = vec![true; labels.len()];
        let Some(selected) = select("Configure this agent in", &labels, &defaults)? else {
            return cancel();
        };
        let selected_destinations: Vec<_> = destinations_available
            .iter()
            .zip(&selected)
            .filter(|(_, chosen)| **chosen)
            .map(|(harness, _)| text(harness, "harness"))
            .collect();
        context.show(&source.harness, &selected_destinations.join(", "), 2)?;
        let mut destinations = Vec::new();
        for (harness, selected) in destinations_available.iter().zip(selected) {
            if !selected {
                continue;
            }
            let harness_name = text(harness, "harness");
            let previous = entries(&definition["assignments"])
                .iter()
                .find(|a| text(a, "harness") == harness_name);
            if selected_destinations.len() > 1 {
                say!(
                    "\n{}",
                    style::bold(
                        Out,
                        &style::path(Out, &context.direction(&source.harness, harness_name))
                    )
                );
            }
            show_original(&source)?;
            let fresh_harness = workflow::harness_inventory(cwd, harness_name)?;
            show_catalog(&fresh_harness)?;
            let Some(model) = choice(
                "Model",
                entries(&fresh_harness["models"]),
                previous.map(|a| text(a, "model")).unwrap_or("inherit"),
            )?
            else {
                return cancel();
            };
            let (efforts, known) = effort_options(&fresh_harness, &model);
            if known {
                say!(
                    "{}",
                    style::dim(
                        Out,
                        &format!(
                            "Effort choices declared by the catalog for {}.",
                            clean(&model)
                        )
                    )
                );
            } else {
                say!("{}", style::warn(Out, &format!("Effort capabilities unknown for {}; showing harness choices, not verified model support.", clean(&model))));
            }
            let previous_effort = previous.map(|a| text(a, "effort")).unwrap_or("inherit");
            if !efforts
                .iter()
                .any(|effort| effort.as_str() == Some(previous_effort))
            {
                say!("{}", style::warn(Out, &format!("Previous effort {} is not among these choices; choose an effort explicitly.", clean(previous_effort))));
            }
            let Some(effort) = choice("Effort", &efforts, previous_effort)? else {
                return cancel();
            };
            say!(
                "{}",
                style::bold(
                    Out,
                    &format!(
                        "Assignment: model {} | effort {}",
                        style::path(Out, &clean(&model)),
                        style::path(Out, &clean(&effort))
                    )
                )
            );
            let Some(settings) = settings(harness, &source, previous)? else {
                return cancel();
            };
            let default_name = previous.map(|a| text(a, "name")).unwrap_or(&name);
            let Some(answer) = line(&format!(
                "Destination agent name [{}] (q cancels):",
                clean(default_name)
            ))?
            else {
                return cancel();
            };
            let destination_name = if answer.is_empty() {
                default_name.to_owned()
            } else {
                answer
            };
            let path = if destination_name == default_name {
                sources
                    .iter()
                    .find(|s| s.harness == harness_name)
                    .map(|s| s.path.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "{}/{}.{}",
                            text(harness, "directory"),
                            destination_name,
                            text(harness, "extension")
                        )
                    })
            } else {
                format!(
                    "{}/{}.{}",
                    text(harness, "directory"),
                    destination_name,
                    text(harness, "extension")
                )
            };
            let existing = targets
                .iter()
                .find(|t| t.harness == harness_name && t.path == path);
            if let Some(existing) = existing {
                say!(
                    "\n{}",
                    style::warn(
                        Out,
                        &format!(
                            "Existing destination: {} | {}",
                            context.direction(&source.harness, harness_name),
                            clean(&path)
                        )
                    )
                );
                say!("Existing target: {}", metadata(existing));
                compare(cwd, &[source.clone(), existing.clone()])?;
                if !confirm(&format!(
                    "Replace {} at {} in {} with {}'s reviewed source? Type yes:",
                    clean(&destination_name),
                    clean(&path),
                    clean(harness_name),
                    clean(&name)
                ))? {
                    return cancel();
                }
            }
            destinations.push(SyncDestination {
                assignment: Assignment {
                    harness: harness_name.into(),
                    name: destination_name,
                    model,
                    effort,
                    settings,
                },
                path,
                digest: existing.map(|t| t.digest.clone()),
            });
        }
        items.push(SyncItem {
            agent,
            source: reference(&source),
            destinations,
        });
    }
    if items.is_empty() {
        return cancel();
    }
    say!("\n{}", style::bold(Out, "Review all selected agents"));
    let Some(why) = line("Reason [Synchronize reviewed native agent configurations] (q cancels):")?
    else {
        return cancel();
    };
    let selection = SyncSelection {
        why: if why.is_empty() {
            "Synchronize reviewed native agent configurations".into()
        } else {
            why
        },
        items,
    };
    let plan = workflow::plan(cwd, &selection)?;
    say!("\n{}", style::bold(Out, "Reviewed plan"));
    say!("Reason: {}", clean(&selection.why));
    for (index, item) in selection.items.iter().enumerate() {
        let name = targets
            .iter()
            .find(|source| source.harness == item.source.harness && source.path == item.source.path)
            .and_then(|source| source.name.clone())
            .unwrap_or_else(|| item.agent.clone().unwrap_or_else(|| "agent".into()));
        let context = Context {
            project: project.clone(),
            name,
            index: index + 1,
            total: selection.items.len(),
        };
        let destinations = item
            .destinations
            .iter()
            .map(|destination| destination.assignment.harness.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        context.show(&item.source.harness, &destinations, 3)?;
        say!(
            "  Preserve {} {} ({})",
            clean(&item.source.harness),
            style::path(Out, &clean(&item.source.path)),
            item.agent
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "new managed identity".into())
        );
        for destination in &item.destinations {
            say!(
                "    {} {}: model {}, effort {} [{}]",
                clean(&destination.assignment.harness),
                style::path(Out, &clean(&destination.path)),
                clean(&destination.assignment.model),
                clean(&destination.assignment.effort),
                if destination.digest.is_some() {
                    "replace reviewed current file"
                } else {
                    "create only if absent"
                }
            );
            say!(
                "      {}",
                if destination.digest.is_some() {
                    style::change(Out, "Replace reviewed current file")
                } else {
                    style::good(Out, "Create only if absent")
                }
            );
            for (key, value) in &destination.assignment.settings {
                say!("      {}: {}", clean(key), clean(&value.to_string()));
            }
        }
    }
    say!("Source unchanged. The complete prompt is transferred to each reviewed destination.");
    say!("{}", style::dim(Out, "Runtime use remains unverified."));
    if !confirm("Apply this entire reviewed plan? Type yes:")? {
        return cancel();
    }
    let (result, code) = workflow::apply(cwd, &selection, text(&plan, "plan_digest"))?;
    if code == 0 && result["applied"] == true {
        say!(
            "Applied {} agent configurations.",
            entries(&result["imported"]).len()
        );
        for item in &selection.items {
            say!(
                "  Source preserved: {} {}",
                clean(&item.source.harness),
                clean(&item.source.path)
            );
            for destination in &item.destinations {
                say!(
                    "  Configured: {} {} | model {} | effort {}",
                    clean(&destination.assignment.harness),
                    clean(&destination.path),
                    clean(&destination.assignment.model),
                    clean(&destination.assignment.effort)
                );
            }
        }
        say!("Configuration applied; runtime use remains unverified.");
    } else {
        say!(
            "{}",
            clean(
                result["error"]
                    .as_str()
                    .unwrap_or("The agent synchronization was not completed.")
            )
        );
        if result["rollback_failed"] == true {
            say!("Restoring native files failed for at least one destination. Inspect destinations before retrying.");
        } else if result["rollback_attempted"] == true {
            say!("Native file restoration was attempted. Inspect destinations before retrying.");
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_provenance_exposes_cache_time_without_terminal_controls() {
        let harness = serde_json::json!({"model_catalog": {
            "source": "local cache", "status": "available", "fetched_at": "2026-10-08T11:04:00Z\n"
        }});
        assert_eq!(
            catalog_provenance(&harness),
            "Model catalog: local cache | available | fetched at 2026-10-08T11:04:00Z "
        );
        assert_eq!(
            catalog_provenance(&serde_json::json!({})),
            "Model catalog: unavailable | unavailable"
        );
    }
    #[test]
    fn effort_choices_follow_selected_model_without_claiming_unknown_capabilities() {
        let harness = serde_json::json!({
            "efforts": ["inherit", "low", "high", "max"],
            "model_catalog": {"source": "local cache", "status": "available", "models": [
                {"id": "review-model", "label": "Review", "efforts": ["low", "high"]},
                {"id": "unknown-model", "efforts": []}
            ]}
        });
        let (options, known) = effort_options(&harness, "review-model");
        assert!(known);
        assert_eq!(
            options,
            vec![
                Value::from("inherit"),
                Value::from("low"),
                Value::from("high")
            ]
        );
        let (options, known) = effort_options(&harness, "unknown-model");
        assert!(!known);
        assert_eq!(options, harness["efforts"].as_array().unwrap().clone());
        assert!(!effort_options(&harness, "custom-model").1);
    }
    #[test]
    fn assignment_prompts_keep_context_and_final_cancellation_preserves_native_files() {
        sync_trial(&["reviewer"], "agents::human::tests::assignment_prompts_keep_context_and_final_cancellation_preserves_native_files");
    }

    #[test]
    fn each_agent_gets_one_step_heading_and_its_own_assignment_summary() {
        sync_trial(
            &["reviewer", "scout"],
            "agents::human::tests::each_agent_gets_one_step_heading_and_its_own_assignment_summary",
        );
    }

    fn sync_trial(names: &[&str], test_name: &str) {
        use std::fs;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        const MARKER: &str = "VIVAC_TEST_HUMAN_SYNC_ROOT";
        if let Some(root) = std::env::var_os(MARKER) {
            let root = std::path::PathBuf::from(root);
            let yes = Args::parse(["--yes".into()]).unwrap();
            assert_eq!(crate::setup::init(&root, &yes).unwrap(), 0);
            for harness in ["claude-code", "codex"] {
                let args = Args::parse([harness.into(), "--yes".into()]).unwrap();
                assert_eq!(crate::setup::dispatch(&root, &args).unwrap(), 0);
            }
            fs::create_dir_all(root.join(".claude/agents")).unwrap();
            let source = |name| {
                format!("---\nname: {name}\ndescription: Review changes.\nmodel: sonnet\n---\nReview the change.\n")
            };
            for name in names {
                fs::write(root.join(format!(".claude/agents/{name}.md")), source(name)).unwrap();
            }
            let before = fs::read(root.join(".vivac/events")).unwrap();
            assert_eq!(sync(&root, &Args::default()).unwrap(), 0);
            assert_eq!(before, fs::read(root.join(".vivac/events")).unwrap());
            for name in names {
                assert_eq!(
                    source(name),
                    fs::read_to_string(root.join(format!(".claude/agents/{name}.md"))).unwrap()
                );
                assert!(!root.join(format!(".codex/agents/{name}.toml")).exists());
            }
            return;
        }
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let scratch =
            Scratch(std::env::temp_dir().join(format!("vivac-human-sync-{}", crate::id::ulid())));
        fs::create_dir_all(&scratch.0).unwrap();
        let output_file = scratch.0.join("output.txt");
        let error_file = scratch.0.join("error.txt");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(MARKER, &scratch.0)
            .env("VIVAC_HOME", scratch.0.join("home"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(fs::File::create(&output_file).unwrap())
            .stderr(fs::File::create(&error_file).unwrap())
            .spawn()
            .unwrap();
        let script = format!(
            "\n{}\nno\n",
            "\ncustom-model\nhigh\n\n\n".repeat(names.len())
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Human sync test exceeded its deadline.");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let output = fs::read_to_string(output_file).unwrap();
        assert!(
            status.success(),
            "{output}\n{}",
            fs::read_to_string(error_file).unwrap()
        );
        assert_eq!(
            output.matches("STEP 1/3: Source & destinations").count(),
            names.len(),
            "{output}"
        );
        assert_eq!(
            output.matches("STEP 2/3: Assignments").count(),
            names.len(),
            "{output}"
        );
        assert_eq!(
            output.matches("STEP 3/3: Reviewed changes").count(),
            names.len(),
            "{output}"
        );
        for (index, name) in names.iter().enumerate() {
            assert!(
                output.contains(&format!(
                    "Agent {}/{}: {name} | claude-code -> codex | STEP 2/3: Assignments",
                    index + 1,
                    names.len()
                )),
                "{output}"
            );
            assert!(
                output.contains(&format!(
                    ".codex/agents/{name}.toml: model custom-model, effort high"
                )),
                "{output}"
            );
        }
        assert_eq!(
            output
                .matches("Original model: sonnet | effort inherit")
                .count(),
            names.len(),
            "{output}"
        );
        assert_eq!(
            output
                .matches("Assignment: model custom-model | effort high")
                .count(),
            names.len(),
            "{output}"
        );
        assert!(
            output.contains("Cancelled. No agent changes were applied."),
            "{output}"
        );
        assert!(!output.contains('\u{1b}'), "{output}");
    }
    #[test]
    fn assignment_heading_keeps_agent_direction_and_step_in_plain_text() {
        let context = Context {
            project: "ridge".into(),
            name: "ridge-gate".into(),
            index: 2,
            total: 4,
        };
        assert_eq!(
            context.heading("claude-code", "codex", 2),
            "ridge | Agent 2/4: ridge-gate | claude-code -> codex | STEP 2/3: Assignments"
        );
    }
    #[test]
    fn original_metadata_cannot_inject_terminal_controls() {
        let source = Candidate {
            harness: "codex".into(),
            path: "agent\x1b[2J.toml".into(),
            digest: String::new(),
            name: None,
            model: Some("model\nname".into()),
            effort: None,
            settings: BTreeMap::new(),
            unsupported: vec![],
            problem: None,
        };
        assert!(!metadata(&source).contains('\x1b'));
        assert!(!metadata(&source).contains('\n'));
        assert!(metadata(&source).contains("model name"));
    }
}
