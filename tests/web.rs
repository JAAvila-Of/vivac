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
        Server::start_serving(dir, &[])
    }

    /// Like `start`, but naming the roots to serve with one `--project` per
    /// directory, so a server can be asked to serve more than the one it
    /// starts in. `dir` is still the working directory: `vivac web` needs a
    /// tree at or above its cwd regardless of what `--project` names.
    fn start_serving(dir: &std::path::Path, roots: &[&std::path::Path]) -> Server {
        let port = free_port();
        let mut args: Vec<std::ffi::OsString> = vec![
            "web".into(),
            "--port".into(),
            port.to_string().into(),
            "--no-open".into(),
        ];
        for r in roots {
            args.push("--project".into());
            args.push((*r).into());
        }
        let mut child = Command::new(BIN)
            .current_dir(dir)
            .args(&args)
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

/// The session token, off the `Set-Cookie` the boot key hands back.
///
/// It used to be read out of a `<meta>` in the boot page. `d190` replaced
/// that page with a redirect, so the token now arrives where a browser
/// picks it up on its own -- which was the whole point of `f189`.
fn token_from(a: &Answer) -> String {
    let raw = a.header("Set-Cookie").unwrap_or_else(|| {
        panic!(
            "no Set-Cookie on the boot answer:
{}",
            a.body
        )
    });
    let after = raw
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .unwrap_or_else(|| panic!("no name=value in {raw}"));
    assert_eq!(after.0.trim(), "vivac_session", "in {raw}");
    after.1.trim().to_string()
}

/// Every `href="..."` value in the page, in the order they appear.
fn hrefs_in(body: &str) -> Vec<String> {
    let marker = "href=\"";
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(marker) {
        let after = &rest[start + marker.len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn up(name: &str) -> Up {
    let sandbox = Sandbox::new_seeded(name);
    let server = Server::start(&sandbox.0);
    Up {
        server,
        _sandbox: sandbox,
    }
}

/// A running server over more than one sandbox, none of which it outlives.
/// Field order matters here for the same reason it does in `Up`.
struct UpMany {
    server: Server,
    _sandboxes: Vec<Sandbox>,
}

impl UpMany {
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

fn up_many(names: &[&str]) -> UpMany {
    let sandboxes: Vec<Sandbox> = names.iter().map(|n| Sandbox::new_seeded(n)).collect();
    let roots: Vec<&std::path::Path> = sandboxes.iter().map(|s| s.0.as_path()).collect();
    let server = Server::start_serving(&sandboxes[0].0, &roots);
    UpMany {
        server,
        _sandboxes: sandboxes,
    }
}

#[test]
fn the_boot_key_hands_over_a_session_cookie_and_redirects() {
    let s = up("boot");
    let a = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    // `d190`: it lands you where the work is, instead of on a page whose
    // whole content was "vivac is listening".
    assert_eq!(a.status, 302, "{}", a.body);
    assert_eq!(a.header("Location"), Some("/"), "{}", a.body);
    assert_eq!(token_from(&a).len(), 64, "{}", a.body);
    let jar = a.header("Set-Cookie").unwrap();
    // The flags are the defence, so they are asserted and not assumed.
    assert!(jar.contains("HttpOnly"), "{jar}");
    assert!(jar.contains("SameSite=Strict"), "{jar}");
    assert!(jar.contains("Path=/"), "{jar}");
    // A session cookie: it dies with the browser, and the server forgets
    // the token when the process does.
    assert!(!jar.contains("Max-Age"), "{jar}");
    assert!(!jar.contains("Expires"), "{jar}");
}

#[test]
fn the_same_boot_url_a_second_time_is_refused() {
    let s = up("boot-twice");
    let path = s.boot_path();
    let first = call(s.port(), &path, &[("Host", s.host())]);
    assert_eq!(first.status, 302, "{}", first.body);
    let second = call(s.port(), &path, &[("Host", s.host())]);
    assert_eq!(
        second.status, 401,
        "a spent boot key still unlocked something:\n{}",
        second.body
    );
}

/// `d145`: the index with exactly one project redirects rather than
/// serving a list nobody needs to read, so "serves" here means the gate let
/// the request through to the router -- not that it came back as `200`.
#[test]
fn a_good_token_in_the_header_serves() {
    let s = up("good-token");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let a = call(
        s.port(),
        "/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 302, "{}", a.body);
}

/// `d145`: with exactly one project, the index does not make anybody click
/// through it.
#[test]
fn a_single_project_index_redirects_to_its_page() {
    let s = up("index-one");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let a = call(
        s.port(),
        "/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 302, "{}", a.body);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        a.header("Location"),
        Some(format!("/p/{id}/")).as_deref(),
        "{:?}",
        a.headers
    );
}

/// `d146`: the link shows the directory's own name and points at its
/// sanitized id. One of the two sandboxes carries a space in its name for
/// exactly that reason: a `href` can never carry one, a link's text can.
#[test]
fn two_or_more_projects_list_with_a_link_each() {
    let up = up_many(&["index-list-a", "index list b"]);
    let boot = call(up.port(), &up.boot_path(), &[("Host", up.host())]);
    let token = token_from(&boot);
    let a = call(
        up.port(),
        "/",
        &[("Host", up.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);

    let hrefs = hrefs_in(&a.body);
    assert_eq!(hrefs.len(), 2, "expected one link per project:\n{}", a.body);
    assert!(
        hrefs.iter().all(|h| !h.contains(' ')),
        "a href carried a raw space, which an id can never have: {hrefs:?}"
    );

    for sandbox in &up._sandboxes {
        let name = sandbox
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            a.body.contains(&name),
            "the link text does not show the raw name {name}:\n{}",
            a.body
        );
    }
}

/// `WEB.md` §3.1 over a real socket: the page a project's `id` routes to,
/// with the focus on it.
#[test]
fn a_projects_today_page_serves_with_its_focus_on_it() {
    let s = up("today");
    s._sandbox
        .ok(&["push", "Fix the cache adapter", "--why", "the bug needs it"]);
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let a = call(
        s.port(),
        &format!("/p/{id}/"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(a.header("Content-Type"), Some("text/html; charset=utf-8"));
    assert!(a.body.contains("Fix the cache adapter"), "{}", a.body);
    assert!(a.body.contains("you are here"), "{}", a.body);
    assert!(a.body.contains("What moved"), "{}", a.body);
}

/// `WEB.md` §7.4: the page loads with no internet. Proved on the bytes that
/// actually left the socket, not on the template they were built from.
#[test]
fn the_served_page_reaches_for_nothing_off_this_machine() {
    let s = up("today-offline");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let a = call(
        s.port(),
        &format!("/p/{id}/"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(!a.body.contains("http://"), "{}", a.body);
    assert!(!a.body.contains("https://"), "{}", a.body);
}

/// An `id` the registry does not hold is a 404, and the id it did not
/// recognise never comes back in the answer.
#[test]
fn a_project_id_the_registry_does_not_hold_is_not_found() {
    let s = up("today-unknown");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let a = call(
        s.port(),
        "/p/not-a-project/",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 404, "{}", a.body);
    assert_eq!(a.body, "not found\n");
}

/// An admitted request for a path `route` does not know is `NotFound`, and
/// that has to come back as its own 404 rather than as the index page --
/// the one thing a security-relevant router must never do is fall open.
#[test]
fn an_unknown_route_is_not_found_and_not_the_index() {
    let s = up("unknown-route");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let a = call(
        s.port(),
        "/does-not-exist",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 404, "{}", a.body);
    assert_eq!(
        a.body, "not found\n",
        "a 404 leaked something else:\n{}",
        a.body
    );
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
    let token = token_from(&boot);
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
    let token = token_from(&boot);
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
    let token = token_from(&boot);
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
    let token = token_from(&boot);
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
/// `WEB.md` §3.2 over a real socket: the lineage of a node, drawn.
///
/// The unit tests build the page from a folded tree; this one proves the
/// route reaches it and that the bytes leaving the socket carry the shape.
#[test]
fn the_lineage_of_a_node_is_served_with_a_step_per_ancestor() {
    let s = up("why-page");
    s._sandbox
        .ok(&["push", "Ship the thing", "--why", "it is the goal"]);
    s._sandbox
        .ok(&["push", "The face is web", "--why", "the owner asked"]);
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let a = call(
        s.port(),
        &format!("/p/{id}/why/2"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(a.header("Content-Type"), Some("text/html; charset=utf-8"));
    // Both steps of the path, and the mark on the one that was asked for.
    assert!(a.body.contains("Ship the thing"), "{}", a.body);
    assert!(a.body.contains("The face is web"), "{}", a.body);
    assert!(a.body.contains("you are here"), "{}", a.body);
    // The drawing is also the way you walk the tree.
    assert!(
        hrefs_in(&a.body).iter().any(|h| h.contains("/why/")),
        "{}",
        a.body
    );
    // §7.4, proved on the bytes that actually left the socket.
    assert!(!a.body.contains("http://"), "{}", a.body);
    assert!(!a.body.contains("https://"), "{}", a.body);
}

/// A node the tree does not hold is a 404, the same as a project it does
/// not hold: a page that draws an empty spine for a typo teaches that the
/// node exists.
#[test]
fn a_lineage_for_a_node_that_is_not_there_is_not_found() {
    let s = up("why-unknown");
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let a = call(
        s.port(),
        &format!("/p/{id}/why/f999"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 404, "{}", a.body);
    assert_eq!(a.body, "not found\n");
}

/// `f189`: the test that was missing, and whose absence let the main
/// surface sit broken for a day with fifteen green ones around it. No
/// `X-Vivac-Token` anywhere -- just the cookie, which is all a browser
/// sends back when it follows a link.
#[test]
fn a_browser_that_only_carries_the_cookie_can_walk_from_today_to_a_lineage() {
    let s = up("browsing");
    s._sandbox
        .ok(&["push", "Ship the thing", "--why", "it is the goal"]);
    s._sandbox
        .ok(&["push", "The face is web", "--why", "the owner asked"]);
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let jar = format!("vivac_session={}", token_from(&boot));
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let today = call(
        s.port(),
        &format!("/p/{id}/"),
        &[("Host", s.host()), ("Cookie", jar.clone())],
    );
    assert_eq!(today.status, 200, "{}", today.body);

    // Follow a link off the page rather than a path written here: a link
    // that goes nowhere is exactly the failure this test exists for.
    let lineage = hrefs_in(&today.body)
        .into_iter()
        .find(|h| h.contains("/why/"))
        .unwrap_or_else(|| {
            panic!(
                "no lineage link on the Today page:
{}",
                today.body
            )
        });
    let a = call(s.port(), &lineage, &[("Host", s.host()), ("Cookie", jar)]);
    assert_eq!(a.status, 200, "{} -> {}", lineage, a.body);
    assert!(a.body.contains("you are here"), "{}", a.body);
}
