//! The sandbox the integration tests share.
//!
//! No dependencies: `CARGO_BIN_EXE_vivac` comes from cargo and the store is a
//! temporary directory. Every test seeds its own tree, because a shared one
//! would make execution order matter.

use std::path::PathBuf;
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

pub struct Sandbox(pub PathBuf);

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
        Sandbox(d)
    }

    pub fn new_seeded(name: &str) -> Sandbox {
        let d = unique("t", name);
        std::fs::create_dir_all(&d).unwrap();
        let c = Sandbox(d);
        c.ok(&["init"]);
        c
    }

    pub fn run(&self, args: &[&str]) -> (String, i32) {
        let o = Command::new(BIN)
            .current_dir(&self.0)
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
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
