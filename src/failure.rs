//! Failures and exit codes.
//!
//! The DX pillar splits the audience in two, and this is the agent's half: a
//! quiet, scriptable CLI with a different exit code per reason. A script has
//! to be able to tell "cannot close yet" from "you typed the command wrong"
//! without reading the prose.

use crate::redact::Finding;

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
}

pub type R = Result<(), Failure>;

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::Model(_) => 1,
            Failure::Usage(_) => 2,
            Failure::Redaction(_) => 3,
            Failure::NoStore => 4,
            Failure::Io(_) => 5,
        }
    }

    pub fn print_to_stderr(&self) {
        eprintln!();
        match self {
            Failure::Model(m) | Failure::Usage(m) => eprintln!("{m}"),
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
            Failure::Model(m) | Failure::Usage(m) => m.trim().to_string(),
            Failure::Redaction(h) => h.to_string(),
            Failure::NoStore => "No .vivac/ here or further up. Plant one: vivac init".into(),
            Failure::Io(e) => format!("Input/output error: {e}"),
        }
    }

    pub fn usage(m: impl Into<String>) -> Failure {
        Failure::Usage(format!("  {}", m.into()))
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::Io(e)
    }
}
