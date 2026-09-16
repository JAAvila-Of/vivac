//! `d598`: every writer holds the tree's lock, so two of them never stamp
//! the same number. Before it, eight writers at once repeated about half
//! of them (`f105`, reproduced on 15-sep-2026).

mod common;
use common::Sandbox;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// Every `node.created`'s number, and every line's `seq`, straight off the log.
fn numbers_and_seqs(log: &str) -> (usize, BTreeSet<u64>, Vec<u64>) {
    let mut created = 0;
    let mut nums = BTreeSet::new();
    let mut seqs = Vec::new();
    for line in log.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        seqs.push(v["seq"].as_u64().unwrap());
        if v["payload"]["type"] == "node.created" {
            created += 1;
            nums.insert(v["payload"]["num"].as_u64().unwrap());
        }
    }
    (created, nums, seqs)
}

#[test]
fn eight_writers_at_once_never_share_a_number() {
    for run in 0..3 {
        let c = Sandbox::new_seeded(&format!("eight-writers-{run}"));
        let writers: Vec<_> = (0..8)
            .map(|w| {
                let dir = c.0.clone();
                let home = c.global_home().to_path_buf();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        let o = Command::new(BIN)
                            .current_dir(&dir)
                            .env("VIVAC_HOME", &home)
                            .args([
                                "add",
                                &format!("writer {w} node {i}"),
                                "--root",
                                "--why",
                                "concurrency",
                            ])
                            .output()
                            .unwrap();
                        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
                    }
                })
            })
            .collect();
        for t in writers {
            t.join().unwrap();
        }
        let (created, nums, seqs) = numbers_and_seqs(&c.log());
        assert_eq!(created, 320);
        assert_eq!(nums.len(), 320, "two nodes share a number");
        // With the lock, the file's order is seq's order: no sort, so an out-of-order line fails too.
        let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
        assert_eq!(seqs, expected, "seq has a gap or a repeat");
        let (out, code) = c.run(&["check"]);
        assert_eq!(code, 0, "{out}");
    }
}

#[test]
fn a_writer_that_cannot_get_the_lock_gives_up_after_five_seconds() {
    let c = Sandbox::new_seeded("held-lock");
    c.ok(&[
        "push",
        "Something to hang from",
        "--why",
        "so the brief has a spine",
    ]);
    let before = c.log();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(c.0.join(".vivac").join("lock"))
        .unwrap();
    lock.lock().unwrap();

    let started = Instant::now();
    let (out, code) = c.run(&["add", "Blocked", "--why", "the lock is held"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains("held this tree for 5 seconds"), "{out}");
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert_eq!(c.log(), before, "something was written without the lock");

    let (out, code) = c.run_stdin(
        &["session", "start", "--hook"],
        r#"{"source":"startup","session_id":"s-held"}"#,
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("vivac · project:"),
        "the brief did not come out: {out}"
    );
    assert!(out.contains("Session not recorded"), "{out}");
    assert_eq!(c.log(), before);

    let (out, code) = c.run_stdin(
        &["session", "end", "--hook"],
        r#"{"source":"startup","session_id":"s-held"}"#,
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(c.log(), before);

    // A turn with nothing to stop must never even ask for the lock: the
    // check that decides that is cheap and runs on the tree already in
    // memory, so a lock somebody else holds must not slow it down at all,
    // let alone up to the five-second deadline above.
    let quiet = Sandbox::new_seeded("held-lock-nothing-to-stop");
    let quiet_lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(quiet.0.join(".vivac").join("lock"))
        .unwrap();
    quiet_lock.lock().unwrap();
    let started = Instant::now();
    let (out, code) = quiet.run_stdin(
        &["session", "end", "--hook"],
        r#"{"source":"startup","session_id":"s-quiet"}"#,
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took the lock even though there was nothing to stop"
    );
    quiet_lock.unlock().unwrap();

    lock.unlock().unwrap();
}

/// Not a test on its own: `a_lock_held_by_a_process_that_died_is_released`
/// runs this binary again with only this one selected, so that a separate
/// process holds the lock and can be killed while holding it.
#[test]
#[ignore = "helper, run by a_lock_held_by_a_process_that_died_is_released"]
fn helper_hold_the_lock() {
    let Ok(path) = std::env::var("VIVAC_TEST_HOLD_LOCK") else {
        return;
    };
    let f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .unwrap();
    f.lock().unwrap();
    println!("lock held");
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn a_lock_held_by_a_process_that_died_is_released() {
    let c = Sandbox::new_seeded("dead-holder");
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "helper_hold_the_lock",
            "--nocapture",
        ])
        .env("VIVAC_TEST_HOLD_LOCK", c.0.join(".vivac").join("lock"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(holder.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            out.read_line(&mut line).unwrap() > 0,
            "the holder never took the lock"
        );
        if line.contains("lock held") {
            break;
        }
    }
    holder.kill().unwrap();
    holder.wait().unwrap();

    let started = Instant::now();
    c.ok(&[
        "add",
        "After the holder died",
        "--why",
        "the system released its lock",
    ]);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "a dead process's lock was not released"
    );
}
