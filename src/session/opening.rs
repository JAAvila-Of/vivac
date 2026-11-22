//! Bounded opening inputs and harness-specific output.

use crate::output::outln;

pub(super) const CONTEXT_BYTES: usize = 60_000;
pub(super) const CLAUDE_PROTOCOL: &str = "vivac.claude.opening.v1";
/// The most UTF-16 code units the command fallback may put in
/// `additionalContext`; a longer opening is never truncated.
pub(super) const FALLBACK_UNITS: usize = 9_900;
pub(super) const FALLBACK_NOTICE: &str = "The vivac opening context was not delivered. Before relying on project context, call vivac_brief (MCP) or run vivac brief.";
const FALLBACK_CONTEXT: &str = "claude-fallback";

/// Whether the hook input carries a receipt the module wrote: the protocol,
/// a prepared status that needs no fallback, and a safe positive counter.
/// The module producer's own guard and the command fallback share this.
pub(super) fn valid_receipt(input: &serde_json::Value) -> bool {
    input
        .as_object()
        .and_then(|input| input.get("vivac_opening"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|receipt| {
            receipt.get("protocol").and_then(serde_json::Value::as_str) == Some(CLAUDE_PROTOCOL)
                && matches!(
                    receipt.get("status").and_then(serde_json::Value::as_str),
                    Some("ready" | "absent")
                )
                && receipt
                    .get("generation")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|generation| (1..=9_007_199_254_740_991).contains(&generation))
        })
}

/// A fallback run the module already covered: valid input and a valid receipt.
pub(super) fn fallback_covered(context: &str) -> bool {
    context == FALLBACK_CONTEXT
        && Request::read(FALLBACK_CONTEXT).is_ok()
        && native_input().is_ok_and(|input| valid_receipt(&input))
}

pub(super) fn session_key(id: &str) -> String {
    format!("session:{id}")
}

pub(super) fn supports_context(context: &str) -> bool {
    matches!(context, "codex" | "claude-module" | FALLBACK_CONTEXT)
}

pub(super) fn absent(context: &str) {
    match context {
        "claude-module" => claude_absent(),
        "codex" | FALLBACK_CONTEXT => {}
        _ => eprintln!("Unsupported opening context."),
    }
}

#[derive(Clone, Copy)]
pub(super) enum Profile {
    CodexSession,
    CodexSubagent,
    ClaudeSession,
    ClaudeSubagent,
    FallbackSession,
    FallbackSubagent,
}

enum Recipient {
    Session { source: String, id: Option<String> },
    Subagent(String),
}

pub(super) struct Request {
    pub profile: Profile,
    recipient: Recipient,
}

impl Request {
    pub fn read(context: &str) -> Result<Self, ()> {
        if !supports_context(context) {
            return Err(());
        }
        let value = native_input()?;
        let profile = native_profile(context, &value)?;
        let recipient = match profile {
            Profile::CodexSubagent | Profile::ClaudeSubagent | Profile::FallbackSubagent => {
                Recipient::Subagent(agent_id(&value)?.to_owned())
            }
            Profile::CodexSession | Profile::ClaudeSession | Profile::FallbackSession => {
                Recipient::Session {
                    source: value
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                    id: value
                        .get("session_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                }
            }
        };
        Ok(Self { profile, recipient })
    }

    /// What the fallback's delivery mark is keyed by: the session for a
    /// session, the agent for a child.
    pub fn delivery_key(&self) -> Option<String> {
        match &self.recipient {
            Recipient::Session { id, .. } => id.as_deref().map(session_key),
            Recipient::Subagent(id) => Some(format!("agent:{id}")),
        }
    }

    pub fn receipt(&self) -> (&str, Option<String>) {
        match &self.recipient {
            Recipient::Session { source, id } => (source, id.clone()),
            Recipient::Subagent(id) => ("subagent", Some(id.clone())),
        }
    }
}

fn native_input() -> Result<serde_json::Value, ()> {
    let raw = super::hook_stdin();
    if raw.is_empty() {
        return Ok(serde_json::json!({}));
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| ())?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(())
    }
}

fn agent_id(value: &serde_json::Value) -> Result<&str, ()> {
    let id = value
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || crate::redact::check_field("session", id).is_some()
    {
        return Err(());
    }
    Ok(id)
}

fn native_profile(context: &str, value: &serde_json::Value) -> Result<Profile, ()> {
    let child = match value.get("hook_event_name") {
        None => false,
        Some(serde_json::Value::String(event)) if event == "SessionStart" => false,
        Some(serde_json::Value::String(event)) if event == "SubagentStart" => true,
        _ => return Err(()),
    };
    match (context, child) {
        ("codex", false) => Ok(Profile::CodexSession),
        ("codex", true) => Ok(Profile::CodexSubagent),
        ("claude-module", false) => Ok(Profile::ClaudeSession),
        ("claude-module", true) => Ok(Profile::ClaudeSubagent),
        (FALLBACK_CONTEXT, false) => Ok(Profile::FallbackSession),
        (FALLBACK_CONTEXT, true) => Ok(Profile::FallbackSubagent),
        _ => Err(()),
    }
}

pub(super) fn invalid_input(context: &str) {
    match context {
        "claude-module" => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "invalid-input"})),
        "codex" => codex_rejection("vivac rejected this Codex opening because its event or recipient is invalid. No partial context was delivered. Native work can continue; check the hook configuration before relying on project context."),
        // No native event is known, so there is nowhere to put a notice.
        FALLBACK_CONTEXT => {}
        _ => eprintln!("Unsupported opening context."),
    }
}

pub(super) fn preparation_failed(context: &str) {
    let profile = match context {
        "codex" => native_input().and_then(|value| native_profile(context, &value)),
        "claude-module" | FALLBACK_CONTEXT => Request::read(context).map(|request| request.profile),
        _ => {
            eprintln!("Unsupported opening context.");
            return;
        }
    };
    match profile {
        Ok(profile) => profile.preparation_failed(),
        Err(()) => invalid_input(context),
    }
}

pub(super) fn claude_absent() {
    outln!(
        "{}",
        serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "absent"})
    );
}

fn codex_rejection(message: &str) {
    outln!(
        "{}",
        serde_json::json!({"continue": true, "systemMessage": message})
    );
}

impl Profile {
    pub fn is_fallback(self) -> bool {
        matches!(self, Self::FallbackSession | Self::FallbackSubagent)
    }

    fn native_event(self) -> &'static str {
        match self {
            Self::CodexSession | Self::ClaudeSession | Self::FallbackSession => "SessionStart",
            Self::CodexSubagent | Self::ClaudeSubagent | Self::FallbackSubagent => "SubagentStart",
        }
    }

    fn fallback_notice(self) {
        outln!(
            "{}",
            serde_json::json!({"hookSpecificOutput": {"hookEventName": self.native_event(), "additionalContext": FALLBACK_NOTICE}})
        );
    }

    pub fn too_large(self) {
        match self {
            Self::FallbackSession | Self::FallbackSubagent => self.fallback_notice(),
            Self::ClaudeSession | Self::ClaudeSubagent => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "too-large"})),
            Self::CodexSession => codex_rejection("vivac could not deliver the complete opening context within its 60000-byte transport limit. No partial context was delivered. Native work can continue; review vivac rules and vivac brief before relying on project context."),
            Self::CodexSubagent => codex_rejection("vivac rejected this Codex subagent opening because its complete context exceeds the 60000-byte transport limit. No partial context was delivered. Native work can continue; restore complete project context before relying on it."),
        }
    }

    fn preparation_failed(self) {
        match self {
            Self::FallbackSession | Self::FallbackSubagent => self.fallback_notice(),
            Self::ClaudeSession | Self::ClaudeSubagent => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "preparation-failed"})),
            Self::CodexSession => codex_rejection("vivac could not prepare the complete opening context. No partial context was delivered. Native work can continue; review the diagnostic and restore readable project context before relying on it."),
            Self::CodexSubagent => codex_rejection("vivac rejected this Codex subagent opening because complete project context could not be prepared. No partial context was delivered. Native work can continue; restore readable project context before relying on it."),
        }
    }

    pub fn ready(self, text: String) {
        match self {
            Self::ClaudeSession | Self::ClaudeSubagent => outln!(
                "{}",
                serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "ready", "context": text})
            ),
            Self::CodexSession
            | Self::CodexSubagent
            | Self::FallbackSession
            | Self::FallbackSubagent => outln!(
                "{}",
                serde_json::json!({"hookSpecificOutput": {"hookEventName": self.native_event(), "additionalContext": text}})
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Request;

    #[test]
    fn unknown_profile_has_no_codex_fallback() {
        assert!(Request::read("future-harness").is_err());
    }
}
