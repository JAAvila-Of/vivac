mod common;
use common::Sandbox;
use serde_json::{json, Value};

fn definition(model: &str) -> String {
    json!({"schema_version":1,"name":"reviewer", "contract":{"purpose":"Review the scoped change.","duties":["Return evidence."],"limits":["Do not publish."],"acceptance":["Cite changed lines."]},"assignments":[{"harness":"codex","name":"reviewer","model":model,"effort":"high","settings":{}}]}).to_string()
}

fn add(c: &Sandbox) -> Value {
    serde_json::from_str(&c.ok(&[
        "agents",
        "add",
        "--definition",
        &definition("inherit"),
        "--why",
        "Record the reviewed contract",
    ]))
    .unwrap()
}

#[test]
fn an_authored_contract_is_a_revision_backed_by_a_decision_in_events() {
    let c = Sandbox::new_seeded("agents-contract");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    let revision = created["revision"].as_str().unwrap();
    assert_ne!(agent, revision);
    let shown: Value = serde_json::from_str(&c.ok(&["agents", "show", agent])).unwrap();
    assert_eq!(shown["revision"], revision);
    assert_eq!(
        shown["definition"]["contract"]["duties"][0],
        "Return evidence."
    );
    let log = c.log();
    assert!(log.contains("agent.recorded"), "{log}");
    assert!(log.contains("\"kind\":\"decision\""), "{log}");
    assert!(std::fs::read_to_string(c.0.join(".vivac/config"))
        .unwrap()
        .contains("this tree holds agent custody"));
    assert!(!c.0.join(".vivac/agents.json").exists());
    let status = parse_run(&c, &["agents", "status", agent], 1);
    assert_eq!(status["agents"][0]["mappings"][0]["configured"], "unbound");
    assert!(status["agents"][0]["mappings"][0]["path"].is_null());
    assert!(status["errors"].as_array().unwrap().is_empty());
}

const TARGET: &str = ".codex/agents/reviewer.toml";

fn parse_run(c: &Sandbox, args: &[&str], expected: i32) -> Value {
    let (text, code) = c.run(args);
    assert_eq!(code, expected, "{text}");
    serde_json::from_str(&text).unwrap()
}

fn bind(c: &Sandbox, agent: &str) {
    c.ok(&[
        "agents",
        "bind",
        agent,
        "--harness",
        "codex",
        "--path",
        TARGET,
    ]);
}

#[test]
fn preview_sync_observation_revision_detachment_and_retirement_form_one_cycle() {
    let c = Sandbox::new_seeded("agents-cycle");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    let revision = created["revision"].as_str().unwrap();
    bind(&c, agent);
    let before = c.log();
    let preview = parse_run(&c, &["agents", "sync", agent], 1);
    assert_eq!(preview["plan"][0]["state"], "missing");
    assert!(!c.0.join(TARGET).exists());
    assert_eq!(before, c.log());
    parse_run(&c, &["agents", "sync", agent, "--yes", "--dry-run"], 1);
    assert_eq!(before, c.log());
    c.ok(&["agents", "sync", agent, "--yes"]);
    let first = std::fs::read(c.0.join(TARGET)).unwrap();
    let materialized = c.log();
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert_eq!(
        materialized,
        c.log(),
        "an idempotent sync appended an event"
    );
    let status = parse_run(&c, &["agents", "status", agent], 1);
    assert_eq!(status["agents"][0]["mappings"][0]["configured"], "current");
    assert_eq!(
        status["agents"][0]["mappings"][0]["observed"]["state"],
        "unverified"
    );
    let observed = parse_run(
        &c,
        &[
            "agents",
            "observe",
            agent,
            "--harness",
            "codex",
            "--path",
            TARGET,
            "--revision",
            revision,
            "--model",
            "inherit",
            "--effort",
            "high",
            "--evidence",
            "The person reported this launch assignment",
        ],
        0,
    );
    assert_eq!(observed["runtime_verified"], false);
    parse_run(&c, &["agents", "status", agent], 0);
    let changed = parse_run(
        &c,
        &[
            "agents",
            "set",
            agent,
            "--definition",
            &definition("explicit-model"),
            "--why",
            "Change only the reviewed model",
        ],
        0,
    );
    assert_ne!(changed["revision"], revision);
    let status = parse_run(&c, &["agents", "status", agent], 1);
    assert_eq!(status["agents"][0]["mappings"][0]["configured"], "pending");
    assert_eq!(
        status["agents"][0]["mappings"][0]["observed"]["state"],
        "stale"
    );
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert_ne!(std::fs::read(c.0.join(TARGET)).unwrap(), first);
    let kept = std::fs::read(c.0.join(TARGET)).unwrap();
    c.ok(&[
        "agents",
        "detach",
        agent,
        "--harness",
        "codex",
        "--path",
        TARGET,
    ]);
    assert_eq!(std::fs::read(c.0.join(TARGET)).unwrap(), kept);
    bind(&c, agent);
    c.ok(&[
        "agents",
        "retire",
        agent,
        "--why",
        "Stop assigning this contract",
    ]);
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert_eq!(std::fs::read(c.0.join(TARGET)).unwrap(), kept);
    let shown = parse_run(&c, &["agents", "show", agent], 0);
    assert_eq!(shown["definition"]["retired"], true);
    assert!(c.log().contains("\"state\":\"superseded\""));
}

#[test]
fn changing_one_harness_does_not_rewrite_an_equivalent_projection_of_the_other() {
    let c = Sandbox::new_seeded("agents-two-harnesses");
    let mut both: Value = serde_json::from_str(&definition("inherit")).unwrap();
    both["assignments"].as_array_mut().unwrap().push(json!({"harness":"claude-code","name":"reviewer","model":"inherit","effort":"high","settings":{}}));
    let created = parse_run(
        &c,
        &[
            "agents",
            "add",
            "--definition",
            &both.to_string(),
            "--why",
            "Declare both assignments",
        ],
        0,
    );
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    c.ok(&[
        "agents",
        "bind",
        agent,
        "--harness",
        "claude-code",
        "--path",
        ".claude/agents/reviewer.md",
    ]);
    c.ok(&["agents", "sync", agent, "--yes"]);
    let claude = std::fs::read(c.0.join(".claude/agents/reviewer.md")).unwrap();
    let codex = std::fs::read(c.0.join(TARGET)).unwrap();
    both["assignments"][0]["model"] = json!("explicit-model");
    c.ok(&[
        "agents",
        "set",
        agent,
        "--definition",
        &both.to_string(),
        "--why",
        "Change only Codex",
    ]);
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert_eq!(
        std::fs::read(c.0.join(".claude/agents/reviewer.md")).unwrap(),
        claude
    );
    assert_ne!(std::fs::read(c.0.join(TARGET)).unwrap(), codex);
    let status = parse_run(
        &c,
        &["agents", "status", agent, "--harness", "claude-code"],
        1,
    );
    assert_eq!(status["agents"][0]["mappings"][0]["configured"], "current");
}

#[test]
fn manual_divergence_requires_the_exact_current_digest_and_explicit_yes() {
    let c = Sandbox::new_seeded("agents-divergence");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    c.ok(&["agents", "sync", agent, "--yes"]);
    let path = c.0.join(TARGET);
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        original.replace("Return evidence.", "Manual reviewed difference."),
    )
    .unwrap();
    let log = c.log();
    let preview = parse_run(&c, &["agents", "diff", agent], 1);
    assert_eq!(preview["plan"][0]["state"], "diverged");
    assert!(!preview.to_string().contains("Manual reviewed difference"));
    parse_run(&c, &["agents", "sync", agent, "--yes"], 1);
    parse_run(
        &c,
        &["agents", "sync", agent, "--yes", "--accept-digest", "wrong"],
        1,
    );
    assert_eq!(log, c.log());
    let digest = preview["plan"][0]["before"]["digest"].as_str().unwrap();
    c.ok(&["agents", "sync", agent, "--yes", "--accept-digest", digest]);
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[test]
fn adoption_checks_metadata_and_records_no_native_body() {
    let c = Sandbox::new_seeded("agents-adoption");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    std::fs::create_dir_all(c.0.join(".codex/agents")).unwrap();
    std::fs::write(c.0.join(TARGET), "name = \"reviewer\"\nmodel_reasoning_effort = \"high\"\ndescription = \"Old description\"\ndeveloper_instructions = \"PRIVATE LEGACY BODY\"\n").unwrap();
    let scan = parse_run(&c, &["agents", "scan"], 0);
    assert_eq!(scan["unmanaged"].as_array().unwrap().len(), 1);
    assert!(!scan.to_string().contains("PRIVATE LEGACY BODY"));
    let digest = scan["unmanaged"][0]["digest"].as_str().unwrap();
    parse_run(
        &c,
        &[
            "agents",
            "bind",
            agent,
            "--harness",
            "codex",
            "--path",
            TARGET,
        ],
        1,
    );
    parse_run(
        &c,
        &[
            "agents",
            "adopt",
            agent,
            "--harness",
            "codex",
            "--path",
            TARGET,
            "--digest",
            "wrong",
            "--why",
            "Review this file",
        ],
        1,
    );
    c.ok(&[
        "agents",
        "adopt",
        agent,
        "--harness",
        "codex",
        "--path",
        TARGET,
        "--digest",
        digest,
        "--why",
        "Use only the authored reviewed contract",
    ]);
    assert!(!c.log().contains("PRIVATE LEGACY BODY"));
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert!(!std::fs::read_to_string(c.0.join(TARGET))
        .unwrap()
        .contains("PRIVATE LEGACY BODY"));
}

#[test]
fn unsupported_native_settings_survive_even_an_accepted_fingerprint() {
    let c = Sandbox::new_seeded("agents-unsupported");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    c.ok(&["agents", "sync", agent, "--yes"]);
    let path = c.0.join(TARGET);
    let native = std::fs::read_to_string(&path).unwrap() + "\nunknown_setting = true\n";
    std::fs::write(&path, &native).unwrap();
    let preview = parse_run(&c, &["agents", "diff", agent], 1);
    let digest = preview["plan"][0]["before"]["digest"].as_str().unwrap();
    let log = c.log();
    parse_run(
        &c,
        &["agents", "sync", agent, "--yes", "--accept-digest", digest],
        1,
    );
    assert_eq!(c.log(), log);
    assert_eq!(std::fs::read_to_string(path).unwrap(), native);
}

#[test]
fn definition_files_unknown_fields_empty_contracts_and_secret_prose_are_checked_before_writes() {
    let c = Sandbox::new_seeded("agents-validation");
    std::fs::write(c.0.join("reviewed.json"), definition("inherit")).unwrap();
    parse_run(
        &c,
        &[
            "agents",
            "add",
            "--definition",
            "reviewed.json",
            "--why",
            "Review the authored file",
        ],
        0,
    );
    let before = c.log();
    let mut invalid: Value = serde_json::from_str(&definition("inherit")).unwrap();
    invalid["native_body"] = json!("Do not import this");
    parse_run(
        &c,
        &[
            "agents",
            "add",
            "--definition",
            &invalid.to_string(),
            "--why",
            "Reject unknown fields",
        ],
        2,
    );
    invalid.as_object_mut().unwrap().remove("native_body");
    invalid["contract"]["duties"] = json!([]);
    parse_run(
        &c,
        &[
            "agents",
            "add",
            "--definition",
            &invalid.to_string(),
            "--why",
            "Reject an empty contract",
        ],
        2,
    );
    invalid["contract"]["duties"] = json!(["Use person@example.com"]);
    parse_run(
        &c,
        &[
            "agents",
            "add",
            "--definition",
            &invalid.to_string(),
            "--why",
            "Reject personal data",
        ],
        3,
    );
    assert_eq!(before, c.log());
}

#[test]
fn a_missing_config_is_not_regenerated_by_scan_but_recovers_the_agent_version_on_a_write() {
    let c = Sandbox::new_seeded("agents-config-recovery");
    let created = add(&c);
    std::fs::remove_file(c.0.join(".vivac/config")).unwrap();
    parse_run(&c, &["agents", "scan"], 5);
    assert!(!c.0.join(".vivac/config").exists());
    let revision = created["revision"].as_str().unwrap();
    c.ok(&[
        "note",
        revision,
        "Recover the config from the complete event log",
    ]);
    assert!(std::fs::read_to_string(c.0.join(".vivac/config"))
        .unwrap()
        .contains("this tree holds agent custody"));
}

#[test]
fn path_escape_and_binding_collision_are_refused_without_changing_authority() {
    let c = Sandbox::new_seeded("agents-target-collision");
    let first = add(&c);
    let second = add(&c);
    let first = first["agent"].as_str().unwrap();
    let second = second["agent"].as_str().unwrap();
    bind(&c, first);
    let before = c.log();
    parse_run(
        &c,
        &[
            "agents",
            "bind",
            second,
            "--harness",
            "codex",
            "--path",
            TARGET,
        ],
        1,
    );
    parse_run(
        &c,
        &[
            "agents",
            "bind",
            second,
            "--harness",
            "codex",
            "--path",
            ".codex/agents/../escape.toml",
        ],
        2,
    );
    assert_eq!(before, c.log());
}

#[test]
fn concurrent_bindings_read_authority_after_the_write_lock() {
    let c = Sandbox::new_seeded("agents-concurrent-bind");
    let first = add(&c);
    let second = add(&c);
    let launch = |id: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
            .current_dir(&c.0)
            .env("VIVAC_HOME", c.global_home())
            .args(["agents", "bind", id, "--harness", "codex", "--path", TARGET])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = launch(first["agent"].as_str().unwrap());
    let second = launch(second["agent"].as_str().unwrap());
    let mut codes = [
        first.wait_with_output().unwrap().status.code().unwrap(),
        second.wait_with_output().unwrap().status.code().unwrap(),
    ];
    codes.sort();
    assert_eq!(codes, [0, 1]);
    let events: Vec<Value> = c
        .log()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["payload"]["type"] == "agent.bound")
            .count(),
        1
    );
    let mut sequences: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    let count = sequences.len();
    sequences.sort();
    sequences.dedup();
    assert_eq!(sequences.len(), count);
}

#[test]
fn deleting_a_projection_requires_explicit_recreation() {
    let c = Sandbox::new_seeded("agents-recreate");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    c.ok(&["agents", "sync", agent, "--yes"]);
    let original = std::fs::read(c.0.join(TARGET)).unwrap();
    std::fs::remove_file(c.0.join(TARGET)).unwrap();
    let before = c.log();
    parse_run(&c, &["agents", "sync", agent], 1);
    assert!(!c.0.join(TARGET).exists());
    assert_eq!(before, c.log());
    c.ok(&["agents", "sync", agent, "--yes"]);
    assert_eq!(std::fs::read(c.0.join(TARGET)).unwrap(), original);
}

#[cfg(windows)]
#[test]
fn failed_receipt_append_rolls_back_the_materialized_file() {
    let c = Sandbox::new_seeded("agents-receipt-failure");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    let path = c.0.join(".vivac/events");
    let before = c.log();
    let original_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let result = c.run(&["agents", "sync", agent, "--yes"]);
    std::fs::set_permissions(path, original_permissions).unwrap();
    assert_eq!(result.1, 5, "{}", result.0);
    assert_eq!(before, c.log());
    assert!(
        !c.0.join(TARGET).exists(),
        "materialization survived without a receipt"
    );
}

#[test]
fn invalid_provenance_in_an_agent_event_is_refused_without_repair() {
    let c = Sandbox::new_seeded("agents-source-missing");
    c.append_raw_line(&json!({"seq":900,"id":"01AGENTRECORDED00000000001","ts":"2026-09-10T10:00:00Z","actor":"a_test0000000","lane":"main", "payload":{"type":"agent.recorded","agent":"01AGENTIDENTITY000000000001","node":"01MISSINGDECISION0000000001","definition":serde_json::from_str::<Value>(&definition("inherit")).unwrap()}}).to_string());
    let before = c.log();
    parse_run(&c, &["agents", "scan"], 1);
    c.ok(&["tree"]);
    let index = c.0.join(".vivac/index");
    let saved = std::fs::read(&index).unwrap();
    let indexed = parse_run(&c, &["agents", "scan"], 1);
    assert!(indexed["error"]
        .as_str()
        .unwrap()
        .contains("source decision"));
    assert_eq!(saved, std::fs::read(&index).unwrap());
    assert_eq!(before, c.log());
}

#[test]
fn an_unknown_config_version_is_withheld_when_managing_agents() {
    let c = Sandbox::new_seeded("agents-config-version-withheld");
    let path = c.0.join(".vivac/config");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let secret = format!("ghp_{}", "a".repeat(30));
    config["version"] = json!(secret);
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let config_before = std::fs::read(&path).unwrap();
    let log_before = std::fs::read(c.0.join(".vivac/events")).unwrap();
    let (output, code) = c.run(&[
        "agents",
        "add",
        "--definition",
        &definition("inherit"),
        "--why",
        "Record the reviewed contract",
    ]);
    assert_eq!(code, 5);
    assert!(
        !output.contains(&secret),
        "configuration values leaked into the response"
    );
    assert_eq!(std::fs::read(path).unwrap(), config_before);
    assert_eq!(
        std::fs::read(c.0.join(".vivac/events")).unwrap(),
        log_before
    );
    assert!(!c.0.join(".codex/agents").exists());
    assert!(!c.0.join(".claude/agents").exists());
}

fn custody_reads(c: &Sandbox, agent: &str) -> Vec<(String, i32)> {
    [
        vec!["agents", "status", agent],
        vec!["agents", "show", agent],
        vec!["agents", "diff", agent],
        vec!["agents", "scan"],
    ]
    .iter()
    .map(|args| c.run(args))
    .collect()
}

#[test]
fn indexed_custody_reads_match_cold_reads_and_never_rebuild_the_index() {
    let c = Sandbox::new_seeded("agents-index");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    bind(&c, agent);
    let index = c.0.join(".vivac/index");
    std::fs::remove_file(&index).ok();
    let cold = custody_reads(&c, agent);
    assert!(!index.exists());
    c.run(&["doctor"]);
    assert!(!index.exists());
    c.ok(&["tree"]);
    let fresh = std::fs::read(&index).unwrap();
    assert_eq!(cold, custody_reads(&c, agent));
    assert_eq!(fresh, std::fs::read(&index).unwrap());
    c.ok(&[
        "agents",
        "set",
        agent,
        "--definition",
        &definition("next-model"),
        "--why",
        "Revise the reviewed assignment",
    ]);
    let tail = custody_reads(&c, agent);
    assert_ne!(cold, tail);
    let stale = std::fs::read(&index).unwrap();
    std::fs::remove_file(&index).unwrap();
    assert_eq!(tail, custody_reads(&c, agent));
    assert!(!index.exists());
    std::fs::write(&index, &stale).unwrap();
    assert_eq!(tail, custody_reads(&c, agent));
    assert_eq!(stale, std::fs::read(&index).unwrap());
    for corrupt in [b"corrupt index".to_vec(), fresh[..12].to_vec()] {
        std::fs::write(&index, &corrupt).unwrap();
        assert_eq!(tail, custody_reads(&c, agent));
        c.run(&["doctor"]);
        assert_eq!(corrupt, std::fs::read(&index).unwrap());
    }
}

#[test]
fn custody_reads_reject_missing_configuration_and_invalid_log_tails_without_writes() {
    use std::io::Write;
    let c = Sandbox::new_seeded("agents-index-authority");
    let created = add(&c);
    let agent = created["agent"].as_str().unwrap();
    c.ok(&["tree"]);
    let index = c.0.join(".vivac/index");
    let saved_index = std::fs::read(&index).unwrap();
    let config = c.0.join(".vivac/config");
    let saved_config = std::fs::read(&config).unwrap();
    let log = c.0.join(".vivac/events");
    let saved_log = std::fs::read(&log).unwrap();
    std::fs::remove_file(&config).unwrap();
    assert_eq!(c.run(&["agents", "status", agent]).1, 5);
    assert!(!config.exists());
    assert_eq!(saved_log, std::fs::read(&log).unwrap());
    assert_eq!(saved_index, std::fs::read(&index).unwrap());
    std::fs::write(&config, &saved_config).unwrap();
    for tail in ["not json\n".to_string(), {
        let mut event: Value = serde_json::from_str(c.log().lines().last().unwrap()).unwrap();
        event["payload"]["type"] = json!("agent.future");
        event.to_string() + "\n"
    }] {
        std::fs::write(&log, &saved_log).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(tail.as_bytes())
            .unwrap();
        let invalid = std::fs::read(&log).unwrap();
        assert_eq!(c.run(&["agents", "status", agent]).1, 5);
        assert_eq!(invalid, std::fs::read(&log).unwrap());
        assert_eq!(saved_config, std::fs::read(&config).unwrap());
        assert_eq!(saved_index, std::fs::read(&index).unwrap());
    }
}
