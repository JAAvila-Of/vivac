mod common;
use common::Sandbox;
use serde_json::{json, Value};

const SOURCE: &str = ".claude/agents/reviewer.md";
const TARGET: &str = ".codex/agents/reviewer.toml";
const PROMPT: &str = "Inspect every requested path.\n\n```text\nReturn file and line evidence.\n```\nNever publish changes.\n";

fn native(c: &Sandbox, path: &str, name: &str, body: &str) {
    let file = c.0.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, format!("---\nname: {name}\ndescription: Inspect the scoped change.\nmodel: sonnet\neffort: high\ntools: Read, Glob, Grep\n---\n{body}")).unwrap();
}

fn run(c: &Sandbox, args: &[&str], expected: i32) -> Value {
    let (text, code) = c.run(args);
    assert_eq!(code, expected, "{text}");
    serde_json::from_str(&text).unwrap()
}

fn imported(c: &Sandbox) -> String {
    native(c, SOURCE, "reviewer", PROMPT);
    run(
        c,
        &[
            "agents",
            "import",
            "--harness",
            "claude-code",
            "--path",
            SOURCE,
            "--why",
            "Preserve the native contract",
            "--yes",
        ],
        0,
    )["agent"]
        .as_str()
        .unwrap()
        .into()
}

fn automatic(c: &Sandbox) {
    run(
        c,
        &[
            "agents",
            "reconcile",
            "--mode",
            "automatic",
            "--yes",
            "--why",
            "Keep project agents synchronized continuously",
        ],
        0,
    );
}

fn connect(c: &Sandbox, agent: &str) {
    let mut definition = run(c, &["agents", "show", agent], 0)["definition"].clone();
    definition["assignments"].as_array_mut().unwrap().push(json!({"harness":"codex","name":"reviewer","model":"inherit","effort":"high","settings":{"sandbox_mode":"read-only"}}));
    run(
        c,
        &[
            "agents",
            "set",
            agent,
            "--definition",
            &definition.to_string(),
            "--why",
            "Add the approved destination assignment",
        ],
        0,
    );
    run(
        c,
        &[
            "agents",
            "bind",
            agent,
            "--harness",
            "codex",
            "--path",
            TARGET,
        ],
        0,
    );
}

#[test]
fn literal_prompt_transfer_and_source_updates_do_not_need_a_model() {
    let c = Sandbox::new_seeded("reconcile-prompt");
    let agent = imported(&c);
    assert!(!c.log().contains("Inspect every requested path."));
    assert!(!c.log().contains("Return file and line evidence."));
    connect(&c, &agent);
    let before = c.log();
    run(&c, &["agents", "reconcile"], 1);
    assert_eq!(before, c.log());
    assert!(!c.0.join(TARGET).exists());
    automatic(&c);
    let target: toml::Table =
        toml::from_str(&std::fs::read_to_string(c.0.join(TARGET)).unwrap()).unwrap();
    assert_eq!(target["developer_instructions"].as_str(), Some(PROMPT));
    assert_eq!(
        std::fs::read_to_string(c.0.join(SOURCE))
            .unwrap()
            .split_once("---\n",)
            .unwrap()
            .1
            .split_once("---\n")
            .unwrap()
            .1,
        PROMPT
    );
    let log = c.log();
    automatic(&c);
    assert_eq!(log, c.log(), "policy and receipts must be idempotent");
    assert!(std::fs::read_to_string(c.0.join(".vivac/config"))
        .unwrap()
        .contains("automatic agent reconciliation"));
    let previous = std::fs::read(c.0.join(TARGET)).unwrap();
    let changed = std::fs::read_to_string(c.0.join(SOURCE))
        .unwrap()
        .replace("Never publish changes.", "Never publish or edit changes.");
    std::fs::write(c.0.join(SOURCE), changed).unwrap();
    let blocked = run(&c, &["agents", "reconcile", "--yes"], 1);
    assert!(!blocked["blocked"].as_array().unwrap().is_empty());
    assert_eq!(previous, std::fs::read(c.0.join(TARGET)).unwrap());
    run(
        &c,
        &[
            "agents",
            "import",
            &agent,
            "--harness",
            "claude-code",
            "--path",
            SOURCE,
            "--why",
            "Review the edited source prompt",
            "--yes",
        ],
        0,
    );
    run(&c, &["agents", "reconcile", "--yes"], 0);
    let target: toml::Table =
        toml::from_str(&std::fs::read_to_string(c.0.join(TARGET)).unwrap()).unwrap();
    assert_eq!(
        target["developer_instructions"].as_str(),
        Some(
            PROMPT
                .replace("Never publish changes.", "Never publish or edit changes.")
                .as_str()
        )
    );
    assert!(!c.log().contains("Never publish or edit changes."));
}

#[test]
fn session_hooks_import_new_agents_and_preserve_detached_files_without_model_calls() {
    let c = Sandbox::new_seeded("reconcile-hooks");
    automatic(&c);
    native(&c, SOURCE, "reviewer", PROMPT);
    let (text, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("Agent custody: mode automatic"), "{text}");
    let status = run(&c, &["agents", "status"], 1);
    assert!(status["unmanaged"].as_array().unwrap().is_empty());
    assert_eq!(
        status["agents"][0]["mappings"][0]["observed"]["state"],
        "unverified"
    );
    let agent = status["agents"][0]["agent"].as_str().unwrap();
    connect(&c, agent);
    assert!(!c.0.join(TARGET).exists());
    c.run_stdin(&["session", "prompt", "--hook"], "{}");
    assert!(c.0.join(TARGET).exists());
    run(
        &c,
        &[
            "agents",
            "detach",
            agent,
            "--harness",
            "codex",
            "--path",
            TARGET,
        ],
        0,
    );
    let saved = std::fs::read(c.0.join(TARGET)).unwrap();
    let result = run(&c, &["agents", "reconcile", "--yes"], 1);
    assert_eq!(result["excluded"].as_array().unwrap().len(), 1);
    assert_eq!(
        run(&c, &["agents", "status"], 1)["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(saved, std::fs::read(c.0.join(TARGET)).unwrap());
    run(
        &c,
        &[
            "agents",
            "reconcile",
            "--mode",
            "manual",
            "--yes",
            "--why",
            "Stop continuous custody",
        ],
        1,
    );
    native(&c, ".claude/agents/later.md", "later", PROMPT);
    c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(
        run(&c, &["agents", "status"], 1)["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_conflict_does_not_stop_an_independent_safe_destination() {
    let c = Sandbox::new_seeded("reconcile-independent");
    let agent = imported(&c);
    connect(&c, &agent);
    run(&c, &["agents", "reconcile", "--yes"], 0);
    let manual = std::fs::read_to_string(c.0.join(TARGET)).unwrap() + "\n# manual override\n";
    std::fs::write(c.0.join(TARGET), &manual).unwrap();
    native(&c, ".claude/agents/other.md", "reviewer", PROMPT);
    let result = run(
        &c,
        &[
            "agents",
            "reconcile",
            "--mode",
            "automatic",
            "--yes",
            "--why",
            "Manage independent destinations",
        ],
        1,
    );
    assert!(result["applied"].as_bool().unwrap());
    assert_eq!(result["imported"].as_array().unwrap().len(), 1);
    assert_ne!(
        result["imported"][0]["agent"].as_str(),
        Some(agent.as_str())
    );
    assert_eq!(result["blocked"].as_array().unwrap().len(), 1);
    assert_eq!(manual, std::fs::read_to_string(c.0.join(TARGET)).unwrap());
}

#[test]
fn previews_and_invalid_policy_requests_never_record_or_materialize() {
    let c = Sandbox::new_seeded("reconcile-preview");
    native(&c, SOURCE, "reviewer", PROMPT);
    let before = c.log();
    let bytes = std::fs::read(c.0.join(SOURCE)).unwrap();
    run(&c, &["agents", "reconcile", "--dry-run"], 1);
    for args in [
        vec![
            "agents",
            "reconcile",
            "--mode",
            "automatic",
            "--why",
            "Scope",
        ],
        vec![
            "agents",
            "reconcile",
            "--mode",
            "automatic",
            "--yes",
            "--why",
            "Scope",
            "--harness",
            "codex",
        ],
        vec!["agents", "reconcile", "--yes", "--dry-run"],
        vec![
            "agents",
            "import",
            "--harness",
            "claude-code",
            "--path",
            SOURCE,
            "--why",
            "Scope",
        ],
    ] {
        run(&c, &args, 2);
    }
    assert_eq!(before, c.log());
    assert_eq!(bytes, std::fs::read(c.0.join(SOURCE)).unwrap());
    assert!(!std::fs::read_to_string(c.0.join(".vivac/config"))
        .unwrap()
        .contains("automatic agent reconciliation"));
}

#[test]
fn unsafe_prompt_import_is_refused_without_echoing_or_recording_it() {
    let c = Sandbox::new_seeded("reconcile-secret");
    let secret = format!("ghp_{}", "x".repeat(30));
    native(&c, SOURCE, "reviewer", &format!("```text\n{secret}\n```\n"));
    let before = c.log();
    let result = run(
        &c,
        &[
            "agents",
            "import",
            "--harness",
            "claude-code",
            "--path",
            SOURCE,
            "--why",
            "Preserve instructions",
            "--yes",
        ],
        3,
    );
    assert!(!result.to_string().contains(&secret));
    assert_eq!(before, c.log());
}

#[test]
fn codex_prompt_transfers_to_claude_without_losing_line_endings() {
    let c = Sandbox::new_seeded("reconcile-reverse");
    let prompt = "Review the scope.\r\n\r\n```rust\r\nassert!(true);\r\n```\r\nDo not edit.\r\n";
    let source = c.0.join(TARGET);
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    let values = toml::Table::from_iter([
        ("name".into(), toml::Value::String("reviewer".into())),
        (
            "description".into(),
            toml::Value::String("Review the requested scope.".into()),
        ),
        (
            "developer_instructions".into(),
            toml::Value::String(prompt.into()),
        ),
    ]);
    std::fs::write(&source, toml::to_string(&values).unwrap()).unwrap();
    let agent = run(
        &c,
        &[
            "agents",
            "import",
            "--harness",
            "codex",
            "--path",
            TARGET,
            "--yes",
            "--why",
            "Preserve the complete Codex prompt",
        ],
        0,
    )["agent"]
        .as_str()
        .unwrap()
        .to_string();
    let mut definition = run(&c, &["agents", "show", &agent], 0)["definition"].clone();
    definition["assignments"].as_array_mut().unwrap().push(json!({
        "harness":"claude-code", "name":"reviewer", "model":"inherit", "effort":"high", "settings":{}
    }));
    run(
        &c,
        &[
            "agents",
            "set",
            &agent,
            "--definition",
            &definition.to_string(),
            "--why",
            "Use the approved Claude assignment",
        ],
        0,
    );
    run(
        &c,
        &[
            "agents",
            "bind",
            &agent,
            "--harness",
            "claude-code",
            "--path",
            SOURCE,
        ],
        0,
    );
    run(&c, &["agents", "reconcile", "--yes"], 0);
    let rendered = std::fs::read_to_string(c.0.join(SOURCE)).unwrap();
    assert_eq!(rendered.split_once("\n---\n").unwrap().1, prompt);
    let before = c.log();
    run(&c, &["agents", "reconcile", "--yes"], 0);
    assert_eq!(before, c.log());
    assert!(!before.contains("assert!(true)"));
    std::fs::remove_file(&source).unwrap();
    assert!(!run(&c, &["agents", "reconcile", "--yes"], 1)["blocked"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(rendered, std::fs::read_to_string(c.0.join(SOURCE)).unwrap());
}

#[test]
fn continuous_custody_authorization_is_independent_for_each_lane() {
    let c = Sandbox::new_seeded("reconcile-lanes");
    let child = c.0.join("second");
    std::fs::create_dir_all(&child).unwrap();
    let at = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .args(args)
            .current_dir(&child)
            .env("VIVAC_HOME", c.global_home())
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "{text}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        text
    };
    at(&["init", "--join", c.0.to_str().unwrap(), "--yes"]);
    automatic(&c);
    let preview: Value = serde_json::from_str(&at(&["agents", "reconcile"])).unwrap();
    assert_eq!(preview["policy"]["mode"], "manual");
    at(&[
        "agents",
        "reconcile",
        "--mode",
        "automatic",
        "--yes",
        "--why",
        "Authorize this lane",
    ]);
    run(
        &c,
        &[
            "agents",
            "reconcile",
            "--mode",
            "manual",
            "--yes",
            "--why",
            "Disable only the first lane",
        ],
        0,
    );
    let before = c.log();
    let preview: Value = serde_json::from_str(&at(&["agents", "reconcile"])).unwrap();
    assert_eq!(preview["policy"]["mode"], "automatic");
    assert_eq!(
        run(&c, &["agents", "reconcile"], 0)["policy"]["mode"],
        "manual"
    );
    assert_eq!(before, c.log());
}

#[test]
fn coordinator_inventory_matches_status_across_observations_and_filters() {
    let c = Sandbox::new_seeded("reconcile-inventory");
    let agent = imported(&c);
    let compare = || {
        for filter in [
            vec![],
            vec![agent.as_str()],
            vec!["--harness", "codex"],
            vec![agent.as_str(), "--harness", "claude-code"],
        ] {
            let mut status_args = vec!["agents", "status"];
            status_args.extend(filter.iter().copied());
            let status: Value = serde_json::from_str(&c.run(&status_args).0).unwrap();
            let mut reconcile_args = vec!["agents", "reconcile"];
            reconcile_args.extend(filter);
            let reconciliation: Value = serde_json::from_str(&c.run(&reconcile_args).0).unwrap();
            assert_eq!(reconciliation["unverified"], status["unverified"]);
            let expected: Vec<Value> = status["agents"].as_array().unwrap().iter()
                .flat_map(|agent| agent["mappings"].as_array().unwrap().iter()
                    .filter(|mapping| mapping["configured"] == "unbound")
                    .map(|mapping| json!({"agent":agent["agent"],"harness":mapping["harness"],"state":"unbound","reason":"Declare a destination explicitly."})))
                .collect();
            let actual: Vec<Value> = reconciliation["blocked"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["state"] == "unbound")
                .cloned()
                .collect();
            assert_eq!(actual, expected);
        }
    };
    compare();
    let mut definition = run(&c, &["agents", "show", &agent], 0)["definition"].clone();
    definition["assignments"].as_array_mut().unwrap().push(json!({"harness":"codex","name":"reviewer","model":"inherit","effort":"high","settings":{}}));
    run(
        &c,
        &[
            "agents",
            "set",
            &agent,
            "--definition",
            &definition.to_string(),
            "--why",
            "Declare another assignment",
        ],
        0,
    );
    compare();
    let revision = run(&c, &["agents", "show", &agent], 0)["revision"]
        .as_str()
        .unwrap()
        .to_string();
    for model in ["other-model", "sonnet"] {
        run(
            &c,
            &[
                "agents",
                "observe",
                &agent,
                "--harness",
                "claude-code",
                "--path",
                SOURCE,
                "--revision",
                &revision,
                "--model",
                model,
                "--effort",
                "high",
                "--evidence",
                "The person reported this launch assignment",
            ],
            i32::from(model != "sonnet"),
        );
        compare();
    }
    definition["contract"]["purpose"] = json!("Inspect another requested scope.");
    run(
        &c,
        &[
            "agents",
            "set",
            &agent,
            "--definition",
            &definition.to_string(),
            "--why",
            "Revise the reviewed purpose",
        ],
        0,
    );
    compare();
    run(
        &c,
        &[
            "agents",
            "retire",
            &agent,
            "--why",
            "Retire the reviewed contract",
        ],
        0,
    );
    compare();
}
