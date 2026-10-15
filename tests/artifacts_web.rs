mod common;
#[path = "common/http.rs"]
mod http;
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
        read_response(&mut BufReader::new(stream))
    }
    fn headers(&self) -> String {
        let token = self.cookie.split_once('=').unwrap().1;
        format!("Content-Type: application/json\r\nOrigin: http://127.0.0.1:{}\r\nX-Vivac-Token: {token}\r\n", self.port)
    }
    fn artifacts_path(&self, operation: &str) -> String {
        format!("{}artifacts/{operation}", self.path)
    }
    fn post(&self, operation: &str, body: &Value, status: u16) -> Value {
        let response = self.call(
            "POST",
            &self.artifacts_path(operation),
            &self.headers(),
            &body.to_string(),
        );
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
        assert!(response
            .to_ascii_lowercase()
            .contains("referrer-policy: no-referrer\r\n"));
        json_body(&response)
    }
}

fn read_response(reader: &mut impl BufRead) -> String {
    let mut response = String::new();
    let mut status = String::new();
    assert!(reader.read_line(&mut status).unwrap() > 0);
    assert!(status.is_ascii());
    response.push_str(&status);
    let mut content_length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        assert!(line.is_ascii());
        response.push_str(&line);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed.split_once(':').unwrap();
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = Some(value.trim().parse().unwrap());
        }
        if name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
        {
            chunked = true;
        }
    }
    let body = if chunked || content_length.is_some() {
        http::read_body(reader, content_length.unwrap_or(0), chunked)
    } else {
        let mut body = Vec::new();
        reader.read_to_end(&mut body).unwrap();
        body
    };
    response.push_str(std::str::from_utf8(&body).unwrap());
    response
}

#[test]
fn responses_decode_http_framing_before_multibyte_utf8() {
    for (headers, wire_body) in [
        (
            "Transfer-Encoding: chunked\r\n",
            &b"1\r\nA\r\n1\r\n\xce\r\n1;part=end\r\n\xb1\r\n0\r\n\r\n"[..],
        ),
        ("Content-Length: 3\r\n", &b"A\xce\xb1"[..]),
        ("", &b"A\xce\xb1"[..]),
    ] {
        let head = format!("HTTP/1.1 200 OK\r\n{headers}X-Vivac-Test: framing\r\n\r\n");
        let mut wire = head.as_bytes().to_vec();
        wire.extend_from_slice(wire_body);
        let decoded = read_response(&mut BufReader::new(std::io::Cursor::new(wire)));
        assert_eq!(decoded, format!("{head}Aα"));
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
        "Review the selected delivery",
        "--why",
        "retain the reviewed revision",
    ]);
    sandbox
}

#[test]
fn page_operations_and_download_refuse_unreviewed_requests() {
    let c = seeded("artifacts-web-gates");
    let server = Server::start(&c);
    let path = format!("{}artifacts", server.path);
    let page = server.call("GET", &path, "", "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page
        .to_ascii_lowercase()
        .contains("referrer-policy: same-origin\r\n"));
    assert!(page.contains("form-action 'self'"));
    assert!(page.contains("id=\"rows\""));
    let before = c.log();
    for method in ["GET", "OPTIONS", "PUT"] {
        assert!(server
            .call(method, &format!("{path}/list"), "", "")
            .starts_with("HTTP/1.1 405"));
    }
    assert!(server
        .call(
            "POST",
            &format!("{path}/list"),
            "Content-Type: application/json\r\n",
            "{}"
        )
        .starts_with("HTTP/1.1 403"));
    server.post("list", &json!({"operation":"list","extra":true}), 400);
    server.post(
        "list",
        &json!({"operation":"capture","description":"Selected fixture layout"}),
        400,
    );
    assert!(server
        .call(
            "POST",
            &format!("{path}/list?extra=1"),
            &server.headers(),
            "{}"
        )
        .starts_with("HTTP/1.1 404"));
    assert!(server
        .call(
            "POST",
            &format!("{path}/list"),
            &server.headers(),
            &"x".repeat(16 * 1024 + 1)
        )
        .starts_with("HTTP/1.1 413"));
    let cross = server
        .headers()
        .replace("Origin: http://127.0.0.1:", "Origin: http://localhost:");
    assert!(server
        .call("POST", &format!("{path}/list"), &cross, "{}")
        .starts_with("HTTP/1.1 403"));
    assert_eq!(
        server.post("capabilities", &json!({"operation":"capabilities"}), 200)["operation"],
        "capabilities"
    );
    assert_eq!(before, c.log());
}

#[test]
fn exact_download_survives_newer_revision_and_source_loss_but_refuses_corruption() {
    let c = seeded("artifacts-web-download");
    std::fs::write(c.0.join("delivery.txt"), b"first saved delivery").unwrap();
    let first = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "delivery.txt",
            "--from",
            "g1",
            "--json",
        ],
    )["delivery"]
        .clone();
    c.ok(&[
        "decide",
        "Accept the first delivery",
        "--reason",
        "Review the selected saved revision",
        "--ref",
        first["reference"].as_str().unwrap(),
    ]);
    std::fs::write(c.0.join("delivery.txt"), b"second saved delivery").unwrap();
    let second = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "delivery.txt",
            "--from",
            "g1",
            "--artifact",
            first["artifact"].as_str().unwrap(),
            "--previous",
            first["revision"].as_str().unwrap(),
            "--json",
        ],
    )["delivery"]
        .clone();
    std::fs::remove_file(c.0.join("delivery.txt")).unwrap();
    let mut server = Server::start(&c);
    let headers = format!(
        "Content-Type: application/x-www-form-urlencoded\r\nOrigin: http://127.0.0.1:{}\r\n",
        server.port
    );
    let body = format!(
        "artifact={}&revision={}&token={}",
        first["artifact"].as_str().unwrap(),
        first["revision"].as_str().unwrap(),
        server.cookie.split_once('=').unwrap().1
    );
    let path = server.artifacts_path("download");
    let inventory = server.post("list", &json!({"operation":"list","all":true}), 200);
    assert_eq!(inventory["artifacts"][0]["revision"], second["revision"]);
    assert_eq!(
        inventory["artifacts"][0]["artifact_decision_references"][0]["revision"],
        first["revision"]
    );
    assert_eq!(
        inventory["artifacts"][0]["artifact_decision_references"][0]["alias"],
        "d2"
    );
    assert_eq!(
        inventory["artifacts"][0]["artifact_decision_references_total"],
        1
    );
    let selected = server.post(
        "show",
        &json!({"operation":"show","artifact":first["artifact"],"revision":first["revision"]}),
        200,
    );
    assert_eq!(selected["revisions"][0]["revision"], first["revision"]);
    assert_eq!(selected["latest_revision"], second["revision"]);
    let response = server.call("POST", &path, &headers, &body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response
        .to_ascii_lowercase()
        .contains("content-type: application/octet-stream\r\n"));
    assert!(response
        .to_ascii_lowercase()
        .contains("x-content-type-options: nosniff\r\n"));
    assert!(response
        .to_ascii_lowercase()
        .contains("content-disposition: attachment;"));
    assert!(response
        .to_ascii_lowercase()
        .contains("referrer-policy: no-referrer\r\n"));
    assert_eq!(
        response.split_once("\r\n\r\n").unwrap().1,
        "first saved delivery"
    );
    let cookie = std::mem::take(&mut server.cookie);
    let header_token = format!(
        "{headers}X-Vivac-Token: {}\r\n",
        cookie.split_once('=').unwrap().1
    );
    assert!(server
        .call("POST", &path, &header_token, &body)
        .starts_with("HTTP/1.1 403"));
    server.cookie = cookie;
    for invalid in [
        format!("{body}&extra=1"),
        format!("{body}&artifact=x"),
        body.replace("artifact=", "artifact=%"),
        body.replace("token=", "token=+"),
    ] {
        let response = server.call("POST", &path, &headers, &invalid);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(!response
            .to_ascii_lowercase()
            .contains("content-disposition:"));
    }
    let wrong_token = body.replace("token=", "token=0");
    assert!(server
        .call("POST", &path, &headers, &wrong_token)
        .starts_with("HTTP/1.1 400"));
    let mut wrong = body.clone().into_bytes();
    let last = wrong.last_mut().unwrap();
    *last = if *last == b'0' { b'1' } else { b'0' };
    assert!(server
        .call(
            "POST",
            &path,
            &headers,
            std::str::from_utf8(&wrong).unwrap()
        )
        .starts_with("HTTP/1.1 403"));
    let foreign = headers.replace("Origin: http://127.0.0.1:", "Origin: http://localhost:");
    assert!(server
        .call("POST", &path, &foreign, &body)
        .starts_with("HTTP/1.1 403"));
    let opaque = headers.replace(
        &format!("Origin: http://127.0.0.1:{}", server.port),
        "Origin: null",
    );
    let response = server.call("POST", &path, &opaque, &body);
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(response
        .to_ascii_lowercase()
        .contains("referrer-policy: no-referrer\r\n"));
    assert!(!response
        .to_ascii_lowercase()
        .contains("content-disposition:"));
    let unknown = body.replace(
        first["artifact"].as_str().unwrap(),
        "01aaaaaaaaaaaaaaaaaaaaaaaa",
    );
    assert!(server
        .call("POST", &path, &headers, &unknown)
        .starts_with("HTTP/1.1 400"));
    assert!(server
        .call(
            "POST",
            &path,
            &server.headers(),
            &json!({"artifact":first["artifact"],"revision":first["revision"]}).to_string()
        )
        .starts_with("HTTP/1.1 415"));
    std::fs::write(
        c.0.join(".vivac-artifacts/objects")
            .join(format!("{}.blob", first["revision"].as_str().unwrap())),
        b"modified saved bytes",
    )
    .unwrap();
    let response = server.call("POST", &path, &headers, &body);
    assert!(!response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(!response
        .to_ascii_lowercase()
        .contains("content-disposition:"));
    assert!(!response.contains("modified saved bytes"));
    assert!(response.contains(&format!(
        "#artifact={}&amp;revision={}",
        first["artifact"].as_str().unwrap(),
        first["revision"].as_str().unwrap()
    )));
    assert!(!std::fs::read_dir(c.0.join(".vivac-artifacts/objects"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-")));
}

#[test]
fn bundle_preview_keeps_the_selected_complete_revision_after_source_and_store_loss() {
    let c = seeded("artifacts-web-bundle-preview");
    std::fs::create_dir_all(c.0.join("delivery/pages/scripts")).unwrap();
    std::fs::create_dir_all(c.0.join("delivery/styles")).unwrap();
    let html = "<!doctype html><link rel=\"stylesheet\" href=\"../styles/main.css\"><script src=\"scripts/app.js\"></script>";
    std::fs::write(c.0.join("delivery/pages/index.html"), html).unwrap();
    std::fs::write(c.0.join("delivery/styles/main.css"), "body { color: red; }").unwrap();
    std::fs::write(
        c.0.join("delivery/pages/scripts/app.js"),
        "window.retained = 'first';",
    )
    .unwrap();
    std::fs::write(
        c.0.join("selection.json"),
        json!({
            "root": "delivery", "entrypoint": "pages/index.html",
            "members": ["pages/index.html", "styles/main.css", "pages/scripts/app.js"],
            "requirements": []
        })
        .to_string(),
    )
    .unwrap();
    let first = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "--bundle",
            "selection.json",
            "--from",
            "g1",
            "--json",
        ],
    )["delivery"]
        .clone();
    std::fs::write(
        c.0.join("delivery/pages/scripts/app.js"),
        "window.retained = 'second';",
    )
    .unwrap();
    cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "--bundle",
            "selection.json",
            "--from",
            "g1",
            "--artifact",
            first["artifact"].as_str().unwrap(),
            "--previous",
            first["revision"].as_str().unwrap(),
            "--json",
        ],
    );
    std::fs::remove_dir_all(c.0.join("delivery")).unwrap();
    let mut server = Server::start(&c);
    let headers = format!(
        "Content-Type: application/x-www-form-urlencoded\r\nOrigin: http://127.0.0.1:{}\r\n",
        server.port
    );
    let session_token = server.cookie.split_once('=').unwrap().1.to_owned();
    let body = format!(
        "artifact={}&revision={}&token={session_token}",
        first["artifact"].as_str().unwrap(),
        first["revision"].as_str().unwrap()
    );
    let route = server.artifacts_path("preview");
    let cookie = std::mem::take(&mut server.cookie);
    let no_session = server.call("POST", &route, &headers, &body);
    assert!(no_session.starts_with("HTTP/1.1 401"), "{no_session}");
    let header_token = format!("{headers}X-Vivac-Token: {session_token}\r\n");
    let no_cookie = server.call("POST", &route, &header_token, &body);
    assert!(no_cookie.starts_with("HTTP/1.1 403"), "{no_cookie}");
    for response in [&no_session, &no_cookie] {
        assert!(!response.contains(&session_token), "{response}");
        assert!(!response.contains(html), "{response}");
        assert!(!response.contains("window.retained"), "{response}");
    }
    server.cookie = cookie;
    let refused = server.call("POST", &server.artifacts_path("download"), &headers, &body);
    assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
    assert!(
        refused.contains("Bundle download is not supported."),
        "{refused}"
    );
    let response = server.call("POST", &route, &headers, &body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(!response.contains(&session_token));
    let location = response
        .split_once("content=\"0;url=")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    let (host, token) = location
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let port: u16 = host.strip_prefix("localhost:").unwrap().parse().unwrap();
    let call = |method: &str, path: &str, headers: &str, body: &str| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{headers}\r\n{body}"
        )
        .unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    std::fs::remove_dir_all(c.0.join(".vivac-artifacts")).unwrap();
    let wrapper = call("GET", &format!("/{token}"), "", "");
    assert!(wrapper.contains(first["revision"].as_str().unwrap()));
    assert!(!wrapper.contains("window.retained"));
    let control = format!("token={token}");
    let control_headers = format!("Origin: http://{host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", control.len());
    let run = call("POST", "/run", &control_headers, &control);
    assert!(run.starts_with("HTTP/1.1 303"), "{run}");
    assert!(!run.contains(token));
    let entry = run
        .lines()
        .find_map(|line| line.strip_prefix("Location: "))
        .unwrap();
    let resources = entry.strip_suffix("pages/index.html").unwrap();
    assert!(resources.starts_with("/assets/"));
    assert!(!resources.contains(token));
    for (member, media_type, bytes) in [
        ("pages/index.html", "text/html", html),
        ("styles/main.css", "text/css", "body { color: red; }"),
        (
            "pages/scripts/app.js",
            "text/javascript",
            "window.retained = 'first';",
        ),
    ] {
        let response = call(
            "GET",
            &format!("{resources}{member}"),
            "Origin: null\r\n",
            "",
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with(bytes));
        assert!(response.contains(&format!("Content-Type: {media_type}; charset=utf-8")));
        assert!(!response.contains(token));
        assert!(!response.contains(&session_token));
    }
    assert!(
        call("GET", &format!("{resources}.vivac-delivery.json"), "", "")
            .starts_with("HTTP/1.1 404")
    );
    assert!(call(
        "POST",
        "/close",
        &control_headers.replace(&format!("http://{host}"), "null"),
        &control
    )
    .starts_with("HTTP/1.1 403"));
    assert!(call("POST", "/close", &control_headers, &control).contains("Preview closed."));
}

#[test]
fn preview_opens_exact_retained_html_and_preserves_native_gates() {
    let c = seeded("artifacts-web-preview");
    std::fs::write(
        c.0.join("layout.HTML"),
        b"<!doctype html><p>first retained view</p>",
    )
    .unwrap();
    let first = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "layout.HTML",
            "--from",
            "g1",
            "--json",
        ],
    )["delivery"]
        .clone();
    std::fs::write(c.0.join("layout.HTML"), b"<!doctype html><p>newer view</p>").unwrap();
    cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "layout.HTML",
            "--from",
            "g1",
            "--artifact",
            first["artifact"].as_str().unwrap(),
            "--previous",
            first["revision"].as_str().unwrap(),
            "--json",
        ],
    );
    std::fs::remove_file(c.0.join("layout.HTML")).unwrap();
    let mut server = Server::start(&c);
    let headers = format!(
        "Content-Type: application/x-www-form-urlencoded\r\nOrigin: http://127.0.0.1:{}\r\n",
        server.port
    );
    let body = format!(
        "artifact={}&revision={}&token={}",
        first["artifact"].as_str().unwrap(),
        first["revision"].as_str().unwrap(),
        server.cookie.split_once('=').unwrap().1
    );
    let path = server.artifacts_path("preview");
    let response = server.call("POST", &path, &headers, &body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("http-equiv=\"refresh\""));
    assert!(response.contains("Opening preview"));
    assert!(response.contains("Open isolated preview"));
    let location = response
        .split_once("content=\"0;url=")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    assert!(location.starts_with("http://localhost:"), "{location}");
    assert!(!location.starts_with(&format!("http://127.0.0.1:{}", server.port)));
    assert!(!location.contains(server.cookie.split_once('=').unwrap().1));
    assert!(response.contains(&format!("href=\"{location}\"")));
    assert!(!response.to_ascii_lowercase().contains("location:"));
    assert!(!response.contains(server.cookie.split_once('=').unwrap().1));
    assert!(!response.contains("first retained view"));
    let (authority, preview_path) = location
        .trim_start_matches("http://")
        .split_once('/')
        .unwrap();
    assert_eq!(preview_path.len(), 64);
    assert!(preview_path
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    let port: u16 = authority.rsplit(':').next().unwrap().parse().unwrap();
    let mut preview_stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    preview_stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    write!(
        preview_stream,
        "GET /{preview_path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut wrapper = String::new();
    preview_stream.read_to_string(&mut wrapper).unwrap();
    assert!(wrapper.starts_with("HTTP/1.1 200"), "{wrapper}");
    assert!(wrapper.contains(first["revision"].as_str().unwrap()));
    assert!(wrapper.contains("Run HTML"));
    assert!(
        !wrapper.contains("first retained view"),
        "The wrapper must not run the retained HTML before Run HTML"
    );
    for required in [
        "referrer-policy: no-referrer",
        "cache-control: no-store",
        "cross-origin-opener-policy: same-origin",
        "script-src 'none'",
        "form-action 'none'",
    ] {
        assert!(response.to_ascii_lowercase().contains(required));
    }
    let cookie = std::mem::take(&mut server.cookie);
    let no_session = server.call("POST", &path, &headers, &body);
    assert!(no_session.starts_with("HTTP/1.1 401"), "{no_session}");
    let header_token = format!(
        "{headers}X-Vivac-Token: {}\r\n",
        cookie.split_once('=').unwrap().1
    );
    let no_cookie = server.call("POST", &path, &header_token, &body);
    assert!(no_cookie.starts_with("HTTP/1.1 403"), "{no_cookie}");
    server.cookie = cookie;
    for invalid in [
        format!("{body}&artifact=x"),
        format!("{body}&extra=x"),
        body.replace("token=", "token=+"),
    ] {
        assert!(server
            .call("POST", &path, &headers, &invalid)
            .starts_with("HTTP/1.1 400"));
    }
    let duplicate_cookie = format!("{headers}Cookie: {}\r\n", server.cookie);
    let duplicate_origin = format!("{headers}Origin: http://127.0.0.1:{}\r\n", server.port);
    let duplicate_host = format!("{headers}Host: 127.0.0.1:{}\r\n", server.port);
    for operation in ["download", "preview"] {
        for duplicates in [&duplicate_cookie, &duplicate_origin, &duplicate_host] {
            assert!(server
                .call("POST", &server.artifacts_path(operation), duplicates, &body)
                .starts_with("HTTP/1.1 403"));
        }
        let duplicate_type =
            format!("{headers}Content-Type: application/x-www-form-urlencoded\r\n");
        assert!(server
            .call(
                "POST",
                &server.artifacts_path(operation),
                &duplicate_type,
                &body
            )
            .starts_with("HTTP/1.1 415"));
    }
    assert!(server
        .call("GET", &path, &headers, "")
        .starts_with("HTTP/1.1 405"));
    assert!(server
        .call("POST", &path, &server.headers(), &body)
        .starts_with("HTTP/1.1 415"));
    assert!(server
        .call("POST", &path, &headers, &"x".repeat(16 * 1024 + 1))
        .starts_with("HTTP/1.1 413"));
    let mut wrong_token = body.clone().into_bytes();
    let last = wrong_token.last_mut().unwrap();
    *last = if *last == b'0' { b'1' } else { b'0' };
    assert!(server
        .call(
            "POST",
            &path,
            &headers,
            std::str::from_utf8(&wrong_token).unwrap()
        )
        .starts_with("HTTP/1.1 403"));
    let foreign = headers.replace("Origin: http://127.0.0.1:", "Origin: http://localhost:");
    assert!(server
        .call("POST", &path, &foreign, &body)
        .starts_with("HTTP/1.1 403"));
    let wrong_artifact = body.replace(
        first["artifact"].as_str().unwrap(),
        "01aaaaaaaaaaaaaaaaaaaaaaaa",
    );
    assert!(server
        .call("POST", &path, &headers, &wrong_artifact)
        .starts_with("HTTP/1.1 400"));
    std::fs::write(c.0.join("text.txt"), b"retained plain text").unwrap();
    let text = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "text.txt",
            "--from",
            "g1",
            "--json",
        ],
    )["delivery"]
        .clone();
    let text_body = format!(
        "artifact={}&revision={}&token={}",
        text["artifact"].as_str().unwrap(),
        text["revision"].as_str().unwrap(),
        server.cookie.split_once('=').unwrap().1
    );
    let other_artifact = body.replace(
        first["artifact"].as_str().unwrap(),
        text["artifact"].as_str().unwrap(),
    );
    assert!(server
        .call("POST", &path, &headers, &other_artifact)
        .starts_with("HTTP/1.1 400"));
    let unsupported = server.call("POST", &path, &headers, &text_body);
    assert!(unsupported.starts_with("HTTP/1.1 400"), "{unsupported}");
    assert!(unsupported.contains("Only retained HTML files can be previewed."));
    std::fs::write(
        c.0.join(".vivac-artifacts/objects")
            .join(format!("{}.blob", first["revision"].as_str().unwrap())),
        b"corrupted retained HTML",
    )
    .unwrap();
    let refused = server.call("POST", &path, &headers, &body);
    assert!(!refused.starts_with("HTTP/1.1 200"), "{refused}");
    assert!(refused.contains("Preview refused"));
    assert!(!refused.contains("corrupted retained HTML"));
    assert!(refused.contains(&format!(
        "#artifact={}&amp;revision={}",
        first["artifact"].as_str().unwrap(),
        first["revision"].as_str().unwrap()
    )));
}

#[test]
fn web_reads_match_cli_and_capabilities_do_not_refresh_a_broken_log() {
    let c = seeded("artifacts-web-shared-engine");
    std::fs::write(c.0.join("delivery.txt"), b"saved delivery").unwrap();
    let saved = cli(
        &c,
        &[
            "artifacts",
            "capture",
            "--description",
            "Selected fixture layout",
            "delivery.txt",
            "--from",
            "g1",
            "--json",
        ],
    )["delivery"]
        .clone();
    let server = Server::start(&c);
    let expected = cli(&c, &["artifacts", "list", "--all", "--json"]);
    let listed = server.post("list", &json!({"operation":"list","all":true}), 200);
    assert_eq!(listed, expected);
    assert_eq!(listed["artifacts"][0]["producer_node"]["alias"], "g1");
    let shown = server.post(
        "show",
        &json!({"operation":"show","artifact":saved["artifact"],"revision":saved["revision"]}),
        200,
    );
    assert_eq!(shown["revisions"][0]["reference"], saved["reference"]);
    assert_eq!(shown["revisions"][0]["producer"], saved["producer"]);
    let before = c.log();
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(c.0.join(".vivac/events"))
        .unwrap();
    log.write_all(b"broken event line\n").unwrap();
    drop(log);
    let page = server.call("GET", &format!("{}artifacts", server.path), "", "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert_eq!(
        server.post("capabilities", &json!({"operation":"capabilities"}), 200)["operation"],
        "capabilities"
    );
    server.post("list", &json!({"operation":"list","all":true}), 409);
    assert_eq!(c.log(), format!("{before}broken event line\n"));
}
