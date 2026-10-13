use super::agents_skill::{Definition, Skill};
use std::path::Path;

pub(super) const CLAUDE_LABEL: &str = ".claude/skills/vivac-artifacts/SKILL.md";
pub(super) const CODEX_LABEL: &str = ".agents/skills/vivac-artifacts/SKILL.md";

static ARTIFACTS: Definition = Definition {
    name: "vivac-artifacts",
    content: include_str!("artifacts-skill.md"),
    create_description: "optional guidance for selected artifact deliveries",
};

pub(super) fn read(here: &Path, label: &'static str) -> Skill {
    Skill::read_with(here, label, &ARTIFACTS)
}

pub(super) fn advice(harness: super::Harness) -> String {
    let name = match harness {
        super::Harness::ClaudeCode => "Claude Code",
        super::Harness::Codex => "Codex",
    };
    format!("\nExisting artifacts: ask {name}:\n\n  Use the vivac-artifacts skill to bring selected existing artifacts into vivac.\n")
}
