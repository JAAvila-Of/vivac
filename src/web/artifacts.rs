use super::gate::constant_time_eq as same_token;
use super::{escape, header, header_value, FAVICON, WEB_CSS};
use crate::project::Project;
use serde_json::{json, Value};
use std::io::Read;

const LIMIT: u64 = 16 * 1024;
const JSON: &str = "application/json; charset=utf-8";
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

fn headers() -> Vec<tiny_http::Header> {
    vec![
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
    for h in headers() {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn refuse(request: tiny_http::Request, status: u16, message: &str) {
    respond(request, status, JSON, json!({"error":message}).to_string());
}

fn status(code: i32) -> u16 {
    match code {
        0 => 200,
        1 => 409,
        2 => 400,
        3 => 422,
        4 => 404,
        _ => 500,
    }
}

fn opaque_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn download_fields(body: &[u8]) -> Option<(&str, &str, &str)> {
    if !body.is_ascii() || body.iter().any(|byte| matches!(byte, b'%' | b'+')) {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    let (mut artifact, mut revision, mut token) = (None, None, None);
    for field in body.split('&') {
        let (key, value) = field.split_once('=')?;
        let slot = match key {
            "artifact" => &mut artifact,
            "revision" => &mut revision,
            "token" => &mut token,
            _ => return None,
        };
        if slot.replace(value).is_some() {
            return None;
        }
    }
    let (artifact, revision, token) = (artifact?, revision?, token?);
    if !opaque_id(artifact)
        || !opaque_id(revision)
        || token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some((artifact, revision, token))
}

fn download_failure(
    request: tiny_http::Request,
    code: u16,
    base: &str,
    selected: Option<(&str, &str)>,
) {
    let back = match selected {
        Some((artifact, revision)) => format!("{base}#artifact={artifact}&revision={revision}"),
        None => base.to_owned(),
    };
    respond(request, code, super::HTML, format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Download refused</title>{FAVICON}<style>{WEB_CSS}</style></head><body><main class=\"page\"><h1>Download refused</h1><p>The selected revision could not be downloaded safely. Check its retained bytes before trying again.</p><p><a href=\"{}\">Back to the selected revision</a></p></main></body></html>", escape(&back)));
}

pub(super) fn serve(
    mut request: tiny_http::Request,
    id: &str,
    project: &mut Project,
    operation: &str,
    token: &str,
) {
    let base = format!("/p/{id}/artifacts");
    let expected = if operation.is_empty() {
        tiny_http::Method::Get
    } else {
        tiny_http::Method::Post
    };
    if *request.method() != expected {
        return refuse(request, 405, "Method not allowed.");
    }
    if operation.is_empty() {
        return respond(request, 200, super::HTML, page(id, &project.name, token));
    }
    let download = operation == "download";
    if download {
        let cookie = header_value(request.headers(), "cookie").unwrap_or("");
        let supplied = cookie
            .split(';')
            .filter_map(|part| part.trim().split_once('='))
            .filter(|(name, _)| *name == super::SESSION_COOKIE)
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        if supplied.len() != 1 || !same_token(supplied[0], token) {
            return download_failure(request, 403, &base, None);
        }
    }
    let host = header_value(request.headers(), "host").unwrap_or("");
    if header_value(request.headers(), "origin") != Some(format!("http://{host}").as_str()) {
        return if download {
            download_failure(request, 403, &base, None)
        } else {
            refuse(request, 403, "Same-origin review token required.")
        };
    }
    if !download
        && !same_token(
            header_value(request.headers(), "x-vivac-token").unwrap_or(""),
            token,
        )
    {
        return refuse(request, 403, "Same-origin review token required.");
    }
    let expected_type = if download {
        "application/x-www-form-urlencoded"
    } else {
        "application/json"
    };
    if header_value(request.headers(), "content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        != Some(expected_type)
    {
        return if download {
            download_failure(request, 415, &base, None)
        } else {
            refuse(request, 415, "JSON content required.")
        };
    }
    if request
        .body_length()
        .is_some_and(|size| size as u64 > LIMIT)
    {
        return if download {
            download_failure(request, 413, &base, None)
        } else {
            refuse(request, 413, "Request too large.")
        };
    }
    let mut body = Vec::new();
    if request
        .as_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut body)
        .is_err()
    {
        return if download {
            download_failure(request, 400, &base, None)
        } else {
            refuse(request, 400, "Request could not be read.")
        };
    }
    if body.len() as u64 > LIMIT {
        return if download {
            download_failure(request, 413, &base, None)
        } else {
            refuse(request, 413, "Request too large.")
        };
    }
    if download {
        let Some((artifact, revision, supplied)) = download_fields(&body) else {
            return download_failure(request, 400, &base, None);
        };
        let selected = Some((artifact, revision));
        if !same_token(supplied, token) {
            return download_failure(request, 403, &base, selected);
        }
        return match crate::artifacts::project_download(project, artifact, revision) {
            Ok((reader, filename)) => {
                let size = reader.size() as usize;
                let response = tiny_http::Response::new(
                    tiny_http::StatusCode(200),
                    headers(),
                    reader,
                    Some(size),
                    None,
                )
                .with_header(header("Content-Type", "application/octet-stream"))
                .with_header(header(
                    "Content-Disposition",
                    &format!("attachment; filename=\"{filename}\""),
                ));
                let _ = request.respond(response);
            }
            Err(error) => download_failure(request, status(error.code()), &base, selected),
        };
    }
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return refuse(request, 400, "Invalid artifact request."),
    };
    if value.get("operation").and_then(Value::as_str) != Some(operation) {
        return refuse(request, 400, "Artifact operation does not match the route.");
    }
    let args = match crate::artifacts::request_args(&value) {
        Ok(args) => args,
        Err(_) => return refuse(request, 400, "Invalid artifact request."),
    };
    let (value, code) = crate::artifacts::run_project_value(project, &args);
    respond(request, status(code), JSON, value.to_string());
}

fn page(id: &str, name: &str, token: &str) -> String {
    let bootstrap = json!({"token":token,"base":format!("/p/{id}/artifacts"),"project_base":format!("/p/{id}"),"project_name":name}).to_string().replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026");
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Artifacts - {name}</title>{FAVICON}<style>{WEB_CSS}
{css}</style></head><body><main class="page artifact-page"><p class="crumb"><a href="/p/{id}/">Today</a> / Artifacts</p><nav class="project-sections" aria-label="Project sections"><a href="/p/{id}/">Today</a><a href="/p/{id}/tree">Map</a><a href="/p/{id}/agents">Agents</a><span aria-current="page">Artifacts</span></nav><header><h1>Artifacts</h1><p>{name}</p></header><p class="promise">Find saved deliveries, the work that produced them, and the exact revisions referenced by decisions.</p><p id="message" role="status" aria-live="polite"></p><div class="filters"><label>Scope <select id="scope"><option value="all">Whole project</option><option value="focus">Focus lineage</option><option value="from">From node</option></select></label><label>Node alias or ID <input id="from" hidden disabled></label><label>Search this page <input id="search" type="search"></label><button id="capture" type="button">Capture delivery</button><button id="capabilities" type="button">Supported types</button></div><section id="inventory"><div class="panel table-wrap"><table><thead><tr><th>Delivery</th><th>Latest revision</th><th>Decision references</th><th>Integrity</th></tr></thead><tbody id="rows"></tbody></table></div><div class="pagination"><button id="prev-page" type="button">Previous page</button><button id="next-page" type="button">Next page</button></div></section><section id="detail" hidden><button id="back" type="button">Back to inventory</button><div class="workspace"><aside class="panel" id="revisions"></aside><article class="panel" id="revision-content"></article></div></section><dialog id="dialog"><div id="dialog-content"></div></dialog><footer>The same operations in a terminal: <code>vivac artifacts</code></footer></main><script type="application/json" id="bootstrap">{bootstrap}</script><script>{script}</script></body></html>"#,
        name = escape(name),
        id = escape(id),
        css = include_str!("artifacts.css"),
        script = include_str!("artifacts.js")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_download_requires_exact_ascii_fields_without_decoding() {
        let artifact = "01arz3ndektsv4rrffq69g5fav";
        let token = "a".repeat(64);
        let valid = format!("artifact={artifact}&revision={artifact}&token={token}");
        assert!(download_fields(valid.as_bytes()).is_some());
        for body in [
            format!("{valid}&extra=x"),
            format!("{valid}&token={token}"),
            valid.replace("artifact=", "artifact=%"),
            valid.replace("token=", "token=+"),
            valid.replace(artifact, "../outside"),
            valid.replace("revision=", "revision=ÃƒÂ©"),
        ] {
            assert!(download_fields(body.as_bytes()).is_none());
        }
    }

    #[test]
    fn bootstrap_and_page_escape_project_prose() {
        let page = page(
            "demo",
            "</script><script>alert(1)</script>&",
            &"a".repeat(64),
        );
        assert!(!page.contains("</script><script>alert(1)"));
        assert!(page.contains("\\u003c/script\\u003e"));
        assert!(page.contains("&lt;/script&gt;"));
    }
}
