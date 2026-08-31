//! Argument parsing by hand.
//!
//! No `clap`. The security pillar wants few dependencies to audit and the
//! performance one pays for process startup on every call --there is no
//! daemon-- so the surface is this: positionals, `--key value` and flags.
//! It fits in forty lines, and every flag can carry a Spanish alias without
//! anyone having to know about it.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Args {
    pub positionals: Vec<String>,
    opts: HashMap<String, Vec<String>>,
}

/// English is canonical; the Spanish name of every flag keeps working.
///
/// The tool was written in Spanish and three real trees were seeded with those
/// flags already in the fingers. Renaming them outright would have broken
/// scripts for nothing: normalizing here means everything downstream sees one
/// name, the help teaches only English, and the alias costs one match arm.
///
/// An unknown flag is not in this table, so it comes out exactly as it was
/// typed and `unknown` can quote it back.
fn canonical(k: &str) -> &str {
    match k {
        "alternativa" => "alternative",
        "bloquea" => "blocks",
        "cascada" => "cascade",
        "forzar" => "force",
        "luego" => "next",
        "padre" => "parent",
        "por" => "why",
        "razon" => "reason",
        "reabrir" => "reopen",
        "rescatar" => "rescue",
        "tipo" => "type",
        "todo" => "all",
        other => other,
    }
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
                a.opts.entry(canonical(k).to_string()).or_default().extend(val);
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
    pub fn unknown(&self, permitidas: &[&str]) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .opts
            .keys()
            .map(|k| k.as_str())
            .filter(|k| !permitidas.contains(k))
            .collect();
        v.sort_unstable();
        v
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

    /// The Spanish name of a flag still works, and lands on the English one.
    ///
    /// Three real trees were seeded with those flags in the fingers. Dropping
    /// them would break scripts for nothing, so they normalize at the door and
    /// nothing downstream ever sees two names for one thing.
    #[test]
    fn the_spanish_alias_resolves_to_the_english_name() {
        let a = p("t --por reason --bloquea --padre 3 --forzar --luego x");
        assert_eq!(a.opt("why"), Some("reason"));
        assert!(a.has("blocks"));
        assert_eq!(a.opt("parent"), Some("3"));
        assert!(a.has("force"));
        assert_eq!(a.opt("next"), Some("x"));

        // And it is genuinely the same key, not a second one.
        assert!(a.unknown(&["why", "blocks", "parent", "force", "next"]).is_empty());

        // An unknown flag is not in the table, so it is quoted back verbatim.
        assert_eq!(p("--inventada 1").unknown(&["why"]), vec!["inventada"]);
    }

    /// Every Spanish name in the alias table maps to a different English one.
    ///
    /// The table is the one place in the crate where a Spanish string is load
    /// bearing, so a global rename can translate it and break nothing at
    /// compile time. This test is the thing that notices.
    #[test]
    fn no_alias_translated_itself_away() {
        for es in [
            "por", "tipo", "bloquea", "forzar", "luego", "padre", "cascada",
            "rescatar", "reabrir", "todo", "razon", "alternativa",
        ] {
            let en = canonical(es);
            assert_ne!(en, es, "the alias for --{es} was lost");
            assert_eq!(canonical(en), en, "--{en} is not canonical");
        }
    }
}
