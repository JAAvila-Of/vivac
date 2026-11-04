use super::{json, refused, schema, Location, Result, Role, Selector};
use crate::event::{Body, Event, Kind};
use crate::model::Tree;
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;

const MAX_LINE: usize = 1024 * 1024;
const REMOVED: &str = "[Removed after a sensitive-data incident]";

fn event(bytes: &[u8]) -> Result<(serde_json::Value, Event)> {
    let value = json::decode(bytes)?;
    schema::event(&value)?;
    let event = serde_json::from_value(value.clone()).map_err(|_| refused())?;
    Ok((value, event))
}

pub(super) fn read(
    reader: &mut impl BufRead,
    selectors: &[Selector],
) -> Result<(String, Tree, Vec<Location>)> {
    if selectors.is_empty() {
        return Err(refused());
    }
    let selected: BTreeSet<_> = selectors.iter().cloned().collect();
    if selected.len() != selectors.len() {
        return Err(refused());
    }
    let mut found = BTreeSet::new();
    let mut nodes = BTreeMap::<String, (u64, Kind)>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    let mut ids = BTreeSet::new();
    let mut previous = 0;
    let mut total = 0u64;
    let mut identity = None;
    let mut tree = Tree::default();
    let mut locations = Vec::new();
    loop {
        let mut line = Vec::new();
        loop {
            let available = reader.fill_buf().map_err(|_| refused())?;
            if available.is_empty() {
                break;
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            total = total.checked_add(count as u64).ok_or_else(refused)?;
            if total > crate::backup::MAX_FILE {
                return Err(refused());
            }
            if line.len() + count > MAX_LINE {
                return Err(refused());
            }
            line.extend_from_slice(&available[..count]);
            reader.consume(count);
            if line.last() == Some(&b'\n') {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        if line.last() != Some(&b'\n') {
            return Err(refused());
        }
        let (mut value, original) = event(&line)?;
        if original.seq <= previous || !ids.insert(original.id.clone()) {
            return Err(refused());
        }
        previous = original.seq;
        identity.get_or_insert_with(|| original.id.clone());
        crate::backup::lane_id(&original.lane).map_err(|_| refused())?;
        let mut candidates = Vec::new();
        match &original.payload {
            Body::NodeCreated {
                node, num, kind, ..
            } => {
                if nodes.insert(node.clone(), (*num, *kind)).is_some()
                    || aliases
                        .insert(format!("{}{num}", kind.prefix()), node.clone())
                        .is_some()
                {
                    return Err(refused());
                }
                candidates.push((node.clone(), Role::Title, "/payload/title".to_owned()));
                if value["payload"].get("why").is_some() {
                    candidates.push((node.clone(), Role::Why, "/payload/why".to_owned()));
                }
            }
            Body::Promoted { node } => {
                let (num, kind) = nodes.get_mut(node).ok_or_else(refused)?;
                aliases.remove(&format!("{}{num}", kind.prefix()));
                *kind = Kind::Goal;
                if aliases.insert(format!("g{num}"), node.clone()).is_some() {
                    return Err(refused());
                }
            }
            Body::NodeNoted { node, .. } => {
                candidates.push((node.clone(), Role::Notes, "/payload/note".to_owned()))
            }
            Body::StateChanged { node, .. } if value["payload"].get("outcome").is_some() => {
                candidates.push((node.clone(), Role::Outcome, "/payload/outcome".to_owned()))
            }
            Body::VivacCreated { stack, .. } => {
                for (index, (alias, _)) in stack.iter().enumerate() {
                    let node = aliases.get(alias).ok_or_else(refused)?;
                    candidates.push((
                        node.clone(),
                        Role::Title,
                        format!("/payload/stack/{index}/1"),
                    ));
                }
            }
            _ => {}
        }
        for (node, role, pointer) in candidates {
            let selector = Selector {
                node: node.clone(),
                role,
            };
            if selected.contains(&selector) {
                if !nodes.contains_key(&node) {
                    return Err(refused());
                }
                let field = value.pointer_mut(&pointer).ok_or_else(refused)?;
                if !field.is_string() {
                    return Err(refused());
                }
                *field = REMOVED.into();
                found.insert(selector);
                locations.push(Location {
                    node,
                    role,
                    seq: original.seq,
                    pointer,
                });
            }
        }
        let projection = serde_json::to_vec(&value).map_err(|_| refused())?;
        crate::backup::privacy::check_json(projection.as_slice()).map_err(|_| refused())?;
        let validated: Event = serde_json::from_value(value).map_err(|_| refused())?;
        tree.apply(
            validated.seq,
            &validated.ts,
            &validated.lane,
            &validated.payload,
        );
    }
    if found != selected || tree.has_pending() || !tree.repeated_nums.is_empty() {
        return Err(refused());
    }
    crate::backup::validate_lineage(&tree).map_err(|_| refused())?;
    locations.sort();
    Ok((identity.ok_or_else(refused)?, tree, locations))
}

#[test]
fn unknown_event_fields_refuse_instead_of_disappearing() {
    let line = br#"{"seq":1,"id":"fixture","ts":"now","actor":"opaque","lane":"main","extra":"ordinary text","payload":{"type":"lane.claimed","lane":"main"}}"#;
    assert!(event(line).is_err());
}

#[test]
fn assignment_settings_are_open_maps_but_all_keys_and_values_are_inspected() {
    let definition = serde_json::json!({"schema_version":1,"name":"reviewer","contract":{"purpose":"Review","duties":[],"limits":[],"acceptance":[]},"assignments":[{"harness":"codex","name":"reviewer","model":"fixture","effort":"medium","settings":{"custom":{"nested":[1.5,true,"Safe value"]}}}]});
    let raw = serde_json::json!({"seq":1,"id":"event","ts":"now","actor":"opaque","lane":"main","payload":{"type":"agent.recorded","agent":"agent","node":"decision","definition":definition}});
    let (value, _) = event(&serde_json::to_vec(&raw).unwrap()).unwrap();
    crate::backup::privacy::check_json(serde_json::to_vec(&value).unwrap().as_slice()).unwrap();
    for key in [false, true] {
        let mut value = value.clone();
        let secret = format!("ghp_{}", "k".repeat(32));
        value["payload"]["definition"]["assignments"][0]["settings"] = if key {
            serde_json::json!({secret:"ordinary"})
        } else {
            serde_json::json!({"custom":secret})
        };
        assert!(event(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(
            crate::backup::privacy::check_json(serde_json::to_vec(&value).unwrap().as_slice())
                .is_err()
        );
    }
}
