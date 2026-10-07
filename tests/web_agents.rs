mod common;
use common::Sandbox;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

struct Server {
    child: Child,
    port: u16,
    cookie: String,
    path: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn start(c: &Sandbox) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vivac"))
            .current_dir(&c.0)
            .env("VIVAC_HOME", c.global_home())
            .args(["web", "--port", "0", "--no-open"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let boot = loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.contains("?k=") {
                break line[line.find("http://").unwrap()..].trim().to_string();
            }
        };
        let authority = boot.trim_start_matches("http://");
        let (host, path) = authority.split_once('/').unwrap();
        let port = host.rsplit(':').next().unwrap().parse().unwrap();
        let mut server = Self {
            child,
            port,
            cookie: String::new(),
            path: String::new(),
        };
        let response = server.call("GET", &format!("/{path}"), "", "");
        server.cookie = response
            .lines()
            .find_map(|line| {
                line.strip_prefix("Set-Cookie: ")
                    .or_else(|| line.strip_prefix("set-cookie: "))
            })
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        server.path = response
            .lines()
            .find_map(|line| {
                line.strip_prefix("Location: ")
                    .or_else(|| line.strip_prefix("location: "))
            })
            .unwrap()
            .trim()
            .to_string();
        server
    }
    fn call(&self, method: &str, path: &str, headers: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(stream,"{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: {}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",self.port,self.cookie,body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
    fn headers(&self) -> String {
        let token = self.cookie.split_once('=').unwrap().1;
        format!("Content-Type: application/json\r\nOrigin: http://127.0.0.1:{}\r\nX-Vivac-Token: {token}\r\n", self.port)
    }
    fn agents_path(&self, operation: &str) -> String {
        format!("{}agents/{operation}", self.path)
    }
    fn post(&self, operation: &str, body: &Value, status: u16) -> Value {
        let response = self.call(
            "POST",
            &self.agents_path(operation),
            &self.headers(),
            &body.to_string(),
        );
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
        json_body(&response)
    }
}

fn json_body(response: &str) -> Value {
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn cli(c: &Sandbox, args: &[&str]) -> Value {
    let (body, code) = c.run(args);
    assert_eq!(code, 0, "{body}");
    serde_json::from_str(&body).unwrap()
}

const PROMPT: &str = "Inspect the entire change.\n</script><img src=x onerror=alert('native-agent')>\n```text\nPreserve literal examples.\n```\n";

fn selected(c: &Sandbox) -> Value {
    std::fs::create_dir_all(c.0.join(".claude/agents")).unwrap();
    std::fs::write(c.0.join(".claude/agents/reviewer.md"), format!("---\nname: reviewer\ndescription: Review scoped changes.\nmodel: sonnet\neffort: high\n---\n{PROMPT}")).unwrap();
    let (output, code) = c.run(&["setup", "codex", "--yes"]);
    assert_eq!(code, 0, "{output}");
    let inventory = cli(c, &["agents", "inventory"]);
    let source = &inventory["unmanaged"][0];
    json!({"why":"Synchronize the reviewed configuration","items":[{"agent":null,
        "source":{"harness":source["harness"],"path":source["path"],"digest":source["digest"]},
        "destinations":[{"assignment":{"harness":"codex","name":"reviewer","model":"inherit","effort":"high","settings":{"sandbox_mode":"read-only"}},
            "path":".codex/agents/reviewer.toml","digest":null}]}]})
}

#[test]
fn agents_page_inventory_and_post_guards_share_the_backend() {
    let c = Sandbox::new_seeded("web-agents");
    let server = Server::start(&c);
    let path = format!("{}agents", server.path);
    let page = server.call("GET", &path, "", "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("connect-src 'self'"));
    assert!(page.contains("Compare configurations"));
    let inventory = server.call("GET", &format!("{path}/inventory"), "", "");
    assert!(inventory.starts_with("HTTP/1.1 200"));
    assert!(inventory.contains("\"harnesses\""));
    let before = c.log();
    for method in ["GET", "OPTIONS"] {
        assert!(server
            .call(method, &format!("{path}/apply"), "", "")
            .starts_with("HTTP/1.1 405"));
    }
    assert!(server
        .call(
            "POST",
            &format!("{path}/apply"),
            "Content-Type: application/json\r\n",
            "{}"
        )
        .starts_with("HTTP/1.1 403"));
    let token = server.cookie.split_once('=').unwrap().1;
    let headers=format!("Content-Type: application/json\r\nOrigin: http://127.0.0.1:{}\r\nX-Vivac-Token: {token}\r\n",server.port);
    assert!(server
        .call(
            "POST",
            &format!("{path}/plan"),
            &headers,
            "{\"why\":\"Review\",\"items\":[],\"unknown\":true}"
        )
        .starts_with("HTTP/1.1 400"));
    let cross = headers.replace(
        &format!("Origin: http://127.0.0.1:{}", server.port),
        &format!("Origin: http://localhost:{}", server.port),
    );
    assert!(server
        .call("POST", &format!("{path}/plan"), &cross, "{}")
        .starts_with("HTTP/1.1 403"));
    assert_eq!(before, c.log());
}

#[test]
fn http_review_matches_cli_and_transfers_literal_prompt_without_rendering_it_in_pages() {
    let c = Sandbox::new_seeded("web-agents-transfer");
    let selection = selected(&c);
    let server = Server::start(&c);
    let before = c.log();
    let page = server.call("GET", &format!("{}agents", server.path), "", "");
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(!page.contains(PROMPT));
    assert!(!page.contains("onerror=alert('native-agent')"));
    let response = server.call("GET", &server.agents_path("inventory"), "", "");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_eq!(json_body(&response), cli(&c, &["agents", "inventory"]));
    assert!(!response.contains("native-agent"));
    let compared = json!({"references":[selection["items"][0]["source"].clone()]});
    let comparison = server.post("compare", &compared, 200);
    assert_eq!(
        comparison,
        cli(
            &c,
            &["agents", "compare", "--selection", &compared.to_string()]
        )
    );
    assert_eq!(comparison["sources"][0]["body"], PROMPT);
    let comparison_response = server.call(
        "POST",
        &server.agents_path("compare"),
        &server.headers(),
        &compared.to_string(),
    );
    assert!(comparison_response
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    assert!(comparison_response
        .to_ascii_lowercase()
        .contains("x-content-type-options: nosniff"));
    let plan = server.post("plan", &selection, 200);
    assert_eq!(
        plan,
        cli(
            &c,
            &["agents", "plan", "--selection", &selection.to_string()]
        )
    );
    assert!(!plan.to_string().contains("native-agent"));
    assert_eq!(
        before,
        c.log(),
        "Page, inventory, comparison and planning must not append events"
    );
    let applied = server.post(
        "apply",
        &json!({"selection":selection,"plan_digest":plan["plan_digest"]}),
        200,
    );
    assert_eq!(applied["exit_code"], 0);
    assert_eq!(applied["result"]["applied"], true);
    let document: toml::Table =
        toml::from_str(&std::fs::read_to_string(c.0.join(".codex/agents/reviewer.toml")).unwrap())
            .unwrap();
    assert_eq!(document["developer_instructions"].as_str(), Some(PROMPT));
    assert_eq!(document["sandbox_mode"].as_str(), Some("read-only"));
    assert!(!c.log().contains("native-agent"));
    assert!(!c.log().contains("Preserve literal examples."));
}

#[test]
fn stale_http_plan_refuses_new_destination_without_recording_or_overwriting_it() {
    let c = Sandbox::new_seeded("web-agents-stale");
    let selection = selected(&c);
    let server = Server::start(&c);
    let plan = server.post("plan", &selection, 200);
    let before = c.log();
    let destination = c.0.join(".codex/agents/reviewer.toml");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, "An unreviewed native configuration.").unwrap();
    server.post(
        "apply",
        &json!({"selection":selection,"plan_digest":plan["plan_digest"]}),
        409,
    );
    assert_eq!(before, c.log());
    assert_eq!(
        std::fs::read_to_string(destination).unwrap(),
        "An unreviewed native configuration."
    );
}

#[test]
fn post_guards_reject_missing_or_wrong_review_token_origin_type_and_oversized_body() {
    let c = Sandbox::new_seeded("web-agents-post-guards");
    let selection = selected(&c);
    let server = Server::start(&c);
    let before = c.log();
    let path = server.agents_path("plan");
    let token = server.cookie.split_once('=').unwrap().1;
    let origin = format!("Origin: http://127.0.0.1:{}\r\n", server.port);
    for (headers, status) in [
        (format!("Content-Type: application/json\r\n{origin}"), 403),
        (format!("Content-Type: application/json\r\n{origin}X-Vivac-Token: wrong-token\r\n"), 401),
        (format!("Content-Type: application/json\r\nX-Vivac-Token: {token}\r\n"), 403),
        (format!("Content-Type: application/json\r\nOrigin: https://example.invalid\r\nX-Vivac-Token: {token}\r\n"), 403),
        (format!("Content-Type: application/json\r\nOrigin: http://127.0.0.1:{}\r\nX-Vivac-Token: {token}\r\n", server.port.wrapping_add(1)), 403),
    ] {
        let response = server.call("POST", &path, &headers, &selection.to_string());
        assert!(response.starts_with(&format!("HTTP/1.1 {status}")), "{response}");
    }
    let wrong_type = server.headers().replace("application/json", "text/plain");
    assert!(server
        .call("POST", &path, &wrong_type, &selection.to_string())
        .starts_with("HTTP/1.1 415"));
    let response = server.call(
        "POST",
        &path,
        &server.headers(),
        &" ".repeat(128 * 1024 + 1),
    );
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert_eq!(before, c.log());
    assert!(!c.0.join(".codex/agents/reviewer.toml").exists());
}
