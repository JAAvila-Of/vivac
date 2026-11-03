mod common;

use common::Sandbox;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

fn measure(text: &str) -> Value {
    let chars = text.chars().count();
    json!({"utf8_bytes": text.len(), "unicode_chars": chars,
        "estimated_tokens_chars_div_4": chars.div_ceil(4)})
}

fn record(report: &mut BTreeMap<String, Value>, key: String, text: &str) {
    assert!(report.insert(key, measure(text)).is_none());
}

fn descriptions(value: &Value, output: &mut String) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "description" && value.is_string() {
                    output.push_str(value.as_str().expect("a schema description is text"));
                } else {
                    descriptions(value, output);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                descriptions(value, output);
            }
        }
        _ => {}
    }
}

fn remove_descriptions(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, value| key != "description" || !value.is_string());
            for value in fields.values_mut() {
                remove_descriptions(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_descriptions(value);
            }
        }
        _ => {}
    }
}

#[test]
fn instruction_changes_preserve_the_mcp_contract() {
    let c = Sandbox::new_seeded("instruction-contract");
    let tools = tool_list(&c);
    let mut hashes = BTreeMap::new();
    for tool in tools.as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let mut contract = json!({
            "name": tool["name"], "title": tool["title"],
            "inputSchema": tool["inputSchema"], "annotations": tool["annotations"]
        });
        remove_descriptions(&mut contract);
        let bytes = serde_json::to_vec(&contract).unwrap();
        assert!(hashes
            .insert(name.to_string(), format!("{:x}", Sha256::digest(bytes)))
            .is_none());
    }
    let budgets: Value =
        serde_json::from_str(include_str!("data/instruction-budget.json")).unwrap();
    assert_eq!(
        serde_json::to_value(hashes).unwrap(),
        budgets["mcp_contract_sha256"]
    );
}

fn tool_list(c: &Sandbox) -> Value {
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"budget","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    let mut child = Command::new(BIN)
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("CODEX_HOME", c.0.join(".codex"))
        .env("TZ", "UTC")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let replies: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    replies
        .into_iter()
        .find(|reply| reply["id"] == 2)
        .expect("tools/list answered")["result"]["tools"]
        .clone()
}

fn protocol(text: &str, heading: &str) -> String {
    let marker = format!(" {heading}\n");
    let (_, rest) = text.split_once(&marker).expect("fixed protocol heading");
    let body = rest.split("\n\n").next().unwrap();
    format!("{marker}{body}\n")
}

fn skill(report: &mut BTreeMap<String, Value>, name: &str, text: &str) {
    let end = text
        .strip_prefix("---\n")
        .expect("skill frontmatter")
        .find("\n---\n")
        .unwrap()
        + 9;
    let (frontmatter, body) = text.split_at(end);
    record(report, format!("skills.{name}.frontmatter"), frontmatter);
    record(report, format!("skills.{name}.body"), body);
    record(report, format!("skills.{name}.complete"), text);
    let description = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap();
    record(report, format!("catalog.{name}.description"), description);
    // The fingerprint changes with content; its fixed width does not.
    record(report, format!("skills.{name}.installed_marker"),
        "<!-- written by vivac setup; fingerprint 0000000000000000; setup removes it with --undo while the text is unchanged -->\n");
}

#[test]
fn owned_model_instructions_stay_within_reviewed_budgets() {
    let mut report = BTreeMap::new();
    let c = Sandbox::new_seeded("instruction-budget");
    let tools = tool_list(&c);
    for tool in tools.as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        record(
            &mut report,
            format!("mcp.{name}.description"),
            tool["description"].as_str().unwrap(),
        );
        let schema = &tool["inputSchema"];
        let mut arguments = String::new();
        descriptions(schema, &mut arguments);
        record(
            &mut report,
            format!("mcp.{name}.argument_descriptions"),
            &arguments,
        );
        // Compact canonical JSON includes all schema keys and values.
        record(
            &mut report,
            format!("mcp.{name}.schema_json"),
            &serde_json::to_string(schema).unwrap(),
        );
    }
    for focus in [false, true] {
        if focus {
            c.ok(&["push", "A goal", "--why", "A fixture needs focus"]);
        }
        let (text, code) = c.run_stdin(&["session", "start", "--hook"], "{}");
        assert_eq!(code, 0, "{text}");
        for heading in [
            "ARTIFACT DELIVERIES",
            "AGENT CUSTODY",
            "WRITE AT THESE SEAMS",
        ] {
            record(
                &mut report,
                format!(
                    "hook.{}.{}",
                    if focus { "focus" } else { "no_focus" },
                    heading.to_lowercase().replace(' ', "_")
                ),
                &protocol(&text, heading),
            );
        }
    }
    skill(
        &mut report,
        "vivac-migrate",
        &format!(
            "{}{}",
            include_str!("../src/setup/skill-frontmatter.md"),
            include_str!("../src/setup/skill-body.md")
        ),
    );
    skill(
        &mut report,
        "vivac-agents",
        include_str!("../src/setup/agents-skill.md"),
    );
    skill(
        &mut report,
        "vivac-artifacts",
        include_str!("../src/setup/artifacts-skill.md"),
    );
    println!(
        "instruction_budget={}",
        serde_json::to_string(&report).unwrap()
    );
    let budgets: Value =
        serde_json::from_str(include_str!("data/instruction-budget.json")).unwrap();
    let limits = budgets["ceilings"].as_object().unwrap();
    assert_eq!(
        report.len(),
        limits.len(),
        "review added or removed surfaces"
    );
    for (surface, actual) in report {
        let ceiling = limits
            .get(&surface)
            .unwrap_or_else(|| panic!("unreviewed surface: {surface}"));
        for key in [
            "utf8_bytes",
            "unicode_chars",
            "estimated_tokens_chars_div_4",
        ] {
            assert!(
                actual[key].as_u64().unwrap() <= ceiling[key].as_u64().unwrap(),
                "{surface}.{key}: {} exceeds {}",
                actual[key],
                ceiling[key]
            );
        }
    }
}
