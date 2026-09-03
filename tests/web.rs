//! `vivac web` — the defenses, proved over a real socket.
//!
//! `d149` put the rules in `src/web/gate.rs` as a pure function so every
//! denial could be a unit test with no server at all. This is the other
//! half: the same rules, read off a `TcpStream` a browser would actually
//! open, so a header that never reaches `Gate::admit` -- wrong case, wrong
//! name, dropped by the socket layer -- would still show up here.
//!
//! There is no HTTP client in the dependency tree and this does not add one:
//! the client is ours to write, and a request is four lines and a blank one.

mod common;
use common::Sandbox;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// A port nothing else is listening on right now. `vivac web --port 0` would
/// pick one itself, but only the process that bound it would know which --
/// so the test binds one first, reads it, lets it go, and hands the number
/// to `--port` instead.
fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap().port()
}

struct Server {
    child: Child,
    port: u16,
    /// The URL printed at startup, key and all. Good for exactly one call.
    boot_url: String,
}

impl Server {
    fn start(dir: &std::path::Path) -> Server {
        let port = free_port();
        let mut child = Command::new(BIN)
            .current_dir(dir)
            .args(["web", "--port", &port.to_string(), "--no-open"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut boot_url = None;
        // Two lines are printed before the server ever blocks in `recv()`,
        // and both contain `http://`: the first names the address it bound,
        // the second is the boot url. Only the second carries the key.
        for _ in 0..10 {
            let mut line = String::new();
            let n = reader.read_line(&mut line).unwrap();
            assert!(n > 0, "the server exited before printing a boot url");
            if let Some(at) = line.find("http://") {
                if line.contains("?k=") {
                    boot_url = Some(line[at..].trim().to_string());
                    break;
                }
            }
        }
        Server {
            child,
            port,
            boot_url: boot_url.expect("no boot url in the server's startup lines"),
        }
    }

    /// The path and query of the boot url, with the scheme and address
    /// stripped: what `call` below needs, since it addresses the port
    /// itself.
    fn boot_path(&self) -> String {
        let after_scheme = self.boot_url.trim_start_matches("http://");
        let slash = after_scheme
            .find('/')
            .expect("the boot url has no path at all");
        after_scheme[slash..].to_string()
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A running server over a sandbox it does not outlive.
///
/// Field order matters here: Rust drops a struct's fields in the order they
/// are declared, and `server` has to go first. Killing the process before
/// the sandbox removes its directory is what keeps the cleanup from racing
/// a process that still has files in it open.
struct Up {
    server: Server,
    _sandbox: Sandbox,
}

impl Up {
    fn port(&self) -> u16 {
        self.server.port
    }

    fn boot_path(&self) -> String {
        self.server.boot_path()
    }

    fn host(&self) -> String {
        self.server.host()
    }
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One request, written by hand: a request line, whatever headers the test
/// passes, and a blank line. `Connection: close` is added on every call so
/// the response can be read to its end without trusting keep-alive.
fn call(port: u16, path: &str, headers: &[(&str, String)]) -> Answer {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let mut request = format!("GET {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).unwrap();

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("not a status line: {status_line:?}"));

    let mut headers_out = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (k, v) = trimmed
            .split_once(':')
            .unwrap_or_else(|| panic!("not a header line: {trimmed:?}"));
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        if k.eq_ignore_ascii_case("content-length") {
            content_length = v.parse().unwrap_or(0);
        }
        headers_out.push((k, v));
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).unwrap();
    Answer {
        status,
        headers: headers_out,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// The token out of the boot page's `<meta name="vivac-token" content="...">`.
fn token_from(body: &str) -> String {
    let marker = "content=\"";
    let start = body
        .find(marker)
        .unwrap_or_else(|| panic!("no token meta tag in the boot page:\n{body}"))
        + marker.len();
    let rest = &body[start..];
    let end = rest.find('"').unwrap();
    rest[..end].to_string()
}

fn up(name: &str) -> Up {
    let sandbox = Sandbox::new_seeded(name);
    let server = Server::start(&sandbox.0);
    Up {
        server,
        _sandbox: sandbox,
    }
}

#[test]
fn the_boot_url_returns_the_token() {
    let s = up("boot");
    let a = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(token_from(&a.body).len(), 64, "{}", a.body);
}

#[test]
fn the_same_boot_url_a_second_time_is_refused() {
    let s = up("boot-twice");
    let path = s.boot_path();
    let first = call(s.port(), &path, &[("Host", s.host())]);
    assert_eq!(first.status, 200, "{}", first.body);
    let second = call(s.port(), &path, &[("Host", s.host())]);
    assert_eq!(
        second.status, 401,
        "a spent boot key still unlocked something:\n{}",
        second.body
    );
}

#[test]
fn a_good_token_in_the_header_serves() {
    let s = up("good-token");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot.body);
    let a = call(
        s.port(),
        "/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
}

#[test]
fn no_token_at_all_is_refused() {
    let s = up("no-token");
    let a = call(s.port(), "/", &[("Host", s.host())]);
    assert_eq!(a.status, 401, "{}", a.body);
}

#[test]
fn a_foreign_host_is_refused_even_with_the_right_token() {
    let s = up("foreign-host");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot.body);
    let a = call(
        s.port(),
        "/",
        &[("Host", "malo.com".to_string()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 403, "{}", a.body);
}

#[test]
fn a_foreign_origin_is_refused() {
    let s = up("foreign-origin");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot.body);
    let a = call(
        s.port(),
        "/",
        &[
            ("Host", s.host()),
            ("Origin", "http://malo.com".to_string()),
            ("X-Vivac-Token", token),
        ],
    );
    assert_eq!(a.status, 403, "{}", a.body);
}

/// Nothing here is CORS-visible from another origin: the missing
/// `Access-Control-Allow-Origin` is exactly what leaves a page with a stolen
/// token unable to read the answer.
#[test]
fn no_response_ever_carries_a_cors_header() {
    let s = up("no-cors");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot.body);
    let served = call(
        s.port(),
        "/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    let refused = call(s.port(), "/", &[("Host", s.host())]);
    let foreign = call(s.port(), "/", &[("Host", "malo.com".to_string())]);
    for a in [&boot, &served, &refused, &foreign] {
        assert!(
            a.header("Access-Control-Allow-Origin").is_none(),
            "a response carried a CORS header:\n{:?}",
            a.headers
        );
    }
}

#[test]
fn every_response_carries_the_security_headers() {
    let s = up("security-headers");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot.body);
    let served = call(
        s.port(),
        "/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    let refused = call(s.port(), "/", &[("Host", s.host())]);
    let foreign = call(s.port(), "/", &[("Host", "malo.com".to_string())]);
    for a in [&boot, &served, &refused, &foreign] {
        assert!(
            a.header("Content-Security-Policy").is_some(),
            "no CSP on a {} response",
            a.status
        );
        assert_eq!(
            a.header("Referrer-Policy"),
            Some("no-referrer"),
            "the boot url carries the key in its query string, and this header \
             is what keeps it out of a `Referer`: {:?}",
            a.headers
        );
    }
}
