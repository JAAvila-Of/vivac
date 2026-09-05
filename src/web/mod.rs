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
mod today;
mod tree;
mod why;

use crate::failure::{Failure, R};
use crate::project::Registry;
use gate::{Denial, Gate, Incoming, Verdict, SESSION_COOKIE};
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

/// `WEB.md` §5: embedded, because the CSP admits an inline `<style>` and no
/// external stylesheet at all, and in its own file because nobody maintains
/// two hundred lines of CSS inside a string literal. One skin for every
/// page: two stylesheets for one visual language are two places to diverge.
pub(crate) const WEB_CSS: &str = include_str!("web.css");

/// Everything that reaches a page goes through here first.
///
/// Not a nicety for this page's list of directory names: every surface that
/// comes after interpolates titles, reasons and notes, which is prose a
/// person wrote. A tree is allowed to hold a node called `<script>`, and the
/// place that decides it cannot execute is here, once, rather than every
/// call site remembering to.
pub(crate) fn escape(s: &str) -> String {
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

/// A node's alias, as a link to its lineage.
///
/// Every alias on every page is one, and that is `d147` made concrete: it
/// kept the tree only as the way you reach a lineage, so a node you can see
/// anywhere is a node you can ask "why" about. `f189` is why this is worth
/// stating -- a page full of links nobody can follow was green for a day.
pub(crate) fn alias_link(project: &str, alias: &str) -> String {
    let alias = escape(alias);
    format!(
        "<a href=\"/p/{p}/why/{alias}\">{alias}</a>",
        p = escape(project)
    )
}

/// What an admitted request is asking for. The gate has already said the
/// request may be answered; this says what with.
enum Route<'a> {
    /// `GET /` -- the index of projects.
    Index,
    /// `GET /p/<id>/` -- one project's Today page.
    Today(&'a str),
    /// `GET /p/<id>/why/<node>` -- one node's lineage, drawn (`d145`).
    Why(&'a str, &'a str),
    /// `GET /p/<id>/tree` -- the whole tree, drawn (`WEB.md` §3.6, `d194`).
    Tree(&'a str),
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
        // The trailing slash is optional on every path under a project and
        // never redirected: no page here links to the other spelling.
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        match rest.split_once('/') {
            None if !rest.is_empty() => return Route::Today(rest),
            Some((id, tail)) if !id.is_empty() => {
                // `why/<node>` and nothing deeper. A node id never contains
                // a slash, so anything that still does is not one.
                if let Some(node) = tail.strip_prefix("why/") {
                    if !node.is_empty() && !node.contains('/') {
                        return Route::Why(id, node);
                    }
                } else if tail == "tree" {
                    return Route::Tree(id);
                }
            }
            _ => {}
        }
    }
    Route::NotFound
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
    redirect_with(request, location, None)
}

/// Spending the boot key: hand over the session cookie and land the browser
/// where the work is (`d190`).
///
/// The flags are the defence and every one of them is load-bearing.
/// `SameSite=Strict` is what keeps another page in the same browser --
/// which `gate` names as the realistic attacker -- from having the cookie
/// ride along on a request it started. `HttpOnly` keeps it out of reach of
/// script. No `Max-Age` and no `Expires` make it a session cookie: it dies
/// with the browser, and the token it carries dies with this process
/// anyway. There is no `Secure`, because this is `http://127.0.0.1` and
/// `Secure` would stop the cookie being sent at all.
fn boot_redirect(request: tiny_http::Request, token: &str) {
    let jar = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict",
        SESSION_COOKIE
    );
    redirect_with(request, "/", Some(header("Set-Cookie", &jar)))
}

fn redirect_with(request: tiny_http::Request, location: &str, extra: Option<tiny_http::Header>) {
    let mut response = tiny_http::Response::from_string(String::new())
        .with_status_code(302)
        .with_header(header("Location", location));
    for h in security_headers() {
        response = response.with_header(h);
    }
    if let Some(h) = extra {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn handle(gate: &mut Gate, registry: &mut Registry, request: tiny_http::Request) {
    let path = request.url().to_string();
    let host = header_value(request.headers(), "host").map(str::to_string);
    let origin = header_value(request.headers(), "origin").map(str::to_string);
    let token = header_value(request.headers(), "x-vivac-token").map(str::to_string);
    let cookie = header_value(request.headers(), "cookie").map(str::to_string);
    let incoming = Incoming {
        path: &path,
        host: host.as_deref(),
        origin: origin.as_deref(),
        token: token.as_deref(),
        cookie: cookie.as_deref(),
    };
    match gate.admit(&incoming) {
        Verdict::Boot => boot_redirect(request, gate.token()),
        Verdict::Serve => match route(&path) {
            Route::Index => match registry.projects() {
                [one] => redirect(request, &format!("/p/{}/", one.id)),
                many => respond(request, 200, HTML, today::index_page(many)),
            },
            Route::Today(id) => match registry.by_id(id) {
                Some(project) => {
                    // Cloned before the refresh below borrows the project
                    // mutably, which is the same dance `mcp` does.
                    let name = project.name.clone();
                    let key = project.id.clone();
                    match project.current_with_log() {
                        Ok((ctx, log)) => {
                            let page = today::today_page(&key, &name, &ctx.tree, log);
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
            // The lineage of one node (`WEB.md` §3.2). Same dance as
            // `Today` above, and the same reason for saying nothing in the
            // body when the store cannot be read.
            Route::Why(id, node) => match registry.by_id(id) {
                Some(project) => {
                    let name = project.name.clone();
                    let key = project.id.clone();
                    match project.current_with_log() {
                        Ok((ctx, log)) => match why::why_page(&key, &name, &ctx.tree, log, node) {
                            Some(page) => respond(request, 200, HTML, page),
                            // A node this tree does not hold. The id it did
                            // not recognise does not come back in the
                            // answer, exactly as an unknown project's does
                            // not.
                            None => respond(request, 404, TEXT, "not found\n".to_string()),
                        },
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
            // The whole tree, drawn (`WEB.md` §3.6). Same dance as `Today`
            // above, and the same reason for saying nothing in the body
            // when the store cannot be read.
            Route::Tree(id) => match registry.by_id(id) {
                Some(project) => {
                    let name = project.name.clone();
                    let key = project.id.clone();
                    match project.current() {
                        Ok(ctx) => {
                            respond(request, 200, HTML, tree::tree_page(&key, &name, &ctx.tree))
                        }
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
    use super::{escape, route, Route};

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

    /// The write path (`WEB.md` §4) is the one surface `d145` reserved a URL
    /// for and nobody has built yet. Until it exists it is a 404, not an
    /// empty page.
    ///
    /// This test used to name `why/3` as the unbuilt one. It stopped being
    /// unbuilt, and then so did `tree` (`WEB.md` §3.6).
    #[test]
    fn a_path_under_a_project_that_does_not_exist_yet_is_not_found() {
        assert!(matches!(route("/p/vivac/op/push"), Route::NotFound));
    }

    #[test]
    fn a_lineage_routes_under_its_project() {
        assert!(matches!(
            route("/p/vivac/why/f4"),
            Route::Why("vivac", "f4")
        ));
        assert!(matches!(
            route("/p/vivac/why/f4/"),
            Route::Why("vivac", "f4")
        ));
    }

    /// `WEB.md` §3.6: the global graph routes with or without the trailing
    /// slash, the same as every other path under a project.
    #[test]
    fn a_tree_routes_under_its_project() {
        assert!(matches!(route("/p/vivac/tree"), Route::Tree("vivac")));
        assert!(matches!(route("/p/vivac/tree/"), Route::Tree("vivac")));
    }

    #[test]
    fn a_lineage_path_with_no_node_on_it_is_not_found() {
        assert!(matches!(route("/p/vivac/why/"), Route::NotFound));
        assert!(matches!(route("/p/vivac/why"), Route::NotFound));
        assert!(matches!(route("/p//why/f4"), Route::NotFound));
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
}
