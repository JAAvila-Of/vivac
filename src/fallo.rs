//! Failures and exit codes.
//!
//! The DX pillar splits the audience in two, and this is the agent's half: a
//! quiet, scriptable CLI with a different exit code per reason. A script has
//! to be able to tell "cannot close yet" from "you typed the command wrong"
//! without reading the prose.

use crate::redact::Hallazgo;

pub enum Fallo {
    /// The model refuses the operation. There is only one such rule today:
    /// closing with open blockers. `MODEL.md` §7.
    Modelo(String),
    /// The command is malformed.
    Uso(String),
    /// The redaction guard. Security pillar.
    Redaccion(Box<Hallazgo>),
    /// There is no `.vivac/` here or further up.
    SinStore,
    Io(std::io::Error),
}

pub type R = Result<(), Fallo>;

impl Fallo {
    pub fn codigo(&self) -> i32 {
        match self {
            Fallo::Modelo(_) => 1,
            Fallo::Uso(_) => 2,
            Fallo::Redaccion(_) => 3,
            Fallo::SinStore => 4,
            Fallo::Io(_) => 5,
        }
    }

    pub fn imprimir(&self) {
        eprintln!();
        match self {
            Fallo::Modelo(m) | Fallo::Uso(m) => eprintln!("{m}"),
            Fallo::Redaccion(h) => eprintln!("{h}"),
            Fallo::SinStore => {
                eprintln!("  No .vivac/ here or further up.");
                eprintln!();
                eprintln!("  Plant the tree:  vivac init");
            }
            Fallo::Io(e) => eprintln!("  Input/output error: {e}"),
        }
        eprintln!();
    }

    pub fn uso(m: impl Into<String>) -> Fallo {
        Fallo::Uso(format!("  {}", m.into()))
    }
}

impl From<std::io::Error> for Fallo {
    fn from(e: std::io::Error) -> Fallo {
        Fallo::Io(e)
    }
}
