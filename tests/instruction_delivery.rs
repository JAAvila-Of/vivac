//! Cross-event acceptance for emitted instruction context, not harness reception.

mod common;

use common::Sandbox;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const OPENING_SOURCES: [&str; 4] = ["startup", "resume", "clear", "compact"];
const START_COMMAND: &str = "vivac session start --hook";

fn run(sandbox: &Sandbox, cwd: &Path, args: &[&str], input: Option<&Value>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(cwd)
        .env("VIVAC_HOME", sandbox.global_home())
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "vivac {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn configured_opening(sandbox: &Sandbox, opening_dir: &Path, harness: &str) {
    run(sandbox, opening_dir, &["setup", harness, "--yes"], None);
    let file = match harness {
        "claude-code" => ".claude/settings.json",
        "codex" => ".codex/hooks.json",
        _ => unreachable!(),
    };
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(opening_dir.join(file)).unwrap()).unwrap();
    if harness == "claude-code" {
        assert!(
            settings["hooks"].get("SessionStart").is_none(),
            "{settings}"
        );
        let guards: Vec<_> = settings["hooks"]["UserPromptSubmit"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["hooks"].as_array().unwrap())
            .filter(|handler| handler["command"] == "vivac session guard --hook")
            .collect();
        assert_eq!(guards.len(), 1, "{settings}");
        assert_eq!(guards[0]["type"], "command");
        let bundle = opening_dir.join(".claude/skills/vivac-opening");
        let manifest = std::fs::read_to_string(bundle.join(".claude-plugin/plugin.json")).unwrap();
        assert_eq!(
            manifest,
            include_str!("../src/setup/claude-opening/.claude-plugin/plugin.json")
        );
        let hooks_text = std::fs::read_to_string(bundle.join("hooks/hooks.json")).unwrap();
        assert_eq!(
            hooks_text,
            include_str!("../src/setup/claude-opening/hooks/hooks.json")
        );
        let hooks: Value = serde_json::from_str(&hooks_text).unwrap();
        assert_eq!(hooks["modules"], json!(["./register.mjs"]));
        let module = std::fs::read_to_string(bundle.join("hooks/register.mjs")).unwrap();
        assert_eq!(
            module,
            include_str!("../src/setup/claude-opening/hooks/register.mjs")
        );
        assert!(module
            .contains("['vivac', 'session', 'start', '--hook', '--context', 'claude-module']"));
        let receipt: Value = serde_json::from_str(
            &std::fs::read_to_string(bundle.join(".vivac-owned.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["schema"], 1);
        assert_eq!(receipt["version"], "opening-reference-v1");
        assert_eq!(receipt["files"].as_object().unwrap().len(), 3);
        return;
    }
    let openings = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(openings.len(), 1, "{settings}");
    let opening = &openings[0];
    if harness == "codex" {
        assert_eq!(opening["matcher"], OPENING_SOURCES.join("|"), "{settings}");
    } else {
        assert!(opening.get("matcher").is_none(), "{settings}");
    }
    let handlers = opening["hooks"].as_array().unwrap();
    assert_eq!(handlers.len(), 1, "{settings}");
    assert_eq!(handlers[0]["type"], "command");
    let expected = if harness == "codex" {
        assert_eq!(handlers[0]["additionalContextLimit"], 0);
        assert_eq!(handlers[0]["async"], false);
        "vivac session start --hook --context codex"
    } else {
        START_COMMAND
    };
    assert_eq!(handlers[0]["command"], expected);
}

fn opening_events(sandbox: &Sandbox) -> Vec<Value> {
    sandbox
        .log()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["payload"]["type"] == "session.started")
        .collect()
}

fn acceptance_matrix(harness: &str, nested: bool) {
    let sandbox = Sandbox::new_seeded("instruction-delivery");
    let alternate = Sandbox::new_seeded("alternate-delivery-tree");
    alternate.ok(&[
        "add",
        "Alternate tree mandate must not enter the emitted context",
        "--root",
        "--type",
        "pillar",
        "--why",
        "Only the process working directory chooses the tree",
    ]);
    let alternate_before = alternate.log();
    let cwd = if nested {
        let repository = sandbox.0.join("nested-repository");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        let subdirectory = repository.join("src").join("component");
        std::fs::create_dir_all(&subdirectory).unwrap();
        run(&sandbox, &subdirectory, &["init", "--yes"], None);
        assert!(!repository.join(".vivac").exists());
        let anchor = subdirectory.join(".vivac");
        assert!(anchor.join("lane").is_file());
        assert!(!anchor.join("events").exists());
        assert!(!anchor.join("config").exists());
        let lane: Value =
            serde_json::from_str(&std::fs::read_to_string(anchor.join("lane")).unwrap()).unwrap();
        let first_event: Value =
            serde_json::from_str(sandbox.log().lines().next().unwrap()).unwrap();
        assert_eq!(lane["project"], first_event["id"]);
        subdirectory
    } else {
        sandbox.0.clone()
    };
    if harness != "claude-module" {
        configured_opening(&sandbox, &cwd, harness);
    }

    let mut sentinels = Vec::new();
    for source in OPENING_SOURCES {
        let pillar_title = format!(
            "Delivery {source} pillar mandate remains complete even beyond the ordinary abbreviated title width"
        );
        let constraint_title = format!(
            "Delivery {source} invariant remains complete even beyond the ordinary abbreviated title width"
        );
        let pillar_body = format!("Full pillar instruction body for delivery {source}");
        let constraint_body = format!("Full invariant instruction body for delivery {source}");
        for (kind, title, body) in [
            ("pillar", &pillar_title, &pillar_body),
            ("constraint", &constraint_title, &constraint_body),
        ] {
            run(
                &sandbox,
                &sandbox.0,
                &["add", title, "--root", "--type", kind, "--why", body],
                None,
            );
        }
        sentinels.push((pillar_title, constraint_title, pillar_body, constraint_body));
        let session = format!("delivery-{harness}-{source}-{nested}");
        let input = json!({
            "hook_event_name": "SessionStart",
            "source": source,
            "session_id": session,
            "cwd": alternate.0,
        });
        // The generated command is checked above; execute reviewed argv directly.
        // This exercises emission without pretending to launch either harness.
        let args: &[&str] = if harness == "codex" {
            &["session", "start", "--hook", "--context", "codex"]
        } else if matches!(harness, "claude-code" | "claude-module") {
            &["session", "start", "--hook", "--context", "claude-module"]
        } else {
            &["session", "start", "--hook"]
        };
        let output = run(&sandbox, &cwd, args, Some(&input));
        let context = if harness == "codex" {
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                response["hookSpecificOutput"]["hookEventName"],
                "SessionStart"
            );
            response["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .to_string()
        } else if matches!(harness, "claude-code" | "claude-module") {
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(response["protocol"], "vivac.claude.opening.v1");
            assert_eq!(response["status"], "ready");
            response["context"].as_str().unwrap().to_string()
        } else {
            String::from_utf8(output.stdout).unwrap()
        };
        assert!(context.starts_with("vivac"), "{context}");
        for required in [
            "PILLARS",
            "INVARIANTS",
            "Before deciding or reviewing, run vivac rules for full bodies and rules.",
            "AGENT CUSTODY",
            "WRITE AT THESE SEAMS",
        ] {
            assert!(context.contains(required), "missing {required}: {context}");
        }
        let rules_output = run(&sandbox, &cwd, &["rules", "--json"], None);
        let rules: Value = serde_json::from_slice(&rules_output.stdout).unwrap();
        for (pillar_title, constraint_title, pillar_body, constraint_body) in &sentinels {
            for title in [pillar_title, constraint_title] {
                assert!(context.contains(title), "missing complete title: {context}");
            }
            for (section, title, body) in [
                ("pillars", pillar_title, pillar_body),
                ("invariants", constraint_title, constraint_body),
            ] {
                assert!(
                    !context.contains(body),
                    "body unexpectedly injected: {context}"
                );
                let node = rules[section]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|node| node["title"] == *title)
                    .unwrap_or_else(|| panic!("missing {title} in rules: {rules}"));
                assert_eq!(node["why"], *body, "{rules}");
            }
        }
        assert!(!context.contains("Alternate tree mandate"), "{context}");
        let openings = opening_events(&sandbox);
        assert_eq!(openings.len(), sentinels.len(), "{openings:?}");
        let last = &openings.last().unwrap()["payload"];
        assert_eq!(last["source"], source);
        assert_eq!(last["session"], session);
        assert_eq!(
            alternate_before,
            alternate.log(),
            "payload cwd selected another tree"
        );
    }
}

#[test]
fn claude_code_openings_emit_fresh_governance_from_workspace_root() {
    acceptance_matrix("claude-code", false);
}

#[test]
fn claude_code_openings_emit_fresh_governance_beyond_nested_git_root() {
    acceptance_matrix("claude-code", true);
}

#[test]
fn codex_openings_emit_fresh_governance_from_workspace_root() {
    acceptance_matrix("codex", false);
}

#[test]
fn codex_openings_emit_fresh_governance_beyond_nested_git_root() {
    acceptance_matrix("codex", true);
}

#[test]
fn claude_module_openings_emit_fresh_governance_from_workspace_root() {
    acceptance_matrix("claude-module", false);
}

#[test]
fn claude_module_openings_emit_fresh_governance_beyond_nested_git_root() {
    acceptance_matrix("claude-module", true);
}
