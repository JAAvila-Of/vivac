mod common;
use common::Sandbox;
use serde_json::{json, Value};

fn run(c: &Sandbox, args: &[&str], code: i32) -> Value {
    let (text, actual) = c.run(args);
    assert_eq!(actual, code, "{text}");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn inventory_is_pure_and_requires_setup_evidence() {
    let c = Sandbox::new_seeded("workflow-inventory");
    std::fs::create_dir_all(c.0.join(".codex/agents")).unwrap();
    let before = c.log();
    let inventory = run(&c, &["agents", "inventory"], 0);
    assert_eq!(before, c.log());
    assert_eq!(
        inventory["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["harness"] == "codex")
            .unwrap()["configured"],
        false
    );
}

fn selection(c: &Sandbox) -> Value {
    let path = ".claude/agents/reviewer.md";
    std::fs::create_dir_all(c.0.join(".claude/agents")).unwrap();
    std::fs::write(c.0.join(path), "---\nname: reviewer\ndescription: Review scoped changes.\nmodel: sonnet\neffort: high\n---\nInspect the full scope.\n```text\nKeep examples.\n```\n").unwrap();
    let (_, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0);
    let inventory = run(c, &["agents", "inventory"], 0);
    let source = &inventory["unmanaged"][0];
    json!({"why":"Preserve the reviewed agent", "items":[{"agent":null,"source":{"harness":source["harness"],"path":source["path"],"digest":source["digest"]},"destinations":[{"assignment":{"harness":"codex","name":"reviewer","model":"inherit","effort":"high","settings":{"sandbox_mode":"read-only"}},"path":".codex/agents/reviewer.toml","digest":null}]}]})
}

#[test]
fn reviewed_batch_transfers_literal_prompt_and_refuses_stale_preview() {
    let c = Sandbox::new_seeded("workflow-stale");
    let selected = selection(&c).to_string();
    let before = c.log();
    let preview = run(&c, &["agents", "plan", "--selection", &selected], 0);
    assert_eq!(before, c.log());
    std::fs::write(
        c.0.join(".claude/agents/reviewer.md"),
        "changed outside review",
    )
    .unwrap();
    let (_, code) = c.run(&[
        "agents",
        "apply",
        "--selection",
        &selected,
        "--plan-digest",
        preview["plan_digest"].as_str().unwrap(),
        "--yes",
    ]);
    assert_ne!(code, 0);
    assert_eq!(before, c.log());
    assert!(!c.0.join(".codex/agents/reviewer.toml").exists());
}

#[test]
fn reviewed_transfer_records_references_without_prompt_contents() {
    let c = Sandbox::new_seeded("workflow-transfer");
    let selected = selection(&c).to_string();
    let preview = run(&c, &["agents", "plan", "--selection", &selected], 0);
    run(
        &c,
        &[
            "agents",
            "apply",
            "--selection",
            &selected,
            "--plan-digest",
            preview["plan_digest"].as_str().unwrap(),
            "--yes",
        ],
        0,
    );
    let text = std::fs::read_to_string(c.0.join(".codex/agents/reviewer.toml")).unwrap();
    let document: toml::Table = toml::from_str(&text).unwrap();
    assert_eq!(
        document["developer_instructions"].as_str(),
        Some("Inspect the full scope.\n```text\nKeep examples.\n```\n")
    );
    assert!(!c.log().contains("Inspect the full scope."));
    assert!(!c.log().contains("Keep examples."));
}

#[test]
fn invalid_second_item_does_not_import_or_write_the_first() {
    let c = Sandbox::new_seeded("workflow-batch");
    let mut chosen = selection(&c);
    let mut invalid = chosen["items"][0].clone();
    invalid["source"]["path"] = json!(".claude/agents/absent.md");
    invalid["destinations"][0]["path"] = json!(".codex/agents/absent.toml");
    chosen["items"].as_array_mut().unwrap().push(invalid);
    let before = c.log();
    let (_, code) = c.run(&[
        "agents",
        "apply",
        "--selection",
        &chosen.to_string(),
        "--plan-digest",
        &"0".repeat(64),
        "--yes",
    ]);
    assert_ne!(code, 0);
    assert_eq!(before, c.log());
    assert!(!c.0.join(".codex/agents/reviewer.toml").exists());
}

#[test]
fn appearing_destination_invalidates_the_review_without_events() {
    let c = Sandbox::new_seeded("workflow-target-stale");
    let selected = selection(&c).to_string();
    let preview = run(&c, &["agents", "plan", "--selection", &selected], 0);
    let before = c.log();
    let target = c.0.join(".codex/agents/reviewer.toml");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "Keep these unreviewed bytes.").unwrap();
    let (_, code) = c.run(&[
        "agents",
        "apply",
        "--selection",
        &selected,
        "--plan-digest",
        preview["plan_digest"].as_str().unwrap(),
        "--yes",
    ]);
    assert_ne!(code, 0);
    assert_eq!(before, c.log());
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "Keep these unreviewed bytes."
    );
}

#[test]
fn native_homonyms_are_not_merged_into_existing_custody() {
    let c = Sandbox::new_seeded("workflow-identity");
    let chosen = selection(&c);
    let serialized = chosen.to_string();
    let preview = run(&c, &["agents", "plan", "--selection", &serialized], 0);
    let applied = run(
        &c,
        &[
            "agents",
            "apply",
            "--selection",
            &serialized,
            "--plan-digest",
            preview["plan_digest"].as_str().unwrap(),
            "--yes",
        ],
        0,
    );
    let first = applied["imported"][0]["agent"].as_str().unwrap();
    let inventory = run(&c, &["agents", "inventory"], 0);
    assert_eq!(inventory["agents"].as_array().unwrap().len(), 1);
    assert_eq!(inventory["agents"][0]["agent"], first);
    assert!(inventory["agents"][0]["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|mapping| mapping["configured"] == "current"));
    // The same name and native path do not authorize adopting a managed source as a new identity.
    let before = c.log();
    let (_, code) = c.run(&["agents", "plan", "--selection", &serialized]);
    assert_ne!(code, 0);
    assert_eq!(before, c.log());
}

#[test]
fn comparison_is_opt_in_and_refuses_secret_prompt_contents() {
    let c = Sandbox::new_seeded("workflow-compare");
    let chosen = selection(&c);
    let comparison = json!({"references":[chosen["items"][0]["source"].clone()]}).to_string();
    let before = c.log();
    let compared = run(&c, &["agents", "compare", "--selection", &comparison], 0);
    assert!(compared["sources"][0]["body"]
        .as_str()
        .unwrap()
        .contains("Keep examples."));
    assert_eq!(before, c.log());
    let path = c.0.join(".claude/agents/reviewer.md");
    let secret = format!("ghp_{}", "a".repeat(36));
    std::fs::write(
        &path,
        format!("---\nname: reviewer\ndescription: Review scope.\nmodel: sonnet\n---\n{secret}\n"),
    )
    .unwrap();
    let inventory = run(&c, &["agents", "inventory"], 0);
    let comparison = json!({"references":[{"harness":"claude-code","path":".claude/agents/reviewer.md","digest":inventory["unmanaged"][0]["digest"]}]}).to_string();
    let (output, code) = c.run(&["agents", "compare", "--selection", &comparison]);
    assert_ne!(code, 0);
    assert!(!output.contains(&secret));
    assert_eq!(before, c.log());
}

#[test]
fn three_custom_agents_transfer_together_and_reviewed_changes_can_be_resolved() {
    let c = Sandbox::new_seeded("workflow-three-agents");
    let mut chosen = selection(&c);
    let original = std::fs::read_to_string(c.0.join(".claude/agents/reviewer.md")).unwrap();
    for name in ["scout", "gate"] {
        std::fs::write(
            c.0.join(format!(".claude/agents/{name}.md")),
            original.replace("name: reviewer", &format!("name: {name}")),
        )
        .unwrap();
    }
    let inventory = run(&c, &["agents", "inventory"], 0);
    chosen["items"] = json!(inventory["unmanaged"].as_array().unwrap().iter().map(|source| {
        let name = source["name"].as_str().unwrap();
        json!({"agent":null,"source":{"harness":source["harness"],"path":source["path"],"digest":source["digest"]},
            "destinations":[{"assignment":{"harness":"codex","name":name,"model":"inherit","effort":"high","settings":{"sandbox_mode":"read-only"}},
                "path":format!(".codex/agents/{name}.toml"),"digest":null}]})
    }).collect::<Vec<_>>());
    let serialized = chosen.to_string();
    let preview = run(&c, &["agents", "plan", "--selection", &serialized], 0);
    run(
        &c,
        &[
            "agents",
            "apply",
            "--selection",
            &serialized,
            "--plan-digest",
            preview["plan_digest"].as_str().unwrap(),
            "--yes",
        ],
        0,
    );
    let inventory = run(&c, &["agents", "inventory"], 0);
    assert_eq!(inventory["agents"].as_array().unwrap().len(), 3);
    assert!(inventory["unmanaged"].as_array().unwrap().is_empty());
    let numbers: Vec<u64> = c
        .log()
        .lines()
        .filter_map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (event["payload"]["type"] == "node.created")
                .then(|| event["payload"]["num"].as_u64().unwrap())
        })
        .collect();
    assert_eq!(
        numbers.len(),
        numbers
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        "Every staged decision must have a distinct alias"
    );
    let (checked, code) = c.run(&["check"]);
    assert_eq!(code, 0, "{checked}");
    let reviewer = inventory["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "reviewer")
        .unwrap();
    let target = c.0.join(".codex/agents/reviewer.toml");
    let native = std::fs::read_to_string(&target).unwrap();
    std::fs::write(
        &target,
        native.replace("Inspect the full scope.", "Inspect the changed scope."),
    )
    .unwrap();
    let inventory = run(&c, &["agents", "inventory"], 0);
    let changed = inventory["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "reviewer")
        .unwrap();
    let source = changed["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["harness"] == "codex")
        .unwrap();
    let destination = changed["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["harness"] == "claude-code")
        .unwrap();
    let reviewed = json!({"why":"Use the explicitly reviewed native revision", "items":[{"agent":reviewer["agent"],
        "source":{"harness":source["harness"],"path":source["path"],"digest":source["digest"]},
        "destinations":[{"assignment":{"harness":"claude-code","name":"reviewer","model":"sonnet","effort":"high","settings":{}},
            "path":destination["path"],"digest":destination["digest"]}]}]});
    // Claude must be configured before it can become a destination, even though it was a valid import source.
    let (_, code) = c.run(&["setup", "claude-code", "--yes"]);
    assert_eq!(code, 0);
    let preview = run(
        &c,
        &["agents", "plan", "--selection", &reviewed.to_string()],
        0,
    );
    run(
        &c,
        &[
            "agents",
            "apply",
            "--selection",
            &reviewed.to_string(),
            "--plan-digest",
            preview["plan_digest"].as_str().unwrap(),
            "--yes",
        ],
        0,
    );
    assert!(
        std::fs::read_to_string(c.0.join(".claude/agents/reviewer.md"))
            .unwrap()
            .contains("Inspect the changed scope.")
    );
}
