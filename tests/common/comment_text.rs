//! Where the comments are in each kind of file the crate publishes, and the
//! words inside them.
//!
//! `literal_spans.rs` knows Rust. The other four kinds of file -- JavaScript,
//! CSS, the Python tools and the workflow YAML -- each get one small scanner
//! here, and every scanner answers the same question the Rust one does: which
//! (start, end) char spans are comment, with a comment marker inside a string
//! never counted. They return spans rather than text so that
//! [`comment_blocks`] can join the consecutive lines of one paragraph before
//! anybody looks at the words, which is what keeps a backtick span that is
//! split across two `///` lines in one piece.
//!
//! Included with `#[path = "common/comment_text.rs"] mod comment_text;`
//! next to `literal_spans`, by the one file that reads comments.

use super::literal_spans;
use std::collections::BTreeSet;

pub type Span = (usize, usize);

const QUOTE: char = '\u{27}';
const BACKSLASH: char = '\u{5c}';

/// Every comment span of a Rust file.
pub fn rust_spans(src: &str) -> Vec<Span> {
    literal_spans::comment_spans(src)
}

struct Script<'a> {
    chars: &'a [char],
    spans: Vec<Span>,
}

impl Script<'_> {
    /// Scans code from `i`. When `nested`, it stops after the `}` that closes
    /// a template literal's `${`, and returns where it stopped.
    fn code(&mut self, mut i: usize, nested: bool) -> usize {
        let len = self.chars.len();
        let mut seen = String::new();
        let mut depth = 0usize;
        while i < len {
            let c = self.chars[i];
            let next = self.chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                let start = i;
                while i < len && self.chars[i] != '\n' {
                    i += 1;
                }
                self.spans.push((start, i));
            } else if c == '/' && next == Some('*') {
                let start = i;
                i += 2;
                while i + 1 < len && !(self.chars[i] == '*' && self.chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(len);
                self.spans.push((start, i));
            } else if c == '"' || c == QUOTE {
                i = self.quoted(i, c);
                seen.push('x');
            } else if c == '`' {
                i = self.template(i + 1);
                seen.push('x');
            } else if c == '/' {
                if starts_regex(&seen) {
                    i = self.regex(i);
                    seen.push('x');
                } else {
                    seen.push('/');
                    i += 1;
                }
            } else if c == '{' {
                depth += 1;
                seen.push(c);
                i += 1;
            } else if c == '}' {
                if nested && depth == 0 {
                    return i + 1;
                }
                depth = depth.saturating_sub(1);
                seen.push(c);
                i += 1;
            } else if c.is_whitespace() {
                if !seen.ends_with(' ') {
                    seen.push(' ');
                }
                i += 1;
            } else {
                seen.push(c);
                i += 1;
            }
        }
        i
    }

    fn quoted(&self, mut i: usize, quote: char) -> usize {
        i += 1;
        while i < self.chars.len() {
            match self.chars[i] {
                BACKSLASH => i += 2,
                '\n' => return i,
                c if c == quote => return i + 1,
                _ => i += 1,
            }
        }
        i
    }

    fn regex(&self, mut i: usize) -> usize {
        i += 1;
        let mut in_class = false;
        while i < self.chars.len() {
            match self.chars[i] {
                BACKSLASH => i += 2,
                '\n' => return i,
                '[' => {
                    in_class = true;
                    i += 1;
                }
                ']' => {
                    in_class = false;
                    i += 1;
                }
                '/' if !in_class => return i + 1,
                _ => i += 1,
            }
        }
        i
    }

    fn template(&mut self, mut i: usize) -> usize {
        while i < self.chars.len() {
            match self.chars[i] {
                BACKSLASH => i += 2,
                '`' => return i + 1,
                '$' if self.chars.get(i + 1) == Some(&'{') => i = self.code(i + 2, true),
                _ => i += 1,
            }
        }
        i
    }
}

/// Whether a `/` after `seen` opens a regex literal rather than dividing.
fn starts_regex(seen: &str) -> bool {
    let before = seen.trim_end();
    let Some(last) = before.chars().last() else {
        return true;
    };
    if "(,=:[!&|?{};".contains(last) {
        return true;
    }
    before.strip_suffix("return").is_some_and(|rest| {
        !rest
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
    })
}

/// Every `//` and `/* */` comment span of a JavaScript file, outside strings,
/// template literals (with their nested `${}`) and regex literals.
pub fn js_spans(src: &str) -> Vec<Span> {
    let chars: Vec<char> = src.chars().collect();
    let mut script = Script {
        chars: &chars,
        spans: Vec::new(),
    };
    script.code(0, false);
    script.spans
}

/// Every `/* */` comment span of a CSS file, outside quoted strings.
pub fn css_spans(src: &str) -> Vec<Span> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            out.push((start, i));
        } else if c == '"' || c == QUOTE {
            i += 1;
            while i < chars.len() {
                if chars[i] == BACKSLASH {
                    i += 2;
                } else if chars[i] == '\n' {
                    break;
                } else if chars[i] == c {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Every `#` comment and every triple-quoted string of a Python file. In
/// the tools the triple-quoted ones are docstrings, which are prose; the
/// one-line strings are data and are skipped.
pub fn py_spans(src: &str) -> Vec<Span> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push((start, i));
        } else if c == '"' || c == QUOTE {
            if chars.get(i + 1) == Some(&c) && chars.get(i + 2) == Some(&c) {
                let start = i;
                i += 3;
                while i < chars.len() {
                    if chars[i] == BACKSLASH {
                        i += 2;
                    } else if chars[i] == c
                        && chars.get(i + 1) == Some(&c)
                        && chars.get(i + 2) == Some(&c)
                    {
                        i += 3;
                        break;
                    } else {
                        i += 1;
                    }
                }
                out.push((start, i.min(chars.len())));
            } else {
                i += 1;
                while i < chars.len() {
                    if chars[i] == BACKSLASH {
                        i += 2;
                    } else if chars[i] == '\n' {
                        break;
                    } else if chars[i] == c {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Every `#` comment span of a YAML file: a `#` at the start of a line or
/// after whitespace, outside quoted scalars. A quote opens a scalar only at
/// the start of a token, so an apostrophe inside a plain word does not.
pub fn yaml_spans(src: &str) -> Vec<Span> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let token_start = i == 0 || chars[i - 1].is_whitespace() || "[{,".contains(chars[i - 1]);
        if c == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push((start, i));
        } else if c == '"' && token_start {
            i += 1;
            while i < chars.len() {
                if chars[i] == BACKSLASH {
                    i += 2;
                } else if chars[i] == '"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if c == QUOTE && token_start {
            i += 1;
            while i < chars.len() {
                if chars[i] == QUOTE && chars.get(i + 1) == Some(&QUOTE) {
                    i += 2;
                } else if chars[i] == QUOTE {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The text of each span, with spans on consecutive lines joined into one
/// block: only whitespace between them, and exactly one line break.
pub fn comment_blocks(src: &str, spans: &[Span]) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut previous_end: Option<usize> = None;
    for &(a, e) in spans {
        let text: String = chars[a..e].iter().collect();
        let joins = previous_end.is_some_and(|p| {
            let gap = &chars[p..a];
            gap.iter().all(|c| c.is_whitespace()) && gap.iter().filter(|c| **c == '\n').count() == 1
        });
        match out.last_mut() {
            Some(last) if joins => {
                last.push('\n');
                last.push_str(&text);
            }
            _ => out.push(text),
        }
        previous_end = Some(e);
    }
    out
}

/// The English-or-not words of one block of comment: backtick code spans and
/// URLs removed first, then every maximal run of letters, lower-cased.
/// Quotations in ordinary quotes stay in, because quotations are how the
/// Spanish got in.
pub fn comment_words(text: &str) -> BTreeSet<String> {
    let mut prose = String::new();
    let mut code = String::new();
    let mut in_code = false;
    for c in text.chars() {
        if c == '`' {
            in_code = !in_code;
            code.clear();
        } else if in_code {
            code.push(c);
        } else {
            prose.push(c);
        }
    }
    // A backtick that never closes opened nothing: what follows it is prose,
    // and dropping it would hide words instead of reading them.
    if in_code {
        prose.push_str(&code);
    }
    let mut bare = String::new();
    let mut rest = prose.as_str();
    while let Some(at) = ["http://", "https://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        bare.push_str(&rest[..at]);
        let tail = &rest[at..];
        rest = tail.find(char::is_whitespace).map_or("", |n| &tail[n..]);
    }
    bare.push_str(rest);
    bare.split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}
