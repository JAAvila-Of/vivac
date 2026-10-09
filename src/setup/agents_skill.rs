use super::claude_code::{extract_marker, SkillState};
use crate::plan::PlanItem;
use std::path::{Path, PathBuf};

const CONTENT: &str = include_str!("agents-skill.md");
pub(super) const CLAUDE_LABEL: &str = ".claude/skills/vivac-agents/SKILL.md";
pub(super) const CODEX_LABEL: &str = ".agents/skills/vivac-agents/SKILL.md";

pub(super) fn installed(here: &Path, label: &'static str) -> bool {
    matches!(Skill::read(here, label).state, SkillState::Same)
}

fn text() -> String {
    let end = CONTENT
        .strip_prefix("---\n")
        .unwrap()
        .find("\n---\n")
        .unwrap()
        + 9;
    let (frontmatter, body) = CONTENT.split_at(end);
    let fingerprint = super::fnv1a64(CONTENT.as_bytes());
    format!("{frontmatter}<!-- written by vivac setup; fingerprint {fingerprint:016x}; setup removes it with --undo while the text is unchanged -->\n{body}")
}

fn state(existing: &str) -> SkillState {
    if existing == text() {
        return SkillState::Same;
    }
    let Some((claimed, content)) = extract_marker(existing) else {
        return SkillState::Conflict;
    };
    let frontmatter = content
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"));
    let ours = frontmatter.is_some_and(|(header, _)| {
        header
            .lines()
            .filter(|line| line.starts_with("name:"))
            .eq(["name: vivac-agents"])
    });
    if ours && u64::from_str_radix(&claimed, 16).ok() == Some(super::fnv1a64(content.as_bytes())) {
        SkillState::Replaceable
    } else {
        SkillState::Conflict
    }
}

pub(super) struct Skill {
    path: PathBuf,
    label: &'static str,
    state: SkillState,
    original: Option<Vec<u8>>,
}

impl Skill {
    pub(super) fn read(here: &Path, label: &'static str) -> Self {
        let path = here.join(label);
        let mut skill = Self {
            path,
            label,
            state: SkillState::Missing,
            original: None,
        };
        let mut component = here.to_path_buf();
        let parts: Vec<_> = Path::new(label).components().collect();
        for (index, part) in parts.iter().enumerate() {
            component.push(part.as_os_str());
            match std::fs::symlink_metadata(&component) {
                Ok(metadata)
                    if metadata.file_type().is_symlink()
                        || (index + 1 < parts.len() && !metadata.is_dir())
                        || (index + 1 == parts.len() && !metadata.is_file()) =>
                {
                    skill.state = SkillState::Conflict;
                    return skill;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return skill,
                Err(_) => {
                    skill.state = SkillState::Conflict;
                    return skill;
                }
            }
        }
        match std::fs::read(&skill.path) {
            Ok(bytes) => {
                skill.state = std::str::from_utf8(&bytes)
                    .map(state)
                    .unwrap_or(SkillState::Conflict);
                skill.original = Some(bytes);
            }
            Err(_) => skill.state = SkillState::Conflict,
        }
        skill
    }

    pub(super) fn conflict(&self) -> Option<String> {
        matches!(self.state, SkillState::Conflict).then(|| format!(
            "{} cannot be replaced: it is unreadable, uses a link, or is not an unchanged vivac-agents skill written by setup.", self.label
        ))
    }

    pub(super) fn needs_write(&self) -> bool {
        matches!(self.state, SkillState::Missing | SkillState::Replaceable)
    }

    pub(super) fn ours(&self) -> bool {
        matches!(self.state, SkillState::Same | SkillState::Replaceable)
    }

    pub(super) fn plan(&self) -> PlanItem {
        let (verb, what) = match self.state {
            SkillState::Missing => ("create", "how agents work with vivac"),
            SkillState::Same => ("keep", "already there"),
            SkillState::Replaceable => ("replace", "the copy an earlier vivac wrote"),
            SkillState::Conflict => unreachable!("a skill conflict never reaches the plan"),
        };
        PlanItem::new(verb, self.label, what)
    }

    pub(super) fn undo_plan(&self) -> PlanItem {
        if self.ours() {
            PlanItem::new("remove", self.label, "the skill setup wrote")
        } else {
            PlanItem::new("keep", self.label, "left as it is")
        }
    }

    pub(super) fn write(&self, writes: &mut Vec<super::PlannedWrite>) {
        if self.needs_write() {
            writes.push(super::PlannedWrite::write(
                self.path.clone(),
                text(),
                self.original.clone(),
            ));
        }
    }

    pub(super) fn delete(&self, writes: &mut Vec<super::PlannedWrite>) {
        if self.ours() {
            writes.push(super::PlannedWrite::delete(
                self.path.clone(),
                self.original.clone().unwrap(),
            ));
        }
    }

    pub(super) fn clean_empty_directory(&self) {
        if self.ours() {
            super::claude_code::remove_if_empty(self.path.parent());
        }
    }
}
