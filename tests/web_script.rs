//! `map.js` is one function, and inside it `var` belongs to the whole of
//! that function rather than to the block it is written in: two top-level
//! declarations of the same name are one variable, and whichever assignment
//! runs last is what every closure that captured the first one sees.
//!
//! That is how "Where am I?" stopped doing anything. The button and the
//! search's list of hits on the page were both `here`; the page finished
//! loading by assigning the list, so by the time anyone clicked, the
//! button's handler was reading `dataset` off an array and threw before it
//! could move.

use std::collections::BTreeMap;

const MAP_JS: &str = include_str!("../src/web/map.js");
const RENDER_RS: &str = include_str!("../src/render.rs");

/// Two spaces in is the top level of the one function the file is.
#[test]
fn map_js_declares_no_top_level_name_twice() {
    let mut first_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut twice = Vec::new();
    for (i, line) in MAP_JS.lines().enumerate() {
        let Some(rest) = line.strip_prefix("  var ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        match first_seen.get(&name) {
            Some(first) => twice.push(format!("{name}: lines {first} and {}", i + 1)),
            None => {
                first_seen.insert(name, i + 1);
            }
        }
    }
    assert!(
        first_seen.len() >= 10,
        "only {} top-level declarations found: what broke is the parsing here, not map.js",
        first_seen.len()
    );
    assert!(
        twice.is_empty(),
        "map.js declares a top-level name twice, and the two are one variable:\n  {}",
        twice.join("\n  ")
    );
}

/// The `start-end` pairs of `map.js`'s `diacritics` character class, read
/// as codepoints. The class is written in `\uXXXX` escapes and has to stay
/// that way: a combining mark written literally is invisible on the page,
/// and an editor that normalizes what it saves can rewrite it without a
/// diff anyone would notice. So the line has to be plain ASCII, and every
/// range is two escapes joined by a `-`.
fn js_diacritic_ranges() -> Vec<(u32, u32)> {
    let line = MAP_JS
        .lines()
        .find(|l| l.contains("var diacritics"))
        .unwrap_or_else(|| panic!("no `var diacritics` line found in map.js"));
    assert!(
        line.is_ascii(),
        "map.js writes a diacritic literally in its `diacritics` regex; \
         write it as a \\u escape:\n{line}"
    );
    let inner = line
        .split('[')
        .nth(1)
        .and_then(|s| s.split(']').next())
        .unwrap_or_else(|| panic!("the diacritics line has no `[...]` character class:\n{line}"));
    let escape = |s: &str| -> Option<u32> {
        let hex = s.strip_prefix("\\u")?;
        (hex.len() == 4)
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten()
    };
    let mut ranges = Vec::new();
    let mut rest = inner;
    while !rest.is_empty() {
        let (low, high) = (rest.get(..6), rest.get(7..13));
        let pair = (rest.get(6..7) == Some("-"))
            .then(|| Some((escape(low?)?, escape(high?)?)))
            .flatten();
        let Some(pair) = pair else {
            panic!("the diacritics class is not a run of `\\uXXXX-\\uXXXX` ranges at: {rest}");
        };
        ranges.push(pair);
        rest = &rest[13..];
    }
    ranges
}

/// The hex digits starting at `start`, however many there are: enough to
/// read `0x036F` as `036F` without assuming every range in `is_diacritic`
/// stays four digits wide forever.
fn hex_at(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    let digits = std::str::from_utf8(&bytes[start..i]).ok()?;
    u32::from_str_radix(digits, 16).ok().map(|v| (v, i))
}

/// The `0x????..=0x????` ranges written inside `fn is_diacritic` in
/// `render.rs` -- the Rust half of the fold. An `include_str!` read rather
/// than a call into the crate: this is an integration test, and
/// `is_diacritic` is private.
fn rust_diacritic_ranges() -> Vec<(u32, u32)> {
    let start = RENDER_RS
        .find("fn is_diacritic")
        .unwrap_or_else(|| panic!("is_diacritic not found in render.rs"));
    let rest = &RENDER_RS[start..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("is_diacritic's closing brace not found"));
    let body = &rest[..end];
    let bytes = body.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if &bytes[i..i + 2] == b"0x" {
            if let Some((low, after)) = hex_at(bytes, i + 2) {
                if body[after..].starts_with("..=0x") {
                    if let Some((high, after_high)) = hex_at(bytes, after + 5) {
                        ranges.push((low, high));
                        i = after_high;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    ranges
}

/// `f577`'s second half: the web's find box has to fold diacritics away
/// exactly like the CLI's `find` does, or `dueno` reaches half the tree
/// depending on which face asked. The two lists of ranges are hand-written
/// in two different languages, which is exactly the pair that drifts the
/// day only one of them gets a block added -- so this reads both back out
/// of the source rather than trusting a comment that says they agree.
#[test]
fn the_web_and_the_cli_fold_the_same_diacritics() {
    let mut web = js_diacritic_ranges();
    let mut cli = rust_diacritic_ranges();
    assert!(
        !web.is_empty(),
        "found no diacritic ranges in map.js's `diacritics` regex -- \
         the parser broke, not the ranges"
    );
    assert!(
        !cli.is_empty(),
        "found no diacritic ranges in render.rs's `is_diacritic` -- \
         the parser broke, not the ranges"
    );
    web.sort_unstable();
    cli.sort_unstable();
    assert_eq!(
        web, cli,
        "\n  the web and the CLI fold different diacritics:\n  \
         map.js:    {web:?}\n  render.rs: {cli:?}\n"
    );
}
