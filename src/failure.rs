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
            Failure::NoStore | Failure::TreeNotFound(_) => 4,
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
    /// The remedy named here has to be one that works **today**. It used
    /// to name `--join`, a flag no release of this crate has ever taken;
    /// `setup` refused it with exit 2, and whoever was not standing in a
    /// worktree -- the registry wiped, the machine reimaged, the folder
    /// copied somewhere else -- had no way out this crate actually offers
    /// (`t594` branch-fix-1 #5). Running `setup` in the tree's own folder
    /// registers it again, which is exactly the gap. `--join` can replace
    /// this once it exists, which is `t594` §4.5.4.
    pub fn tree_not_found() -> Failure {
        Failure::TreeNotFound(
            "  This folder is a lane of a tree this machine's registry does not know.\n  \
             Run this in the tree's own folder to put it back:  vivac setup claude-code"
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
             here, make it one:  vivac setup claude-code"
                .into(),
        )
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::Io(e)
    }
}
