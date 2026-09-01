//! The event. `ROADMAP.md` §4 keeps it in a single append-only file and the
//! tree comes from folding it: if the log is the truth, the stack is computed.
//! Two homes for the same state contradict principle 1 of `MODEL.md`.

use serde::{Deserialize, Serialize};

/// Node types. `MODEL.md` §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Goal,
    Task,
    Decision,
    Question,
    Constraint,
    Finding,
    Assumption,
}

impl Kind {
    /// Alias prefix. `MODEL.md` §3.6.
    pub fn prefix(self) -> char {
        match self {
            Kind::Goal => 'g',
            Kind::Task => 't',
            Kind::Decision => 'd',
            Kind::Question => 'q',
            Kind::Constraint => 'c',
            Kind::Finding => 'f',
            Kind::Assumption => 'a',
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "goal" => Kind::Goal,
            "task" => Kind::Task,
            "decision" => Kind::Decision,
            "question" => Kind::Question,
            "constraint" => Kind::Constraint,
            "finding" => Kind::Finding,
            "assumption" => Kind::Assumption,
            _ => return None,
        })
    }

    pub const ALL: &'static str = "goal, task, decision, question, constraint, finding, assumption";
}

/// Canonical states.
///
/// `MODEL.md` §4.2 gives different names per type --a `goal` is `achieved`, a
/// `decision` is `standing`-- but the state machine is the same in all five
/// cases. They are stored canonical and translated for display: if the log
/// stored the synonym, every query would have to know all seven types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Active,
    Done,
    Suspended,
    Abandoned,
    Superseded,
}

impl State {
    pub fn is_open(self) -> bool {
        self == State::Active
    }

    /// The word this state goes by for this type.
    pub fn word(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (State::Active, Kind::Decision) => "standing",
            (State::Active, _) => "open",
            (State::Done, Kind::Goal) => "achieved",
            (State::Done, Kind::Question) => "answered",
            (State::Done, _) => "closed",
            (State::Suspended, _) => "parked",
            (State::Abandoned, _) => "abandoned",
            (State::Superseded, _) => "superseded",
        }
    }

    /// A one-letter mark. Meaning is never encoded in colour alone: this is
    /// read in black and white and over ssh.
    pub fn mark(self) -> char {
        match self {
            State::Active => ' ',
            State::Done => 'x',
            State::Suspended => '~',
            State::Abandoned => '!',
            State::Superseded => '-',
        }
    }
}

/// Flags orthogonal to state. `MODEL.md` §4.2: a `task` can be `active` and
/// `suspect` at once, and modelling them as states would give an untenable
/// cartesian product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    /// Something it depends on fell over. Always carries a reason.
    Suspect,
    /// Worth a look, without claiming it is wrong.
    Review,
    /// Stale: untouched while what it covers was changing.
    Stale,
}

impl Flag {
    pub fn parse(s: &str) -> Option<Flag> {
        Some(match s {
            "suspect" => Flag::Suspect,
            "review" => Flag::Review,
            "stale" | "old" => Flag::Stale,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            Flag::Suspect => "suspect",
            Flag::Review => "review",
            Flag::Stale => "stale",
        }
    }

    pub const ALL: &'static str = "suspect, review, stale";
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
    pub fn word(self) -> &'static str {
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
pub struct Event {
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
    pub payload: Body,
}

/// The event body.
///
/// The fields are English, and only English. They each carried a
/// `serde(alias)` with their Spanish name while the port was in flight, and
/// `d45` retired that layer: the log now reads one spelling.
///
/// The way out was to migrate the data, not to carry the compatibility layer
/// forever. The three real trees were rewritten **before** anything was
/// removed, the migration was checked by diffing the output byte for byte
/// against the binary that still read both spellings, and each previous log
/// stayed beside its tree as `events.pre-english`. The log is the source of
/// truth and it is append-only, so a rename that could not read what is
/// already written would not be a rename, it would be a data loss.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Body {
    /// The `spawns` edge travels **inside** the node, not as a separate event.
    /// That way invariant 11 of `MODEL.md` §9 --at most one incoming `spawns`,
    /// provenance is a tree and not a DAG-- is held up by the schema and not
    /// by a check somebody can forget.
    #[serde(rename = "node.created")]
    NodeCreated {
        node: String,
        num: u64,
        // Not `type`: that name is taken by the enum tag above.
        kind: Kind,
        title: String,
        #[serde(default)]
        why: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        blocks: bool,
        #[serde(default)]
        refs: Vec<String>,
        #[serde(default)]
        governs: Vec<String>,
    },
    #[serde(rename = "state.changed")]
    StateChanged {
        node: String,
        state: State,
        #[serde(default)]
        outcome: String,
        /// A forced close is legitimate, but it has to be a decision and not
        /// an oversight: that is why it leaves a trace here. `MODEL.md` §7.
        #[serde(default)]
        forced: bool,
    },
    #[serde(rename = "node.noted")]
    NodeNoted { node: String, note: String },
    #[serde(rename = "edge.blocks")]
    BlockChanged { node: String, blocks: bool },
    #[serde(rename = "stack.pushed")]
    Pushed { node: String },
    #[serde(rename = "stack.popped")]
    Popped { node: String },
    #[serde(rename = "stack.promoted")]
    Promoted { node: String },
    /// The reason is mandatory: `BRIEF-SPEC.md` §10 tests it, because a flag
    /// with no reason informs nobody and is only noise.
    #[serde(rename = "flag.raised")]
    FlagRaised {
        node: String,
        flag: Flag,
        reason: String,
    },
    #[serde(rename = "flag.cleared")]
    FlagCleared { node: String, flag: Flag },
    #[serde(rename = "vivac.created")]
    VivacCreated {
        vivac: String,
        num: u64,
        kind: VivacKind,
        /// Frozen stack with titles: a vivac has to stay readable even after
        /// the nodes have changed.
        stack: Vec<(String, String)>,
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
        label: String,
    },
    /// A session was opened and the brief was injected.
    ///
    /// Without it there is no answer to *was the brief read?*: a session
    /// boundary was only ever the gap between two writes, and a gap also
    /// happens when somebody goes to lunch.
    ///
    /// `source` is a `String` and not an enum **on purpose**. If a new kind of
    /// opening ever shows up, an enum would make the whole line unreadable and
    /// count it among the broken ones; a string lets it through and leaves the
    /// decision to whoever reads.
    ///
    /// Everything here is an opaque identifier. The payload this is built from
    /// also carries the path of the transcript, which holds the user's home
    /// directory: it is read past and never written down.
    #[serde(rename = "session.started")]
    SessionStarted {
        source: String,
        #[serde(default)]
        focus: Option<String>,
        #[serde(default)]
        vivac: Option<String>,
        #[serde(default)]
        session: Option<String>,
    },
}
