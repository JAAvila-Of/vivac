//! Typed parameters for the fourteen write operations, built from `Args`.
//!
//! `t106`: today an operation takes `&Args` and reads it with `.opt`, `.has`,
//! `.list` scattered through its body. That reads only the CLI, and the
//! coming MCP server has no `Args` to hand it -- it has JSON. Splitting
//! "what does this operation need" from "where did it come from" is what
//! lets both build the same struct.
//!
//! **What lives here is shape, not meaning.** A struct field stays exactly
//! as untyped as the operation reads it today: `Kind::parse`, `Flag::parse`
//! and `ctx.resolve` all stay inside the operation, because every one of
//! them needs the tree and `from_args` never gets a `Ctx`. What moves here is
//! only the reading of `Args` itself, byte for byte -- including the
//! `usage` messages that do not need the tree to be produced. A message that
//! does need it (a missing focus, an id that will not resolve) stays where
//! the tree is, in the operation.

use crate::args::Args;
use crate::failure::Failure;

pub struct Push {
    pub title: String,
    pub why: String,
    pub kind: Option<String>,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub blocks: bool,
}

impl Push {
    pub fn from_args(a: &Args) -> Result<Push, Failure> {
        let title = a
            .positional(0)
            .ok_or_else(|| Failure::usage("usage: vivac push \"<title>\" --why \"<reason>\""))?;
        let why = a.opt("why").ok_or_else(|| {
            Failure::usage(
                "Missing --why. A detour with no reason is exactly the failure this\n  \
                 exists to attack: in a month nobody will know why.",
            )
        })?;
        Ok(Push {
            title: title.to_string(),
            why: why.to_string(),
            kind: a.opt("type").map(str::to_string),
            refs: a.list("ref"),
            governs: a.list("governs"),
            blocks: a.has("blocks"),
        })
    }
}

pub struct Pop {
    pub outcome: String,
    pub next: Option<String>,
    pub force: bool,
}

impl Pop {
    pub fn from_args(a: &Args) -> Result<Pop, Failure> {
        Ok(Pop {
            outcome: a.positional(0).unwrap_or("").to_string(),
            next: a.opt("next").map(str::to_string),
            force: a.has("force"),
        })
    }
}

pub struct Done {
    pub id: String,
    pub outcome: String,
    pub force: bool,
}

impl Done {
    pub fn from_args(a: &Args) -> Result<Done, Failure> {
        let id = a
            .positional(0)
            .ok_or_else(|| Failure::usage("usage: vivac done <id> [\"<outcome>\"] [--force]"))?;
        Ok(Done {
            id: id.to_string(),
            outcome: a.positional(1).unwrap_or("").to_string(),
            force: a.has("force"),
        })
    }
}

/// The two raw words `named_or_focus` disambiguates. Which of "an id", "a
/// reason" or "the focus" they mean needs the tree, so that logic --unchanged
/// -- stays in the operation; this only carries what `Args` held.
pub struct Park {
    pub node: Option<String>,
    pub reason: Option<String>,
}

impl Park {
    pub fn from_args(a: &Args) -> Result<Park, Failure> {
        Ok(Park {
            node: a.positional(0).map(str::to_string),
            reason: a.positional(1).map(str::to_string),
        })
    }
}

pub struct Add {
    pub title: String,
    pub parent: Option<String>,
    pub kind: Option<String>,
    pub why: String,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub blocks: bool,
}

impl Add {
    pub fn from_args(a: &Args) -> Result<Add, Failure> {
        let title = a.positional(0).ok_or_else(|| {
            Failure::usage("usage: vivac add \"<title>\" [--parent N] [--why \"<reason>\"]")
        })?;
        Ok(Add {
            title: title.to_string(),
            parent: a.opt("parent").map(str::to_string),
            kind: a.opt("type").map(str::to_string),
            why: a.opt_or("why"),
            refs: a.list("ref"),
            governs: a.list("governs"),
            blocks: a.has("blocks"),
        })
    }
}

/// The node named (or not) and the note itself. `note` disambiguates the two
/// raw positionals with its own logic, not `named_or_focus`'s, and that logic
/// needs the tree for the "one word, no focus" case, so it stays in the
/// operation along with the rest.
pub struct Note {
    pub node: Option<String>,
    pub note: Option<String>,
}

impl Note {
    pub fn from_args(a: &Args) -> Result<Note, Failure> {
        Ok(Note {
            node: a.positional(0).map(str::to_string),
            note: a.positional(1).map(str::to_string),
        })
    }
}

pub struct Block {
    pub id: String,
    pub off: bool,
}

impl Block {
    pub fn from_args(a: &Args) -> Result<Block, Failure> {
        let id = a
            .positional(0)
            .ok_or_else(|| Failure::usage("usage: vivac block <id> [--off]"))?;
        Ok(Block {
            id: id.to_string(),
            off: a.has("off"),
        })
    }
}

/// The id named, or none: `promote` falls back to the focus, and whether
/// that fallback exists needs the tree.
pub struct Promote {
    pub id: Option<String>,
}

impl Promote {
    pub fn from_args(a: &Args) -> Result<Promote, Failure> {
        Ok(Promote {
            id: a.positional(0).map(str::to_string),
        })
    }
}

pub struct Abandon {
    pub node: Option<String>,
    pub reason: Option<String>,
    pub rescue: Vec<String>,
    pub cascade: bool,
}

impl Abandon {
    pub fn from_args(a: &Args) -> Result<Abandon, Failure> {
        Ok(Abandon {
            node: a.positional(0).map(str::to_string),
            reason: a.positional(1).map(str::to_string),
            rescue: a.list("rescue"),
            cascade: a.has("cascade"),
        })
    }
}

pub struct Focus {
    pub id: String,
    pub reopen: bool,
}

impl Focus {
    pub fn from_args(a: &Args) -> Result<Focus, Failure> {
        let id = a
            .positional(0)
            .ok_or_else(|| Failure::usage("usage: vivac focus <id> [--reopen]"))?;
        Ok(Focus {
            id: id.to_string(),
            reopen: a.has("reopen"),
        })
    }
}

pub struct Flag {
    pub id: String,
    pub flag: String,
    pub off: bool,
    /// Mandatory unless `off`, and that condition does not need the tree
    /// either -- but it is a rule about raising a flag, not about reading a
    /// command line, so it stays validated in the operation, next to the
    /// rest of what `BRIEF-SPEC.md` §10 requires of a flag.
    pub why: Option<String>,
}

impl Flag {
    pub fn from_args(a: &Args) -> Result<Flag, Failure> {
        let (Some(sid), Some(sb)) = (a.positional(0), a.positional(1)) else {
            return Err(Failure::usage(
                "usage: vivac flag <id> <flag> --why \"<reason>\"  |  --off\n\n  \
                 Flags: suspect, review, stale",
            ));
        };
        Ok(Flag {
            id: sid.to_string(),
            flag: sb.to_string(),
            off: a.has("off"),
            why: a.opt("why").map(str::to_string),
        })
    }
}

pub struct Decide {
    pub title: String,
    pub parent: Option<String>,
    pub reason: String,
    pub alternatives: Vec<String>,
    pub supersedes: Option<String>,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub blocks: bool,
}

impl Decide {
    pub fn from_args(a: &Args) -> Result<Decide, Failure> {
        let title = a.positional(0).ok_or_else(|| {
            Failure::usage(
                "usage: vivac decide \"<title>\" --reason \"<r>\" [--alternative X] [--supersedes d9]",
            )
        })?;
        let reason = a.opt("reason").ok_or_else(|| {
            Failure::usage(
                "Missing --reason. A decision with no reason is a datum, not a decision.",
            )
        })?;
        Ok(Decide {
            title: title.to_string(),
            parent: a.opt("parent").map(str::to_string),
            reason: reason.to_string(),
            alternatives: a.list("alternative"),
            supersedes: a.opt("supersedes").map(str::to_string),
            refs: a.list("ref"),
            governs: a.list("governs"),
            blocks: a.has("blocks"),
        })
    }
}

pub struct Save {
    pub label: String,
    pub next: String,
}

impl Save {
    pub fn from_args(a: &Args) -> Result<Save, Failure> {
        Ok(Save {
            label: a.positional(0).unwrap_or("").to_string(),
            next: a.opt_or("next"),
        })
    }
}

pub struct Restore {
    pub vivac: String,
}

impl Restore {
    pub fn from_args(a: &Args) -> Result<Restore, Failure> {
        let s = a
            .positional(0)
            .ok_or_else(|| Failure::usage("usage: vivac restore <v>"))?;
        Ok(Restore {
            vivac: s.to_string(),
        })
    }
}
