//! Harness formats stay behind a capability boundary.

pub use super::catalog::ModelCatalog;
use super::types::{contract_text, Assignment, Candidate, Definition};
use crate::failure::Failure;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

const MAX_NATIVE_BYTES: u64 = 1024 * 1024;

pub trait Adapter {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn precedence(&self) -> &str;
    fn directory(&self) -> &str;
    fn extension(&self) -> &str;
    fn capabilities(&self) -> &[&str];
    fn model_catalog(&self, _root: &Path) -> ModelCatalog {
        ModelCatalog::unavailable()
    }
    fn inspect(&self, root: &Path, relative: &str) -> Result<Candidate, Failure>;
    fn render(
        &self,
        agent: &str,
        revision: &str,
        definition: &Definition,
        assignment: &Assignment,
        prompt: Option<&str>,
    ) -> Result<String, Failure>;
    fn validate(&self, assignment: &Assignment) -> Result<(), Failure>;
    fn prompt_source(&self, text: &str) -> Result<(String, Option<String>), Failure>;

    fn paths(&self, root: &Path) -> Result<Vec<String>, Failure> {
        safe_path(root, self.directory(), self.directory())?;
        let directory = root.join(self.directory());
        if !directory.exists() {
            // A dangling link must not masquerade as an absent directory.
            if fs::symlink_metadata(&directory).is_ok() {
                return Err(Failure::usage("Agent directories must not be links."));
            }
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&directory)?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some(self.extension()) {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(Failure::usage("An agent filename is not valid UTF-8."));
            };
            let relative = format!("{}/{name}", self.directory());
            safe_path(root, &relative, self.directory())?;
            paths.push(relative);
        }
        paths.sort();
        Ok(paths)
    }

    fn discover(&self, root: &Path) -> Result<Vec<Candidate>, Failure> {
        self.paths(root)?
            .iter()
            .map(|path| self.inspect(root, path))
            .collect()
    }
}

pub fn all() -> Vec<Box<dyn Adapter>> {
    {
        #[cfg(test)]
        {
            vec![Box::new(Codex), Box::new(ClaudeCode), Box::new(Example)]
        }
        #[cfg(not(test))]
        {
            vec![Box::new(Codex), Box::new(ClaudeCode)]
        }
    }
}

pub fn get(name: &str) -> Option<Box<dyn Adapter>> {
    all().into_iter().find(|adapter| adapter.name() == name)
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn read_prompt(root: &Path, harness: &str, relative: &str) -> Result<String, Failure> {
    read_source(root, harness, relative).map(|source| source.0)
}

pub fn read_source(
    root: &Path,
    harness: &str,
    relative: &str,
) -> Result<(String, String, String), Failure> {
    let adapter =
        get(harness).ok_or_else(|| Failure::usage("Unsupported prompt source harness."))?;
    let bytes = read_native(root, relative, adapter.directory())?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| Failure::usage("Prompt source is not UTF-8."))?;
    let (prompt, description) = adapter.prompt_source(text)?;
    if prompt.trim().is_empty() {
        return Err(Failure::usage("Native prompt is empty."));
    }
    if let Some(finding) = crate::redact::check_prompt(&prompt) {
        return Err(Failure::Redaction(Box::new(finding)));
    }
    Ok((
        prompt,
        description.ok_or_else(|| Failure::usage("Native description is missing."))?,
        digest(&bytes),
    ))
}

pub fn safe_path(
    root: &Path,
    relative: &str,
    directory: &str,
) -> Result<std::path::PathBuf, Failure> {
    let path = Path::new(relative);
    if relative.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || !(relative == directory || relative.starts_with(&format!("{directory}/")))
    {
        return Err(Failure::usage(
            "Agent paths must stay inside the adapter's project directory.",
        ));
    }
    if let Some(finding) = crate::redact::check_field("agent path", relative) {
        return Err(Failure::Redaction(Box::new(finding)));
    }
    let mut current = root.to_path_buf();
    if is_link(&fs::symlink_metadata(root)?) {
        return Err(Failure::usage(
            "Agent roots must not be symbolic links or reparse points.",
        ));
    }
    for part in path.components() {
        current.push(part.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(Failure::usage(
                    "Agent paths must not cross symbolic links or reparse points.",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(root.join(path))
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn read_native(root: &Path, relative: &str, directory: &str) -> Result<Vec<u8>, Failure> {
    let path = safe_path(root, relative, directory)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() > MAX_NATIVE_BYTES {
        return Err(Failure::usage(
            "An agent must be a regular file of at most 1 MiB.",
        ));
    }
    let bytes = fs::read(&path)?;
    if bytes.len() as u64 > MAX_NATIVE_BYTES {
        return Err(Failure::usage(
            "An agent grew beyond the 1 MiB limit while it was read.",
        ));
    }
    Ok(bytes)
}

fn empty_candidate(harness: &str, path: &str, bytes: &[u8]) -> Candidate {
    Candidate {
        harness: harness.into(),
        path: path.into(),
        digest: digest(bytes),
        name: None,
        model: None,
        effort: None,
        settings: BTreeMap::new(),
        unsupported: Vec::new(),
        problem: None,
    }
}

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_common(
    assignment: &Assignment,
    harness: &str,
    efforts: &[&str],
) -> Result<(), Failure> {
    if assignment.harness != harness || !slug(&assignment.name) {
        return Err(Failure::usage(
            "An assignment needs a supported harness and a plain agent name.",
        ));
    }
    if assignment.model.is_empty()
        || assignment.model.len() > 200
        || assignment.model.chars().any(char::is_control)
    {
        return Err(Failure::usage(
            "An assignment needs an explicit model or inherit.",
        ));
    }
    if !efforts.contains(&assignment.effort.as_str()) {
        return Err(Failure::usage(
            "The adapter does not support this effort level.",
        ));
    }
    for (key, value) in &assignment.settings {
        if let Some(finding) = crate::redact::check_field("agent setting name", key) {
            return Err(Failure::Redaction(Box::new(finding)));
        }
        check_value(value)?;
    }
    for value in [&assignment.name, &assignment.model, &assignment.effort] {
        if let Some(finding) = crate::redact::check_field("agent assignment", value) {
            return Err(Failure::Redaction(Box::new(finding)));
        }
    }
    Ok(())
}

pub fn check_value(value: &Value) -> Result<(), Failure> {
    match value {
        Value::String(text) => {
            if let Some(finding) = crate::redact::check_field("agent setting", text) {
                return Err(Failure::Redaction(Box::new(finding)));
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                check_value(entry)?;
            }
        }
        Value::Object(entries) => {
            for (key, entry) in entries {
                if let Some(finding) = crate::redact::check_field("agent setting name", key) {
                    return Err(Failure::Redaction(Box::new(finding)));
                }
                check_value(entry)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn metadata(
    candidate: &mut Candidate,
    values: &BTreeMap<String, Value>,
    core: &[&str],
    settings: &[&str],
    effort: &str,
) {
    candidate.name = values
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    candidate.model = values
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(Some("inherit".into()));
    candidate.effort = values
        .get(effort)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(Some("inherit".into()));
    if ["name", "model", effort]
        .iter()
        .any(|key| values.get(*key).is_some_and(|value| !value.is_string()))
    {
        candidate.problem = Some("Unsupported agent metadata types.".into());
    }
    for (key, value) in values {
        if settings.contains(&key.as_str()) {
            candidate.settings.insert(key.clone(), value.clone());
        } else if !core.contains(&key.as_str()) {
            candidate.unsupported.push(
                if crate::redact::check_field("agent field", key).is_none() {
                    key.clone()
                } else {
                    "withheld field".into()
                },
            );
        }
    }
    if candidate.name.as_deref().is_none_or(|name| !slug(name)) {
        candidate.problem = Some("Missing or unsupported agent name.".into());
    }
    let mut safe = candidate.settings.values().all(|v| check_value(v).is_ok());
    for value in [&candidate.name, &candidate.model, &candidate.effort]
        .into_iter()
        .flatten()
    {
        safe &= crate::redact::check_field("agent metadata", value).is_none();
    }
    if !safe {
        candidate.name = None;
        candidate.model = None;
        candidate.effort = None;
        candidate.settings.clear();
        candidate.problem = Some("Agent metadata was withheld by the redaction guard.".into());
    }
}

const CODEX_SETTINGS: &[&str] = &["sandbox_mode"];
const CLAUDE_SETTINGS: &[&str] = &[
    "tools",
    "disallowedTools",
    "permissionMode",
    "maxTurns",
    "background",
    "omitClaudeMd",
    "isolation",
];

pub struct Codex;
impl Adapter for Codex {
    fn model_catalog(&self, root: &Path) -> ModelCatalog {
        super::catalog::codex(root)
    }
    fn prompt_source(&self, text: &str) -> Result<(String, Option<String>), Failure> {
        let table = toml::from_str::<toml::Table>(text)
            .map_err(|_| Failure::usage("Native prompt could not be read."))?;
        let description = table
            .get("description")
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        let prompt = table
            .get("developer_instructions")
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Failure::usage("Native prompt could not be read."))?;
        Ok((prompt, description))
    }
    fn name(&self) -> &str {
        "codex"
    }
    fn version(&self) -> &str {
        "codex-agent-v1"
    }
    fn precedence(&self) -> &str {
        "Project agent definition; invocation and runtime overrides require separate evidence."
    }
    fn directory(&self) -> &str {
        ".codex/agents"
    }
    fn extension(&self) -> &str {
        "toml"
    }
    fn capabilities(&self) -> &[&str] {
        &["model", "effort", "sandbox_mode"]
    }
    fn validate(&self, assignment: &Assignment) -> Result<(), Failure> {
        validate_common(
            assignment,
            self.name(),
            &[
                "inherit", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ],
        )?;
        for (key, value) in &assignment.settings {
            if key != "sandbox_mode"
                || !matches!(
                    value.as_str(),
                    Some("read-only" | "workspace-write" | "danger-full-access")
                )
            {
                return Err(Failure::usage("Unsupported Codex agent setting."));
            }
        }
        Ok(())
    }
    fn inspect(&self, root: &Path, relative: &str) -> Result<Candidate, Failure> {
        let bytes = read_native(root, relative, self.directory())?;
        let mut candidate = empty_candidate(self.name(), relative, &bytes);
        let parsed = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| toml::from_str::<toml::Table>(s).ok());
        if let Some(values) = parsed {
            let values = serde_json::to_value(values)
                .ok()
                .and_then(|v| serde_json::from_value::<BTreeMap<String, Value>>(v).ok());
            if let Some(values) = values {
                metadata(
                    &mut candidate,
                    &values,
                    &[
                        "name",
                        "description",
                        "model",
                        "model_reasoning_effort",
                        "developer_instructions",
                    ],
                    CODEX_SETTINGS,
                    "model_reasoning_effort",
                );
                if values.get("description").and_then(Value::as_str).is_none()
                    || values
                        .get("developer_instructions")
                        .and_then(Value::as_str)
                        .is_none()
                {
                    candidate.problem =
                        Some("Codex agent description or instructions are missing.".into());
                }
            } else {
                candidate.problem = Some("Unsupported Codex metadata types.".into());
            }
        } else {
            candidate.problem = Some("Invalid Codex agent TOML or UTF-8.".into());
        }
        validate_candidate(self, &mut candidate);
        Ok(candidate)
    }
    fn render(
        &self,
        agent: &str,
        revision: &str,
        definition: &Definition,
        assignment: &Assignment,
        prompt: Option<&str>,
    ) -> Result<String, Failure> {
        self.validate(assignment)?;
        let mut values = assignment.settings.clone();
        values.insert("name".into(), Value::String(assignment.name.clone()));
        values.insert(
            "description".into(),
            Value::String(definition.contract.purpose.clone()),
        );
        values.insert(
            "developer_instructions".into(),
            Value::String(
                prompt
                    .map(str::to_owned)
                    .unwrap_or_else(|| contract_text(&definition.contract)),
            ),
        );
        if assignment.model != "inherit" {
            values.insert("model".into(), Value::String(assignment.model.clone()));
        }
        if assignment.effort != "inherit" {
            values.insert(
                "model_reasoning_effort".into(),
                Value::String(assignment.effort.clone()),
            );
        }
        let body = toml::to_string_pretty(&values)
            .map_err(|_| Failure::usage("The agent assignment cannot be rendered as TOML."))?;
        Ok(format!(
            "# vivac agent={agent} revision={revision} adapter={}\n{body}",
            self.version()
        ))
    }
}

pub struct ClaudeCode;
impl Adapter for ClaudeCode {
    fn model_catalog(&self, root: &Path) -> ModelCatalog {
        super::catalog::claude(root)
    }
    fn prompt_source(&self, text: &str) -> Result<(String, Option<String>), Failure> {
        let rest = text
            .strip_prefix("---\r\n")
            .or_else(|| text.strip_prefix("---\n"))
            .ok_or_else(|| Failure::usage("Native prompt frontmatter is missing."))?;
        let mut offset = 0;
        let mut body = None;
        let mut description = None;
        for line in rest.split_inclusive('\n') {
            offset += line.len();
            if line.trim_end_matches(['\r', '\n']) == "---" {
                let values: BTreeMap<String, Value> =
                    serde_yaml_ng::from_str(&rest[..offset - line.len()])
                        .map_err(|_| Failure::usage("Native description could not be read."))?;
                description = values
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                body = Some(rest[offset..].to_string());
                break;
            }
        }
        let body =
            body.ok_or_else(|| Failure::usage("Native prompt frontmatter is incomplete."))?;
        Ok((body, description))
    }
    fn name(&self) -> &str {
        "claude-code"
    }
    fn version(&self) -> &str {
        "claude-agent-v1"
    }
    fn precedence(&self) -> &str {
        "Project agents take precedence over user agents with the same name; invocation overrides require separate evidence."
    }
    fn directory(&self) -> &str {
        ".claude/agents"
    }
    fn extension(&self) -> &str {
        "md"
    }
    fn capabilities(&self) -> &[&str] {
        &[
            "model",
            "effort",
            "tools",
            "disallowedTools",
            "permissionMode",
            "maxTurns",
            "background",
            "omitClaudeMd",
            "isolation",
        ]
    }
    fn validate(&self, assignment: &Assignment) -> Result<(), Failure> {
        validate_common(
            assignment,
            self.name(),
            &["inherit", "low", "medium", "high", "xhigh", "max"],
        )?;
        for (key, value) in &assignment.settings {
            let valid = match key.as_str() {
                "tools" | "disallowedTools" => value.as_str().is_some(),
                "permissionMode" => matches!(
                    value.as_str(),
                    Some(
                        "default"
                            | "acceptEdits"
                            | "auto"
                            | "dontAsk"
                            | "bypassPermissions"
                            | "plan"
                            | "manual"
                    )
                ),
                "maxTurns" => value.as_u64().is_some_and(|n| n > 0),
                "background" | "omitClaudeMd" => value.as_bool().is_some(),
                "isolation" => value.as_str() == Some("worktree"),
                _ => false,
            };
            if !valid {
                return Err(Failure::usage("Unsupported Claude Code agent setting."));
            }
        }
        Ok(())
    }
    fn inspect(&self, root: &Path, relative: &str) -> Result<Candidate, Failure> {
        let bytes = read_native(root, relative, self.directory())?;
        let mut candidate = empty_candidate(self.name(), relative, &bytes);
        let yaml = std::str::from_utf8(&bytes).ok().and_then(|s| {
            let s = s
                .strip_prefix("---\r\n")
                .or_else(|| s.strip_prefix("---\n"))?;
            let mut offset = 0;
            for line in s.split_inclusive('\n') {
                if line.trim_end_matches(['\r', '\n']) == "---" {
                    return Some(&s[..offset]);
                }
                offset += line.len();
            }
            None
        });
        if let Some(values) =
            yaml.and_then(|s| serde_yaml_ng::from_str::<BTreeMap<String, Value>>(s).ok())
        {
            metadata(
                &mut candidate,
                &values,
                &["name", "description", "model", "effort"],
                CLAUDE_SETTINGS,
                "effort",
            );
            if values.get("description").and_then(Value::as_str).is_none() {
                candidate.problem = Some("Claude Code agent description is missing.".into());
            }
        } else {
            candidate.problem = Some("Invalid Claude Code agent frontmatter or UTF-8.".into());
        }
        validate_candidate(self, &mut candidate);
        Ok(candidate)
    }
    fn render(
        &self,
        agent: &str,
        revision: &str,
        definition: &Definition,
        assignment: &Assignment,
        prompt: Option<&str>,
    ) -> Result<String, Failure> {
        self.validate(assignment)?;
        let mut values = assignment.settings.clone();
        values.insert("name".into(), Value::String(assignment.name.clone()));
        values.insert(
            "description".into(),
            Value::String(definition.contract.purpose.clone()),
        );
        values.insert("model".into(), Value::String(assignment.model.clone()));
        if assignment.effort != "inherit" {
            values.insert("effort".into(), Value::String(assignment.effort.clone()));
        }
        let yaml = serde_yaml_ng::to_string(&values)
            .map_err(|_| Failure::usage("The agent assignment cannot be rendered as YAML."))?;
        if let Some(prompt) = prompt {
            return Ok(format!(
                "---\n# vivac agent={agent} revision={revision} adapter={}\n{yaml}---\n{prompt}",
                self.version()
            ));
        }
        Ok(format!(
            "---\n{yaml}---\n<!-- vivac agent={agent} revision={revision} adapter={} -->\n\n{}",
            self.version(),
            contract_text(&definition.contract)
        ))
    }
}

fn validate_candidate(adapter: &dyn Adapter, candidate: &mut Candidate) {
    if candidate.problem.is_some() {
        return;
    }
    let assignment = Assignment {
        harness: candidate.harness.clone(),
        name: candidate.name.clone().unwrap_or_default(),
        model: candidate.model.clone().unwrap_or_default(),
        effort: candidate.effort.clone().unwrap_or_default(),
        settings: candidate.settings.clone(),
    };
    if adapter.validate(&assignment).is_err() {
        candidate.problem =
            Some("The native assignment has unsupported settings or effort.".into());
    }
}

#[cfg(test)]
pub struct Example;
#[cfg(test)]
impl Adapter for Example {
    fn prompt_source(&self, text: &str) -> Result<(String, Option<String>), Failure> {
        let values: Value = serde_json::from_str(text)
            .map_err(|_| Failure::usage("Native prompt could not be read."))?;
        let prompt = values["instructions"]
            .as_str()
            .ok_or_else(|| Failure::usage("Native prompt could not be read."))?;
        Ok((prompt.into(), Some("Example agent contract.".into())))
    }
    fn name(&self) -> &str {
        "example"
    }
    fn version(&self) -> &str {
        "example-v1"
    }
    fn precedence(&self) -> &str {
        "Project definitions only."
    }
    fn directory(&self) -> &str {
        ".example/agents"
    }
    fn extension(&self) -> &str {
        "json"
    }
    fn capabilities(&self) -> &[&str] {
        &["model", "effort"]
    }
    fn validate(&self, _: &Assignment) -> Result<(), Failure> {
        Ok(())
    }
    fn inspect(&self, root: &Path, path: &str) -> Result<Candidate, Failure> {
        let bytes = read_native(root, path, self.directory())?;
        let mut candidate = empty_candidate(self.name(), path, &bytes);
        let values = serde_json::from_slice(&bytes).unwrap();
        metadata(
            &mut candidate,
            &values,
            &["name", "model", "effort", "instructions"],
            &[],
            "effort",
        );
        if root.join(".test-race").exists() {
            let mut changed = bytes;
            changed.push(b'\n');
            fs::write(root.join(path), changed)?;
        }
        Ok(candidate)
    }
    fn render(
        &self,
        _: &str,
        _: &str,
        definition: &Definition,
        assignment: &Assignment,
        prompt: Option<&str>,
    ) -> Result<String, Failure> {
        Ok(serde_json::json!({"name": assignment.name, "model": assignment.model, "effort": assignment.effort, "instructions": prompt.map(str::to_owned).unwrap_or_else(|| contract_text(&definition.contract))}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> Definition {
        Definition {
            prompt: None,
            schema_version: 1,
            name: "reviewer".into(),
            retired: false,
            contract: super::super::types::Contract {
                purpose: "Review quoted \"names\" and paths.".into(),
                duties: vec!["Return evidence.".into()],
                limits: vec!["Do not publish.".into()],
                acceptance: vec!["Cite the changed lines.".into()],
            },
            assignments: vec![],
        }
    }
    #[test]
    fn both_native_formats_round_trip_without_copying_a_prompt_into_metadata() {
        let root = std::env::temp_dir().join(format!("vivac-agent-adapters-{}", crate::id::ulid()));
        fs::create_dir_all(&root).unwrap();
        for adapter in all() {
            let assignment = Assignment {
                harness: adapter.name().into(),
                name: "reviewer".into(),
                model: "inherit".into(),
                effort: "high".into(),
                settings: BTreeMap::new(),
            };
            let text = adapter
                .render("agent-id", "revision-id", &definition(), &assignment, None)
                .unwrap();
            let path = format!("{}/reviewer.{}", adapter.directory(), adapter.extension());
            fs::create_dir_all(root.join(adapter.directory())).unwrap();
            fs::write(root.join(&path), text).unwrap();
            let candidate = adapter.inspect(&root, &path).unwrap();
            assert_eq!(candidate.name.as_deref(), Some("reviewer"));
            assert_eq!(candidate.model.as_deref(), Some("inherit"));
            assert_eq!(candidate.effort.as_deref(), Some("high"));
            assert!(candidate.problem.is_none(), "{:?}", candidate.problem);
            assert!(candidate.unsupported.is_empty());
            assert!(!serde_json::to_string(&candidate)
                .unwrap()
                .contains("Return evidence"));
            let prompt = "Inspect the scope.\r\n```text\r\nReturn complete evidence.\r\n```\r\n";
            let text = adapter
                .render(
                    "agent-id",
                    "revision-id",
                    &definition(),
                    &assignment,
                    Some(prompt),
                )
                .unwrap();
            fs::write(root.join(&path), text).unwrap();
            assert_eq!(read_prompt(&root, adapter.name(), &path).unwrap(), prompt);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unsupported_capabilities_are_refused_instead_of_dropped() {
        let mut assignment = Assignment {
            harness: "codex".into(),
            name: "reviewer".into(),
            model: "inherit".into(),
            effort: "high".into(),
            settings: BTreeMap::new(),
        };
        assignment
            .settings
            .insert("hooks".into(), Value::Object(Default::default()));
        assert!(Codex.validate(&assignment).is_err());
        assignment.harness = "claude-code".into();
        assignment.settings.clear();
        assignment.effort = "ultra".into();
        assert!(ClaudeCode.validate(&assignment).is_err());
    }
    #[test]
    fn inspection_preserves_unsupported_fields_and_withholds_malformed_source() {
        let root = std::env::temp_dir().join(format!("vivac-inspection-{}", crate::id::ulid()));
        fs::create_dir_all(root.join(".claude/agents")).unwrap();
        let path = ".claude/agents/reviewer.md";
        fs::write(root.join(path), "---\nname: reviewer\ndescription: Review\nmodel: inherit\neffort: high\nmemory: project\n---\nPrivate instructions that must not be imported.\n").unwrap();
        let candidate = ClaudeCode.inspect(&root, path).unwrap();
        assert_eq!(candidate.unsupported, ["memory"]);
        assert!(!serde_json::to_string(&candidate)
            .unwrap()
            .contains("Private instructions"));
        fs::write(
            root.join(path),
            "---\nname: [malformed\nPrivate source text\n---\n",
        )
        .unwrap();
        let candidate = ClaudeCode.inspect(&root, path).unwrap();
        assert!(candidate.problem.is_some());
        assert!(!serde_json::to_string(&candidate)
            .unwrap()
            .contains("Private source text"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn paths_cannot_escape_or_cross_links() {
        let root = std::env::temp_dir();
        for path in [
            "../secret",
            ".codex/agents/../../secret",
            ".codex\\agents\\a.toml",
            "other/a.toml",
        ] {
            assert!(safe_path(&root, path, ".codex/agents").is_err());
        }
    }
    #[test]
    fn another_adapter_uses_the_same_discovery_contract() {
        let root =
            std::env::temp_dir().join(format!("vivac-example-adapter-{}", crate::id::ulid()));
        fs::create_dir_all(root.join(Example.directory())).unwrap();
        let assignment = Assignment {
            harness: "example".into(),
            name: "reviewer".into(),
            model: "chosen-model".into(),
            effort: "high".into(),
            settings: BTreeMap::new(),
        };
        fs::write(
            root.join(".example/agents/reviewer.json"),
            Example
                .render("id", "revision", &definition(), &assignment, None)
                .unwrap(),
        )
        .unwrap();
        fs::write(root.join(".example/agents/ignored.txt"), "unrelated").unwrap();
        let candidates = Example.discover(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].harness, "example");
        assert_eq!(candidates[0].model.as_deref(), Some("chosen-model"));
        assert_eq!(candidates[0].effort.as_deref(), Some("high"));
        fs::remove_dir_all(root).unwrap();
    }
}
