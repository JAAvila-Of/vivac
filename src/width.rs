//! What a terminal shows, as opposed to what `char` counts (`f443`).
//!
//! A title is cut, padded and wrapped by the columns it takes, and it is cut
//! only between extended grapheme clusters: a cut inside one drops an accent
//! or leaves half a flag. No cluster is ever counted wider than two columns,
//! which keeps a ZWJ sequence or a flag at the two a terminal gives it.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The most columns one grapheme cluster is counted as taking.
const CLUSTER_MAX: usize = 2;

/// The columns one grapheme cluster takes.
fn cluster_width(cluster: &str) -> usize {
    UnicodeWidthStr::width(cluster).min(CLUSTER_MAX)
}

/// The columns `s` takes in a terminal.
pub(crate) fn width(s: &str) -> usize {
    s.graphemes(true).map(cluster_width).sum()
}

/// `s` followed by spaces up to `columns` columns; `s` itself when it already
/// takes that many or more.
pub(crate) fn pad(s: &str, columns: usize) -> String {
    format!("{s}{}", " ".repeat(columns.saturating_sub(width(s))))
}

/// The longest prefix of `s` made of whole grapheme clusters that takes at
/// most `columns` columns.
pub(crate) fn take(s: &str, columns: usize) -> &str {
    let mut used = 0;
    for (at, cluster) in s.grapheme_indices(true) {
        used += cluster_width(cluster);
        if used > columns {
            return &s[..at];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_takes_a_column_a_character() {
        assert_eq!(width("hello"), 5);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcd", 2), "abcd");
        assert_eq!(take("hello", 3), "hel");
        assert_eq!(take("hello", 9), "hello");
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        assert_eq!(width("数据"), 4);
        assert_eq!(take("数据库", 5), "数据");
        assert_eq!(take("数据库", 1), "");
    }

    #[test]
    fn a_cluster_is_never_split_and_never_wider_than_two() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let flag = "\u{1F1F5}\u{1F1EA}";
        assert_eq!(width(family), 2);
        assert_eq!(width(flag), 2);
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(take(&format!("a{family}"), 2), "a");
        assert_eq!(take(&format!("a{flag}"), 3), format!("a{flag}"));
        assert_eq!(take("ae\u{301}", 1), "a");
    }
}
