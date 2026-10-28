use super::*;
use crate::event::{Kind, State};
use crate::ops::{Ctx, Whose};
use crate::store::Store;

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("vivac-opening-{}", crate::id::ulid()));
        Store::create(&root).unwrap();
        Self(root)
    }

    fn ctx(&self) -> Ctx {
        Ctx::load_for_write(Store::open(self.0.clone()).unwrap(), Whose::Founding).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn node(num: u64, kind: Kind, title: &str) -> Body {
    Body::NodeCreated {
        node: format!("n{num}"),
        num,
        kind,
        title: title.into(),
        why: "Exercise opening consistency".into(),
        parent: None,
        blocks: false,
        refs: vec![],
        governs: vec![],
        arms: vec![],
        against: None,
    }
}

fn generation(ctx: &mut Ctx, num: u64, title: &str) {
    let mine = ctx.lock_for_write().unwrap();
    let mut bodies = vec![];
    if num > 1 {
        for old in [num - 1, num + 99] {
            bodies.push(Body::StateChanged {
                node: format!("n{old}"),
                state: State::Done,
                outcome: "Replaced by the next generation".into(),
                forced: false,
                until: None,
            });
        }
        bodies.push(Body::Popped {
            node: format!("n{}", num - 1),
        });
    }
    bodies.extend([
        node(num, Kind::Task, title),
        node(num + 100, Kind::Constraint, &format!("{title} mandate")),
        Body::Pushed {
            node: format!("n{num}"),
        },
        Body::VivacCreated {
            vivac: format!("v{num}"),
            num,
            kind: VivacKind::Manual,
            stack: vec![(format!("n{num}"), title.into())],
            working_set: vec![],
            next_intent: format!("Continue {title}"),
            anchor: Default::default(),
            anchors: vec![],
            node_ref: None,
            label: title.into(),
        },
    ]);
    ctx.emit(bodies).unwrap();
    if mine {
        ctx.unlock();
    }
}

fn report(applied: bool) -> (serde_json::Value, i32) {
    (
        serde_json::json!({"policy":{"mode":"automatic"}, "applied":applied,
        "needs_review":[], "blocked":[]}),
        0,
    )
}

fn isolated(name: &str) -> bool {
    const MARKER: &str = "VIVAC_OPENING_TEST_CHILD";
    if std::env::var(MARKER).as_deref() == Ok(name) {
        return false;
    }
    let scratch = Fixture::new();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("session::tests::{name}"),
            "--include-ignored",
            "--nocapture",
        ])
        .env(MARKER, name)
        .env("VIVAC_HOME", scratch.0.join("home"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

#[test]
fn opening_rejects_unavailable_snapshot_metadata() {
    if isolated("opening_rejects_unavailable_snapshot_metadata") {
        return;
    }
    let f = Fixture::new();
    let mut reader = f.ctx();
    generation(&mut reader, 1, "Generation alpha");
    reader.seen = (0, None);
    let result = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        panic!("Unverified context must not reconcile")
    });
    assert!(
        matches!(result, Err(Failure::Model(ref message)) if message.contains("metadata is unavailable"))
    );
    let (events, _) = reader.store.read_all().unwrap();
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, Body::SessionStarted { .. })));
}

#[test]
fn opening_custody_rejects_a_broken_snapshot() {
    if isolated("opening_custody_rejects_a_broken_snapshot") {
        return;
    }
    let f = Fixture::new();
    let mut ctx = f.ctx();
    ctx.tree.broken_lines = 1;
    let before = std::fs::read(ctx.store.log()).unwrap();
    let (value, code) = crate::agents::reconcile_for_opening(&ctx);
    assert_eq!(code, 5, "{value}");
    assert_eq!(
        value["error"],
        "Agent custody could not complete the operation; inspect tree and file access."
    );
    assert_eq!(std::fs::read(ctx.store.log()).unwrap(), before);
}

#[test]
fn opening_custody_rejects_a_different_project_or_lane() {
    if isolated("opening_custody_rejects_a_different_project_or_lane") {
        return;
    }
    let f = Fixture::new();
    let other = Fixture::new();
    let mut ctx = f.ctx();
    ctx.store = Store::open(other.0.clone()).unwrap();
    let (value, code) = crate::agents::reconcile_for_opening(&ctx);
    assert_ne!(code, 0, "{value}");
    assert!(value["error"].as_str().unwrap().contains("snapshot"));

    let mut ctx = f.ctx();
    ctx.lane = Some("another-lane".into());
    let (value, code) = crate::agents::reconcile_for_opening(&ctx);
    assert_ne!(code, 0, "{value}");
    assert!(value["error"].as_str().unwrap().contains("snapshot"));

    let mut ctx = f.ctx();
    ctx.store.config.project_id = "another-project".into();
    let (value, code) = crate::agents::reconcile_for_opening(&ctx);
    assert_ne!(code, 0, "{value}");
    assert!(value["error"].as_str().unwrap().contains("snapshot"));
}

fn automation(ctx: &mut Ctx, num: u64, enabled: bool) {
    let mine = ctx.lock_for_write().unwrap();
    let id = crate::id::ulid();
    let mut decision = node(num, Kind::Decision, "Choose agent custody policy");
    if let Body::NodeCreated { node, .. } = &mut decision {
        *node = id.clone();
    }
    ctx.emit(vec![
        decision,
        Body::AgentAutomationConfigured { node: id, enabled },
    ])
    .unwrap();
    if mine {
        ctx.unlock();
    }
}

#[test]
fn opening_custody_matches_fresh_manual_and_automatic_reports() {
    if isolated("opening_custody_matches_fresh_manual_and_automatic_reports") {
        return;
    }
    let f = Fixture::new();
    let mut ctx = f.ctx();
    for enabled in [false, true] {
        automation(&mut ctx, u64::from(enabled) + 1, enabled);
        let before = std::fs::read(ctx.store.log()).unwrap();
        let expected = crate::agents::reconcile_for_hook(&ctx.lane_dir);
        assert_eq!(
            expected.0["policy"]["mode"],
            if enabled { "automatic" } else { "manual" }
        );
        assert_eq!(crate::agents::reconcile_for_opening(&ctx), expected);
        assert_eq!(std::fs::read(ctx.store.log()).unwrap(), before);
    }
    ctx.lane = None;
    assert_eq!(
        crate::agents::reconcile_for_opening(&ctx),
        crate::agents::reconcile_for_hook(&ctx.lane_dir)
    );
}

#[test]
fn opening_custody_rechecks_revocation_after_the_snapshot() {
    if isolated("opening_custody_rechecks_revocation_after_the_snapshot") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    automation(&mut writer, 1, true);
    let reader = f.ctx();
    std::fs::create_dir_all(f.0.join(".claude/agents")).unwrap();
    std::fs::write(
        f.0.join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Inspect the scoped change.\nmodel: sonnet\neffort: high\ntools: Read, Glob, Grep\n---\nReturn file and line evidence.\n",
    )
    .unwrap();
    automation(&mut writer, 2, false);
    let before = std::fs::read(writer.store.log()).unwrap();
    let (value, _) = crate::agents::reconcile_for_opening(&reader);
    assert_eq!(value["policy"]["mode"], "manual", "{value}");
    assert_eq!(value["applied"], false, "{value}");
    assert_eq!(std::fs::read(writer.store.log()).unwrap(), before);
    assert!(!writer
        .tree
        .agent_history
        .iter()
        .any(|(_, body)| matches!(body, Body::AgentRecorded { .. })));
}

#[test]
fn opening_rebuilds_after_a_real_append_during_reconciliation() {
    if isolated("opening_rebuilds_after_a_real_append_during_reconciliation") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    generation(&mut writer, 1, "Generation alpha");
    let mut reader = f.ctx();
    let mut calls = 0;
    let prepared = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        calls += 1;
        if calls == 1 {
            generation(&mut writer, 2, "Generation beta");
        }
        report(false)
    })
    .unwrap()
    .unwrap();
    assert!(
        prepared.text.contains("Generation beta mandate"),
        "{}",
        prepared.text
    );
    assert!(
        !prepared.text.contains("Generation alpha"),
        "{}",
        prepared.text
    );
    assert_eq!(prepared.shown_focus.as_deref(), Some("n2"));
    assert_eq!(prepared.shown_vivac.as_deref(), Some("v2"));
    assert_eq!(calls, 2);
}

#[test]
fn opening_rejects_two_changing_generations_without_a_receipt() {
    if isolated("opening_rejects_two_changing_generations_without_a_receipt") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    generation(&mut writer, 1, "Generation alpha");
    let mut reader = f.ctx();
    let mut num = 1;
    let result = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        num += 1;
        generation(&mut writer, num, "Generation beta");
        report(false)
    });
    assert!(
        matches!(result, Err(Failure::Model(ref message)) if message.contains("both preparation attempts"))
    );
    let (events, broken) = reader.store.read_all().unwrap();
    assert_eq!(broken, 0);
    assert!(!events
        .iter()
        .any(|e| matches!(e.payload, Body::SessionStarted { .. })));
    assert_eq!(num, 3);
}

#[test]
fn opening_keeps_applied_custody_and_uses_the_stable_pending_report() {
    if isolated("opening_keeps_applied_custody_and_uses_the_stable_pending_report") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    generation(&mut writer, 1, "Generation alpha");
    let mut reader = f.ctx();
    let mut calls = 0;
    let prepared = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        calls += 1;
        if calls == 1 {
            generation(&mut writer, 2, "Generation beta");
            let (mut value, code) = report(true);
            value["needs_review"] = serde_json::json!([{}, {}]);
            return (value, code);
        }
        // The refresh must release its own lock before reconciliation runs again.
        assert!(writer.lock_for_write().unwrap());
        writer.unlock();
        let (mut value, code) = report(false);
        value["policy"]["mode"] = serde_json::json!("manual");
        (value, code)
    })
    .unwrap()
    .unwrap();
    assert!(prepared
        .text
        .contains("mode manual; changes applied true; needs review 0; blocked 0."));
    assert!(!reader.holds_write_lock());
}

#[test]
fn opening_receipt_keeps_the_ids_shown_before_a_later_append() {
    if isolated("opening_receipt_keeps_the_ids_shown_before_a_later_append") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    generation(&mut writer, 1, "Generation alpha");
    let mut reader = f.ctx();
    let prepared = prepare_opening(&mut reader, &Args::default(), "test", |_| report(false))
        .unwrap()
        .unwrap();
    generation(&mut writer, 2, "Generation beta");
    record_opening(
        &mut reader,
        "startup",
        None,
        prepared.shown_focus,
        prepared.shown_vivac,
    )
    .unwrap();
    assert!(prepared.text.contains("Generation alpha"));
    assert_eq!(reader.tree.focus().unwrap().id, "n2");
    let (events, _) = reader.store.read_all().unwrap();
    assert!(
        matches!(&events.last().unwrap().payload, Body::SessionStarted { focus, vivac, .. } if focus.as_deref() == Some("n1") && vivac.as_deref() == Some("v1"))
    );
}

#[test]
fn opening_preserves_a_callers_lock() {
    if isolated("opening_preserves_a_callers_lock") {
        return;
    }
    let f = Fixture::new();
    let mut reader = f.ctx();
    generation(&mut reader, 1, "Generation alpha");
    assert!(reader.lock_for_write().unwrap());
    let prepared = prepare_opening(&mut reader, &Args::default(), "test", |_| report(false))
        .unwrap()
        .unwrap();
    record_opening(
        &mut reader,
        "startup",
        None,
        prepared.shown_focus,
        prepared.shown_vivac,
    )
    .unwrap();
    assert!(reader.holds_write_lock());
    reader.unlock();
}

#[test]
fn opening_initial_overflow_never_reconciles() {
    if isolated("opening_initial_overflow_never_reconciles") {
        return;
    }
    let f = Fixture::new();
    let mut reader = f.ctx();
    generation(&mut reader, 1, &"Long mandate ".repeat(6000));
    let result = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        panic!("Oversized initial context must not reconcile")
    })
    .unwrap();
    assert!(result.is_none());
}

#[test]
fn opening_rebuilt_overflow_keeps_prior_changes_without_a_receipt() {
    if isolated("opening_rebuilt_overflow_keeps_prior_changes_without_a_receipt") {
        return;
    }
    let f = Fixture::new();
    let mut writer = f.ctx();
    generation(&mut writer, 1, "Generation alpha");
    let mut reader = f.ctx();
    let mut calls = 0;
    let result = prepare_opening(&mut reader, &Args::default(), "test", |_| {
        calls += 1;
        generation(&mut writer, 2, &"Long mandate ".repeat(6000));
        report(true)
    })
    .unwrap();
    assert!(result.is_none());
    assert_eq!(calls, 1);
    let (events, _) = reader.store.read_all().unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(&e.payload, Body::Pushed { node } if node == "n2")));
    assert!(!events
        .iter()
        .any(|e| matches!(e.payload, Body::SessionStarted { .. })));
}

#[test]
#[ignore = "Run in release mode to measure opening preparation and receipt separately"]
fn measure_opening_preparation_and_receipt() {
    require_release_mode();
    if isolated("measure_opening_preparation_and_receipt") {
        return;
    }
    use std::time::{Duration, Instant};
    fn summary(label: &str, times: &mut [Duration]) {
        times.sort();
        let rank = |percent: usize| (percent * times.len()).div_ceil(100) - 1;
        eprintln!(
            "{label}: samples={} p50_us={} p99_us={} max_us={}",
            times.len(),
            times[rank(50)].as_micros(),
            times[rank(99)].as_micros(),
            times.last().unwrap().as_micros()
        );
    }
    eprintln!("method=nearest-rank warmup=5 samples=200 startup_subtracted=false");
    for count in [100_u64, 10_000] {
        let f = Fixture::new();
        let mut writer = f.ctx();
        writer.lock_for_write().unwrap();
        let mut bodies = vec![];
        for num in 1..=count {
            bodies.push(node(num, Kind::Task, &format!("Measured work {num}")));
            if num > 100 {
                bodies.push(Body::StateChanged {
                    node: format!("n{num}"),
                    state: State::Done,
                    outcome: "Finished measured work".into(),
                    forced: false,
                    until: None,
                });
            }
        }
        bodies.push(Body::Pushed { node: "n1".into() });
        writer.emit(bodies).unwrap();
        writer.unlock();
        let mut loads = vec![];
        let mut preparations = vec![];
        let mut receipts = vec![];
        let mut bytes = 0;
        for sample in 0..205 {
            let start = Instant::now();
            let mut ctx = f.ctx();
            let loaded = start.elapsed();
            let start = Instant::now();
            let prepared = prepare_opening(
                &mut ctx,
                &Args::default(),
                "measure",
                crate::agents::reconcile_for_opening,
            )
            .unwrap()
            .unwrap();
            let preparation = start.elapsed();
            assert!(prepared.text.contains("Measured work 1"));
            assert_eq!(prepared.shown_focus.as_deref(), Some("n1"));
            bytes = prepared.text.len();
            assert!(bytes <= opening::CONTEXT_BYTES);
            // A real append makes the receipt acquire and refresh the loaded context.
            writer.lock_for_write().unwrap();
            writer
                .emit(vec![Body::NodeNoted {
                    node: "n1".into(),
                    note: "Measured concurrent append".into(),
                }])
                .unwrap();
            writer.unlock();
            let start = Instant::now();
            record_opening(
                &mut ctx,
                "startup",
                None,
                prepared.shown_focus,
                prepared.shown_vivac,
            )
            .unwrap();
            let receipt = start.elapsed();
            let (events, broken) = ctx.store.read_all().unwrap();
            assert_eq!(broken, 0);
            assert!(matches!(
                events.last().unwrap().payload,
                Body::SessionStarted { .. }
            ));
            if sample >= 5 {
                loads.push(loaded);
                preparations.push(preparation);
                receipts.push(receipt);
            }
        }
        eprintln!(
            "nodes={count} fronts=100 bytes={bytes} fixture={}",
            f.0.display()
        );
        summary("load", &mut loads);
        summary("prepare", &mut preparations);
        summary("receipt_refresh", &mut receipts);
    }
}

fn require_release_mode() {
    #[cfg(debug_assertions)]
    panic!("Run this benchmark in release mode.");
}
