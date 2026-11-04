mod capture;
mod history;
mod json;
mod measurements;
mod schema;
#[cfg(test)]
mod tests;

use cap_std::fs::{Dir, File};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

type Result<T> = std::result::Result<T, crate::failure::Failure>;

fn refused() -> crate::failure::Failure {
    crate::failure::Failure::usage(
        "Sanitation planning refused: unsupported, unsafe or changed input.",
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Title,
    Why,
    Notes,
    Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selector {
    pub node: String,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Location {
    pub node: String,
    pub role: Role,
    pub seq: u64,
    pub pointer: String,
}

pub(crate) struct Request {
    pub source_root: PathBuf,
    pub lane_roots: BTreeMap<String, PathBuf>,
    pub selection: Vec<Selector>,
    pub destination: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    path: PathBuf,
    identity: Identity,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    root: String,
    path: String,
    size: u64,
    sha256: String,
    identity: Identity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExternalAction {
    RestrictSource,
    ReviewBackupsAndExports,
    ReviewNativeFilesAndSessions,
    ReviewRemoteCopies,
    RetireOtherWriters,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    operation_id: String,
    source_tree: String,
    source_project: String,
    successor_tree: String,
    successor_project: String,
    successor_actor: String,
    share: bool,
    owner_lane: String,
    roots: BTreeMap<String, Root>,
    files: Vec<Member>,
    selection: Vec<Selector>,
    locations: Vec<Location>,
    destination: PathBuf,
    destination_parent: Identity,
    external_actions: Vec<ExternalAction>,
}

struct Pins {
    roots: BTreeMap<String, Dir>,
    files: Vec<File>,
    destination_parent: Dir,
}

pub(crate) struct CapturedPlan {
    envelope: Envelope,
    pins: Pins,
}

#[derive(Serialize)]
pub(crate) struct Summary<'a> {
    operation_id: &'a str,
    source_tree: &'a str,
    source_project: &'a str,
    successor_tree: &'a str,
    successor_project: &'a str,
    selection: &'a [Selector],
    locations: &'a [Location],
    root_count: usize,
    file_count: usize,
    external_actions: &'a [ExternalAction],
    plan_digest: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl CapturedPlan {
    pub(crate) fn capture(request: Request) -> Result<Self> {
        capture::capture(request).map_err(|_| refused())
    }

    pub(crate) fn encode_private(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(&self.envelope).map_err(|_| refused())?;
        if bytes.len() as u64 > crate::backup::MAX_MANIFEST {
            return Err(refused());
        }
        Ok(bytes)
    }

    pub(crate) fn decode_private(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > crate::backup::MAX_MANIFEST {
            return Err(refused());
        }
        let value = json::decode(bytes)?;
        let envelope: Envelope = serde_json::from_value(value).map_err(|_| refused())?;
        capture::decode(envelope).map_err(|_| refused())
    }

    pub(crate) fn plan_digest(&self) -> Result<String> {
        Ok(digest(&self.encode_private()?))
    }

    pub(crate) fn confirm(&self, plan_digest: &str) -> Result<()> {
        if self.plan_digest()? != plan_digest {
            return Err(refused());
        }
        Ok(())
    }

    pub(crate) fn summary(&self) -> Result<Summary<'_>> {
        Ok(Summary {
            operation_id: &self.envelope.operation_id,
            source_tree: &self.envelope.source_tree,
            source_project: &self.envelope.source_project,
            successor_tree: &self.envelope.successor_tree,
            successor_project: &self.envelope.successor_project,
            selection: &self.envelope.selection,
            locations: &self.envelope.locations,
            root_count: self.envelope.roots.len(),
            file_count: self.envelope.files.len(),
            external_actions: &self.envelope.external_actions,
            plan_digest: self.plan_digest()?,
        })
    }

    pub(crate) fn verify_unchanged(&self) -> Result<()> {
        capture::verify(self).map_err(|_| refused())
    }
}
