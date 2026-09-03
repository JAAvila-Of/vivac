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
//! **The pages here are scaffolding, not surfaces.** The real ones --
//! `WEB.md` §3.1 and §3.2 -- come once this layer is in place and proven.

mod gate;

use crate::failure::{Failure, R};
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

fn serve_page(ids: &[&str]) -> String {
    let items: String = ids
        .iter()
        .map(|id| format!("<li>{}</li>\n", escape(id)))
        .collect();
    format!(
        "<!doctype html>\n\
         <html><head><meta charset=\"utf-8\"></head>\n\
         <body><p>vivac is running.</p><ul>\n{items}</ul></body></html>\n"
    )
}

fn respond(request: tiny_http::Request, status: u16, content_type: &str, body: String) {
    let response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Content-Security-Policy", CSP))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header("Cache-Control", "no-store"));
    // A client that closed the connection before the answer arrived is not
    // this server's failure to report.
    let _ = request.respond(response);
}

fn handle(gate: &mut Gate, registry: &Registry, request: tiny_http::Request) {
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
        Verdict::Serve => respond(request, 200, HTML, serve_page(&registry.ids())),
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
    let registry = Registry::open(roots)?;

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
        handle(&mut gate, &registry, request);
    }
}

#[cfg(test)]
mod tests {
    use super::escape;

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
}
