//! The event. `ROADMAP.md` §4 keeps it in a single append-only file and the
//! tree comes from folding it: if the log is the truth, the stack is computed.
//! Two homes for the same state contradict principle 1 of `MODEL.md`.

use serde::{Deserialize, Serialize};

/// Node types. `MODEL.md` §4.2. `Pillar` and `Rule` are `t411`: added at the
/// end, so the order of what already existed never moves.
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
    Pillar,
    Rule,
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
            Kind::Pillar => 'p',
            Kind::Rule => 'r',
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
            "pillar" => Kind::Pillar,
            "rule" => Kind::Rule,
            _ => return None,
        })
    }

    /// The word this type goes by. The inverse of `parse`, and spelled the
    /// same way the log serialises it.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Goal => "goal",
            Kind::Task => "task",
            Kind::Decision => "decision",
            Kind::Question => "question",
            Kind::Constraint => "constraint",
            Kind::Finding => "finding",
            Kind::Assumption => "assumption",
            Kind::Pillar => "pillar",
            Kind::Rule => "rule",
        }
    }

    pub const ALL: &'static str =
        "goal, task, decision, question, constraint, finding, assumption, pillar, rule";

    /// `word`, with the indefinite article it takes: `"a task"`, `"an
    /// assumption"`. Every message that forms "a" plus a type's word reads
    /// this instead, since `assumption` is the one of the nine that needs
    /// "an".
    pub fn with_article(self) -> String {
        let article = if self == Kind::Assumption { "an" } else { "a" };
        format!("{article} {}", self.word())
    }
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

/// A rule's arm: the command or test that verifies it, and the folder it
/// runs in, relative to the one that holds `.vivac`. `d441`: the folder is
/// part of the arm's identity, not an afterthought -- the same command in
/// two folders are two different arms, since the folder decides what the
/// command actually checks.
///
/// No `#[serde(default)]` on either field: a line missing `dir` was written
/// by a format this version does not fully know, and `t411` §13 bis refuses
/// it rather than guessing which folder was meant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arm {
    pub dir: String,
    pub command: String,
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
        /// A rule's arms, given at birth. Empty for a rule with none -- which
        /// means it is judged -- and for every type that is not a rule.
        /// `d415`. Omitted when empty, so a pillar or any other kind that
        /// never carries one writes exactly what it always has.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arms: Vec<Arm>,
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
    /// A rule gains a command or test that verifies it. `d415`: vivac never
    /// runs it, only stores and hands it back. Shaped like `flag.raised`
    /// on purpose -- an addition, never a rewrite of what was already there.
    /// `dir` is `d441`: no `#[serde(default)]`, because a line missing it is
    /// one an older format wrote, and `t411` §13 bis has to refuse it rather
    /// than silently treat it as the tree's own folder.
    #[serde(rename = "arm.added")]
    ArmAdded {
        node: String,
        dir: String,
        command: String,
    },
    /// The mirror of `arm.added`, shaped like `flag.cleared`.
    #[serde(rename = "arm.removed")]
    ArmRemoved {
        node: String,
        dir: String,
        command: String,
    },
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

impl Body {
    /// Every event-type tag this version's `Body` can read, in enum
    /// declaration order. `t411` §13: a line whose own `type` is well-formed
    /// JSON but missing from this list came from a newer vivac, and the
    /// reader refuses before writing rather than silently dropping a rule it
    /// can no longer see. Tied to every variant by
    /// `tests::known_events_cover_every_variant`, which fails to compile the
    /// moment a variant is added here without a line in that test -- a hand
    /// kept list with nothing forcing it to move would desynchronise the
    /// first time somebody forgot.
    pub(crate) const KNOWN_EVENTS: &'static [&'static str] = &[
        "node.created",
        "state.changed",
        "node.noted",
        "edge.blocks",
        "stack.pushed",
        "stack.popped",
        "stack.promoted",
        "flag.raised",
        "flag.cleared",
        "arm.added",
        "arm.removed",
        "vivac.created",
        "session.started",
    ];
}

/// Why a line failed to deserialise as an `Event`, when that reason is worth
/// refusing over rather than skipping. `t411` §13.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    /// The line's own `payload.type` names an event this version's `Body`
    /// does not carry.
    EventType(String),
    /// The line is a `node.created` whose `kind` is not one `Kind::parse`
    /// knows.
    NodeKind(String),
    /// The line is a well-formed event of a type and kind this version
    /// knows, and still does not deserialise: something added after this
    /// version, such as a new flag value. Skipping it would
    /// reopen the hole `f419` measured -- the next write reusing the number
    /// of what was skipped -- one release later.
    Shape(String),
}

/// Classifies a line that has **already** failed to deserialise as an
/// `Event`. `None` covers the ordinary accident -- broken JSON, a truncated
/// tail, anything without the shape of an event -- and that line stays
/// counted and skipped exactly as it always has. `Some` covers every line
/// that does have the shape of an event -- valid JSON, a numeric `seq`, a
/// `payload` with a `type` -- since a torn write never produces one of those,
/// and one this version cannot read was written by a newer vivac.
///
/// Only ever called on a line that already failed the ordinary parse, so a
/// healthy log pays nothing for this.
pub(crate) fn unknown_reason_for(line: &str) -> Option<UnknownReason> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    v.get("seq")?.as_u64()?;
    let payload = v.get("payload")?.as_object()?;
    let event_type = payload.get("type")?.as_str()?;
    if !Body::KNOWN_EVENTS.contains(&event_type) {
        return Some(UnknownReason::EventType(event_type.to_string()));
    }
    if event_type == "node.created" {
        if let Some(kind) = payload.get("kind").and_then(|k| k.as_str()) {
            if Kind::parse(kind).is_none() {
                return Some(UnknownReason::NodeKind(kind.to_string()));
            }
        }
    }
    Some(UnknownReason::Shape(event_type.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive on purpose -- no wildcard arm -- so a variant added to
    /// `Body` without a line here fails to compile instead of drifting
    /// silently out of `KNOWN_EVENTS` below.
    fn tag_of(b: &Body) -> &'static str {
        match b {
            Body::NodeCreated { .. } => "node.created",
            Body::StateChanged { .. } => "state.changed",
            Body::NodeNoted { .. } => "node.noted",
            Body::BlockChanged { .. } => "edge.blocks",
            Body::Pushed { .. } => "stack.pushed",
            Body::Popped { .. } => "stack.popped",
            Body::Promoted { .. } => "stack.promoted",
            Body::FlagRaised { .. } => "flag.raised",
            Body::FlagCleared { .. } => "flag.cleared",
            Body::ArmAdded { .. } => "arm.added",
            Body::ArmRemoved { .. } => "arm.removed",
            Body::VivacCreated { .. } => "vivac.created",
            Body::SessionStarted { .. } => "session.started",
        }
    }

    fn one_of_each() -> Vec<Body> {
        vec![
            Body::NodeCreated {
                node: "n".into(),
                num: 1,
                kind: Kind::Task,
                title: "t".into(),
                why: String::new(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
            Body::StateChanged {
                node: "n".into(),
                state: State::Done,
                outcome: String::new(),
                forced: false,
            },
            Body::NodeNoted {
                node: "n".into(),
                note: "x".into(),
            },
            Body::BlockChanged {
                node: "n".into(),
                blocks: true,
            },
            Body::Pushed { node: "n".into() },
            Body::Popped { node: "n".into() },
            Body::Promoted { node: "n".into() },
            Body::FlagRaised {
                node: "n".into(),
                flag: Flag::Review,
                reason: "x".into(),
            },
            Body::FlagCleared {
                node: "n".into(),
                flag: Flag::Review,
            },
            Body::ArmAdded {
                node: "n".into(),
                dir: "vivac".into(),
                command: "x".into(),
            },
            Body::ArmRemoved {
                node: "n".into(),
                dir: "vivac".into(),
                command: "x".into(),
            },
            Body::VivacCreated {
                vivac: "v".into(),
                num: 1,
                kind: VivacKind::Manual,
                stack: vec![],
                working_set: vec![],
                next_intent: String::new(),
                anchor: crate::anchor::AnchorRef::default(),
                node_ref: None,
                label: String::new(),
            },
            Body::SessionStarted {
                source: "s".into(),
                focus: None,
                vivac: None,
                session: None,
            },
        ]
    }

    #[test]
    fn known_events_cover_every_variant() {
        let samples = one_of_each();
        for s in &samples {
            let tag = tag_of(s);
            assert!(
                Body::KNOWN_EVENTS.contains(&tag),
                "missing from KNOWN_EVENTS: {tag}"
            );
            let v = serde_json::to_value(s).unwrap();
            assert_eq!(v["type"], tag, "tag_of disagrees with serde's own tag");
        }
        assert_eq!(
            samples.len(),
            Body::KNOWN_EVENTS.len(),
            "a variant or a listed tag is unmatched"
        );
    }

    #[test]
    fn an_unknown_event_type_is_flagged() {
        let line = r#"{"seq":1,"id":"x","ts":"t","actor":"a","lane":"main","payload":{"type":"node.evolved","node":"n"}}"#;
        assert_eq!(
            unknown_reason_for(line),
            Some(UnknownReason::EventType("node.evolved".to_string()))
        );
    }

    #[test]
    fn an_unknown_node_kind_is_flagged() {
        let line = r#"{"seq":1,"id":"x","ts":"t","actor":"a","lane":"main","payload":{"type":"node.created","node":"n","num":1,"kind":"epic","title":"t"}}"#;
        assert_eq!(
            unknown_reason_for(line),
            Some(UnknownReason::NodeKind("epic".to_string()))
        );
    }

    #[test]
    fn ordinary_broken_json_is_not_flagged() {
        assert_eq!(unknown_reason_for("{not json"), None);
        assert_eq!(
            unknown_reason_for(r#"{"payload":{"type":"node.created"}}"#),
            None,
            "no seq at all is an ordinary broken line, not a newer vivac"
        );
    }

    #[test]
    fn a_known_event_this_version_cannot_read_is_flagged() {
        // `node.noted` is known, and still fails the ordinary parse: a field
        // added or reshaped after this version. Skipped, it would be the
        // hole `f419` measured, one release later.
        let line = r#"{"seq":1,"id":"x","ts":"t","actor":"a","lane":"main","payload":{"type":"node.noted","node":"n"}}"#;
        assert_eq!(
            unknown_reason_for(line),
            Some(UnknownReason::Shape("node.noted".to_string()))
        );
    }

    /// A known type carrying a value this version does not know: a flag
    /// value that is not `suspect`, `review` or `stale`, which is what a
    /// newer release adding a flag would write.
    #[test]
    fn a_known_type_with_a_field_value_this_version_does_not_know_is_flagged() {
        let line = r#"{"seq":1,"id":"x","ts":"t","actor":"a","lane":"main","payload":{"type":"flag.raised","node":"n","flag":"advise","reason":"x"}}"#;
        assert_eq!(
            unknown_reason_for(line),
            Some(UnknownReason::Shape("flag.raised".to_string()))
        );
    }
}
