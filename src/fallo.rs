//! Fallos y codigos de salida.
//!
//! El pilar de DX parte la audiencia en dos y esta es la mitad del agente:
//! CLI silenciosa, scriptable, con codigos de salida distintos por motivo. Un
//! script tiene que poder distinguir "no puede cerrar todavia" de "escribiste
//! mal el comando" sin leer la prosa.

use crate::redact::Hallazgo;

pub enum Fallo {
    /// El modelo rechaza la operacion. Hoy solo hay una regla asi: el cierre
    /// con bloqueantes abiertos. `MODEL.md` §7.
    Modelo(String),
    /// El comando esta mal escrito.
    Uso(String),
    /// La guarda de redaccion. Pilar de seguridad.
    Redaccion(Box<Hallazgo>),
    /// No hay `.vivac/` aqui ni mas arriba.
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
                eprintln!("  No hay .vivac/ aqui ni mas arriba.");
                eprintln!();
                eprintln!("  Sembrar el arbol:  vivac init");
            }
            Fallo::Io(e) => eprintln!("  Error de entrada/salida: {e}"),
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
