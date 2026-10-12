mod common;

use common::Sandbox;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");
const OPERATION: &str = "01cccccccccccccccccccccccc";

struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start(sandbox: &Sandbox) -> Self {
        let mut child = Command::new(BIN)
            .current_dir(&sandbox.0)
            .env("VIVAC_HOME", sandbox.global_home())
            .env("TZ", "UTC")
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            next: 1,
        };
        let initialized = server.ask("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"artifact-tests","version":"1"}}));
        assert!(initialized["result"]["capabilities"]["tools"].is_object());
        writeln!(
            server.input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        server.input.flush().unwrap();
        server
    }

    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        assert_ne!(
            self.output.read_line(&mut line).unwrap(),
            0,
            "server closed without a reply"
        );
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        assert!(reply.get("error").is_none(), "{reply}");
        reply
    }

    fn artifacts(&mut self, arguments: Value) -> Value {
        let reply = self.ask(
            "tools/call",
            json!({"name":"vivac_artifacts","arguments":arguments}),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_isolated(root: &std::path::Path) {
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let workspace = crate_root
        .parent()
        .filter(|parent| parent.parent().is_some())
        .unwrap_or(&crate_root);
    assert!(
        !root.starts_with(workspace),
        "artifact fixtures must be outside the workspace"
    );
    assert!(
        !root
            .ancestors()
            .skip(1)
            .any(|parent| ["config", "events", "lane"]
                .into_iter()
                .any(|name| parent.join(".vivac").join(name).exists())),
        "artifact fixtures must not inherit a tree"
    );
}

fn seeded(name: &str) -> Sandbox {
    let sandbox = Sandbox::new_empty(name);
    let root = sandbox.0.canonicalize().unwrap();
    assert_isolated(&root);
    sandbox.ok(&["init", "--yes"]);
    sandbox.ok(&[
        "push",
        "Retain the selected design",
        "--why",
        "review exact local deliveries",
    ]);
    sandbox
}

fn cli(sandbox: &Sandbox, args: &[&str]) -> Value {
    serde_json::from_str(&sandbox.ok(args)).unwrap()
}

#[test]
fn artifact_tool_discovery_declares_the_selected_operations() {
    let sandbox = seeded("artifact-mcp-schema");
    let mut server = Server::start(&sandbox);
    let reply = server.ask("tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "vivac_artifacts")
        .expect("artifact tool is discoverable");
    let properties = &tool["inputSchema"]["properties"];
    let operations = properties["operation"]["enum"].as_array().unwrap();
    for operation in ["capture", "list", "show", "check", "restore"] {
        assert!(operations.contains(&json!(operation)), "{tool}");
    }
    for name in [
        "artifact",
        "path",
        "from",
        "revision",
        "to",
        "operation_id",
        "limit",
        "offset",
    ] {
        assert!(properties.get(name).is_some(), "missing {name}: {tool}");
    }
}

#[test]
fn mcp_and_cli_share_revision_identity_retention_and_restore_behavior() {
    let sandbox = seeded("artifact-mcp-revisions");
    let first = b"<html>First selected layout</html>\n";
    fs::write(sandbox.0.join("design.html"), first).unwrap();
    let mut server = Server::start(&sandbox);
    let captured = server.artifacts(
        json!({"operation":"capture","path":"design.html","from":"g1","operation_id":OPERATION}),
    );
    assert_eq!(captured["exit_code"], 0, "{captured}");
    let delivery = &captured["result"]["delivery"];
    let artifact = delivery["artifact"].as_str().unwrap();
    let revision = delivery["revision"].as_str().unwrap();
    let listed = server.artifacts(json!({"operation":"list","all":true,"limit":1,"offset":0}));
    assert_eq!(listed["exit_code"], 0);
    assert_eq!(
        listed["result"],
        cli(
            &sandbox,
            &[
                "artifacts",
                "list",
                "--all",
                "--limit",
                "1",
                "--offset",
                "0",
                "--json"
            ]
        )
    );
    let shown =
        server.artifacts(json!({"operation":"show","artifact":artifact,"revision":revision}));
    assert_eq!(
        shown["result"],
        cli(
            &sandbox,
            &[
                "artifacts",
                "show",
                artifact,
                "--revision",
                revision,
                "--json"
            ]
        )
    );
    fs::write(sandbox.0.join("design.html"), b"Second selected layout\n").unwrap();
    let next = cli(
        &sandbox,
        &[
            "artifacts",
            "capture",
            "design.html",
            "--artifact",
            artifact,
            "--from",
            "g1",
            "--json",
        ],
    );
    assert_ne!(next["delivery"]["revision"], revision);
    let log = sandbox.log();
    let retried = server.artifacts(
        json!({"operation":"capture","path":"design.html","from":"g1","operation_id":OPERATION}),
    );
    assert_eq!(retried["result"]["already_recorded"], true);
    assert_eq!(retried["result"]["delivery"], *delivery);
    assert_eq!(sandbox.log(), log);
    fs::remove_file(sandbox.0.join("design.html")).unwrap();
    let checked =
        server.artifacts(json!({"operation":"check","artifact":artifact,"revision":revision}));
    assert_eq!(checked["exit_code"], 1);
    assert_eq!(checked["result"]["source"]["status"], "missing");
    assert_eq!(checked["result"]["retained"]["status"], "matches");
    assert_eq!(checked["result"]["approval"], "not_inferred");
    let restored = server.artifacts(json!({"operation":"restore","artifact":artifact,"revision":revision,"to":"recovered.html"}));
    assert_eq!(restored["exit_code"], 0);
    assert_eq!(fs::read(sandbox.0.join("recovered.html")).unwrap(), first);
    let occupied =
        server.artifacts(json!({"operation":"restore","artifact":artifact,"to":"recovered.html"}));
    assert_ne!(occupied["exit_code"], 0);
    assert_eq!(fs::read(sandbox.0.join("recovered.html")).unwrap(), first);
    assert_eq!(sandbox.log(), log);
}

#[test]
fn mcp_capture_refuses_sensitive_body_without_protocol_or_log_leaks() {
    let sandbox = seeded("artifact-mcp-private");
    let log = sandbox.log();
    fs::write(
        sandbox.0.join("unsafe.html"),
        b"Private fixture marker person@example.com",
    )
    .unwrap();
    let mut server = Server::start(&sandbox);
    let refused = server.artifacts(json!({"operation":"capture","path":"unsafe.html","from":"g1"}));
    assert_eq!(refused["exit_code"], 3);
    let reply = refused.to_string();
    assert!(!reply.contains("Private fixture marker"));
    assert!(!reply.contains("person@example.com"));
    assert_eq!(sandbox.log(), log);
    let listed = server.artifacts(json!({"operation":"list","all":true}));
    assert_eq!(listed["exit_code"], 0);
    assert!(listed["result"]["artifacts"].as_array().unwrap().is_empty());
}

#[test]
fn mcp_retry_rejects_a_different_explicit_previous_revision_without_reading_source() {
    let sandbox = seeded("artifact-mcp-previous");
    fs::write(sandbox.0.join("design.html"), b"Selected layout\n").unwrap();
    let mut server = Server::start(&sandbox);
    let first = server.artifacts(json!({"operation":"capture", "path":"design.html", "from":"g1"}));
    let artifact = first["result"]["delivery"]["artifact"].as_str().unwrap();
    let previous = first["result"]["delivery"]["revision"].as_str().unwrap();
    let args = json!({"operation":"capture", "path":"design.html", "from":"g1",
        "artifact":artifact, "previous":previous, "operation_id":OPERATION});
    let recorded = server.artifacts(args.clone());
    assert_eq!(recorded["exit_code"], 0);
    fs::remove_file(sandbox.0.join("design.html")).unwrap();
    let log = sandbox.log();
    let retried = server.artifacts(args);
    assert_eq!(retried["exit_code"], 0);
    assert_eq!(
        retried["result"]["delivery"],
        recorded["result"]["delivery"]
    );
    let wrong = server.artifacts(
        json!({"operation":"capture", "path":"design.html", "from":"g1",
        "artifact":artifact, "previous":"01aaaaaaaaaaaaaaaaaaaaaaaa", "operation_id":OPERATION}),
    );
    assert_ne!(wrong["exit_code"], 0);
    assert_eq!(sandbox.log(), log);
}
