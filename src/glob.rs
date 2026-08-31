//! Glob matching, just enough for `governs`.
//!
//! `src/auth/**`, `*.rs`, `src/?ain.rs`. No dependencies: the security pillar
//! prefers little to audit, and this is forty lines.

/// Does the pattern cover the path? Slashes are normalized, so a `governs`
/// written on Windows holds on Linux and the other way round.
pub fn covers(patron: &str, file_path: &str) -> bool {
    if patron.is_empty() {
        return false;
    }
    let p: Vec<char> = patron.replace('\\', "/").chars().collect();
    let r: Vec<char> = file_path.replace('\\', "/").chars().collect();
    matches_at(&p, &r)
}

fn matches_at(p: &[char], r: &[char]) -> bool {
    if p.is_empty() {
        return r.is_empty();
    }
    if p[0] == '*' {
        // `**` crosses slashes; `*` stays inside one segment.
        if p.len() > 1 && p[1] == '*' {
            let rest = if p.len() > 2 && p[2] == '/' {
                &p[3..]
            } else {
                &p[2..]
            };
            if rest.is_empty() {
                return true;
            }
            return (0..=r.len()).any(|i| matches_at(rest, &r[i..]));
        }
        let rest = &p[1..];
        let hasta = r.iter().position(|c| *c == '/').unwrap_or(r.len());
        return (0..=hasta).any(|i| matches_at(rest, &r[i..]));
    }
    if r.is_empty() {
        return false;
    }
    if p[0] == '?' && r[0] != '/' {
        return matches_at(&p[1..], &r[1..]);
    }
    p[0] == r[0] && matches_at(&p[1..], &r[1..])
}

#[cfg(test)]
mod tests {
    use super::covers;

    #[test]
    fn literals_and_stars() {
        assert!(covers("src/main.rs", "src/main.rs"));
        assert!(!covers("src/main.rs", "src/other.rs"));
        assert!(covers("src/*.rs", "src/main.rs"));
        assert!(
            !covers("src/*.rs", "src/auth/main.rs"),
            "* does not cross slashes"
        );
        assert!(covers("src/**", "src/auth/token.rs"));
        assert!(covers("src/**/*.rs", "src/auth/token.rs"));
        assert!(covers("**/*.rs", "a/b/c.rs"));
        assert!(covers("src/?ain.rs", "src/main.rs"));
    }

    #[test]
    fn slashes_are_normalized() {
        assert!(covers("src/auth/**", "src\\auth\\token.rs"));
        assert!(covers("src\\auth\\**", "src/auth/token.rs"));
    }

    #[test]
    fn an_empty_pattern_covers_nothing() {
        assert!(!covers("", "src/main.rs"));
    }
}
