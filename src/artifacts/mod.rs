//! Selected local deliveries retain bytes outside the provenance tree.
pub(crate) mod bundle;
mod files;
mod guard;
mod media;
mod native;
pub(crate) mod preview;
pub(crate) mod projection;
mod runtime;
mod sqlite;

use crate::args::Args;
use crate::event::Body;
use crate::failure::Failure;
use crate::model::Tree;
use crate::ops::{Ctx, Whose};
use crate::store::{self, Store};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_PAGE_SIZE: usize = 100;

pub(crate) fn normalize_description(value: &str) -> Result<&str, Failure> {
    if value.len() > 1120
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err(Failure::usage(
            "Artifact description must be a single line of up to 280 Unicode characters.",
        ));
    }
    let description = value.trim();
    if description.is_empty() || description.chars().count() > 280 {
        return Err(Failure::usage(
            "Artifact description must contain between 1 and 280 Unicode characters.",
        ));
    }
    if let Some(found) = crate::redact::check_field("artifact description", description) {
        return Err(Failure::Redaction(Box::new(found)));
    }
    Ok(description)
}

pub(crate) fn deserialize_description<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    normalize_description(&value)
        .map_err(|_| serde::de::Error::custom("Invalid artifact description."))?;
    Ok(value)
}

fn capabilities() -> Value {
    let bundles = bundle::SUPPORTED;
    let bundle_capabilities = if bundles {
        json!({"supported":true,"formats":["html","css","js","png","mp4"],"max_bytes":MAX_FILE_BYTES,"max_members":64,"max_manifest_bytes":65536,
            "media": {"profile":"reviewed-media-v1", "review_required":true, "review_scope":"exact_member_digests", "review_evidence":"caller_attestation", "automatic_inspection":"structure_and_accessible_text", "visual_inspection":false, "audio_inspection":false,
                "png":{"profile":"static-png-v1","max_width":4096,"max_height":4096,"max_pixels":16777216,"max_decoded_bytes_per_revision":media::MAX_DECODED_BYTES,"interlaced":false,"max_metadata_bytes":65536,"max_chunks":1024,"unknown_chunks":"refused"},
                "mp4":{"profile":"h264-video-v1","fragmented":false,"audio":false,"max_tracks":1,"max_seconds":300,"max_samples":8192,"max_samples_per_revision":media::MAX_VIDEO_SAMPLES,"max_nal_units_per_revision":media::MAX_NAL_UNITS,"nal_budget_scope":"sample_nal_units","max_width":4096,"max_height":4096,"max_pixels":16000000,"max_metadata_bytes":65536,"max_boxes":1024,"sequence_parameter_sets":1,"picture_parameter_sets":1,"slice_groups":false,"sei":"utf8_user_data_unregistered","unknown_boxes":"refused"}}})
    } else {
        json!({"supported":false,"reason":"Complete bundle custody is not supported on this platform."})
    };
    let mut value = json!({
        "operation": "capabilities",
        "schema_version": 1,
        "scope": "engine",
        "description": {"required_for_capture": true, "max_characters": 280, "scope": "revision"},
        "delivery_types": {
            "file": {"supported": true, "formats": ["utf8_text"], "max_bytes": MAX_FILE_BYTES},
            "bundle": bundle_capabilities,
            "sqlite_application": {
                "supported": bundles,
                "formats": ["html", "css", "js", "sqlite3"],
                "profile": bundle::SQLITE_PROFILE,
                "transformation": "logical-export-v1",
                "journal_mode": "standalone_rollback",
                "encoding": "utf8",
                "auxiliary_files": "refused",
                "source_profiles": {
                    "sqlite-standalone-v1": {"supported": bundles, "auxiliary_files": "refused"},
                    "readonly-wal-v1": {
                        "supported": bundles,
                        "selection": "database_source",
                        "existing_inputs": ["database", "wal", "shm"],
                        "max_data_input_bytes": MAX_FILE_BYTES,
                        "max_wal_input_bytes": MAX_FILE_BYTES,
                        "shared_memory_contents_read": false,
                        "source_writes": false,
                        "retained_profile": bundle::SQLITE_PROFILE
                    }
                },
                "runtime": {"api":"initSqlJs", "generated_member":runtime::PATH, "known":[{"id":runtime::ID,"path":runtime::PATH,"bytes":runtime::BYTES,"sha256":runtime::SHA256}], "available_for_capture":[runtime::ID]},
                "max_bytes": MAX_FILE_BYTES,
                "max_members": 64,
                "max_manifest_bytes": 65536,
                "max_name_bytes": 128,
                "max_value_bytes": 65536,
                "max_row_bytes": 262144,
                "max_rows": 100000,
                "max_tables": 16,
                "max_columns_per_table": 32,
                "max_columns": 256,
                "max_indexes": 32,
                "max_schema_bytes": 65536,
                "max_vm_steps": 20000000,
                "timeout_seconds": 2,
                "max_sqlite_heap_bytes": 1048576,
                "max_auxiliary_bytes": 2097152
            },
            "sqlite_snapshot": {"supported": false, "reason": "Standalone database-only delivery is not supported; select one database in a complete SQLite application bundle."}
        },
        "limits": {"max_file_bytes": MAX_FILE_BYTES, "max_revision_bytes": MAX_FILE_BYTES, "max_files_per_revision": if bundles {64} else {1}, "max_manifest_bytes": if bundles {65536} else {0}, "max_page_size": MAX_PAGE_SIZE},
        "execution": {
            "provided": true,
            "bundle_preview": bundles,
            "scope": "retained_html_preview",
            "network_isolation": false,
            "approval_implied": false,
            "sandbox": "allow-scripts",
            "external_subresources": false,
            "eval": false,
            "server_lifetime_seconds": 900,
            "max_simultaneous_servers": 4,
            "request_timeout_seconds": 2,
            "response_timeout_seconds": 10
        },
        "bytes_checked": false,
        "event_recorded": false
    });
    if !bundles {
        value["delivery_types"]["sqlite_application"]["reason"] =
            json!("SQLite application custody is not supported on this platform.");
    }
    value
}

#[derive(Serialize)]
struct Revision<'a> {
    artifact: &'a str,
    revision: &'a str,
    operation: &'a str,
    producer: &'a str,
    previous: Option<&'a str>,
    source: &'a str,
    description: Option<&'a str>,
    site: &'a str,
    size: u64,
    digest: &'a str,
    object: &'a str,
    lane: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    member_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_bytes: Option<u64>,
    database: Option<&'a str>,
    profile: Option<&'a str>,
    runtime: Option<&'a str>,
    runtime_sha256: Option<&'a str>,
}

impl<'a> Revision<'a> {
    fn from_body(lane: &'a str, body: &'a Body) -> Option<Self> {
        let (artifact, revision, operation, producer, previous, source, site, size, digest, object) =
            match body {
                Body::ArtifactCaptured {
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source,
                    site,
                    size,
                    digest,
                    object,
                }
                | Body::ArtifactBundleCaptured {
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source,
                    site,
                    size,
                    digest,
                    object,
                    ..
                }
                | Body::ArtifactCapturedV2 {
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source,
                    site,
                    size,
                    digest,
                    object,
                    ..
                }
                | Body::ArtifactBundleCapturedV2 {
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source,
                    site,
                    size,
                    digest,
                    object,
                    ..
                }
                | Body::ArtifactSqliteApplicationCaptured {
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source,
                    site,
                    size,
                    digest,
                    object,
                    ..
                } => (
                    artifact, revision, operation, producer, previous, source, site, size, digest,
                    object,
                ),
                _ => return None,
            };
        let (delivery_type, selection, entrypoint, member_count, manifest_bytes, database, profile) =
            match body {
                Body::ArtifactBundleCaptured {
                    selection,
                    entrypoint,
                    member_count,
                    manifest_bytes,
                    ..
                }
                | Body::ArtifactBundleCapturedV2 {
                    selection,
                    entrypoint,
                    member_count,
                    manifest_bytes,
                    ..
                } => (
                    Some("bundle"),
                    Some(selection.as_str()),
                    Some(entrypoint.as_str()),
                    Some(*member_count),
                    Some(*manifest_bytes),
                    None,
                    None,
                ),
                Body::ArtifactSqliteApplicationCaptured {
                    selection,
                    entrypoint,
                    member_count,
                    manifest_bytes,
                    database,
                    profile,
                    ..
                } => (
                    Some("sqlite_application"),
                    Some(selection.as_str()),
                    Some(entrypoint.as_str()),
                    Some(*member_count),
                    Some(*manifest_bytes),
                    Some(database.as_str()),
                    Some(profile.as_str()),
                ),
                _ => (None, None, None, None, None, None, None),
            };
        let (runtime, runtime_sha256) = match body {
            Body::ArtifactSqliteApplicationCaptured {
                runtime,
                runtime_sha256,
                ..
            } => (Some(runtime.as_str()), Some(runtime_sha256.as_str())),
            _ => (None, None),
        };
        Some(Self {
            artifact,
            revision,
            operation,
            producer,
            previous: previous.as_deref(),
            source,
            description: match body {
                Body::ArtifactCapturedV2 { description, .. }
                | Body::ArtifactBundleCapturedV2 { description, .. }
                | Body::ArtifactSqliteApplicationCaptured { description, .. } => {
                    Some(description.as_str())
                }
                _ => None,
            },
            site,
            size: *size,
            digest,
            object,
            lane,
            delivery_type,
            selection,
            entrypoint,
            member_count,
            manifest_bytes,
            database,
            profile,
            runtime,
            runtime_sha256,
        })
    }

    fn is_bundle(&self) -> bool {
        matches!(self.delivery_type, Some("bundle" | "sqlite_application"))
    }
}

#[derive(Default)]
struct Ledger<'a> {
    history: &'a [(String, Body)],
    projection: Option<&'a projection::Projection>,
    revisions: BTreeMap<&'a str, usize>,
    latest: BTreeMap<&'a str, &'a str>,
    operations: BTreeMap<&'a str, &'a str>,
    invalid_receipts: usize,
    ambiguous_receipts: usize,
    unresolved_receipts: usize,
}

impl<'a> Ledger<'a> {
    fn at(&self, index: usize) -> Option<Revision<'a>> {
        let (lane, body) = self.history.get(index)?;
        Revision::from_body(lane, body)
    }

    fn get(&self, id: &str) -> Option<Revision<'a>> {
        let index = self
            .projection
            .and_then(|p| p.revision_index(id))
            .or_else(|| self.revisions.get(id).copied())?;
        self.at(index)
    }

    fn latest(&self, artifact: &str) -> Option<Revision<'a>> {
        self.projection
            .and_then(|p| p.latest_index(artifact))
            .and_then(|i| self.at(i))
            .or_else(|| self.latest.get(artifact).and_then(|id| self.get(id)))
    }

    fn operation(&self, operation: &str) -> Option<Revision<'a>> {
        self.projection
            .and_then(|p| p.operation_index(operation))
            .and_then(|i| self.at(i))
            .or_else(|| self.operations.get(operation).and_then(|id| self.get(id)))
    }

    fn count(&self) -> usize {
        self.projection.map_or(self.revisions.len(), |p| p.count())
    }

    fn entries(&self) -> impl Iterator<Item = Revision<'a>> + '_ {
        self.revisions
            .values()
            .copied()
            .chain(
                self.projection
                    .into_iter()
                    .flat_map(|p| p.revisions().map(|(_, i)| i)),
            )
            .filter_map(|i| self.at(i))
    }

    fn latest_entries(&self) -> impl Iterator<Item = Revision<'a>> + '_ {
        self.latest.values().filter_map(|id| self.get(id)).chain(
            self.projection
                .into_iter()
                .flat_map(|p| p.latest())
                .filter_map(|(_, i)| self.at(i)),
        )
    }
}

fn valid_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|b| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&b))
        && value.as_bytes()[0] <= b'7'
}

fn guard_source_reference(source: &str) -> Result<(), Failure> {
    if source.is_empty() || source.len() > 4096 {
        return Err(Failure::usage(
            "Artifact source reference must contain between 1 and 4096 bytes.",
        ));
    }
    for value in std::iter::once(source).chain(source.split(['/', '\\'])) {
        if let Some(found) = crate::redact::check_field("artifact reference", value) {
            return Err(Failure::Redaction(Box::new(found)));
        }
    }
    Ok(())
}

fn metadata(revision: &Revision) -> Result<(), Failure> {
    if let Some(description) = revision.description {
        normalize_description(description)?;
    }
    if ![
        revision.artifact,
        revision.revision,
        revision.operation,
        revision.producer,
        revision.site,
    ]
    .into_iter()
    .all(valid_id)
        || revision
            .previous
            .as_ref()
            .is_some_and(|value| !valid_id(value))
        || revision.size > MAX_FILE_BYTES
        || revision.digest.len() != 64
        || !revision
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || revision.object
            != format!(
                "objects/{}.{}",
                revision.revision,
                if revision.is_bundle() {
                    "bundle"
                } else {
                    "blob"
                }
            )
        || revision.source.is_empty()
        || revision.source.len() > 4096
    {
        return Err(Failure::Model("Invalid artifact custody metadata.".into()));
    }
    guard_source_reference(revision.source)?;
    if revision.delivery_type == Some("sqlite_application") {
        let database = revision
            .database
            .ok_or_else(|| Failure::Model("Missing SQLite database reference.".into()))?;
        guard_source_reference(database)?;
        bundle::validate_database_path(database)?;
        if revision.profile != Some(bundle::SQLITE_PROFILE) {
            return Err(Failure::Model(
                "Unsupported SQLite application profile.".into(),
            ));
        }
        let descriptor = runtime::known_descriptor(
            revision
                .runtime
                .ok_or_else(|| Failure::Model("Missing product runtime identity.".into()))?,
        )?;
        if revision.runtime_sha256 != Some(descriptor.sha256.as_str()) {
            return Err(Failure::Model(
                "Product runtime receipt does not match the engine catalog.".into(),
            ));
        }
    } else if revision.database.is_some()
        || revision.profile.is_some()
        || revision.runtime.is_some()
        || revision.runtime_sha256.is_some()
    {
        return Err(Failure::Model(
            "Unexpected SQLite application metadata.".into(),
        ));
    }
    if revision.is_bundle() {
        let selection = revision
            .selection
            .ok_or_else(|| Failure::Model("Missing bundle selection reference.".into()))?;
        guard_source_reference(selection)?;
        files::source_path(selection)?;
        if revision.source != "." {
            files::source_path(revision.source)?;
        }
        let entrypoint = revision
            .entrypoint
            .ok_or_else(|| Failure::Model("Missing bundle entrypoint.".into()))?;
        guard_source_reference(entrypoint)?;
        bundle::validate_path(entrypoint)?;
        if !entrypoint
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("html"))
            || !revision
                .member_count
                .is_some_and(|count| (1..=64).contains(&count))
            || !revision
                .manifest_bytes
                .is_some_and(|bytes| bytes > 0 && bytes <= 65536 && bytes <= revision.size)
        {
            return Err(Failure::Model("Invalid bundle custody metadata.".into()));
        }
    }
    if let Some(found) = crate::redact::check_field("artifact reference", revision.lane) {
        return Err(Failure::Redaction(Box::new(found)));
    }
    Ok(())
}

fn replay(tree: &Tree) -> Result<Ledger<'_>, Failure> {
    if !tree
        .artifact_projection
        .is_valid(tree.artifact_history.len())
    {
        return Err(Failure::Model(
            "Artifact custody contains a conflicting or unresolved delivery.".into(),
        ));
    }
    Ok(Ledger {
        history: &tree.artifact_history,
        projection: Some(&tree.artifact_projection),
        ..Ledger::default()
    })
}

fn diagnostic_ledger(tree: &Tree) -> Ledger<'_> {
    let mut ledger = Ledger {
        history: &tree.artifact_history,
        ..Ledger::default()
    };
    let mut revisions = BTreeMap::<&str, usize>::new();
    let mut operations = BTreeMap::<&str, usize>::new();
    for (lane, body) in &tree.artifact_history {
        if let Some(entry) = Revision::from_body(lane, body) {
            *revisions.entry(entry.revision).or_default() += 1;
            *operations.entry(entry.operation).or_default() += 1;
        }
    }
    for (index, (lane, body)) in tree.artifact_history.iter().enumerate() {
        let Some(entry) = Revision::from_body(lane, body) else {
            ledger.invalid_receipts += 1;
            continue;
        };
        if metadata(&entry).is_err() {
            ledger.invalid_receipts += 1;
        } else if revisions[entry.revision] != 1 || operations[entry.operation] != 1 {
            ledger.ambiguous_receipts += 1;
        } else {
            ledger.revisions.insert(entry.revision, index);
        }
    }
    drop(revisions);
    drop(operations);
    let mut latest = BTreeMap::new();
    for (index, (lane, body)) in tree.artifact_history.iter().enumerate() {
        let Some(entry) = Revision::from_body(lane, body) else {
            continue;
        };
        if ledger.revisions.get(entry.revision).copied() != Some(index) {
            continue;
        }
        if tree.node(entry.producer).is_none()
            || latest.get(entry.artifact).copied() != entry.previous
            || entry
                .previous
                .and_then(|id| ledger.get(id))
                .is_some_and(|previous| previous.delivery_type != entry.delivery_type)
        {
            ledger.unresolved_receipts += 1;
        }
        latest.insert(entry.artifact, entry.revision);
    }
    ledger
}

fn context(cwd: &Path, diagnose: bool, capture: bool) -> Result<Ctx, Failure> {
    let located = store::locate(cwd)?.ok_or(Failure::NoStore)?;
    let store = Store::open_strict(located.root.clone())?;
    let ctx = if capture {
        Ctx::load_for_capture(store, Whose::Resolved(&located))?
    } else {
        Ctx::load_for_write(store, Whose::Resolved(&located))?
    };
    if ctx.tree.broken_lines != 0 && !diagnose {
        return Err(Failure::Model(
            "Artifact custody refuses an unreadable event log.".into(),
        ));
    }
    Ok(ctx)
}

fn required<'a>(args: &'a Args, key: &str) -> Result<&'a str, Failure> {
    args.opt(key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Failure::usage(format!("Artifact operation requires --{key}.")))
}

fn validate(args: &Args) -> Result<&str, Failure> {
    let operation = args.positional(0).unwrap_or("list");
    let allowed: &[&str] = match operation {
        "capabilities" => &["json"],
        "list" => &["json", "all", "from", "limit", "offset"],
        "show" => &["json", "revision", "limit", "offset"],
        "capture" => &["json", "from", "artifact", "previous", "operation", "bundle", "description"],
        "check" if args.has("store") => &["json", "revision", "store", "limit", "offset"],
        "check" => &["json", "revision", "store"],
        "restore" => &["json", "revision", "to", "orphan"],
        "preview" => &["json", "revision", "no-open"],
        _ => return Err(Failure::usage(
            "Unknown artifact operation; use capabilities, list, show, capture, check, restore or preview.",
        )),
    };
    if !args.unknown(allowed).is_empty()
        || !args
            .extra(if operation == "list" || operation == "capabilities" {
                usize::from(args.positional(0).is_some())
            } else {
                2
            })
            .is_empty()
        || allowed.iter().any(|key| {
            !["json", "all", "store", "no-open"].contains(key)
                && args.has(key)
                && args.list(key).len() != 1
        })
    {
        return Err(Failure::usage("Unsupported or repeated artifact argument."));
    }
    if operation == "preview" {
        let artifact = args
            .positional(1)
            .ok_or_else(|| Failure::usage("A stable artifact identity is required."))?;
        let revision = required(args, "revision")?;
        if !valid_id(artifact) || !valid_id(revision) {
            return Err(Failure::usage("Invalid artifact or revision identifier."));
        }
    }
    if operation == "capture" {
        normalize_description(required(args, "description")?)?;
    }
    if operation == "capture" && (args.has("bundle") == args.positional(1).is_some()) {
        return Err(Failure::usage(
            "Capture requires exactly one source path or --bundle selector.",
        ));
    }
    if operation == "capture" && args.has("bundle") && !bundle::SUPPORTED {
        return Err(Failure::usage(
            "Complete bundle custody is not supported on this platform.",
        ));
    }
    if operation == "check"
        && args.has("store")
        && (args.positional(1).is_some() || args.has("revision"))
    {
        return Err(Failure::usage(
            "--store checks the store and does not take an artifact or revision.",
        ));
    }
    if operation == "restore"
        && args.has("orphan")
        && (args.positional(1).is_some() || args.has("revision"))
    {
        return Err(Failure::usage(
            "--orphan names an unrecorded object, not an artifact revision.",
        ));
    }
    if operation == "list" && args.has("all") && args.has("from") {
        return Err(Failure::usage("Choose --all or --from, not both."));
    }
    Ok(operation)
}

fn revision<'a>(ledger: &Ledger<'a>, args: &Args) -> Result<Revision<'a>, Failure> {
    let artifact = args
        .positional(1)
        .ok_or_else(|| Failure::usage("A stable artifact identity is required."))?;
    let id = args
        .opt("revision")
        .or_else(|| ledger.latest(artifact).map(|r| r.revision))
        .ok_or_else(|| Failure::usage("No artifact has this stable identity."))?;
    ledger
        .get(id)
        .filter(|revision| revision.artifact == artifact)
        .ok_or_else(|| Failure::usage("This revision does not belong to the selected artifact."))
}

fn references(tree: &Tree, revision: &Revision<'_>) -> (Vec<Value>, usize) {
    let reference = format!("artifact:{}@{}", revision.artifact, revision.revision);
    let mut entries = Vec::new();
    let mut total = 0;
    for node in tree
        .nodes_iter()
        .filter(|node| node.refs(tree).contains(&reference.as_str()))
    {
        total += 1;
        if entries.len() < 100 {
            entries.push(json!({"node": node.id, "alias": node.alias(), "kind": node.kind.word(), "state": node.state.word(node.kind)}));
        }
    }
    (entries, total)
}

fn describe(tree: &Tree, revision: &Revision) -> Value {
    let mut value = serde_json::to_value(revision).expect("artifact metadata is serializable");
    value["reference"] = json!(format!(
        "artifact:{}@{}",
        revision.artifact, revision.revision
    ));
    value["retained_check"] = json!("unverified");
    if let Some(producer) = tree.node(revision.producer) {
        value["producer_node"] =
            json!({"node":producer.id,"alias":producer.alias(),"title":producer.title(tree)});
    }
    let (references, total) = references(tree, revision);
    value["decision_references"] = json!(references);
    value["decision_references_total"] = json!(total);
    value["decision_references_truncated"] = json!(total > 100);
    value
}

fn artifact_references(tree: &Tree, ledger: &Ledger, artifact: &str) -> (Vec<Value>, usize) {
    let mut entries = Vec::new();
    let mut total = 0;
    for node in tree
        .nodes_iter()
        .filter(|node| node.kind == crate::event::Kind::Decision)
    {
        let refs = node.refs(tree);
        for (index, reference) in refs.iter().enumerate() {
            if refs[..index].contains(reference) {
                continue;
            }
            let Some((identity, revision)) = reference
                .strip_prefix("artifact:")
                .and_then(|reference| reference.split_once('@'))
            else {
                continue;
            };
            if identity != artifact
                || ledger
                    .get(revision)
                    .is_none_or(|revision| revision.artifact != artifact)
            {
                continue;
            }
            total += 1;
            if entries.len() < 100 {
                entries.push(json!({"revision":revision,"node":node.id,"alias":node.alias(),"kind":node.kind.word(),"state":node.state.word(node.kind)}));
            }
        }
    }
    (entries, total)
}

fn describe_list(tree: &Tree, ledger: &Ledger, revision: &Revision) -> Value {
    let mut value = describe(tree, revision);
    let (references, total) = artifact_references(tree, ledger, revision.artifact);
    value["artifact_decision_references"] = json!(references);
    value["artifact_decision_references_total"] = json!(total);
    value["artifact_decision_references_truncated"] = json!(total > 100);
    value
}

fn list(ctx: &Ctx, ledger: &Ledger, args: &Args) -> Result<Value, Failure> {
    let number = |key: &str, default: usize| -> Result<usize, Failure> {
        args.opt(key).map_or(Ok(default), |s| {
            s.parse()
                .map_err(|_| Failure::usage("Pagination values must be nonnegative integers."))
        })
    };
    let limit = number("limit", 20)?;
    let offset = number("offset", 0)?;
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(Failure::usage(
            "Artifact page limit must be between 1 and 100.",
        ));
    }
    let node = match args.opt("from") {
        Some(id) => Some(
            ctx.tree
                .resolve(id)
                .ok_or_else(|| Failure::usage("The requested node does not exist."))?,
        ),
        None if !args.has("all") => ctx.tree.focus(),
        None => None,
    };
    let mut scope = HashSet::new();
    if let Some(node) = node {
        scope.insert(node.id.as_str());
        scope.extend(ctx.tree.ancestors(node.num).iter().map(|n| n.id.as_str()));
    }
    let linked_refs: HashSet<_> = ctx
        .tree
        .nodes_iter()
        .filter(|n| scope.contains(n.id.as_str()))
        .flat_map(|n| n.refs(&ctx.tree))
        .collect();
    let selected = || {
        ledger.latest_entries().filter(|revision| {
            (args.has("all") || node.is_some())
                && (args.has("all")
                    || scope.contains(revision.producer)
                    || linked_refs.contains(
                        format!("artifact:{}@{}", revision.artifact, revision.revision).as_str(),
                    ))
        })
    };
    let total = selected().count();
    let entries: Vec<_> = selected()
        .skip(offset)
        .take(limit)
        .map(|r| describe_list(&ctx.tree, ledger, &r))
        .collect();
    let next = offset.saturating_add(entries.len());
    Ok(
        json!({"operation":"list", "artifacts":entries,"total":total,"next_offset": if next < total {Some(next)} else {None},"scope":if args.has("all") {"project"} else {"focus_lineage"},"bytes_checked":false}),
    )
}

enum CapturePlan {
    AlreadyRecorded(Value),
    Prepared(Box<Body>),
}

fn prepare_capture(ctx: &Ctx, args: &Args) -> Result<CapturePlan, Failure> {
    let description = normalize_description(required(args, "description")?)?;
    let is_bundle = args.has("bundle");
    let source = args
        .opt("bundle")
        .or_else(|| args.positional(1))
        .ok_or_else(|| Failure::usage("Capture requires a project-relative source path."))?;
    if is_bundle {
        guard_source_reference(source)?;
        files::source_path(source)?;
    }
    let producer = ctx
        .tree
        .resolve(required(args, "from")?)
        .ok_or_else(|| Failure::usage("The producer node does not exist."))?
        .id
        .clone();
    let ledger = replay(&ctx.tree)?;
    let operation = args
        .opt("operation")
        .map(str::to_string)
        .unwrap_or_else(crate::id::ulid);
    if !valid_id(&operation) {
        return Err(Failure::usage(
            "Operation identity must be a valid opaque ULID.",
        ));
    }
    if let Some(recorded) = ledger.operation(operation.as_str()) {
        if recorded.description.map(str::trim) != Some(description)
            || recorded.is_bundle() != is_bundle
            || (if is_bundle {
                recorded.selection
            } else {
                Some(recorded.source)
            }) != Some(source)
            || recorded.producer != producer
            || args
                .opt("artifact")
                .is_some_and(|id| id != recorded.artifact)
            || (args.has("previous") && args.opt("previous") != recorded.previous)
        {
            return Err(Failure::Model(
                "This operation identity already names a different delivery.".into(),
            ));
        }
        return Ok(CapturePlan::AlreadyRecorded(
            json!({"operation":"capture","already_recorded":true,"delivery":describe(&ctx.tree,&recorded)}),
        ));
    }
    let artifact = args
        .opt("artifact")
        .map(str::to_string)
        .unwrap_or_else(crate::id::ulid);
    if !valid_id(&artifact) {
        return Err(Failure::usage("Artifact identity must be an opaque ULID."));
    }
    if ledger
        .latest(artifact.as_str())
        .is_some_and(|revision| revision.is_bundle() != is_bundle)
    {
        return Err(Failure::Model(
            "An artifact identity cannot change its delivery type.".into(),
        ));
    }
    let previous = ledger
        .latest(artifact.as_str())
        .map(|r| r.revision.to_owned());
    if args.has("artifact") && previous.is_none() {
        return Err(Failure::usage("No artifact has this stable identity."));
    }
    if args.has("previous") && args.opt("previous") != previous.as_deref() {
        return Err(Failure::Model(
            "The expected previous revision is no longer current.".into(),
        ));
    }
    let previous_delivery_type = ledger
        .latest(artifact.as_str())
        .and_then(|revision| revision.delivery_type);
    drop(ledger);
    let revision = crate::id::ulid();
    if is_bundle {
        if let Some(found) = crate::redact::check_field(
            "artifact reference",
            ctx.lane.as_deref().unwrap_or_default(),
        ) {
            return Err(Failure::Redaction(Box::new(found)));
        }
        let selection = bundle::load_selection(&ctx.lane_dir, source)?;
        guard_source_reference(&selection.root)?;
        bundle::validate_selection(&selection)?;
        let delivery_type = if selection.database.is_some() {
            "sqlite_application"
        } else {
            "bundle"
        };
        if previous_delivery_type.is_some_and(|previous| previous != delivery_type) {
            return Err(Failure::Model(
                "An artifact identity cannot change its delivery type.".into(),
            ));
        }
        let retained = bundle::capture(&ctx.lane_dir, &selection, &revision)?;
        if let Some(database) = selection.database {
            let runtime = retained.runtime.ok_or_else(|| {
                Failure::Model("Missing generated product runtime metadata.".into())
            })?;
            return Ok(CapturePlan::Prepared(Box::new(
                Body::ArtifactSqliteApplicationCaptured {
                    description: description.into(),
                    artifact,
                    revision,
                    operation,
                    producer,
                    previous,
                    source: selection.root,
                    selection: source.into(),
                    site: retained.retained.site,
                    size: retained.retained.size,
                    digest: retained.retained.digest,
                    object: retained.retained.object,
                    entrypoint: retained.entrypoint,
                    member_count: retained.members,
                    manifest_bytes: retained.manifest_bytes,
                    database,
                    profile: bundle::SQLITE_PROFILE.into(),
                    runtime: runtime.id,
                    runtime_sha256: runtime.sha256,
                },
            )));
        }
        let body = Body::ArtifactBundleCapturedV2 {
            description: description.into(),
            artifact,
            revision,
            operation,
            producer,
            previous,
            source: selection.root,
            selection: source.into(),
            site: retained.retained.site,
            size: retained.retained.size,
            digest: retained.retained.digest,
            object: retained.retained.object,
            entrypoint: retained.entrypoint,
            member_count: retained.members,
            manifest_bytes: retained.manifest_bytes,
        };
        return Ok(CapturePlan::Prepared(Box::new(body)));
    }
    guard_source_reference(source)?;
    let retained = files::capture(&ctx.lane_dir, source, &revision)?;
    let body = Body::ArtifactCapturedV2 {
        description: description.into(),
        artifact,
        revision: revision.clone(),
        operation,
        producer,
        previous,
        source: source.into(),
        site: retained.site,
        size: retained.size,
        digest: retained.digest,
        object: retained.object,
    };
    Ok(CapturePlan::Prepared(Box::new(body)))
}

fn capture(ctx: &mut Ctx, args: &Args) -> Result<Value, Failure> {
    match prepare_capture(ctx, args)? {
        CapturePlan::AlreadyRecorded(value) => Ok(value),
        CapturePlan::Prepared(body) => commit_capture(ctx, *body),
    }
}

fn commit_capture(ctx: &mut Ctx, body: Body) -> Result<Value, Failure> {
    commit_capture_with(ctx, body, |ctx, body| ctx.emit(vec![body]))
}

fn commit_capture_with(
    ctx: &mut Ctx,
    body: Body,
    emit: impl FnOnce(&mut Ctx, Body) -> Result<(), Failure>,
) -> Result<Value, Failure> {
    // File I/O is complete before the tree's lock is acquired.
    ctx.lock_for_write()?;
    if ctx.tree.broken_lines != 0 {
        return Err(Failure::Model("The event log became unreadable; retained bytes are unrecorded. Run artifacts check --store.".into()));
    }
    let now = replay(&ctx.tree)?;
    let lane = ctx.lane.clone().unwrap_or_default();
    let entry = Revision::from_body(&lane, &body).expect("capture constructs custody metadata");
    if now.latest(entry.artifact).map(|r| r.revision) != entry.previous
        || now
            .latest(entry.artifact)
            .is_some_and(|revision| revision.delivery_type != entry.delivery_type)
        || now.operation(entry.operation).is_some()
        || ctx.tree.node(entry.producer).is_none()
    {
        return Err(Failure::Model("A concurrent delivery changed the plan; retained bytes are unrecorded. Run artifacts check --store.".into()));
    }
    // Pending lanes are declared by emit; their event lane remains the authority.
    metadata(&entry)?;
    drop(now);
    let revision = entry.revision.to_owned();
    emit(ctx, body)?;
    let recorded = replay(&ctx.tree)?;
    Ok(
        json!({"operation":"capture","already_recorded":false,"delivery":describe(&ctx.tree,&recorded.get(&revision).expect("the emitted revision is recorded")),"source_snapshot":"not_proven","retention":"local_only"}),
    )
}

fn checked(result: Result<Option<(String, u64)>, Failure>, revision: &Revision) -> Value {
    match result {
        Ok(Some((digest, size))) if digest == revision.digest && size == revision.size => {
            json!({"status":"matches"})
        }
        Ok(Some(_)) => json!({"status":"changed"}),
        Ok(None) => json!({"status":"missing"}),
        Err(error) => json!({"status":"read_failed","exit_code":error.code()}),
    }
}

fn bundle_manifest(
    root: &Path,
    revision: &Revision,
    full: bool,
) -> Result<bundle::Manifest, Failure> {
    let manifest = if full {
        bundle::check(root, revision.object, revision.digest, revision.size)?
    } else {
        bundle::read_manifest(root, revision.object, revision.digest, revision.size)?
    };
    if revision.entrypoint != Some(manifest.entrypoint.as_str())
        || revision.member_count != Some(manifest.members.len())
        || revision.manifest_bytes != Some(manifest.manifest_bytes)
        || revision.delivery_type != Some(manifest.delivery_type.as_str())
        || revision.database
            != manifest
                .database
                .as_ref()
                .map(|database| database.path.as_str())
        || revision.profile
            != manifest
                .database
                .as_ref()
                .map(|database| database.profile.as_str())
        || revision.runtime != manifest.runtime.as_ref().map(|runtime| runtime.id.as_str())
        || revision.runtime_sha256
            != manifest
                .runtime
                .as_ref()
                .map(|runtime| runtime.sha256.as_str())
    {
        return Err(Failure::Model(
            "Bundle manifest does not match its custody receipt.".into(),
        ));
    }
    Ok(manifest)
}

fn retained_check(root: &Path, revision: &Revision) -> Value {
    if revision.is_bundle() {
        match bundle_manifest(root, revision, true) {
            Ok(_) => json!({"status":"matches"}),
            Err(error) => json!({"status":"read_failed","exit_code":error.code()}),
        }
    } else {
        checked(files::inspect_retained(root, revision.object), revision)
    }
}

fn check(ctx: &Ctx, ledger: &Ledger, args: &Args) -> Result<(Value, i32), Failure> {
    if args.has("store") {
        let number = |key: &str, default: usize| -> Result<usize, Failure> {
            args.opt(key).map_or(Ok(default), |value| {
                value
                    .parse()
                    .map_err(|_| Failure::usage("Pagination values must be nonnegative integers."))
            })
        };
        let limit = number("limit", 20)?;
        let offset = number("offset", 0)?;
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Failure::usage(
                "Artifact page limit must be between 1 and 100.",
            ));
        }
        let page_end = offset.saturating_add(limit);
        let complete = ctx.tree.broken_lines == 0;
        let provenance_complete = complete
            && ledger.invalid_receipts == 0
            && ledger.ambiguous_receipts == 0
            && ledger.unresolved_receipts == 0;
        let mut failed = !provenance_complete;
        let local = files::site_id(&ctx.lane_dir)?;
        let mut recorded = Vec::new();
        let recorded_total = ledger.count();
        for (index, revision) in ledger.entries().enumerate() {
            let retained = retained_check(&ctx.lane_dir, &revision);
            failed |= retained["status"] != "matches";
            if (offset..page_end).contains(&index) {
                recorded.push(json!({"artifact":revision.artifact,"revision":revision.revision,"site":revision.site,"local_site":local,"retained":retained}));
            }
        }
        let mut orphans = Vec::new();
        let mut unattributed = Vec::new();
        let mut temporary = Vec::new();
        let mut bundle_objects = Vec::new();
        let mut orphans_total = 0usize;
        let mut unattributed_total = 0usize;
        let mut temporaries_total = 0usize;
        // Inventory pages follow filesystem enumeration order. The traversal
        // checks every entry and does not promise a filesystem snapshot.
        files::for_each_object(&ctx.lane_dir, |object| {
            let known = object
                .strip_prefix("objects/")
                .and_then(|name| {
                    name.strip_suffix(".blob")
                        .or_else(|| name.strip_suffix(".bundle"))
                })
                .and_then(|id| ledger.get(id))
                .is_some_and(|receipt| receipt.object == object);
            if known {
                return Ok(());
            }
            failed = true;
            let (entries, total) = if !object.ends_with(".blob") && !object.ends_with(".bundle") {
                (&mut temporary, &mut temporaries_total)
            } else if provenance_complete {
                (&mut orphans, &mut orphans_total)
            } else {
                (&mut unattributed, &mut unattributed_total)
            };
            if object.ends_with(".bundle") {
                let status = match bundle::inspect_retained(&ctx.lane_dir, &object) {
                    Ok(Some(_)) => json!({"status":"matches"}),
                    Ok(None) => json!({"status":"missing"}),
                    Err(error) => json!({"status":"read_failed","exit_code":error.code()}),
                };
                if (offset..page_end).contains(total) {
                    bundle_objects
                        .push(json!({"object":object,"retained":status,"provenance":"unknown"}));
                }
            }
            if (offset..page_end).contains(total) {
                entries.push(object);
            }
            *total += 1;
            Ok(())
        })?;
        let next_offset = [
            recorded_total,
            orphans_total,
            unattributed_total,
            temporaries_total,
        ]
        .into_iter()
        .any(|total| total > page_end)
        .then_some(page_end);
        let mut value = json!({
            "operation":"check", "scope":"store", "recorded":recorded,
            "recorded_total":recorded_total, "orphans":orphans, "orphans_total":orphans_total,
            "unattributed":unattributed, "unattributed_total":unattributed_total,
            "temporaries":temporary, "temporaries_total":temporaries_total,
            "next_offset":next_offset, "log_integrity":if complete {"complete"} else {"incomplete"},
            "broken_lines":ctx.tree.broken_lines, "provenance_complete":provenance_complete,
            "invalid_receipts":ledger.invalid_receipts, "ambiguous_receipts":ledger.ambiguous_receipts,
            "unresolved_receipts":ledger.unresolved_receipts, "provenance_reconstructed":false,
            "checked_at":crate::clock::now_rfc3339()
        });
        if !bundle_objects.is_empty() {
            value["bundle_objects"] = json!(bundle_objects);
        }
        return Ok((value, i32::from(failed)));
    }
    let revision = revision(ledger, args)?;
    if revision.is_bundle() {
        let result = bundle_manifest(&ctx.lane_dir, &revision, true);
        let retained = match &result {
            Ok(_) => json!({"status":"matches"}),
            Err(error) => json!({"status":"read_failed","exit_code":error.code()}),
        };
        let source = if files::site_id(&ctx.lane_dir)?.as_deref() != Some(revision.site) {
            json!({"status":"foreign_site","bytes_checked":false})
        } else if let Ok(manifest) = &result {
            bundle::source_check(&ctx.lane_dir, revision.source, revision.selection, manifest)
        } else {
            json!({"status":"not_checked","bytes_checked":false})
        };
        let code = i32::from(retained["status"] != "matches" || source["status"] != "matches");
        let value = json!({"operation":"check","artifact":revision.artifact,"revision":revision.revision,"source":source,"retained":retained,"checked_at":crate::clock::now_rfc3339(),"approval":"not_inferred"});
        return Ok((value, code));
    }
    let retained = retained_check(&ctx.lane_dir, &revision);
    let source = if files::site_id(&ctx.lane_dir)?.as_deref() == Some(revision.site) {
        checked(files::inspect(&ctx.lane_dir, revision.source), &revision)
    } else {
        json!({"status":"foreign_site","bytes_checked":false})
    };
    let code = i32::from(retained["status"] != "matches" || source["status"] != "matches");
    Ok((
        json!({"operation":"check","artifact":revision.artifact,"revision":revision.revision,"source":source,"retained":retained,"checked_at":crate::clock::now_rfc3339(),"approval":"not_inferred"}),
        code,
    ))
}

fn restore(ctx: &Ctx, ledger: &Ledger, args: &Args) -> Result<Value, Failure> {
    let to = required(args, "to")?;
    if let Some(orphan) = args.opt("orphan") {
        if !valid_id(orphan) {
            return Err(Failure::usage(
                "Orphan identity must be an opaque revision ULID.",
            ));
        }
        if ctx.tree.artifact_history.iter().any(|(_, body)| {
            matches!(body, Body::ArtifactCaptured { revision, .. } | Body::ArtifactBundleCaptured { revision, .. } | Body::ArtifactCapturedV2 { revision, .. } | Body::ArtifactBundleCapturedV2 { revision, .. } | Body::ArtifactSqliteApplicationCaptured { revision, .. } if revision == orphan)
        }) {
            return Err(Failure::usage(
                "This object has a recorded revision; restore its artifact identity.",
            ));
        }
        let object = format!("objects/{orphan}.blob");
        let bundle_object = format!("objects/{orphan}.bundle");
        let file = files::inspect_retained(&ctx.lane_dir, &object)?;
        let bundle = bundle::inspect_retained(&ctx.lane_dir, &bundle_object)?;
        match (file, bundle) {
            (Some((digest, size)), None) => {
                files::restore(&ctx.lane_dir, &object, &digest, size, to)?
            }
            (None, Some((digest, size))) => {
                bundle::restore(&ctx.lane_dir, &bundle_object, &digest, size, to)?
            }
            (None, None) => return Err(Failure::usage("No unrecorded object has this identity.")),
            (Some(_), Some(_)) => {
                return Err(Failure::usage(
                    "This unrecorded identity is ambiguous; inspect the store.",
                ))
            }
        }
        return Ok(
            json!({"operation":"restore","orphan":orphan,"destination":to,"provenance":"unknown","event_recorded":false}),
        );
    }
    let revision = revision(ledger, args)?;
    if revision.is_bundle() {
        required(args, "revision")?;
        bundle_manifest(&ctx.lane_dir, &revision, false)?;
        bundle::restore(
            &ctx.lane_dir,
            revision.object,
            revision.digest,
            revision.size,
            to,
        )?;
    } else {
        files::restore(
            &ctx.lane_dir,
            revision.object,
            revision.digest,
            revision.size,
            to,
        )?;
    }
    Ok(
        json!({"operation":"restore","artifact":revision.artifact,"revision":revision.revision,"destination":to,"event_recorded":false}),
    )
}

fn execute(cwd: &Path, args: &Args) -> Result<(Value, i32), Failure> {
    let operation = validate(args)?;
    if operation == "preview" {
        return Err(Failure::usage(
            "Artifact preview is interactive; use the CLI or web.",
        ));
    }
    if operation == "capabilities" {
        return Ok((capabilities(), 0));
    }
    let diagnose = (operation == "check" && args.has("store"))
        || (operation == "restore" && args.has("orphan"));
    let mut ctx = context(cwd, diagnose, operation == "capture")?;
    if operation == "capture" {
        return capture(&mut ctx, args).map(|value| (value, 0));
    }
    execute_read(&ctx, args)
}

fn execute_read(ctx: &Ctx, args: &Args) -> Result<(Value, i32), Failure> {
    let operation = validate(args)?;
    if operation == "preview" {
        return Err(Failure::usage(
            "Artifact preview is interactive; use the CLI or web.",
        ));
    }
    let diagnose = (operation == "check" && args.has("store"))
        || (operation == "restore" && args.has("orphan"));
    if ctx.tree.broken_lines != 0 && !diagnose {
        return Err(Failure::Model(
            "Artifact custody refuses an unreadable event log.".into(),
        ));
    }
    let ledger = if diagnose {
        diagnostic_ledger(&ctx.tree)
    } else {
        replay(&ctx.tree)?
    };
    match operation {
        "list" => list(ctx, &ledger, args).map(|value| (value, 0)),
        "show" => {
            let selected = revision(&ledger, args)?;
            let parse = |key: &str, default: usize| {
                args.opt(key).map_or(Ok(default), |s| {
                    s.parse::<usize>().map_err(|_| {
                        Failure::usage("Pagination values must be nonnegative integers.")
                    })
                })
            };
            let limit = parse("limit", 20)?;
            let offset = parse("offset", 0)?;
            if !(1..=MAX_PAGE_SIZE).contains(&limit) {
                return Err(Failure::usage(
                    "Artifact page limit must be between 1 and 100.",
                ));
            }
            let selected_revisions = ledger.entries().filter(|r| {
                r.artifact == selected.artifact
                    && (!args.has("revision") || r.revision == selected.revision)
            });
            let total = ledger
                .entries()
                .filter(|r| {
                    r.artifact == selected.artifact
                        && (!args.has("revision") || r.revision == selected.revision)
                })
                .count();
            let entries: Vec<_> = selected_revisions
                .skip(offset)
                .take(limit)
                .map(|r| describe(&ctx.tree, &r))
                .collect();
            let next = offset.saturating_add(entries.len());
            let mut value = json!({"operation":"show","artifact":selected.artifact,"latest_revision":ledger.latest(selected.artifact).map(|revision| revision.revision),"revisions":entries,"bytes_checked":false,"total":total,"next_offset":if next<total {Some(next)} else {None}});
            if selected.is_bundle() {
                value["members_revision"] = json!(selected.revision);
                match bundle_manifest(&ctx.lane_dir, &selected, false) {
                    Ok(manifest) => {
                        let members: Vec<_> =
                            manifest.members.iter().skip(offset).take(limit).collect();
                        let next = offset.saturating_add(members.len());
                        value["members"] = json!(members);
                        value["total_members"] = json!(manifest.members.len());
                        value["members_next_offset"] =
                            json!((next < manifest.members.len()).then_some(next));
                        value["entrypoint"] = json!(manifest.entrypoint);
                        value["requirements"] = json!(manifest.requirements);
                        if let Some(review) = &manifest.media_review {
                            value["media_review"] = json!(review);
                            value["media_inspection"] = json!({"automatic":"structure_and_accessible_text","visual":false,"audio":false,"review_evidence":"caller_attestation","approval":"not_inferred"});
                        }
                        value["manifest_status"] =
                            json!({"status":"readable","members_checked":false});
                    }
                    Err(error) => {
                        value["members"] = json!([]);
                        value["total_members"] = json!(selected.member_count);
                        value["members_next_offset"] = Value::Null;
                        value["manifest_status"] = json!({"status":"unavailable","exit_code":error.code(),"members_checked":false});
                    }
                }
            }
            Ok((value, 0))
        }
        "check" => check(ctx, &ledger, args),
        "restore" => restore(ctx, &ledger, args).map(|value| (value, 0)),
        _ => unreachable!(),
    }
}

fn envelope(result: Result<(Value, i32), Failure>) -> (Value, i32) {
    match result {
        Ok(value) => value,
        Err(error) => {
            let code = error.code();
            let message = match error {
                Failure::Usage(message) | Failure::Model(message) => message,
                Failure::Redaction(_) => "Sensitive or unsupported artifact data was refused; no delivery was recorded.".into(),
                Failure::NoStore => "No vivac tree is available; run vivac init first.".into(),
                _ => "Artifact custody could not complete the operation. Check file access and run artifacts check --store for unrecorded retained bytes.".into(),
            };
            (json!({"error":message,"exit_code":code}), code)
        }
    }
}

pub(crate) fn run_value(cwd: &Path, args: &Args) -> (Value, i32) {
    envelope(execute(cwd, args))
}

pub(crate) fn run_project_value(
    project: &mut crate::project::Project,
    args: &Args,
) -> (Value, i32) {
    let result = (|| {
        let operation = validate(args)?;
        if operation == "preview" {
            return Err(Failure::usage(
                "Artifact preview is interactive; use the CLI or web.",
            ));
        }
        if operation == "capabilities" {
            return Ok((capabilities(), 0));
        }
        if operation == "capture" {
            let ctx = project.current()?;
            if ctx.tree.broken_lines != 0 {
                return Err(Failure::Model(
                    "Artifact custody refuses an unreadable event log.".into(),
                ));
            }
            match prepare_capture(ctx, args)? {
                CapturePlan::AlreadyRecorded(value) => Ok((value, 0)),
                CapturePlan::Prepared(body) => project
                    .write(|ctx| commit_capture(ctx, *body))
                    .map(|value| (value, 0)),
            }
        } else {
            execute_read(project.current()?, args)
        }
    })();
    envelope(result)
}

pub(crate) use files::Download;

pub(crate) struct PreviewRevision {
    pub(crate) reader: Download,
    pub(crate) bundle: Option<bundle::PreviewResources>,
    runtime: Option<runtime::Descriptor>,
    pub(crate) artifact: String,
    pub(crate) revision: String,
    pub(crate) source: String,
    pub(crate) description: Option<String>,
    pub(crate) producer: String,
    pub(crate) digest: String,
}

pub(crate) fn project_preview(
    project: &mut crate::project::Project,
    artifact: &str,
    revision: &str,
) -> Result<PreviewRevision, Failure> {
    let (ctx, selected) = project_revision(project, artifact, revision)?;
    preview_revision(ctx, selected)
}

fn preview_revision(ctx: &Ctx, selected: Revision<'_>) -> Result<PreviewRevision, Failure> {
    if selected.is_bundle() {
        bundle_manifest(&ctx.lane_dir, &selected, false)?;
        let (reader, bundle) = bundle::preview(
            &ctx.lane_dir,
            selected.object,
            selected.digest,
            selected.size,
        )?;
        return Ok(PreviewRevision {
            reader,
            runtime: selected
                .runtime
                .map(runtime::known_descriptor)
                .transpose()?,
            description: selected.description.map(str::to_owned),
            bundle: Some(bundle),
            artifact: selected.artifact.to_owned(),
            revision: selected.revision.to_owned(),
            source: selected
                .entrypoint
                .expect("validated bundle entrypoint")
                .to_owned(),
            producer: selected.producer.to_owned(),
            digest: selected.digest.to_owned(),
        });
    }
    if !selected
        .source
        .rsplit_once('.')
        .is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
        })
    {
        return Err(Failure::usage("Only retained HTML files can be previewed."));
    }
    let reader = files::download(
        &ctx.lane_dir,
        selected.object,
        selected.digest,
        selected.size,
    )?;
    Ok(PreviewRevision {
        reader,
        runtime: None,
        description: selected.description.map(str::to_owned),
        bundle: None,
        artifact: selected.artifact.to_owned(),
        revision: selected.revision.to_owned(),
        source: selected.source.to_owned(),
        producer: selected.producer.to_owned(),
        digest: selected.digest.to_owned(),
    })
}

fn project_revision<'a>(
    project: &'a mut crate::project::Project,
    artifact: &str,
    revision: &str,
) -> Result<(&'a Ctx, Revision<'a>), Failure> {
    if !valid_id(artifact) || !valid_id(revision) {
        return Err(Failure::usage("Invalid artifact or revision identifier."));
    }
    let ctx = project.current()?;
    if ctx.tree.broken_lines != 0 {
        return Err(Failure::Model(
            "Artifact custody refuses an unreadable event log.".into(),
        ));
    }
    let ledger = replay(&ctx.tree)?;
    let selected = ledger
        .get(revision)
        .filter(|selected| selected.artifact == artifact)
        .ok_or_else(|| Failure::usage("This revision does not belong to the selected artifact."))?;
    Ok((ctx, selected))
}

pub(crate) fn project_download(
    project: &mut crate::project::Project,
    artifact: &str,
    revision: &str,
) -> Result<(Download, String), Failure> {
    let (ctx, selected) = project_revision(project, artifact, revision)?;
    if selected.is_bundle() {
        return Err(Failure::usage("Bundle download is not supported. Restore the complete revision with artifacts restore."));
    }
    let filename = selected
        .source
        .rsplit('/')
        .next()
        .filter(|name| {
            !name.is_empty()
                && *name != "."
                && *name != ".."
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{revision}.bin"));
    let reader = files::download(
        &ctx.lane_dir,
        selected.object,
        selected.digest,
        selected.size,
    )?;
    Ok((reader, filename))
}

fn start_preview(cwd: &Path, args: &Args) -> Result<(preview::Preview, Value), Failure> {
    validate(args)?;
    let selected = {
        let ctx = context(cwd, false, false)?;
        let ledger = replay(&ctx.tree)?;
        preview_revision(&ctx, revision(&ledger, args)?)?
    };
    let mut value = json!({
            "operation": "preview",
            "artifact": selected.artifact,
            "revision": selected.revision,
            "source": selected.source,
            "description": selected.description,
            "producer": selected.producer,
            "digest": selected.digest,
            "size": selected.reader.size(),
            "network_isolation": false,
            "approval_implied": false
    });
    let preview = preview::start(selected)?;
    value["url"] = json!(preview.url());
    Ok((preview, value))
}

pub fn run(cwd: &Path, args: &Args) -> Result<i32, Failure> {
    if args.positional(0) == Some("preview") {
        let (preview, value) = match start_preview(cwd, args) {
            Ok(ready) => ready,
            Err(error) => {
                let (value, code) = envelope(Err(error));
                if args.has("json") {
                    crate::render::print_json(value)?;
                } else {
                    print(&value)?;
                }
                return Ok(code);
            }
        };
        if args.has("json") {
            crate::render::print_json(value)?;
        } else {
            crate::output::outln!("{}", preview.url());
        }
        crate::output::flush();
        if !args.has("no-open") {
            crate::web::open_browser(preview.url());
        }
        preview.wait();
        return Ok(0);
    }
    let (value, code) = run_value(cwd, args);
    if args.has("json") {
        crate::render::print_json(value)?;
    } else {
        print(&value)?;
    }
    Ok(code)
}

fn print(value: &Value) -> Result<(), Failure> {
    if let Some(message) = value["error"].as_str() {
        crate::output::outln!("Refused: {message}");
        return Ok(());
    }
    if value["operation"] == "capabilities" {
        let max_mib = MAX_FILE_BYTES / (1024 * 1024);
        if bundle::SUPPORTED {
            crate::output::outln!("Artifacts: UTF-8 files or complete selected HTML/CSS/JS bundles, up to {max_mib} MiB per revision; bundles up to 64 members and 64 KiB manifest; pages up to {MAX_PAGE_SIZE}.");
            crate::output::outln!("Web media: selected static PNG and single-track H.264 MP4 without audio. Prior human review of exact media bytes is required; automatic inspection covers structure and accessible text, not visual content. Unknown structures are refused.");
        } else {
            crate::output::outln!("Artifacts: one UTF-8 text file per revision, up to {max_mib} MiB; pages up to {MAX_PAGE_SIZE}. Complete bundle custody is not supported on this platform.");
        }
        if value["delivery_types"]["sqlite_application"]["supported"] == true {
            crate::output::outln!("SQLite applications: complete HTML/CSS/JS with one explicit database, retained as a standalone logical export. Vivac supplies a fixed JavaScript runtime and retains it with the revision; no backend is provided.");
            crate::output::outln!("SQLite WAL input requires the explicit readonly-wal-v1 source profile and existing database, WAL and SHM inputs. Native coordination locks protect the read; source files are not written or checkpointed.");
        } else {
            crate::output::outln!("SQLite application custody is not supported by this engine.");
        }
        crate::output::outln!("Standalone database snapshots are not supported. Retained HTML preview runs with browser restrictions; complete network isolation is not guaranteed.");
        crate::output::outln!(
            "Every capture requires a single-line description of up to 280 Unicode characters."
        );
        return Ok(());
    }
    crate::output::outln!(
        "Artifacts | {}",
        value["operation"].as_str().unwrap_or("list")
    );
    if let Some(entries) = value["artifacts"]
        .as_array()
        .or_else(|| value["revisions"].as_array())
    {
        for entry in entries {
            crate::output::outln!(
                "  {} | {} | {:?} | retained check: unverified",
                entry["artifact"].as_str().unwrap_or(""),
                entry["revision"].as_str().unwrap_or(""),
                entry["source"].as_str().unwrap_or("")
            );
            crate::output::outln!(
                "    Description: {}",
                entry["description"]
                    .as_str()
                    .unwrap_or("No description recorded.")
            );
        }
        if entries.is_empty() {
            crate::output::outln!("  No artifacts in this scope. Use --all to list the project.");
        }
    } else {
        crate::render::print_json(value.clone())?;
    }
    Ok(())
}

pub(crate) fn request_args(value: &Value) -> Result<Args, Failure> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request<'a> {
        operation: &'a str,
        artifact: Option<&'a str>,
        path: Option<&'a str>,
        description: Option<&'a str>,
        bundle: Option<&'a str>,
        from: Option<&'a str>,
        revision: Option<&'a str>,
        previous: Option<&'a str>,
        operation_id: Option<&'a str>,
        to: Option<&'a str>,
        orphan: Option<&'a str>,
        all: Option<bool>,
        store: Option<bool>,
        limit: Option<u64>,
        offset: Option<u64>,
    }
    let fields = [
        "operation",
        "artifact",
        "path",
        "description",
        "bundle",
        "from",
        "revision",
        "previous",
        "operation_id",
        "to",
        "orphan",
        "all",
        "store",
        "limit",
        "offset",
    ];
    let object = value
        .as_object()
        .ok_or_else(|| Failure::usage("Invalid artifact request arguments."))?;
    if object.get("operation").and_then(Value::as_str) == Some("preview") {
        return Err(Failure::usage(
            "Artifact preview is interactive; use the CLI or web.",
        ));
    }
    if object.get("operation").and_then(Value::as_str) == Some("capabilities") && object.len() != 1
    {
        return Err(Failure::usage(
            "Capabilities does not take additional arguments.",
        ));
    }
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(Failure::usage("Invalid artifact request arguments."));
    }
    if object.contains_key("description")
        && object.get("operation").and_then(Value::as_str) != Some("capture")
    {
        return Err(Failure::usage(
            "Only capture takes an artifact description.",
        ));
    }
    if object.iter().any(|(key, value)| {
        if key == "operation" {
            return !value.is_string();
        }
        if value.is_null() {
            return false;
        }
        match key.as_str() {
            "all" | "store" => !value.is_boolean(),
            "limit" | "offset" => value.as_u64().is_none(),
            _ => !value.is_string(),
        }
    }) {
        return Err(Failure::usage("Invalid artifact request arguments."));
    }
    let request = <Request<'_> as serde::Deserialize>::deserialize(value)
        .map_err(|_| Failure::usage("Invalid artifact request arguments."))?;
    if request.operation.len() > 16
        || request.description.is_some_and(|value| value.len() > 1120)
        || [request.path, request.bundle, request.to, request.from]
            .into_iter()
            .flatten()
            .any(|value| value.len() > 4096)
        || [
            request.artifact,
            request.revision,
            request.previous,
            request.operation_id,
            request.orphan,
        ]
        .into_iter()
        .flatten()
        .any(|value| value.len() > 26)
    {
        return Err(Failure::usage("Invalid artifact request arguments."));
    }
    if (request.all.is_some() && request.operation != "list")
        || (request.store.is_some() && request.operation != "check")
    {
        return Err(Failure::usage(
            "This artifact operation does not take that boolean argument.",
        ));
    }
    let mut flags = Vec::new();
    let operation = request.operation;
    let positional = if operation == "capture" {
        request.path.map(str::to_owned)
    } else {
        request.artifact.map(str::to_owned)
    };
    if operation != "capture"
        && (request.path.is_some() || request.bundle.is_some() || request.description.is_some())
    {
        return Err(Failure::usage(
            "Only capture takes a source path or bundle selector.",
        ));
    }
    if operation == "capture" {
        if request.path.is_some() == request.bundle.is_some() {
            return Err(Failure::usage(
                "Capture requires exactly one path or bundle selector.",
            ));
        }
        if let Some(artifact) = request.artifact {
            flags.push(format!("--artifact={artifact}"));
        }
    }
    for (key, value) in [
        ("bundle", request.bundle),
        ("description", request.description),
        ("from", request.from),
        ("revision", request.revision),
        ("previous", request.previous),
        ("operation", request.operation_id),
        ("to", request.to),
        ("orphan", request.orphan),
    ] {
        if let Some(value) = value {
            flags.push(format!("--{key}={value}"));
        }
    }
    for (key, value) in [("all", request.all), ("store", request.store)] {
        if value == Some(true) {
            flags.push(format!("--{key}"));
        }
    }
    for (key, value) in [("limit", request.limit), ("offset", request.offset)] {
        if let Some(value) = value {
            flags.push(format!("--{key}={value}"));
        }
    }
    let mut args = Args::parse(flags)?;
    args.positionals.push(operation.to_owned());
    args.positionals.extend(positional);
    validate(&args)?;
    Ok(args)
}

/// A typed artifact reference never becomes a filename or an inferred approval.
pub(crate) fn validate_references(tree: &Tree, refs: &[String]) -> Result<(), Failure> {
    if !refs
        .iter()
        .any(|reference| reference.starts_with("artifact:"))
    {
        return Ok(());
    }
    let ledger = replay(tree)?;
    for reference in refs
        .iter()
        .filter(|reference| reference.starts_with("artifact:"))
    {
        let Some((artifact, revision)) = reference
            .strip_prefix("artifact:")
            .and_then(|r| r.split_once('@'))
        else {
            return Err(Failure::usage(
                "Artifact references require artifact:<identity>@<revision>.",
            ));
        };
        if ledger
            .get(revision)
            .is_none_or(|entry| entry.artifact != artifact)
        {
            return Err(Failure::usage(
                "The artifact reference does not name a recorded revision.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn standalone_root(name: &str) -> std::path::PathBuf {
    tests::standalone_root(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn preview_cli_requires_an_exact_revision_and_rejects_unrelated_flags() {
        let artifact = crate::id::ulid();
        let revision = crate::id::ulid();
        let valid = Args::parse(vec![
            "preview".into(),
            artifact.clone(),
            format!("--revision={revision}"),
            "--no-open".into(),
            "--json".into(),
        ])
        .unwrap();
        assert_eq!(validate(&valid).unwrap(), "preview");
        for words in [
            vec!["preview".into(), artifact.clone()],
            vec![
                "preview".into(),
                artifact.clone(),
                "--revision=latest".into(),
            ],
            vec![
                "preview".into(),
                artifact.clone(),
                format!("--revision={revision}"),
                format!("--revision={revision}"),
            ],
            vec![
                "preview".into(),
                artifact.clone(),
                format!("--revision={revision}"),
                "--all".into(),
            ],
            vec![
                "preview".into(),
                artifact.clone(),
                format!("--revision={revision}"),
                "extra".into(),
            ],
            vec!["preview".into(), format!("--revision={revision}")],
        ] {
            assert!(validate(&Args::parse(words).unwrap()).is_err());
        }
        assert!(Args::parse(vec!["--no-open=true".into()]).is_err());
        assert!(Args::parse(vec!["--json=true".into()]).is_err());
    }

    #[test]
    fn preview_is_interactive_and_never_runs_through_value_or_mcp_paths() {
        let mut fixture = PreviewFixture::new("mockup.html");
        let args = Args::parse(vec![
            "preview".into(),
            fixture.delivery["artifact"].as_str().unwrap().into(),
            format!(
                "--revision={}",
                fixture.delivery["revision"].as_str().unwrap()
            ),
        ])
        .unwrap();
        let (value, code) = run_project_value(&mut fixture.project, &args);
        assert_eq!(code, 2);
        assert!(value["error"].as_str().unwrap().contains("interactive"));
        let (value, code) = run_value(&fixture.root, &args);
        assert_eq!(code, 2);
        assert!(value["error"].as_str().unwrap().contains("interactive"));
        let error = request_args(&json!({"operation":"preview","artifact":fixture.delivery["artifact"],"revision":fixture.delivery["revision"]})).unwrap_err();
        assert!(error.message().contains("interactive"));
        let execution = capabilities()["execution"].clone();
        assert_eq!(execution["provided"], true);
        assert_eq!(execution["scope"], "retained_html_preview");
        assert_eq!(execution["network_isolation"], false);
    }

    struct PreviewFixture {
        project: crate::project::Project,
        root: PreviewRoot,
        delivery: Value,
    }

    struct PreviewRoot(std::path::PathBuf);

    impl std::ops::Deref for PreviewRoot {
        type Target = std::path::PathBuf;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for PreviewRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl PreviewFixture {
        fn new(source: &str) -> Self {
            let (root, mut ctx, producer) = capture_commit_fixture();
            if source != "mockup.html" {
                fs::rename(root.join("mockup.html"), root.join(source)).unwrap();
            }
            let args = Args::parse(vec![
                "capture".into(),
                "--description=Selected fixture layout".into(),
                source.into(),
                format!("--from={producer}"),
            ])
            .unwrap();
            let delivery = capture(&mut ctx, &args).unwrap()["delivery"].clone();
            ctx.unlock();
            drop(ctx);
            let project = crate::project::Project::open(
                root.clone(),
                "Preview fixture".into(),
                "preview-fixture".into(),
                Whose::Founding,
            )
            .unwrap();
            Self {
                project,
                root: PreviewRoot(root),
                delivery,
            }
        }

        fn preview(&mut self) -> Result<PreviewRevision, Failure> {
            project_preview(
                &mut self.project,
                self.delivery["artifact"].as_str().unwrap(),
                self.delivery["revision"].as_str().unwrap(),
            )
        }
    }

    #[test]
    fn preview_selects_the_exact_old_revision_after_new_capture_and_source_deletion() {
        use std::io::Read;
        let mut fixture = PreviewFixture::new("mockup.html");
        fs::write(
            fixture.root.join("mockup.html"),
            b"<html>Later delivery</html>",
        )
        .unwrap();
        let args = Args::parse(vec![
            "capture".into(),
            "--description=Selected fixture layout".into(),
            "mockup.html".into(),
            format!("--from={}", fixture.delivery["producer"].as_str().unwrap()),
            format!(
                "--artifact={}",
                fixture.delivery["artifact"].as_str().unwrap()
            ),
            format!(
                "--previous={}",
                fixture.delivery["revision"].as_str().unwrap()
            ),
        ])
        .unwrap();
        let later = fixture.project.write(|ctx| capture(ctx, &args)).unwrap();
        assert_ne!(later["delivery"]["revision"], fixture.delivery["revision"]);
        fs::remove_file(fixture.root.join("mockup.html")).unwrap();
        let mut selected = fixture.preview().unwrap();
        assert_eq!(
            selected.artifact,
            fixture.delivery["artifact"].as_str().unwrap()
        );
        assert_eq!(
            selected.revision,
            fixture.delivery["revision"].as_str().unwrap()
        );
        assert_eq!(selected.source, "mockup.html");
        assert_eq!(
            selected.producer,
            fixture.delivery["producer"].as_str().unwrap()
        );
        assert_eq!(
            selected.digest,
            fixture.delivery["digest"].as_str().unwrap()
        );
        assert_eq!(
            selected.reader.size(),
            fixture.delivery["size"].as_u64().unwrap()
        );
        let mut bytes = Vec::new();
        selected.reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"<html>Selected delivery</html>");
    }

    #[test]
    fn preview_refuses_a_revision_from_another_artifact() {
        let mut fixture = PreviewFixture::new("mockup.html");
        let error = project_preview(
            &mut fixture.project,
            &crate::id::ulid(),
            fixture.delivery["revision"].as_str().unwrap(),
        )
        .err()
        .expect("the wrong artifact must be refused");
        assert_eq!(
            error.message(),
            "This revision does not belong to the selected artifact."
        );
    }

    #[test]
    fn preview_refuses_missing_or_corrupted_retained_bytes() {
        for missing in [false, true] {
            let mut fixture = PreviewFixture::new("mockup.html");
            let object = fixture
                .root
                .join(".vivac-artifacts")
                .join(fixture.delivery["object"].as_str().unwrap());
            if missing {
                fs::remove_file(object).unwrap();
            } else {
                fs::write(object, b"<html>Changed delivery!</html>").unwrap();
            }
            assert!(fixture.preview().is_err());
            assert!(!fs::read_dir(fixture.root.join(".vivac-artifacts/objects"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tmp-")));
        }
    }

    #[test]
    fn preview_checks_the_html_suffix_before_attempting_a_retained_copy() {
        let mut fixture = PreviewFixture::new("mockup.txt");
        fs::remove_file(
            fixture
                .root
                .join(".vivac-artifacts")
                .join(fixture.delivery["object"].as_str().unwrap()),
        )
        .unwrap();
        let error = fixture
            .preview()
            .err()
            .expect("text delivery must be refused");
        assert_eq!(
            error.message(),
            "Only retained HTML files can be previewed."
        );
    }

    #[test]
    fn preview_accepts_html_suffixes_without_case_sensitivity() {
        for source in ["mockup.HTML", "mockup.hTm"] {
            let mut fixture = PreviewFixture::new(source);
            assert_eq!(fixture.preview().unwrap().source, source);
        }
    }

    #[test]
    fn an_incomplete_log_checks_surviving_receipts_without_claiming_orphans() {
        let root =
            std::env::temp_dir().join(format!("vivac-artifact-incomplete-{}", crate::id::ulid()));
        let mut store = Store::create(&root).unwrap();
        fs::write(
            root.join("mockup.html"),
            "<!doctype html><title>Preview</title>",
        )
        .unwrap();
        let revision = crate::id::ulid();
        let retained = files::capture(&root, "mockup.html", &revision).unwrap();
        let unrecorded = crate::id::ulid();
        let extra = files::capture(&root, "mockup.html", &unrecorded).unwrap();
        let lock = store.lock_for_write().unwrap();
        let producer = crate::id::ulid();
        store
            .append(
                &lock,
                crate::lane::MAIN,
                vec![Body::NodeCreated {
                    node: producer.clone(),
                    num: 1,
                    kind: crate::event::Kind::Task,
                    title: "Produce preview".into(),
                    why: String::new(),
                    parent: None,
                    blocks: false,
                    refs: vec![],
                    governs: vec![],
                    arms: vec![],
                    against: None,
                }],
                0,
                false,
            )
            .unwrap();
        store
            .append(
                &lock,
                crate::lane::MAIN,
                vec![Body::ArtifactCaptured {
                    artifact: crate::id::ulid(),
                    revision,
                    operation: crate::id::ulid(),
                    producer,
                    previous: None,
                    source: "mockup.html".into(),
                    site: retained.site,
                    size: retained.size,
                    digest: retained.digest,
                    object: retained.object,
                }],
                1,
                false,
            )
            .unwrap();
        drop(lock);
        fs::OpenOptions::new()
            .append(true)
            .open(store.log())
            .unwrap()
            .write_all(b"{\"payload\":")
            .unwrap();
        let log_before = fs::read(store.log()).unwrap();
        let config_before = fs::read(root.join(store::DIR).join(store::CONFIG)).unwrap();
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let ctx = Ctx::load_for_write(store, Whose::Founding).unwrap();
        let ledger = diagnostic_ledger(&ctx.tree);
        let (result, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(result["log_integrity"], "incomplete");
        assert_eq!(result["broken_lines"], 1);
        assert_eq!(result["provenance_complete"], false);
        assert_eq!(result["recorded"][0]["retained"]["status"], "matches");
        assert_eq!(result["orphans"], json!([]));
        assert_eq!(result["unattributed"], json!([extra.object]));
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log_before);
        assert_eq!(
            fs::read(root.join(store::DIR).join(store::CONFIG)).unwrap(),
            config_before
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decision_references_report_the_total_and_bound_the_payload() {
        let mut tree = Tree::default();
        let artifact = crate::id::ulid();
        let revision = crate::id::ulid();
        let producer = crate::id::ulid();
        let reference = format!("artifact:{artifact}@{revision}");
        for num in 1..=125 {
            tree.apply(
                num,
                "2026-10-11T00:00:00Z",
                crate::lane::MAIN,
                &Body::NodeCreated {
                    node: if num == 1 {
                        producer.clone()
                    } else {
                        crate::id::ulid()
                    },
                    num,
                    kind: crate::event::Kind::Decision,
                    title: "Review reference".into(),
                    why: String::new(),
                    parent: None,
                    blocks: false,
                    refs: vec![reference.clone()],
                    governs: vec![],
                    arms: vec![],
                    against: None,
                },
            );
        }
        tree.apply(
            126,
            "2026-10-11T00:00:00Z",
            crate::lane::MAIN,
            &Body::ArtifactCaptured {
                artifact,
                revision: revision.clone(),
                operation: crate::id::ulid(),
                producer,
                previous: None,
                source: "mockup.html".into(),
                site: crate::id::ulid(),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.blob"),
            },
        );
        let ledger = replay(&tree).unwrap();
        let described = describe(&tree, &ledger.get(&revision).unwrap());
        assert_eq!(
            described["decision_references"].as_array().unwrap().len(),
            100
        );
        assert_eq!(described["decision_references_total"], 125);
        assert_eq!(described["decision_references_truncated"], true);
        assert!(described.get("approval").is_none());
        let listed = describe_list(&tree, &ledger, &ledger.get(&revision).unwrap());
        assert_eq!(
            listed["artifact_decision_references"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
        assert_eq!(listed["artifact_decision_references_total"], 125);
        assert_eq!(listed["artifact_decision_references_truncated"], true);
        assert_eq!(
            listed["artifact_decision_references"][0]["revision"],
            revision
        );
    }

    #[test]
    fn artifact_decisions_credit_only_existing_revisions_of_the_same_delivery() {
        let mut tree = Tree::default();
        let artifact = crate::id::ulid();
        let foreign = crate::id::ulid();
        let first = crate::id::ulid();
        let second = crate::id::ulid();
        let other = crate::id::ulid();
        let producer = crate::id::ulid();
        let reference = format!("artifact:{artifact}@{first}");
        for (num, kind, refs) in [
            (1, crate::event::Kind::Task, vec![reference.clone()]),
            (
                2,
                crate::event::Kind::Decision,
                vec![
                    reference.clone(),
                    reference,
                    format!("artifact:{artifact}@{other}"),
                    format!("artifact:{foreign}@{first}"),
                    format!("artifact:{artifact}@{}", crate::id::ulid()),
                ],
            ),
            (
                3,
                crate::event::Kind::Decision,
                vec![format!("artifact:{artifact}@{second}")],
            ),
        ] {
            tree.apply(
                num,
                "2026-10-11T00:00:00Z",
                crate::lane::MAIN,
                &Body::NodeCreated {
                    node: if num == 1 {
                        producer.clone()
                    } else {
                        crate::id::ulid()
                    },
                    num,
                    kind,
                    title: "Review selected delivery".into(),
                    why: String::new(),
                    parent: None,
                    blocks: false,
                    refs,
                    governs: vec![],
                    arms: vec![],
                    against: None,
                },
            );
        }
        for (seq, identity, revision, previous) in [
            (4, artifact.clone(), first.clone(), None),
            (5, artifact.clone(), second.clone(), Some(first.clone())),
            (6, foreign, other, None),
        ] {
            tree.apply(
                seq,
                "2026-10-11T00:00:00Z",
                crate::lane::MAIN,
                &Body::ArtifactCaptured {
                    artifact: identity,
                    revision: revision.clone(),
                    operation: crate::id::ulid(),
                    producer: producer.clone(),
                    previous,
                    source: "delivery.txt".into(),
                    site: crate::id::ulid(),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{revision}.blob"),
                },
            );
        }
        let ledger = replay(&tree).unwrap();
        let listed = describe_list(&tree, &ledger, &ledger.latest(&artifact).unwrap());
        assert_eq!(listed["revision"], second);
        assert_eq!(listed["artifact_decision_references_total"], 2);
        assert_eq!(listed["artifact_decision_references_truncated"], false);
        let refs = listed["artifact_decision_references"].as_array().unwrap();
        assert!(refs
            .iter()
            .any(|entry| entry["revision"] == first && entry["alias"] == "d2"));
        assert!(refs
            .iter()
            .any(|entry| entry["revision"] == second && entry["alias"] == "d3"));
        assert!(refs
            .iter()
            .all(|entry| entry["kind"] == "decision" && entry.get("title").is_none()));
    }

    #[test]
    fn diagnostic_receipts_exclude_invalid_and_ambiguous_metadata_without_inventing_provenance() {
        let mut tree = Tree::default();
        let revision = crate::id::ulid();
        let receipt = Body::ArtifactCaptured {
            artifact: crate::id::ulid(),
            revision: revision.clone(),
            operation: crate::id::ulid(),
            producer: crate::id::ulid(),
            previous: Some(crate::id::ulid()),
            source: "mockup.html".into(),
            site: crate::id::ulid(),
            size: 17,
            digest: "a".repeat(64),
            object: format!("objects/{revision}.blob"),
        };
        tree.artifact_history.push(("main".into(), receipt.clone()));
        let ledger = diagnostic_ledger(&tree);
        let borrowed = ledger.get(&revision).unwrap();
        let Body::ArtifactCaptured { source, .. } = &tree.artifact_history[0].1 else {
            unreachable!()
        };
        assert_eq!(borrowed.source.as_ptr(), source.as_ptr());
        assert!(ledger.latest.is_empty());
        assert_eq!(ledger.unresolved_receipts, 1);
        assert!(replay(&tree).is_err());
        drop(ledger);
        tree.artifact_history.push(("main".into(), receipt.clone()));
        let mut invalid = receipt;
        let Body::ArtifactCaptured {
            digest,
            revision,
            operation,
            ..
        } = &mut invalid
        else {
            unreachable!()
        };
        *digest = "invalid".into();
        *revision = crate::id::ulid();
        *operation = crate::id::ulid();
        tree.artifact_history.push(("main".into(), invalid));
        let ledger = diagnostic_ledger(&tree);
        assert_eq!(ledger.invalid_receipts, 1);
        assert_eq!(ledger.ambiguous_receipts, 2);
        assert!(ledger.revisions.is_empty());
    }

    #[test]
    fn ten_thousand_artifact_projection_entries_fit_the_auxiliary_memory_budget() {
        let mut tree = Tree::default();
        let producer = format!("{:026x}", 1);
        tree.apply(
            1,
            "2026-10-11T00:00:00Z",
            crate::lane::MAIN,
            &Body::NodeCreated {
                node: producer.clone(),
                num: 1,
                kind: crate::event::Kind::Task,
                title: "Produce selected deliveries".into(),
                why: String::new(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        );
        for index in 0..10_000u64 {
            let revision = format!("{:026x}", 20_000 + index);
            tree.artifact_history.push((
                "main".into(),
                Body::ArtifactCaptured {
                    artifact: format!("{:026x}", 10_000 + index),
                    revision: revision.clone(),
                    operation: format!("{:026x}", 30_000 + index),
                    producer: producer.clone(),
                    previous: None,
                    source: "mockup.html".into(),
                    site: format!("{:026x}", 2),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{revision}.blob"),
                },
            ));
        }
        tree.rebuild_artifact_projection();
        let (ledger, peak) = crate::test_alloc::measure(|| replay(&tree).unwrap());
        eprintln!("Artifact projection peak auxiliary allocation: {peak} bytes for 10000 distinct artifacts.");
        assert!(
            peak <= 2 * 1024 * 1024,
            "artifact projection allocated {peak} auxiliary bytes"
        );
        assert_eq!(ledger.count(), 10_000);
        assert_eq!(ledger.projection.unwrap().latest_count(), 10_000);
        assert_eq!(ledger.projection.unwrap().operations_count(), 10_000);
        let last = format!("{:026x}", 29_999);
        assert_eq!(ledger.get(&last).unwrap().source, "mockup.html");
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure resident metadata reads."]
    fn measure_resident_inventory_with_ten_thousand_artifacts_and_decisions() {
        let root = standalone_root("artifact-inventory-benchmark");
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_for_write(store, Whose::Founding).unwrap();
        let producer = format!("{:026x}", 1);
        ctx.tree.apply(
            1,
            "2026-10-11T00:00:00Z",
            crate::lane::MAIN,
            &Body::NodeCreated {
                node: producer.clone(),
                num: 1,
                kind: crate::event::Kind::Task,
                title: "Produce selected deliveries".into(),
                why: String::new(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        );
        for index in 0..10_000u64 {
            let artifact = format!("{:026x}", 10_000 + index);
            let revision = format!("{:026x}", 20_000 + index);
            ctx.tree.apply(
                index + 2,
                "2026-10-11T00:00:00Z",
                crate::lane::MAIN,
                &Body::NodeCreated {
                    node: format!("{:026x}", 100_000 + index),
                    num: index + 2,
                    kind: crate::event::Kind::Decision,
                    title: "Reference selected delivery".into(),
                    why: String::new(),
                    parent: None,
                    blocks: false,
                    refs: vec![format!("artifact:{artifact}@{revision}")],
                    governs: vec![],
                    arms: vec![],
                    against: None,
                },
            );
            ctx.tree.artifact_history.push((
                "main".into(),
                Body::ArtifactCaptured {
                    artifact,
                    revision: revision.clone(),
                    operation: format!("{:026x}", 30_000 + index),
                    producer: producer.clone(),
                    previous: None,
                    source: "delivery.txt".into(),
                    site: format!("{:026x}", 2),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{revision}.blob"),
                },
            ));
        }
        ctx.tree.rebuild_artifact_projection();
        let args = Args::parse(vec!["list".into(), "--all".into()]).unwrap();
        let mut durations = Vec::with_capacity(100);
        let mut peak = 0;
        for _ in 0..100 {
            let ((value, duration), allocated) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let ledger = replay(&ctx.tree).unwrap();
                let value = list(&ctx, &ledger, &args).unwrap();
                (value, started.elapsed())
            });
            assert_eq!(value["total"], 10_000);
            assert_eq!(value["artifacts"].as_array().unwrap().len(), 20);
            assert!(value["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["artifact_decision_references_total"] == 1));
            durations.push(duration);
            peak = peak.max(allocated);
        }
        durations.sort();
        eprintln!("Resident artifact inventory metadata: 100 release samples, 10000 artifacts and 10000 decisions, page of 20; p50={:?}, p99={:?}, peak auxiliary={peak} bytes. Includes replay validation and LIST metadata construction. Context loading, socket handling, serialization and byte inspection are excluded.", durations[49], durations[98]);
        assert!(
            peak <= 2 * 1024 * 1024,
            "inventory metadata allocated {peak} auxiliary bytes"
        );
        assert!(
            durations[98] < std::time::Duration::from_millis(50),
            "resident inventory metadata exceeded its fifty millisecond p99 budget"
        );
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
    }

    fn store_check_fixture(count: u64) -> (std::path::PathBuf, Ctx) {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("vivac-store-page-{}", crate::id::ulid()));
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_for_write(store, Whose::Founding).unwrap();
        ctx.tree.apply(
            1,
            "2026-10-11T00:00:00Z",
            crate::lane::MAIN,
            &Body::NodeCreated {
                node: format!("{:026x}", 1),
                num: 1,
                kind: crate::event::Kind::Task,
                title: "Produce selected deliveries".into(),
                why: String::new(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        );
        let objects = root.join(".vivac-artifacts").join("objects");
        fs::create_dir_all(&objects).unwrap();
        let bytes = b"<html>Preview</html>";
        let digest = format!("{:x}", Sha256::digest(bytes));
        for index in 0..count {
            let revision = format!("{:026x}", 20_000 + index);
            fs::write(objects.join(format!("{revision}.blob")), bytes).unwrap();
            ctx.tree.artifact_history.push((
                "main".into(),
                Body::ArtifactCaptured {
                    artifact: format!("{:026x}", 10_000 + index),
                    revision: revision.clone(),
                    operation: format!("{:026x}", 30_000 + index),
                    producer: format!("{:026x}", 1),
                    previous: None,
                    source: "mockup.html".into(),
                    site: format!("{:026x}", 2),
                    size: bytes.len() as u64,
                    digest: digest.clone(),
                    object: format!("objects/{revision}.blob"),
                },
            ));
        }
        ctx.tree.rebuild_artifact_projection();
        (root, ctx)
    }

    #[test]
    fn store_check_pages_output_but_checks_damage_beyond_the_page() {
        let (root, ctx) = store_check_fixture(30);
        let objects = root.join(".vivac-artifacts").join("objects");
        fs::write(objects.join(format!("{:026x}.blob", 20_029)), b"changed").unwrap();
        let first_page =
            Args::parse(vec!["check".into(), "--store".into(), "--limit=2".into()]).unwrap();
        validate(&first_page).unwrap();
        let ledger = diagnostic_ledger(&ctx.tree);
        let (damaged, code) = check(&ctx, &ledger, &first_page).unwrap();
        assert_eq!(
            code, 1,
            "damage beyond the displayed page must fail the whole check"
        );
        assert!(damaged["recorded"]
            .as_array()
            .unwrap()
            .iter()
            .all(|receipt| receipt["retained"]["status"] == "matches"));
        assert_eq!(damaged["orphans_total"], 0);
        assert_eq!(damaged["temporaries_total"], 0);
        for index in 0..30 {
            fs::write(
                objects.join(format!("{:026x}.blob", 50_000 + index)),
                b"unrecorded",
            )
            .unwrap();
            fs::write(
                objects.join(format!(".tmp-{:026x}", 60_000 + index)),
                b"staged",
            )
            .unwrap();
        }
        let args = Args::parse(vec!["check".into(), "--store".into(), "--limit=2".into()]).unwrap();
        validate(&args).unwrap();
        let ledger = diagnostic_ledger(&ctx.tree);
        let (result, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(result["recorded"].as_array().unwrap().len(), 2);
        assert_eq!(result["recorded"][0]["retained"]["status"], "matches");
        assert_eq!(result["recorded_total"], 30);
        assert_eq!(result["orphans"].as_array().unwrap().len(), 2);
        assert_eq!(result["orphans_total"], 30);
        assert_eq!(result["temporaries"].as_array().unwrap().len(), 2);
        assert_eq!(result["temporaries_total"], 30);
        assert_eq!(result["next_offset"], 2);
        assert_eq!(result["unattributed_total"], 0);
        let single = Args::parse(vec![
            "check".into(),
            format!("{:026x}", 10_000),
            "--limit=2".into(),
        ])
        .unwrap();
        assert!(validate(&single).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ten_thousand_store_checks_fit_the_auxiliary_memory_budget_with_paged_output() {
        let (root, ctx) = store_check_fixture(10_000);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let ((result, code), peak) = crate::test_alloc::measure(|| {
            let ledger = diagnostic_ledger(&ctx.tree);
            check(&ctx, &ledger, &args).unwrap()
        });
        eprintln!("Artifact store check peak auxiliary allocation: {peak} bytes for 10000 receipts and objects.");
        assert!(
            peak <= 2 * 1024 * 1024,
            "artifact store check allocated {peak} auxiliary bytes"
        );
        assert_eq!(code, 0);
        assert_eq!(result["recorded_total"], 10_000);
        assert_eq!(result["recorded"].as_array().unwrap().len(), 20);
        assert_eq!(result["next_offset"], 20);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_check_pagination_is_available_through_the_structured_request_only_for_store_checks() {
        let args = request_args(&json!({"operation":"check", "store":true, "limit":2, "offset":4}))
            .unwrap();
        assert_eq!(args.opt("limit"), Some("2"));
        assert_eq!(args.opt("offset"), Some("4"));
        assert!(request_args(
            &json!({"operation":"check", "artifact":format!("{:026x}", 1), "limit":2})
        )
        .is_err());
        assert!(request_args(&json!({"operation":"list", "store":true})).is_err());
    }

    #[test]
    fn diagnostic_receipts_report_broken_chains_without_discarding_valid_hash_metadata() {
        let (root, mut ctx) = store_check_fixture(3);
        let artifact = match &ctx.tree.artifact_history[0].1 {
            Body::ArtifactCaptured { artifact, .. } => artifact.clone(),
            _ => unreachable!(),
        };
        for (_, body) in &mut ctx.tree.artifact_history[1..] {
            let Body::ArtifactCaptured {
                artifact: current,
                previous,
                ..
            } = body
            else {
                unreachable!()
            };
            *current = artifact.clone();
            *previous = Some(format!("{:026x}", 20_000));
        }
        let ledger = diagnostic_ledger(&ctx.tree);
        assert_eq!(ledger.revisions.len(), 3);
        assert_eq!(ledger.unresolved_receipts, 1);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (result, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(result["log_integrity"], "complete");
        assert_eq!(result["provenance_complete"], false);
        assert_eq!(result["unresolved_receipts"], 1);
        assert!(result["recorded"]
            .as_array()
            .unwrap()
            .iter()
            .all(|receipt| receipt["retained"]["status"] == "matches"));
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture_boundary(crate_root: &Path) -> &Path {
        crate_root
            .parent()
            .filter(|parent| parent.parent().is_some())
            .unwrap_or(crate_root)
    }

    #[test]
    fn a_crate_below_the_filesystem_root_allows_external_temporary_fixtures() {
        let temporary = std::env::temp_dir();
        let volume = temporary.ancestors().last().unwrap();
        let crate_root = volume.join("app");
        let boundary = fixture_boundary(&crate_root);
        assert_eq!(boundary, crate_root);
        assert!(!volume.join("tmp").join("fixture").starts_with(boundary));
        assert!(crate_root.join("fixture").starts_with(boundary));
    }

    #[test]
    fn a_crate_below_a_workspace_rejects_every_fixture_inside_that_workspace() {
        let temporary = std::env::temp_dir();
        let volume = temporary.ancestors().last().unwrap();
        let workspace = volume.join("workspace");
        let crate_root = workspace.join("vivac");
        let boundary = fixture_boundary(&crate_root);
        assert_eq!(boundary, workspace);
        assert!(workspace.join("fixture").starts_with(boundary));
        assert!(crate_root.join("fixture").starts_with(boundary));
        assert!(!volume.join("tmp").join("fixture").starts_with(boundary));
    }

    fn guard_fixture_root(root: &Path, crate_root: &Path) {
        let workspace = fixture_boundary(crate_root);
        assert!(
            !root.starts_with(workspace),
            "artifact fixtures must be outside the workspace"
        );
        assert!(
            !root
                .ancestors()
                .skip(1)
                .any(|parent| ["config", "events", "lane"]
                    .into_iter()
                    .any(|name| parent.join(".vivac").join(name).exists())),
            "artifact fixtures must not inherit a tree"
        );
    }

    pub(super) fn standalone_root(name: &str) -> std::path::PathBuf {
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let root = temporary.join(format!("vivac-{name}-{}", crate::id::ulid()));
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        guard_fixture_root(&root, &crate_root);
        root
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_alias_is_rejected_for_an_ancestral_tree() {
        let base = standalone_root("artifact-alias");
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join(".vivac")).unwrap();
        fs::write(
            base.join(".vivac").join("config"),
            b"Synthetic ancestry marker",
        )
        .unwrap();
        let alias = base.join("alias");
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        let temporary = alias.canonicalize().unwrap();
        let root = temporary.join("uninitialized-child");
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        assert!(std::panic::catch_unwind(|| guard_fixture_root(&root, &crate_root)).is_err());
        assert!(!root.exists());
        fs::remove_file(alias).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    fn capture_commit_fixture() -> (std::path::PathBuf, Ctx, String) {
        let root = standalone_root("artifact-commit");
        let mut store = Store::create(&root).unwrap();
        let producer = crate::id::ulid();
        let lock = store.lock_for_write().unwrap();
        store
            .append(
                &lock,
                crate::lane::MAIN,
                vec![Body::NodeCreated {
                    node: producer.clone(),
                    num: 1,
                    kind: crate::event::Kind::Task,
                    title: "Produce selected delivery".into(),
                    why: String::new(),
                    parent: None,
                    blocks: false,
                    refs: vec![],
                    governs: vec![],
                    arms: vec![],
                    against: None,
                }],
                0,
                false,
            )
            .unwrap();
        drop(lock);
        fs::write(root.join("mockup.html"), b"<html>Selected delivery</html>").unwrap();
        let ctx = Ctx::load_for_write(store, Whose::Founding).unwrap();
        (root, ctx, producer)
    }

    fn staged_delivery(
        root: &Path,
        artifact: &str,
        producer: &str,
        previous: Option<&str>,
        operation: &str,
    ) -> Body {
        let revision = crate::id::ulid();
        let retained = files::capture(root, "mockup.html", &revision).unwrap();
        Body::ArtifactCaptured {
            artifact: artifact.into(),
            revision,
            operation: operation.into(),
            producer: producer.into(),
            previous: previous.map(str::to_owned),
            source: "mockup.html".into(),
            site: retained.site,
            size: retained.size,
            digest: retained.digest,
            object: retained.object,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    const BUNDLE_HTML: &[u8] = b"<html><link rel=stylesheet href=assets/style.css><script src=assets/main.js></script></html>";
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    const BUNDLE_CSS: &[u8] = b"body { color: navy; }";
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    const BUNDLE_JS: &[u8] = b"document.title = 'Selected delivery';";

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn bundle_commit_fixture() -> (std::path::PathBuf, Ctx, String) {
        let (root, ctx, producer) = capture_commit_fixture();
        fs::create_dir_all(root.join("app/assets")).unwrap();
        for (path, bytes) in [
            ("index.html", BUNDLE_HTML),
            ("assets/style.css", BUNDLE_CSS),
            ("assets/main.js", BUNDLE_JS),
        ] {
            fs::write(root.join("app").join(path), bytes).unwrap();
        }
        fs::write(root.join("selection.json"), serde_json::to_vec(&json!({
            "root":"app", "entrypoint":"index.html", "members":["index.html","assets/style.css","assets/main.js"], "requirements":[]
        })).unwrap()).unwrap();
        (root, ctx, producer)
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn staged_bundle_delivery(
        root: &Path,
        artifact: &str,
        producer: &str,
        previous: Option<&str>,
        operation: &str,
    ) -> Body {
        let revision = crate::id::ulid();
        let selection = bundle::BundleSelection {
            root: "app".into(),
            entrypoint: "index.html".into(),
            members: vec![
                "index.html".into(),
                "assets/style.css".into(),
                "assets/main.js".into(),
            ],
            requirements: vec![],
            database: None,
            runtime: None,
            media_review: None,
            database_source: None,
        };
        let retained = bundle::capture(root, &selection, &revision).unwrap();
        Body::ArtifactBundleCaptured {
            artifact: artifact.into(),
            revision,
            operation: operation.into(),
            producer: producer.into(),
            previous: previous.map(str::to_owned),
            source: selection.root,
            selection: "selection.json".into(),
            site: retained.retained.site,
            size: retained.retained.size,
            digest: retained.retained.digest,
            object: retained.retained.object,
            entrypoint: retained.entrypoint,
            member_count: retained.members,
            manifest_bytes: retained.manifest_bytes,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn assert_rescued_bundle(root: &Path, destination: &str, object: &str) {
        let destination = root.join(destination);
        for (path, bytes) in [
            ("index.html", BUNDLE_HTML),
            ("assets/style.css", BUNDLE_CSS),
            ("assets/main.js", BUNDLE_JS),
        ] {
            assert_eq!(fs::read(destination.join(path)).unwrap(), bytes);
        }
        let retained_manifest = fs::read(
            root.join(".vivac-artifacts")
                .join(object)
                .join(".vivac-delivery.json"),
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join(".vivac-delivery.json")).unwrap(),
            retained_manifest
        );
        let manifest: Value = serde_json::from_slice(&retained_manifest).unwrap();
        assert_eq!(manifest["members"].as_array().unwrap().len(), 3);
        assert_eq!(manifest["entrypoint"], "index.html");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn sqlite_commit_fixture() -> (std::path::PathBuf, Ctx, String) {
        use libsqlite3_sys as ffi;
        let fixture = bundle_commit_fixture();
        let path =
            std::ffi::CString::new(fixture.0.join("app/data.sqlite").to_str().unwrap()).unwrap();
        let _engine = sqlite::test_engine_guard().unwrap();
        unsafe {
            let mut database = std::ptr::null_mut();
            assert_eq!(
                ffi::sqlite3_open(path.as_ptr(), &mut database),
                ffi::SQLITE_OK
            );
            assert_eq!(ffi::sqlite3_exec(database, c"CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items VALUES(1,'Synthetic fixture data')".as_ptr(), None, std::ptr::null_mut(), std::ptr::null_mut()), ffi::SQLITE_OK);
            assert_eq!(ffi::sqlite3_close(database), ffi::SQLITE_OK);
        }
        fs::write(fixture.0.join("selection.json"), serde_json::to_vec(&json!({"root":"app", "entrypoint":"index.html", "members":["index.html","assets/style.css","assets/main.js","data.sqlite"], "database":"data.sqlite", "runtime":runtime::ID, "requirements":[]})).unwrap()).unwrap();
        fixture
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn staged_sqlite_delivery(
        root: &Path,
        artifact: &str,
        producer: &str,
        previous: Option<&str>,
        operation: &str,
    ) -> Body {
        let revision = crate::id::ulid();
        let selection = bundle::load_selection(root, "selection.json").unwrap();
        let retained = bundle::capture(root, &selection, &revision).unwrap();
        Body::ArtifactSqliteApplicationCaptured {
            artifact: artifact.into(),
            revision,
            operation: operation.into(),
            producer: producer.into(),
            previous: previous.map(str::to_owned),
            source: selection.root,
            description: "Selected SQLite application".into(),
            selection: "selection.json".into(),
            site: retained.retained.site,
            size: retained.retained.size,
            digest: retained.retained.digest,
            object: retained.retained.object,
            entrypoint: retained.entrypoint,
            member_count: retained.members,
            manifest_bytes: retained.manifest_bytes,
            database: "data.sqlite".into(),
            profile: bundle::SQLITE_PROFILE.into(),
            runtime: runtime::ID.into(),
            runtime_sha256: runtime::SHA256.into(),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn assert_rescued_sqlite(root: &Path, destination: &str, object: &str) {
        let retained = root.join(".vivac-artifacts").join(object);
        for path in [
            "index.html",
            "assets/style.css",
            "assets/main.js",
            "data.sqlite",
            runtime::PATH,
            ".vivac-delivery.json",
        ] {
            assert_eq!(
                fs::read(root.join(destination).join(path)).unwrap(),
                fs::read(retained.join(path)).unwrap()
            );
        }
        let manifest: Value =
            serde_json::from_slice(&fs::read(retained.join(".vivac-delivery.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["members"].as_array().unwrap().len(), 5);
        assert_eq!(manifest["delivery_type"], "sqlite_application");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn competing_published_bundles_leave_the_rejected_complete_object_rescuable() {
        for shared_operation in [false, true] {
            let (root, mut ctx, producer) = bundle_commit_fixture();
            let artifact = crate::id::ulid();
            let first =
                staged_bundle_delivery(&root, &artifact, &producer, None, &crate::id::ulid());
            let saved = commit_capture(&mut ctx, first).unwrap();
            ctx.unlock();
            let previous = saved["delivery"]["revision"].as_str().unwrap();
            let mut winner =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let mut loser =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let operation = crate::id::ulid();
            let winning_body =
                staged_bundle_delivery(&root, &artifact, &producer, Some(previous), &operation);
            let winning_revision = Revision::from_body("main", &winning_body)
                .unwrap()
                .revision
                .to_owned();
            let losing_operation = if shared_operation {
                operation.clone()
            } else {
                crate::id::ulid()
            };
            let losing_body = staged_bundle_delivery(
                &root,
                &artifact,
                &producer,
                Some(previous),
                &losing_operation,
            );
            let losing_revision = Revision::from_body("main", &losing_body)
                .unwrap()
                .revision
                .to_owned();
            let losing_object = format!("objects/{losing_revision}.bundle");
            commit_capture(&mut winner, winning_body).unwrap();
            winner.unlock();
            let log = fs::read(ctx.store.log()).unwrap();
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            assert!(commit_capture(&mut loser, losing_body).is_err());
            loser.unlock();
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            assert_eq!(
                replay(&loser.tree)
                    .unwrap()
                    .latest(&artifact)
                    .unwrap()
                    .revision,
                winning_revision
            );
            let ledger = diagnostic_ledger(&loser.tree);
            let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
            let (inventory, code) = check(&loser, &ledger, &args).unwrap();
            assert_eq!(code, 1);
            assert_eq!(inventory["recorded_total"], 2);
            assert_eq!(inventory["orphans"], json!([losing_object]));
            assert_eq!(
                inventory["bundle_objects"][0]["retained"]["status"],
                "matches"
            );
            let rescue = Args::parse(vec![
                "restore".into(),
                format!("--orphan={losing_revision}"),
                "--to=rescue".into(),
            ])
            .unwrap();
            let restored = restore(&loser, &ledger, &rescue).unwrap();
            assert_eq!(restored["provenance"], "unknown");
            assert_eq!(restored["event_recorded"], false);
            assert!(restored.get("producer").is_none());
            assert_rescued_bundle(&root, "rescue", &losing_object);
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            drop(loser);
            drop(winner);
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn a_bundle_append_failure_keeps_the_complete_object_rescuable_without_a_receipt() {
        let (root, mut ctx, producer) = bundle_commit_fixture();
        let body = staged_bundle_delivery(
            &root,
            &crate::id::ulid(),
            &producer,
            None,
            &crate::id::ulid(),
        );
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let object = format!("objects/{revision}.bundle");
        let log = fs::read(ctx.store.log()).unwrap();
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        assert!(commit_capture_with(&mut ctx, body, |_, _| Err(Failure::Io(
            std::io::Error::other("Injected bundle append failure.")
        )))
        .is_err());
        ctx.unlock();
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        let ledger = diagnostic_ledger(&ctx.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["recorded_total"], 0);
        assert_eq!(inventory["orphans"], json!([object]));
        assert_eq!(
            inventory["bundle_objects"][0]["retained"]["status"],
            "matches"
        );
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue".into(),
        ])
        .unwrap();
        let restored = restore(&ctx, &ledger, &rescue).unwrap();
        assert_eq!(restored["provenance"], "unknown");
        assert_eq!(restored["event_recorded"], false);
        assert!(restored.get("producer").is_none());
        assert_rescued_bundle(&root, "rescue", &object);
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn a_partial_bundle_append_preserves_the_exact_tail_and_allows_only_unknown_rescue() {
        let (root, mut ctx, producer) = bundle_commit_fixture();
        let artifact = crate::id::ulid();
        let body = staged_bundle_delivery(&root, &artifact, &producer, None, &crate::id::ulid());
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let object = format!("objects/{revision}.bundle");
        let mut broken = fs::read(ctx.store.log()).unwrap();
        broken.extend_from_slice(b"{\"payload\":");
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        assert!(commit_capture_with(&mut ctx, body, |ctx, _| {
            fs::OpenOptions::new()
                .append(true)
                .open(ctx.store.log())
                .unwrap()
                .write_all(b"{\"payload\":")
                .unwrap();
            Err(Failure::Io(std::io::Error::other(
                "Injected partial bundle append failure.",
            )))
        })
        .is_err());
        ctx.unlock();
        assert_eq!(fs::read(ctx.store.log()).unwrap(), broken);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        let diagnostic =
            Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                .unwrap();
        assert_eq!(diagnostic.tree.broken_lines, 1);
        let ledger = diagnostic_ledger(&diagnostic.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&diagnostic, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["recorded_total"], 0);
        assert_eq!(inventory["provenance_complete"], false);
        assert_eq!(inventory["unattributed"], json!([object]));
        assert_eq!(inventory["orphans"], json!([]));
        let normal = Args::parse(vec![
            "restore".into(),
            artifact,
            format!("--revision={revision}"),
            "--to=normal".into(),
        ])
        .unwrap();
        assert!(execute_read(&diagnostic, &normal).is_err());
        assert!(!root.join("normal").exists());
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue".into(),
        ])
        .unwrap();
        let restored = restore(&diagnostic, &ledger, &rescue).unwrap();
        assert_eq!(restored["provenance"], "unknown");
        assert_eq!(restored["event_recorded"], false);
        assert!(restored.get("producer").is_none());
        assert_rescued_bundle(&root, "rescue", &object);
        assert_eq!(fs::read(ctx.store.log()).unwrap(), broken);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        drop(diagnostic);
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn competing_published_sqlite_applications_leave_the_rejected_complete_object_rescuable() {
        for shared_operation in [false, true] {
            let (root, mut ctx, producer) = sqlite_commit_fixture();
            let artifact = crate::id::ulid();
            let first =
                staged_sqlite_delivery(&root, &artifact, &producer, None, &crate::id::ulid());
            let saved = commit_capture(&mut ctx, first).unwrap();
            ctx.unlock();
            let previous = saved["delivery"]["revision"].as_str().unwrap();
            let mut winner =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let mut loser =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let operation = crate::id::ulid();
            let winning_body =
                staged_sqlite_delivery(&root, &artifact, &producer, Some(previous), &operation);
            let winning_revision = Revision::from_body("main", &winning_body)
                .unwrap()
                .revision
                .to_owned();
            let losing_operation = if shared_operation {
                operation.clone()
            } else {
                crate::id::ulid()
            };
            let losing_body = staged_sqlite_delivery(
                &root,
                &artifact,
                &producer,
                Some(previous),
                &losing_operation,
            );
            let losing_revision = Revision::from_body("main", &losing_body)
                .unwrap()
                .revision
                .to_owned();
            let losing_object = format!("objects/{losing_revision}.bundle");
            commit_capture(&mut winner, winning_body).unwrap();
            winner.unlock();
            let log = fs::read(ctx.store.log()).unwrap();
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            assert!(commit_capture(&mut loser, losing_body).is_err());
            loser.unlock();
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            assert_eq!(
                replay(&loser.tree)
                    .unwrap()
                    .latest(&artifact)
                    .unwrap()
                    .revision,
                winning_revision
            );
            let ledger = diagnostic_ledger(&loser.tree);
            let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
            let (inventory, code) = check(&loser, &ledger, &args).unwrap();
            assert_eq!(code, 1);
            assert_eq!(inventory["recorded_total"], 2);
            assert_eq!(inventory["orphans"], json!([losing_object]));
            assert_eq!(
                inventory["bundle_objects"][0]["retained"]["status"],
                "matches"
            );
            let rescue = Args::parse(vec![
                "restore".into(),
                format!("--orphan={losing_revision}"),
                "--to=rescue".into(),
            ])
            .unwrap();
            let restored = restore(&loser, &ledger, &rescue).unwrap();
            assert_eq!(restored["provenance"], "unknown");
            assert_eq!(restored["event_recorded"], false);
            assert!(restored.get("producer").is_none());
            assert_rescued_sqlite(&root, "rescue", &losing_object);
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            drop(loser);
            drop(winner);
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn a_sqlite_application_append_failure_keeps_the_complete_object_rescuable_without_a_receipt() {
        let (root, mut ctx, producer) = sqlite_commit_fixture();
        let body = staged_sqlite_delivery(
            &root,
            &crate::id::ulid(),
            &producer,
            None,
            &crate::id::ulid(),
        );
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let object = format!("objects/{revision}.bundle");
        let log = fs::read(ctx.store.log()).unwrap();
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        assert!(commit_capture_with(&mut ctx, body, |_, _| Err(Failure::Io(
            std::io::Error::other("Injected bundle append failure.")
        )))
        .is_err());
        ctx.unlock();
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        let ledger = diagnostic_ledger(&ctx.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["recorded_total"], 0);
        assert_eq!(inventory["orphans"], json!([object]));
        assert_eq!(
            inventory["bundle_objects"][0]["retained"]["status"],
            "matches"
        );
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue".into(),
        ])
        .unwrap();
        let restored = restore(&ctx, &ledger, &rescue).unwrap();
        assert_eq!(restored["provenance"], "unknown");
        assert_eq!(restored["event_recorded"], false);
        assert!(restored.get("producer").is_none());
        assert_rescued_sqlite(&root, "rescue", &object);
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn a_partial_sqlite_application_append_preserves_the_exact_tail_and_allows_only_unknown_rescue()
    {
        let (root, mut ctx, producer) = sqlite_commit_fixture();
        let artifact = crate::id::ulid();
        let body = staged_sqlite_delivery(&root, &artifact, &producer, None, &crate::id::ulid());
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let object = format!("objects/{revision}.bundle");
        let mut broken = fs::read(ctx.store.log()).unwrap();
        broken.extend_from_slice(b"{\"payload\":");
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        assert!(commit_capture_with(&mut ctx, body, |ctx, _| {
            fs::OpenOptions::new()
                .append(true)
                .open(ctx.store.log())
                .unwrap()
                .write_all(b"{\"payload\":")
                .unwrap();
            Err(Failure::Io(std::io::Error::other(
                "Injected partial bundle append failure.",
            )))
        })
        .is_err());
        ctx.unlock();
        assert_eq!(fs::read(ctx.store.log()).unwrap(), broken);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        let diagnostic =
            Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                .unwrap();
        assert_eq!(diagnostic.tree.broken_lines, 1);
        let ledger = diagnostic_ledger(&diagnostic.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&diagnostic, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["recorded_total"], 0);
        assert_eq!(inventory["provenance_complete"], false);
        assert_eq!(inventory["unattributed"], json!([object]));
        assert_eq!(inventory["orphans"], json!([]));
        let normal = Args::parse(vec![
            "restore".into(),
            artifact,
            format!("--revision={revision}"),
            "--to=normal".into(),
        ])
        .unwrap();
        assert!(execute_read(&diagnostic, &normal).is_err());
        assert!(!root.join("normal").exists());
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue".into(),
        ])
        .unwrap();
        let restored = restore(&diagnostic, &ledger, &rescue).unwrap();
        assert_eq!(restored["provenance"], "unknown");
        assert_eq!(restored["event_recorded"], false);
        assert!(restored.get("producer").is_none());
        assert_rescued_sqlite(&root, "rescue", &object);
        assert_eq!(fs::read(ctx.store.log()).unwrap(), broken);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        drop(diagnostic);
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    #[ignore = "Invoked by the bundle process interruption test with an isolated fixture."]
    fn bundle_interruption_child() {
        let root = std::path::PathBuf::from(
            std::env::var_os("VIVAC_BUNDLE_INTERRUPTION_ROOT")
                .expect("an isolated interruption fixture is required"),
        );
        assert!(root.is_absolute());
        let root = root.canonicalize().unwrap();
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        guard_fixture_root(&root, &crate_root);
        assert!(
            root.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .strip_prefix("vivac-artifact-commit-")
                .is_some_and(valid_id),
            "the interruption root must be a generated commit fixture"
        );
        for (path, bytes) in [
            ("index.html", BUNDLE_HTML),
            ("assets/style.css", BUNDLE_CSS),
            ("assets/main.js", BUNDLE_JS),
        ] {
            assert_eq!(fs::read(root.join("app").join(path)).unwrap(), bytes);
        }
        let mode = std::env::var("VIVAC_BUNDLE_INTERRUPTION_MODE")
            .expect("an interruption mode is required");
        match mode.as_str() {
            "before_publication" => {
                let root_handle = files::open_root(&root).unwrap();
                let custody = files::store(&root_handle, true).unwrap();
                let objects = files::directory(&custody, "objects", true).unwrap();
                let preparation = native::Preparation::new(&objects).unwrap();
                let mut partial = preparation.dir.create("index.html").unwrap();
                partial
                    .write_all(b"<html>Partial selected delivery</html>")
                    .unwrap();
                partial.sync_all().unwrap();
                native::sync_preparation(&preparation.dir).unwrap();
                std::process::exit(86);
            }
            "after_publication" => {
                let ctx = Ctx::load_for_capture(
                    Store::open_strict(root.clone()).unwrap(),
                    Whose::Founding,
                )
                .unwrap();
                let producer = ctx.tree.resolve("t1").unwrap().id.clone();
                staged_bundle_delivery(
                    &root,
                    &crate::id::ulid(),
                    &producer,
                    None,
                    &crate::id::ulid(),
                );
                std::process::exit(86);
            }
            _ => panic!("Unknown bundle interruption mode."),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn interrupted_bundle_preparation_and_publication_remain_diagnosable_and_rescuable() {
        for mode in ["before_publication", "after_publication"] {
            let (root, ctx, _) = bundle_commit_fixture();
            let log = fs::read(ctx.store.log()).unwrap();
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            drop(ctx);
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "artifacts::tests::bundle_interruption_child",
                    "--test-threads=1",
                ])
                .env("VIVAC_BUNDLE_INTERRUPTION_ROOT", &root)
                .env("VIVAC_BUNDLE_INTERRUPTION_MODE", mode)
                .output()
                .unwrap();
            assert_eq!(
                child.status.code(),
                Some(86),
                "interruption mode {mode}: stdout: {}; stderr: {}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            let diagnostic =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            assert_eq!(diagnostic.tree.broken_lines, 0);
            let ledger = diagnostic_ledger(&diagnostic.tree);
            let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
            let (inventory, code) = check(&diagnostic, &ledger, &args).unwrap();
            assert_eq!(code, 1);
            assert_eq!(inventory["recorded_total"], 0);
            assert_eq!(inventory["unattributed_total"], 0);
            if mode == "before_publication" {
                assert_eq!(inventory["orphans_total"], 0);
                assert_eq!(inventory["temporaries_total"], 1);
                let temporary = inventory["temporaries"][0].as_str().unwrap();
                assert!(temporary.starts_with("objects/.tmp-bundle-"));
                assert_eq!(
                    fs::read(
                        root.join(".vivac-artifacts")
                            .join(temporary)
                            .join("index.html")
                    )
                    .unwrap(),
                    b"<html>Partial selected delivery</html>"
                );
                assert!(!root.join("rescue").exists());
            } else {
                assert_eq!(inventory["temporaries_total"], 0);
                assert_eq!(inventory["orphans_total"], 1);
                assert_eq!(
                    inventory["bundle_objects"][0]["retained"]["status"],
                    "matches"
                );
                let object = inventory["orphans"][0].as_str().unwrap();
                let revision = object
                    .strip_prefix("objects/")
                    .unwrap()
                    .strip_suffix(".bundle")
                    .unwrap();
                let rescue = Args::parse(vec![
                    "restore".into(),
                    format!("--orphan={revision}"),
                    "--to=rescue".into(),
                ])
                .unwrap();
                let restored = restore(&diagnostic, &ledger, &rescue).unwrap();
                assert_eq!(restored["provenance"], "unknown");
                assert_eq!(restored["event_recorded"], false);
                assert!(restored.get("producer").is_none());
                assert_rescued_bundle(&root, "rescue", object);
            }
            assert_eq!(fs::read(diagnostic.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            drop(diagnostic);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn oversized_source_references_are_rejected_before_detector_allocation() {
        let source = format!("person@example.com/{}", "x\n".repeat(2 * 1024 * 1024));
        let (result, peak) = crate::test_alloc::measure(|| guard_source_reference(&source));
        assert!(
            peak <= 2 * 1024 * 1024,
            "oversized source allocated {peak} auxiliary bytes"
        );
        assert!(matches!(result, Err(Failure::Usage(_))));
        assert!(matches!(guard_source_reference(""), Err(Failure::Usage(_))));
    }

    #[test]
    fn oversized_request_fields_are_rejected_without_cloning_the_json_request() {
        let long = "x".repeat(2 * 1024 * 1024);
        let path = json!({"operation":"capture", "path":long, "from":"1"});
        let mut unknown = serde_json::Map::new();
        unknown.insert("operation".into(), json!("list"));
        unknown.insert("x".repeat(2 * 1024 * 1024), Value::Bool(true));
        let unknown = Value::Object(unknown);
        let limit = json!({"operation":"list", "limit":"x".repeat(2 * 1024 * 1024)});
        let store = json!({"operation":"check", "store":"x".repeat(2 * 1024 * 1024)});
        for value in [&path, &unknown, &limit, &store] {
            let (result, peak) = crate::test_alloc::measure(|| request_args(value));
            assert!(
                peak <= 2 * 1024 * 1024,
                "invalid request allocated {peak} auxiliary bytes"
            );
            assert!(matches!(result, Err(Failure::Usage(_))));
        }
    }

    #[test]
    fn sensitive_source_components_are_rejected_before_retaining_bytes() {
        assert!(crate::redact::check_field("artifact reference", "person@example.com").is_some());
        let (root, ctx, producer) = capture_commit_fixture();
        let source = "person@example.com/mockup.html";
        fs::create_dir(root.join("person@example.com")).unwrap();
        fs::write(root.join(source), b"<html>Selected delivery</html>").unwrap();
        let log_before = fs::read(ctx.store.log()).unwrap();
        let args = Args::parse(vec![
            "capture".into(),
            "--description=Selected fixture layout".into(),
            source.into(),
            format!("--from={producer}"),
        ])
        .unwrap();
        assert!(matches!(
            prepare_capture(&ctx, &args),
            Err(Failure::Redaction(_))
        ));
        assert!(!root.join(".vivac-artifacts").exists());
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_published_deliveries_leave_the_rejected_object_available_for_explicit_rescue() {
        for shared_operation in [false, true] {
            let (root, mut ctx, producer) = capture_commit_fixture();
            let artifact = crate::id::ulid();
            let first = staged_delivery(&root, &artifact, &producer, None, &crate::id::ulid());
            let saved = commit_capture(&mut ctx, first).unwrap();
            ctx.unlock();
            let previous = saved["delivery"]["revision"].as_str().unwrap();
            let mut winner =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let mut loser =
                Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                    .unwrap();
            let operation = crate::id::ulid();
            let winning_body =
                staged_delivery(&root, &artifact, &producer, Some(previous), &operation);
            let losing_operation = if shared_operation {
                operation.clone()
            } else {
                crate::id::ulid()
            };
            let losing_body = staged_delivery(
                &root,
                &artifact,
                &producer,
                Some(previous),
                &losing_operation,
            );
            let losing_revision = Revision::from_body("main", &losing_body)
                .unwrap()
                .revision
                .to_owned();
            commit_capture(&mut winner, winning_body).unwrap();
            winner.unlock();
            let log = fs::read(ctx.store.log()).unwrap();
            assert!(commit_capture(&mut loser, losing_body).is_err());
            loser.unlock();
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            let ledger = diagnostic_ledger(&loser.tree);
            let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
            let (inventory, code) = check(&loser, &ledger, &args).unwrap();
            assert_eq!(code, 1);
            assert_eq!(inventory["recorded_total"], 2);
            assert_eq!(
                inventory["orphans"],
                json!([format!("objects/{losing_revision}.blob")])
            );
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            let rescue = Args::parse(vec![
                "restore".into(),
                format!("--orphan={losing_revision}"),
                "--to=rescue.html".into(),
            ])
            .unwrap();
            let restored = restore(&loser, &ledger, &rescue).unwrap();
            assert_eq!(restored["provenance"], "unknown");
            assert_eq!(restored["event_recorded"], false);
            assert_eq!(
                fs::read(root.join("rescue.html")).unwrap(),
                b"<html>Selected delivery</html>"
            );
            assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
            assert_eq!(fs::read(&config_path).unwrap(), config);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn an_append_failure_keeps_published_bytes_rescuable_without_a_delivery_event() {
        let (root, mut ctx, producer) = capture_commit_fixture();
        let body = staged_delivery(
            &root,
            &crate::id::ulid(),
            &producer,
            None,
            &crate::id::ulid(),
        );
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let log = fs::read(ctx.store.log()).unwrap();
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        let result = commit_capture_with(&mut ctx, body, |_, _| {
            Err(Failure::Io(std::io::Error::other(
                "Injected append failure.",
            )))
        });
        assert!(result.is_err());
        ctx.unlock();
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        let ledger = diagnostic_ledger(&ctx.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&ctx, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["recorded_total"], 0);
        assert_eq!(
            inventory["orphans"],
            json!([format!("objects/{revision}.blob")])
        );
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue.html".into(),
        ])
        .unwrap();
        assert_eq!(
            restore(&ctx, &ledger, &rescue).unwrap()["provenance"],
            "unknown"
        );
        assert_eq!(
            fs::read(root.join("rescue.html")).unwrap(),
            b"<html>Selected delivery</html>"
        );
        assert_eq!(fs::read(ctx.store.log()).unwrap(), log);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure metadata phases."]
    fn measure_ten_thousand_artifact_metadata_phases() {
        let (root, ctx) = store_check_fixture(10_000);
        let reference = format!("artifact:{:026x}@{:026x}", 19_999, 29_999);
        let refs = vec![reference];
        let mut replay_samples = Vec::with_capacity(100);
        let mut reference_samples = Vec::with_capacity(100);
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let ledger = replay(&ctx.tree).unwrap();
            drop(ledger);
            replay_samples.push(started.elapsed());
            let started = std::time::Instant::now();
            validate_references(&ctx.tree, &refs).unwrap();
            reference_samples.push(started.elapsed());
        }
        replay_samples.sort();
        reference_samples.sort();
        eprintln!("Artifact metadata phases: 100 samples, 10000 distinct artifacts, short source paths; replay p99={:?}, typed reference p99={:?}. File copy, append, sync, lock wait and output are excluded.", replay_samples[98], reference_samples[98]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_simulated_partial_append_preserves_the_broken_tail_and_allows_only_unknown_rescue() {
        let (root, mut ctx, producer) = capture_commit_fixture();
        let body = staged_delivery(
            &root,
            &crate::id::ulid(),
            &producer,
            None,
            &crate::id::ulid(),
        );
        let revision = Revision::from_body("main", &body)
            .unwrap()
            .revision
            .to_owned();
        let config_path = root.join(store::DIR).join(store::CONFIG);
        let config = fs::read(&config_path).unwrap();
        let result = commit_capture_with(&mut ctx, body, |ctx, _| {
            fs::OpenOptions::new()
                .append(true)
                .open(ctx.store.log())
                .unwrap()
                .write_all(b"{\"payload\":")
                .unwrap();
            Err(Failure::Io(std::io::Error::other(
                "Injected partial append failure.",
            )))
        });
        assert!(result.is_err());
        ctx.unlock();
        let broken = fs::read(ctx.store.log()).unwrap();
        let diagnostic =
            Ctx::load_for_write(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                .unwrap();
        assert_eq!(diagnostic.tree.broken_lines, 1);
        let ledger = diagnostic_ledger(&diagnostic.tree);
        let args = Args::parse(vec!["check".into(), "--store".into()]).unwrap();
        let (inventory, code) = check(&diagnostic, &ledger, &args).unwrap();
        assert_eq!(code, 1);
        assert_eq!(inventory["provenance_complete"], false);
        assert_eq!(
            inventory["unattributed"],
            json!([format!("objects/{revision}.blob")])
        );
        assert_eq!(inventory["orphans"], json!([]));
        let rescue = Args::parse(vec![
            "restore".into(),
            format!("--orphan={revision}"),
            "--to=rescue.html".into(),
        ])
        .unwrap();
        let restored = restore(&diagnostic, &ledger, &rescue).unwrap();
        assert_eq!(restored["provenance"], "unknown");
        assert_eq!(restored["event_recorded"], false);
        assert_eq!(
            fs::read(root.join("rescue.html")).unwrap(),
            b"<html>Selected delivery</html>"
        );
        assert_eq!(fs::read(ctx.store.log()).unwrap(), broken);
        assert_eq!(fs::read(&config_path).unwrap(), config);
        fs::remove_dir_all(root).unwrap();
    }

    fn capture_tail_fixture() -> (std::path::PathBuf, Ctx, String, Body) {
        let (root, old, producer) = capture_commit_fixture();
        drop(old);
        let ctx = Ctx::load_for_capture(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
            .unwrap();
        let revision = crate::id::ulid();
        let body = Body::ArtifactCaptured {
            artifact: crate::id::ulid(),
            revision: revision.clone(),
            operation: crate::id::ulid(),
            producer: producer.clone(),
            previous: None,
            source: "mockup.html".into(),
            site: crate::id::ulid(),
            size: 17,
            digest: "a".repeat(64),
            object: format!("objects/{revision}.blob"),
        };
        (root, ctx, producer, body)
    }

    fn tail_note(producer: &str, seq: u64, note: &str) -> Vec<u8> {
        tail_event(
            Body::NodeNoted {
                node: producer.into(),
                note: note.into(),
            },
            seq,
            crate::lane::MAIN,
        )
    }

    fn tail_event(payload: Body, seq: u64, lane: &str) -> Vec<u8> {
        let event = crate::event::Event {
            seq,
            id: crate::id::ulid(),
            ts: "2026-01-01T00:00:00Z".into(),
            actor: "opaque".into(),
            lane: lane.into(),
            payload,
        };
        let mut bytes = serde_json::to_vec(&event).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn append_capture_tail(ctx: &Ctx, bytes: &[u8]) {
        use std::io::Write;
        let lock = ctx.store.lock_for_write().unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(ctx.store.log())
            .unwrap()
            .write_all(bytes)
            .unwrap();
        drop(lock);
    }

    #[test]
    fn capture_tail_accepts_exact_byte_and_event_limits() {
        for events in [2, 64] {
            let (root, mut ctx, producer, body) = capture_tail_fixture();
            let initial = ctx.tree.seq;
            let mut tail = Vec::new();
            for index in 0..events {
                tail.extend(tail_note(&producer, initial + index + 1, "External work"));
            }
            if events == 2 {
                tail.extend(vec![b'\n'; 64 * 1024 - tail.len()]);
                assert_eq!(tail.len(), 64 * 1024);
            }
            append_capture_tail(&ctx, &tail);
            let ((result, seq), peak) = crate::test_alloc::measure(|| {
                let result = commit_capture(&mut ctx, body);
                ctx.unlock();
                (result, ctx.tree.seq)
            });
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
            result.unwrap();
            assert_eq!(seq, initial + events + 1);
            eprintln!(
                "Capture tail at {events} events and {} bytes: auxiliary peak={peak} bytes.",
                tail.len()
            );
            assert!(
                peak <= 2 * 1024 * 1024,
                "maximum capture tail allocated {peak} auxiliary bytes"
            );
        }
    }

    #[test]
    fn capture_tail_rejects_over_limit_without_applying_a_prefix_or_writing_a_receipt() {
        for excessive_events in [false, true] {
            let (root, mut ctx, producer, body) = capture_tail_fixture();
            let initial = ctx.tree.seq;
            let mut tail = Vec::new();
            for index in 0..if excessive_events { 65 } else { 2 } {
                tail.extend(tail_note(&producer, initial + index + 1, "External work"));
            }
            if !excessive_events {
                tail.extend(vec![b'\n'; 64 * 1024 + 1 - tail.len()]);
            }
            append_capture_tail(&ctx, &tail);
            let log = fs::read(ctx.store.log()).unwrap();
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            let result = commit_capture(&mut ctx, body);
            ctx.unlock();
            let seq = ctx.tree.seq;
            let unchanged = fs::read(ctx.store.log()).unwrap() == log
                && fs::read(config_path).unwrap() == config;
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
            assert!(
                result.is_err(),
                "a capture accepted an excessive external tail"
            );
            assert_eq!(seq, initial, "a rejected tail partially changed the tree");
            assert!(unchanged, "a rejected capture changed log or config bytes");
        }
    }

    #[test]
    fn capture_tail_rejects_broken_torn_and_unknown_lines_without_applying_valid_prefixes() {
        for suffix in [
            b"{bad}\n".as_slice(),
            b"{\"payload\":".as_slice(),
            b"{\"payload\":{\"type\":\"future.event\"}}\n".as_slice(),
        ] {
            let (root, mut ctx, producer, body) = capture_tail_fixture();
            let initial = ctx.tree.seq;
            let mut tail = tail_note(&producer, initial + 1, "External work");
            tail.extend(suffix);
            append_capture_tail(&ctx, &tail);
            let log = fs::read(ctx.store.log()).unwrap();
            let result = commit_capture(&mut ctx, body);
            ctx.unlock();
            let seq = ctx.tree.seq;
            let unchanged = fs::read(ctx.store.log()).unwrap() == log;
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
            assert!(result.is_err());
            assert_eq!(seq, initial, "a rejected tail partially changed the tree");
            assert!(unchanged);
        }
    }

    #[test]
    fn capture_tail_rejects_replacement_truncation_and_full_line_checkpoint_changes() {
        use std::io::{Seek, SeekFrom, Write};
        for change in 0..3 {
            let (root, mut ctx, producer, body) = capture_tail_fixture();
            append_capture_tail(&ctx, &tail_note(&producer, 2, &"x".repeat(1024)));
            drop(ctx);
            ctx = Ctx::load_for_capture(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
                .unwrap();
            let log_path = ctx.store.log();
            let fingerprint = store::fingerprint(&log_path);
            let stamp = fs::metadata(&log_path).unwrap().modified().unwrap();
            let mut bytes = fs::read(&log_path).unwrap();
            let start = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
            let position = start
                + bytes[start..]
                    .windows(b"\"seq\":2".len())
                    .position(|window| window == b"\"seq\":2")
                    .unwrap()
                + b"\"seq\":".len();
            assert!(bytes.len() - position > 256);
            if change == 0 {
                store::replace_whole(&log_path, &root.join("replacement"), &bytes).unwrap();
            } else if change == 1 {
                bytes[position] = b'9';
                let mut file = fs::OpenOptions::new().write(true).open(&log_path).unwrap();
                file.seek(SeekFrom::Start(position as u64)).unwrap();
                file.write_all(b"9").unwrap();
            } else {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&log_path)
                    .unwrap()
                    .set_len(bytes.len() as u64 - 1)
                    .unwrap();
                bytes.pop();
            }
            fs::OpenOptions::new()
                .write(true)
                .open(&log_path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(stamp))
                .unwrap();
            if change != 2 {
                assert_eq!(store::fingerprint(&log_path), fingerprint);
            }
            let config_path = root.join(store::DIR).join(store::CONFIG);
            let config = fs::read(&config_path).unwrap();
            let initial = ctx.tree.seq;
            let first = commit_capture(&mut ctx, body.clone());
            ctx.unlock();
            let second = commit_capture(&mut ctx, body);
            ctx.unlock();
            let unchanged =
                fs::read(log_path).unwrap() == bytes && fs::read(config_path).unwrap() == config;
            let seq = ctx.tree.seq;
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
            assert!(first.is_err());
            assert!(
                second.is_err(),
                "a stale capture context recovered without a fresh load"
            );
            assert_eq!(seq, initial);
            assert!(unchanged);
        }
    }

    #[test]
    fn capture_tail_rejects_a_visible_historical_rewrite_with_an_intact_last_line() {
        use std::io::{Seek, SeekFrom, Write};
        let (root, mut ctx, producer, body) = capture_tail_fixture();
        append_capture_tail(&ctx, &tail_note(&producer, 2, "External work"));
        drop(ctx);
        ctx = Ctx::load_for_capture(Store::open_strict(root.clone()).unwrap(), Whose::Founding)
            .unwrap();
        let path = ctx.store.log();
        let before = fs::read(&path).unwrap();
        let start = before.iter().position(|byte| *byte == b'\n').unwrap() + 1;
        let stamp = fs::metadata(&path).unwrap().modified().unwrap();
        let fingerprint = store::fingerprint(&path);
        let position = before
            .windows(b"\"seq\":1".len())
            .position(|window| window == b"\"seq\":1")
            .unwrap()
            + b"\"seq\":".len();
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(position as u64)).unwrap();
        file.write_all(b"7").unwrap();
        file.set_times(
            fs::FileTimes::new().set_modified(stamp + std::time::Duration::from_secs(2)),
        )
        .unwrap();
        drop(file);
        let changed = fs::read(&path).unwrap();
        assert_eq!(&changed[start..], &before[start..]);
        let current = store::fingerprint(&path);
        assert_eq!(current.0, fingerprint.0);
        assert_ne!(current.1, fingerprint.1);
        let result = commit_capture(&mut ctx, body);
        ctx.unlock();
        let seq = ctx.tree.seq;
        let unchanged = fs::read(path).unwrap() == changed;
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
        assert!(result.is_err());
        assert_eq!(seq, 2);
        assert!(unchanged);
    }

    #[test]
    fn capture_tail_accepts_crlf_and_keeps_its_cursor_current_after_own_appends() {
        let (root, mut ctx, producer, body) = capture_tail_fixture();
        let mut tail = b"\r\n".to_vec();
        let mut note = tail_note(&producer, 2, "External work");
        note.insert(note.len() - 1, b'\r');
        tail.extend(note);
        append_capture_tail(&ctx, &tail);
        let first = commit_capture(&mut ctx, body.clone());
        ctx.unlock();
        let mut second = body;
        if let Body::ArtifactCaptured {
            artifact,
            revision,
            operation,
            object,
            ..
        } = &mut second
        {
            *artifact = crate::id::ulid();
            *revision = crate::id::ulid();
            *operation = crate::id::ulid();
            *object = format!("objects/{revision}.blob");
        }
        let second = commit_capture(&mut ctx, second);
        ctx.unlock();
        let (recorded, broken) = ctx.store.read_all().unwrap();
        let seq = ctx.tree.seq;
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
        first.unwrap();
        second.unwrap();
        assert_eq!(broken, 0);
        assert_eq!(seq, 4);
        assert_eq!(
            recorded.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn capture_tail_rejects_json_item_overflow_before_applying_valid_events() {
        let (root, mut ctx, producer, body) = capture_tail_fixture();
        let mut tail = tail_note(&producer, 2, "External work");
        let mut value: Value =
            serde_json::from_slice(&tail_note(&producer, 3, "External work")).unwrap();
        value["extra"] = json!(vec![0; 1025]);
        tail.extend(serde_json::to_vec(&value).unwrap());
        tail.push(b'\n');
        assert!(tail.len() < 64 * 1024);
        append_capture_tail(&ctx, &tail);
        let ((result, seq), peak) = crate::test_alloc::measure(|| {
            let result = commit_capture(&mut ctx, body);
            ctx.unlock();
            (result, ctx.tree.seq)
        });
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
        assert!(result.is_err());
        assert_eq!(seq, 1);
        assert!(peak <= 2 * 1024 * 1024);
    }

    #[test]
    fn capture_tail_refreshes_the_authoritative_head_and_rejects_a_stale_plan() {
        for shared_operation in [false, true] {
            let (root, mut ctx, _, planned) = capture_tail_fixture();
            let mut winner = planned.clone();
            if let Body::ArtifactCaptured {
                revision,
                operation,
                object,
                ..
            } = &mut winner
            {
                *revision = crate::id::ulid();
                *object = format!("objects/{revision}.blob");
                if !shared_operation {
                    *operation = crate::id::ulid();
                }
            }
            append_capture_tail(&ctx, &tail_event(winner.clone(), 2, crate::lane::MAIN));
            let before = fs::read(ctx.store.log()).unwrap();
            let result = commit_capture(&mut ctx, planned);
            ctx.unlock();
            let history = ctx.tree.artifact_history.clone();
            let unchanged = fs::read(ctx.store.log()).unwrap() == before;
            drop(ctx);
            fs::remove_dir_all(root).unwrap();
            assert!(result.is_err());
            assert_eq!(history, vec![(crate::lane::MAIN.into(), winner)]);
            assert!(unchanged);
        }
    }

    #[test]
    fn capture_tail_refreshes_main_claimed_before_authorizing_a_receipt() {
        let (root, mut ctx, _, body) = capture_tail_fixture();
        append_capture_tail(
            &ctx,
            &tail_event(
                Body::LaneClaimed {
                    lane: crate::lane::MAIN.into(),
                },
                2,
                "other",
            ),
        );
        let before = fs::read(ctx.store.log()).unwrap();
        let result = commit_capture(&mut ctx, body);
        ctx.unlock();
        let claimed = ctx.tree.main_claimed;
        let unchanged = fs::read(ctx.store.log()).unwrap() == before;
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
        assert!(result.is_err());
        assert!(claimed);
        assert!(unchanged);
    }

    #[test]
    fn capture_tail_resolves_a_late_producer_and_sorts_nodes_like_a_full_fold() {
        let (root, mut ctx, _, mut body) = capture_tail_fixture();
        let parent = crate::id::ulid();
        let child = crate::id::ulid();
        let create = |node: &str, num: u64, parent: Option<String>| Body::NodeCreated {
            node: node.into(),
            num,
            kind: crate::event::Kind::Task,
            title: "Produce selected work".into(),
            why: String::new(),
            parent,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        };
        let mut tail = tail_event(
            create(&child, 3, Some(parent.clone())),
            2,
            crate::lane::MAIN,
        );
        tail.extend(tail_event(create(&parent, 2, None), 3, crate::lane::MAIN));
        append_capture_tail(&ctx, &tail);
        if let Body::ArtifactCaptured { producer, .. } = &mut body {
            *producer = child.clone();
        }
        let result = commit_capture(&mut ctx, body);
        ctx.unlock();
        let (recorded, broken) = ctx.store.read_all().unwrap();
        let expected = crate::model::fold(&recorded, broken);
        let roots_match = ctx.tree.roots == expected.roots;
        let child_parent = ctx.tree.node(&child).unwrap().parent;
        let expected_parent = expected.node(&child).unwrap().parent;
        let children_match = ctx
            .tree
            .children(2)
            .iter()
            .map(|node| node.num)
            .collect::<Vec<_>>()
            == expected
                .children(2)
                .iter()
                .map(|node| node.num)
                .collect::<Vec<_>>();
        let alias = ctx.tree.resolve("t3").map(|node| node.num);
        let aliases_match = alias == expected.resolve("t3").map(|node| node.num);
        let history = ctx.tree.artifact_history.clone();
        let expected_history = expected.artifact_history;
        drop(ctx);
        fs::remove_dir_all(root).unwrap();
        result.unwrap();
        assert!(roots_match && children_match && aliases_match);
        assert_eq!(alias, Some(3));
        assert_eq!(child_parent, expected_parent);
        assert_eq!(child_parent, Some(2));
        assert_eq!(history, expected_history);
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure receipt commits after external writes."]
    fn measure_one_hundred_receipt_commits_after_external_writes() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        let root = standalone_root("artifact-stale-commit-benchmark");
        let mut store = Store::create(&root).unwrap();
        let producer = format!("{:026x}", 1);
        let receipt = |artifact: u64, revision: u64, operation: u64| {
            let revision = format!("{revision:026x}");
            Body::ArtifactCapturedV2 {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{artifact:026x}"),
                revision: revision.clone(),
                operation: format!("{operation:026x}"),
                producer: producer.clone(),
                previous: None,
                source: "mockup.html".into(),
                site: format!("{:026x}", 2),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.blob"),
            }
        };
        let mut bodies = Vec::with_capacity(10_001);
        bodies.push(Body::NodeCreated {
            node: producer.clone(),
            num: 1,
            kind: crate::event::Kind::Task,
            title: "Produce selected deliveries".into(),
            why: String::new(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        for index in 0..10_000u64 {
            bodies.push(receipt(10_000 + index, 20_000 + index, 30_000 + index));
        }
        for body in bodies.iter().skip(1) {
            assert!(
                matches!(body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
        }
        let lock = store.lock_for_write().unwrap();
        store
            .append(&lock, crate::lane::MAIN, bodies, 0, false)
            .unwrap();
        drop(lock);
        let mut ctx = Ctx::load_for_capture(store, Whose::Founding).unwrap();
        let mut external = Store::open_strict(root.clone()).unwrap();
        let mut durations = Vec::with_capacity(100);
        let mut results = Vec::with_capacity(100);
        let mut observed_sequences = [0; 100];
        let mut external_sequences = [0; 100];
        let mut peak = 0;
        for index in 0..100u64 {
            let lock = external.lock_for_write().unwrap();
            let appended = external
                .append(
                    &lock,
                    crate::lane::MAIN,
                    vec![Body::NodeNoted {
                        node: producer.clone(),
                        note: format!("External selected work {index}"),
                    }],
                    10_001 + 2 * index,
                    false,
                )
                .unwrap();
            drop(lock);
            external_sequences[index as usize] = appended.events[0].seq;
            let body = receipt(60_000 + index, 50_000 + index, 40_000 + index);
            assert!(
                matches!(&body, Body::ArtifactCapturedV2 { description, .. } if description.chars().count() == 280)
            );
            let ((result, duration), sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let result = commit_capture(&mut ctx, body);
                ctx.unlock();
                (result, started.elapsed())
            });
            observed_sequences[index as usize] = ctx.tree.seq;
            durations.push(duration);
            results.push(result);
            peak = peak.max(sample_peak);
        }
        durations.sort();
        let recorded = ctx.store.read_all();
        drop(external);
        drop(ctx);
        fs::remove_dir_all(&root).unwrap();
        eprintln!("Receipt commits after external writes: 100 release samples, plain native filesystem fixture, one producer and 10000 prior distinct receipts; p50={:?}, p99={:?}, max={:?}, peak auxiliary={peak} bytes. Before every sample another Store appended one valid NodeNoted under the same tree lock, outside timing and the allocation counter. Each sample measures real commit_capture plus unlock, including stale refresh, guards, append and response. Initial authoritative streaming capture loading, prepared metadata and byte copying are excluded. Byte capture is not measured. Five millisecond budget passed={}.", durations[49], durations[98], durations[99], durations[98] < std::time::Duration::from_millis(5));
        for result in results {
            result.unwrap();
        }
        let (recorded, broken) = recorded.unwrap();
        assert_eq!(broken, 0);
        assert_eq!(recorded.len(), 10_201);
        for index in 0..100usize {
            let external_seq = 10_002 + 2 * index as u64;
            assert_eq!(external_sequences[index], external_seq);
            assert_eq!(observed_sequences[index], external_seq + 1);
            let note = &recorded[10_001 + 2 * index];
            assert_eq!(note.seq, external_seq);
            assert!(
                matches!(&note.payload, Body::NodeNoted { node, note } if node == &producer && note == &format!("External selected work {index}"))
            );
            let saved = &recorded[10_002 + 2 * index];
            assert_eq!(saved.seq, external_seq + 1);
            assert!(
                matches!(&saved.payload, Body::ArtifactCapturedV2 { revision, producer: node, .. } if revision == &format!("{:026x}", 50_000 + index as u64) && node == &producer)
            );
        }
        assert!(
            peak <= 2 * 1024 * 1024,
            "receipt commit after an external write allocated {peak} auxiliary bytes"
        );
        assert!(
            durations[98] < std::time::Duration::from_millis(5),
            "receipt commit after an external write exceeded its strict five millisecond p99 budget"
        );
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure receipt commits after external writes."]
    fn measure_one_hundred_resident_receipt_commits_after_external_writes() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "Run this benchmark in release mode."
        );
        let root = standalone_root("artifact-project-stale-commit-benchmark");
        let mut store = Store::create(&root).unwrap();
        let producer = format!("{:026x}", 1);
        let receipt = |artifact: u64, revision: u64, operation: u64| {
            let revision = format!("{revision:026x}");
            Body::ArtifactCapturedV2 {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{artifact:026x}"),
                revision: revision.clone(),
                operation: format!("{operation:026x}"),
                producer: producer.clone(),
                previous: None,
                source: "mockup.html".into(),
                site: format!("{:026x}", 2),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.blob"),
            }
        };
        let mut bodies = Vec::with_capacity(10_001);
        bodies.push(Body::NodeCreated {
            node: producer.clone(),
            num: 1,
            kind: crate::event::Kind::Task,
            title: "Produce selected deliveries".into(),
            why: String::new(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        for index in 0..10_000u64 {
            bodies.push(receipt(10_000 + index, 20_000 + index, 30_000 + index));
        }
        for body in bodies.iter().skip(1) {
            assert!(
                matches!(body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
        }
        let lock = store.lock_for_write().unwrap();
        store
            .append(&lock, crate::lane::MAIN, bodies, 0, false)
            .unwrap();
        drop(lock);
        drop(store);
        let mut project = crate::project::Project::open(
            root.clone(),
            "Receipt benchmark".into(),
            "receipt-benchmark".into(),
            Whose::Founding,
        )
        .unwrap();
        let mut external = Store::open_strict(root.clone()).unwrap();
        let mut durations = Vec::with_capacity(100);
        let mut results = Vec::with_capacity(100);
        let mut observed_sequences = [0; 100];
        let mut external_sequences = [0; 100];
        let mut peak = 0;
        for index in 0..100u64 {
            let lock = external.lock_for_write().unwrap();
            let appended = external
                .append(
                    &lock,
                    crate::lane::MAIN,
                    vec![Body::NodeNoted {
                        node: producer.clone(),
                        note: format!("External selected work {index}"),
                    }],
                    10_001 + 2 * index,
                    false,
                )
                .unwrap();
            drop(lock);
            external_sequences[index as usize] = appended.events[0].seq;
            let body = receipt(60_000 + index, 50_000 + index, 40_000 + index);
            assert!(
                matches!(&body, Body::ArtifactCapturedV2 { description, .. } if description.chars().count() == 280)
            );
            let ((result, sequence, duration), sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let mut sequence = 0;
                let result = project.write(|ctx| {
                    let result = commit_capture(ctx, body);
                    sequence = ctx.tree.seq;
                    result
                });
                (result, sequence, started.elapsed())
            });
            observed_sequences[index as usize] = sequence;
            durations.push(duration);
            results.push(result);
            peak = peak.max(sample_peak);
        }
        durations.sort();
        let recorded = external.read_all();
        let resident_pairs = project.current_with_log().map(|(_, log)| {
            log.len() == 10_201 && log[10_001..].as_chunks::<2>().0.iter().enumerate().all(|(index, pair)| {
                let sequence = 10_002 + 2 * index as u64;
                pair[0].seq == sequence && pair[1].seq == sequence + 1
                    && matches!(&pair[0].payload, Body::NodeNoted { node, note } if node == &producer && note == &format!("External selected work {index}"))
                    && matches!(&pair[1].payload, Body::ArtifactCapturedV2 { revision, producer: node, .. } if revision == &format!("{:026x}", 50_000 + index as u64) && node == &producer)
            })
        });
        drop(external);
        drop(project);
        fs::remove_dir_all(&root).unwrap();
        eprintln!("Resident Project.write receipt commits after external writes: 100 release samples, plain native filesystem fixture, one producer and 10000 prior distinct receipts; p50={:?}, p99={:?}, max={:?}, peak auxiliary={peak} bytes. Before every sample another Store appended one valid NodeNoted under the same tree lock, outside timing and the allocation counter. Each sample measures real Project.write with commit_capture, including lock, stale refresh, guards, append, response, unlock and resident log bookkeeping. Initial Project opening and its loaded tree/log, prepared metadata and byte copying are excluded. MCP tool envelope serialization is not measured. Byte capture is not measured. Five millisecond budget passed={}.", durations[49], durations[98], durations[99], durations[98] < std::time::Duration::from_millis(5));
        for result in results {
            result.unwrap();
        }
        assert!(
            resident_pairs.unwrap(),
            "the resident log lost an external note or its following receipt"
        );
        let (recorded, broken) = recorded.unwrap();
        assert_eq!(broken, 0);
        assert_eq!(recorded.len(), 10_201);
        for index in 0..100usize {
            let external_seq = 10_002 + 2 * index as u64;
            assert_eq!(external_sequences[index], external_seq);
            assert_eq!(observed_sequences[index], external_seq + 1);
            let note = &recorded[10_001 + 2 * index];
            assert_eq!(note.seq, external_seq);
            assert!(
                matches!(&note.payload, Body::NodeNoted { node, note } if node == &producer && note == &format!("External selected work {index}"))
            );
            let saved = &recorded[10_002 + 2 * index];
            assert_eq!(saved.seq, external_seq + 1);
            assert!(
                matches!(&saved.payload, Body::ArtifactCapturedV2 { revision, producer: node, .. } if revision == &format!("{:026x}", 50_000 + index as u64) && node == &producer)
            );
        }
        assert!(
            peak <= 2 * 1024 * 1024,
            "resident receipt commit after an external write allocated {peak} auxiliary bytes"
        );
        assert!(
            durations[98] < std::time::Duration::from_millis(5),
            "resident receipt commit after an external write exceeded its strict five millisecond p99 budget"
        );
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure complete receipt commits."]
    fn measure_one_hundred_complete_bundle_receipt_commits_with_ten_thousand_prior_artifacts() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "bundle receipt measurements require release mode"
        );
        let root = standalone_root("bundle-commit-benchmark");
        let mut store = Store::create(&root).unwrap();
        let producer = format!("{:026x}", 1);
        let receipt = |artifact: u64, revision: u64, operation: u64| {
            let revision = format!("{revision:026x}");
            Body::ArtifactBundleCapturedV2 {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{artifact:026x}"),
                revision: revision.clone(),
                operation: format!("{operation:026x}"),
                producer: producer.clone(),
                previous: None,
                source: "app".into(),
                selection: "selection.json".into(),
                site: format!("{:026x}", 2),
                size: 2 * 1024 * 1024,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.bundle"),
                entrypoint: "index.html".into(),
                member_count: 64,
                manifest_bytes: 32 * 1024,
            }
        };
        let mut bodies = Vec::with_capacity(10_001);
        bodies.push(Body::NodeCreated {
            node: producer.clone(),
            num: 1,
            kind: crate::event::Kind::Task,
            title: "Produce selected deliveries".into(),
            why: String::new(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        for index in 0..10_000u64 {
            bodies.push(receipt(10_000 + index, 20_000 + index, 30_000 + index));
        }
        for body in bodies.iter().skip(1) {
            assert!(
                matches!(body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
        }
        let lock = store.lock_for_write().unwrap();
        store
            .append(&lock, crate::lane::MAIN, bodies, 0, false)
            .unwrap();
        drop(lock);
        let mut ctx = Ctx::load_for_capture(store, Whose::Founding).unwrap();
        let samples: Vec<_> = (0..100u64)
            .map(|index| receipt(60_000 + index, 50_000 + index, 40_000 + index))
            .collect();
        let mut durations = Vec::with_capacity(100);
        let mut peak = 0;
        for body in samples {
            assert!(
                matches!(&body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
            let ((result, duration), sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let result = commit_capture(&mut ctx, body);
                ctx.unlock();
                (result, started.elapsed())
            });
            result.unwrap();
            durations.push(duration);
            peak = peak.max(sample_peak);
        }
        durations.sort();
        eprintln!("Complete bundle receipt commit: 100 release samples, 10000 prior distinct bundle receipts, one producer; p50={:?}, p99={:?}, max={:?}, peak auxiliary={peak} bytes. Includes lock, refresh, guards, CAS, append, projection, JSON response construction and unlock. Synthetic metadata describes 64 members, a 32 KiB manifest and 1 MiB total. Fixture setup, tree loading, prepared metadata, retained byte copying and stdout serialization are excluded.", durations[49], durations[98], durations[99]);
        let (recorded, broken) = ctx.store.read_all().unwrap();
        let folded_receipts = ctx.tree.artifact_history.len();
        drop(ctx);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(broken, 0);
        assert_eq!(recorded.len(), 10_101);
        assert_eq!(folded_receipts, 10_100);
        for (index, event) in recorded.iter().enumerate() {
            assert_eq!(event.seq, index as u64 + 1);
            if index == 0 {
                assert!(
                    matches!(&event.payload, Body::NodeCreated { node, .. } if node == &producer)
                );
            } else if index <= 10_000 {
                let prior = index as u64 - 1;
                assert_eq!(
                    event.payload,
                    receipt(10_000 + prior, 20_000 + prior, 30_000 + prior)
                );
            } else {
                let sample = index as u64 - 10_001;
                assert_eq!(
                    event.payload,
                    receipt(60_000 + sample, 50_000 + sample, 40_000 + sample)
                );
            }
        }
        assert!(
            peak <= 2 * 1024 * 1024,
            "complete bundle receipt commit allocated {peak} auxiliary bytes"
        );
        assert!(
            durations[98] < std::time::Duration::from_millis(5),
            "complete bundle receipt commit exceeded its strict five millisecond p99 budget"
        );
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure complete receipt commits."]
    fn measure_one_hundred_complete_sqlite_receipt_commits_with_ten_thousand_prior_artifacts() {
        assert!(
            std::hint::black_box(!cfg!(debug_assertions)),
            "bundle receipt measurements require release mode"
        );
        let root = standalone_root("sqlite-commit-benchmark");
        let mut store = Store::create(&root).unwrap();
        let producer = format!("{:026x}", 1);
        let receipt = |artifact: u64, revision: u64, operation: u64| {
            let revision = format!("{revision:026x}");
            Body::ArtifactSqliteApplicationCaptured {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{artifact:026x}"),
                revision: revision.clone(),
                operation: format!("{operation:026x}"),
                producer: producer.clone(),
                previous: None,
                source: "app".into(),
                selection: "selection.json".into(),
                site: format!("{:026x}", 2),
                size: 1024 * 1024,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.bundle"),
                entrypoint: "index.html".into(),
                member_count: 64,
                manifest_bytes: 32 * 1024,
                database: String::from("data.sqlite"),
                profile: bundle::SQLITE_PROFILE.into(),
                runtime: runtime::ID.into(),
                runtime_sha256: runtime::SHA256.into(),
            }
        };
        let mut bodies = Vec::with_capacity(10_001);
        bodies.push(Body::NodeCreated {
            node: producer.clone(),
            num: 1,
            kind: crate::event::Kind::Task,
            title: "Produce selected deliveries".into(),
            why: String::new(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        for index in 0..10_000u64 {
            bodies.push(receipt(10_000 + index, 20_000 + index, 30_000 + index));
        }
        for body in bodies.iter().skip(1) {
            assert!(
                matches!(body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactSqliteApplicationCaptured { description, .. } if description.chars().count() == 280)
            );
        }
        let lock = store.lock_for_write().unwrap();
        store
            .append(&lock, crate::lane::MAIN, bodies, 0, false)
            .unwrap();
        drop(lock);
        let mut ctx = Ctx::load_for_capture(store, Whose::Founding).unwrap();
        let samples: Vec<_> = (0..100u64)
            .map(|index| receipt(60_000 + index, 50_000 + index, 40_000 + index))
            .collect();
        let mut durations = Vec::with_capacity(100);
        let mut peak = 0;
        for body in samples {
            assert!(
                matches!(&body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactSqliteApplicationCaptured { description, .. } if description.chars().count() == 280)
            );
            let ((result, duration), sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let result = commit_capture(&mut ctx, body);
                ctx.unlock();
                (result, started.elapsed())
            });
            result.unwrap();
            durations.push(duration);
            peak = peak.max(sample_peak);
        }
        durations.sort();
        eprintln!("Complete SQLite application receipt commit: 100 release samples, 10000 prior distinct SQLite application receipts, one producer; p50={:?}, p99={:?}, max={:?}, peak auxiliary={peak} bytes. Includes lock, refresh, guards, CAS, append, projection, JSON response construction and unlock. Synthetic metadata describes 64 members, a 32 KiB manifest and 2 MiB total. Fixture setup, tree loading, prepared metadata, retained byte copying and stdout serialization are excluded.", durations[49], durations[98], durations[99]);
        let (recorded, broken) = ctx.store.read_all().unwrap();
        let folded_receipts = ctx.tree.artifact_history.len();
        drop(ctx);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(broken, 0);
        assert_eq!(recorded.len(), 10_101);
        assert_eq!(folded_receipts, 10_100);
        for (index, event) in recorded.iter().enumerate() {
            assert_eq!(event.seq, index as u64 + 1);
            if index == 0 {
                assert!(
                    matches!(&event.payload, Body::NodeCreated { node, .. } if node == &producer)
                );
            } else if index <= 10_000 {
                let prior = index as u64 - 1;
                assert_eq!(
                    event.payload,
                    receipt(10_000 + prior, 20_000 + prior, 30_000 + prior)
                );
            } else {
                let sample = index as u64 - 10_001;
                assert_eq!(
                    event.payload,
                    receipt(60_000 + sample, 50_000 + sample, 40_000 + sample)
                );
            }
        }
        assert!(
            peak <= 2 * 1024 * 1024,
            "complete SQLite application receipt commit allocated {peak} auxiliary bytes"
        );
        assert!(
            durations[98] < std::time::Duration::from_millis(5),
            "complete SQLite application receipt commit exceeded its strict five millisecond p99 budget"
        );
    }

    #[test]
    #[ignore = "Run explicitly in release mode to measure complete receipt commits."]
    fn measure_one_hundred_complete_receipt_commits_with_ten_thousand_prior_artifacts() {
        let root = standalone_root("artifact-commit-benchmark");
        let mut store = Store::create(&root).unwrap();
        let producer = format!("{:026x}", 1);
        let mut bodies = Vec::with_capacity(10_001);
        bodies.push(Body::NodeCreated {
            node: producer.clone(),
            num: 1,
            kind: crate::event::Kind::Task,
            title: "Produce selected deliveries".into(),
            why: String::new(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        for index in 0..10_000u64 {
            let revision = format!("{:026x}", 20_000 + index);
            bodies.push(Body::ArtifactCapturedV2 {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{:026x}", 10_000 + index),
                revision: revision.clone(),
                operation: format!("{:026x}", 30_000 + index),
                producer: producer.clone(),
                previous: None,
                source: "mockup.html".into(),
                site: format!("{:026x}", 2),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.blob"),
            });
        }
        for body in bodies.iter().skip(1) {
            assert!(
                matches!(body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
        }
        let lock = store.lock_for_write().unwrap();
        store
            .append(&lock, crate::lane::MAIN, bodies, 0, false)
            .unwrap();
        drop(lock);
        let mut ctx = Ctx::load_for_capture(store, Whose::Founding).unwrap();
        eprintln!("Receipt context loading: authoritative streaming capture loader.");
        let samples: Vec<_> = (0..100u64)
            .map(|index| {
                let revision = format!("{:026x}", 50_000 + index);
                Body::ArtifactCapturedV2 {
                    description: "Selected layout revision. "
                        .repeat(12)
                        .chars()
                        .take(280)
                        .collect(),
                    artifact: format!("{:026x}", 60_000 + index),
                    revision: revision.clone(),
                    operation: format!("{:026x}", 40_000 + index),
                    producer: producer.clone(),
                    previous: None,
                    source: "mockup.html".into(),
                    site: format!("{:026x}", 2),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{revision}.blob"),
                }
            })
            .collect();
        let mut durations = Vec::with_capacity(100);
        let mut peak = 0;
        for body in samples {
            assert!(
                matches!(&body, Body::ArtifactCapturedV2 { description, .. } | Body::ArtifactBundleCapturedV2 { description, .. } if description.chars().count() == 280)
            );
            let ((result, duration), sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let result = commit_capture(&mut ctx, body);
                ctx.unlock();
                (result, started.elapsed())
            });
            result.unwrap();
            durations.push(duration);
            peak = peak.max(sample_peak);
        }
        durations.sort();
        eprintln!("Complete artifact receipt commit: 100 samples, 10000 prior distinct artifacts, short source paths; p50={:?}, p99={:?}, max={:?}, peak auxiliary={peak} bytes. Includes lock, guards, replay, real append, response and unlock; retained byte copying is excluded. Five millisecond budget passed={}. Synthetic object metadata is used.", durations[49], durations[98], durations[99], durations[98] < std::time::Duration::from_millis(5));
        assert!(
            peak <= 2 * 1024 * 1024,
            "complete receipt commit allocated {peak} auxiliary bytes"
        );
        let summarize = |label: &str, values: &mut Vec<std::time::Duration>| {
            values.sort();
            eprintln!(
                "{label}: 100 samples, p50={:?}, p99={:?}, max={:?}",
                values[49], values[98], values[99]
            );
        };
        let mut locks = Vec::with_capacity(100);
        let mut emissions = Vec::with_capacity(100);
        let mut residuals = Vec::with_capacity(100);
        let mut unlocks = Vec::with_capacity(100);
        let mut profiled = Vec::with_capacity(100);
        let mut ordinary = Vec::with_capacity(100);
        let mut profile_peak = 0;
        let mut append_phases: [Vec<std::time::Duration>; 6] =
            std::array::from_fn(|_| Vec::with_capacity(100));
        let mut emit_residuals = Vec::with_capacity(100);
        for index in 0..100u64 {
            let revision = format!("{:026x}", 70_000 + index);
            let body = Body::ArtifactCapturedV2 {
                description: "Selected layout revision. "
                    .repeat(12)
                    .chars()
                    .take(280)
                    .collect(),
                artifact: format!("{:026x}", 80_000 + index),
                revision: revision.clone(),
                operation: format!("{:026x}", 90_000 + index),
                producer: producer.clone(),
                previous: None,
                source: "mockup.html".into(),
                site: format!("{:026x}", 2),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{revision}.blob"),
            };
            let ((lock_time, emit_time, residual, unlock_time, total, phases), sample_peak) =
                crate::test_alloc::measure(|| {
                    let total_started = std::time::Instant::now();
                    ctx.lock_for_write().unwrap();
                    let lock_time = total_started.elapsed();
                    let commit_started = std::time::Instant::now();
                    let mut emit_time = std::time::Duration::ZERO;
                    let mut phases = [std::time::Duration::ZERO; 6];
                    commit_capture_with(&mut ctx, body, |ctx, body| {
                        let started = std::time::Instant::now();
                        let (result, recorded) = store::profile_append(|| ctx.emit(vec![body]));
                        phases = recorded;
                        emit_time = started.elapsed();
                        result
                    })
                    .unwrap();
                    let residual = commit_started.elapsed().saturating_sub(emit_time);
                    let unlock_started = std::time::Instant::now();
                    ctx.unlock();
                    (
                        lock_time,
                        emit_time,
                        residual,
                        unlock_started.elapsed(),
                        total_started.elapsed(),
                        phases,
                    )
                });
            profile_peak = profile_peak.max(sample_peak);
            emit_residuals.push(emit_time.saturating_sub(phases.iter().copied().sum()));
            for (values, phase) in append_phases.iter_mut().zip(phases) {
                values.push(phase);
            }
            locks.push(lock_time);
            emissions.push(emit_time);
            residuals.push(residual);
            unlocks.push(unlock_time);
            profiled.push(total);
        }
        for _ in 0..100 {
            let body = Body::NodeNoted {
                node: producer.clone(),
                note: "Record selected delivery work".into(),
            };
            let (elapsed, sample_peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                ctx.lock_for_write().unwrap();
                ctx.emit(vec![body]).unwrap();
                ctx.unlock();
                started.elapsed()
            });
            profile_peak = profile_peak.max(sample_peak);
            ordinary.push(elapsed);
        }
        for (label, values) in [
            "Append IDs and serialization",
            "Append open",
            "Append metadata",
            "Append seek and tail read",
            "Append config marker and write",
            "Append handle close",
        ]
        .into_iter()
        .zip(append_phases.iter_mut())
        {
            summarize(label, values);
        }
        summarize("Emit residual excluding measured append phases (includes Tree.apply and fingerprint, not attributed exclusively)", &mut emit_residuals);
        summarize("Receipt lock/refresh", &mut locks);
        summarize("Receipt emit", &mut emissions);
        summarize(
            "Receipt metadata and JSON response residual (includes idempotent lock check)",
            &mut residuals,
        );
        summarize("Receipt unlock", &mut unlocks);
        summarize("Profiled real receipt route total", &mut profiled);
        summarize(
            "Node note in capture context: lock/emit/unlock total",
            &mut ordinary,
        );
        eprintln!("Profile auxiliary peak={profile_peak} bytes; plain native filesystem fixture with one producer node and 10000 initial synthetic receipts plus up to 300 measured writes. Retained byte copying is excluded; clocks and callback instrumentation exist only in this ignored test.");
        assert!(profile_peak <= 2 * 1024 * 1024);
        let log = ctx.store.log();
        let mut readonly_open = Vec::with_capacity(100);
        let mut readonly_close = Vec::with_capacity(100);
        let mut append_open = Vec::with_capacity(100);
        let mut append_close = Vec::with_capacity(100);
        let mut cloned_open = Vec::with_capacity(100);
        let mut cloned_close = Vec::with_capacity(100);
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let handle = fs::File::open(&log).unwrap();
            readonly_open.push(started.elapsed());
            let started = std::time::Instant::now();
            drop(handle);
            readonly_close.push(started.elapsed());
            let started = std::time::Instant::now();
            let handle = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(&log)
                .unwrap();
            append_open.push(started.elapsed());
            let started = std::time::Instant::now();
            drop(handle);
            append_close.push(started.elapsed());
        }
        let shared = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&log)
            .unwrap();
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let handle = shared.try_clone().unwrap();
            cloned_open.push(started.elapsed());
            let started = std::time::Instant::now();
            drop(handle);
            cloned_close.push(started.elapsed());
        }
        drop(shared);
        summarize("Read-only File::open probe", &mut readonly_open);
        summarize("Read-only handle close probe", &mut readonly_close);
        summarize("Read+append OpenOptions probe", &mut append_open);
        summarize("Read+append handle close probe", &mut append_close);
        summarize(
            "Already open read+append handle try_clone probe",
            &mut cloned_open,
        );
        summarize("Cloned handle close probe", &mut cloned_close);
        eprintln!("Handle probes are diagnostics only: each operation has 100 samples, close is timed separately from open/clone, read-only/read+append probes run before a shared handle is held, and the original read+append handle for clone probes is opened outside sample timing. No bytes are written by these probes.");
        let prior = ctx.tree.artifact_history.len();
        let mut project = crate::project::Project::open(
            root.clone(),
            "Receipt benchmark".into(),
            "receipt-benchmark".into(),
            Whose::Founding,
        )
        .unwrap();
        let resident_bodies: Vec<_> = (0..100u64)
            .map(|index| {
                let revision = format!("{:026x}", 110_000 + index);
                Body::ArtifactCapturedV2 {
                    description: "Selected layout revision. "
                        .repeat(12)
                        .chars()
                        .take(280)
                        .collect(),
                    artifact: format!("{:026x}", 100_000 + index),
                    revision: revision.clone(),
                    operation: format!("{:026x}", 120_000 + index),
                    producer: producer.clone(),
                    previous: None,
                    source: "mockup.html".into(),
                    site: format!("{:026x}", 2),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{revision}.blob"),
                }
            })
            .collect();
        let mut resident_durations = Vec::with_capacity(100);
        let mut resident_peak = 0;
        for body in resident_bodies {
            let ((result, duration), peak) = crate::test_alloc::measure(|| {
                let started = std::time::Instant::now();
                let result = project.write(|ctx| commit_capture(ctx, body));
                (result, started.elapsed())
            });
            result.unwrap();
            resident_peak = resident_peak.max(peak);
            resident_durations.push(duration);
        }
        summarize(
            "Real resident Project.write receipt route",
            &mut resident_durations,
        );
        eprintln!("Resident Project.write auxiliary peak={resident_peak} bytes; {prior} prior synthetic receipts, one producer, plain fixture. Includes lock/refresh, append, response, unlock and resident log bookkeeping. Project open/loaded tree and prepared metadata bodies are outside timing; no Project handle existed during the earlier bare Ctx samples. Retained byte copying is excluded.");
        assert!(resident_peak <= 2 * 1024 * 1024);
        let probe_path = root.join("open-probe");
        fs::write(&probe_path, b"Synthetic handle probe").unwrap();
        let mut reopened_writer = Vec::with_capacity(100);
        let mut readonly_after_write = Vec::with_capacity(100);
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let mut writer = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(&probe_path)
                .unwrap();
            reopened_writer.push(started.elapsed());
            std::io::Write::write_all(&mut writer, b"x").unwrap();
            drop(writer);
        }
        let mut writer = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&probe_path)
            .unwrap();
        for _ in 0..100 {
            std::io::Write::write_all(&mut writer, b"x").unwrap();
            let started = std::time::Instant::now();
            let probe = fs::File::open(&probe_path).unwrap();
            assert!(probe.metadata().unwrap().len() > 0);
            drop(probe);
            readonly_after_write.push(started.elapsed());
        }
        drop(writer);
        summarize(
            "Synthetic file A: read+append open before each write/close",
            &mut reopened_writer,
        );
        summarize(
            "Synthetic file B: read-only open/metadata/close after each retained-writer write",
            &mut readonly_after_write,
        );
        eprintln!("Interleaved write probes use the auxiliary synthetic open-probe file only; the event log is untouched by these probes. Each pattern has 100 samples. Pattern A times open only; pattern B times the full read-only probe while the writer stays open. Diagnostic results are not a production budget claim.");
        drop(project);
        fs::remove_dir_all(root).unwrap();
    }
}
