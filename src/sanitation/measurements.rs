use super::{CapturedPlan, Location, Request, Role, Selector};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const NODE: &str = "measurement-node-000000001";
const LINE_LIMIT: usize = 1024 * 1024;
const PROSE: &str = "Synthetic measurement prose with ordinary words. ";

#[derive(Clone, Copy)]
enum Case {
    Nodes,
    SelectedVolume,
    RetainedVolume,
    LogOver,
    LineAt,
    LineOver,
    EnvelopeAt,
    EnvelopeOver,
}

impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Nodes => "distinct_nodes_10000",
            Self::SelectedVolume => "selected_notes_log_limit",
            Self::RetainedVolume => "retained_notes_log_limit",
            Self::LogOver => "log_limit_plus_one",
            Self::LineAt => "event_line_at_limit",
            Self::LineOver => "event_line_limit_plus_one",
            Self::EnvelopeAt => "envelope_near_limit",
            Self::EnvelopeOver => "envelope_over_limit",
        }
    }

    fn accepted(self) -> bool {
        !matches!(self, Self::LogOver | Self::LineOver | Self::EnvelopeOver)
    }
}

struct Fixture {
    directory: PathBuf,
    log_bytes: u64,
    events: u64,
    nodes: u64,
    notes: u64,
    role: Role,
    predicted_envelope_bytes: Option<usize>,
}

fn event(seq: u64, payload: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "seq": seq,
        "id": format!("measurement-event-{seq:09}"),
        "ts": "2026-01-01T00:00:00Z",
        "actor": "measurement-actor",
        "lane": "main",
        "payload": payload,
    }))
    .expect("Synthetic event encoding failed.");
    bytes.push(b'\n');
    bytes
}

fn note(seq: u64, encoded_bytes: Option<usize>) -> Vec<u8> {
    let overhead = event(seq, json!({"type":"node.noted", "node":NODE, "note":""})).len();
    let length = encoded_bytes.map_or(PROSE.len(), |size| {
        size.checked_sub(overhead)
            .expect("Requested synthetic line is too short.")
    });
    let mut prose = PROSE.repeat(length.div_ceil(PROSE.len()));
    prose.truncate(length);
    event(seq, json!({"type":"node.noted", "node":NODE, "note":prose}))
}

fn prefix(writer: &mut impl Write, nodes: u64) -> u64 {
    let declaration = event(
        1,
        json!({"type":"lane.declared", "lane":"main", "name":"Synthetic measurement", "repos":[]}),
    );
    writer.write_all(&declaration).unwrap();
    let mut bytes = declaration.len() as u64;
    for number in 1..=nodes {
        let line = event(
            number + 1,
            json!({"type":"node.created", "node":format!("measurement-node-{number:09}"),
                "num":number, "kind":"task", "title":"Synthetic measurement node"}),
        );
        writer.write_all(&line).unwrap();
        bytes += line.len() as u64;
    }
    bytes
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "vivac-sanitation-measurements-{}",
            crate::id::ulid()
        ));
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join("source")).unwrap();
        fs::create_dir(directory.join("source/.vivac")).unwrap();
        fs::write(
            directory.join("source/.vivac/config"),
            br#"{"version":1,"project_id":"measurement-project","actor":"measurement-actor","share":false}"#,
        )
        .unwrap();
        fs::write(directory.join("source/.vivac/lock"), b"").unwrap();
        Self {
            directory,
            log_bytes: 0,
            events: 0,
            nodes: 0,
            notes: 0,
            role: Role::Title,
            predicted_envelope_bytes: None,
        }
    }

    fn source(&self) -> PathBuf {
        self.directory.join("source")
    }

    fn request(&self) -> Request {
        Request {
            source_root: self.source(),
            lane_roots: BTreeMap::new(),
            selection: vec![Selector {
                node: NODE.into(),
                role: self.role,
            }],
            destination: self.directory.join("successor"),
        }
    }

    fn write(&mut self, nodes: u64, notes: u64, line_bytes: Option<usize>) {
        let file = File::create(self.source().join(".vivac/events")).unwrap();
        let mut writer = BufWriter::new(file);
        let mut bytes = prefix(&mut writer, nodes);
        for index in 0..notes {
            let line = note(nodes + index + 2, line_bytes);
            writer.write_all(&line).unwrap();
            bytes += line.len() as u64;
        }
        writer.flush().unwrap();
        self.log_bytes = bytes;
        self.events = 1 + nodes + notes;
        self.nodes = nodes;
        self.notes = notes;
    }

    fn volume(&mut self, role: Role) {
        self.role = role;
        let file = File::create(self.source().join(".vivac/events")).unwrap();
        let mut writer = BufWriter::new(file);
        let mut bytes = prefix(&mut writer, 1);
        let mut notes = 0;
        while bytes < crate::backup::MAX_FILE {
            let line_bytes = (crate::backup::MAX_FILE - bytes).min(LINE_LIMIT as u64) as usize;
            let line = note(notes + 3, Some(line_bytes));
            writer.write_all(&line).unwrap();
            bytes += line.len() as u64;
            notes += 1;
        }
        writer.flush().unwrap();
        self.log_bytes = bytes;
        self.events = notes + 2;
        self.nodes = 1;
        self.notes = notes;
    }

    fn envelope(&mut self, over: bool) {
        self.role = Role::Notes;
        self.write(1, 1, None);
        let base = CapturedPlan::capture(self.request()).expect("Envelope calibration refused.");
        let template = serde_json::to_value(&base.envelope).unwrap();
        drop(base);
        let mut low = 1;
        let location_bytes = serde_json::to_vec(&location(3)).unwrap().len() + 1;
        let mut high = crate::backup::MAX_MANIFEST as usize / location_bytes + 2;
        assert!(projected_envelope(&template, high) > crate::backup::MAX_MANIFEST as usize);
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if projected_envelope(&template, middle) <= crate::backup::MAX_MANIFEST as usize {
                low = middle;
            } else {
                high = middle;
            }
        }
        let notes = if over { high } else { low };
        self.predicted_envelope_bytes = Some(projected_envelope(&template, notes));
        self.write(1, notes as u64, None);
    }

    fn prepare(case: Case) -> Self {
        let mut fixture = Self::new();
        match case {
            Case::Nodes => fixture.write(10_000, 0, None),
            Case::SelectedVolume => fixture.volume(Role::Notes),
            Case::RetainedVolume => fixture.volume(Role::Title),
            Case::LogOver => {
                fixture.write(1, 1, None);
                File::options()
                    .write(true)
                    .open(fixture.source().join(".vivac/events"))
                    .unwrap()
                    .set_len(crate::backup::MAX_FILE + 1)
                    .unwrap();
                fixture.log_bytes = crate::backup::MAX_FILE + 1;
            }
            Case::LineAt | Case::LineOver => {
                fixture.role = Role::Notes;
                let extra = usize::from(matches!(case, Case::LineOver));
                fixture.write(1, 1, Some(LINE_LIMIT + extra));
            }
            Case::EnvelopeAt => fixture.envelope(false),
            Case::EnvelopeOver => fixture.envelope(true),
        }
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("Synthetic measurement cleanup failed.");
    }
}

fn location(seq: u64) -> Location {
    Location {
        node: NODE.into(),
        role: Role::Notes,
        seq,
        pointer: "/payload/note".into(),
    }
}

fn projected_envelope(template: &Value, notes: usize) -> usize {
    let mut envelope = template.clone();
    envelope["locations"] = serde_json::to_value(
        (0..notes)
            .map(|index| location(index as u64 + 3))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut bytes = prefix(&mut std::io::sink(), 1);
    for index in 0..notes {
        bytes += note(index as u64 + 3, None).len() as u64;
    }
    for file in envelope["files"].as_array_mut().unwrap() {
        if file["path"] == ".vivac/events" {
            file["size"] = bytes.into();
        }
    }
    serde_json::to_vec(&envelope).unwrap().len()
}

#[derive(PartialEq, Eq)]
struct State {
    size: u64,
    modified: SystemTime,
    digest: [u8; 32],
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, State> {
    fn visit(path: &Path, states: &mut BTreeMap<PathBuf, State>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let mut digest = Sha256::new();
            if metadata.is_dir() {
                visit(&path, states);
            } else {
                let mut file = File::open(&path).unwrap();
                let mut buffer = [0; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    digest.update(&buffer[..count]);
                }
            }
            states.insert(
                path,
                State {
                    size: metadata.len(),
                    modified: metadata.modified().unwrap(),
                    digest: digest.finalize().into(),
                },
            );
        }
    }
    let mut states = BTreeMap::new();
    visit(root, &mut states);
    states
}

struct Observation<'a> {
    case: Case,
    series: usize,
    sample: usize,
    phase: &'a str,
    outcome: &'a str,
    elapsed: Option<Duration>,
    peak: Option<usize>,
    envelope_bytes: Option<usize>,
    locations: Option<usize>,
    members: Option<usize>,
}

fn emit(fixture: &Fixture, observation: Observation<'_>) {
    eprintln!(
        "{}",
        json!({
            "case":observation.case.name(), "series":observation.series,
            "sample":observation.sample, "phase":observation.phase,
            "log_bytes":fixture.log_bytes,
            "log_limit_gap_bytes":i128::from(crate::backup::MAX_FILE) - i128::from(fixture.log_bytes),
            "event_count":fixture.events, "node_count":fixture.nodes,
            "selection_count":1, "location_count":observation.locations,
            "member_count":observation.members, "envelope_bytes":observation.envelope_bytes,
            "predicted_envelope_bytes":fixture.predicted_envelope_bytes,
            "outcome":observation.outcome,
            "elapsed_ms":observation.elapsed.map(|duration| duration.as_secs_f64() * 1000.0),
            "peak_rust_heap_bytes":observation.peak,
            "os":std::env::consts::OS, "arch":std::env::consts::ARCH, "profile":"release",
        })
    );
}

fn run(case: Case) {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "Measurements require cargo test --release."
    );
    for series in 1..=2 {
        for sample in 1..=3 {
            let fixture = Fixture::prepare(case);
            let before = snapshot(&fixture.source());
            let request = fixture.request();
            let ((result, elapsed), peak) = crate::test_alloc::measure(|| {
                let start = Instant::now();
                let result = CapturedPlan::capture(request);
                (result, start.elapsed())
            });
            let accepted = result.is_ok();
            let mut envelope_bytes = None;
            let mut locations = None;
            let mut members = None;
            if let Ok(plan) = &result {
                envelope_bytes = Some(plan.encode_private().unwrap().len());
                locations = Some(plan.envelope.locations.len());
                members = Some(plan.envelope.files.len());
                assert_eq!(plan.envelope.selection.len(), 1);
                let expected_locations = if fixture.role == Role::Notes {
                    fixture.notes
                } else {
                    1
                };
                assert_eq!(locations, Some(expected_locations as usize));
                assert_eq!(members, Some(3));
                if let Some(predicted) = fixture.predicted_envelope_bytes {
                    assert_eq!(envelope_bytes, Some(predicted));
                }
            }
            emit(
                &fixture,
                Observation {
                    case,
                    series,
                    sample,
                    phase: "capture",
                    outcome: if accepted { "accepted" } else { "refused" },
                    elapsed: Some(elapsed),
                    peak: Some(peak),
                    envelope_bytes,
                    locations,
                    members,
                },
            );
            if let Ok(plan) = &result {
                let ((verified, elapsed), peak) = crate::test_alloc::measure(|| {
                    let start = Instant::now();
                    let result = plan.verify_unchanged();
                    (result, start.elapsed())
                });
                emit(
                    &fixture,
                    Observation {
                        case,
                        series,
                        sample,
                        phase: "verify",
                        outcome: if verified.is_ok() {
                            "accepted"
                        } else {
                            "refused"
                        },
                        elapsed: Some(elapsed),
                        peak: Some(peak),
                        envelope_bytes,
                        locations,
                        members,
                    },
                );
                assert!(
                    verified.is_ok(),
                    "Unchanged synthetic source verification refused."
                );
            } else {
                emit(
                    &fixture,
                    Observation {
                        case,
                        series,
                        sample,
                        phase: "verify",
                        outcome: "not_run",
                        elapsed: None,
                        peak: None,
                        envelope_bytes: None,
                        locations: None,
                        members: None,
                    },
                );
            }
            assert!(
                before == snapshot(&fixture.source()),
                "Measurement source changed."
            );
            assert!(
                matches!(fs::symlink_metadata(fixture.request().destination), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "Measurement destination was created."
            );
            assert_eq!(accepted, case.accepted(), "Unexpected measurement outcome.");
        }
    }
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn distinct_nodes_10000() {
    run(Case::Nodes);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn selected_notes_log_limit() {
    run(Case::SelectedVolume);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn retained_notes_log_limit() {
    run(Case::RetainedVolume);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn log_limit_plus_one() {
    run(Case::LogOver);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn event_line_at_limit() {
    run(Case::LineAt);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn event_line_limit_plus_one() {
    run(Case::LineOver);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn envelope_near_limit() {
    run(Case::EnvelopeAt);
}

#[test]
#[ignore = "Opt-in release measurement; run this case separately."]
fn envelope_over_limit() {
    run(Case::EnvelopeOver);
}

#[test]
fn synthetic_note_framing_counts_newline_and_preserves_json() {
    let minimum = note(3, Some(note(3, None).len() - PROSE.len())).len();
    for requested in [minimum, minimum + 1, 1024] {
        let bytes = note(3, Some(requested));
        assert_eq!(bytes.len(), requested);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let decoded: crate::event::Event = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.seq, 3);
        assert!(
            matches!(decoded.payload, crate::event::Body::NodeNoted { node, .. } if node == NODE)
        );
    }
}

#[test]
fn envelope_projection_matches_small_real_capture() {
    let mut fixture = Fixture::new();
    fixture.role = Role::Notes;
    fixture.write(1, 1, None);
    let base = CapturedPlan::capture(fixture.request()).unwrap();
    let template = serde_json::to_value(&base.envelope).unwrap();
    drop(base);
    let predicted = projected_envelope(&template, 3);
    fixture.write(1, 3, None);
    let captured = CapturedPlan::capture(fixture.request()).unwrap();
    assert_eq!(captured.encode_private().unwrap().len(), predicted);
    captured.verify_unchanged().unwrap();
}
