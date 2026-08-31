//! The store: one directory, two files.
//!
//! ```text
//! .vivac/
//!   events    append-only log, one JSON per line   <- SOURCE OF TRUTH
//!   config    project_id and opaque actor
//! ```
//!
//! There is no `index.db` and no `state`. `ROADMAP.md` §4 leaves them out of
//! Tier 0 on purpose: with dozens of nodes, folding the log in memory is
//! instant, and SQLite lands when the performance pillar asks for it --ten
//! thousand nodes, FTS5-- not before. Storing the stack apart would be the
//! second home of the same state.

use crate::{clock, id};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".vivac";
pub const LOG: &str = "events";
pub const CONFIG: &str = "config";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub project_id: String,
    /// Opaque identifier for this install. **It carries no email and no name**:
    /// the security pillar forbids it, and vetoes `MODEL.md` §3.4.
    pub actor: String,
}

impl Config {
    fn new_seeded() -> Config {
        Config {
            version: 1,
            project_id: id::ulid(),
            actor: format!("a_{}", &id::ulid()[..12]),
        }
    }
}

pub struct Store {
    pub root: PathBuf,
    pub config: Config,
}

/// Walks up from `from_dir` looking for a `.vivac/`. No daemon and no environment
/// variable: the same rule as git, already in everyone's fingers.
pub fn find_root(from_dir: &Path) -> Option<PathBuf> {
    let mut d = from_dir.to_path_buf();
    loop {
        if d.join(DIR).is_dir() {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

impl Store {
    pub fn open(root: PathBuf) -> std::io::Result<Store> {
        let p = root.join(DIR).join(CONFIG);
        let config = match fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).map_err(std::io::Error::other)?,
            Err(_) => {
                // A `.vivac/` with no config comes from an earlier version or a
                // half-finished delete. Fill it in rather than fail: the tree,
                // which is what matters, lives in `events`.
                let c = Config::new_seeded();
                write_config(&root, &c)?;
                c
            }
        };
        Ok(Store { root, config })
    }

    pub fn create(root: &Path) -> std::io::Result<Store> {
        let d = root.join(DIR);
        fs::create_dir_all(&d)?;
        let config = Config::new_seeded();
        write_config(root, &config)?;
        if !d.join(LOG).exists() {
            File::create(d.join(LOG))?;
        }
        Ok(Store {
            root: root.to_path_buf(),
            config,
        })
    }

    pub fn log(&self) -> PathBuf {
        self.root.join(DIR).join(LOG)
    }
}

fn write_config(root: &Path, c: &Config) -> std::io::Result<()> {
    let mut f = File::create(root.join(DIR).join(CONFIG))?;
    f.write_all(serde_json::to_string_pretty(c)?.as_bytes())?;
    f.write_all(b"\n")
}

impl Store {
    /// Reads the whole log. An unreadable line **does not abort**: it is
    /// counted and skipped. A half-written log has to stay readable, or the
    /// tool that keeps the thread becomes the one that loses it.
    pub fn read_all(&self) -> std::io::Result<(Vec<crate::event::Event>, usize)> {
        let f = match File::open(self.log()) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
            Err(e) => return Err(e),
        };
        let mut eventos = Vec::new();
        let mut rotas = 0usize;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(e) => eventos.push(e),
                Err(_) => rotas += 1,
            }
        }
        Ok((eventos, rotas))
    }

    /// Appends events at the end. One line per event, rewriting nothing.
    ///
    /// This is the critical path of the agent's turn: a p99 < 5 ms budget.
    /// That is why there is no `fsync` --on Windows it costs more than the
    /// whole budget-- and why it opens in `append` mode, which makes each
    /// single-line write atomic and removes the need for a lock.
    pub fn append(&self, body: Vec<crate::event::Body>, from_seq: u64) -> std::io::Result<()> {
        let mut buf = String::with_capacity(256 * body.len());
        for (i, c) in body.into_iter().enumerate() {
            let e = crate::event::Event {
                seq: from_seq + i as u64 + 1,
                id: id::ulid(),
                ts: clock::now_rfc3339(),
                actor: self.config.actor.clone(),
                lane: "main".into(),
                payload: c,
            };
            buf.push_str(&serde_json::to_string(&e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
    }
}

impl Store {
    /// Writes already-built events, keeping their original timestamp. Only
    /// `import` uses it: a tree from elsewhere keeps its dates, because
    /// otherwise the migration flattens the only timeline it had.
    pub fn write_raw(&self, eventos: &[crate::event::Event]) -> std::io::Result<()> {
        let mut buf = String::with_capacity(256 * eventos.len());
        for e in eventos {
            buf.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_upward() {
        let tmp = std::env::temp_dir().join(format!("vivac-t-{}", id::ulid()));
        let depth_of = tmp.join("a").join("b").join("c");
        fs::create_dir_all(&depth_of).unwrap();
        assert!(find_root(&depth_of).is_none());
        Store::create(&tmp).unwrap();
        assert_eq!(find_root(&depth_of).unwrap(), tmp);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_actor_carries_no_personal_data() {
        let c = Config::new_seeded();
        assert!(c.actor.starts_with("a_"));
        assert!(!c.actor.contains('@'));
        assert_ne!(c.actor, whoami_ish());
    }

    fn whoami_ish() -> String {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default()
    }
}
