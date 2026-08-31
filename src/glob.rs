//! Glob matching, just enough for `governs`.
//!
//! `src/auth/**`, `*.rs`, `src/?ain.rs`. No dependencies: the security pillar
//! prefers little to audit, and this is forty lines.

/// Does the pattern cover the path? Slashes are normalized, so a `governs`
/// written on Windows holds on Linux and the other way round.
pub fn cubre(patron: &str, ruta: &str) -> bool {
    if patron.is_empty() {
        return false;
    }
    let p: Vec<char> = patron.replace('\\', "/").chars().collect();
    let r: Vec<char> = ruta.replace('\\', "/").chars().collect();
    casa(&p, &r)
}

fn casa(p: &[char], r: &[char]) -> bool {
    if p.is_empty() {
        return r.is_empty();
    }
    if p[0] == '*' {
        // `**` crosses slashes; `*` stays inside one segment.
        if p.len() > 1 && p[1] == '*' {
            let resto = if p.len() > 2 && p[2] == '/' {
                &p[3..]
            } else {
                &p[2..]
            };
            if resto.is_empty() {
                return true;
            }
            return (0..=r.len()).any(|i| casa(resto, &r[i..]));
        }
        let resto = &p[1..];
        let hasta = r.iter().position(|c| *c == '/').unwrap_or(r.len());
        return (0..=hasta).any(|i| casa(resto, &r[i..]));
    }
    if r.is_empty() {
        return false;
    }
    if p[0] == '?' && r[0] != '/' {
        return casa(&p[1..], &r[1..]);
    }
    p[0] == r[0] && casa(&p[1..], &r[1..])
}

#[cfg(test)]
mod tests {
    use super::cubre;

    #[test]
    fn literales_y_estrellas() {
        assert!(cubre("src/main.rs", "src/main.rs"));
        assert!(!cubre("src/main.rs", "src/other.rs"));
        assert!(cubre("src/*.rs", "src/main.rs"));
        assert!(!cubre("src/*.rs", "src/auth/main.rs"), "* does not cross slashes");
        assert!(cubre("src/**", "src/auth/token.rs"));
        assert!(cubre("src/**/*.rs", "src/auth/token.rs"));
        assert!(cubre("**/*.rs", "a/b/c.rs"));
        assert!(cubre("src/?ain.rs", "src/main.rs"));
    }

    #[test]
    fn las_barras_se_normalizan() {
        assert!(cubre("src/auth/**", "src\\auth\\token.rs"));
        assert!(cubre("src\\auth\\**", "src/auth/token.rs"));
    }

    #[test]
    fn nada_cubre_con_patron_vacio() {
        assert!(!cubre("", "src/main.rs"));
    }
}
