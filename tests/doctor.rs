//! Diagnose configuration separately from execution, without repairing anything.
mod common;
use common::Sandbox;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(at).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_string_lossy().into(),
                    std::fs::read(p).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    if !root.exists() {
        return out;
    }
    walk(root, root, &mut out);
    out
}

fn report(c: &Sandbox, harness: Option<&str>, code: i32) -> Value {
    let mut args = vec!["doctor", "--json"];
    if let Some(h) = harness {
        args.push(h);
    }
    let before = snapshot(&c.0);
    let before_home = (c.global_home().exists(), snapshot(c.global_home()));
    let (text, actual) = c.run(&args);
    assert_eq!(actual, code, "{text}");
    assert_eq!(snapshot(&c.0), before, "doctor changed project files");
    assert_eq!(
        (c.global_home().exists(), snapshot(c.global_home())),
        before_home,
        "doctor changed the registry"
    );
    serde_json::from_str(&text).expect(&text)
}

fn check<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("missing {name}: {r}"))
}

#[test]
fn no_tree_reports_a_remedy_without_planting() {
    let c = Sandbox::new_empty("doctor-empty");
    let r = report(&c, None, 1);
    assert_eq!(check(&r, "tree")["status"], "error");
    assert!(check(&r, "tree")["fix"]
        .as_str()
        .unwrap()
        .contains("vivac init"));
}

#[test]
fn agents_added_after_setup_are_reported_without_adoption_or_prompt_disclosure() {
    let c = Sandbox::new_seeded("doctor-unmanaged-agent");
    c.ok(&["setup", "codex", "--yes"]);
    std::fs::create_dir_all(c.0.join(".codex/agents")).unwrap();
    std::fs::write(c.0.join(".codex/agents/reviewer.toml"), "name = 'reviewer'\ndescription = 'Review'\nmodel = 'inherit'\nmodel_reasoning_effort = 'high'\ndeveloper_instructions = 'A private native instruction body.'\n").unwrap();
    let r = report(&c, Some("codex"), 0);
    let custody = check(&r, "agent custody");
    assert_eq!(custody["status"], "warning");
    assert!(custody["detail"].as_str().unwrap().contains("1 unmanaged"));
    assert!(!r.to_string().contains("private native instruction"));
    assert!(!c.log().contains("agent.recorded"));
}

#[test]
fn configuration_is_not_execution_and_both_harnesses_are_detected() {
    let c = Sandbox::new_seeded("doctor-configured");
    for h in ["claude-code", "codex"] {
        c.ok(&["setup", h, "--yes"]);
    }
    let r = report(&c, None, 0);
    for name in [
        "tree",
        "claude-code SessionStart",
        "codex Stop",
        "claude-code MCP",
        "codex MCP",
    ] {
        assert_eq!(check(&r, name)["status"], "ok", "{r}");
    }
    assert_eq!(check(&r, "observed SessionStart")["status"], "warning");
    assert_eq!(check(&r, "observed Stop")["status"], "warning");
    assert_eq!(check(&r, "codex trust")["status"], "warning");
    let selected = report(&c, Some("claude-code"), 0);
    assert!(!selected.to_string().contains("codex"));
    // A nested working folder resolves the same lane's configuration.
    let nested = c.in_folder("src/nested");
    let r = report(&nested, None, 0);
    assert_eq!(check(&r, "claude-code Stop")["status"], "ok");
}

#[test]
fn observed_hooks_are_shared_lane_evidence_and_doctor_never_runs_them() {
    let c = Sandbox::new_seeded("doctor-observed");
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["push", "A goal", "--why", "needed"]);
    let (_, code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"source":"startup","session_id":"s1"}"#,
    );
    assert_eq!(code, 0);
    let (_, code) = c.run_stdin(&["session", "end", "--hook"], "{}");
    assert_eq!(code, 0);
    let r = report(&c, None, 0);
    for name in ["observed SessionStart", "observed Stop"] {
        let v = check(&r, name);
        assert_eq!(v["status"], "ok", "{r}");
        assert!(v["detail"]
            .as_str()
            .unwrap()
            .contains("harness not identified"));
    }
    let other = c.in_folder("other-lane");
    other.ok(&[
        "init",
        "--join",
        c.0.to_str().unwrap(),
        "--lane-name",
        "other",
        "--yes",
    ]);
    other.ok(&["setup", "claude-code", "--yes"]);
    let r = report(&other, None, 0);
    assert_eq!(check(&r, "observed SessionStart")["status"], "warning");
    assert_eq!(check(&r, "observed Stop")["status"], "warning");
}

#[test]
fn local_project_settings_can_disable_the_shared_hooks() {
    let c = Sandbox::new_seeded("doctor-local-disabled");
    c.ok(&["setup", "claude-code", "--yes"]);
    std::fs::write(
        c.0.join(".claude/settings.local.json"),
        r#"{"disableAllHooks":true}"#,
    )
    .unwrap();
    let r = report(&c, None, 1);
    assert_eq!(check(&r, "claude-code local hooks")["status"], "error");
}

#[test]
fn missing_config_is_not_regenerated_and_a_warm_index_cannot_hide_log_damage() {
    let c = Sandbox::new_seeded("doctor-damaged");
    c.ok(&["setup", "claude-code", "--yes"]);
    c.ok(&["stats"]); // Populate the index before damaging the source of truth.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(c.0.join(".vivac/events"))
        .unwrap()
        .write_all(b"not JSON\n")
        .unwrap();
    let r = report(&c, None, 1);
    assert!(check(&r, "tree")["detail"]
        .as_str()
        .unwrap()
        .contains("unreadable event"));
    std::fs::remove_file(c.0.join(".vivac/config")).unwrap();
    let r = report(&c, None, 1);
    assert_eq!(check(&r, ".vivac/config")["status"], "error");
    assert!(!c.0.join(".vivac/config").exists());
}

#[test]
fn malformed_project_files_do_not_echo_their_contents() {
    let c = Sandbox::new_seeded("doctor-private");
    std::fs::create_dir_all(c.0.join(".claude")).unwrap();
    std::fs::write(
        c.0.join(".claude/settings.json"),
        "{private-content-sentinel",
    )
    .unwrap();
    std::fs::write(c.0.join(".mcp.json"), "[]").unwrap();
    let r = report(&c, None, 1);
    assert_eq!(check(&r, ".claude/settings.json")["status"], "error");
    assert!(!r.to_string().contains("private-content-sentinel"));
}

#[test]
fn disabled_and_custom_hooks_are_not_claimed_to_work() {
    let c = Sandbox::new_seeded("doctor-disabled");
    c.ok(&["setup", "claude-code", "--yes"]);
    let path = c.0.join(".claude/settings.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["disableAllHooks"] = true.into();
    v["hooks"]["SessionStart"][0]["hooks"][0]["command"] =
        "vivac session start --hook --private-content-sentinel".into();
    std::fs::write(&path, v.to_string()).unwrap();
    let r = report(&c, None, 1);
    assert_eq!(check(&r, "claude-code")["status"], "error");
    assert_eq!(check(&r, "claude-code SessionStart")["status"], "warning");
    assert!(!r.to_string().contains("private-content-sentinel"));
}

#[test]
fn only_valid_hook_modes_and_event_matchers_are_verified() {
    let c = Sandbox::new_seeded("doctor-hook-modes");
    c.ok(&["setup", "claude-code", "--yes"]);
    let path = c.0.join(".claude/settings.json");
    let original: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
        for invalid in [serde_json::json!(42), serde_json::json!(null)] {
            let mut v = original.clone();
            v["hooks"][event][0]["matcher"] = invalid;
            std::fs::write(&path, v.to_string()).unwrap();
            let r = report(&c, None, 0);
            assert_eq!(
                check(&r, &format!("claude-code {event}"))["status"],
                "warning"
            );
        }
        for invalid in [serde_json::json!("false"), serde_json::json!(true)] {
            let mut v = original.clone();
            v["hooks"][event][0]["hooks"][0]["async"] = invalid;
            std::fs::write(&path, v.to_string()).unwrap();
            let r = report(&c, None, 0);
            assert_eq!(
                check(&r, &format!("claude-code {event}"))["status"],
                "warning"
            );
        }
        if event != "SessionStart" {
            let mut v = original.clone();
            v["hooks"][event][0]["matcher"] = "startup|resume|clear|compact".into();
            std::fs::write(&path, v.to_string()).unwrap();
            let r = report(&c, None, 0);
            assert_eq!(
                check(&r, &format!("claude-code {event}"))["status"],
                "warning"
            );
        }
    }
}

#[test]
fn codex_markers_alone_do_not_verify_a_changed_server() {
    let c = Sandbox::new_seeded("doctor-codex-edit");
    c.ok(&["setup", "codex", "--yes"]);
    let path = c.0.join(".codex/config.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("command = \"vivac\"", "command = \"another-program\"");
    std::fs::write(path, text).unwrap();
    let r = report(&c, None, 0);
    assert_eq!(check(&r, "codex MCP")["status"], "warning");
    assert!(!r.to_string().contains("another-program"));
}

#[test]
fn arguments_are_validated_before_reading_the_tree() {
    let c = Sandbox::new_empty("doctor-args");
    for args in [
        &["doctor", "unknown"][..],
        &["doctor", "codex", "extra"],
        &["doctor", "--force"],
    ] {
        let (_, code) = c.run(args);
        assert_eq!(code, 2);
    }
}
