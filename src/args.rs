//! Argument parsing by hand.
//!
//! No `clap`. The security pillar wants few dependencies to audit and the
//! performance one pays for process startup on every call --there is no
//! daemon-- so the surface is this: positionals, `--key value` and flags.
//! It fits in forty lines.
//!
//! Nothing here normalises anything. It used to fold a table of Spanish
//! aliases at the door, and `d45` retired it: a flag is English or it is
//! unknown, and an unknown flag is refused rather than ignored. The two
//! rules that survive that are `unknown` and `extra`, and they say the same
//! thing about the two halves of a command line -- what the CLI did not
//! understand, it does not keep quiet about.

use crate::failure::Failure;
use std::collections::HashMap;

/// Flags that never carry a value, checked here rather than per command:
/// the parser runs before anything knows which command is even valid, so
/// there is no table to look a word up in yet (`f556`).
///
/// A word on this list eating the word after it turned `push --blocks
/// "title"` into a push with no title at all -- the flag took `"title"` as
/// its own value and left nothing behind for the positional. The list is
/// global on purpose: one word cannot be a switch for one command and a
/// value-carrying option for another, so nothing here is allowed to grow a
/// value anywhere in the binary.
pub(crate) const SWITCHES: &[&str] = &[
    "blocks",
    "root",
    "hook",
    "json",
    "all",
    "full",
    "force",
    "cascade",
    "off",
    "reopen",
    "gates",
    "everywhere",
    "no-open",
    "yes",
    "dry-run",
    "undo",
    "new-tree",
];

#[derive(Debug, Default)]
pub struct Args {
    pub positionals: Vec<String>,
    opts: HashMap<String, Vec<String>>,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(it: I) -> Result<Args, Failure> {
        let v: Vec<String> = it.into_iter().collect();
        let mut a = Args::default();
        let mut i = 0;
        while i < v.len() {
            if let Some(k) = v[i].strip_prefix("--") {
                let (k, inline) = match k.split_once('=') {
                    Some((k, val)) => (k, Some(val.to_string())),
                    None => (k, None),
                };
                if SWITCHES.contains(&k) {
                    if let Some(val) = inline {
                        return Err(Failure::usage(format!(
                            "--{k} does not take a value: leave out \"={val}\"."
                        )));
                    }
                    a.opts.entry(k.to_string()).or_default();
                } else {
                    let val = inline.or_else(|| {
                        v.get(i + 1).filter(|n| !n.starts_with("--")).map(|n| {
                            i += 1;
                            n.clone()
                        })
                    });
                    a.opts.entry(k.to_string()).or_default().extend(val);
                }
            } else {
                a.positionals.push(v[i].clone());
            }
            i += 1;
        }
        Ok(a)
    }

    pub fn has(&self, k: &str) -> bool {
        self.opts.contains_key(k)
    }

    pub fn opt(&self, k: &str) -> Option<&str> {
        self.opts.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }

    pub fn opt_or(&self, k: &str) -> String {
        self.opt(k).unwrap_or_default().to_string()
    }

    /// Repeatable: `--ref a --ref b`.
    pub fn list(&self, k: &str) -> Vec<String> {
        self.opts.get(k).cloned().unwrap_or_default()
    }

    pub fn positional(&self, i: usize) -> Option<&str> {
        self.positionals.get(i).map(|s| s.as_str())
    }

    /// Options this command does not know.
    ///
    /// Swallowing them is the worst possible failure on the half of the
    /// interface the agent uses: you type `--kind finding`, the CLI says
    /// nothing, and the node keeps the default type. Nobody notices until
    /// they look at the tree. Found exactly that way, while using it.
    pub fn unknown(&self, allowed: &[&str]) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .opts
            .keys()
            .map(|k| k.as_str())
            .filter(|k| !allowed.contains(k))
            .collect();
        v.sort_unstable();
        v
    }

    /// Positionals beyond the ones the command takes.
    ///
    /// The mirror of `unknown`, and it exists because that one only ever
    /// covered the flags. The bare words went through in silence: `vivac add
    /// "title" "junk"` kept the title and dropped the rest with an exit code
    /// of 0. So did `--governs a b`, which is how it was actually found --
    /// a flag takes one value, so `b` stops being part of the flag and
    /// becomes a positional nobody was looking at (`f52`).
    pub fn extra(&self, takes: usize) -> &[String] {
        self.positionals.get(takes..).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Args {
        Args::parse(s.split_whitespace().map(String::from)).unwrap()
    }

    #[test]
    fn positionals_and_options() {
        let a = p("title --why reason --blocks --ref one --ref two");
        assert_eq!(a.positional(0), Some("title"));
        assert_eq!(a.opt("why"), Some("reason"));
        assert!(a.has("blocks"));
        assert_eq!(a.list("ref"), vec!["one", "two"]);
    }

    #[test]
    fn a_flag_next_to_another_flag() {
        // `--blocks --why x`: `--blocks` does not eat the `--why`.
        let a = p("--blocks --why x");
        assert!(a.has("blocks"));
        assert_eq!(a.opt("blocks"), None);
        assert_eq!(a.opt("why"), Some("x"));
    }

    #[test]
    fn an_option_that_does_not_exist_does_not_pass_in_silence() {
        let a = p("x --kind finding --type task");
        assert_eq!(a.unknown(&["type", "why"]), vec!["kind"]);
        assert!(a.unknown(&["type", "kind"]).is_empty());
    }

    #[test]
    fn equals_sign() {
        let a = p("--type=decision");
        assert_eq!(a.opt("type"), Some("decision"));
    }

    /// A flag is English or it is nothing.
    ///
    /// The Spanish names the tool grew up with were normalized at the door
    /// until `d45` retired them. They are now unknown flags, and an unknown
    /// flag is **refused**, not ignored: a typo that changes nothing silently
    /// is worse than one that stops you.
    #[test]
    fn a_flag_that_is_not_english_is_unknown_rather_than_ignored() {
        let a = p("t --padre 3");
        assert_eq!(a.opt("parent"), None);
        assert_eq!(a.unknown(&["why", "parent"]), vec!["padre"]);
    }

    /// An unknown flag comes out exactly as it was typed, so the message that
    /// refuses it can name it.
    #[test]
    fn an_unknown_flag_is_quoted_back_verbatim() {
        assert_eq!(p("--nonesuch 1").unknown(&["why"]), vec!["nonesuch"]);
    }

    /// A word too many is refused rather than dropped.
    ///
    /// `--governs a b` is the way in that actually happened, and it does not
    /// look like a positional at all when you type it.
    #[test]
    fn a_positional_too_many_is_not_swallowed() {
        assert!(p("title --why reason").extra(1).is_empty());
        assert_eq!(p("title junk --why reason").extra(1), ["junk"]);
        assert_eq!(p("title --governs a b").extra(1), ["b"]);
        // A command that takes two words is not tripped by its second one.
        assert!(p("3 suspect --why reason").extra(2).is_empty());
    }

    /// `f556`: `--blocks` never takes a value, so `push --blocks "title"`
    /// must leave `"title"` as a positional rather than swallow it.
    #[test]
    fn a_switch_never_eats_the_word_after_it() {
        let a = p("--blocks t --why w");
        assert!(a.has("blocks"));
        assert_eq!(a.opt("blocks"), None);
        assert_eq!(a.positional(0), Some("t"));
        assert_eq!(a.opt("why"), Some("w"));
    }

    /// `--blocks=x` used to store `"x"` in silence. A switch has nothing to
    /// store, so this is refused as a usage error instead.
    #[test]
    fn equals_on_a_switch_is_a_usage_error() {
        let e = Args::parse(vec!["--blocks=x".to_string()]).unwrap_err();
        assert_eq!(e.code(), 2);
        let msg = e.message();
        assert!(msg.contains("--blocks"), "{msg}");
        assert!(msg.contains("does not take a value"), "{msg}");
    }

    /// `f556`: `setup --yes claude-code` is the command line that surfaced
    /// the bug -- `--yes` used to take `"claude-code"` as its own value and
    /// leave nothing behind for the harness name.
    #[test]
    fn a_switch_before_a_positional_leaves_the_positional_alone() {
        let a = p("--yes claude-code");
        assert!(a.has("yes"));
        assert_eq!(a.opt("yes"), None);
        assert_eq!(a.positional(0), Some("claude-code"));
    }

    /// `f632`, the same shape as `f556` above: `--new-tree` never takes a
    /// value, so `setup --new-tree claude-code` must leave `claude-code` as
    /// a positional rather than swallow it as `--new-tree`'s own value.
    #[test]
    fn new_tree_never_eats_the_word_after_it() {
        let a = p("--new-tree claude-code");
        assert!(a.has("new-tree"));
        assert_eq!(a.opt("new-tree"), None);
        assert_eq!(a.positional(0), Some("claude-code"));
    }

    /// The same word cannot be a switch for one command and a value-carrying
    /// option for another, because the list in `SWITCHES` is global and
    /// applies before any command is even known. This scans the crate for
    /// `.opt`, `.opt_or` or `.list` called with one of those names, which
    /// would always come back empty now that the word never keeps a value --
    /// a silent break rather than a loud one.
    #[test]
    fn no_switch_is_ever_read_as_if_it_carried_a_value() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut pending = vec![root];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                // Every module in this crate keeps its `#[cfg(test)] mod
                // tests` at the bottom, so this is where the tests that
                // exercise `.opt("blocks")` and the like live, and it is not
                // where a real command would ever read one for a value.
                let code = src.split("#[cfg(test)]").next().unwrap_or(&src);
                for switch in SWITCHES {
                    for accessor in [".opt(\"", ".opt_or(\"", ".list(\""] {
                        if code.contains(&format!("{accessor}{switch}\")")) {
                            offenders.push(format!("{switch} via {accessor}...) in {path:?}"));
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }
}
