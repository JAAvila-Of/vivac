use super::*;

pub(crate) fn run() {
    require_release_without_test_harness();
    let root = env_path("VIVAC_OPENING_ROOT");
    let nodes = env_num::<u64>("VIVAC_OPENING_NODES");
    let fronts = env_num::<u64>("VIVAC_OPENING_FRONTS");
    let samples = env_num::<usize>("VIVAC_OPENING_SAMPLES");
    let warmups = env_num::<usize>("VIVAC_OPENING_WARMUPS");
    let index_mode = env_str("VIVAC_OPENING_INDEX");
    let receipt_mode = env_str("VIVAC_OPENING_RECEIPT");
    let output = env_path("VIVAC_OPENING_OUTPUT");
    assert!(nodes >= fronts && fronts > 0);
    assert!(!root.exists(), "fixture root must be new: {}", root.display());
    crate::store::Store::create(&root).expect("create isolated fixture");
    seed(&root, nodes, fronts);
    if index_mode == "warm" {
        let store = crate::store::Store::open(root.clone()).expect("open fixture");
        crate::index::load(&store, true).expect("create derived index");
        assert!(store.index_path().is_file(), "warm index was not persisted");
    }

    let mut rows = Vec::with_capacity(samples);
    for iteration in 0..(warmups + samples) {
        let all = std::time::Instant::now();
        let start = std::time::Instant::now();
        let store = crate::store::Store::open(root.clone()).expect("open fixture store");
        let mut ctx = crate::ops::Ctx::load_for_opening(store, crate::ops::Whose::Founding)
            .expect("load opening context");
        let loaded = start.elapsed();
        let start = std::time::Instant::now();
        let prepared = prepare_opening(
            &mut ctx,
            &crate::args::Args::default(),
            "measure",
            crate::agents::reconcile_for_opening,
        )
        .expect("prepare opening")
        .expect("bounded synthetic context");
        let preparation = start.elapsed();
        let load_prepare = all.elapsed();
        assert!(prepared.text.contains("Measured work 1"));
        assert_eq!(prepared.shown_focus.as_deref(), Some(node_id(1).as_str()));
        assert!(prepared.text.len() <= opening::CONTEXT_BYTES);
        let shown_focus = prepared.shown_focus.clone();
        let shown_vivac = prepared.shown_vivac.clone();

        if receipt_mode == "refresh" {
            let store = crate::store::Store::open(root.clone()).expect("open concurrent writer");
            let mut writer = crate::ops::Ctx::load_for_write(store, crate::ops::Whose::Founding)
                .expect("load concurrent writer");
            let mine = writer.lock_for_write().expect("lock concurrent writer");
            writer
                .emit(vec![crate::event::Body::NodeNoted {
                    node: node_id(1),
                    note: "Synthetic append between preparation and receipt".into(),
                }])
                .expect("append concurrent note");
            if mine {
                writer.unlock();
            }
        }
        let start = std::time::Instant::now();
        record_opening(
            &mut ctx,
            "startup",
            None,
            shown_focus.clone(),
            shown_vivac.clone(),
        )
        .expect("record opening receipt");
        let receipt = start.elapsed();

        let (events, broken) = ctx.store.read_all().expect("validate fixture log");
        assert_eq!(broken, 0, "benchmark fixture has broken event lines");
        let receipt_count = events
            .iter()
            .filter(|event| matches!(event.payload, crate::event::Body::SessionStarted { .. }))
            .count();
        assert_eq!(receipt_count, iteration + 1, "each iteration records exactly one receipt");
        assert!(matches!(
            events.last().map(|event| (&event.payload, event.seq)),
            Some((crate::event::Body::SessionStarted { focus, vivac, .. }, seq))
                if seq == events.len() as u64 && focus == &shown_focus && vivac == &shown_vivac
        ));
        if iteration >= warmups {
            rows.push(serde_json::json!({
                "load_ns": loaded.as_nanos(),
                "prepare_ns": preparation.as_nanos(),
                "load_prepare_ns": load_prepare.as_nanos(),
                "receipt_ns": receipt.as_nanos(),
                "context_bytes": prepared.text.len()
            }));
        }
    }
    let report = serde_json::json!({
        "index": index_mode,
        "receipt": receipt_mode,
        "nodes": nodes,
        "fronts": fronts,
        "samples": rows,
        "summary": {
            "load_ns": percentiles(&rows, "load_ns"),
            "prepare_ns": percentiles(&rows, "prepare_ns"),
            "load_prepare_ns": percentiles(&rows, "load_prepare_ns"),
            "receipt_ns": percentiles(&rows, "receipt_ns")
        }
    });
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).expect("write raw samples");
}

#[cfg(any(test, debug_assertions))]
fn require_release_without_test_harness() {
    panic!("opening phase measurement requires release and cfg(test)=false");
}

#[cfg(not(any(test, debug_assertions)))]
fn require_release_without_test_harness() {}

fn seed(root: &std::path::Path, nodes: u64, fronts: u64) {
    let store = crate::store::Store::open(root.to_path_buf()).expect("open fixture");
    let mut writer = crate::ops::Ctx::load_for_write(store, crate::ops::Whose::Founding)
        .expect("load fixture writer");
    let mine = writer.lock_for_write().expect("lock fixture");
    let mut events = Vec::with_capacity(nodes as usize * 2 + 1);
    for num in 1..=nodes {
        events.push(crate::event::Body::NodeCreated {
            node: node_id(num),
            num,
            kind: crate::event::Kind::Task,
            title: format!("Measured work {num}"),
            why: "Exercise opening performance".into(),
            parent: None,
            blocks: false,
            refs: vec![],
            governs: vec![],
            arms: vec![],
            against: None,
        });
        if num > fronts {
            events.push(crate::event::Body::StateChanged {
                node: node_id(num),
                state: crate::event::State::Done,
                outcome: "Finished measured work".into(),
                forced: false,
                until: None,
            });
        }
    }
    events.push(crate::event::Body::Pushed { node: node_id(1) });
    writer.emit(events).expect("write synthetic tree");
    if mine {
        writer.unlock();
    }
}

fn node_id(num: u64) -> String {
    format!("{num:026}")
}

fn percentiles(rows: &[serde_json::Value], key: &str) -> serde_json::Value {
    let mut values = rows
        .iter()
        .map(|row| row[key].as_u64().unwrap())
        .collect::<Vec<_>>();
    values.sort_unstable();
    let rank = |percent: usize| (percent * values.len()).div_ceil(100).saturating_sub(1);
    serde_json::json!({
        "p50": values[rank(50)],
        "p99": values[rank(99)],
        "max": values.last().copied().unwrap_or_default()
    })
}

fn env_str(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"))
}

fn env_path(name: &str) -> std::path::PathBuf {
    env_str(name).into()
}

fn env_num<T: std::str::FromStr>(name: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    env_str(name).parse().unwrap_or_else(|error| panic!("invalid {name}: {error:?}"))
}
