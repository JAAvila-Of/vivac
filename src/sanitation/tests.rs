use super::*;
use std::fs;
use std::path::Path;

const SOURCE: &str = include_str!("../../tests/data/incident/source.jsonl");
const NODE: &str = "01J00000000000000000000032";

struct Fixture {
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("vivac-sanitation-{}", crate::id::ulid()));
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join("source")).unwrap();
        fs::create_dir(directory.join("source/.vivac")).unwrap();
        fs::write(directory.join("source/.vivac/events"), SOURCE).unwrap();
        fs::write(
            directory.join("source/.vivac/config"),
            br#"{"version":1,"project_id":"01J00000000000000000000041","actor":"a_fixture"}"#,
        )
        .unwrap();
        fs::write(directory.join("source/.vivac/lock"), b"").unwrap();
        Self { directory }
    }
    fn source(&self) -> PathBuf {
        self.directory.join("source")
    }
    fn request(&self) -> Request {
        Request {
            source_root: self.source(),
            lane_roots: BTreeMap::new(),
            selection: [Role::Title, Role::Notes, Role::Outcome]
                .into_iter()
                .map(|role| Selector {
                    node: NODE.into(),
                    role,
                })
                .collect(),
            destination: self.directory.join("successor"),
        }
    }
    fn log(&self, text: &str) {
        fs::write(self.source().join(".vivac/events"), text).unwrap();
    }
    fn capture(&self) -> Result<CapturedPlan> {
        CapturedPlan::capture(self.request())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
    fn visit(path: &Path, files: &mut BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.insert(
                    path.clone(),
                    (
                        fs::read(&path).unwrap(),
                        fs::metadata(&path).unwrap().modified().unwrap(),
                    ),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, &mut files);
    files
}

#[test]
fn fixed_oracle_selects_exact_history_without_writes() {
    let fixture = Fixture::new();
    fs::write(fixture.source().join(".vivac/index"), b"old derived bytes").unwrap();
    let before = snapshot(&fixture.source());
    let plan = fixture.capture().unwrap();
    let case: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/data/incident/case.json")).unwrap();
    let actual: Vec<_> = plan
        .envelope
        .locations
        .iter()
        .map(|location| serde_json::json!({"seq":location.seq,"pointer":location.pointer}))
        .collect();
    let mut expected = case["changes"].as_array().unwrap().clone();
    let mut actual = actual;
    expected.sort_by_key(|v| (v["seq"].as_u64(), v["pointer"].as_str().unwrap().to_owned()));
    actual.sort_by_key(|v| (v["seq"].as_u64(), v["pointer"].as_str().unwrap().to_owned()));
    assert_eq!(actual, expected);
    plan.verify_unchanged().unwrap();
    let encoded = plan.encode_private().unwrap();
    let decoded = CapturedPlan::decode_private(&encoded).unwrap();
    assert_eq!(encoded, decoded.encode_private().unwrap());
    assert_eq!(plan.plan_digest().unwrap(), decoded.plan_digest().unwrap());
    plan.confirm(&plan.plan_digest().unwrap()).unwrap();
    assert!(plan.confirm(&plan.envelope.operation_id).is_err());
    assert_ne!(plan.envelope.successor_tree, plan.envelope.source_tree);
    assert_ne!(
        plan.envelope.successor_project,
        plan.envelope.source_project
    );
    assert_ne!(
        plan.envelope.successor_tree,
        plan.envelope.successor_project
    );
    assert!(!plan.envelope.share);
    let summary = serde_json::to_string(&plan.summary().unwrap()).unwrap();
    assert!(!summary.contains("Fixture title"));
    assert!(!summary.contains("source_root"));
    for file in &plan.envelope.files {
        assert!(!summary.contains(&file.sha256));
    }
    assert_eq!(snapshot(&fixture.source()), before);
    assert!(!fixture.directory.join("successor").exists());
}

#[test]
fn why_selection_is_independent_and_requires_occurrences() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.selection = vec![Selector {
        node: NODE.into(),
        role: Role::Why,
    }];
    let plan = CapturedPlan::capture(request).unwrap();
    assert_eq!(plan.envelope.locations.len(), 1);
    assert_eq!(plan.envelope.locations[0].pointer, "/payload/why");
    for node in ["t2", "01j00000000000000000000032", "absent"] {
        let mut request = fixture.request();
        request.selection[0].node = node.into();
        assert!(CapturedPlan::capture(request).is_err());
    }
    let mut request = fixture.request();
    request.selection.push(request.selection[0].clone());
    assert!(CapturedPlan::capture(request).is_err());
}

#[test]
fn unknown_fields_variants_duplicates_and_invalid_types_refuse() {
    let fixture = Fixture::new();
    for changed in [
        SOURCE.replacen("\"seq\":1", "\"extra\":false,\"seq\":1", 1),
        SOURCE.replacen(
            "\"type\":\"lane.declared\"",
            "\"extra\":false,\"type\":\"lane.declared\"",
            1,
        ),
        SOURCE.replacen("\"kind\":\"null\"", "\"extra\":false,\"kind\":\"null\"", 1),
        SOURCE.replacen("lane.declared", "lane.future", 1),
        SOURCE.replacen("\"seq\":1", "\"seq\":1,\"seq\":1", 1),
        SOURCE.replacen(
            "\"title\":\"Fixture title selected for removal\"",
            "\"title\":19",
            1,
        ),
        SOURCE.replacen("\"seq\":2", "\"seq\":1", 1),
        SOURCE.replacen(
            "01J00000000000000000000002",
            "01J00000000000000000000001",
            1,
        ),
        SOURCE.trim_end().to_owned(),
        SOURCE.replacen("\"seq\":1", "\"seq\":0", 1),
    ] {
        assert_ne!(changed, SOURCE);
        fixture.log(&changed);
        assert!(fixture.capture().is_err());
    }
}

#[test]
fn selected_privacy_scope_never_expands_by_text_equality() {
    let fixture = Fixture::new();
    let secret = format!("ghp_{}", "k".repeat(32));
    let selected = SOURCE.replace("Fixture title selected for removal", &secret);
    fixture.log(&selected);
    let plan = fixture.capture().unwrap();
    assert!(!String::from_utf8(plan.encode_private().unwrap())
        .unwrap()
        .contains(&secret));
    assert!(!serde_json::to_string(&plan.summary().unwrap())
        .unwrap()
        .contains(&secret));
    for text in [
        selected.replace("Keep this reason unchanged", &secret),
        selected.replace("Keep the fixture graph stable", &secret),
        selected.replace("Fixture checkpoint", &secret),
        selected.replacen("\"name\":", &format!("\"{secret}\":false,\"name\":"), 1),
    ] {
        fixture.log(&text);
        let error = fixture.capture().err().unwrap();
        assert!(!error.message().contains(&secret));
        assert_eq!(error.message(), refused().message());
    }
}

#[test]
fn unresolved_historical_alias_refuses() {
    let fixture = Fixture::new();
    for text in [
        SOURCE.replacen("\"t2\"", "\"g2\"", 1),
        SOURCE.replacen("\"g2\"", "\"t2\"", 1),
    ] {
        fixture.log(&text);
        assert!(fixture.capture().is_err());
    }
}

#[test]
fn changes_same_length_and_timestamp_missing_and_replaced_refuse() {
    let fixture = Fixture::new();
    let plan = fixture.capture().unwrap();
    let path = fixture.source().join(".vivac/events");
    let metadata = fs::metadata(&path).unwrap();
    fixture.log(&SOURCE.replace("First fixture note", "Other fixture note"));
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), metadata.len());
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
    assert!(plan.verify_unchanged().is_err());
    fixture.log(SOURCE);
    plan.verify_unchanged().unwrap();
    fs::rename(&path, fixture.source().join(".vivac/old-events")).unwrap();
    assert!(plan.verify_unchanged().is_err());
    fixture.log(SOURCE);
    assert!(plan.verify_unchanged().is_err());
}

#[test]
fn missing_lock_is_not_created_and_writer_excludes_capture() {
    let fixture = Fixture::new();
    let path = fixture.source().join(".vivac/lock");
    fs::remove_file(&path).unwrap();
    assert!(fixture.capture().is_err());
    assert!(!path.exists());
    fs::write(&path, b"").unwrap();
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    writer.lock().unwrap();
    assert!(fixture.capture().is_err());
    drop(writer);
    let plan = fixture.capture().unwrap();
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    writer.try_lock().unwrap();
    assert!(plan.verify_unchanged().is_err());
}

#[test]
fn destination_and_metadata_refusals_leave_source_intact() {
    let fixture = Fixture::new();
    for destination in [
        fixture.source(),
        fixture.source().join("child"),
        fixture.directory.clone(),
        fixture.directory.join("missing/child"),
    ] {
        let mut request = fixture.request();
        request.destination = destination;
        assert!(CapturedPlan::capture(request).is_err());
    }
    let before = snapshot(&fixture.source());
    fs::create_dir(fixture.directory.join(".git")).unwrap();
    assert!(fixture.capture().is_err());
    fs::remove_dir(fixture.directory.join(".git")).unwrap();
    fs::create_dir(fixture.source().join(".git")).unwrap();
    assert!(fixture.capture().is_ok());
    assert_eq!(before, snapshot(&fixture.source()));
    let mut request = fixture.request();
    request
        .lane_roots
        .insert("main".into(), fixture.directory.clone());
    assert!(CapturedPlan::capture(request).is_err());
}

#[test]
fn plan_decode_bounds_closed_fields_and_digest_bindings() {
    let fixture = Fixture::new();
    let mut plan = fixture.capture().unwrap();
    let digest = plan.plan_digest().unwrap();
    plan.envelope.destination = fixture.directory.join("another");
    assert!(plan.confirm(&digest).is_err());
    plan.envelope.files[0].sha256 = "b".repeat(64);
    assert!(plan.confirm(&digest).is_err());
    let bytes = fixture.capture().unwrap().encode_private().unwrap();
    for changed in [
        b" ".repeat(512 * 1024 + 1),
        [b"{\"extra\":false,".as_slice(), &bytes[1..]].concat(),
        [b"{\"version\":1,".as_slice(), &bytes[1..]].concat(),
    ] {
        assert!(CapturedPlan::decode_private(&changed).is_err());
    }
}

#[test]
fn metadata_and_event_limits_apply_before_decoding() {
    let fixture = Fixture::new();
    fixture.log(&format!("{}\n", "x".repeat(1024 * 1024)));
    assert!(fixture.capture().is_err());
    fixture.log(SOURCE);
    let config = fixture.source().join(".vivac/config");
    fs::write(&config, b" ".repeat(64 * 1024 + 1)).unwrap();
    assert!(fixture.capture().is_err());
}

fn append(fixture: &Fixture, bodies: Vec<serde_json::Value>) {
    let mut text = SOURCE.to_owned();
    for (index, payload) in bodies.into_iter().enumerate() {
        let seq = index + 14;
        let lane = if payload["type"] == "lane.declared" {
            payload["lane"].as_str().unwrap_or("main")
        } else {
            "main"
        };
        text.push_str(&serde_json::json!({"seq":seq,"id":format!("fixture-event-{seq}"),"ts":"2026-01-01T00:01:00Z","actor":"a_fixture","lane":lane,"payload":payload}).to_string());
        text.push('\n');
    }
    fixture.log(&text);
}

#[test]
fn authorized_sensitive_artifact_policy_is_preserved_and_required() {
    use crate::artifacts::policy::{CapturePolicy, Coverage, Inspection, Sensitivity};
    let fixture = Fixture::new();
    let secret = format!("ghp_{}", "k".repeat(32));
    let payload = format!("<!doctype html><html><body>{secret}</body></html>");
    let revision = crate::id::ulid();
    let site = crate::id::ulid();
    let object = format!("objects/{revision}.blob");
    fs::create_dir_all(fixture.source().join(".vivac-artifacts/objects")).unwrap();
    fs::write(fixture.source().join(".vivac-artifacts/.site"), &site).unwrap();
    let path = fixture.source().join(format!(".vivac-artifacts/{object}"));
    fs::write(&path, &payload).unwrap();
    let inspection = Inspection {
        policy: CapturePolicy::AllowInsecure,
        sensitivity: Sensitivity::Detected,
        coverage: Coverage::Complete,
        review_required: true,
    };
    let producer = crate::id::ulid();
    let creation = serde_json::json!({"type":"node.created","node":producer,"num":3,"kind":"task","title":"Retain a synthetic artifact"});
    let body = serde_json::json!({"type":"artifact.captured_v3","artifact":crate::id::ulid(),"revision":revision,"operation":crate::id::ulid(),"producer":producer,"previous":null,"source":"delivery.html","description":"Selected synthetic delivery","site":site,"size":payload.len(),"digest":digest(payload.as_bytes()),"object":object,"inspection":inspection});
    append(&fixture, vec![creation.clone(), body.clone()]);
    let before = snapshot(&fixture.source());
    let plan = fixture.capture().unwrap();
    assert!(plan
        .envelope
        .files
        .iter()
        .any(|file| file.path == format!(".vivac-artifacts/{object}")));
    assert!(!String::from_utf8(plan.encode_private().unwrap())
        .unwrap()
        .contains(&secret));
    assert!(!serde_json::to_string(&plan.summary().unwrap())
        .unwrap()
        .contains(&secret));
    assert_eq!(before, snapshot(&fixture.source()));
    fs::write(&path, payload.replace("html", "HTML")).unwrap();
    assert!(plan.verify_unchanged().is_err());
    assert!(fixture.capture().is_err());
    fs::remove_file(&path).unwrap();
    assert!(fixture.capture().is_err());
    fs::write(&path, &payload).unwrap();
    let mut strict = body;
    strict["type"] = "artifact.captured_v2".into();
    strict.as_object_mut().unwrap().remove("inspection");
    append(&fixture, vec![creation, strict]);
    assert!(fixture.capture().is_err());
}

#[test]
fn complete_managed_agent_is_checked_without_native_preparation() {
    let fixture = Fixture::new();
    let path = ".codex/agents/reviewer.toml";
    let prompt = "Review synthetic changes.";
    let native = format!("name = \"reviewer\"\ndescription = \"Review work\"\nmodel = \"gpt-6.1-sol\"\nmodel_reasoning_effort = \"medium\"\ndeveloper_instructions = \"{prompt}\"\n");
    fs::create_dir_all(fixture.source().join(".codex/agents")).unwrap();
    fs::write(fixture.source().join(path), &native).unwrap();
    let revision = "fixture-agent-decision";
    let agent = "fixture-agent";
    let creation = serde_json::json!({"type":"node.created","node":revision,"num":3,"kind":"decision","title":"Record reviewer","why":"Review work"});
    let definition = serde_json::json!({"schema_version":1,"name":"reviewer","contract":{"purpose":"Review work","duties":["Inspect changes"],"limits":["Report findings"],"acceptance":["Evidence supplied"]},"assignments":[{"harness":"codex","name":"reviewer","model":"gpt-6.1-sol","effort":"medium","settings":{}}],"retired":false});
    let recorded = serde_json::json!({"type":"agent.recorded","agent":agent,"node":revision,"definition":definition});
    let bound = serde_json::json!({"type":"agent.bound","agent":agent,"harness":"codex","path":path,"baseline":digest(native.as_bytes())});
    append(
        &fixture,
        vec![creation.clone(), recorded.clone(), bound.clone()],
    );
    let before = snapshot(&fixture.source());
    let plan = fixture.capture().unwrap();
    assert!(plan.envelope.files.iter().any(|file| file.path == path));
    plan.verify_unchanged().unwrap();
    assert_eq!(before, snapshot(&fixture.source()));
    fs::write(
        fixture.source().join(path),
        native.replace("Review synthetic", "Change synthetic"),
    )
    .unwrap();
    assert!(fixture.capture().is_err());
    assert!(plan.verify_unchanged().is_err());
    fs::write(
        fixture.source().join(path),
        native.replace(prompt, &format!("ghp_{}", "k".repeat(32))),
    )
    .unwrap();
    assert!(fixture.capture().is_err());
    fs::remove_file(fixture.source().join(path)).unwrap();
    assert!(fixture.capture().is_err());
    fs::write(fixture.source().join(path), &native).unwrap();
    for key in ["extra", "contract"] {
        let mut invalid = recorded.clone();
        invalid["definition"][key] = serde_json::json!({"unknown":false});
        append(&fixture, vec![creation.clone(), invalid, bound.clone()]);
        assert!(fixture.capture().is_err());
    }
}

#[test]
fn lane_markers_and_required_roots_are_bound_to_discovery_identity() {
    let fixture = Fixture::new();
    let path = fixture.source().join(".vivac/lane");
    for marker in [
        serde_json::json!({"version":1,"id":"main","project":"01J00000000000000000000041"}),
        serde_json::json!({"version":1,"id":"main","project":"01J00000000000000000000001","extra":false}),
        serde_json::json!({"version":2,"id":"main","project":"01J00000000000000000000001"}),
    ] {
        fs::write(&path, marker.to_string()).unwrap();
        assert!(fixture.capture().is_err());
    }
    fs::write(
        &path,
        serde_json::json!({"version":1,"id":"main","project":"01J00000000000000000000001"})
            .to_string(),
    )
    .unwrap();
    let plan = fixture.capture().unwrap();
    fs::remove_file(&path).unwrap();
    assert!(plan.verify_unchanged().is_err());
    append(
        &fixture,
        vec![serde_json::json!({"type":"lane.claimed","lane":"main"})],
    );
    assert!(fixture.capture().is_err());
    fixture.log(SOURCE);
    let plan = fixture.capture().unwrap();
    fs::write(&path, b"{}").unwrap();
    assert!(plan.verify_unchanged().is_err());
}

#[test]
fn oversized_files_and_changed_roots_are_refused() {
    let fixture = Fixture::new();
    let plan = fixture.capture().unwrap();
    let encoded = plan.encode_private().unwrap();
    drop(plan);
    let old = fixture.directory.join("old-source");
    fs::rename(fixture.source(), &old).unwrap();
    assert!(CapturedPlan::decode_private(&encoded).is_err());
    fs::create_dir(fixture.source()).unwrap();
    assert!(CapturedPlan::decode_private(&encoded).is_err());
    fs::remove_dir(fixture.source()).unwrap();
    fs::rename(old, fixture.source()).unwrap();
    let log = fs::OpenOptions::new()
        .write(true)
        .open(fixture.source().join(".vivac/events"))
        .unwrap();
    log.set_len(crate::backup::MAX_FILE + 1).unwrap();
    assert!(fixture.capture().is_err());
}

#[test]
fn nested_event_extensions_and_keys_are_never_ignored() {
    let fixture = Fixture::new();
    let cases = [
        serde_json::json!({"type":"node.created","node":"third-node","num":3,"kind":"rule","title":"Rule","arms":[{"dir":".","command":"check","extra":false}]}),
        serde_json::json!({"type":"node.created","node":"third-node","num":3,"kind":"decision","title":"Decision","against":[{"node":NODE,"why":"Reason","extra":false}]}),
        serde_json::json!({"type":"against.added","node":NODE,"against":[{"node":NODE,"why":"Reason","extra":false}]}),
        serde_json::json!({"type":"lane.declared","lane":"main","name":"Lane","repos":[{"path":".","extra":false}]}),
        serde_json::json!({"type":"where.changed","repos":[{"path":".","extra":false}]}),
        serde_json::json!({"type":"vivac.created","vivac":"third-stop","num":3,"kind":"manual","stack":[],"working_set":[],"next_intent":"Continue","anchor":{"kind":"null","id":""},"anchors":[{"path":".","sha":"abc","extra":false}]}),
    ];
    for body in cases {
        append(&fixture, vec![body]);
        assert!(fixture.capture().is_err());
    }
}

#[test]
fn full_log_requires_complete_nonzero_records_but_allows_sequence_gaps() {
    let fixture = Fixture::new();
    for text in [
        format!("{SOURCE}\n"),
        format!("{SOURCE}\0\n"),
        format!("{SOURCE}{{\"seq\":14}}\n"),
    ] {
        fixture.log(&text);
        assert!(fixture.capture().is_err());
    }
    let text: String = SOURCE
        .lines()
        .map(|line| {
            let mut event: serde_json::Value = serde_json::from_str(line).unwrap();
            event["seq"] = serde_json::json!(event["seq"].as_u64().unwrap() * 2);
            event.to_string() + "\n"
        })
        .collect();
    fixture.log(&text);
    assert_eq!(fixture.capture().unwrap().envelope.locations.len(), 7);
}

#[test]
fn metadata_missing_corrupt_and_private_plan_selection_changes_refuse() {
    let fixture = Fixture::new();
    let original = fs::read(fixture.source().join(".vivac/config")).unwrap();
    for bytes in [
        b"{}".to_vec(),
        b"{\"version\":1,\"version\":1}".to_vec(),
        b"{\"version\":999,\"project_id\":\"fixture\",\"actor\":\"opaque\"}".to_vec(),
    ] {
        fs::write(fixture.source().join(".vivac/config"), bytes).unwrap();
        assert!(fixture.capture().is_err());
    }
    fs::remove_file(fixture.source().join(".vivac/config")).unwrap();
    assert!(fixture.capture().is_err());
    fs::write(fixture.source().join(".vivac/config"), original).unwrap();
    let plan = fixture.capture().unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&plan.encode_private().unwrap()).unwrap();
    value["selection"][0]["role"] = "why".into();
    assert!(CapturedPlan::decode_private(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::from_slice(&plan.encode_private().unwrap()).unwrap();
    value["files"].as_array_mut().unwrap().pop();
    assert!(CapturedPlan::decode_private(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn explicit_linked_lane_markers_are_complete_and_rechecked() {
    let fixture = Fixture::new();
    let lane = crate::id::ulid();
    append(
        &fixture,
        vec![
            serde_json::json!({"type":"lane.declared","lane":lane,"name":"Linked fixture","repos":[]}),
        ],
    );
    let linked = fixture.directory.join("linked");
    fs::create_dir(&linked).unwrap();
    fs::create_dir(linked.join(".vivac")).unwrap();
    let marker = linked.join(".vivac/lane");
    let mut request = fixture.request();
    request.lane_roots.insert(lane.clone(), linked.clone());
    assert!(CapturedPlan::capture(request).is_err());
    for value in [
        serde_json::json!({}),
        serde_json::json!({"version":1,"id":lane,"project":"other-tree"}),
        serde_json::json!({"version":1,"id":"main","project":"01J00000000000000000000001"}),
    ] {
        fs::write(&marker, value.to_string()).unwrap();
        let mut request = fixture.request();
        request.lane_roots.insert(lane.clone(), linked.clone());
        assert!(CapturedPlan::capture(request).is_err());
    }
    fs::write(
        &marker,
        serde_json::json!({"version":1,"id":lane,"project":"01J00000000000000000000001"})
            .to_string(),
    )
    .unwrap();
    let mut request = fixture.request();
    request.lane_roots.insert(lane.clone(), linked.clone());
    let plan = CapturedPlan::capture(request).unwrap();
    assert_eq!(plan.envelope.roots.len(), 2);
    assert!(plan
        .envelope
        .files
        .iter()
        .any(|file| file.root == lane && file.path == ".vivac/lane"));
    fs::write(&marker, b"{}").unwrap();
    assert!(plan.verify_unchanged().is_err());
}

#[test]
fn required_source_files_must_have_single_links() {
    let fixture = Fixture::new();
    let extra = fixture.directory.join("linked-events");
    fs::hard_link(fixture.source().join(".vivac/events"), &extra).unwrap();
    assert!(fixture.capture().is_err());
    fs::remove_file(extra).unwrap();
    fixture.capture().unwrap().verify_unchanged().unwrap();
}
