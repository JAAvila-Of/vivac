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
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// The port out of a boot url's authority.
///
/// The test does not pick the port: `--port 0` lets the operating system
/// pick, and the server prints the address it bound. So the number arrives
/// from the process that is holding the socket, and there is no instant in
/// which it is free for something else to take -- which is the point. This
/// used to bind a port here, read its number, release it, and hand that
/// number to the child; on a runner with tests in parallel another test
/// took it in the gap and the server died at startup (`f385`).
fn port_of(url: &str) -> u16 {
    let authority = url.trim_start_matches("http://");
    let authority = authority.split('/').next().unwrap_or(authority);
    authority
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no port in the boot url: {url}"))
}

struct Server {
    child: Child,
    port: u16,
    /// The URL printed at startup, key and all. Good for exactly one call.
    boot_url: String,
}

impl Server {
    fn start(sandbox: &Sandbox) -> Server {
        Server::start_serving(&sandbox.0, sandbox.global_home(), &[])
    }

    /// Like `start`, but naming the roots to serve with one `--project` per
    /// directory, so a server can be asked to serve more than the one it
    /// starts in. `dir` is still the working directory, and since `d199`
    /// that is all it is: `vivac web` no longer needs a tree at or above its
    /// cwd, and what the cwd decides now is only where `/` lands.
    fn start_serving(
        dir: &std::path::Path,
        home: &std::path::Path,
        roots: &[&std::path::Path],
    ) -> Server {
        let mut args: Vec<std::ffi::OsString> = vec![
            "web".into(),
            // Zero, and never a number of ours: see `port_of`.
            "--port".into(),
            "0".into(),
            "--no-open".into(),
        ];
        for r in roots {
            args.push("--project".into());
            args.push((*r).into());
        }
        let mut child = Command::new(BIN)
            .current_dir(dir)
            .env("VIVAC_HOME", home)
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
        // the second is the boot url. Only the second carries the key -- and
        // it names the same address, so it is the one datum the test needs
        // and the port comes out of it too.
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
        let boot_url = boot_url.expect("no boot url in the server's startup lines");
        Server {
            child,
            port: port_of(&boot_url),
            boot_url,
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

/// The body, however the server chose to frame it.
///
/// `tiny_http` answers with `Content-Length` while it can and switches to
/// `Transfer-Encoding: chunked` once a response outgrows its buffer. Every
/// page was small enough for the first branch until the map arrived at
/// rather more than a megabyte, and a client that only knew lengths read
/// every one of those pages as empty -- with a `200` in hand, which is the
/// worst way to be wrong. The client is ours, so it learns the other
/// framing rather than the pages staying small enough to avoid it.
fn read_body(reader: &mut impl BufRead, content_length: usize, chunked: bool) -> Vec<u8> {
    if !chunked {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).unwrap();
        return body;
    }
    let mut body = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        // A chunk size is hex, and may carry extensions after a semicolon
        // that nothing here needs.
        let size = usize::from_str_radix(
            header
                .trim_end_matches(['\r', '\n'])
                .split(';')
                .next()
                .unwrap_or("")
                .trim(),
            16,
        )
        .unwrap_or_else(|_| panic!("not a chunk size: {header:?}"));
        if size == 0 {
            break;
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk).unwrap();
        body.extend_from_slice(&chunk);
        // The CRLF that closes the chunk.
        let mut end = [0u8; 2];
        reader.read_exact(&mut end).unwrap();
    }
    body
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
    let mut chunked = false;
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
        if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
            chunked = true;
        }
        headers_out.push((k, v));
    }

    let body = read_body(&mut reader, content_length, chunked);
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
    let server = Server::start(&sandbox);
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

/// Started from a directory that is not any of them, because since `d199`
/// that is what reaches the index: from inside a project, `/` lands on that
/// project instead of listing.
fn up_many(names: &[&str]) -> UpMany {
    let sandboxes: Vec<Sandbox> = names.iter().map(|n| Sandbox::new_seeded(n)).collect();
    let roots: Vec<&std::path::Path> = sandboxes.iter().map(|s| s.0.as_path()).collect();
    let server = Server::start_serving(&std::env::temp_dir(), sandboxes[0].global_home(), &roots);
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

/// `WEB.md` §3.6 over a real socket: the global graph, at the route `d194`
/// gave it. Both spellings serve it, and a project the registry does not
/// hold is still a 404 there too, not the page for someone else's tree.
#[test]
fn the_tree_page_routes_with_or_without_a_slash_and_a_bogus_project_is_still_not_found() {
    let s = up("tree-route");
    s._sandbox
        .ok(&["push", "Ship the thing", "--why", "it is the goal"]);
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    for path in [format!("/p/{id}/tree"), format!("/p/{id}/tree/")] {
        let a = call(
            s.port(),
            &path,
            &[("Host", s.host()), ("X-Vivac-Token", token.clone())],
        );
        assert_eq!(a.status, 200, "{path}: {}", a.body);
    }

    let bogus = call(
        s.port(),
        "/p/not-a-project/tree",
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(bogus.status, 404, "{}", bogus.body);
}

/// `d391`: the map draws every node once, leaves included. There are no
/// tiles any more -- a leaf used to exist only inside its parent's comb --
/// and every alias still reaches its own lineage, which is exactly what a
/// click costs a reader whose browser runs no script.
#[test]
fn every_node_is_one_row_that_still_reaches_its_own_lineage() {
    let s = up("map-rows");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "A branch",
        "--why",
        "it needs its own children",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf beside it",
        "--why",
        "it stands alone",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf below the branch",
        "--why",
        "it hangs off the branch",
        "--parent",
        "2",
    ]);

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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);

    for alias in ["g1", "t2", "t3", "t4"] {
        let needle = format!("href=\"/p/{id}/why/{alias}\"");
        assert_eq!(
            a.body.matches(&needle).count(),
            1,
            "{alias} should reach its own lineage exactly once:\n{}",
            a.body
        );
        assert_eq!(
            a.body.matches(&format!("id=\"{alias}\"")).count(),
            1,
            "{alias} should be one row and no more:\n{}",
            a.body
        );
    }
}

/// `d391`, and `d387` before it: one flat list, one row per node, every row
/// at the same offset. A row's index in that list *is* the row its station
/// is drawn on, so a nest here would put the drawing and the list on
/// different lines.
///
/// The old page gave a row only to a node with children and drew a leaf as
/// a tile in its parent's comb. The map gives every node a row, which is
/// what lets a leaf carry a station of its own.
#[test]
fn the_map_is_one_flat_list_with_a_row_for_every_node() {
    let s = up("map-flat");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "A branch",
        "--why",
        "it needs its own children",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf beside it",
        "--why",
        "it stands alone",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf below the branch",
        "--why",
        "it hangs off the branch",
        "--parent",
        "2",
    ]);

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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);

    assert_eq!(
        a.body.matches("<ol class=\"stops\">").count(),
        1,
        "a second list is a nest, and a nest moves a row off its station:\n{}",
        a.body
    );
    assert_eq!(a.body.matches("<li class=\"stop").count(), 4, "{}", a.body);

    // Tree order, and the two gutters drawing the same four stations on the
    // same four rows.
    for (row, alias) in ["g1", "t2", "t4", "t3"].iter().enumerate() {
        assert!(
            a.body
                .contains(&format!("id=\"{alias}\" data-stop=\"{row}\"")),
            "{alias} should be row {row}:\n{}",
            a.body
        );
        assert_eq!(
            a.body.matches(&format!("data-stop=\"{row}\" cx=")).count(),
            2,
            "one station in each of the two gutters:\n{}",
            a.body
        );
    }
}

/// The drawing carries the fan-out in the radius of a station, and a radius
/// saturates: past thirty children they all look alike. The number on the
/// row is what tells those apart, which is why `d194`'s ban on counting it
/// does not carry over -- that ban existed because a comb's *width* said it
/// exactly, and there is no comb any more.
#[test]
fn the_row_counts_the_fan_out_the_radius_can_no_longer_tell_apart() {
    let s = up("map-fan");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "A branch",
        "--why",
        "it needs its own children",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf beside it",
        "--why",
        "it stands alone",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf below the branch",
        "--why",
        "it hangs off the branch",
        "--parent",
        "2",
    ]);

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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);

    // g1 has two children, t2 has one, and the two leaves have none and
    // therefore no number at all.
    assert!(
        a.body.contains("<span class=\"fan\">2</span>"),
        "{}",
        a.body
    );
    assert_eq!(
        a.body.matches("<span class=\"fan\">1</span>").count(),
        1,
        "{}",
        a.body
    );
    assert_eq!(a.body.matches("class=\"fan\"").count(), 2, "{}", a.body);
}

/// DX pillar: state never rides on a colour alone. Each row spells its
/// state as a word in `title`, not only through its class and the fill of
/// its station.
#[test]
fn state_reaches_the_row_as_a_word_in_its_title_attribute_not_only_as_a_class() {
    let s = up("map-state-words");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "Still open",
        "--why",
        "nothing settled it yet",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "Already closed",
        "--why",
        "it shipped",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&["done", "3", "shipped it"]);
    s._sandbox.ok(&[
        "add",
        "Parked for later",
        "--why",
        "it is not due yet",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&["park", "4", "waiting on the release"]);

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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(
        a.body.contains("title=\"t2 · open · Still open\""),
        "{}",
        a.body
    );
    assert!(
        a.body.contains("title=\"t3 · closed · Already closed\""),
        "{}",
        a.body
    );
    assert!(
        a.body.contains("title=\"t4 · parked · Parked for later\""),
        "{}",
        a.body
    );
    // And the word is not the only cue either: the closed one is struck
    // through and its station is hollow.
    assert!(a.body.contains("<li class=\"stop shut\""), "{}", a.body);
}

/// The promise `d391` signs, over HTTP rather than through a unit test: the
/// detail of every node is already on the page when it arrives, so reading
/// one costs no second request. That is what "without letting go" means in
/// a browser -- and it is also why the CSP can keep saying `default-src
/// 'none'`, with no way for the page to talk to anything at all.
#[test]
fn the_detail_of_every_node_arrives_with_the_page() {
    let s = up("map-detail");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "A branch",
        "--why",
        "it needs its own children",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&["note", "2", "the first note"]);
    s._sandbox
        .ok(&["note", "2", "and the one that corrects it"]);

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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);

    assert!(
        a.body
            .contains("<script type=\"application/json\" id=\"map-data\">"),
        "{}",
        a.body
    );
    for why in ["it is the goal", "it needs its own children"] {
        assert!(
            a.body.contains(why),
            "{why} is not on the page:\n{}",
            a.body
        );
    }
    // `f389`: both notes travel, not only the last one written.
    assert!(a.body.contains("the first note"), "{}", a.body);
    assert!(
        a.body.contains("and the one that corrects it"),
        "{}",
        a.body
    );
    assert!(a.body.contains("2 notes"), "{}", a.body);
}

/// Folding is a link, so it has to survive the gate as well as the router:
/// the query is the one part of a URL the gate reads for itself, looking for
/// a boot key, and a fold arriving there must be served rather than treated
/// as a key that is missing.
#[test]
fn a_fold_travels_in_the_url_and_is_served_like_any_other_page() {
    let s = up("map-fold");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    s._sandbox.ok(&[
        "add",
        "A branch",
        "--why",
        "it needs its own children",
        "--parent",
        "1",
    ]);
    s._sandbox.ok(&[
        "add",
        "A leaf below the branch",
        "--why",
        "it hangs off the branch",
        "--parent",
        "2",
    ]);

    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let id = s
        ._sandbox
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let headers = [("Host", s.host()), ("X-Vivac-Token", token)];

    let whole = call(s.port(), &format!("/p/{id}/tree"), &headers);
    assert_eq!(whole.status, 200, "{}", whole.body);
    assert_eq!(whole.body.matches("<li class=\"stop").count(), 3);

    let folded = call(s.port(), &format!("/p/{id}/tree?fold=t2"), &headers);
    assert_eq!(folded.status, 200, "{}", folded.body);
    assert_eq!(
        folded.body.matches("<li class=\"stop").count(),
        2,
        "t2's own child should be off the page:\n{}",
        folded.body
    );
    assert!(
        folded.body.contains("<span class=\"held\">+1</span>"),
        "{}",
        folded.body
    );
    // And the control that undoes it is a link in the gutter, one per lane
    // width, so it works with no script at all.
    assert_eq!(folded.body.matches("class=\"fold on\"").count(), 2);
}

/// `WEB.md` §7.4: the whole map loads with no internet, the same as every
/// other page.
///
/// It is the first page with a drawing and a script in it, so this is also
/// the first time the promise means more than "no `<img>`". The assertion
/// used to be that the page carried no `<svg>` and no `<script>` at all,
/// which stopped saying anything the day it carried both. What it has to
/// say instead is that neither of them *reaches*: no `src`, no `@import`,
/// no `<use>`, and no absolute URL anywhere on the page.
#[test]
fn the_map_reaches_for_nothing_off_this_machine() {
    let s = up("tree-offline");
    s._sandbox
        .ok(&["push", "Ship the thing", "--why", "it is the goal"]);
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
        &format!("/p/{id}/tree"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(!a.body.contains("http://"), "{}", a.body);
    assert!(!a.body.contains("https://"), "{}", a.body);
    for reaching in ["src=", "@import", "<use", "<image", "url("] {
        assert!(
            !a.body.contains(reaching),
            "{reaching} would fetch something:\n{}",
            a.body
        );
    }
}

/// `f189`'s test, extended: the tree the front page links to has to be
/// reachable with nothing but the cookie a browser keeps on its own.
#[test]
fn a_browser_that_only_carries_the_cookie_can_walk_from_today_to_the_tree() {
    let s = up("tree-browsing");
    s._sandbox
        .ok(&["push", "Ship the thing", "--why", "it is the goal"]);
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

    let tree_link = hrefs_in(&today.body)
        .into_iter()
        .find(|h| h.contains("/tree"))
        .unwrap_or_else(|| {
            panic!(
                "no link to the tree on the Today page:
{}",
                today.body
            )
        });
    let a = call(s.port(), &tree_link, &[("Host", s.host()), ("Cookie", jar)]);
    assert_eq!(a.status, 200, "{} -> {}", tree_link, a.body);
}

// ---------------------------------------------------------------------------
// `d199`, `d200` and `d374`: the index of projects, and what a URL's `<id>`
// names. Everything below is about the two ways in and the answer to an
// ambiguous one.
// ---------------------------------------------------------------------------

/// The id the registry keys a project by, read the way the registry reads it:
/// the id of its first event. Not `config.project_id`, which `f266`
/// disqualified because a missing `config` is silently regenerated.
fn first_event_id(root: &std::path::Path) -> String {
    let log = std::fs::read_to_string(root.join(".vivac").join("events")).unwrap();
    let line = log.lines().next().expect("a tree with no events has no id");
    let at = line.find("\"id\":\"").expect("no id on the first event") + 6;
    let rest = &line[at..];
    rest[..rest.find('"').unwrap()].to_string()
}

/// Two projects whose directories carry the *same* name, which `Sandbox` will
/// not produce on its own because it keeps every name unique. They share one
/// `VIVAC_HOME`, so one registry holds both.
fn twins(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let home = std::env::temp_dir().join(format!("t-home-twins-{name}"));
    std::fs::create_dir_all(&home).unwrap();
    let mut roots = Vec::new();
    for side in ["a", "b"] {
        let d = std::env::temp_dir()
            .join(format!("t-twins-{name}-{side}"))
            .join("same-name");
        std::fs::create_dir_all(&d).unwrap();
        let ok = Command::new(BIN)
            .current_dir(&d)
            .env("VIVAC_HOME", &home)
            .args(["init"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "init failed in a twin");
        roots.push(d);
    }
    (home, roots[0].clone(), roots[1].clone())
}

/// `d199`: the server opens from a directory that is not a project at all.
///
/// This used to be impossible for a reason no error explained: `main` looked
/// for a tree at or above the cwd and returned `NoStore` before the `web`
/// branch was ever reached, so the command died in a directory it had no
/// business needing a tree in. That the server answers here at all is half of
/// what this asserts; the other half is that `/` is then the index, because
/// there is no "the project" to land on.
#[test]
fn started_outside_a_project_the_index_is_what_answers() {
    let one = Sandbox::new_seeded("outside-one");
    let two = Sandbox::new_seeded_in("outside-two", one.global_home());
    let server = Server::start_serving(
        &std::env::temp_dir(),
        one.global_home(),
        &[one.0.as_path(), two.0.as_path()],
    );
    let boot = call(server.port, &server.boot_path(), &[("Host", server.host())]);
    let token = token_from(&boot);
    let a = call(
        server.port,
        "/",
        &[("Host", server.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    let one_name = one.0.file_name().unwrap().to_string_lossy().into_owned();
    let two_name = two.0.file_name().unwrap().to_string_lossy().into_owned();
    assert!(a.body.contains(&one_name), "{}", a.body);
    assert!(a.body.contains(&two_name), "{}", a.body);
}

/// `d200`: the index is a page, not the bare `<ul>` it used to be.
///
/// The list of links had no stylesheet, no `<title>` and no viewport, which
/// made it the only surface here that was not one. It also said nothing a
/// `cd` did not already say, and the promise it now has to keep is that you
/// can see which project moved and which has been sitting still.
#[test]
fn the_index_is_a_page_and_carries_what_d200_admitted() {
    let one = Sandbox::new_seeded("index-fields-one");
    let two = Sandbox::new_seeded_in("index-fields-two", one.global_home());
    two.ok(&["push", "Ship the thing", "--why", "it is the goal"]);
    let server = Server::start_serving(
        &std::env::temp_dir(),
        one.global_home(),
        &[one.0.as_path(), two.0.as_path()],
    );
    let boot = call(server.port, &server.boot_path(), &[("Host", server.host())]);
    let token = token_from(&boot);
    let a = call(
        server.port,
        "/",
        &[("Host", server.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(a.body.contains("<title>"), "no title: {}", a.body);
    assert!(a.body.contains("<style>"), "no stylesheet: {}", a.body);
    assert!(
        a.body.contains("width=device-width"),
        "no viewport: {}",
        a.body
    );
    // The focus: one of `d200`'s four, and the one that says what a project
    // was doing when it stopped.
    assert!(a.body.contains("Ship the thing"), "no focus: {}", a.body);
    // The silence, which is the field the promise is actually about.
    assert!(a.body.contains("moved today"), "no silence: {}", a.body);
    // And the project with nothing in it says so rather than being blank.
    assert!(a.body.contains("no focus"), "{}", a.body);
}

/// `d374`: the permanent form of the URL opens the project.
#[test]
fn a_project_opens_by_its_permanent_id() {
    let s = up("permanent-id");
    s._sandbox
        .ok(&["push", "Root goal", "--why", "it is the goal"]);
    let ulid = first_event_id(&s._sandbox.0);
    let boot = call(s.port(), &s.boot_path(), &[("Host", s.host())]);
    let token = token_from(&boot);
    let a = call(
        s.port(),
        &format!("/p/{ulid}/"),
        &[("Host", s.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(a.body.contains("Root goal"), "{}", a.body);
}

/// `d374`: a name more than one project carries is answered with the choice,
/// never with a guess.
///
/// The `-2` this replaces made the second twin reachable at `same-name-2`,
/// and `f373` showed what that cost: the suffix was positional, so the first
/// twin leaving the registry handed its URL to the second and a saved link
/// opened the wrong tree without an error.
#[test]
fn a_name_two_projects_share_is_a_choice_and_not_a_guess() {
    let (home, a_root, b_root) = twins("choice");
    let server = Server::start_serving(
        &std::env::temp_dir(),
        &home,
        &[a_root.as_path(), b_root.as_path()],
    );
    let boot = call(server.port, &server.boot_path(), &[("Host", server.host())]);
    let token = token_from(&boot);
    let a = call(
        server.port,
        "/p/same-name/",
        &[("Host", server.host()), ("X-Vivac-Token", token)],
    );
    assert_eq!(a.status, 300, "{}", a.body);
    // The `-2` is gone: nothing here invents a second spelling.
    assert!(!a.body.contains("same-name-2"), "{}", a.body);
}

/// The security pillar allows a project's *name* across this boundary and
/// nothing else. A real path carries the name of whoever owns the machine,
/// so the page that has to tell two identically named projects apart is
/// exactly the one where a path would be easiest to reach for.
#[test]
fn the_choice_tells_them_apart_without_naming_a_path() {
    let (home, a_root, b_root) = twins("no-paths");
    let server = Server::start_serving(
        &std::env::temp_dir(),
        &home,
        &[a_root.as_path(), b_root.as_path()],
    );
    let boot = call(server.port, &server.boot_path(), &[("Host", server.host())]);
    let token = token_from(&boot);
    let a = call(
        server.port,
        "/p/same-name/",
        &[("Host", server.host()), ("X-Vivac-Token", token)],
    );
    for root in [&a_root, &b_root] {
        let parent = root.parent().unwrap().to_string_lossy().into_owned();
        assert!(
            !a.body.contains(&parent),
            "a path reached the page: {}",
            a.body
        );
        // The other spelling of the separator too: a fragment is a leak.
        assert!(
            !a.body.contains(&parent.replace('\\', "/")),
            "a path reached the page: {}",
            a.body
        );
    }
}
