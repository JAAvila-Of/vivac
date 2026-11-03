mod common;

use common::Sandbox;
use serde_json::{json, Value};

const SOURCE: &str = include_str!("data/incident/source.jsonl");
const SUCCESSOR: &str = include_str!("data/incident/successor.jsonl");
const CASE: &str = include_str!("data/incident/case.json");

fn records(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn mount(text: &str, project_id: &str, name: &str) -> Sandbox {
    let sandbox = Sandbox::new_seeded(name);
    let config_path = sandbox.0.join(".vivac/config");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["project_id"] = json!(project_id);
    std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::write(sandbox.0.join(".vivac/events"), text).unwrap();
    sandbox
}

fn read_json(sandbox: &Sandbox, args: &[&str]) -> Value {
    serde_json::from_str(&sandbox.ok(args)).unwrap()
}

#[test]
fn fixed_histories_are_readable_and_keep_the_same_graph() {
    let case: Value = serde_json::from_str(CASE).unwrap();
    let source = mount(
        SOURCE,
        case["source_project_id"].as_str().unwrap(),
        "incident-source",
    );
    let successor = mount(
        SUCCESSOR,
        case["successor_project_id"].as_str().unwrap(),
        "incident-successor",
    );
    for sandbox in [&source, &successor] {
        sandbox.ok(&["check"]);
    }
    let source_tree = read_json(&source, &["tree", "--json"]);
    let successor_tree = read_json(&successor, &["tree", "--json"]);
    for tree in [&source_tree, &successor_tree] {
        assert_eq!(tree.as_array().unwrap().len(), 1);
        assert_eq!(tree[0]["alias"], "g1");
        assert_eq!(tree[0]["state"], "active");
        assert_eq!(tree[0]["children"].as_array().unwrap().len(), 1);
        let child = &tree[0]["children"][0];
        assert_eq!(child["alias"], "g2");
        assert_eq!(child["kind"], "goal");
        assert_eq!(child["parent"], "g1");
        assert_eq!(child["state"], "abandoned");
        assert_eq!(child["id"], case["selected_node"]);
    }
    assert_eq!(source_tree[0]["id"], successor_tree[0]["id"]);
    assert_eq!(source_tree[0]["title"], successor_tree[0]["title"]);
    assert_eq!(
        source_tree[0]["children"][0]["why"],
        successor_tree[0]["children"][0]["why"]
    );
    let successor_node = read_json(&successor, &["why", "g2", "--json"]);
    assert_eq!(successor_node["node"]["title"], case["marker"]);
    assert_eq!(successor_node["node"]["outcome"], case["marker"]);
    let notes = successor_node["node"]["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|note| note["note"] == case["marker"]));
    assert_eq!(source.log(), SOURCE);
    assert_eq!(successor.log(), SUCCESSOR);
}

#[test]
fn fixed_successor_changes_only_selected_text_and_first_event_identity() {
    let case: Value = serde_json::from_str(CASE).unwrap();
    let source = records(SOURCE);
    let successor = records(SUCCESSOR);
    assert_eq!(source.len(), successor.len());
    assert_eq!(source.len(), 13);
    assert_ne!(source[0]["id"], successor[0]["id"]);
    assert_ne!(case["source_project_id"], case["successor_project_id"]);
    assert_eq!(
        case["selected_fields"],
        json!(["title", "notes", "outcome"])
    );
    let changes = case["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 7);
    for (index, (before, after)) in source.iter().zip(&successor).enumerate() {
        assert_eq!(before["seq"].as_u64(), Some(index as u64 + 1));
        let mut before_structure = before.clone();
        let mut after_structure = after.clone();
        if index == 0 {
            before_structure["id"] = Value::Null;
            after_structure["id"] = Value::Null;
        }
        for change in changes
            .iter()
            .filter(|change| change["seq"] == before["seq"])
        {
            let pointer = change["pointer"].as_str().unwrap();
            let old = before.pointer(pointer).and_then(Value::as_str).unwrap();
            assert_ne!(json!(old), case["marker"]);
            assert_eq!(after.pointer(pointer), Some(&case["marker"]));
            assert!(!SUCCESSOR.contains(old));
            *before_structure.pointer_mut(pointer).unwrap() = Value::Null;
            *after_structure.pointer_mut(pointer).unwrap() = Value::Null;
        }
        assert_eq!(before_structure, after_structure, "event {}", index + 1);
    }
    assert_eq!(source[8]["payload"]["stack"][1][0], "t2");
    assert_eq!(
        source[8]["payload"]["stack"][1][1],
        source[3]["payload"]["title"]
    );
    assert_eq!(successor[8]["payload"]["stack"][1][0], "t2");
    assert_eq!(source[3]["payload"]["kind"], "task");
    assert_eq!(source[3]["payload"]["node"], case["selected_node"]);
    assert_eq!(source[9]["payload"]["type"], "stack.promoted");
    assert_eq!(source[9]["payload"]["node"], case["selected_node"]);
    assert_eq!(source[10]["payload"]["stack"][0][0], "g2");
    assert_eq!(
        source[10]["payload"]["stack"][0][1],
        source[3]["payload"]["title"]
    );
    assert_eq!(successor[10]["payload"]["stack"][0][0], "g2");
    assert_eq!(successor[8]["payload"]["stack"][1][1], case["marker"]);
    assert_eq!(successor[10]["payload"]["stack"][0][1], case["marker"]);
}

#[test]
fn current_projection_does_not_account_for_every_historical_outcome() {
    let case: Value = serde_json::from_str(CASE).unwrap();
    let source = mount(
        SOURCE,
        case["source_project_id"].as_str().unwrap(),
        "incident-projection",
    );
    let history = records(SOURCE);
    let earlier = history[6]["payload"]["outcome"].as_str().unwrap();
    let latest = history[11]["payload"]["outcome"].as_str().unwrap();
    assert_ne!(earlier, latest);
    let projection = read_json(&source, &["why", "g2", "--json"]);
    assert_eq!(projection["node"]["outcome"], latest);
    assert!(!projection.to_string().contains(earlier));
    assert!(source.log().contains(earlier));
    assert!(source.log().contains(latest));
    assert_eq!(projection["node"]["state"], "abandoned");
    assert_eq!(source.log(), SOURCE);
}
