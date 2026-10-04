//! Declared agent contracts and explicit harness assignments.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub purpose: String,
    pub duties: Vec<String>,
    pub limits: Vec<String>,
    pub acceptance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub harness: String,
    pub name: String,
    pub model: String,
    pub effort: String,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub schema_version: u32,
    pub name: String,
    pub contract: Contract,
    pub assignments: Vec<Assignment>,
    #[serde(default)]
    pub retired: bool,
}

/// Native-file metadata only. The prompt body never leaves the adapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub harness: String,
    pub path: String,
    pub digest: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub settings: BTreeMap<String, Value>,
    pub unsupported: Vec<String>,
    pub problem: Option<String>,
}

pub fn contract_text(contract: &Contract) -> String {
    let mut text = format!("{}\n", contract.purpose);
    for (heading, entries) in [
        ("Duties", &contract.duties),
        ("Limits", &contract.limits),
        ("Acceptance", &contract.acceptance),
    ] {
        text.push_str(&format!("\n{heading}:\n"));
        for entry in entries {
            text.push_str(&format!("- {entry}\n"));
        }
    }
    text
}
