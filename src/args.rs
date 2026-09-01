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

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Args {
    pub positionals: Vec<String>,
    opts: HashMap<String, Vec<String>>,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(it: I) -> Args {
        let v: Vec<String> = it.into_iter().collect();
        let mut a = Args::default();
        let mut i = 0;
        while i < v.len() {
            if let Some(k) = v[i].strip_prefix("--") {
                let (k, inline) = match k.split_once('=') {
                    Some((k, val)) => (k, Some(val.to_string())),
                    None => (k, None),
                };
                let val = inline.or_else(|| {
                    v.get(i + 1).filter(|n| !n.starts_with("--")).map(|n| {
                        i += 1;
                        n.clone()
                    })
                });
                a.opts.entry(k.to_string()).or_default().extend(val);
            } else {
                a.positionals.push(v[i].clone());
            }
            i += 1;
        }
        a
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
        Args::parse(s.split_whitespace().map(String::from))
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
}
