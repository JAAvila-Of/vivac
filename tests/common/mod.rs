//! The sandbox the integration tests share.
//!
//! No dependencies: `CARGO_BIN_EXE_vivac` comes from cargo and the store is a
//! temporary directory. Every test seeds its own tree, because a shared one
//! would make execution order matter.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

/// A directory name nothing else can take.
///
/// It used to be the clock alone, and the clock is not a source of
/// uniqueness: on the machine this was written on, six consecutive reads of
/// the system time come back identical, and every test in a file passes the
/// same `name`. Two of them then seeded the same store, and `vivac check`
/// reported every number twice. It had been true on all three platforms all
/// along; macOS is just where the threads stopped saving it, on the first run
/// the suite ever had outside one developer's machine.
///
/// The pid separates test binaries, the counter separates calls inside one,
/// and the clock separates runs whose pid the system reused. Unique by
/// construction rather than by luck.
fn unique(prefix: &str, name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vivac-{prefix}-{name}-{}-{n}-{ts}",
        std::process::id()
    ))
}

pub struct Sandbox(pub PathBuf, PathBuf);

impl Sandbox {
    /// A directory with no `.vivac/`. For proving the tool stays quiet where
    /// nobody planted it.
    ///
    /// `mod common` is compiled once per test binary, so the ones that do not
    /// use this see it as dead. It is not.
    #[allow(dead_code)]
    pub fn new_empty(name: &str) -> Sandbox {
        let d = unique("v", name);
        std::fs::create_dir_all(&d).unwrap();
        Sandbox(d, unique("v-home", name))
    }

    /// Like [`Self::new_empty`], but pointed at a `VIVAC_HOME` a caller
    /// already holds instead of a fresh one of its own. `find --everywhere`
    /// has to work from here: it reads the registry, not the tree
    /// underfoot, and this is the directory with no tree underfoot at all.
    #[allow(dead_code)]
    pub fn new_empty_in(name: &str, home: &Path) -> Sandbox {
        let d = unique("v", name);
        std::fs::create_dir_all(&d).unwrap();
        Sandbox(d, home.to_path_buf())
    }

    pub fn new_seeded(name: &str) -> Sandbox {
        let d = unique("t", name);
        std::fs::create_dir_all(&d).unwrap();
        let c = Sandbox(d, unique("t-home", name));
        c.ok(&["init"]);
        c
    }

    /// A seeded project registered into a `VIVAC_HOME` a caller already
    /// holds, rather than one of its own. `find --everywhere` (`d273`)
    /// answers from the registry, so proving it needs two or more projects
    /// sharing one -- everything else in this file keeps each sandbox's
    /// home to itself, which is exactly wrong for that question.
    #[allow(dead_code)]
    pub fn new_seeded_in(name: &str, home: &Path) -> Sandbox {
        let d = unique("t", name);
        std::fs::create_dir_all(&d).unwrap();
        let c = Sandbox(d, home.to_path_buf());
        c.ok(&["init"]);
        c
    }

    /// Where `VIVAC_HOME` points for every subprocess this sandbox spawns.
    ///
    /// A sibling temporary directory, unique to this sandbox and never
    /// created up front: `t265`'s registry only writes here on its own, and
    /// this is what keeps that write off the machine running the suite.
    #[allow(dead_code)]
    pub fn global_home(&self) -> &Path {
        &self.1
    }

    pub fn run(&self, args: &[&str]) -> (String, i32) {
        let o = Command::new(BIN)
            .current_dir(&self.0)
            .env("VIVAC_HOME", &self.1)
            .args(args)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
            o.status.code().unwrap_or(-1),
        )
    }

    pub fn ok(&self, args: &[&str]) -> String {
        let (s, c) = self.run(args);
        assert_eq!(c, 0, "`vivac {}` failed with {c}:\n{s}", args.join(" "));
        s
    }

    /// Runs the binary with a payload on stdin, the way a hook is called.
    #[allow(dead_code)]
    pub fn run_stdin(&self, args: &[&str], stdin: &str) -> (String, i32) {
        use std::io::Write;
        let mut child = Command::new(BIN)
            .current_dir(&self.0)
            .env("VIVAC_HOME", &self.1)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        (
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
            o.status.code().unwrap_or(-1),
        )
    }

    /// The raw log. Some things are only provable against what was written,
    /// not against what a command chose to print.
    #[allow(dead_code)]
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.0.join(".vivac").join("events")).unwrap_or_default()
    }

    /// Appends a line straight to `events`, bypassing every command. `t411`
    /// §13's own tests need lines no CLI path would ever write: a line
    /// well-formed enough to name an event type or a node kind this version
    /// does not know, or one broken in ways a real crash leaves behind.
    #[allow(dead_code)]
    pub fn append_raw_line(&self, line: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.0.join(".vivac").join("events"))
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    /// A line only a newer vivac could have written: well-formed JSON with a
    /// numeric `seq` and a `payload.type` this version's `Body` does not
    /// carry. `t411` §13. The exact `seq` given does not matter -- the line
    /// never folds, since deserialising it as an `Event` fails before `seq`
    /// is read for anything but a shape check.
    #[allow(dead_code)]
    pub fn append_unknown_event_type(&self) {
        self.append_raw_line(
            r#"{"seq":999,"id":"01UNKNOWNTYPEAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.evolved","node":"01UNKNOWNTYPEBBBBBBBBBBBBB"}}"#,
        );
    }

    /// The same, with a well-formed `node.created` whose `kind` this version
    /// cannot parse.
    #[allow(dead_code)]
    pub fn append_unknown_node_kind(&self) {
        self.append_raw_line(
            r#"{"seq":999,"id":"01UNKNOWNKINDAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01UNKNOWNKINDBBBBBBBBBBBBB","num":999,"kind":"epic","title":"From a newer vivac"}}"#,
        );
    }

    /// A type this version knows, carrying a value it does not: a flag that
    /// is not `suspect`, `review` or `stale`. The shape a newer vivac writes
    /// the day it adds a value to a field that already exists.
    #[allow(dead_code)]
    pub fn append_unreadable_known_event(&self) {
        self.append_raw_line(
            r#"{"seq":999,"id":"01UNKNOWNVALUEAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"flag.raised","node":"01UNKNOWNVALUEBBBBBBBBBBBB","flag":"advise","reason":"from a newer vivac"}}"#,
        );
    }

    /// `d441` on top of `t411` §13 bis: an `arm.added` missing `dir`, the
    /// shape a format before the folder existed would have written.
    #[allow(dead_code)]
    pub fn append_arm_added_without_dir(&self) {
        self.append_raw_line(
            r#"{"seq":999,"id":"01ARMNODIRAAAAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"arm.added","node":"01ARMNODIRBBBBBBBBBBBBBBBB","command":"cargo test"}}"#,
        );
    }

    /// The same, with a `node.created` whose `arms` are bare strings -- the
    /// shape `t411`'s first round wrote, before `d441` made the folder part
    /// of the pair.
    #[allow(dead_code)]
    pub fn append_node_created_with_string_arms(&self) {
        self.append_raw_line(
            r#"{"seq":999,"id":"01OLDARMSHAPEAAAAAAAAAAAAA","ts":"2026-01-01T00:00:00Z","actor":"a_test0000000","lane":"main","payload":{"type":"node.created","node":"01OLDARMSHAPEBBBBBBBBBBBBB","num":999,"kind":"rule","title":"From before d441","arms":["cargo test"]}}"#,
        );
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
        std::fs::remove_dir_all(&self.1).ok();
    }
}
