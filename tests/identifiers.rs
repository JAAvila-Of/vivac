//! What a stranger reads when they open the source: the names in the code.
//!
//! `english.rs` guards the chrome the binary prints. This guards the other
//! public surface. The project's rule is that everything a third party can
//! read is written in English, and identifiers are on that list.
//!
//! It exists because the rule leaked a second time, and quietly. `d45`
//! renamed the visible surface -- flags, log fields, help, types -- and
//! stopped at the skin: twenty-seven identifiers went on reading in Spanish
//! through three published releases, and the suite stayed green for every one
//! of them.
//!
//! **The list is an allow-list, and that is the whole point.** A list of the
//! Spanish words somebody already caught goes green over the next one nobody
//! thought of. That is not a worry, it is a measurement: the output guard's
//! vocabulary is derived from every word the binary ever printed in Spanish,
//! and it recognised **ten of those twenty-seven**. `cima`, `crudo`, `hoy`
//! and `mayus` walked straight through, because a word has to have been
//! printed to be on that list and these were never printed.
//!
//! Reading the identifiers by hand does not close it either, and that is
//! measured too. A pass that listed every `let`, every `fn` and every
//! parameter found twenty-three of the twenty-seven. The four it could not
//! see were `PREFIJOS`, `CORTES`, `BORDES` and `PRESUPUESTO`: the pattern
//! doing the reading started at a lower case letter, so a constant was
//! invisible to it by construction. This guard lower cases everything before
//! it compares, which is how it found them.
//!
//! So the question is inverted. Every word of every identifier has to appear
//! in `tests/data/identifier-vocabulary.txt`, and a word gets in by being
//! written down on purpose.
//!
//! The vocabulary's own honesty rests on two checks rather than on trust.
//! `the_vocabulary_carries_no_spanish` refuses anything the output guard
//! bans, so this list cannot widen that one. And the match has to be exact in
//! both directions, so a word cannot linger with a licence after the last
//! identifier that used it is gone.

use std::collections::{BTreeMap, BTreeSet};

const VOCABULARY: &str = "tests/data/identifier-vocabulary.txt";
const SPANISH: &str = "tests/data/spanish-vocabulary.txt";

/// `era`, `doe`, `yoe`, `doy` and `mp` are Howard Hinnant's own names in the
/// `civil_from_days` algorithm, which `clock.rs` implements. Renaming them
/// would make it unrecognisable against its reference, and that costs more
/// than the collision does. `era` is the only one of the five that the
/// Spanish vocabulary happens to contain.
const KNOWN_ENGLISH: &[&str] = &["era"];

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_list(rel: &str) -> BTreeSet<String> {
    std::fs::read_to_string(root().join(rel))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every Rust file the crate publishes.
fn sources() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in ["src", "tests", "tests/common"] {
        let Ok(entries) = std::fs::read_dir(root().join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The source with every comment and every literal removed.
///
/// Prose and printed strings are guarded elsewhere, and they are written in
/// sentences rather than in identifiers: folding them in here would drag the
/// whole English language into the vocabulary. Char literals go too. A `'"'`
/// left in place opens a string that never closes and swallows the rest of
/// the file, which is a silent hole in a guard rather than a failure.
fn code_only(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let quote = '\u{27}';
    let backslash = '\u{5c}';
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else if c == 'r' && matches!(b.get(i + 1), Some('"') | Some('#')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                j += 1;
                while j < b.len() {
                    if b[j] == '"' && b[j + 1..].iter().take(hashes).all(|c| *c == '#') {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                i = j;
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '"' {
            i += 1;
            while i < b.len() {
                if b[i] == backslash {
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if c == quote {
            // A char literal has a closing quote; a lifetime does not.
            let escaped = b.get(i + 1) == Some(&backslash);
            let closes = if escaped {
                b.get(i + 3) == Some(&quote)
            } else {
                b.get(i + 2) == Some(&quote)
            };
            if closes {
                i += if escaped { 4 } else { 3 };
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Splits an identifier into words: on `_`, and where a lower case letter is
/// followed by an upper case one. `HashMap` gives `hash` and `map`, while
/// `json` stays whole -- splitting at every capital would shred an acronym
/// into single letters. Everything comes out in lower case, so `PREFIJOS` is
/// caught by the same line of the list that catches `prefijos`.
fn split_into(ident: &str, out: &mut BTreeSet<String>) {
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in ident.chars() {
        if c == '_' {
            if !cur.is_empty() {
                out.insert(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase();
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
}

/// Every word used inside an identifier anywhere the crate publishes, each
/// one against the first file that uses it.
///
/// The file is what turns a failure into a fix. A bare list of words sends
/// you grepping, and the first four this guard ever caught were spelt only in
/// capitals, so the obvious grep came back empty on all four.
///
/// Single letters and all-digit fragments are dropped: `i`, `n` and the `64`
/// of `u64` say nothing about which language anybody was writing in.
fn words_in_code() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for path in sources() {
        let where_at = path
            .strip_prefix(root())
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        let code = code_only(&std::fs::read_to_string(&path).unwrap());
        let mut ident = String::new();
        for c in code.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_alphanumeric() || c == '_' {
                ident.push(c);
                continue;
            }
            // A run that opens with a digit is a numeric literal and not an
            // identifier, because Rust does not allow one: `0u64`, `38usize`
            // and `0x1f` are not words in any language.
            if !ident.is_empty() && !ident.starts_with(|c: char| c.is_ascii_digit()) {
                let mut words = BTreeSet::new();
                split_into(&ident, &mut words);
                for w in words {
                    if w.len() > 1 && !w.chars().all(|c| c.is_ascii_digit()) {
                        out.entry(w).or_insert_with(|| where_at.clone());
                    }
                }
            }
            ident.clear();
        }
    }
    out
}

/// The vocabulary has to match the tree **exactly**, in both directions.
///
/// The missing half is the guard. The surplus half is not bookkeeping: a word
/// left in the file after the identifier that used it is gone is a name
/// already blessed, waiting for whoever reaches for it next. One arrived
/// within an hour of this file being written, swept in from a stale packaged
/// copy of 0.1.0 sitting under `target/`, and it would have sat there
/// permitting the Spanish for *budget* for good.
#[test]
fn every_identifier_reads_as_english() {
    let used = words_in_code();
    let listed = read_list(VOCABULARY);
    let missing: Vec<(&String, &String)> =
        used.iter().filter(|(w, _)| !listed.contains(*w)).collect();
    let surplus: Vec<&String> = listed.iter().filter(|w| !used.contains_key(*w)).collect();
    if missing.is_empty() && surplus.is_empty() {
        return;
    }
    let mut msg = String::from("\n  The identifier vocabulary does not match the tree.\n\n");
    if !missing.is_empty() {
        msg.push_str(&format!(
            "  {} word(s) used in the code and not in the file:\n",
            missing.len()
        ));
        for (w, file) in &missing {
            msg.push_str(&format!("      {w:<24} {file}\n"));
        }
        msg.push('\n');
    }
    if !surplus.is_empty() {
        msg.push_str(&format!(
            "  {} word(s) in the file that no identifier uses any more:\n      {}\n\n",
            surplus.len(),
            surplus
                .iter()
                .map(|w| w.as_str())
                .collect::<Vec<_>>()
                .join("\n      ")
        ));
    }
    msg.push_str(
        "  Read them before accepting them. Everything public is written in English,\n  \
         so a word that is not gets an identifier renamed rather than a line added.\n  \
         `python tools/identifier-vocabulary.py` rewrites the file from the block below.\n\n",
    );
    msg.push_str("--- BEGIN VOCABULARY ---\n");
    for w in used.keys() {
        msg.push_str(w);
        msg.push('\n');
    }
    msg.push_str("--- END VOCABULARY ---\n");
    panic!("{msg}");
}

#[test]
fn the_vocabulary_carries_no_spanish() {
    let spanish = read_list(SPANISH);
    let blessed: Vec<String> = read_list(VOCABULARY)
        .into_iter()
        .filter(|w| spanish.contains(w) && !KNOWN_ENGLISH.contains(&w.as_str()))
        .collect();
    assert!(
        blessed.is_empty(),
        "\n  The identifier vocabulary blesses {} word(s) the output guard bans:\n\n      {}\n\n  \
         A guard does not get to widen its own list. Rename the identifier -- or, if the\n  \
         word really is English in this context, say so in KNOWN_ENGLISH with the reason.\n",
        blessed.len(),
        blessed.join("\n      ")
    );
}

#[test]
fn the_vocabulary_is_sorted_and_says_nothing_twice() {
    let raw = std::fs::read_to_string(root().join(VOCABULARY)).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut tidy = lines.clone();
    tidy.sort_unstable();
    tidy.dedup();
    assert_eq!(
        lines, tidy,
        "\n  {VOCABULARY} is out of order or repeats itself. A list nobody can scan is a\n  \
         list nobody checks. `python tools/identifier-vocabulary.py` rewrites it.\n"
    );
}
