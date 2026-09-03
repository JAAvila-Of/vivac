//! `vivac web` — the tree served over HTTP, to a browser on this machine and
//! no other.
//!
//! `d127`/`d141`: the web is the main face and the defenses come first, with
//! a test per case, before a single page exists to attack. `mod gate` is
//! that layer, written to know nothing about a socket so its denials are
//! unit tests rather than tests that first have to stand up a server. This
//! module is the socket: it binds one, reads the handful of headers `Gate`
//! needs, and turns a `Verdict` into an HTTP response.
//!
//! **No CORS, anywhere.** Every response below carries its own set of
//! security headers and none of them is `Access-Control-Allow-*`. A page
//! served from another origin gets nothing back it can read, and `OPTIONS`
//! is answered exactly like any other method -- no preflight is ever
//! satisfied, which is what makes the missing `Access-Control-Allow-*`
//! actually matter.
//!
//! **The index is routing, not a surface.** It only ever redirects to one
//! project or lists which ones there are; the real page -- `WEB.md` §3.1,
//! the Today page a project's `id` routes to -- comes once this layer is in
//! place and proven.

mod gate;

use crate::changes::{self, Boundary, Changed};
use crate::event::{Event, State};
use crate::failure::{Failure, R};
use crate::model::{Node, Tree};
use crate::project::Registry;
use gate::{Denial, Gate, Incoming, Verdict};
use std::path::PathBuf;

const HTML: &str = "text/html; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";

/// `WEB.md` §4.1: no inline network resource loads, no framing, no form
/// submission anywhere else, and nothing sniffs the body into something it
/// is not.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; \
                    img-src data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'";

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("a header built from a literal name and an ASCII value")
}

/// The value of one header, matched by name without regard to case -- HTTP
/// never promises what case a client sends one in.
fn header_value<'a>(headers: &'a [tiny_http::Header], name: &'static str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// Everything that reaches a page goes through here first.
///
/// Not a nicety for this page's list of directory names: every surface that
/// comes after interpolates titles, reasons and notes, which is prose a
/// person wrote. A tree is allowed to hold a node called `<script>`, and the
/// place that decides it cannot execute is here, once, rather than every
/// call site remembering to.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn boot_page(token: &str) -> String {
    let token = escape(token);
    format!(
        "<!doctype html>\n\
         <html><head><meta charset=\"utf-8\">\n\
         <meta name=\"vivac-token\" content=\"{token}\"></head>\n\
         <body>vivac is listening.</body></html>\n"
    )
}

/// What an admitted request is asking for. The gate has already said the
/// request may be answered; this says what with.
enum Route<'a> {
    /// `GET /` -- the index of projects.
    Index,
    /// `GET /p/<id>/` -- one project's Today page.
    Today(&'a str),
    NotFound,
}

/// Same shape as `Gate::admit`, and the same reason (`d149`): its cases are
/// unit tests, not tests that first have to stand up a socket.
///
/// **No percent-decoding.** An `id` is sanitized to a character set that
/// never needs it, so a path that still carries a `%` simply matches no
/// project and falls through to `NotFound`. Writing a decoder would add a
/// parser to the one path security is watching, which is exactly what
/// `d138` refused to do for headers.
fn route(path: &str) -> Route<'_> {
    let path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
    if path == "/" {
        return Route::Index;
    }
    if let Some(rest) = path.strip_prefix("/p/") {
        // The trailing slash of `/p/<id>/` is optional and never redirected:
        // no page here links to the other spelling.
        let id = rest.strip_suffix('/').unwrap_or(rest);
        if !id.is_empty() && !id.contains('/') {
            return Route::Today(id);
        }
    }
    Route::NotFound
}

/// The index: a link per project, the `name` as its text and the `id` in
/// its `href`. Reached only with two or more projects -- with exactly one,
/// `handle` redirects there instead of listing it (`d145`).
fn index_page(projects: &[crate::project::Project]) -> String {
    let items: String = projects
        .iter()
        .map(|p| {
            format!(
                "<li><a href=\"/p/{}/\">{}</a></li>\n",
                escape(&p.id),
                escape(&p.name)
            )
        })
        .collect();
    format!(
        "<!doctype html>\n\
         <html><head><meta charset=\"utf-8\"></head>\n\
         <body><ul>\n{items}</ul></body></html>\n"
    )
}

/// `WEB.md` §5: embedded, because the CSP admits an inline `<style>` and no
/// external stylesheet at all, and in its own file because nobody maintains
/// two hundred lines of CSS inside a string literal.
const TODAY_CSS: &str = include_str!("today.css");

/// One node as a row: its alias, its title, and at most one line under them.
///
/// Everything here goes through `escape`. A title is prose somebody wrote,
/// and a tree is allowed to hold a node called `<script>`.
fn row(n: &Node, note: &str, note_class: &str) -> String {
    let mut s = format!(
        "<li><span class=\"alias\">{}</span><p class=\"title\">{}</p>",
        escape(&n.alias()),
        escape(&n.title)
    );
    if !note.is_empty() {
        let class = if note_class.is_empty() {
            "note".to_string()
        } else {
            format!("note {note_class}")
        };
        s.push_str(&format!("<p class=\"{class}\">{}</p>", escape(note)));
    }
    s.push_str("</li>\n");
    s
}

/// A named list, or nothing at all when there is nothing in it. An empty
/// heading is a line that says only that you have to read on.
fn group(title: &str, rows: String) -> String {
    if rows.is_empty() {
        return String::new();
    }
    format!("<h3>{title}</h3>\n<ul class=\"nodes\">\n{rows}</ul>\n")
}

/// Where the stretch is measured from, in a sentence. The facts come from
/// the same `Boundary` the CLI prints; only the wording is this page's.
fn since_line(since: &Boundary, stops: usize) -> String {
    match since {
        Boundary::Beginning {
            asked_for_manual: false,
        } => "Since the beginning: no stops yet.".to_string(),
        Boundary::Beginning {
            asked_for_manual: true,
        } => "Since the beginning: no stop here was made by hand.".to_string(),
        Boundary::Stop { vivac, .. } => {
            let date = crate::clock::date_of(&vivac.ts);
            let tail = match stops {
                0 => String::new(),
                1 => ", 1 stop since".to_string(),
                n => format!(", {n} stops since"),
            };
            format!(
                "Since {}, the last stop you made, {date}{tail}.",
                escape(&vivac.alias())
            )
        }
    }
}

/// The block this page exists for. It is rendered **whether or not anything
/// moved**: the promise is that you find out without having had to ask, and
/// a section that vanishes when the answer is "nothing" is that question put
/// straight back.
fn moved_section(changed: &Changed) -> String {
    let mut body = String::new();
    body.push_str(&group(
        "Opened",
        changed.opened.iter().map(|n| row(n, "", "")).collect(),
    ));
    body.push_str(&group(
        "Closed",
        changed
            .closed
            .iter()
            .map(|c| {
                let note = if c.forced && c.outcome.is_empty() {
                    "forced".to_string()
                } else if c.forced {
                    format!("forced: {}", c.outcome)
                } else {
                    c.outcome.clone()
                };
                row(c.node, &note, if c.forced { "forced" } else { "" })
            })
            .collect(),
    ));
    body.push_str(&group(
        "Flagged",
        changed
            .flagged
            .iter()
            .map(|f| {
                let note = if f.reason.is_empty() {
                    f.flag.word().to_string()
                } else {
                    format!("{}: {}", f.flag.word(), f.reason)
                };
                row(f.node, &note, "flag")
            })
            .collect(),
    ));
    body.push_str(&group(
        "Moved",
        changed
            .moved
            .iter()
            .map(|m| row(m.node, m.state.word(m.node.kind), ""))
            .collect(),
    ));

    if let Some(tail) = changes::tail_phrase(&changed.tail) {
        body.push_str(&format!("<p class=\"note\">{}</p>\n", escape(&tail)));
    }
    if body.is_empty() {
        body.push_str("<p class=\"empty\">Nothing has moved.</p>\n");
    }

    format!(
        "<section id=\"moved\">\n<h2>What moved</h2>\n<p class=\"since\">{}</p>\n{body}</section>\n",
        since_line(&changed.since, changed.tail.stops)
    )
}

/// The stack, top to bottom, with the focus marked by a word and not only by
/// a rule beside it: the DX pillar does not allow a meaning that only a
/// colour or a border carries.
fn stack_section(tree: &Tree) -> String {
    let stack: Vec<&Node> = tree.stack.iter().filter_map(|id| tree.node(id)).collect();
    if stack.is_empty() {
        return "<section id=\"focus\">\n<h2>Where you are</h2>\n\
                <p class=\"empty\">Empty stack.</p>\n</section>\n"
            .to_string();
    }
    let last = stack.len() - 1;
    let items: String = stack
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let here = if i == last {
                "<span class=\"here-mark\">you are here</span>"
            } else {
                ""
            };
            format!(
                "<li{}><span class=\"alias\">{}</span><p class=\"title\">{}{here}</p></li>\n",
                if i == last { " class=\"here\"" } else { "" },
                escape(&n.alias()),
                escape(&n.title)
            )
        })
        .collect();
    format!(
        "<section id=\"focus\">\n<h2>Where you are</h2>\n<ol class=\"stack\">\n{items}</ol>\n</section>\n"
    )
}

/// What reaches this point: the standing decisions and the invariants, both
/// picked by the same functions the brief picks them with (`WEB.md` §2).
fn governs_section(tree: &Tree) -> String {
    let focus = tree.focus();
    let lineage: Vec<&Node> = focus.map(|f| tree.ancestors(&f.id)).unwrap_or_default();
    let on_lineage: std::collections::HashSet<&str> =
        lineage.iter().map(|n| n.id.as_str()).collect();

    let decisions: String = match focus {
        Some(f) => crate::brief::standing(tree, f, &on_lineage)
            .iter()
            .map(|n| row(n, "", ""))
            .collect(),
        // No focus, no path, so nothing reaches "this point" except what
        // governs the whole project -- which is what the invariants below
        // already carry. The brief answers the same way.
        None => String::new(),
    };
    let invariants: String = crate::brief::constraints(tree, &lineage)
        .iter()
        .map(|n| row(n, if n.flags.is_empty() { "" } else { "at risk" }, "flag"))
        .collect();

    let mut body = String::new();
    body.push_str(&group("Standing decisions", decisions));
    body.push_str(&group("Invariants", invariants));
    if body.is_empty() {
        body.push_str("<p class=\"empty\">Nothing governs this point yet.</p>\n");
    }
    format!("<section id=\"governs\">\n<h2>What governs this point</h2>\n{body}</section>\n")
}

/// The product's differentiator, and it only ever has content if parking
/// costs what popping costs.
fn parked_section(tree: &Tree) -> String {
    let mut ps: Vec<&Node> = tree
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();
    ps.sort_by_key(|n| n.num);
    let body = if ps.is_empty() {
        "<p class=\"empty\">Nothing parked.</p>\n".to_string()
    } else {
        format!(
            "<ul class=\"nodes\">\n{}</ul>\n",
            ps.iter()
                .map(|n| row(n, &n.outcome, ""))
                .collect::<String>()
        )
    };
    format!("<section id=\"parked\">\n<h2>Do not touch now</h2>\n{body}</section>\n")
}

/// `WEB.md` §3.1 -- Today.
///
/// > You find out what moved while you were not looking **without having had
/// > to ask whether anything did**.
///
/// The four blocks come in the order that sentence implies rather than the
/// brief's: what moved is why this page exists, and a page whose reason for
/// existing sits below the fold does not keep it.
///
/// Every one of the four is answered by a function the CLI calls too. This
/// page picks no nodes of its own.
fn today_page(name: &str, tree: &Tree, log: &[Event]) -> String {
    let boundary = changes::manual_boundary(tree);
    let mut changed = changes::collect(tree, log, boundary.seq());
    changed.since = boundary;

    format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Today - {name_t}</title>\n\
         <style>\n{TODAY_CSS}</style></head>\n\
         <body><div class=\"page\">\n\
         <header><h1>{name_t}</h1>\n\
         <p class=\"promise\">What moved while you were not looking.</p></header>\n\
         <main>\n{moved}{focus}{governs}{parked}</main>\n\
         <footer>The same reading in a terminal: \
         <code>vivac changes --since manual</code></footer>\n\
         </div></body></html>\n",
        name_t = escape(name),
        moved = moved_section(&changed),
        focus = stack_section(tree),
        governs = governs_section(tree),
        parked = parked_section(tree),
    )
}

/// The headers every response carries, `Location` on a redirect included.
fn security_headers() -> [tiny_http::Header; 4] {
    [
        header("Content-Security-Policy", CSP),
        header("X-Content-Type-Options", "nosniff"),
        header("Referrer-Policy", "no-referrer"),
        header("Cache-Control", "no-store"),
    ]
}

fn respond(request: tiny_http::Request, status: u16, content_type: &str, body: String) {
    let mut response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type));
    for h in security_headers() {
        response = response.with_header(h);
    }
    // A client that closed the connection before the answer arrived is not
    // this server's failure to report.
    let _ = request.respond(response);
}

/// `302` to a project's Today page. `d145`: an index with exactly one
/// project does not make anybody click through it.
fn redirect(request: tiny_http::Request, location: &str) {
    let mut response = tiny_http::Response::from_string(String::new())
        .with_status_code(302)
        .with_header(header("Location", location));
    for h in security_headers() {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn handle(gate: &mut Gate, registry: &mut Registry, request: tiny_http::Request) {
    let path = request.url().to_string();
    let host = header_value(request.headers(), "host").map(str::to_string);
    let origin = header_value(request.headers(), "origin").map(str::to_string);
    let token = header_value(request.headers(), "x-vivac-token").map(str::to_string);
    let incoming = Incoming {
        path: &path,
        host: host.as_deref(),
        origin: origin.as_deref(),
        token: token.as_deref(),
    };
    match gate.admit(&incoming) {
        Verdict::Boot => respond(request, 200, HTML, boot_page(gate.token())),
        Verdict::Serve => match route(&path) {
            Route::Index => match registry.projects() {
                [one] => redirect(request, &format!("/p/{}/", one.id)),
                many => respond(request, 200, HTML, index_page(many)),
            },
            Route::Today(id) => match registry.by_id(id) {
                Some(project) => {
                    // Cloned before the refresh below borrows the project
                    // mutably, which is the same dance `mcp` does.
                    let name = project.name.clone();
                    match project.current_with_log() {
                        Ok((ctx, log)) => {
                            let page = today_page(&name, &ctx.tree, log);
                            respond(request, 200, HTML, page)
                        }
                        // The store is on disk and this process is not its
                        // only writer, so a read can fail between one request
                        // and the next. The reason does not go in the body:
                        // an io error carries the path it failed on, and a
                        // path is the one thing the security pillar says
                        // never leaves this machine's own head.
                        Err(_) => respond(
                            request,
                            500,
                            TEXT,
                            "the store could not be read\n".to_string(),
                        ),
                    }
                }
                None => respond(request, 404, TEXT, "not found\n".to_string()),
            },
            Route::NotFound => respond(request, 404, TEXT, "not found\n".to_string()),
        },
        Verdict::Deny(Denial::ForeignHost) | Verdict::Deny(Denial::ForeignOrigin) => {
            respond(request, 403, TEXT, "forbidden\n".to_string())
        }
        Verdict::Deny(Denial::NoValidToken) => respond(
            request,
            401,
            TEXT,
            "no session. run: vivac web\n".to_string(),
        ),
    }
}

/// Opens the system browser on `url`. Failing is not an error: the caller
/// already printed the same URL, so a browser that does not open costs the
/// user one copy and paste, not the session.
///
/// The Windows branch hands the URL to `cmd`, which parses it again after
/// this process has finished quoting it. That is safe for exactly one
/// reason: `Gate::boot_url` builds `http://127.0.0.1:<port>/?k=<hex>` and
/// nothing else, so the string carries no `&` for `cmd` to read as a
/// separator. A second query parameter would break that, and would have to
/// reach the browser some other way.
fn open_browser(url: &str) {
    let launched = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    let _ = launched;
}

/// Binds `127.0.0.1` -- and nothing else; there is no flag for another
/// address -- serves `roots`, and blocks until the process is killed.
pub fn serve(roots: Vec<PathBuf>, port: Option<u16>, open: bool) -> R {
    let server = tiny_http::Server::http(("127.0.0.1", port.unwrap_or(0)))
        .map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    let bound_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| Failure::usage("vivac web needs a TCP address to bind"))?;

    // The port is not known until after the bind when it was ephemeral, and
    // the gate's `Host`/`Origin` checks are pinned to it.
    let mut gate = Gate::new(bound_port)?;
    let mut registry = Registry::open(roots)?;

    let url = gate.boot_url();
    println!("  vivac web listening on http://127.0.0.1:{bound_port}");
    println!("  open this to start a session: {url}");
    if open {
        open_browser(&url);
    }

    // One thread, on purpose and not as a shortcut: the page loads no
    // external resource (see the module doc), so one page load is exactly
    // one request, and there is exactly one user of this process.
    loop {
        let request = server.recv().map_err(Failure::Io)?;
        handle(&mut gate, &mut registry, request);
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, route, today_page, Route};
    use crate::event::{Body, Event, Kind, VivacKind};
    use crate::model::fold;

    #[test]
    fn the_characters_that_can_change_a_page_are_escaped() {
        assert_eq!(
            escape("<b>a & \"b\" 'c'</b>"),
            "&lt;b&gt;a &amp; &quot;b&quot; &#39;c&#39;&lt;/b&gt;"
        );
    }

    #[test]
    fn text_with_nothing_to_escape_comes_back_whole() {
        assert_eq!(escape("vivac-project"), "vivac-project");
    }

    #[test]
    fn a_node_title_cannot_close_the_tag_it_sits_in() {
        assert!(!escape("</li><script>alert(1)</script>").contains('<'));
    }

    #[test]
    fn the_root_path_routes_to_the_index() {
        assert!(matches!(route("/"), Route::Index));
    }

    #[test]
    fn a_query_string_does_not_change_where_the_root_routes() {
        assert!(matches!(route("/?k=abc"), Route::Index));
    }

    #[test]
    fn a_projects_today_page_routes_with_or_without_a_trailing_slash() {
        assert!(matches!(route("/p/vivac/"), Route::Today("vivac")));
        assert!(matches!(route("/p/vivac"), Route::Today("vivac")));
    }

    #[test]
    fn a_path_under_a_project_that_does_not_exist_yet_is_not_found() {
        assert!(matches!(route("/p/vivac/why/3"), Route::NotFound));
    }

    #[test]
    fn an_empty_id_is_not_found() {
        assert!(matches!(route("/p/"), Route::NotFound));
    }

    #[test]
    fn a_percent_encoded_id_is_not_decoded_and_so_matches_nothing_real() {
        assert!(matches!(route("/p/%76ivac/"), Route::Today("%76ivac")));
    }

    #[test]
    fn an_unrecognised_path_is_not_found() {
        assert!(matches!(route("/other"), Route::NotFound));
    }

    /// Four events are enough for a page: a goal to stand on, a stop to
    /// measure from, and a node born after it.
    fn ev(seq: u64, payload: Body) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload,
        }
    }

    fn born(seq: u64, num: u64, title: &str, parent: Option<&str>) -> Event {
        ev(
            seq,
            Body::NodeCreated {
                node: format!("n{num}"),
                num,
                kind: Kind::Goal,
                title: title.to_string(),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
            },
        )
    }

    fn stop(seq: u64, num: u64, kind: VivacKind) -> Event {
        ev(
            seq,
            Body::VivacCreated {
                vivac: format!("v{num}"),
                num,
                kind,
                stack: vec![],
                working_set: vec![],
                next_intent: String::new(),
                anchor: crate::anchor::AnchorRef::default(),
                node_ref: None,
                label: String::new(),
            },
        )
    }

    /// The one rule the whole page rests on: a title is prose somebody
    /// wrote, and a tree is allowed to hold a node called `<script>`.
    #[test]
    fn a_title_that_looks_like_markup_reaches_the_page_escaped() {
        let events = vec![born(1, 1, "<script>alert(1)</script>", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        assert!(!page.contains("<script>alert(1)"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
    }

    /// The order is the page's argument. What moved is why this page exists,
    /// so it comes before the three blocks that say where you are.
    #[test]
    fn what_moved_comes_before_the_rest() {
        let events = vec![born(1, 1, "A goal", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        let at = |id: &str| page.find(id).unwrap_or_else(|| panic!("no {id}:\n{page}"));
        assert!(at("id=\"moved\"") < at("id=\"focus\""), "{page}");
        assert!(at("id=\"focus\"") < at("id=\"governs\""), "{page}");
        assert!(at("id=\"governs\"") < at("id=\"parked\""), "{page}");
    }

    /// The promise is that you find out **without having had to ask whether
    /// anything did**. A block that disappears when the answer is "nothing"
    /// is that question put straight back, so it is rendered either way.
    #[test]
    fn the_moved_block_is_there_even_when_nothing_moved() {
        let events = vec![born(1, 1, "A goal", None), stop(2, 1, VivacKind::Manual)];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        assert!(page.contains("id=\"moved\""), "{page}");
        assert!(page.contains("the last stop you made"), "{page}");
        assert!(page.contains("Nothing has moved."), "{page}");
    }

    /// The boundary is the last stop **made by hand**, the same one
    /// `changes --since manual` measures from. A stop the hook wrote sits
    /// inside the stretch and does not end it.
    #[test]
    fn the_boundary_is_the_last_stop_made_by_hand() {
        let events = vec![
            born(1, 1, "A goal", None),
            stop(2, 1, VivacKind::Manual),
            born(3, 2, "Born after the stop", Some("n1")),
            stop(4, 2, VivacKind::Auto),
        ];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        assert!(page.contains("Born after the stop"), "{page}");
        assert!(page.contains("1 stop since"), "{page}");
    }

    /// DX pillar: a meaning never rides on a colour or a rule alone. Every
    /// state the page shows is also a word on the page.
    #[test]
    fn a_state_is_carried_by_a_word_and_not_only_by_a_class() {
        let events = vec![
            born(1, 1, "A goal", None),
            born(2, 2, "Parked work", Some("n1")),
            ev(
                3,
                Body::StateChanged {
                    node: "n2".to_string(),
                    state: crate::event::State::Suspended,
                    outcome: "waiting on day 14".to_string(),
                    forced: false,
                },
            ),
        ];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        assert!(page.contains("Do not touch now"), "{page}");
        assert!(page.contains("waiting on day 14"), "{page}");
        assert!(page.contains("parked"), "{page}");
    }

    /// `WEB.md` §7.4: the page loads with no internet at all. Nothing here
    /// may reach past `127.0.0.1`, and the cheapest proof is that no absolute
    /// url is written into it in the first place.
    #[test]
    fn the_page_asks_for_nothing_off_this_machine() {
        let events = vec![born(1, 1, "A goal", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", &tree, &events);
        assert!(!page.contains("http://"), "{page}");
        assert!(!page.contains("https://"), "{page}");
        assert!(!page.contains("//fonts."), "{page}");
    }
}
