use super::{refused, Result};
use serde_json::Value;

fn closed(value: &Value, fields: &str) -> Result<()> {
    let object = value.as_object().ok_or_else(refused)?;
    if object
        .keys()
        .any(|key| !fields.split_whitespace().any(|field| field == key))
    {
        return Err(refused());
    }
    Ok(())
}

pub(super) fn event(value: &Value) -> Result<()> {
    closed(value, "seq id ts actor lane payload")?;
    let body = &value["payload"];
    let fields = match body["type"].as_str().ok_or_else(refused)? {
        "node.created" => "type node num kind title why parent blocks refs governs arms against",
        "state.changed" => "type node state outcome forced until",
        "node.noted" => "type node note",
        "edge.blocks" => "type node blocks",
        "stack.pushed" => "type node",
        "stack.popped" => "type node",
        "stack.promoted" => "type node",
        "flag.raised" => "type node flag reason on",
        "flag.cleared" => "type node flag",
        "arm.added" => "type node dir command",
        "arm.removed" => "type node dir command",
        "against.added" => "type node against",
        "vivac.created" => "type vivac num kind stack working_set next_intent anchor anchors node_ref label",
        "session.started" => "type source focus vivac session",
        "lane.declared" => "type lane name repos",
        "lane.claimed" => "type lane",
        "where.changed" => "type repos",
        "artifact.captured" => "type artifact revision operation producer previous source site size digest object",
        "artifact.bundle_captured" => "type artifact revision operation producer previous source selection site size digest object entrypoint member_count manifest_bytes",
        "artifact.captured_v2" => "type artifact revision operation producer previous source description site size digest object",
        "artifact.bundle_captured_v2" => "type artifact revision operation producer previous source description selection site size digest object entrypoint member_count manifest_bytes",
        "artifact.sqlite_application_captured" => "type artifact revision operation producer previous source description selection site size digest object entrypoint member_count manifest_bytes database profile runtime runtime_sha256",
        "artifact.captured_v3" => "type artifact revision operation producer previous source description site size digest object inspection",
        "artifact.bundle_captured_v3" => "type artifact revision operation producer previous source description selection site size digest object entrypoint member_count manifest_bytes inspection",
        "artifact.sqlite_application_captured_v2" => "type artifact revision operation producer previous source description selection site size digest object entrypoint member_count manifest_bytes database profile runtime runtime_sha256 inspection",
        "agent.automation.configured" => "type node enabled",
        "agent.recorded" => "type agent node definition",
        "agent.bound" => "type agent harness path baseline",
        "agent.detached" => "type agent harness path",
        "agent.materialized" => "type agent revision harness path digest adapter",
        "agent.observed" => "type agent revision harness path model effort evidence",
        _ => return Err(refused()),
    };
    closed(body, fields)?;
    for (field, fields) in [
        ("arms", "dir command"),
        ("against", "node why"),
        (
            "repos",
            if body["type"] == "where.changed" {
                "path branch sha rebasing missing withheld"
            } else {
                "path root"
            },
        ),
        ("anchors", "path branch sha"),
    ] {
        if let Some(values) = body.get(field).filter(|v| !v.is_null()) {
            for nested in values.as_array().ok_or_else(refused)? {
                closed(nested, fields)?;
            }
        }
    }
    if let Some(anchor) = body.get("anchor") {
        closed(anchor, "kind id")?;
    }
    if let Some(inspection) = body.get("inspection") {
        closed(inspection, "policy sensitivity coverage review_required")?;
    }
    if let Some(definition) = body.get("definition") {
        closed(
            definition,
            "schema_version name contract assignments prompt retired",
        )?;
        closed(&definition["contract"], "purpose duties limits acceptance")?;
        for assignment in definition["assignments"].as_array().ok_or_else(refused)? {
            closed(assignment, "harness name model effort settings")?;
            if let Some(settings) = assignment.get("settings") {
                settings.as_object().ok_or_else(refused)?;
                let bytes = serde_json::to_vec(settings).map_err(|_| refused())?;
                crate::backup::privacy::check_json(bytes.as_slice()).map_err(|_| refused())?;
            }
        }
        if let Some(prompt) = definition.get("prompt").filter(|value| !value.is_null()) {
            closed(prompt, "harness path digest")?;
        }
    }
    Ok(())
}

#[test]
fn raw_agent_definitions_reject_nested_extensions_before_event_decoding() {
    let base = serde_json::json!({"payload":{"type":"agent.recorded","agent":"agent","node":"decision","definition":{"schema_version":1,"name":"reviewer","contract":{"purpose":"Review","duties":[],"limits":[],"acceptance":[]},"assignments":[{"harness":"codex","name":"reviewer","model":"fixture","effort":"medium","settings":{"custom":{"nested":false}}}],"prompt":{"harness":"codex","path":".codex/agents/reviewer.toml","digest":"digest"},"retired":false}}});
    event(&base).unwrap();
    for pointer in [
        "/payload/definition",
        "/payload/definition/contract",
        "/payload/definition/assignments/0",
        "/payload/definition/prompt",
    ] {
        let mut value = base.clone();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), false.into());
        assert!(event(&value).is_err());
    }
}

#[test]
fn raw_inspections_reject_extensions_before_event_decoding() {
    let mut value = serde_json::json!({"payload":{"type":"artifact.captured_v3","inspection":{"policy":"allow_insecure","sensitivity":"detected","coverage":"complete","review_required":true}}});
    event(&value).unwrap();
    value["payload"]["inspection"]["extra"] = false.into();
    assert!(event(&value).is_err());
}
