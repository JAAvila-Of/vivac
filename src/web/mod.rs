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
//! **`/` always answers the index** (`d809`). Where a session starts is
//! decided once, when the boot key is spent: on the Today page of the
//! project the server was started in, or on the index when it was started
//! anywhere else. After that the index is one link away from every Today
//! page, so no working directory can take it out of reach.

mod gate;
mod map;
mod today;
mod why;

use crate::failure::{Failure, R};
use crate::output::{flush, outln};
use crate::project::{Located, Named, Registry};
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

/// The tab's icon: the logo's mark, the map's rail from a parent to its
/// child, in a heavier cut than `docs/img/mark-light.svg` because a tab
/// draws it at sixteen pixels. It rides inside every page as a `data:`
/// image, which the CSP already admits, so there is no route to serve it
/// and no second request behind the gate. The colours are the skin's ink
/// and accent, and the SVG switches them itself with the tab's theme.
///
/// Base64 and not the SVG as text: an SVG has to name its namespace, and
/// the namespace is a URL. Nothing is ever fetched from it, but the tests
/// that hold every page to reaching for nothing off this machine read
/// `http://` as exactly that, and they should stay that strict. The text
/// this decodes to is `FAVICON_SVG` in the tests below, which check it.
pub(crate) const FAVICON: &str = concat!(
    "<link rel=\"icon\" type=\"image/svg+xml\" href=\"data:image/svg+xml;base64,",
    "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjUgNSA1",
    "NCA1NCI+PHN0eWxlPi5pe2ZpbGw6IzE2MTgxZH0ucntzdHJva2U6IzJmNWQ1MH1AbWVkaWEg",
    "KHByZWZlcnMtY29sb3Itc2NoZW1lOmRhcmspey5pe2ZpbGw6I2U2ZTVlMX0ucntzdHJva2U6",
    "IzdmYmZhOX19PC9zdHlsZT48cGF0aCBjbGFzcz0iciIgZD0iTTE3IDE3VjM2UTE3IDQ3IDI4",
    "IDQ3SDQ2IiBmaWxsPSJub25lIiBzdHJva2Utd2lkdGg9IjcuNSIgc3Ryb2tlLWxpbmVjYXA9",
    "InJvdW5kIi8+PGNpcmNsZSBjbGFzcz0iaSIgY3g9IjE3IiBjeT0iMTYiIHI9IjguNSIvPjxj",
    "aXJjbGUgY2xhc3M9ImkiIGN4PSI0OCIgY3k9IjQ3IiByPSI4LjUiLz48L3N2Zz4=",
    "\">"
);

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
    /// `GET /p/<id>/tree` -- the whole tree, drawn (`WEB.md` §3.6, `d391`).
    /// The query rides along: it is where the map carries what the reader
    /// has folded away, so a view of the tree is a URL and nothing else.
    Tree(&'a str, &'a str),
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
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path, ""),
    };
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
                    return Route::Tree(id, query);
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

/// Spending the boot key: hand over the session cookie and land the browser
/// on the landing project's Today page when there is one, `/` when there is
/// none (`d809`, refining `d190`/`d145`: `/` itself always answers the
/// index now -- see `Route::Index` below -- so this is the one place left
/// that skips the click-through an index of a single project never needed,
/// and the only way a working directory inside a project still lands you on
/// it rather than on the list).
///
/// The flags are the defence and every one of them is load-bearing.
/// `SameSite=Strict` is what keeps another page in the same browser --
/// which `gate` names as the realistic attacker -- from having the cookie
/// ride along on a request it started. `HttpOnly` keeps it out of reach of
/// script. No `Max-Age` and no `Expires` make it a session cookie: it dies
/// with the browser, and the token it carries dies with this process
/// anyway. There is no `Secure`, because this is `http://127.0.0.1` and
/// `Secure` would stop the cookie being sent at all.
fn boot_redirect(request: tiny_http::Request, token: &str, landing: Option<&str>) {
    let jar = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict",
        SESSION_COOKIE
    );
    let location = match landing {
        Some(id) => format!("/p/{id}/"),
        None => "/".to_string(),
    };
    redirect_with(request, &location, Some(header("Set-Cookie", &jar)))
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

/// `d810`: the same sentence `find --everywhere` prints for the same shape
/// of answer (`render.rs`'s own `find_everywhere`), so a person who meets
/// both surfaces reads one wording rather than two for "a root the registry
/// named did not open".
pub(crate) fn unreachable_sentence(names: &[String]) -> String {
    format!(
        "{} project{} unreachable: {}",
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        names.join(", ")
    )
}

/// The two answers that are not "one project", shared by the three routes
/// under `/p/<id>/` so each of them only has to say what it does with the
/// one it got (`d374`).
///
/// An id the registry does not carry is a 404 and the id does not come back
/// in the body. A name more than one root carries is `300 Multiple Choices`,
/// which is the one status that means exactly this: the server understood,
/// and is handing the choice back rather than making it. The status matters
/// beyond politeness here -- one of this product's two audiences reads codes,
/// not pages, and a 200 would tell it the question had been answered.
fn not_one(request: tiny_http::Request, registry: &mut Registry, named: Named) {
    match named {
        Named::Ambiguous(which) => {
            let page = today::choose_page(registry.all(), &which);
            respond(request, 300, HTML, page)
        }
        _ => respond(
            request,
            404,
            TEXT,
            "not found
"
            .to_string(),
        ),
    }
}

fn handle(
    gate: &mut Gate,
    registry: &mut Registry,
    landing: Option<&str>,
    request: tiny_http::Request,
) {
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
        Verdict::Boot => boot_redirect(request, gate.token(), landing),
        Verdict::Serve => match route(&path) {
            // `d809`: `/` always answers the index -- with a landing project
            // or without one, with one project or with many. It used to hand
            // out `d199`'s redirect instead, whenever the working directory
            // sat inside a project, and that made the index unreachable for
            // the rest of the session: there was no page left that linked to
            // it. The redirect a landing project earns did not go away; it
            // moved to `boot_redirect` above, which is spent once per
            // session rather than run on every request.
            Route::Index => {
                let unreachable = registry.unreachable().to_vec();
                let page = today::index_page(registry.all(), &unreachable);
                respond(request, 200, HTML, page)
            }
            Route::Today(id) => match registry.named(id) {
                Named::One(i) => {
                    let project = registry.at(i);
                    // Cloned before the refresh below borrows the project
                    // mutably, which is the same dance `mcp` does.
                    let name = project.name.clone();
                    let key = id.to_string();
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
                other => not_one(request, registry, other),
            },
            // The lineage of one node (`WEB.md` §3.2). Same dance as
            // `Today` above, and the same reason for saying nothing in the
            // body when the store cannot be read.
            Route::Why(id, node) => match registry.named(id) {
                Named::One(i) => {
                    let project = registry.at(i);
                    let name = project.name.clone();
                    let key = id.to_string();
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
                other => not_one(request, registry, other),
            },
            // The whole tree, as a map (`WEB.md` §3.6). Same dance as `Today`
            // above, and the same reason for saying nothing in the body
            // when the store cannot be read.
            Route::Tree(id, query) => match registry.named(id) {
                Named::One(i) => {
                    let project = registry.at(i);
                    let name = project.name.clone();
                    let key = id.to_string();
                    match project.current() {
                        Ok(ctx) => respond(
                            request,
                            200,
                            HTML,
                            map::map_page(&key, &name, &ctx.tree, query),
                        ),
                        Err(_) => respond(
                            request,
                            500,
                            TEXT,
                            "the store could not be read\n".to_string(),
                        ),
                    }
                }
                other => not_one(request, registry, other),
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
/// address -- serves `required` and `optional` (`d810`: the same kind of
/// root, told apart only by what a failure to open one costs), and blocks
/// until the process is killed. `cwd_located` is what `store::locate`
/// answered for the working directory, if it sits inside a project at all.
/// Its `root` decides where the boot key lands you (`d809`, refining
/// `d199`) -- `/p/<id>/` for the project it names, `/` when there is none --
/// and nothing else: `/` itself always answers the index now, so a working
/// directory inside a project no longer costs the rest of the session its
/// way back to it. Handed to `Registry::open` whole, which is what lets it
/// sign as that lane only for that one project, never for the others
/// `required`/`optional` may also name (`t594`, twice: the second time
/// before `Registry::open` reached this far).
pub fn serve(
    required: Vec<PathBuf>,
    optional: Vec<PathBuf>,
    cwd_located: Option<Located>,
    port: Option<u16>,
    open: bool,
) -> R {
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
    let cwd_root = cwd_located.as_ref().map(|l| l.root.clone());
    let here = cwd_located.map(|l| {
        let root = l.root.clone();
        (root, l)
    });
    let mut registry = Registry::open(required, optional, here)?;

    // Resolved once: the registry does not change while the server is up,
    // and canonicalizing per request would put a filesystem call on the one
    // path the gate is watching. Prefers the ULID for the same reason the
    // index does -- the landing link is the one a person bookmarks.
    let landing: Option<String> = cwd_root.and_then(|c| {
        let key = std::fs::canonicalize(&c).unwrap_or(c);
        registry
            .all()
            .iter()
            .find(|p| std::fs::canonicalize(&p.root).unwrap_or_else(|_| p.root.clone()) == key)
            .map(|p| {
                p.ulid()
                    .map(str::to_string)
                    .unwrap_or_else(|| p.slug.clone())
            })
    });

    let url = gate.boot_url();
    outln!("  vivac web listening on http://127.0.0.1:{bound_port}");
    outln!("  open this to start a session: {url}");
    // `d810`: the set is fixed at startup -- the registry does not change
    // while the server runs -- so there is nothing to say again later.
    let unreachable = registry.unreachable();
    if !unreachable.is_empty() {
        outln!("  {}", unreachable_sentence(unreachable));
    }
    // This is the one line the person starting the server needs before the
    // loop below blocks for good, so it cannot wait in `output`'s buffer for
    // a `main` that will not run again until the process is killed.
    flush();
    if open {
        open_browser(&url);
    }

    // One thread, on purpose and not as a shortcut: the page loads no
    // external resource (see the module doc), so one page load is exactly
    // one request, and there is exactly one user of this process.
    loop {
        let request = server.recv().map_err(Failure::Io)?;
        handle(&mut gate, &mut registry, landing.as_deref(), request);
    }
}

#[cfg(test)]
mod tests {
    /// What `FAVICON` carries, as text. It lives here and not beside the
    /// constant because only the tests read it: the page gets the base64.
    const FAVICON_SVG: &str = concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"5 5 54 54\"><style",
        ">.i{fill:#16181d}.r{stroke:#2f5d50}@media (prefers-color-scheme:dark){",
        ".i{fill:#e6e5e1}.r{stroke:#7fbfa9}}</style><path class=\"r\" d=\"M17 1",
        "7V36Q17 47 28 47H46\" fill=\"none\" stroke-width=\"7.5\" stroke-lineca",
        "p=\"round\"/><circle class=\"i\" cx=\"17\" cy=\"16\" r=\"8.5\"/><circl",
        "e class=\"i\" cx=\"48\" cy=\"47\" r=\"8.5\"/></svg>",
    );

    fn from_base64(text: &str) -> Vec<u8> {
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut count = 0;
        let mut out = Vec::new();
        for c in text.bytes().filter(|&c| c != b'=') {
            let v = table.iter().position(|&t| t == c).expect("a base64 digit") as u32;
            bits = bits << 6 | v;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
                bits &= (1 << count) - 1;
            }
        }
        out
    }

    /// The icon travels inside the page, so it has to be something the
    /// page's own policy lets it load, and the base64 has to be exactly
    /// the SVG above, which names no address but its own namespace.
    #[test]
    fn the_favicon_is_the_mark_as_an_image_the_csp_admits() {
        assert!(super::CSP.contains("img-src data:"), "{}", super::CSP);
        let marker = "href=\"data:image/svg+xml;base64,";
        let at = super::FAVICON
            .find(marker)
            .expect("a base64 SVG in the link");
        let payload = &super::FAVICON[at + marker.len()..];
        let payload = &payload[..payload.find('"').expect("the attribute closes")];
        let decoded = String::from_utf8(from_base64(payload)).expect("UTF-8");
        assert_eq!(decoded, FAVICON_SVG);
        let rest = decoded.replace("xmlns=\"http://www.w3.org/2000/svg\"", "");
        for reach in ["http", "href", "url("] {
            assert!(
                !rest.contains(reach),
                "the icon reaches out with {reach}: {rest}"
            );
        }
    }

    /// Every page that writes a head carries the icon. Read off the source,
    /// like the routes: a page added tomorrow with no icon fails here
    /// rather than showing a blank tab.
    #[test]
    fn every_page_head_carries_the_favicon() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("web");
        let mut heads = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Split, so this test's own source counts as neither.
            let opened = text.matches(concat!("<he", "ad>")).count();
            let icons = text.matches(concat!("{FAV", "ICON}")).count();
            assert_eq!(
                opened,
                icons,
                "{} writes {opened} heads and {icons} icons",
                path.display()
            );
            heads += opened;
        }
        assert!(
            heads >= 4,
            "only {heads} heads found: the search broke, not the pages"
        );
    }

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
        assert!(matches!(route("/p/vivac/tree"), Route::Tree("vivac", "")));
        assert!(matches!(route("/p/vivac/tree/"), Route::Tree("vivac", "")));
        // The query is the map's fold state, so it has to survive the router.
        assert!(matches!(
            route("/p/vivac/tree?fold=g1"),
            Route::Tree("vivac", "fold=g1")
        ));
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
