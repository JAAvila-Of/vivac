//! Local harness metadata is evidence of choices, not account access.
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MODELS: usize = 256;
const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

#[derive(Clone, Debug, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelCatalog {
    pub source: String,
    pub status: String,
    pub models: Vec<ModelOption>,
    pub configured_models: Vec<String>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl ModelCatalog {
    pub fn unavailable() -> Self {
        Self::empty(
            "adapter",
            "unavailable",
            "No local model catalog is available.",
        )
    }

    fn empty(source: &str, status: &str, note: &str) -> Self {
        Self {
            source: source.into(),
            status: status.into(),
            models: Vec::new(),
            configured_models: Vec::new(),
            note: note.into(),
            fetched_at: None,
            client_version: None,
            revision: None,
        }
    }
}

fn harness_home(variable: &str, directory: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::store::home_dir().map(|home| home.join(directory)))
}

fn read(path: &Path) -> Result<Option<String>, ()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_BYTES => {}
        Ok(_) => return Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(());
    }
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/[]".contains(&byte))
        && crate::redact::check_field("model", value).is_none()
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.chars().any(char::is_control)
        && crate::redact::check_field("model label", value).is_none()
}

pub fn codex(root: &Path) -> ModelCatalog {
    let Some(home) = harness_home("CODEX_HOME", ".codex") else {
        return ModelCatalog::unavailable();
    };
    codex_at(root, &home)
}

fn other_provider(table: &toml::Table) -> bool {
    table
        .get("model_provider")
        .is_some_and(|value| value.as_str() != Some("openai"))
        || table.contains_key("model_catalog_json")
        || table.contains_key("profile")
        || table.contains_key("profiles")
        || table
            .values()
            .any(|value| value.as_table().is_some_and(other_provider))
}

fn codex_at(root: &Path, home: &Path) -> ModelCatalog {
    let source = "codex-local-cache";
    let mut configured_models = BTreeSet::new();
    for path in [home.join("config.toml"), root.join(".codex/config.toml")] {
        match read(&path) {
            Ok(Some(text)) => match toml::from_str::<toml::Table>(&text) {
                Ok(table) if other_provider(&table) => return ModelCatalog::empty(source, "unavailable", "Custom providers, profiles or model catalogs require their own catalog; the OpenAI cache was not used."),
                Ok(table) => {
                    if let Some(model) = table.get("model").and_then(toml::Value::as_str).filter(|model| identifier(model)) {
                        configured_models.insert(model.to_owned());
                    }
                },
                Err(_) => return ModelCatalog::empty(source, "error", "Local harness configuration could not be read safely."),
            },
            Ok(None) => {},
            Err(_) => return ModelCatalog::empty(source, "error", "Local harness configuration could not be read safely."),
        }
    }
    let mut catalog = match read(&home.join("models_cache.json")) {
        Ok(Some(text)) => {
            let mut catalog = codex_cache(&text);
            catalog.revision = Some(crate::agents::adapters::digest(text.as_bytes()));
            catalog
        }
        Ok(None) => ModelCatalog::empty(
            source,
            "unavailable",
            "No local Codex model cache was found; configured models remain selectable.",
        ),
        Err(_) => ModelCatalog::empty(
            source,
            "error",
            "Local model cache could not be read within the 2 MiB limit.",
        ),
    };
    catalog.configured_models = configured_models.into_iter().collect();
    catalog
}

fn codex_cache(text: &str) -> ModelCatalog {
    let source = "codex-local-cache";
    let error = || {
        ModelCatalog::empty(
            source,
            "error",
            "Local model cache has invalid or unsafe metadata.",
        )
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return error();
    };
    let Some(rows) = value.get("models").and_then(Value::as_array) else {
        return error();
    };
    if rows.len() > MAX_MODELS {
        return error();
    }
    let mut models = BTreeMap::new();
    for row in rows {
        if row.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        let Some(id) = row
            .get("slug")
            .and_then(Value::as_str)
            .filter(|value| identifier(value))
        else {
            return error();
        };
        let display = row
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(id);
        if !label(display) {
            return error();
        }
        let mut efforts = Vec::new();
        if let Some(levels) = row
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
        {
            if levels.len() > EFFORTS.len() {
                return error();
            }
            for level in levels {
                let Some(effort) = level.get("effort").and_then(Value::as_str) else {
                    return error();
                };
                if !EFFORTS.contains(&effort) {
                    return error();
                }
                if !efforts.iter().any(|value| value == effort) {
                    efforts.push(effort.to_owned());
                }
            }
        }
        let default_effort = row
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .filter(|value| efforts.iter().any(|effort| effort == value))
            .map(str::to_owned);
        models.insert(
            id.to_owned(),
            ModelOption {
                id: id.into(),
                label: display.into(),
                efforts,
                default_effort,
            },
        );
    }
    let fetched_at = value
        .get("fetched_at")
        .and_then(Value::as_str)
        .filter(|value| {
            value.len() <= 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || b"-:.TZ+ ".contains(&byte))
        })
        .map(str::to_owned);
    let client_version = value
        .get("client_version")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 80
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
                && crate::redact::check_field("client version", value).is_none()
        })
        .map(str::to_owned);
    ModelCatalog {
        source: source.into(),
        status: "cached".into(),
        models: models.into_values().collect(),
        configured_models: Vec::new(),
        fetched_at,
        client_version,
        revision: Some(crate::agents::adapters::digest(text.as_bytes())),
        note:
            "Mutable local cache choices; cache context, session, TTL and account availability are unverified."
                .into(),
    }
}

pub fn claude(root: &Path) -> ModelCatalog {
    claude_at(
        root,
        harness_home("CLAUDE_CONFIG_DIR", ".claude").as_deref(),
    )
}

fn claude_at(root: &Path, home: Option<&Path>) -> ModelCatalog {
    let source = "claude-local-settings";
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join("settings.json"));
    }
    paths.extend([
        root.join(".claude/settings.json"),
        root.join(".claude/settings.local.json"),
    ]);
    let mut models = BTreeMap::new();
    for path in paths {
        let text = match read(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(_) => {
                return ModelCatalog::empty(
                    source,
                    "error",
                    "Local settings could not be read within the 2 MiB limit.",
                )
            }
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return ModelCatalog::empty(source, "error", "Local settings have invalid metadata.");
        };
        if !value.is_object() {
            return ModelCatalog::empty(source, "error", "Local settings have invalid metadata.");
        }
        if let Some(model) = value.get("model") {
            let Some(id) = model.as_str().filter(|id| identifier(id)) else {
                return ModelCatalog::empty(
                    source,
                    "error",
                    "Local settings have unsafe model metadata.",
                );
            };
            models.insert(
                id.to_owned(),
                ModelOption {
                    id: id.into(),
                    label: id.into(),
                    efforts: Vec::new(),
                    default_effort: None,
                },
            );
        }
    }
    ModelCatalog { source: source.into(), status: if models.is_empty() { "unavailable" } else { "configured" }.into(),
        models: models.into_values().collect(), configured_models: Vec::new(), fetched_at: None, client_version: None, revision: None,
        note: "Configured local model choices only. Managed model restrictions, supported effort levels and runtime access are unverified; local availableModels is not an operative restriction.".into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "vivac-catalog-{}-{}",
            std::process::id(),
            crate::agents::adapters::digest(&getrandom_bytes())
        ));
        fs::create_dir_all(path.join(".codex")).unwrap();
        path
    }
    fn getrandom_bytes() -> [u8; 8] {
        let mut bytes = [0; 8];
        getrandom::getrandom(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn configured_choices_survive_missing_or_invalid_cache_without_efforts() {
        let root = fixture();
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.toml"), "model = 'configured-home'\n").unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            "model = 'configured-project'\n",
        )
        .unwrap();
        for cache in [None, Some("{"), Some(r#"{"models":[]}"#)] {
            if let Some(text) = cache {
                fs::write(home.join("models_cache.json"), text).unwrap();
            }
            let result = serde_json::to_value(codex_at(&root, &home)).unwrap();
            assert_eq!(
                result["configured_models"],
                serde_json::json!(["configured-home", "configured-project"])
            );
            assert_eq!(result["models"], serde_json::json!([]));
            assert_eq!(
                result["status"],
                if cache.is_none() {
                    "unavailable"
                } else if cache == Some("{") {
                    "error"
                } else {
                    "cached"
                }
            );
        }
        fs::write(
            home.join("models_cache.json"),
            vec![b' '; MAX_BYTES as usize + 1],
        )
        .unwrap();
        let result = serde_json::to_value(codex_at(&root, &home)).unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(
            result["configured_models"],
            serde_json::json!(["configured-home", "configured-project"])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incompatible_or_invalid_config_withholds_all_configured_choices() {
        let root = fixture();
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.toml"), "model = 'configured-home'\n").unwrap();
        for config in [
            "model_provider = 'other'\n",
            "profile = 'work'\n",
            "[profiles.work]\nmodel = 'custom'\n",
            "model_catalog_json = 'custom.json'\n",
            "model = [",
        ] {
            fs::write(root.join(".codex/config.toml"), config).unwrap();
            let result = serde_json::to_value(codex_at(&root, &home)).unwrap();
            assert_eq!(result["configured_models"], serde_json::json!([]));
            assert!(result["models"].as_array().unwrap().is_empty());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_revision_changes_and_client_version_is_whitelisted() {
        let root = fixture();
        let home = root.join(".codex");
        let cache = home.join("models_cache.json");
        let first = r#"{"models":[],"client_version":"1.2.3-preview.4","identity":"private"}"#;
        fs::write(&cache, first).unwrap();
        let before = serde_json::to_value(codex_at(&root, &home)).unwrap();
        assert_eq!(before["client_version"], "1.2.3-preview.4");
        assert_eq!(
            before["revision"],
            crate::agents::adapters::digest(first.as_bytes())
        );
        for version in [
            "bad\nversion",
            "private@example.com",
            "private/path",
            "é",
            &"1".repeat(81),
        ] {
            let text = serde_json::json!({"models":[], "client_version":version}).to_string();
            fs::write(&cache, &text).unwrap();
            let after = serde_json::to_value(codex_at(&root, &home)).unwrap();
            assert!(after.get("client_version").is_none());
            assert_ne!(before["revision"], after["revision"]);
            assert_eq!(fs::read_to_string(&cache).unwrap(), text);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_whitelists_metadata_and_preserves_per_model_efforts() {
        let result = codex_cache(
            r#"{"identity":"private","base_instructions":"private","fetched_at":"2026-10-08T00:00:00Z","models":[{"slug":"test-full","display_name":"Full","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}],"default_reasoning_level":"low","base_instructions":"private"},{"slug":"test-small","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"max"}]},{"slug":"hidden","visibility":"hide"}]}"#,
        );
        assert_eq!(result.status, "cached");
        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].efforts, ["low", "ultra"]);
        assert_eq!(result.models[1].efforts, ["low", "max"]);
        assert_eq!(result.models[0].default_effort.as_deref(), Some("low"));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("identity"));
    }

    #[test]
    fn malformed_and_terminal_controls_are_rejected() {
        for text in [
            "{",
            r#"{"models":[{"slug":"bad\u001b[31m","visibility":"list"}]}"#,
            r#"{"models":[{"slug":"okay","display_name":"bad\nlabel","visibility":"list"}]}"#,
        ] {
            assert_eq!(codex_cache(text).status, "error");
        }
    }

    #[test]
    fn absent_oversized_and_custom_provider_cache_are_not_used() {
        let root = fixture();
        let home = root.join(".codex");
        assert_eq!(codex_at(&root, &home).status, "unavailable");
        fs::write(
            home.join("models_cache.json"),
            vec![b' '; MAX_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(codex_at(&root, &home).status, "error");
        fs::write(home.join("config.toml"), "model_provider = 'other'\n").unwrap();
        assert_eq!(codex_at(&root, &home).status, "unavailable");
        fs::write(
            home.join("config.toml"),
            "model_catalog_json = 'custom.json'\n",
        )
        .unwrap();
        assert_eq!(codex_at(&root, &home).status, "unavailable");
        fs::write(
            home.join("config.toml"),
            "[profiles.work]\nmodel = 'custom'\n",
        )
        .unwrap();
        assert_eq!(codex_at(&root, &home).status, "unavailable");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_settings_are_configured_choices_not_a_catalog_or_policy() {
        let root = fixture();
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(
            root.join(".claude/settings.json"),
            r#"{"model":"sonnet","availableModels":["other"],"env":{"TOKEN":"private"}}"#,
        )
        .unwrap();
        fs::write(
            root.join(".claude/settings.local.json"),
            r#"{"model":"custom-model"}"#,
        )
        .unwrap();
        let result = claude_at(&root, None);
        assert_eq!(result.status, "configured");
        assert_eq!(result.models.len(), 2);
        assert!(result.models.iter().all(|model| model.efforts.is_empty()));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("other"));
        fs::remove_dir_all(root).unwrap();
    }
}
