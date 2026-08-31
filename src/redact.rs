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
pub struct Hallazgo {
    pub regla: &'static str,
    pub campo: String,
    pub muestra: String,
    pub consejo: &'static str,
}

/// Prefixes published by whoever issues the credential. Zero ambiguity: if
/// they show up, it is a key. The number is the minimum count of characters
/// that must follow the prefix for it to count.
const PREFIJOS: &[(&str, usize)] = &[
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

const CONSEJO_CLAVE: &str = "Escribi que credencial era y donde vive, nunca su valor. \
     Ej: rotar el token de CI, esta en el secreto SONAR_TOKEN.";
const CONSEJO_PII: &str = "Referencia el rol, no a la persona ni su ruta. \
     Ej: el revisor del PR, la carpeta del usuario.";
const CONSEJO_ENTROPIA: &str = "Si no es una clave, dale un nombre en vez de pegar el valor. \
     Si lo es, no entra: guarda donde vive, no cual es.";

/// Checks one field. `None` means it may be written.
pub fn revisar(campo: &str, texto: &str) -> Option<Hallazgo> {
    if texto.contains("-----BEGIN") && texto.contains("PRIVATE KEY") {
        return Some(Hallazgo {
            regla: "private key in PEM format",
            campo: campo.to_string(),
            muestra: "-----BEGIN ... PRIVATE KEY-----".into(),
            consejo: CONSEJO_CLAVE,
        });
    }
    tokens(texto).find_map(|tok| revisar_token(campo, tok))
}

/// Checks several fields at once. Returns the first that fails, which is all
/// that is needed: the operation is refused whole.
pub fn revisar_campos(campos: &[(&str, &str)]) -> Option<Hallazgo> {
    campos.iter().find_map(|(c, t)| revisar(c, t))
}

fn revisar_token(campo: &str, tok: &str) -> Option<Hallazgo> {
    let hallazgo = |regla, consejo| {
        Some(Hallazgo {
            regla,
            campo: campo.to_string(),
            muestra: enmascarar(tok),
            consejo,
        })
    };

    for (p, min) in PREFIJOS {
        if tok.starts_with(p) && tok.len() >= p.len() + min {
            return hallazgo("prefijo de credencial conocido", CONSEJO_CLAVE);
        }
    }
    if tok.len() == 20
        && AWS.iter().any(|p| tok.starts_with(p))
        && tok
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return hallazgo("clave de acceso de AWS", CONSEJO_CLAVE);
    }
    if es_jwt(tok) {
        return hallazgo("JSON Web Token", CONSEJO_CLAVE);
    }
    if es_correo(tok) {
        return hallazgo("direccion de correo (dato personal)", CONSEJO_PII);
    }
    if es_ruta_de_casa(tok) {
        return hallazgo(
            "ruta del directorio de un usuario (dato personal)",
            CONSEJO_PII,
        );
    }
    if entropia_sospechosa(tok) {
        return hallazgo(
            "high-entropy string with no known shape",
            CONSEJO_ENTROPIA,
        );
    }
    None
}

/// Splits on whitespace and on the signs that are never part of a
/// credential. Dashes, dots, slashes and underscores are kept, because they are.
fn tokens(texto: &str) -> impl Iterator<Item = &str> {
    const CORTES: &[char] = &[
        ',', ';', '"', '\'', '(', ')', '[', ']', '{', '}', '<', '>', '`',
    ];
    const BORDES: &[char] = &['.', ':', '!', '?'];
    texto
        .split(|c: char| c.is_whitespace() || CORTES.contains(&c))
        .map(|t| t.trim_matches(|c: char| BORDES.contains(&c)))
        .filter(|t| !t.is_empty())
}

fn es_jwt(tok: &str) -> bool {
    if !tok.starts_with("eyJ") {
        return false;
    }
    let partes: Vec<&str> = tok.split('.').collect();
    partes.len() == 3 && partes.iter().all(|p| p.len() >= 8) && partes[1].starts_with("eyJ")
}

fn es_correo(tok: &str) -> bool {
    let Some((usuario, dominio)) = tok.split_once('@') else {
        return false;
    };
    if usuario.is_empty() {
        return false;
    }
    let Some((host, tld)) = dominio.rsplit_once('.') else {
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
fn es_ruta_de_casa(tok: &str) -> bool {
    let bajo = tok.to_ascii_lowercase().replace('\\', "/");
    ["/users/", "/home/"].iter().any(|p| {
        bajo.find(p)
            .map(|i| bajo[i + p.len()..].split('/').next().unwrap_or("").len() > 1)
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
fn entropia_sospechosa(tok: &str) -> bool {
    if !(24..=512).contains(&tok.len()) || forma_conocida(tok) || !alfabeto_de_credencial(tok) {
        return false;
    }
    let digito = tok.bytes().any(|b| b.is_ascii_digit());
    let minus = tok.bytes().any(|b| b.is_ascii_lowercase());
    let mayus = tok.bytes().any(|b| b.is_ascii_uppercase());
    digito && minus && mayus && vocales(tok) < 0.26 && shannon(tok) >= 3.8
}

fn alfabeto_de_credencial(tok: &str) -> bool {
    tok.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'=' | b'~'))
}

fn vocales(tok: &str) -> f64 {
    let n = tok.bytes().filter(|b| b.is_ascii_alphabetic()).count();
    if n == 0 {
        return 0.0;
    }
    let v = tok.bytes().filter(|b| b"aeiouAEIOU".contains(b)).count();
    v as f64 / n as f64
}

/// Long random-looking things that are not secrets and turn up constantly in
/// a real provenance tree: SHAs, UUIDs, ULIDs, paths and URLs.
fn forma_conocida(tok: &str) -> bool {
    let sin_guiones: String = tok.chars().filter(|c| *c != '-').collect();
    if !sin_guiones.is_empty() && sin_guiones.bytes().all(|b| b.is_ascii_hexdigit()) {
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
    let mut cuenta = [0u32; 256];
    for b in s.as_bytes() {
        cuenta[*b as usize] += 1;
    }
    let n = s.len() as f64;
    -cuenta
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
fn enmascarar(tok: &str) -> String {
    let visibles: String = tok.chars().take(4).collect();
    format!("{visibles}******** ({} caracteres)", tok.chars().count())
}

impl std::fmt::Display for Hallazgo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  Rechazado: {}\n\n      campo     {}\n      encontro  {}\n\n  {}",
            self.regla, self.campo, self.muestra, self.consejo
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rechaza(t: &str) -> bool {
        revisar("title", t).is_some()
    }

    #[test]
    fn claves_conocidas() {
        assert!(rechaza(
            "the token is sqa_9f3c1d7e5b2a48c6d0e1f2a3b4c5d6e7f8091a2b"
        ));
        assert!(rechaza("ghp_16C7e42F292c6912E7710c838347Ae178B4a"));
        assert!(rechaza(
            "usar sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345"
        ));
        assert!(rechaza("AKIAIOSFODNN7EXAMPLE"));
        assert!(rechaza("xoxb-2444-8172-abcdefghijkl"));
        assert!(rechaza("-----BEGIN RSA PRIVATE KEY-----"));
    }

    #[test]
    fn jwt() {
        assert!(rechaza(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r"
        ));
    }

    #[test]
    fn datos_personales() {
        assert!(rechaza("preguntarle a alguien@ejemplo.com"));
        assert!(rechaza("vive en C:\\Users\\unnombre\\proyectos"));
        assert!(rechaza("/home/unnombre/.config/vivac"));
        assert!(rechaza("/Users/unnombre/Library"));
    }

    #[test]
    fn lo_que_tiene_que_pasar() {
        // Ordinary prose from a real tree.
        assert!(!rechaza("Port to Rust in the public vivac/ repo"));
        assert!(!rechaza(
            "csharpsquid:S1192 literales duplicados entre archivos"
        ));
        assert!(!rechaza(
            "PermissionServiceAdapter.cs:278 is out of scope"
        ));
        // Technical references that look random and are not secrets.
        assert!(!rechaza("commit e90b4832f1a4c6d8b0e2f4a6c8d0e2f4a6c8d0e2"));
        assert!(!rechaza("node 01j8xq2m4k7pabcdefghijklmn"));
        assert!(!rechaza("id 550e8400-e29b-41d4-a716-446655440000"));
        assert!(!rechaza(
            "ver https://github.com/rust-lang/rust/issues/12345"
        ));
        // The tilde identifies nobody: it resolves on the reading machine.
        assert!(!rechaza("the hook lives in ~/.claude/settings.json"));
        // Long camelCase names, the likeliest source of a false positive.
        assert!(!rechaza("MeetingPolicyMaskCalculatorFactoryProvider"));
        assert!(!rechaza("ApplyVisibilityPolicyPerMeetingHandler"));
        assert!(!rechaza("ReunionV2PolicyMaskCalculatorFactory"));
    }

    #[test]
    fn el_heuristico_de_entropia_sigue_cazando() {
        // No known prefix: only the shape is left. These have to fall.
        assert!(rechaza("Xk7fQ2mZp9RtLw4sVb8NcJ3hGd6y"));
        assert!(rechaza("p8KdReQvXnLYtSbGmZwHfJcT3x9Wq2Vz"));
    }

    #[test]
    fn la_muestra_no_lleva_el_secreto() {
        let h = revisar("titulo", "sqa_9f3c1d7e5b2a48c6d0e1f2a3b4c5d6e7f8091a2b").unwrap();
        assert!(!h.muestra.contains("9f3c1d7e"));
        assert!(h.muestra.starts_with("sqa_"));
    }

    #[test]
    fn devuelve_el_primer_campo_que_falla() {
        let h = revisar_campos(&[
            ("title", "all fine"),
            ("por", "ghp_16C7e42F292c6912E7710c838347Ae178B4a"),
        ]);
        assert_eq!(h.unwrap().campo, "por");
    }
}
