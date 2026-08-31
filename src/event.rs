//! The event. `ROADMAP.md` §4 keeps it in a single append-only file and the
//! tree comes from folding it: if the log is the truth, the stack is computed.
//! Two homes for the same state contradict principle 1 of `MODEL.md`.

use serde::{Deserialize, Serialize};

/// Node types. `MODEL.md` §4.2.
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
    /// Alias prefix. `MODEL.md` §3.6.
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

/// Canonical states.
///
/// `MODEL.md` §4.2 gives different names per type --a `goal` is `achieved`, a
/// `decision` is `standing`-- but the state machine is the same in all five
/// cases. They are stored canonical and translated for display: if the log
/// stored the synonym, every query would have to know all seven types.
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

    /// The word this state goes by for this type.
    pub fn palabra(self, tipo: Tipo) -> &'static str {
        match (self, tipo) {
            (Estado::Active, Tipo::Decision) => "standing",
            (Estado::Active, _) => "open",
            (Estado::Done, Tipo::Goal) => "achieved",
            (Estado::Done, Tipo::Question) => "answered",
            (Estado::Done, _) => "closed",
            (Estado::Suspended, _) => "parked",
            (Estado::Abandoned, _) => "abandoned",
            (Estado::Superseded, _) => "superseded",
        }
    }

    /// A one-letter mark. Meaning is never encoded in colour alone: this is
    /// read in black and white and over ssh.
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

/// Flags orthogonal to state. `MODEL.md` §4.2: a `task` can be `active` and
/// `suspect` at once, and modelling them as states would give an untenable
/// cartesian product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bandera {
    /// Something it depends on fell over. Always carries a reason.
    Suspect,
    /// Worth a look, without claiming it is wrong.
    Review,
    /// Stale: untouched while what it covers was changing.
    Stale,
}

impl Bandera {
    pub fn desde(s: &str) -> Option<Bandera> {
        Some(match s {
            "suspect" | "sospechoso" => Bandera::Suspect,
            "review" | "revisar" => Bandera::Review,
            "stale" | "viejo" => Bandera::Stale,
            _ => return None,
        })
    }

    pub fn palabra(self) -> &'static str {
        match self {
            Bandera::Suspect => "suspect",
            Bandera::Review => "review",
            Bandera::Stale => "stale",
        }
    }

    pub const TODAS: &'static str = "suspect, review, stale";
}

/// A vivac: a safe stop partway up. `MODEL.md` §4.7 calls it a **single
/// primitive with three uses**: `push`, `pop` and session end all generate
/// vivacs, they are not different mechanisms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VivacKind {
    Push,
    Pop,
    Park,
    Manual,
    Auto,
}

impl VivacKind {
    pub fn palabra(self) -> &'static str {
        match self {
            VivacKind::Push => "push",
            VivacKind::Pop => "pop",
            VivacKind::Park => "park",
            VivacKind::Manual => "manual",
            VivacKind::Auto => "auto",
        }
    }
}

/// One event from the log. `MODEL.md` §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evento {
    pub seq: u64,
    pub id: String,
    pub ts: String,
    /// Opaque identifier of whoever originated it. **Never email or name**:
    /// `MODEL.md` §3.4 proposed `git config user.email` and the security
    /// pillar vetoes it. Generated at `init` and kept in `config`.
    pub actor: String,
    pub lane: String,
    /// The body is **nested**, not flattened.
    ///
    /// `#[serde(flatten)]` forces serde through an intermediate map, and that
    /// is paid on every startup, which here means every call because there is
    /// no daemon. Besides, `MODEL.md` §3.2 already said `payload`: flattening
    /// it was a convenience of mine, not a decision of the model.
    pub payload: Cuerpo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Cuerpo {
    /// The `spawns` edge travels **inside** the node, not as a separate event.
    /// That way invariant 11 of `MODEL.md` §9 --at most one incoming `spawns`,
    /// provenance is a tree and not a DAG-- is held up by the schema and not
    /// by a check somebody can forget.
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
        /// A forced close is legitimate, but it has to be a decision and not
        /// an oversight: that is why it leaves a trace here. `MODEL.md` §7.
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
    /// The reason is mandatory: `BRIEF-SPEC.md` §10 tests it, because a flag
    /// with no reason informs nobody and is only noise.
    #[serde(rename = "flag.raised")]
    BanderaAlzada {
        nodo: String,
        bandera: Bandera,
        motivo: String,
    },
    #[serde(rename = "flag.cleared")]
    BanderaBajada { nodo: String, bandera: Bandera },
    #[serde(rename = "vivac.created")]
    VivacCreado {
        vivac: String,
        num: u64,
        kind: VivacKind,
        /// Frozen stack with titles: a vivac has to stay readable even after
        /// the nodes have changed.
        pila: Vec<(String, String)>,
        /// Paths of the pitch. Not measured --that would need `post_tool`, which
        /// is not in Tier 0-- but derived from the `governs` the stack declares.
        working_set: Vec<String>,
        /// The resume payload: what you were about to do next.
        next_intent: String,
        #[serde(default)]
        anchor: crate::anchor::AnchorRef,
        #[serde(default)]
        node_ref: Option<String>,
        #[serde(default)]
        etiqueta: String,
    },
}
