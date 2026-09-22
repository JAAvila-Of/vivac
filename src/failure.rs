//! Failures and exit codes.
//!
//! The DX pillar splits the audience in two, and this is the agent's half: a
//! quiet, scriptable CLI with a different exit code per reason. A script has
//! to be able to tell "cannot close yet" from "you typed the command wrong"
//! without reading the prose.

use crate::redact::Finding;

#[derive(Debug)]
pub enum Failure {
    /// The model refuses the operation. There is only one such rule today:
    /// closing with open blockers. `MODEL.md` §7.
    Model(String),
    /// The command is malformed.
    Usage(String),
    /// The redaction guard. Security pillar.
    Redaction(Box<Finding>),
    /// There is no `.vivac/` here or further up.
    NoStore,
    /// `setup`'s own shape of `NoStore` (`d723` piece B): there is no tree
    /// resolvable from here at all, so `setup` has nothing to configure.
    /// Shares `NoStore`'s exit code -- a script telling "no tree" apart
    /// from "I refused" needs the same answer from `setup` as from every
    /// other command that finds none -- but names the two ways forward
    /// that are actually `setup`'s to hand back, `vivac init` and `vivac
    /// init --join`, instead of `NoStore`'s own generic sentence.
    SetupNoTree,
    Io(std::io::Error),
    /// A line in the log is well-formed JSON but names an event type or a
    /// node kind this version does not know: `t411` §13. Shares `Io`'s exit
    /// code -- the store is the thing this process cannot make sense of,
    /// same as any other log it fails to read.
    NewerVivac(String),
    /// Another process held the tree's write lock past the deadline
    /// (`d598`). Shares `Io`'s exit code: the store is what this process
    /// could not get to.
    Busy(String),
    /// The folder holds the tree but is not one of its lanes: `main` was
    /// claimed by another folder, which is what `relocate` leaves behind.
    /// Exit 1, like any other refusal the model itself makes. Raised by
    /// `Ctx::lock_for_write` (`ops.rs`), the only place that decides which
    /// lane a folder is writing as (`t594` §2.3 rule 3, §6.9).
    NotALane(String),
    /// A lane whose tree this machine's registry does not know. Shares
    /// `NoStore`'s exit code: from the caller's side it is the same answer,
    /// there is no tree to work on from here.
    TreeNotFound(String),
}

pub type R = Result<(), Failure>;

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::Model(_) | Failure::NotALane(_) => 1,
            Failure::Usage(_) => 2,
            Failure::Redaction(_) => 3,
            Failure::NoStore | Failure::TreeNotFound(_) | Failure::SetupNoTree => 4,
            Failure::Io(_) | Failure::NewerVivac(_) | Failure::Busy(_) => 5,
        }
    }

    pub fn print_to_stderr(&self) {
        eprintln!();
        match self {
            Failure::Model(m)
            | Failure::Usage(m)
            | Failure::NewerVivac(m)
            | Failure::Busy(m)
            | Failure::NotALane(m)
            | Failure::TreeNotFound(m) => eprintln!("{m}"),
            Failure::Redaction(h) => eprintln!("{h}"),
            Failure::NoStore => {
                eprintln!("  No .vivac/ here or further up.");
                eprintln!();
                eprintln!("  Plant the tree:  vivac init");
            }
            Failure::SetupNoTree => {
                eprintln!(
                    "  setup writes what an agent reads, and there is no tree here for it to"
                );
                eprintln!("  read. Nothing was written.");
                eprintln!();
                eprintln!("  Plant one here:  vivac init");
                eprintln!("  Or join one that already exists:  vivac init --join <name or path>");
            }
            Failure::Io(e) => eprintln!("  Input/output error: {e}"),
        }
        eprintln!();
    }

    /// The failure as plain text.
    ///
    /// `print_to_stderr` writes for a terminal --leading spaces, blank lines
    /// around it-- and to a channel the model never sees. The MCP server
    /// hands the model this instead, because a refusal it cannot read is a
    /// refusal it cannot act on. Two renderings of the same data, on purpose.
    pub fn message(&self) -> String {
        match self {
            Failure::Model(m)
            | Failure::Usage(m)
            | Failure::NewerVivac(m)
            | Failure::Busy(m)
            | Failure::NotALane(m)
            | Failure::TreeNotFound(m) => m.trim().to_string(),
            Failure::Redaction(h) => h.to_string(),
            Failure::NoStore => "No .vivac/ here or further up. Plant one: vivac init".into(),
            Failure::SetupNoTree => {
                "setup writes what an agent reads, and there is no tree here for it to read. \
                 Nothing was written. Plant one here: vivac init. Or join one that already \
                 exists: vivac init --join <name or path>"
                    .into()
            }
            Failure::Io(e) => format!("Input/output error: {e}"),
        }
    }

    pub fn usage(m: impl Into<String>) -> Failure {
        Failure::Usage(format!("  {}", m.into()))
    }

    pub fn newer_vivac(m: impl Into<String>) -> Failure {
        Failure::NewerVivac(format!("  {}", m.into()))
    }

    pub fn busy(deadline: std::time::Duration) -> Failure {
        Failure::Busy(format!(
            "  Another vivac process has held this tree for {} seconds, so nothing\n  \
             was written. If no other session is writing, close the others and try\n  \
             again.",
            deadline.as_secs()
        ))
    }

    /// A folder whose `.vivac/lane` names a tree this machine's registry has
    /// no path for: it once did, or was joined from another machine, and
    /// nothing here can find where that tree lives now.
    ///
    /// The remedy this used to name -- run `setup` in the tree's own
    /// folder -- was circular for the case that reaches this most often: a
    /// tree whose log is missing but which still carries its own
    /// `.vivac/lane`, telling the person standing in that very folder to go
    /// run something "in the tree's own folder" (`t594`).
    /// `--join` names the actual remedy: the flag is `t594` §4.5.4, and
    /// `d723` piece B moved it, with planting, onto `init` alone.
    pub fn tree_not_found() -> Failure {
        Failure::TreeNotFound(
            "  This folder is a lane of a tree this machine's registry does not know.\n  \
             Join it again:  vivac init --join <path to the tree>"
                .into(),
        )
    }

    /// A folder that holds the tree itself, once `main` has been claimed by
    /// another folder instead: `t594` §2.3 declares the sentence, and
    /// `Ctx::lock_for_write` is what raises it, on the write path only --
    /// reading from such a folder still works.
    pub fn not_a_lane() -> Failure {
        Failure::NotALane(
            "  This folder holds the tree but is not one of its lanes. To write from\n  \
             here, make it one:  vivac init"
                .into(),
        )
    }

    /// A tree resolves from here, but this folder is neither the tree's own
    /// folder nor one of its declared lanes yet (`d723` piece B): `setup`
    /// never writes to the tree on its own behalf any more, so a folder
    /// that would still write as some other folder's lane is one `setup`
    /// refuses rather than configures blindly. Exit 1, the same family as
    /// `already_a_lane`: this is the model itself refusing, not a usage
    /// mistake.
    pub fn not_a_lane_yet() -> Failure {
        Failure::Model(
            "  A tree sits above this folder, and this folder is not one of its lanes\n  \
             yet: work written from here would be recorded as the tree's own folder\n  \
             rather than as this one. Nothing was written.\n\n  \
             Make this folder a lane of that tree:  vivac init"
                .into(),
        )
    }

    /// A folder that already carries somebody else's `.vivac/lane`: exactly
    /// what `relocate` leaves the origin holding. `init` planting a fresh
    /// tree there would go unnoticed -- it exits 0 and prints success --
    /// while `stack` and `push` keep answering for the tree the lane names,
    /// leaving the new, empty one to sit at zero bytes forever (`t594`).
    /// Exit 1, the same as any other refusal the model
    /// itself makes.
    pub fn already_a_lane() -> Failure {
        Failure::Model(
            "  This folder is already a lane of another tree. Planting a tree here\n  \
             would split that product in two. To see where it belongs:  vivac brief"
                .into(),
        )
    }

    /// A folder that holds a tree of its own, told to join a different one
    /// (`--join`). `already_a_lane`'s text would be a lie here: this folder
    /// carries no lane to redirect, it carries the tree itself (`t594`).
    pub fn already_has_a_tree() -> Failure {
        Failure::Model(
            "  This folder holds a tree of its own, so there is nothing to join.\n  \
             See what it already has:  vivac brief"
                .into(),
        )
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::Io(e)
    }
}
