//! The one place that knows where a literal starts and ends in Rust source.
//!
//! `identifiers.rs` strips every string and char literal to find the names
//! left behind. `web_prose.rs` reads the very same spans to find the words
//! inside them. Both need the same answer to "where does this literal end":
//! a raw string counts its hashes, an escaped quote does not close a string,
//! and a `'` that opens a char literal has to be told apart from the one
//! that opens a lifetime. That parsing lives here once, mirroring
//! `tools/spanish-vocabulary.py`'s own `literal_spans` on the Python side of
//! the same wall -- the reason that script gives for keeping exactly one is
//! the reason this file exists.
//!
//! Not `pub` through `tests/common/mod.rs`: the sixteen files that pull in
//! `Sandbox` never touch this, and folding it into that module would leave
//! every one of them carrying two unused functions. Included instead with
//! `#[path = "common/literal_spans.rs"] mod literal_spans;`, by the two
//! files that actually read Rust source.

/// (start, end) char-index spans of every string and char literal in `src`.
///
/// A comment is skipped whole while scanning and never turns into a span
/// here, so a quote written in prose inside a `//` line can never be
/// mistaken for the start of one.
pub fn literal_spans(src: &str) -> Vec<(usize, usize)> {
    let b: Vec<char> = src.chars().collect();
    let quote = '\u{27}';
    let backslash = '\u{5c}';
    let mut out = Vec::new();
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
            let start = i;
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
                out.push((start, j));
                i = j;
            } else {
                i += 1;
            }
        } else if c == '"' {
            let start = i;
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
            out.push((start, i));
        } else if c == quote {
            // A char literal has a closing quote; a lifetime does not.
            let escaped = b.get(i + 1) == Some(&backslash);
            let closes = if escaped {
                b.get(i + 3) == Some(&quote)
            } else {
                b.get(i + 2) == Some(&quote)
            };
            if closes {
                let end = i + if escaped { 4 } else { 3 };
                out.push((i, end));
                i = end;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// (start, end) char-index spans of every `//` and `/* ... */` comment in
/// `src`.
///
/// Built on [`literal_spans`] rather than re-parsing quotes: everything
/// inside a literal is skipped whole, so a `//` written inside a string can
/// never be mistaken for the start of a comment.
pub fn comment_spans(src: &str) -> Vec<(usize, usize)> {
    let b: Vec<char> = src.chars().collect();
    let mut in_literal = vec![false; b.len()];
    for (a, e) in literal_spans(src) {
        for slot in &mut in_literal[a..e] {
            *slot = true;
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if in_literal[i] {
            i += 1;
            continue;
        }
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            let start = i;
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            out.push((start, i));
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            let start = i;
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            out.push((start, i));
        } else {
            i += 1;
        }
    }
    out
}
