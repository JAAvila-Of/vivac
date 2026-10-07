//! Human inventory and a short, reviewed synchronization flow.

use super::terminal::{clean, line, select};
use super::types::{Assignment, Candidate};
use super::workflow::{self, NativeRef, SyncDestination, SyncItem, SyncSelection};
use crate::args::Args;
use crate::failure::Failure;
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

pub(super) fn run(cwd: &Path, _args: &Args) -> Result<i32, Failure> {
    render_inventory(&workflow::inventory(cwd)?)?;
    Ok(0)
}

fn render_inventory(inventory: &Value) -> Result<(), Failure> {
    say!("Agents (configuration status; runtime use is not verified)");
    for agent in entries(&inventory["agents"]) {
        say!(
            "\n{} [{}] managed by vivac{}",
            clean(text(agent, "name")),
            clean(text(agent, "agent")),
            if agent["retired"] == true {
                " (retired)"
            } else {
                ""
            }
        );
        for mapping in entries(&agent["mappings"]) {
            let state = text(mapping, "configured");
            let row = clean(&format!(
                "  {}: {} | {}",
                text(mapping, "harness"),
                mapping["path"].as_str().unwrap_or("not bound"),
                state
            ));
            if state != "current"
                && state != "retired"
                && crate::style::enabled(crate::style::Stream::Out)
            {
                say!("\x1b[31m{row}\x1b[0m");
            } else {
                say!("{row}");
            }
        }
        for source in entries(&agent["sources"]) {
            say!("  {}", metadata(&candidate(source)?));
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
                    "  {}: not configured for this agent",
                    clean(text(harness, "harness"))
                );
            }
        }
    }
    for source in entries(&inventory["unmanaged"]) {
        let native = candidate(source)?;
        say!(
            "\n{} | not managed{}",
            clean(native.name.as_deref().unwrap_or("unnamed")),
            if source["detached"] == true {
                " (detached; excluded from sync)"
            } else {
                ""
            }
        );
        say!("  {}", metadata(&native));
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
    let options: Vec<_> = options.iter().filter_map(Value::as_str).collect();
    say!("Available choices:");
    for (index, option) in options.iter().enumerate() {
        say!("  {}. {}", index + 1, clean(option));
    }
    loop {
        let Some(answer) = line(&format!(
            "{question} [{default}] (identifier or number; q cancels):"
        ))?
        else {
            return Ok(None);
        };
        let chosen = if answer.is_empty() {
            default.to_owned()
        } else if let Some(option) = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| options.get(n))
        {
            (*option).to_owned()
        } else {
            answer
        };
        if !chosen.is_empty() && !chosen.chars().any(char::is_control) {
            return Ok(Some(chosen));
        }
    }
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
    let targets = target_sources(&inventory)?;
    let mut items = Vec::new();
    for ((agent, name, sources, definition), chosen) in pending.into_iter().zip(selected) {
        if !chosen {
            continue;
        }
        say!("\nAgent: {}", clean(&name));
        let Some(source) = choose_source(cwd, &sources)? else {
            return cancel();
        };
        say!("Original: {}", metadata(&source));
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
        let mut destinations = Vec::new();
        for (harness, selected) in destinations_available.iter().zip(selected) {
            if !selected {
                continue;
            }
            let harness_name = text(harness, "harness");
            let previous = entries(&definition["assignments"])
                .iter()
                .find(|a| text(a, "harness") == harness_name);
            say!(
                "{} destination; original model {}, effort {}",
                clean(harness_name),
                clean(source.model.as_deref().unwrap_or("inherit")),
                clean(source.effort.as_deref().unwrap_or("inherit"))
            );
            say!("Models are local identifiers; account access is not verified.");
            let Some(model) = choice(
                "Model",
                entries(&harness["models"]),
                previous.map(|a| text(a, "model")).unwrap_or("inherit"),
            )?
            else {
                return cancel();
            };
            let Some(effort) = choice(
                "Effort",
                entries(&harness["efforts"]),
                previous.map(|a| text(a, "effort")).unwrap_or("inherit"),
            )?
            else {
                return cancel();
            };
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
                say!("Existing target: {}", metadata(existing));
                compare(cwd, &[source.clone(), existing.clone()])?;
                if !confirm(
                    "Replace this exact current target with the reviewed source? Type yes:",
                )? {
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
    say!("\nReviewed plan:");
    say!("Reason: {}", clean(&selection.why));
    for item in &selection.items {
        say!(
            "  Preserve {} {} ({})",
            clean(&item.source.harness),
            clean(&item.source.path),
            item.agent
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "new managed identity".into())
        );
        for destination in &item.destinations {
            say!(
                "    {} {}: model {}, effort {} [{}]",
                clean(&destination.assignment.harness),
                clean(&destination.path),
                clean(&destination.assignment.model),
                clean(&destination.assignment.effort),
                if destination.digest.is_some() {
                    "replace reviewed current file"
                } else {
                    "create only if absent"
                }
            );
            for (key, value) in &destination.assignment.settings {
                say!("      {}: {}", clean(key), clean(&value.to_string()));
            }
        }
    }
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
