mod common;
use common::Sandbox;

fn pillars(brief: &str) -> Vec<&str> {
    brief
        .lines()
        .skip_while(|line| *line != " PILLARS")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .collect()
}

#[test]
fn every_active_pillar_survives_without_focus_and_with_a_small_budget() {
    let c = Sandbox::new_seeded("brief-all-pillars");
    let titles: Vec<String> = (1..=14)
        .map(|i| format!("Mandate {i}: every decision must preserve this complete title through its final words"))
        .collect();
    for title in &titles {
        c.ok(&["add", title, "--type", "pillar", "--why", "governs"]);
    }
    for focused in [false, true] {
        if focused {
            c.ok(&["push", "Work", "--why", "test focus independence"]);
        }
        let brief = c.ok(&["brief", "--budget", "1"]);
        let rows = pillars(&brief);
        assert_eq!(rows.len(), titles.len(), "{brief}");
        for (i, (row, title)) in rows.iter().zip(&titles).enumerate() {
            assert_eq!(*row, format!("  {:<6} {title}", format!("p{}", i + 1)));
        }
        assert!(brief.contains("Before deciding or reviewing"), "{brief}");
        assert!(brief.contains("vivac rules"), "{brief}");
    }
}

#[test]
fn rules_without_pillars_still_have_fixed_orientation() {
    let c = Sandbox::new_seeded("brief-rules-only");
    c.ok(&[
        "add",
        "Keep the contract",
        "--type",
        "rule",
        "--why",
        "governs",
    ]);
    let brief = c.ok(&["brief", "--budget", "1"]);
    assert!(pillars(&brief).is_empty(), "{brief}");
    assert!(brief.contains("Before deciding or reviewing"), "{brief}");
    assert!(brief.contains("vivac rules"), "{brief}");
    assert!(!brief.contains("No active pillars or rules"), "{brief}");
}

#[test]
fn no_governance_points_to_checking_or_proposing_it_with_the_person() {
    let c = Sandbox::new_seeded("brief-no-governance");
    let brief = c.ok(&["brief", "--budget", "1"]);
    assert!(brief.contains("No active pillars or rules"), "{brief}");
    assert!(brief.contains("vivac rules"), "{brief}");
    assert!(brief.contains("with the person"), "{brief}");
    assert!(brief.contains("Do not invent governance"), "{brief}");
}

#[test]
fn only_active_pillars_appear_in_the_fixed_section() {
    let c = Sandbox::new_seeded("brief-pillar-states");
    for title in ["Keep", "Finished", "Abandoned", "Replaced", "Parked"] {
        c.ok(&["add", title, "--type", "pillar", "--why", "governs"]);
    }
    for (num, state) in [
        (2, "done"),
        (3, "abandoned"),
        (4, "superseded"),
        (5, "suspended"),
    ] {
        let log = c.log();
        let node = log
            .lines()
            .find_map(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                (event["payload"]["num"].as_u64() == Some(num))
                    .then(|| event["payload"]["node"].as_str().unwrap().to_string())
            })
            .unwrap();
        c.append_raw_line(&serde_json::json!({
            "seq": 900 + num, "id": format!("01PILLARSTATE{num:013}"),
            "ts": "2026-09-10T10:00:00Z", "actor": "a_test0000000", "lane": "main",
            "payload": {"type": "state.changed", "node": node, "state": state, "outcome": "fixture"}
        }).to_string());
    }
    let brief = c.ok(&["brief", "--budget", "1"]);
    assert_eq!(pillars(&brief), vec!["  p1     Keep"], "{brief}");
}

#[test]
fn cli_mcp_and_hook_deliver_the_same_pillars() {
    let c = Sandbox::new_seeded("brief-pillar-surfaces");
    for title in ["Keep the veto", "Preserve the complete budget mandate"] {
        c.ok(&["add", title, "--type", "pillar", "--why", "governs"]);
    }
    let cli = c.ok(&["brief"]);
    let (hook, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
    assert_eq!(code, 0, "{hook}");
    assert_eq!(pillars(&cli), pillars(&hook));
    let (mcp, code) = c.run_stdin(&["mcp"], concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"vivac_brief\",\"arguments\":{}}}\n"
    ));
    assert_eq!(code, 0, "{mcp}");
    let reply: serde_json::Value = mcp
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|reply| reply["id"] == 2)
        .unwrap();
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(pillars(&cli), pillars(text));
}

#[test]
fn terminal_width_wraps_the_complete_pillar_title_without_color() {
    let c = Sandbox::new_seeded("brief-pillar-wrap");
    let title = "Every decision must preserve the whole mandate including all these final words";
    c.ok(&["add", title, "--type", "pillar", "--why", "governs"]);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "40")
        .env_remove("TERM")
        .args(["brief", "--budget", "1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let brief = String::from_utf8(output.stdout).unwrap();
    let rows = pillars(&brief);
    assert!(rows.len() > 1, "{brief}");
    assert!(rows.iter().all(|row| row.chars().count() <= 40), "{brief}");
    let complete = rows
        .iter()
        .flat_map(|row| row.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(complete, format!("p1 {title}"));
    assert!(!brief.contains('\x1b'), "{brief}");
}
