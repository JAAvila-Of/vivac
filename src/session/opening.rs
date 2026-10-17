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
    ClaudeModule,
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
        match context {
            "claude-module" => {
                let input = super::HookInput::read();
                return Ok(Self {
                    profile: Profile::ClaudeModule,
                    recipient: Recipient::Session {
                        source: input.source,
                        id: input.session,
                    },
                });
            }
            "codex" => {}
            _ => return Err(()),
        }
        let value = codex_input()?;
        let profile = codex_profile(&value)?;
        let recipient = match profile {
            Profile::CodexSubagent => {
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
                Recipient::Subagent(id.to_owned())
            }
            Profile::CodexSession => Recipient::Session {
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
            Profile::ClaudeModule => unreachable!(),
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

fn codex_input() -> Result<serde_json::Value, ()> {
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

fn codex_profile(value: &serde_json::Value) -> Result<Profile, ()> {
    match value.get("hook_event_name") {
        None => Ok(Profile::CodexSession),
        Some(serde_json::Value::String(event)) if event == "SessionStart" => {
            Ok(Profile::CodexSession)
        }
        Some(serde_json::Value::String(event)) if event == "SubagentStart" => {
            Ok(Profile::CodexSubagent)
        }
        _ => Err(()),
    }
}

pub(super) fn invalid_input() {
    codex_rejection("vivac rejected this Codex opening because its event or recipient is invalid. No partial context was delivered. Codex may still continue; check the hook configuration before continuing.");
}

pub(super) fn preparation_failed(context: &str) {
    let profile = match context {
        "claude-module" => Ok(Profile::ClaudeModule),
        "codex" => codex_input().and_then(|value| codex_profile(&value)),
        _ => {
            eprintln!("Unsupported opening context.");
            return;
        }
    };
    match profile {
        Ok(profile) => profile.preparation_failed(),
        Err(()) => invalid_input(),
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
            Self::ClaudeModule => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "too-large"})),
            Self::CodexSession => codex_rejection("vivac could not deliver the complete opening context within its 60000-byte transport limit. This opening must not proceed with partial instructions. Review vivac rules and vivac brief before continuing."),
            Self::CodexSubagent => codex_rejection("vivac rejected this subagent opening because its complete context exceeds the 60000-byte transport limit. No partial context was delivered. Codex may still start this subagent; restore complete project context before continuing."),
        }
    }

    fn preparation_failed(self) {
        match self {
            Self::ClaudeModule => outln!("{}", serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "rejected", "reason": "preparation-failed"})),
            Self::CodexSession => codex_rejection("vivac could not prepare the complete opening context. This opening must not continue with missing instructions. Review the diagnostic and restore readable project context before continuing."),
            Self::CodexSubagent => codex_rejection("vivac rejected this subagent opening because complete project context could not be prepared. No partial context was delivered. Codex may still start this subagent; restore readable project context before continuing."),
        }
    }

    pub fn ready(self, text: String) {
        match self {
            Self::ClaudeModule => outln!(
                "{}",
                serde_json::json!({"protocol": CLAUDE_PROTOCOL, "status": "ready", "context": text})
            ),
            Self::CodexSession | Self::CodexSubagent => {
                let event = match self {
                    Self::CodexSession => "SessionStart",
                    Self::CodexSubagent => "SubagentStart",
                    Self::ClaudeModule => unreachable!(),
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
