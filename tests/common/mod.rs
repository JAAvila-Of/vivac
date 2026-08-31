//! The sandbox the integration tests share.
//!
//! No dependencies: `CARGO_BIN_EXE_vivac` comes from cargo and the store is a
//! temporary directory. Every test seeds its own tree, because a shared one
//! would make execution order matter.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

pub struct Sandbox(pub PathBuf);

impl Sandbox {
    /// A directory with no `.vivac/`. For proving the tool stays quiet where
    /// nobody planted it.
    ///
    /// `mod common` is compiled once per test binary, so the ones that do not
    /// use this see it as dead. It is not.
    #[allow(dead_code)]
    pub fn new_empty(name: &str) -> Sandbox {
        let d = std::env::temp_dir().join(format!(
            "vivac-v-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        Sandbox(d)
    }

    pub fn new_seeded(name: &str) -> Sandbox {
        let d = std::env::temp_dir().join(format!(
            "vivac-t-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
