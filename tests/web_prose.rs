//! What a browser reads: the words `vivac web` writes into its own pages.
//!
//! `english.rs` guards the chrome the binary prints, and it runs commands
//! and reads stdout to do it. The web never writes to stdout -- it serves
//! HTTP -- so it is invisible to that guard by construction. `identifiers.rs`
//! guards the other public surface, the names in the code, and it is
//! invisible to prose for a different reason: it strips every comment and
//! every literal before it compares (see its own line 92 and the reasoning
//! at line 96) -- an allow-list over sentences would need the whole English
//! language in the vocabulary, which is not a list anybody could review.
//! `src/web` is five files and 2253 lines, the product's main face, and
//! until this guard nothing read what it says.
//!
//! So this is a third guard, over the string literals and the comments of
//! `src/web/**/*.rs`, and it is a deny list for the same reason
//! `english.rs` is one: `src/web`'s literals carry HTML tags, CSS and a
//! little JavaScript besides the sentences meant for a person, and an
//! allow-list would have to bless all of that vocabulary too, which is the
//! false-positive rate a guard with no escape hatch cannot afford.
//!
//! `english.rs`'s second rule -- that no snake_case identifier has leaked
//! into prose -- is deliberately not part of this one. A CSS custom property
//! or a JavaScript name sitting in a literal is not a leaked Rust
//! identifier, and this guard has no way to tell the two apart.
//!
//! **What a green run does not promise.** It proves no Spanish was written
//! into the pages' own text -- the labels, the headings, the words this
//! binary chose to write. It does not prove a served page reads as English,
//! and it must not try to: the content on the page is the user's own tree,
//! and a tree can be kept in any language. The project's own tree
//! (`vivac-project/.vivac/`) is entirely in Spanish, and a page that renders
//! it faithfully is behaving correctly, not failing this guard.

#[path = "common/literal_spans.rs"]
mod literal_spans;

use std::collections::BTreeSet;

const SPANISH: &str = "tests/data/spanish-vocabulary.txt";

/// HTML and CSS vocabulary that the deny list happens to also contain --
/// Spanish shares the spelling, not the sentence. `base-uri` is a
/// Content-Security-Policy directive (`src/web/mod.rs`), and `<meta>` is the
/// HTML tag every page's `<head>` carries, including the comment in
/// `src/web/gate.rs` that explains why the CSRF token does not live in one.
/// Mirrors `identifiers.rs`'s `KNOWN_ENGLISH`.
const KNOWN_ENGLISH: &[&str] = &["base", "meta"];

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

/// Every `.rs` file under `src/web`, found by walking all the way down.
///
/// `ce386e2` is the shape this does not repeat: a fixed, non-recursive list
/// of directories left `src/web` -- five files, 2253 lines -- outside a
/// guard that claimed to cover it, and the suite stayed green through all of
/// it.
fn web_sources() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root().join("src").join("web")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// A word found in `src/web`'s prose, together with where it came from --
/// needed to say which file and which kind of prose on a failure.
struct Found {
    word: String,
    file: String,
    kind: &'static str,
}

/// Splits `text` into runs of three or more letters matching
/// `[A-Za-zÀ-ſ]{3,}`, lowercased. The same tokenising
/// `tools/spanish-vocabulary.py` and `english.rs` use, so a word that would
/// trip either of those trips this guard too.
fn words(text: &str) -> Vec<String> {
    // A numeric range rather than a `'\u{c0}'..='\u{17f}'` char literal: the
    // walker that finds identifiers reads a `\u{..}` escape as a fixed
    // four-character span, so a boundary whose hex digits happen to spell a
    // letter (`c0`) leaks through as a phantom identifier instead of being
    // recognised as part of a literal. `0x27` and `0x5c`, the two escapes
    // already in `literal_spans.rs`, dodge this by luck -- their hex digits
    // spell numbers, not letters. This constant does not rely on the same
    // luck.
    fn is_letter(c: char) -> bool {
        c.is_ascii_alphabetic() || (0xc0..=0x17f).contains(&(c as u32))
    }
    let mut out = Vec::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if is_letter(c) {
            run.push(c);
            continue;
        }
        if run.chars().count() >= 3 {
            out.push(run.to_lowercase());
        }
        run.clear();
    }
    out
}

/// Every word of every string literal and every comment under `src/web`.
///
/// Char literals are skipped, the same way
/// `tools/spanish-vocabulary.py`'s `words_in_literals` skips them: a char
/// literal holds at most one escape, never three letters, so the skip
/// changes nothing except making that plain.
fn words_in_web() -> Vec<Found> {
    let mut out = Vec::new();
    for path in web_sources() {
        let where_at = path
            .strip_prefix(root())
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        let src = std::fs::read_to_string(&path).unwrap();
        let chars: Vec<char> = src.chars().collect();
        let categories: [(&str, Vec<(usize, usize)>); 2] = [
            ("literal", literal_spans::literal_spans(&src)),
            ("comment", literal_spans::comment_spans(&src)),
        ];
        for (kind, spans) in categories {
            for (a, e) in spans {
                if chars[a] == '\'' {
                    continue;
                }
                let text: String = chars[a..e].iter().collect();
                for word in words(&text) {
                    out.push(Found {
                        word,
                        file: where_at.clone(),
                        kind,
                    });
                }
            }
        }
    }
    out
}

/// The prose of `src/web` carries no Spanish.
///
/// Every word of every string literal and every comment, checked against the
/// same deny list `english.rs` reads, minus the HTML/CSS collisions in
/// `KNOWN_ENGLISH`.
#[test]
fn the_web_pages_carry_no_spanish() {
    let spanish = read_list(SPANISH);
    let found = words_in_web();
    let offenders: Vec<&Found> = found
        .iter()
        .filter(|f| spanish.contains(&f.word) && !KNOWN_ENGLISH.contains(&f.word.as_str()))
        .collect();
    assert!(
        offenders.is_empty(),
        "\n  {} Spanish word(s) in src/web's prose:\n\n      {}\n\n  \
         Everything a browser reads is public the same way stdout is. Reword the string\n  \
         or the comment -- or, if it is HTML/CSS vocabulary Spanish happens to share and\n  \
         not Spanish prose, add it to KNOWN_ENGLISH with the reason.\n",
        offenders.len(),
        offenders
            .iter()
            .map(|f| format!("{} ({}, {})", f.word, f.file, f.kind))
            .collect::<Vec<_>>()
            .join("\n      ")
    );
}

/// The exemption list carries nothing stale.
///
/// Every word in `KNOWN_ENGLISH` has to actually be needed: found in
/// `src/web`'s prose, and on the deny list it is excused from. Mirrors the
/// both-directions discipline `identifiers.rs` already applies to its own
/// vocabulary, so a licence cannot outlive the string that needed it.
#[test]
fn known_english_carries_nothing_stale() {
    let spanish = read_list(SPANISH);
    let found: BTreeSet<String> = words_in_web().into_iter().map(|f| f.word).collect();
    let stale: Vec<&str> = KNOWN_ENGLISH
        .iter()
        .filter(|w| !found.contains(**w) || !spanish.contains(**w))
        .copied()
        .collect();
    assert!(
        stale.is_empty(),
        "\n  {} word(s) in KNOWN_ENGLISH that are no longer needed: {}\n\n  \
         Either src/web no longer writes the word, or the deny list no longer bans it.\n  \
         A licence that outlives the string it excused is a hole nobody is watching.\n  \
         Remove the entry.\n",
        stale.len(),
        stale.join(", ")
    );
}
