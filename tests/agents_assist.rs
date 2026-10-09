mod common;
use common::Sandbox;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PATH: &str = ".claude/agents/reviewer.md";
const BODY: &str = "Inspect the full scope.\n```text\nKeep examples.\n```\n";

fn run(c: &Sandbox, args: &[&str]) -> Value {
    let (text, code) = raw(c, args);
    assert_eq!(code, 0, "{text}");
    serde_json::from_str(&text).unwrap()
}

fn raw(c: &Sandbox, args: &[&str]) -> (String, i32) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("CODEX_HOME", c.0.join(".codex"))
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
        output.status.code().unwrap_or(-1),
    )
}

fn fixture(name: &str) -> (Sandbox, Value) {
    let c = Sandbox::new_seeded(name);
    c.ok(&["setup", "codex", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex/agents")).unwrap();
    std::fs::write(c.0.join(".codex/models_cache.json"), r#"{"models":[{"slug":"cached-model","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]}]}"#).unwrap();
    std::fs::write(c.0.join(".codex/agents/existing.toml"), "name = \"existing\"\ndescription = \"Review existing changes.\"\nmodel = \"configured-model\"\nmodel_reasoning_effort = \"high\"\ndeveloper_instructions = \"Review the requested scope.\"\n").unwrap();
    std::fs::create_dir_all(c.0.join(".claude/agents")).unwrap();
    std::fs::write(c.0.join(PATH), format!("---\nname: reviewer\ndescription: Review scoped changes.\nmodel: sonnet\neffort: high\npermissionMode: plan\n---\n{BODY}")).unwrap();
    let inventory = run(&c, &["agents", "inventory"]);
    let source = inventory["unmanaged"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == PATH)
        .unwrap();
    let selection = json!({"references":[{"harness":source["harness"],"path":source["path"],"digest":source["digest"]}],"harnesses":["codex"]});
    (c, selection)
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn reject(c: &Sandbox, selection: &Value) -> String {
    let before = files(&c.0);
    let (text, code) = raw(
        c,
        &["agents", "assist", "--selection", &selection.to_string()],
    );
    assert_ne!(code, 0, "{text}");
    assert!(!text.contains(BODY));
    assert_eq!(files(&c.0), before);
    text
}

#[test]
fn assist_returns_literal_source_and_destination_evidence_without_writes() {
    let (c, selection) = fixture("assist-pure");
    let log = c.log();
    let native = std::fs::read(c.0.join(PATH)).unwrap();
    let inventory = run(&c, &["agents", "inventory"]);
    let before = files(&c.0);
    let output = run(
        &c,
        &["agents", "assist", "--selection", &selection.to_string()],
    );
    assert_eq!(output["applied"], false);
    assert_eq!(output["proposer"], "session-model");
    assert_eq!(output["runtime_verified"], false);
    assert_eq!(output["sources"][0]["body"], BODY);
    assert_eq!(
        output["sources"][0]["reference"],
        selection["references"][0]
    );
    assert_eq!(output["sources"][0]["metadata"]["model"], "sonnet");
    assert_eq!(
        output["sources"][0]["metadata"]["settings"]["permissionMode"],
        "plan"
    );
    assert_eq!(
        output["sources"][0]["destinations"][0]["unrepresentable_settings"],
        json!(["permissionMode"])
    );
    assert!(output["sources"][0]["agent"].is_null());
    assert!(output["sources"][0]["revision"].is_null());
    let destination = inventory["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["harness"] == "codex")
        .unwrap();
    assert_eq!(&output["harnesses"][0], destination);
    assert!(output["harnesses"][0]["models"]
        .as_array()
        .unwrap()
        .contains(&json!("configured-model")));
    assert!(output["harnesses"][0]["model_catalog"].is_object());
    assert_eq!(
        output["harnesses"][0]["model_catalog"]["models"][0]["id"],
        "cached-model"
    );
    assert_eq!(
        output["harnesses"][0]["capabilities"],
        json!(["model", "effort", "sandbox_mode"])
    );
    assert!(output.get("selection_template").is_none());
    assert_eq!(c.log(), log);
    assert_eq!(std::fs::read(c.0.join(PATH)).unwrap(), native);
    assert!(!c.0.join(".codex/agents/reviewer.toml").exists());
    assert_eq!(files(&c.0), before);
}

#[test]
fn assist_rejects_stale_and_secret_sources_without_returning_contents() {
    let (c, selection) = fixture("assist-unsafe");
    std::fs::write(
        c.0.join(PATH),
        std::fs::read_to_string(c.0.join(PATH))
            .unwrap()
            .replace(BODY, "Changed instructions.\n"),
    )
    .unwrap();
    assert!(reject(&c, &selection).contains("changed after review"));
    let secret = format!("ghp_{}", "a".repeat(36));
    std::fs::write(
        c.0.join(PATH),
        format!("---\nname: reviewer\ndescription: Review scope.\nmodel: sonnet\n---\n{secret}\n"),
    )
    .unwrap();
    let inventory = run(&c, &["agents", "inventory"]);
    let mut selection = selection;
    selection["references"][0]["digest"] = inventory["unmanaged"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == PATH)
        .unwrap()["digest"]
        .clone();
    assert!(!reject(&c, &selection).contains(&secret));
}

#[test]
fn assist_rejects_aggregate_prompt_contents_over_four_mib() {
    let (c, mut selection) = fixture("assist-size");
    let body = "Review scoped changes.\n".repeat(40_000);
    for index in 0..5 {
        let path = format!(".claude/agents/large-{index}.md");
        std::fs::write(
            c.0.join(path),
            format!(
                "---\nname: large-{index}\ndescription: Review scope.\nmodel: sonnet\n---\n{body}"
            ),
        )
        .unwrap();
    }
    let inventory = run(&c, &["agents", "inventory"]);
    selection["references"] = json!(inventory["unmanaged"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["path"].as_str().unwrap().contains("/large-"))
        .map(|row| json!({"harness":row["harness"],"path":row["path"],"digest":row["digest"]}))
        .collect::<Vec<_>>());
    assert!(reject(&c, &selection).contains("4 MiB"));
}

#[test]
fn assist_requires_distinct_configured_destinations_and_strict_references() {
    let (c, selection) = fixture("assist-invalid");
    std::fs::create_dir_all(c.0.join(".opencode/agents")).unwrap();
    for harnesses in [
        json!([]),
        json!(["unknown"]),
        json!(["opencode"]),
        json!(["codex", "codex"]),
        json!(["claude-code"]),
    ] {
        let mut invalid = selection.clone();
        invalid["harnesses"] = harnesses;
        reject(&c, &invalid);
    }
    let mut duplicate = selection.clone();
    duplicate["references"]
        .as_array_mut()
        .unwrap()
        .push(selection["references"][0].clone());
    reject(&c, &duplicate);
    for references in [
        json!([]),
        json!(vec![selection["references"][0].clone(); 33]),
    ] {
        let mut invalid = selection.clone();
        invalid["references"] = references;
        reject(&c, &invalid);
    }
    let mut extra = selection.clone();
    extra["why"] = json!("Do not write");
    reject(&c, &extra);
    for flag in ["--yes", "--dry-run", "--plan-digest=unused"] {
        assert_ne!(
            c.run(&[
                "agents",
                "assist",
                "--selection",
                &selection.to_string(),
                flag
            ])
            .1,
            0
        );
    }
    assert_ne!(c.run(&["agents", "assist"]).1, 0);
    c.ok(&["setup", "claude-code", "--yes"]);
    let mut own = selection;
    own["harnesses"] = json!(["claude-code"]);
    assert!(reject(&c, &own).contains("other than its harness"));
}

#[test]
fn assist_uses_real_custody_and_refuses_detached_and_retired_sources() {
    let (c, selection) = fixture("assist-custody");
    let imported = run(
        &c,
        &[
            "agents",
            "import",
            "--harness",
            "claude-code",
            "--path",
            PATH,
            "--why",
            "Preserve the reviewed agent",
            "--yes",
        ],
    );
    let agent = imported["agent"].as_str().unwrap();
    let output = run(
        &c,
        &["agents", "assist", "--selection", &selection.to_string()],
    );
    assert_eq!(output["sources"][0]["agent"], agent);
    assert_eq!(output["sources"][0]["revision"], imported["revision"]);
    let homonym = ".claude/agents/homonym.md";
    std::fs::copy(c.0.join(PATH), c.0.join(homonym)).unwrap();
    let mut selection = selection;
    let mut reference = selection["references"][0].clone();
    reference["path"] = json!(homonym);
    selection["references"]
        .as_array_mut()
        .unwrap()
        .push(reference);
    let output = run(
        &c,
        &["agents", "assist", "--selection", &selection.to_string()],
    );
    assert!(output["sources"][1]["agent"].is_null());
    assert!(output["sources"][1]["revision"].is_null());
    c.ok(&[
        "agents",
        "detach",
        agent,
        "--harness",
        "claude-code",
        "--path",
        PATH,
    ]);
    assert!(reject(&c, &selection).contains("Detached"));
    c.ok(&[
        "agents",
        "import",
        agent,
        "--harness",
        "claude-code",
        "--path",
        PATH,
        "--why",
        "Explicitly restore custody",
        "--yes",
    ]);
    c.ok(&[
        "agents",
        "retire",
        agent,
        "--why",
        "Stop assigning this contract",
    ]);
    assert!(reject(&c, &selection).contains("retired"));
}

#[test]
fn assist_accepts_selection_files_and_preserves_requested_harness_order() {
    let (c, mut selection) = fixture("assist-file");
    c.ok(&["setup", "claude-code", "--yes"]);
    selection["harnesses"] = json!(["claude-code", "codex"]);
    std::fs::write(c.0.join("selection.json"), selection.to_string()).unwrap();
    let output = run(&c, &["agents", "assist", "--selection", "selection.json"]);
    assert_eq!(output["harnesses"][0]["harness"], "claude-code");
    assert_eq!(output["harnesses"][1]["harness"], "codex");
    assert_eq!(
        output["sources"][0]["destinations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(output["sources"][0]["destinations"][0]["harness"], "codex");
    std::fs::write(c.0.join(".codex/models_cache.json"), r#"{"models":[{"slug":"fresh-model","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]}]}"#).unwrap();
    let before = files(&c.0);
    let refreshed = run(&c, &["agents", "assist", "--selection", "selection.json"]);
    assert_eq!(
        refreshed["harnesses"][1]["model_catalog"]["models"][0]["id"],
        "fresh-model"
    );
    assert_eq!(
        refreshed["harnesses"][1]["model_catalog"]["models"][0]["efforts"],
        json!(["low"])
    );
    assert_eq!(files(&c.0), before);
}
