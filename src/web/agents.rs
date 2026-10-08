//! Agent comparisons and reviewed writes use the CLI workflow.

use super::{escape, header, header_value, FAVICON, WEB_CSS};
use crate::agents::workflow::{self, NativeRef, SyncSelection};
use serde::Deserialize;
use serde_json::json;
use std::io::Read;
use std::path::Path;

const LIMIT: u64 = 128 * 1024;
const JSON: &str = "application/json; charset=utf-8";
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; connect-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    selection: SyncSelection,
    plan_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compare {
    references: Vec<NativeRef>,
}

fn respond(request: tiny_http::Request, status: u16, content_type: &str, body: String) {
    let response = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Content-Security-Policy", CSP))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header("Cache-Control", "no-store"));
    let _ = request.respond(response);
}

fn refuse(request: tiny_http::Request, code: u16, message: &str) {
    respond(request, code, JSON, json!({"error":message}).to_string());
}

pub(super) fn serve(
    mut request: tiny_http::Request,
    id: &str,
    name: &str,
    cwd: &Path,
    operation: &str,
    token: &str,
) {
    let read = operation.is_empty() || operation == "inventory";
    let expected = if read {
        tiny_http::Method::Get
    } else {
        tiny_http::Method::Post
    };
    if *request.method() != expected {
        return refuse(request, 405, "Method not allowed.");
    }
    if !read {
        let host = header_value(request.headers(), "host").unwrap_or("");
        let origin = header_value(request.headers(), "origin");
        let supplied = header_value(request.headers(), "x-vivac-token").unwrap_or("");
        let same_token = supplied.len() == token.len()
            && supplied
                .bytes()
                .zip(token.bytes())
                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                == 0;
        if origin != Some(format!("http://{host}").as_str()) || !same_token {
            return refuse(request, 403, "Same-origin review token required.");
        }
        if header_value(request.headers(), "content-type")
            .and_then(|s| s.split(';').next())
            .map(str::trim)
            != Some("application/json")
        {
            return refuse(request, 415, "JSON content required.");
        }
    }
    if operation.is_empty() {
        let bootstrap = json!({"token":token,"base":format!("/p/{id}/agents")})
            .to_string()
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026");
        let page = format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Agents - {name}</title>{FAVICON}<style>{WEB_CSS}\n{css}</style></head><body><div class=\"page agents-page\"><p class=\"crumb\"><a href=\"/p/{id}/\">Today</a> / Agents</p><header><h1>Agents</h1><p>{name}</p></header><p class=\"promise\">Compare configurations across harnesses before choosing which version to keep.</p><p id=\"message\" role=\"status\"></p><div class=\"agents-toolbar\"><label><input type=\"checkbox\" id=\"show-all\"> Show synchronized agents</label></div><main id=\"inventory\"></main><section id=\"editor\" hidden><fieldset id=\"editor-fields\"><div class=\"editor-top\"><button id=\"back\" type=\"button\">Back to inventory</button><h2 id=\"agent-context\"></h2></div><div class=\"agents-layout\"><nav id=\"agent-list\" aria-label=\"Agents to configure\"></nav><div class=\"agent-workspace\"><nav id=\"stages\" aria-label=\"Synchronization steps\"></nav><section id=\"source-stage\"></section><section id=\"assignment-stage\" hidden></section><section id=\"review\" hidden><h2>Review changes</h2><p>Review every selected agent and destination. No files change until you apply this plan.</p><label>Reason <input id=\"reason\" value=\"Synchronize the reviewed agents\"></label><div class=\"review-actions\"><button id=\"plan\" type=\"button\">Review changes</button><button id=\"apply\" type=\"button\" disabled>Apply reviewed changes</button></div><div id=\"preview\"></div></section><div class=\"stage-actions\"><button id=\"previous\" type=\"button\">Previous</button><button id=\"next\" type=\"button\">Next</button></div></div></div></fieldset></section><footer>The same workflow in a terminal: <code>vivac agents sync</code></footer></div><script type=\"application/json\" id=\"bootstrap\">{bootstrap}</script><script>{script}</script></body></html>", name=escape(name),id=escape(id),css=include_str!("agents.css"),script=include_str!("agents.js"));
        return respond(request, 200, super::HTML, page);
    }
    let result = if operation == "inventory" {
        workflow::inventory(cwd)
    } else {
        if request
            .body_length()
            .is_some_and(|size| size as u64 > LIMIT)
        {
            return refuse(request, 413, "Request too large.");
        }
        let mut body = Vec::new();
        if request
            .as_reader()
            .take(LIMIT + 1)
            .read_to_end(&mut body)
            .is_err()
        {
            return refuse(request, 400, "Request could not be read.");
        }
        if body.len() as u64 > LIMIT {
            return refuse(request, 413, "Request too large.");
        }
        match operation {
            "plan" => match serde_json::from_slice::<SyncSelection>(&body) {
                Ok(selection) => workflow::plan(cwd, &selection),
                Err(_) => return refuse(request, 400, "Invalid selection."),
            },
            "apply" => match serde_json::from_slice::<Apply>(&body) {
                Ok(selected) => workflow::apply(cwd, &selected.selection, &selected.plan_digest)
                    .map(|(value, code)| json!({"result":value,"exit_code":code})),
                Err(_) => return refuse(request, 400, "Invalid reviewed plan."),
            },
            "compare" => match serde_json::from_slice::<Compare>(&body) {
                Ok(selected) => workflow::compare(cwd, &selected.references),
                Err(_) => return refuse(request, 400, "Invalid references."),
            },
            _ => return refuse(request, 404, "Not found."),
        }
    };
    match result {
        Ok(value) => respond(request, 200, JSON, value.to_string()),
        Err(error) => {
            let status = match error.code() {
                1 => 409,
                2 => 400,
                3 => 422,
                4 => 404,
                _ => 500,
            };
            let (value, _) = crate::agents::response(Err(error));
            respond(request, status, JSON, value.to_string());
        }
    }
}
