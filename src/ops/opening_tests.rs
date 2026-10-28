use super::*;
use std::fs;
use std::io::Write;

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("vivac-opening-load-{}", id::ulid()));
        let store = Store::create(&root).unwrap();
        let mut ctx = Ctx::load_for_write(store, Whose::Founding).unwrap();
        ctx.lock_for_write().unwrap();
        ctx.emit(vec![
            created(1, "Initial work"),
            Body::Pushed { node: node_id(1) },
        ])
        .unwrap();
        ctx.unlock();
        Self(root)
    }

    fn store(&self) -> Store {
        Store::open(self.0.clone()).unwrap()
    }

    fn opening(&self) -> Ctx {
        Ctx::load_for_opening(self.store(), Whose::Founding).unwrap()
    }

    fn append(&self, bodies: Vec<Body>) {
        let mut writer = Ctx::load_for_write(self.store(), Whose::Founding).unwrap();
        writer.lock_for_write().unwrap();
        writer.emit(bodies).unwrap();
        writer.unlock();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn node_id(num: u64) -> String {
    format!("{num:026}")
}

fn created(num: u64, title: &str) -> Body {
    Body::NodeCreated {
        node: node_id(num),
        num,
        kind: Kind::Task,
        title: title.into(),
        why: "Exercise opening loading".into(),
        parent: None,
        blocks: false,
        refs: vec![],
        governs: vec![],
        arms: vec![],
        against: None,
    }
}

fn note(text: &str) -> Body {
    Body::NodeNoted {
        node: node_id(1),
        note: text.into(),
    }
}

fn snapshot(tree: &Tree) -> serde_json::Value {
    serde_json::json!({
        "seq": tree.seq,
        "next_num": tree.next_num,
        "broken": tree.broken_lines,
        "open": crate::render::open_data(tree),
        "rules": crate::render::rules_data(tree),
        "agents": tree.agent_history,
        "artifacts": tree.artifact_history,
        "nodes": tree.nodes_sorted().iter().map(|node| serde_json::json!({
            "num": node.num, "id": node.id, "title": node.title(tree),
            "note": node.note(tree), "why": node.why(tree),
            "state": node.state.word(node.kind), "refs": node.refs(tree),
            "notes_since_child": node.notes_since_child
        })).collect::<Vec<_>>()
    })
}

fn assert_fresh(ctx: &Ctx) {
    let (events, broken) = ctx.store.read_all().unwrap();
    let fresh = fold(&events, broken);
    assert_eq!(snapshot(&ctx.tree), snapshot(&fresh));
}

fn append_raw(f: &Fixture, bytes: &[u8]) {
    let lock = f.store().lock_for_write().unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(f.store().log())
        .unwrap()
        .write_all(bytes)
        .unwrap();
    drop(lock);
}

fn assert_reader_equivalence(f: &Fixture) {
    let expected = f.store().read_all();
    let actual = Ctx::load_for_opening(f.store(), Whose::Founding);
    match (expected, actual) {
        (Ok((events, broken)), Ok(ctx)) => {
            let fresh = fold(&events, broken);
            assert_eq!(snapshot(&ctx.tree), snapshot(&fresh));
        }
        (Err(Failure::NewerVivac(expected)), Err(Failure::NewerVivac(actual))) => {
            assert_eq!(actual, expected);
        }
        (expected, actual) => panic!(
            "Reader mismatch: expected={:?}, actual={:?}",
            expected.map(|(_, broken)| broken),
            actual.map(|ctx| ctx.tree.broken_lines)
        ),
    }
}

fn metadata_event() -> serde_json::Value {
    serde_json::to_value(crate::event::Event {
        seq: 3,
        id: node_id(3),
        ts: "2026-10-25T00:00:00Z".to_string(),
        actor: "opaque".to_string(),
        lane: crate::lane::MAIN.to_string(),
        payload: created(2, "Metadata variant"),
    })
    .unwrap()
}

#[test]
fn opening_metadata_preserves_escapes_unicode_and_extra_fields() {
    let escaped = format!(
        r#"{{"seq":3,"id":"event-\u00e9","ts":"2026-\u0031\u0030-25T00:00:00Z","actor":"opaque-\u00f1\tvalue","lane":"lane-\u00e9","payload":{}}}"#,
        serde_json::to_string(&created(2, "Unicode work 雪")).unwrap()
    );
    let mut extra = metadata_event();
    extra["extra"] = serde_json::json!({"nested": ["ignored", 42]});
    extra["payload"]["extra"] = serde_json::json!(["ignored"]);
    let mut unicode = metadata_event();
    for key in ["id", "actor", "lane"] {
        unicode[key] = serde_json::json!("opaque-雪");
    }
    for line in [escaped, extra.to_string(), unicode.to_string()] {
        let owned: crate::event::Event = serde_json::from_str(&line).unwrap();
        let borrowed: crate::event::Event<crate::event::BorrowedText<'_>> =
            serde_json::from_str(&line).unwrap();
        assert_eq!(
            serde_json::to_value(owned).unwrap(),
            serde_json::to_value(borrowed).unwrap()
        );
        for cached in [false, true] {
            let f = Fixture::new();
            if cached {
                crate::index::load(&f.store(), true).unwrap();
            }
            append_raw(&f, format!("\r\n{line}\r\n \t\r\n").as_bytes());
            assert_reader_equivalence(&f);
            assert_eq!(f.opening().tree.broken_lines, 0);
        }
    }
}

#[test]
fn opening_metadata_requires_every_event_field_with_its_original_type() {
    for key in ["id", "ts", "actor", "lane", "seq", "payload"] {
        for mode in ["missing", "wrong", "null"] {
            for cached in [false, true] {
                let f = Fixture::new();
                if cached {
                    crate::index::load(&f.store(), true).unwrap();
                }
                let mut value = metadata_event();
                match mode {
                    "missing" => {
                        value.as_object_mut().unwrap().remove(key);
                    }
                    "wrong" => {
                        value[key] = if key == "seq" {
                            serde_json::json!("3")
                        } else {
                            serde_json::json!(7)
                        };
                    }
                    "null" => value[key] = serde_json::Value::Null,
                    _ => unreachable!(),
                }
                assert!(
                    serde_json::from_value::<crate::event::Event>(value.clone()).is_err(),
                    "{key}/{mode}"
                );
                append_raw(&f, format!("{value}\n").as_bytes());
                assert_reader_equivalence(&f);
            }
        }
    }
}

#[test]
fn opening_load_matches_fresh_folds_with_missing_fresh_stale_and_invalid_indexes() {
    for mode in ["missing", "fresh", "stale", "invalid"] {
        let f = Fixture::new();
        if mode != "missing" {
            crate::index::load(&f.store(), true).unwrap();
            assert!(f.store().index_path().is_file());
        }
        if mode == "stale" {
            f.append(vec![note("A real indexed tail")]);
        } else if mode == "invalid" {
            fs::write(f.store().index_path(), "Invalid derived bytes").unwrap();
        }
        let before = fs::read(f.store().index_path()).ok();
        let ctx = f.opening();
        assert_fresh(&ctx);
        assert_eq!(fs::read(f.store().index_path()).ok(), before);
    }
}

#[test]
fn opening_refresh_preserves_large_tails_and_the_focus_previously_shown() {
    for bodies in [
        vec![note("A concurrent note"); 65],
        vec![note(&"Large note ".repeat(8_000))],
    ] {
        let f = Fixture::new();
        let mut ctx = f.opening();
        let shown_focus = ctx.tree.focus().map(|node| node.id.clone());
        f.append(bodies);
        ctx.lock_for_write().unwrap();
        session_started(&mut ctx, "startup", None, shown_focus.clone(), None).unwrap();
        ctx.unlock();
        assert_fresh(&ctx);
        let (events, broken) = ctx.store.read_all().unwrap();
        assert_eq!(broken, 0);
        assert!(matches!(&events.last().unwrap().payload,
            Body::SessionStarted { focus, .. } if focus == &shown_focus));
    }
}

#[test]
fn opening_load_does_not_apply_artifact_line_or_json_item_limits() {
    let f = Fixture::new();
    let mut body = created(2, &"Large title ".repeat(50_000));
    if let Body::NodeCreated { refs, .. } = &mut body {
        *refs = (0..1_100).map(|num| format!("src/item-{num}.rs")).collect();
    }
    f.append(vec![body]);
    let ctx = f.opening();
    assert_eq!(ctx.tree.broken_lines, 0);
    assert_fresh(&ctx);
    assert_eq!(ctx.tree.resolve("t2").unwrap().refs(&ctx.tree).len(), 1_100);
}

#[test]
fn opening_refresh_matches_fresh_folds_after_rewrites_truncation_and_broken_tails() {
    for mode in ["rewrite", "truncate", "broken", "partial"] {
        let f = Fixture::new();
        let mut ctx = f.opening();
        let shown_focus = ctx.tree.focus().map(|node| node.id.clone());
        let log = ctx.store.log();
        let lock = f.store().lock_for_write().unwrap();
        match mode {
            "rewrite" => {
                let bytes = fs::read_to_string(&log)
                    .unwrap()
                    .replace("Initial work", "Changed work");
                fs::write(&log, bytes).unwrap();
            }
            "truncate" => {
                let bytes = fs::read(&log).unwrap();
                let end = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
                fs::write(&log, &bytes[..end]).unwrap();
            }
            "broken" | "partial" => {
                let bytes = if mode == "broken" {
                    b"broken event\n".as_slice()
                } else {
                    b"partial event".as_slice()
                };
                fs::OpenOptions::new()
                    .append(true)
                    .open(&log)
                    .unwrap()
                    .write_all(bytes)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(lock);
        ctx.lock_for_write().unwrap();
        session_started(&mut ctx, "startup", None, shown_focus, None).unwrap();
        ctx.unlock();
        assert_fresh(&ctx);
    }
}

#[test]
fn opening_receipts_accept_unterminated_blank_lines_at_the_capture_boundary() {
    for length in [512 * 1024 - 1, 512 * 1024, 512 * 1024 + 1] {
        let f = Fixture::new();
        let lock = f.store().lock_for_write().unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(f.store().log())
            .unwrap()
            .write_all(&vec![b' '; length])
            .unwrap();
        drop(lock);
        let mut ctx = f.opening();
        assert_eq!(ctx.tree.broken_lines, 0);
        ctx.lock_for_write().unwrap();
        session_started(&mut ctx, "startup", None, Some(node_id(1)), None).unwrap();
        ctx.unlock();
        assert_fresh(&ctx);
        let (events, broken) = ctx.store.read_all().unwrap();
        assert_eq!(broken, 0);
        assert!(matches!(
            events.last().unwrap().payload,
            Body::SessionStarted { .. }
        ));
    }
}

#[test]
fn opening_receipt_refolds_a_replacement_with_an_identical_fingerprint() {
    for cached in [false, true] {
        let f = Fixture::new();
        if cached {
            crate::index::load(&f.store(), true).unwrap();
        }
        let mut ctx = f.opening();
        let log = ctx.store.log();
        let before = crate::store::fingerprint(&log);
        let stamp = fs::metadata(&log).unwrap().modified().unwrap();
        let (mut events, _) = ctx.store.read_all().unwrap();
        events[0].seq = 8;
        events[1].seq = 9;
        if let Body::NodeCreated { title, .. } = &mut events[0].payload {
            *title = "Changed work".into();
        }
        let mut replacement = Vec::new();
        for event in events {
            serde_json::to_writer(&mut replacement, &event).unwrap();
            replacement.push(b'\n');
        }
        let lock = f.store().lock_for_write().unwrap();
        crate::store::replace_whole(&log, &f.0.join("replacement"), &replacement).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(stamp))
            .unwrap();
        assert_eq!(crate::store::fingerprint(&log), before);
        drop(lock);
        ctx.lock_for_write().unwrap();
        session_started(&mut ctx, "startup", None, Some(node_id(1)), None).unwrap();
        ctx.unlock();
        assert_fresh(&ctx);
        assert_eq!(ctx.tree.seq, 10);
        assert_eq!(
            ctx.tree.resolve("t1").unwrap().title(&ctx.tree),
            "Changed work"
        );
    }
}

#[test]
fn opening_refresh_refuses_a_log_removed_after_loading_without_recreating_it() {
    for cached in [false, true] {
        let f = Fixture::new();
        if cached {
            crate::index::load(&f.store(), true).unwrap();
        }
        let mut ctx = f.opening();
        let log = ctx.store.log();
        let before = fs::read(&log).unwrap();
        let moved = f.0.join("moved-events");
        let lock = ctx.store.lock_for_write().unwrap();
        fs::rename(&log, &moved).unwrap();
        drop(lock);
        assert!(matches!(ctx.lock_for_write(), Err(Failure::Io(_))));
        assert!(ctx.lock.is_none());
        assert!(!log.exists());
        assert_eq!(fs::read(&moved).unwrap(), before);
        drop(ctx.store.lock_for_write().unwrap());
    }
}

#[test]
fn opening_refresh_refuses_newer_events_or_kinds_without_recording_a_receipt() {
    for field in ["type", "kind"] {
        let f = Fixture::new();
        let mut ctx = f.opening();
        let log = ctx.store.log();
        let (events, _) = ctx.store.read_all().unwrap();
        let mut newer = serde_json::to_value(&events[0]).unwrap();
        newer["seq"] = serde_json::json!(3);
        newer["payload"][field] = serde_json::json!("from-a-newer-version");
        let lock = ctx.store.lock_for_write().unwrap();
        let mut writer = fs::OpenOptions::new().append(true).open(&log).unwrap();
        serde_json::to_writer(&mut writer, &newer).unwrap();
        writer.write_all(b"\n").unwrap();
        drop(writer);
        drop(lock);
        let before = fs::read(&log).unwrap();
        assert!(matches!(ctx.lock_for_write(), Err(Failure::NewerVivac(_))));
        assert!(ctx.lock.is_none());
        assert_eq!(fs::read(&log).unwrap(), before);
        drop(ctx.store.lock_for_write().unwrap());
    }
}
