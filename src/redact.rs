//! La guarda de redaccion. Pilar de seguridad, y tiene veto.
//!
//! Este arbol es un mapa de donde un sistema es debil y todavia no esta
//! arreglado. Lo que no entra, no se filtra. Tres cosas no entran nunca:
//! claves, datos personales y contenido de archivos.
//!
//! De las tres, la ultima no se comprueba aqui --se comprueba no teniendo
//! ninguna operacion que acepte el cuerpo de un archivo-- asi que este modulo
//! cubre las dos primeras.
//!
//! **Ante la duda se rechaza y se dice por que. Nunca se guarda callando.**
//! No hay `--force`: si la guarda se equivoca, se reformula la frase. Un
//! escape a mano seria el camino por el que acabaria entrando justo lo que
//! esto existe para dejar fuera.

/// Lo que la guarda encontro. Nunca lleva el secreto entero.
#[derive(Debug)]
pub struct Hallazgo {
    pub regla: &'static str,
    pub campo: String,
    pub muestra: String,
    pub consejo: &'static str,
}

/// Prefijos publicados por quien emite la credencial. Cero ambiguedad: si
/// aparecen, es una clave. El numero es el minimo de caracteres que tiene que
/// seguir al prefijo para que cuente.
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

/// Claves de acceso de AWS: prefijo fijo y longitud exacta de 20.
const AWS: &[&str] = &["AKIA", "ASIA", "AIDA", "AROA", "AGPA", "ANPA", "ANVA"];

const CONSEJO_CLAVE: &str = "Escribi que credencial era y donde vive, nunca su valor. \
     Ej: rotar el token de CI, esta en el secreto SONAR_TOKEN.";
const CONSEJO_PII: &str = "Referencia el rol, no a la persona ni su ruta. \
     Ej: el revisor del PR, la carpeta del usuario.";
const CONSEJO_ENTROPIA: &str = "Si no es una clave, dale un nombre en vez de pegar el valor. \
     Si lo es, no entra: guarda donde vive, no cual es.";

/// Revisa un campo. `None` significa que puede escribirse.
pub fn revisar(campo: &str, texto: &str) -> Option<Hallazgo> {
    if texto.contains("-----BEGIN") && texto.contains("PRIVATE KEY") {
        return Some(Hallazgo {
            regla: "clave privada en formato PEM",
            campo: campo.to_string(),
            muestra: "-----BEGIN ... PRIVATE KEY-----".into(),
            consejo: CONSEJO_CLAVE,
        });
    }
    tokens(texto).find_map(|tok| revisar_token(campo, tok))
}

/// Revisa varios campos de una vez. Devuelve el primero que falle, que es lo
/// que hace falta: la operacion se rechaza entera.
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
            "cadena de entropia alta sin forma conocida",
            CONSEJO_ENTROPIA,
        );
    }
    None
}

/// Parte por espacios y por los signos que nunca forman parte de una
/// credencial. Guiones, puntos, barras y bajos se conservan porque si forman.
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

/// La ruta de casa lleva el nombre de quien la tiene, que es dato personal.
/// `~` no: se resuelve en la maquina de quien lo lee y no identifica a nadie.
fn es_ruta_de_casa(tok: &str) -> bool {
    let bajo = tok.to_ascii_lowercase().replace('\\', "/");
    ["/users/", "/home/"].iter().any(|p| {
        bajo.find(p)
            .map(|i| bajo[i + p.len()..].split('/').next().unwrap_or("").len() > 1)
            .unwrap_or(false)
    })
}

/// El heuristico incierto, y por eso el mas conservador de los cinco.
///
/// La entropia sola no separa un secreto de un identificador largo:
/// `PermisoServiceAdapter.cs:278` da casi la misma cifra que una clave de la
/// misma longitud. Hacen falta dos filtros mas, y los dos salieron de que un
/// test de esta misma tanda los reclamara:
///
/// - **alfabeto de credencial**: las credenciales se escriben en base64 o
///   hexadecimal, sin puntos ni dos puntos. Un `Archivo.cs:278` queda fuera
///   por la forma, sin mirar la entropia.
/// - **proporcion de vocales**: en una cadena aleatoria las vocales rondan el
///   16 %; en algo que alguien escribio para leerlo, el 35 % o mas. Es el
///   discriminante mas barato que separa `ReunionV2PolicyMaskCalculator` de
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

/// Cosas largas y aleatorias que no son secretos y aparecen todo el tiempo en
/// un arbol de procedencia real: SHAs, UUIDs, ULIDs, rutas y URLs.
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

/// Deja ver de que se trata sin reproducirlo. Se imprime en pantalla, que ya
/// es menos malo que guardarlo, pero tampoco hace falta enseñarlo entero.
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
        revisar("titulo", t).is_some()
    }

    #[test]
    fn claves_conocidas() {
        assert!(rechaza(
            "el token es sqa_9f3c1d7e5b2a48c6d0e1f2a3b4c5d6e7f8091a2b"
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
        // Prosa normal de un arbol real.
        assert!(!rechaza("Portar a Rust en el repo publico vivac/"));
        assert!(!rechaza(
            "csharpsquid:S1192 literales duplicados entre archivos"
        ));
        assert!(!rechaza(
            "PermisoServiceAdapter.cs:278 esta fuera de alcance"
        ));
        // Referencias tecnicas que parecen aleatorias y no son secretos.
        assert!(!rechaza("commit e90b4832f1a4c6d8b0e2f4a6c8d0e2f4a6c8d0e2"));
        assert!(!rechaza("nodo 01j8xq2m4k7pabcdefghijklmn"));
        assert!(!rechaza("id 550e8400-e29b-41d4-a716-446655440000"));
        assert!(!rechaza(
            "ver https://github.com/rust-lang/rust/issues/12345"
        ));
        // El tilde no identifica a nadie: se resuelve en la maquina que lee.
        assert!(!rechaza("el hook vive en ~/.claude/settings.json"));
        // Nombres largos en camelCase, que es lo que mas falso positivo daria.
        assert!(!rechaza("ReunionPolicyMaskCalculatorFactoryProvider"));
        assert!(!rechaza("AplicarPoliticaDeVisibilidadPorReunionHandler"));
        assert!(!rechaza("ReunionV2PolicyMaskCalculatorFactory"));
    }

    #[test]
    fn el_heuristico_de_entropia_sigue_cazando() {
        // Sin prefijo conocido: solo queda la forma. Estas tienen que caer.
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
            ("titulo", "todo bien"),
            ("por", "ghp_16C7e42F292c6912E7710c838347Ae178B4a"),
        ]);
        assert_eq!(h.unwrap().campo, "por");
    }
}
