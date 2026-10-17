//! Bounded opening inputs and harness-specific output.

use crate::output::outln;

pub(super) const CONTEXT_BYTES: usize = 60_000;
pub(super) const CLAUDE_PROTOCOL: &str = "vivac.claude.opening.v1";

pub(super) fn supports_context(context: &str) -> bool {
    matches!(context, "codex" | "claude-module")
}

pub(super) fn absent(context: &str) {
    match context {
        "claude-module" => claude_absent(),
        "codex" => {}
        _ => eprintln!("Unsupported opening context."),
    }
}

#[derive(Clone, Copy)]
pub(super) enum Profile {
    CodexSession,
    CodexSubagent,
    ClaudeSession,
    ClaudeSubagent,
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
            Profile::CodexSubagent | Profile::ClaudeSubagent => {
                Recipient::Subagent(agent_id(&value)?.to_owned())
            }
            Profile::CodexSession | Profile::ClaudeSession => Recipient::Session {
                source: value
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                id: value
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            },
        };
        Ok(Self { profile, recipient })
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
        _ => Err(()),
    }
}

pub(super) fn invalid_input(context: &str) {
    match context {
        "claude-module" => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "invalid-input"})),
        "codex" => codex_rejection("vivac rejected this Codex opening because its event or recipient is invalid. No partial context was delivered. Codex may still continue; check the hook configuration before continuing."),
        _ => eprintln!("Unsupported opening context."),
    }
}

pub(super) fn preparation_failed(context: &str) {
    let profile = match context {
        "codex" => native_input().and_then(|value| native_profile(context, &value)),
        "claude-module" => Request::read(context).map(|request| request.profile),
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
        serde_json::json!({"continue": false, "systemMessage": message})
    );
}

impl Profile {
    pub fn too_large(self) {
        match self {
            Self::ClaudeSession | Self::ClaudeSubagent => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "too-large"})),
            Self::CodexSession => codex_rejection("vivac could not deliver the complete opening context within its 60000-byte transport limit. This opening must not proceed with partial instructions. Review vivac rules and vivac brief before continuing."),
            Self::CodexSubagent => codex_rejection("vivac rejected this subagent opening because its complete context exceeds the 60000-byte transport limit. No partial context was delivered. Codex may still start this subagent; restore complete project context before continuing."),
        }
    }

    fn preparation_failed(self) {
        match self {
            Self::ClaudeSession | Self::ClaudeSubagent => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "preparation-failed"})),
            Self::CodexSession => codex_rejection("vivac could not prepare the complete opening context. This opening must not continue with missing instructions. Review the diagnostic and restore readable project context before continuing."),
            Self::CodexSubagent => codex_rejection("vivac rejected this subagent opening because complete project context could not be prepared. No partial context was delivered. Codex may still start this subagent; restore readable project context before continuing."),
        }
    }

    pub fn ready(self, text: String) {
        match self {
            Self::ClaudeSession | Self::ClaudeSubagent => outln!(
                "{}",
                serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "ready", "context": text})
            ),
            Self::CodexSession | Self::CodexSubagent => {
                let event = match self {
                    Self::CodexSession => "SessionStart",
                    Self::CodexSubagent => "SubagentStart",
                    Self::ClaudeSession | Self::ClaudeSubagent => unreachable!(),
                };
                outln!(
                    "{}",
                    serde_json::json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}})
                );
            }
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
