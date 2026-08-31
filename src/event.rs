//! El evento. `ROADMAP.md` §4 lo deja en un solo archivo append-only y el
//! arbol se obtiene plegandolo: si el log es la verdad, la pila se calcula.
//! Dos sedes del mismo estado contradicen el principio 1 de `MODEL.md`.

use serde::{Deserialize, Serialize};

/// Tipos de nodo. `MODEL.md` §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tipo {
    Goal,
    Task,
    Decision,
    Question,
    Constraint,
    Finding,
    Assumption,
}

impl Tipo {
    /// Prefijo del alias. `MODEL.md` §3.6.
    pub fn prefijo(self) -> char {
        match self {
            Tipo::Goal => 'g',
            Tipo::Task => 't',
            Tipo::Decision => 'd',
            Tipo::Question => 'q',
            Tipo::Constraint => 'c',
            Tipo::Finding => 'f',
            Tipo::Assumption => 'a',
        }
    }

    pub fn desde(s: &str) -> Option<Tipo> {
        Some(match s {
            "goal" | "meta" => Tipo::Goal,
            "task" | "tarea" => Tipo::Task,
            "decision" => Tipo::Decision,
            "question" | "pregunta" => Tipo::Question,
            "constraint" | "restriccion" => Tipo::Constraint,
            "finding" | "hallazgo" => Tipo::Finding,
            "assumption" | "asuncion" => Tipo::Assumption,
            _ => return None,
        })
    }

    pub const TODOS: &'static str =
        "goal, task, decision, question, constraint, finding, assumption";
}

/// Estados canonicos.
///
/// `MODEL.md` §4.2 da nombres distintos por tipo --un `goal` se `achieved`,
/// una `decision` esta `standing`-- pero la maquina de estados es la misma en
/// los cinco casos. Se guardan canonicos y se traducen al presentar: si el
/// log guardara el sinonimo, cada consulta tendria que conocer los siete tipos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Estado {
    Active,
    Done,
    Suspended,
    Abandoned,
    Superseded,
}

impl Estado {
    pub fn abierto(self) -> bool {
        self == Estado::Active
    }

    /// La palabra que le corresponde a este estado en este tipo.
    pub fn palabra(self, tipo: Tipo) -> &'static str {
        match (self, tipo) {
            (Estado::Active, Tipo::Decision) => "vigente",
            (Estado::Active, _) => "abierto",
            (Estado::Done, Tipo::Goal) => "alcanzado",
            (Estado::Done, Tipo::Question) => "contestado",
            (Estado::Done, _) => "cerrado",
            (Estado::Suspended, _) => "aparcado",
            (Estado::Abandoned, _) => "abandonado",
            (Estado::Superseded, _) => "superado",
        }
    }

    /// Marca de una letra. El significado nunca se codifica solo en color:
    /// esto se lee en blanco y negro y por ssh.
    pub fn marca(self) -> char {
        match self {
            Estado::Active => ' ',
            Estado::Done => 'x',
            Estado::Suspended => '~',
            Estado::Abandoned => '!',
            Estado::Superseded => '-',
        }
    }
}

/// Un evento del log. `MODEL.md` §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evento {
    pub seq: u64,
    pub id: String,
    pub ts: String,
    /// Identificador opaco de quien lo origino. **Nunca correo ni nombre**:
    /// `MODEL.md` §3.4 proponia `git config user.email` y el pilar de
    /// seguridad lo veta. Se genera en `init` y vive en `config`.
    pub actor: String,
    pub lane: String,
    /// El cuerpo va **anidado**, no aplanado.
    ///
    /// `#[serde(flatten)]` obliga a serde a pasar por un mapa intermedio y
    /// eso se paga en cada arranque, que aqui es cada llamada porque no hay
    /// demonio. Ademas `MODEL.md` §3.2 ya decia `payload`: aplanarlo fue una
    /// comodidad mia, no una decision del modelo.
    pub payload: Cuerpo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Cuerpo {
    /// La arista `spawns` viaja **dentro** del nodo, no como evento aparte.
    /// Asi la invariante 11 de `MODEL.md` §9 --a lo sumo un `spawns` entrante,
    /// la procedencia es un arbol y no un DAG-- la sostiene el esquema y no
    /// un chequeo que se pueda olvidar.
    #[serde(rename = "node.created")]
    NodoCreado {
        nodo: String,
        num: u64,
        tipo: Tipo,
        titulo: String,
        #[serde(default)]
        por: String,
        #[serde(default)]
        padre: Option<String>,
        #[serde(default)]
        bloquea: bool,
        #[serde(default)]
        refs: Vec<String>,
        #[serde(default)]
        governs: Vec<String>,
    },
    #[serde(rename = "state.changed")]
    EstadoCambiado {
        nodo: String,
        estado: Estado,
        #[serde(default)]
        resultado: String,
        /// Un cierre a la fuerza es legitimo, pero tiene que ser una decision
        /// y no un descuido: por eso deja rastro aqui. `MODEL.md` §7.
        #[serde(default)]
        forzado: bool,
    },
    #[serde(rename = "node.noted")]
    NodoAnotado { nodo: String, nota: String },
    #[serde(rename = "edge.blocks")]
    BloqueoCambiado { nodo: String, bloquea: bool },
    #[serde(rename = "stack.pushed")]
    Apilado { nodo: String },
    #[serde(rename = "stack.popped")]
    Desapilado { nodo: String },
    #[serde(rename = "stack.promoted")]
    Promovido { nodo: String },
}
