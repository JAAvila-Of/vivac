//! The redaction guard. Security pillar, and it holds a veto.
//!
//! This tree is a map of where a system is weak and not yet fixed. What
//! never gets in never leaks. Three things never get in: keys, personal
//! data, and file contents.
//!
//! Of the three, the last is not checked here --it is enforced by having no
//! operation at all that accepts a file body-- so this module covers the
//! first two.
//!
//! **In doubt it refuses and says why. It never stores in silence.**
//! There is no `--force`: if the guard gets it wrong, reword the sentence.
//! A manual escape hatch would be the road by which the very thing this
//! exists to keep out would eventually get in.

/// What the guard found. It never carries the whole secret.
#[derive(Debug)]
pub struct Finding {
    pub rule: &'static str,
    pub field: String,
    pub sample: String,
    pub advice: &'static str,
}

/// Prefixes published by whoever issues the credential. Zero ambiguity: if
/// they show up, it is a key. The number is the minimum count of characters
/// that must follow the prefix for it to count.
const PREFIXES: &[(&str, usize)] = &[
    ("sqa_", 20),
    ("squ_", 20),
    ("sqp_", 20),
    ("ghp_", 30),
    ("gho_", 30),
    ("ghu_", 30),
    ("ghs_", 30),
    ("ghr_", 30),
    ("github_pat_", 20),
    ("glpat-", 15),
    ("sk-ant-", 20),
    ("sk-", 20),
    ("xoxb-", 15),
    ("xoxp-", 15),
    ("xoxa-", 15),
    ("xoxs-", 15),
    ("xapp-", 15),
    ("AIza", 30),
    ("ya29.", 20),
    ("npm_", 30),
    ("dop_v1_", 20),
    ("doo_v1_", 20),
    ("hf_", 30),
    ("sk_live_", 20),
    ("pk_live_", 20),
    ("rk_live_", 20),
    ("SG.", 30),
];

/// AWS access keys: fixed prefix and an exact length of 20.
const AWS: &[&str] = &["AKIA", "ASIA", "AIDA", "AROA", "AGPA", "ANPA", "ANVA"];

const ADVICE_KEY: &str = "Write down which credential it was and where it lives, never \
     its value. E.g.: rotate the CI token, it is in the SONAR_TOKEN secret.";
const ADVICE_PII: &str = "Refer to the role, not to the person nor to their path. \
     E.g.: the PR reviewer, the user home directory.";
const ADVICE_ENTROPY: &str = "If it is not a key, give it a name instead of pasting the \
     value. If it is one, it does not get in: store where it lives, not what it is.";

/// Checks one field. `None` means it may be written.
pub fn check_field(field: &str, text: &str) -> Option<Finding> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY") {
        return Some(Finding {
            rule: "private key in PEM format",
            field: field.to_string(),
            sample: "-----BEGIN ... PRIVATE KEY-----".into(),
            advice: ADVICE_KEY,
        });
    }
    tokens(text).find_map(|tok| check_token(field, tok))
}

/// Checks several fields at once. Returns the first that fails, which is all
/// that is needed: the operation is refused whole.
pub fn check_fields(fields: &[(&str, &str)]) -> Option<Finding> {
    fields.iter().find_map(|(c, t)| check_field(c, t))
}

fn check_token(field: &str, tok: &str) -> Option<Finding> {
    let finding = |rule, advice| {
        Some(Finding {
            rule,
            field: field.to_string(),
            sample: mask(tok),
            advice,
        })
    };

    for (p, min) in PREFIXES {
        if tok.starts_with(p) && tok.len() >= p.len() + min {
            return finding("known credential prefix", ADVICE_KEY);
        }
    }
    if tok.len() == 20
        && AWS.iter().any(|p| tok.starts_with(p))
        && tok
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return finding("AWS access key", ADVICE_KEY);
    }
    if is_jwt(tok) {
        return finding("JSON Web Token", ADVICE_KEY);
    }
    if is_email(tok) {
        return finding("email address (personal data)", ADVICE_PII);
    }
    if is_home_path(tok) {
        return finding("path to a user home directory (personal data)", ADVICE_PII);
    }
    if suspicious_entropy(tok) {
        return finding("high-entropy string with no known shape", ADVICE_ENTROPY);
    }
    None
}

/// Splits on whitespace and on the signs that are never part of a
/// credential. Dashes, dots, slashes and underscores are kept, because they are.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    const SEPARATORS: &[char] = &[
        ',', ';', '"', '\'', '(', ')', '[', ']', '{', '}', '<', '>', '`',
    ];
    const EDGES: &[char] = &['.', ':', '!', '?'];
    text.split(|c: char| c.is_whitespace() || SEPARATORS.contains(&c))
        .map(|t| t.trim_matches(|c: char| EDGES.contains(&c)))
        .filter(|t| !t.is_empty())
}

fn is_jwt(tok: &str) -> bool {
    if !tok.starts_with("eyJ") {
        return false;
    }
    let parts: Vec<&str> = tok.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.len() >= 8) && parts[1].starts_with("eyJ")
}

fn is_email(tok: &str) -> bool {
    let Some((user, domain)) = tok.split_once('@') else {
        return false;
    };
    if user.is_empty() {
        return false;
    }
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    !host.is_empty()
        && (2..=24).contains(&tld.len())
        && tld.bytes().all(|b| b.is_ascii_alphabetic())
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// A home path carries the name of whoever owns it, which is personal data.
/// `~` does not: it resolves on the reader's machine and identifies nobody.
fn is_home_path(tok: &str) -> bool {
    let lower = tok.to_ascii_lowercase().replace('\\', "/");
    ["/users/", "/home/"].iter().any(|p| {
        lower
            .find(p)
            .map(|i| lower[i + p.len()..].split('/').next().unwrap_or("").len() > 1)
            .unwrap_or(false)
    })
}

/// The uncertain heuristic, and for that reason the most conservative of the five.
///
/// Entropy alone does not separate a secret from a long identifier:
/// `PermissionServiceAdapter.cs:278` scores almost the same as a key of the
/// same length. Two more filters are needed, and both came out of a test in
/// this very batch demanding them:
///
/// - **credential alphabet**: credentials are written in base64 or hex, with
///   no dots and no colons. A `File.cs:278` is ruled out by shape alone,
///   without looking at entropy.
/// - **vowel ratio**: in a random string vowels sit around 16 %; in something
///   somebody wrote to be read, 35 % or more. It is the cheapest
///   discriminant that separates `MeetingV2PolicyMaskCalculator` from
///   `Xk7fQ2mZp9RtLw4sVb8N`.
fn suspicious_entropy(tok: &str) -> bool {
    if !(24..=512).contains(&tok.len()) || known_shape(tok) || !credential_alphabet(tok) {
        return false;
    }
    let digit = tok.bytes().any(|b| b.is_ascii_digit());
    let lower = tok.bytes().any(|b| b.is_ascii_lowercase());
    let upper = tok.bytes().any(|b| b.is_ascii_uppercase());
    digit && lower && upper && vowels(tok) < 0.26 && shannon(tok) >= 3.8
}

fn credential_alphabet(tok: &str) -> bool {
    tok.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'=' | b'~'))
}

fn vowels(tok: &str) -> f64 {
    let n = tok.bytes().filter(|b| b.is_ascii_alphabetic()).count();
    if n == 0 {
        return 0.0;
    }
    let v = tok.bytes().filter(|b| b"aeiouAEIOU".contains(b)).count();
    v as f64 / n as f64
}

/// Long random-looking things that are not secrets and turn up constantly in
/// a real provenance tree: SHAs, UUIDs, ULIDs, paths and URLs.
fn known_shape(tok: &str) -> bool {
    let no_dashes: String = tok.chars().filter(|c| *c != '-').collect();
    if !no_dashes.is_empty() && no_dashes.bytes().all(|b| b.is_ascii_hexdigit()) {
        return true; // SHA, checksum, UUID
    }
    if tok.len() == 26
        && tok
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && !b"ilou".contains(&b)))
    {
        return true; // ULID
    }
    tok.contains('/') || tok.contains('\\')
}

fn shannon(s: &str) -> f64 {
    let mut count = [0u32; 256];
    for b in s.as_bytes() {
        count[*b as usize] += 1;
    }
    let n = s.len() as f64;
    -count
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / n;
            p * p.log2()
        })
        .sum::<f64>()
}

/// Shows what it is without reproducing it. It goes to the screen, which is
/// already less bad than storing it, but there is no need to show it whole.
fn mask(tok: &str) -> String {
    let visible: String = tok.chars().take(4).collect();
    format!("{visible}******** ({} characters)", tok.chars().count())
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  Refused: {}\n\n      field   {}\n      found   {}\n\n  {}",
            self.rule, self.field, self.sample, self.advice
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuses(t: &str) -> bool {
        check_field("title", t).is_some()
    }

    #[test]
    fn known_keys() {
        assert!(refuses(
            "the token is sqa_9f3c1d7e5b2a48c6d0e1f2a3b4c5d6e7f8091a2b"
        ));
        assert!(refuses("ghp_16C7e42F292c6912E7710c838347Ae178B4a"));
        assert!(refuses(
            "usar sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345"
        ));
        assert!(refuses("AKIAIOSFODNN7EXAMPLE"));
        assert!(refuses("xoxb-2444-8172-abcdefghijkl"));
        assert!(refuses("-----BEGIN RSA PRIVATE KEY-----"));
    }

    #[test]
    fn jwt() {
        assert!(refuses(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r"
        ));
    }

    #[test]
    fn personal_data() {
        assert!(refuses("preguntarle a alguien@ejemplo.com"));
        assert!(refuses("it lives in C:\\Users\\somename\\projects"));
        assert!(refuses("/home/unnombre/.config/vivac"));
        assert!(refuses("/Users/unnombre/Library"));
    }

    #[test]
    fn what_has_to_get_through() {
        // Ordinary prose from a real tree.
        assert!(!refuses("Port to Rust in the public vivac/ repo"));
        assert!(!refuses(
            "csharpsquid:S1192 literals duplicados entre archivos"
        ));
        assert!(!refuses("PermissionServiceAdapter.cs:278 is out of scope"));
        // Technical references that look random and are not secrets.
        assert!(!refuses("commit e90b4832f1a4c6d8b0e2f4a6c8d0e2f4a6c8d0e2"));
        assert!(!refuses("node 01j8xq2m4k7pabcdefghijklmn"));
        assert!(!refuses("id 550e8400-e29b-41d4-a716-446655440000"));
        assert!(!refuses(
            "ver https://github.com/rust-lang/rust/issues/12345"
        ));
        // The tilde identifies nobody: it resolves on the reading machine.
        assert!(!refuses("the hook lives in ~/.claude/settings.json"));
        // Long camelCase names, the likeliest source of a false positive.
        assert!(!refuses("MeetingPolicyMaskCalculatorFactoryProvider"));
        assert!(!refuses("ApplyVisibilityPolicyPerMeetingHandler"));
        assert!(!refuses("ReunionV2PolicyMaskCalculatorFactory"));
    }

    #[test]
    fn the_entropy_heuristic_still_hunts() {
        // No known prefix: only the shape is left. These have to fall.
        assert!(refuses("Xk7fQ2mZp9RtLw4sVb8NcJ3hGd6y"));
        assert!(refuses("p8KdReQvXnLYtSbGmZwHfJcT3x9Wq2Vz"));
    }

    #[test]
    fn the_sample_does_not_carry_the_secret() {
        let h = check_field("title", "sqa_9f3c1d7e5b2a48c6d0e1f2a3b4c5d6e7f8091a2b").unwrap();
        assert!(!h.sample.contains("9f3c1d7e"));
        assert!(h.sample.starts_with("sqa_"));
    }

    #[test]
    fn it_returns_the_first_field_that_fails() {
        let h = check_fields(&[
            ("title", "all fine"),
            ("why", "ghp_16C7e42F292c6912E7710c838347Ae178B4a"),
        ]);
        assert_eq!(h.unwrap().field, "why");
    }
}
