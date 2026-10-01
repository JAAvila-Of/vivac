//! `f443`: titles are cut and padded by what a terminal shows, not by
//! `char`. A cut by `char` splits a grapheme cluster -- it drops a combining
//! accent, leaves a ZWJ dangling or leaves half a flag -- and a column padded
//! by `char` drifts right after every CJK title, which takes two columns per
//! character.

mod common;
use common::Sandbox;
use unicode_width::UnicodeWidthStr;

const NOW: [&str; 2] = ["--now", "2026-09-15T10:00:00Z"];

/// The spine clips a title at 44 columns and has no spaces to cut back to in
/// these titles, so the cut lands exactly where the test puts it.
fn spine_of(name: &str, title: &str) -> String {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", title, "--why", "to see where the cut falls"]);
    c.ok(&["brief", NOW[0], NOW[1]])
}

fn runs_of_regional_indicators(line: &str) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut run = 0;
    for c in line.chars() {
        if ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) {
            run += 1;
        } else if run > 0 {
            runs.push(run);
            run = 0;
        }
    }
    if run > 0 {
        runs.push(run);
    }
    runs
}

/// Forty letters, then `e` and U+0301: a cut by `char` at 41 lands between
/// them and the accent is lost without a word.
#[test]
fn a_cut_never_drops_a_combining_accent() {
    let title = format!("{}e\u{301}{}", "a".repeat(40), "b".repeat(20));
    let out = spine_of("cut-accent", &title);
    assert!(out.contains("aaaa"), "the title is missing:\n{out}");
    for line in out.lines() {
        assert!(
            !line.contains("e..."),
            "the accent was cut off its letter:\n{line}"
        );
        let chars: Vec<char> = line.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if *c == '\u{301}' {
                assert!(
                    i > 0 && chars[i - 1] == 'e',
                    "a mark that follows nothing it belongs to:\n{line}"
                );
            }
        }
    }
}

/// Thirty-nine letters and a family, which takes two columns: a cut by
/// `char` at 41 keeps the first person and the joiner and drops the rest.
#[test]
fn a_cut_never_leaves_half_an_emoji_family() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let title = format!("{}{family}{}", "a".repeat(39), "b".repeat(20));
    let out = spine_of("cut-family", &title);
    assert!(out.contains("aaaa"), "the title is missing:\n{out}");
    for line in out.lines() {
        assert!(
            !line.contains("\u{200D}...") && !line.ends_with('\u{200D}'),
            "a joiner left dangling:\n{line}"
        );
        if line.contains('\u{200D}') || line.contains('\u{1F468}') {
            assert!(line.contains(family), "half a family:\n{line}");
        }
    }
}

/// Forty letters and the flag of Peru: a cut by `char` at 41 keeps its
/// first regional indicator and drops the second.
#[test]
fn a_cut_never_leaves_half_a_flag() {
    let title = format!("{}\u{1F1F5}\u{1F1EA}{}", "a".repeat(40), "b".repeat(20));
    let out = spine_of("cut-flag", &title);
    assert!(out.contains("aaaa"), "the title is missing:\n{out}");
    for line in out.lines() {
        for run in runs_of_regional_indicators(line) {
            assert_eq!(run % 2, 0, "half a flag:\n{line}");
        }
    }
}

/// `open below` starts at the column the Latin title's line puts it at,
/// even though the Chinese title has far fewer characters than columns.
#[test]
fn columns_line_up_after_a_chinese_title() {
    let c = Sandbox::new_seeded("chinese-columns");
    c.ok(&[
        "add",
        "Connect the database",
        "--type",
        "goal",
        "--why",
        "first",
    ]);
    c.ok(&[
        "add",
        "连接数据库的配置文件",
        "--type",
        "goal",
        "--why",
        "second",
    ]);
    let out = c.ok(&["brief", NOW[0], NOW[1]]);
    let block: Vec<&str> = out
        .lines()
        .skip_while(|l| l.trim() != "OPEN GOALS")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(block.len(), 2, "two goals expected:\n{out}");
    let columns: Vec<usize> = block
        .iter()
        .map(|l| UnicodeWidthStr::width(&l[..l.find("open below").expect("no count")]))
        .collect();
    assert_eq!(
        columns[0],
        columns[1],
        "columns drift:\n{}",
        block.join("\n")
    );
}

/// Forty Chinese characters are eighty columns: the clipped title, ellipsis
/// included, still has to fit the spine's 44.
#[test]
fn a_chinese_title_is_cut_to_columns_not_chars() {
    let title: String = "数据库".chars().cycle().take(40).collect();
    let out = spine_of("chinese-cut", &title);
    let line = out
        .lines()
        .find(|l| l.contains("..."))
        .unwrap_or_else(|| panic!("the title was not clipped:\n{out}"));
    let clipped = &line[line.find("数").unwrap()..];
    let clipped = &clipped[..clipped.find("...").unwrap() + 3];
    assert!(
        UnicodeWidthStr::width(clipped) <= 44,
        "{} columns:\n{clipped}",
        UnicodeWidthStr::width(clipped)
    );
}

/// A note of short CJK words separated by spaces. Without a terminal `why`
/// wraps at `render::WIDTH`, 62 columns, behind an indent the wrap adds on
/// its own, so a line minus its eight-space indent must not exceed 62.
#[test]
fn a_wrapped_chinese_note_stays_inside_the_width_in_columns() {
    let c = Sandbox::new_seeded("chinese-wrap");
    let why = vec!["数据库配置"; 14].join(" ");
    c.ok(&["push", "A goal with a spaced CJK reason", "--why", &why]);
    let out = c.ok(&["why", "1"]);
    let lines: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("        ") && l.contains("数据库配置"))
        .collect();
    assert!(lines.len() >= 2, "the reason was not wrapped:\n{out}");
    for l in lines {
        let w = UnicodeWidthStr::width(l.trim_start());
        assert!(w <= 62, "{w} columns:\n{l}");
    }
}

/// The snippet `find` prints around a hit, out of a reason with no spaces to
/// cut at. The window is `render::WIDTH`, 62 columns, plus the two ellipses.
fn snippet_of(name: &str, why: &str) -> String {
    let c = Sandbox::new_seeded(name);
    c.ok(&["push", "A goal with a long reason", "--why", why]);
    let out = c.ok(&["find", "靶心"]);
    let line = out
        .lines()
        .find(|l| l.contains("why: "))
        .unwrap_or_else(|| panic!("no snippet of the reason:\n{out}"));
    assert!(line.contains("靶心"), "the window lost the hit:\n{out}");
    line[line.find("why: ").unwrap() + 5..].to_string()
}

#[test]
fn a_find_snippet_of_chinese_is_cut_to_columns_not_chars() {
    let why = format!("{}靶心{}", "数".repeat(50), "据".repeat(50));
    let snippet = snippet_of("chinese-snippet", &why);
    let w = UnicodeWidthStr::width(snippet.as_str());
    assert!(w <= 62 + 6, "{w} columns:\n{snippet}");
}

#[test]
fn a_find_snippet_never_leaves_half_a_flag() {
    let flag = "\u{1F1F5}\u{1F1EA}";
    let why = format!("{}靶心{}", flag.repeat(41), flag.repeat(41));
    let snippet = snippet_of("flag-snippet", &why);
    let runs = runs_of_regional_indicators(&snippet);
    assert!(runs.iter().all(|r| r % 2 == 0), "{runs:?}:\n{snippet}");
}
