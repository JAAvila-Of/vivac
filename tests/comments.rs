//! What a stranger reads in the margins: the comments.
//!
//! `english.rs` guards what the binary prints and `identifiers.rs` guards the
//! names in the code, and that one strips the comments before it looks. So
//! nothing read the comments, and Spanish got in: literal quotations of
//! internal documents, in the public source, with the suite green.
//!
//! **The list is an allow-list**, for the reason `identifiers.rs` gives: the
//! rule is "only English", not "no Spanish", and a list of the Spanish words
//! somebody already caught goes green over the next one. Every word of every
//! comment in every tracked file has to appear in
//! `tests/data/comment-vocabulary.txt`, and a word gets in by being written
//! down on purpose.
//!
//! It is a separate file from the identifier vocabulary on purpose. Prose
//! needs `the` and `because`, names do not, and one shared list would let a
//! word that is fine in a sentence license a name in the code.
//!
//! Code in a comment is not prose: a backtick span is removed before the
//! words are read, so test data a comment describes goes in backticks.

#[path = "common/literal_spans.rs"]
mod literal_spans;

#[path = "common/comment_text.rs"]
mod comment_text;

use std::collections::{BTreeMap, BTreeSet};

const VOCABULARY: &str = "tests/data/comment-vocabulary.txt";
const SPANISH: &str = "tests/data/spanish-vocabulary.txt";
/// `base` is ordinary English in the comments -- a base name, base64, the
/// `base` of a path -- that also spells the Spanish word, and `identifiers.rs`
/// carries it for the same reason.
///
/// `todo` is the English word, in the docstring of `tools/check-commits.py`
/// that explains why the commit-message guard once read it as Spanish.
const KNOWN_ENGLISH: &[&str] = &["base", "todo"];

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

/// The comment spans of `src`, by the kind of file `path` names, or `None`
/// for a file whose comments this guard does not read.
fn spans_for(path: &str, src: &str) -> Option<Vec<comment_text::Span>> {
    let name_after = |dir: &str| path.strip_prefix(dir).filter(|rest| !rest.contains('/'));
    if path.ends_with(".rs") {
        Some(comment_text::rust_spans(src))
    } else if name_after("src/web/").is_some_and(|n| n.ends_with(".js")) {
        Some(comment_text::js_spans(src))
    } else if name_after("src/web/").is_some_and(|n| n.ends_with(".css")) {
        Some(comment_text::css_spans(src))
    } else if name_after("tools/").is_some_and(|n| n.ends_with(".py")) {
        Some(comment_text::py_spans(src))
    } else if name_after(".github/workflows/").is_some_and(|n| n.ends_with(".yml")) {
        Some(comment_text::yaml_spans(src))
    } else {
        None
    }
}

/// Every file git tracks. Build output and untracked files never count, which
/// is why this asks git instead of walking the directory.
fn tracked() -> Vec<String> {
    let run = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root())
        .output()
        .expect("this guard asks `git ls-files` which files are tracked, and git did not run");
    assert!(
        run.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout)
        .unwrap()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every word used in a comment anywhere the crate publishes, each one with
/// the first file that uses it, so a failure says where to look.
fn words_in_comments() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for path in tracked() {
        let Ok(src) = std::fs::read_to_string(root().join(&path)) else {
            continue;
        };
        let Some(spans) = spans_for(&path, &src) else {
            continue;
        };
        for block in comment_text::comment_blocks(&src, &spans) {
            for word in comment_text::comment_words(&block) {
                out.entry(word).or_insert_with(|| path.clone());
            }
        }
    }
    out
}

/// The vocabulary has to match the tree **exactly**, in both directions, for
/// the reason `identifiers.rs` gives: a word left in the file after the last
/// comment that used it is gone is a licence waiting for whoever reaches for
/// it next.
#[test]
fn every_comment_reads_as_english() {
    let used = words_in_comments();
    let listed = read_list(VOCABULARY);
    let missing: Vec<(&String, &String)> =
        used.iter().filter(|(w, _)| !listed.contains(*w)).collect();
    let surplus: Vec<&String> = listed.iter().filter(|w| !used.contains_key(*w)).collect();
    if missing.is_empty() && surplus.is_empty() {
        return;
    }
    let mut msg = String::from("\n  The comment vocabulary does not match the tree.\n\n");
    if !missing.is_empty() {
        msg.push_str(&format!(
            "  {} word(s) used in a comment and not in the file:\n",
            missing.len()
        ));
        for (w, file) in &missing {
            msg.push_str(&format!("      {w:<24} {file}\n"));
        }
        msg.push('\n');
    }
    if !surplus.is_empty() {
        msg.push_str(&format!(
            "  {} word(s) in the file that no comment uses any more:\n      {}\n\n",
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
         so a word that is not gets the comment rewritten rather than a line added.\n  \
         `python tools/comment-vocabulary.py` rewrites the file from the block below.\n\n",
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
fn the_comment_vocabulary_carries_no_spanish() {
    let spanish = read_list(SPANISH);
    let blessed: Vec<String> = read_list(VOCABULARY)
        .into_iter()
        .filter(|w| spanish.contains(w) && !KNOWN_ENGLISH.contains(&w.as_str()))
        .collect();
    assert!(
        blessed.is_empty(),
        "\n  The comment vocabulary blesses {} word(s) the output guard bans:\n\n      {}\n\n  \
         A guard does not get to widen its own list. Rewrite the comment -- or, if the\n  \
         word really is English where it appears, say so in KNOWN_ENGLISH with the reason.\n",
        blessed.len(),
        blessed.join("\n      ")
    );
}

/// An exception nothing uses any more is a licence waiting for the next
/// Spanish word that happens to spell it.
#[test]
fn known_english_carries_nothing_stale() {
    let vocabulary = read_list(VOCABULARY);
    let stale: Vec<&str> = KNOWN_ENGLISH
        .iter()
        .copied()
        .filter(|w| !vocabulary.contains(*w))
        .collect();
    assert!(
        stale.is_empty(),
        "\n  KNOWN_ENGLISH excuses {} word(s) no comment uses any more: {}. Drop them.\n",
        stale.len(),
        stale.join(", ")
    );
}

#[test]
fn the_comment_vocabulary_is_sorted_and_says_nothing_twice() {
    let raw = std::fs::read_to_string(root().join(VOCABULARY)).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut tidy = lines.clone();
    tidy.sort_unstable();
    tidy.dedup();
    assert_eq!(
        lines, tidy,
        "\n  {VOCABULARY} is out of order or repeats itself. A list nobody can scan is a\n  \
         list nobody checks. `python tools/comment-vocabulary.py` rewrites it.\n"
    );
}

fn blocks_of(spans: Vec<comment_text::Span>, src: &str) -> Vec<String> {
    comment_text::comment_blocks(src, &spans)
}

#[test]
fn a_rust_marker_inside_a_string_is_not_a_comment() {
    let src = r#"let a = "// not one"; // one
let b = '/'; /* two */ let c = r"/* not */";"#;
    assert_eq!(
        blocks_of(comment_text::rust_spans(src), src),
        vec!["// one", "/* two */"]
    );
}

#[test]
fn consecutive_comment_lines_are_one_block() {
    let src = "/// a `split\n/// span` here\nlet x = 1;\n";
    let blocks = blocks_of(comment_text::rust_spans(src), src);
    assert_eq!(blocks, vec!["/// a `split\n/// span` here"]);
    let words: Vec<String> = comment_text::comment_words(&blocks[0])
        .into_iter()
        .collect();
    assert_eq!(words, vec!["a", "here"]);
}

#[test]
fn backtick_spans_and_urls_are_not_prose() {
    let words = comment_text::comment_words("see `dueño` at https://example.org/ñ now");
    assert_eq!(
        words.into_iter().collect::<Vec<_>>(),
        vec!["at", "now", "see"]
    );
}

#[test]
fn words_are_runs_of_letters_and_unicode_letters_count() {
    let words = comment_text::comment_words("Dueño2árbol, o'clock");
    assert_eq!(
        words.into_iter().collect::<Vec<_>>(),
        vec!["clock", "dueño", "o", "árbol"]
    );
}

#[test]
fn a_backtick_that_never_closes_hides_nothing() {
    let words = comment_text::comment_words("it opens ` and never closes");
    assert!(words.contains("closes"));
}

#[test]
fn a_javascript_marker_inside_any_string_is_not_a_comment() {
    let src = "var a = \"// no\"; var b = '/* no */'; // yes\nvar c = `x ${ \"//no\" + `// no ${ 1 }` } // no`; /* two */\n";
    assert_eq!(
        blocks_of(comment_text::js_spans(src), src),
        vec!["// yes", "/* two */"]
    );
}

#[test]
fn a_javascript_regex_literal_opens_and_closes_nothing() {
    let src = "s.replace(/[&<>\"]/g, f); // one\nvar r = x && /^#\\/'/.test(y); // two\nreturn /\"[/]/; // three\n";
    assert_eq!(
        blocks_of(comment_text::js_spans(src), src),
        vec!["// one", "// two", "// three"]
    );
}

#[test]
fn a_javascript_slash_after_a_value_divides() {
    let src = "var a = b / c; var d = (e) / 2; // real\n";
    assert_eq!(blocks_of(comment_text::js_spans(src), src), vec!["// real"]);
}

#[test]
fn a_css_marker_inside_a_string_is_not_a_comment() {
    let src = "a::after { content: \"/* no */\"; } /* yes */\nb { content: '/*'; } /* two */";
    assert_eq!(
        blocks_of(comment_text::css_spans(src), src),
        vec!["/* yes */", "/* two */"]
    );
}

#[test]
fn a_python_hash_inside_a_string_is_not_a_comment() {
    let src = "a = \"# no\"\nb = '# no'  # yes\nc = \"it's\"  # two\n";
    assert_eq!(
        blocks_of(comment_text::py_spans(src), src),
        vec!["# yes", "# two"]
    );
}

#[test]
fn a_python_docstring_is_a_comment_and_a_one_line_string_is_not() {
    let src = "\"\"\"Module prose.\n\n# not a second one\n\"\"\"\nx = \"data\"\ndef f():\n    '''Doc.'''\n";
    let blocks = blocks_of(comment_text::py_spans(src), src);
    assert_eq!(blocks.len(), 2);
    assert!(blocks[0].contains("Module prose."));
    assert!(blocks[0].contains("not a second one"));
    assert_eq!(blocks[1], "'''Doc.'''");
}

#[test]
fn a_yaml_hash_counts_after_whitespace_and_not_inside_quotes() {
    let src = "# top\nname: \"a # b\" # one\nother: 'c # d' # two\nurl: a#b\nit: don't # three\n";
    assert_eq!(
        blocks_of(comment_text::yaml_spans(src), src),
        vec!["# top", "# one", "# two", "# three"]
    );
}
