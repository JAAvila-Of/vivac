//! The tree as tools an agent can call: an MCP server over standard input.
//!
//! **No new dependency.** MCP over stdio is JSON-RPC 2.0 in newline delimited
//! JSON, and `serde_json` was already here. No runtime, no HTTP, no second
//! crate: the server lives in the binary that is already installed.
//!
//! ```text
//! claude mcp add vivac -- vivac mcp
//! ```
//!
//! **Standard output is the protocol.** A `println!` on a path the server
//! touches is not untidy, it is a malformed frame and the client hangs up. So
//! the reads are called through their builders --`find_data`, `why_data`,
//! `open_data`, `to_text`-- which return the answer instead of printing it,
//! and the server decides what reaches the wire. `tests/mcp.rs` guards that
//! with a test that reads every line back. This used to be the one place in
//! the crate that could not print as it goes; since `output` became the only
//! owner of standard output there is no other, and `tests/no_println.rs`
//! keeps it that way for reasons of its own.
//!
//! **What this is not.** `INTEGRATION.md` §4 is blunt about it: MCP tools are
//! voluntary, and an agent under task pressure does not call them. So this
//! does not fix the capture problem, and it is not offered as a fix. What it
//! answers is `d100`: the memory store this replaces is reachable over MCP,
//! and taking its place means being reachable through the same door, in the
//! tool list, with a schema.
//!
//! **Five reads, ten writes, fifteen tools.** The reads answer `brief`,
//! `find`, `why`, `open` and `rules` -- the last one `t411`'s own pull,
//! since a rule nobody pulls on is a rule that might as well not be there.
//! The writes are `push`, `pop`, `done`, `add`, `decide`, `note`, `park`,
//! `save`, `arm` and `declare` -- the first eight `t106` already turned into
//! functions that hand back an `Outcome` instead of printing one, so this
//! is the second caller that reads the same answer the CLI does.
//!
//! **What stays out, and why.** `abandon` discards a node and every
//! descendant it has; reachable from a tool call, that would happen with
//! nobody watching a terminal, and the security pillar vetoes it outright.
//! `restore` rewrites the stack and sits on the same side of that line.
//! `block`, `flag`, `promote` and `focus` are maintainer surgery, and the
//! maintainer has a terminal. `done` is here without `force` (`d776`): it
//! closes a node the same way the CLI does, but closing over an open
//! closure condition stays the terminal's own call, not a tool's.

use crate::args::Args;
use crate::failure::{Failure, R};
use crate::project::{Project, Registry};
use crate::{brief, index, ops, outcome, params, registry, render, store};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// The version spoken when the client does not name one.
const PROTOCOL: &str = "2025-06-18";

/// What JSON shape an argument's value takes. Every tool used to need only
/// `string`, but `blocks` is a flag and `ref`/`governs`/`alternative` repeat,
/// so `schema` below has two more shapes to say.
#[derive(Clone, Copy)]
enum ArgKind {
    Str,
    Bool,
    List,
}

impl ArgKind {
    /// The word a mismatch message names it by. Not the JSON type `schema`
    /// writes into `inputSchema` -- `List` is `array` there -- because this
    /// reads as a sentence, not as a wire format.
    ///
    /// Its only caller is `Reader::checked`, which exists only under
    /// `debug_assertions`, so this carries the same condition rather than
    /// staying behind and being reported dead by every release build
    /// (`f635`). A field cannot take this treatment as cheaply -- see
    /// `Reader::tool` -- but a free function can, and where it fits it is
    /// better than silencing the warning, because the item really does not
    /// need to exist there.
    #[cfg(debug_assertions)]
    fn word(self) -> &'static str {
        match self {
            ArgKind::Str => "string",
            ArgKind::Bool => "boolean",
            ArgKind::List => "list",
        }
    }
}

struct Arg {
    name: &'static str,
    kind: ArgKind,
    required: bool,
    description: &'static str,
}

struct Tool {
    /// `vivac_<command>`, and the suffix is not decoration: it is the CLI
    /// command this mirrors. `INTEGRATION.md` §8 listed five tools --`ask`,
    /// `answer`, `assume`, `verify`, `refute`-- that no command implements,
    /// and a function reachable from one surface only leaves half the
    /// audience outside it, which the DX pillar refuses by name. The test
    /// `every_tool_is_a_command_the_cli_already_has` keeps the two honest.
    name: &'static str,
    /// What a client shows a person in place of `name` (MCP `title`).
    title: &'static str,
    /// Whether it only reads. Every other tool appends to the log, so
    /// `schema` derives the rest of the annotations from this one bit: none
    /// destroys anything, none is safe to repeat, and none leaves the
    /// machine (`f897`).
    read_only: bool,
    description: &'static str,
    args: &'static [Arg],
}

/// Fifteen, and the number is a budget rather than a stage of growth: every
/// tool here costs context in every session the agent ever opens. The other
/// six write ops -- `block`, `promote`, `abandon`, `focus`, `flag`,
/// `restore` -- stay off this list on purpose; see the module doc.
const TOOLS: &[Tool] = &[
    Tool {
        name: "vivac_brief",
        title: "Brief: where you are and what not to touch",
        read_only: true,
        description: "Where you are in this project and what NOT to touch right now: \
                      the focus with its lineage, the parked nodes with the reason each \
                      was parked for, the decisions that still govern, and the last safe \
                      point with what you were about to do. Read it before anything else \
                      when a session opens.",
        args: &[],
    },
    Tool {
        name: "vivac_find",
        title: "Find nodes by their words",
        read_only: true,
        description: "Search the provenance tree. Returns every node whose title, reason, \
                      note or outcome contains all of the terms, best first, each with \
                      the lineage it hangs from. Ranking is not recency: a hit in the \
                      title outranks a hit in a note, a node holding up more tree \
                      outranks one holding up less, and recency is only the last \
                      tiebreak. Closed nodes are included: what you look for months \
                      later is usually finished.",
        args: &[
            Arg {
                name: "query",
                kind: ArgKind::Str,
                required: true,
                description: "Words to look for. Every one of them has to appear.",
            },
            Arg {
                name: "everywhere",
                kind: ArgKind::Bool,
                required: false,
                description: "Searches every project on the machine rather than this one, except those that keep what they know to themselves. Say which project anything you use comes from.",
            },
        ],
    },
    Tool {
        name: "vivac_why",
        title: "Why a node exists",
        read_only: true,
        description: "Why a node exists: the chain from the goal down to it, what is open \
                      in parallel, what was born from it, and what blocks it from closing. \
                      This is the question the whole tool exists to answer. Open siblings \
                      and children are capped at eight each, every blocking one kept, and \
                      the node's newest three notes come whole with the rest counted; full \
                      brings them all. Walking siblings one after another, pass only: \
                      their shared path came with the first.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The node as the tree names it: g1, t12, f74, d29.",
            },
            Arg {
                name: "project",
                kind: ArgKind::Str,
                required: false,
                description: "Opens a node that lives in another tree: a project name from \
                              `vivac_find`'s `everywhere`, since an alias only means \
                              something inside its own tree.",
            },
            Arg {
                name: "full",
                kind: ArgKind::Bool,
                required: false,
                description: "Every sibling and every child still open, not only the eight \
                              the answer keeps, plus each step's anchor, standing decisions \
                              and what was open at the time: what vivac why --full prints. \
                              Refused together with project.",
            },
            Arg {
                name: "only",
                kind: ArgKind::Bool,
                required: false,
                description: "The node alone: no path, no siblings, no children. For the \
                              next sibling once one why has already brought the path \
                              they share.",
            },
        ],
    },
    Tool {
        name: "vivac_open",
        title: "List the open fronts",
        read_only: true,
        description: "List what is still unfinished in this project: every open node \
                      with nothing open under it, the ones that block their parent first, \
                      then those holding up the most tree, then the newest. Each comes \
                      back as its alias, kind, state, title and the aliases above it; \
                      vivac_why on an alias brings the rest. Use it to answer what is \
                      left or what is waiting. For where this session stands, read \
                      vivac_brief; to look for something by its words, vivac_find.",
        args: &[],
    },
    Tool {
        name: "vivac_rules",
        title: "Pillars, rules and invariants",
        read_only: true,
        description: "What governs this project: every pillar, the rules under each \
                      pillar and those without one, and the invariants. A pillar's \
                      title names it and says what it rejects, in the project's own \
                      words. Each rule carries the commands that verify it, each with \
                      the folder it runs in, relative to the folder that holds .vivac, \
                      or none, which means it is judged. Run a command from its folder: \
                      from anywhere else it can pass without checking anything. Read it \
                      whenever you are asked to check work against the project's rules, \
                      whether or not they arrived when the session opened: vivac hands \
                      you the rules and the commands, and the judging is yours. If it \
                      comes back with no pillar and no rule while the project keeps its \
                      rules in files such as CLAUDE.md or AGENTS.md, propose which are \
                      pillars and which are rules, let the person decide, and write \
                      them with vivac_add.",
        args: &[],
    },
    Tool {
        name: "vivac_push",
        title: "Open a node and step into it",
        read_only: false,
        description: "Open a node and step into it: it becomes the focus, and everything \
                      captured next hangs from it until a matching pop. Call it the moment \
                      a new line of work starts or forks away from the current one -- a \
                      question that has to be settled before continuing, a detour worth \
                      its own trace -- never after the fact, once the reason for taking it \
                      has already faded. Look first with `vivac_find`: work the tree \
                      already holds is never opened twice, and what turns up while on it \
                      is a node under it, not a note. The \
                      focus is wherever work was left, perhaps by another session and \
                      about something else, so name in `parent` the node this work \
                      continues, or pass `root` when it continues nothing. `why` is \
                      mandatory: a detour with no reason recorded is the failure this tree \
                      exists to catch.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "What this node is, in a few words.",
            },
            Arg {
                name: "why",
                kind: ArgKind::Str,
                required: true,
                description: "Why this is happening now. A detour with no reason is what \
                              this field exists to prevent.",
            },
            Arg {
                name: "type",
                kind: ArgKind::Str,
                required: false,
                description: "goal, task, decision, question, constraint, finding, \
                              assumption, pillar or rule. Defaults to goal at the root, \
                              task otherwise. A pillar is titled with its name and what \
                              it restricts, in the project's own words.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this node is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this node's work is expected to touch.",
            },
            Arg {
                name: "arm",
                kind: ArgKind::List,
                required: false,
                description: "Commands that verify this rule, one per entry, all run in \
                              arm_dir. Only for a rule; a rule without one is judged. \
                              vivac never runs them.",
            },
            Arg {
                name: "arm_dir",
                kind: ArgKind::Str,
                required: false,
                description: "The folder every arm given here runs in, relative to the \
                              folder that holds .vivac: vivac, say, or . for that folder \
                              itself. Required with arm, refused without it. It has to \
                              exist.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "Only for a decision: a pillar or rule it was judged \
                              against and a sentence on how it holds, as one entry: \
                              \"r12: the write path stays local\". Repeat for each one.",
            },
            Arg {
                name: "parent",
                kind: ArgKind::Str,
                required: false,
                description: "The node this work continues, when it is not the focus. \
                              The stack is rebuilt as that node's path, the way vivac \
                              focus does, and the new node opens under it; the answer \
                              says what left the stack. Refused together with root, and \
                              on a node that is closed or parked.",
            },
            Arg {
                name: "root",
                kind: ArgKind::Bool,
                required: false,
                description: "Born at the root, with no parent, instead of under the \
                              focus. The stack is left holding only the new node; \
                              nothing on it is closed, and the answer says how to get \
                              back. Refused together with parent.",
            },
        ],
    },
    Tool {
        name: "vivac_pop",
        title: "Close the focus and step back",
        read_only: false,
        description: "Close the current focus and step back to its parent, recording what \
                      came of it. Call it once the work `vivac_push` opened is actually \
                      finished, not on a whim to clear the stack: a node with open closure \
                      conditions refuses to close on its own, because a run that closes \
                      with its findings still open is exactly the mistake that refusal \
                      exists to catch. It steps back to the parent: if the work also \
                      settles that node -- the finding it fixed, the question it \
                      answered -- pop again.",
        args: &[
            Arg {
                name: "outcome",
                kind: ArgKind::Str,
                required: false,
                description: "What happened. Read back later, so leaving it out costs \
                              the next reader the point of the node.",
            },
            Arg {
                name: "next",
                kind: ArgKind::Str,
                required: false,
                description: "What to pick up next, for whoever comes back. Without \
                              it the stop leaves nothing to pick up: the outcome is what \
                              was finished, not what comes next.",
            },
            Arg {
                name: "force",
                kind: ArgKind::Bool,
                required: false,
                description: "Close anyway, over open closure conditions. Leaves a trace \
                              that it happened.",
            },
        ],
    },
    Tool {
        name: "vivac_done",
        title: "Close a node that is not the focus",
        read_only: false,
        description: "Close a node that is not the focus, recording what came of it. \
                      Call it right after writing a lesson or a measurement that asks \
                      nothing of anyone -- a record, whose outcome starts with Record: \
                      -- and for work that was finished somewhere else. It never closes \
                      over open closure conditions; that takes vivac done --force at a \
                      terminal, with a person looking. vivac_pop closes the focus.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The node to close, as the tree names it: f12, t4.",
            },
            Arg {
                name: "outcome",
                kind: ArgKind::Str,
                required: false,
                description: "What came of it. For a record, what it records, starting \
                              with Record:.",
            },
        ],
    },
    Tool {
        name: "vivac_add",
        title: "File a node without moving the focus",
        read_only: false,
        description: "File a node without touching the stack: the focus stays exactly \
                      where it was. Use it for something that belongs in the tree but is \
                      not the next thing about to happen -- a finding surfaced while \
                      working on something else, a sibling task filed for later, a piece \
                      of an existing structure being brought in. `vivac_push` is for what \
                      comes next; this is for what was just noticed. Something left to \
                      do that turns up while working on a node is a task filed under it, \
                      never a note on it. Look first with `vivac_find`: what the tree \
                      already holds is not filed twice. A finding is one node for each thing found that you tell the person, \
                      written when you tell them. One that asks nothing of anyone -- a \
                      lesson, a measurement -- is a record: close it right away with \
                      vivac_done, its outcome starting with Record:. The title is words, \
                      never an id: the node it hangs from goes in `parent`.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "What this node is, in a few words.",
            },
            Arg {
                name: "parent",
                kind: ArgKind::Str,
                required: false,
                description: "The node it hangs from. Defaults to the current focus, or \
                              the root if there is none.",
            },
            Arg {
                name: "why",
                kind: ArgKind::Str,
                required: false,
                description: "Why this matters.",
            },
            Arg {
                name: "type",
                kind: ArgKind::Str,
                required: false,
                description: "goal, task, decision, question, constraint, finding, \
                              assumption, pillar or rule. Defaults to goal at the root, \
                              task otherwise. A pillar is titled with its name and what \
                              it restricts, in the project's own words.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this node is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this node's work is expected to touch.",
            },
            Arg {
                name: "arm",
                kind: ArgKind::List,
                required: false,
                description: "Commands that verify this rule, one per entry, all run in \
                              arm_dir. Only for a rule; a rule without one is judged. \
                              vivac never runs them.",
            },
            Arg {
                name: "arm_dir",
                kind: ArgKind::Str,
                required: false,
                description: "The folder every arm given here runs in, relative to the \
                              folder that holds .vivac: vivac, say, or . for that folder \
                              itself. Required with arm, refused without it. It has to \
                              exist.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "Only for a decision: a pillar or rule it was judged \
                              against and a sentence on how it holds, as one entry: \
                              \"r12: the write path stays local\". Repeat for each one.",
            },
            Arg {
                name: "root",
                kind: ArgKind::Bool,
                required: false,
                description: "Born at the root, with no parent, instead of under the \
                              focus. Refused together with parent.",
            },
        ],
    },
    Tool {
        name: "vivac_decide",
        title: "Record a decision",
        read_only: false,
        description: "Record a decision, with the reason it was made and every alternative \
                      that lost. The choice can be yours or the person's, a limit they \
                      set included; either way it is a decision, never a note. Call it \
                      the moment a choice is actually settled, not before and not long \
                      after: the alternatives are optional in the \
                      schema and not in practice, because without them the same option \
                      gets proposed again in a month by whoever was not in the room. \
                      When the project has pillars or rules, name in `against` the ones \
                      this was judged against, each with a sentence: a pillar judged \
                      in silence reads the same as one skipped.",
        args: &[
            Arg {
                name: "title",
                kind: ArgKind::Str,
                required: true,
                description: "The decision, in a few words.",
            },
            Arg {
                name: "reason",
                kind: ArgKind::Str,
                required: true,
                description: "Why this and not something else. A decision with no reason \
                              is a datum, not a decision.",
            },
            Arg {
                name: "parent",
                kind: ArgKind::Str,
                required: false,
                description: "The node it hangs from. Defaults to the current focus, or \
                              the root if there is none.",
            },
            Arg {
                name: "alternative",
                kind: ArgKind::List,
                required: false,
                description: "An option that was ruled out. Repeat for each one.",
            },
            Arg {
                name: "supersedes",
                kind: ArgKind::Str,
                required: false,
                description: "An earlier decision this one retires.",
            },
            Arg {
                name: "blocks",
                kind: ArgKind::Bool,
                required: false,
                description: "Its parent cannot close while this one is still open.",
            },
            Arg {
                name: "ref",
                kind: ArgKind::List,
                required: false,
                description: "Paths or identifiers this decision is about.",
            },
            Arg {
                name: "governs",
                kind: ArgKind::List,
                required: false,
                description: "Globs of files this decision's work is expected to touch.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: false,
                description: "A pillar or rule this was judged against and a sentence on \
                              how it holds, as one entry: \"r12: the write path stays \
                              local\". Repeat for each one. vivac checks that the pillar \
                              or rule exists and still governs; the sentence is not judged.",
            },
            Arg {
                name: "root",
                kind: ArgKind::Bool,
                required: false,
                description: "Born at the root, with no parent, instead of under the \
                              focus. Refused together with parent.",
            },
        ],
    },
    Tool {
        name: "vivac_note",
        title: "Attach a fact to a node",
        read_only: false,
        description: "Attach a fact to a node without changing its state or the stack: \
                      how the work on it went, or something that changed outside git -- \
                      CI, a tracker, the cloud -- that the tree is the only record of. A \
                      note never carries a finding, a choice (yours or the person's) or \
                      something left to do: each of those is a node under this one, filed \
                      with `vivac_add` or `vivac_decide`, so it shows up as what it is. \
                      When a node keeps collecting notes with nothing filed under it, the \
                      answer says so.",
        args: &[
            Arg {
                name: "note",
                kind: ArgKind::Str,
                required: true,
                description: "The fact to attach.",
            },
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: false,
                description: "The node to attach it to. Defaults to the current focus.",
            },
        ],
    },
    Tool {
        name: "vivac_park",
        title: "Park a node: not now",
        read_only: false,
        description: "Suspend a node without abandoning it: it drops off the stack and \
                      becomes something a later session is told not to touch. Given an \
                      `until` day, the brief puts it back in front of whoever opens a \
                      session on or after that day; it stays parked until somebody \
                      takes it back. Call it when the person \
                      says not now, or when work is stuck on something outside this \
                      session -- never as a substitute for `vivac_pop` on something that \
                      is simply finished. What is put off has to be a node first: if it \
                      is not in the tree yet, file it with `vivac_add` and park that.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: false,
                description: "The node to park. Defaults to the current focus.",
            },
            Arg {
                name: "reason",
                kind: ArgKind::Str,
                required: false,
                description: "Why it waits: the person's own words when they said not \
                              now. Read back verbatim under DO NOT TOUCH NOW.",
            },
            Arg {
                name: "until",
                kind: ArgKind::Str,
                required: false,
                description: "YYYY-MM-DD, a day after today: the node comes back in \
                              vivac_brief under BACK FROM PARKED on that day. Leave it \
                              out to park with no return date.",
            },
        ],
    },
    Tool {
        name: "vivac_save",
        title: "Record a safe stop",
        read_only: false,
        description: "Record a safe stop: a label for this point and what was about to \
                      happen next. Each call adds a new stop and never replaces an earlier \
                      one, and it neither moves the focus nor closes anything. The latest \
                      stop is what vivac_brief shows as the last one, so a session that \
                      picks the thread back up -- this one later, or someone else's -- \
                      starts where this one left off instead of guessing from the log. \
                      The answer also says what the stop found unfinished, when there is \
                      any: files not committed, commits not pushed, and files changed that \
                      no node claims. \
                      Call it at a clean seam: before the session ends, before a long \
                      pause or a handoff, or when the person asks for a safe point. To \
                      set a node aside, vivac_park; to keep a fact on one, vivac_note.",
        args: &[
            Arg {
                name: "label",
                kind: ArgKind::Str,
                required: false,
                description: "A short name for this stop. Left out, vivac writes one from what \
                              was opened and closed since the last stop made by hand.",
            },
            Arg {
                name: "next",
                kind: ArgKind::Str,
                required: false,
                description: "What was about to happen next.",
            },
        ],
    },
    Tool {
        name: "vivac_arm",
        title: "Record the command that verifies a rule",
        read_only: false,
        description: "Record a command that verifies a rule and the folder it runs in, or \
                      with off, remove one. vivac never runs it: it hands it to whoever \
                      checks the rule. Call it the moment a test for a rule exists, \
                      because the day a rule became checkable is part of its history.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The rule, as the tree names it: r12.",
            },
            Arg {
                name: "command",
                kind: ArgKind::Str,
                required: true,
                description: "The command or test that verifies it, as someone would \
                              type it.",
            },
            Arg {
                name: "dir",
                kind: ArgKind::Str,
                required: true,
                description: "The folder the command runs in, relative to the folder \
                              that holds .vivac: vivac, say, or . for that folder \
                              itself. It has to exist.",
            },
            Arg {
                name: "off",
                kind: ArgKind::Bool,
                required: false,
                description: "Remove this command, in this folder, from the rule \
                              instead of adding it.",
            },
        ],
    },
    Tool {
        name: "vivac_declare",
        title: "Record what a decision was judged against",
        read_only: false,
        description: "Record, after the fact, the pillars or rules a decision was judged \
                      against, each with a sentence. Call it the moment the judging \
                      happens -- someone asks whether a decision holds against a rule, \
                      and it gets checked -- because when a decision was judged is part \
                      of its history, and this one shows as late. It only adds to a \
                      decision that already exists, and declaring the same rule again \
                      replaces its sentence. A new decision takes what it was judged \
                      against when it is recorded, through `vivac_decide`; `vivac_rules` \
                      lists the pillars and rules there are.",
        args: &[
            Arg {
                name: "id",
                kind: ArgKind::Str,
                required: true,
                description: "The decision, as the tree names it: d12.",
            },
            Arg {
                name: "against",
                kind: ArgKind::List,
                required: true,
                description: "A pillar or rule it was judged against and a sentence on \
                              how it holds, as one entry: \"r12: the write path stays \
                              local\". Repeat for each one.",
            },
        ],
    },
];

fn schema(t: &Tool) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<&str> = Vec::new();
    for a in t.args {
        let mut entry = serde_json::Map::new();
        match a.kind {
            ArgKind::Str => {
                entry.insert("type".to_string(), json!("string"));
            }
            ArgKind::Bool => {
                entry.insert("type".to_string(), json!("boolean"));
            }
            ArgKind::List => {
                entry.insert("type".to_string(), json!("array"));
                entry.insert("items".to_string(), json!({ "type": "string" }));
            }
        }
        entry.insert("description".to_string(), json!(a.description));
        properties.insert(a.name.to_string(), Value::Object(entry));
        if a.required {
            required.push(a.name);
        }
    }
    let mut annotations = json!({
        "readOnlyHint": t.read_only,
        "destructiveHint": false,
        "openWorldHint": false,
    });
    if !t.read_only {
        annotations["idempotentHint"] = json!(false);
    }
    json!({
        "name": t.name,
        "title": t.title,
        "description": t.description,
        "inputSchema": {
            "type": "object",
            "properties": Value::Object(properties),
            "required": required,
        },
        "annotations": annotations,
    })
}

fn ok(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn rpc_error(id: &Value, code: i32, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A refusal the model can read and act on.
///
/// It is a successful frame with `isError` raised, not a JSON-RPC error: a
/// protocol error is for the client and the model never sees it, and "no such
/// node: t999" is exactly the kind of thing the model has to see to fix its
/// own next call.
fn tool_error(id: &Value, message: String) -> String {
    ok(
        id,
        json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
    )
}

fn tool_ok(id: &Value, text: String) -> String {
    ok(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
    )
}

fn pretty(v: Value) -> Result<String, Failure> {
    serde_json::to_string_pretty(&v).map_err(|e| Failure::Io(std::io::Error::other(e)))
}

/// The one door every argument comes through. A schema and the call that
/// reads it are two sources, and nothing else keeps them together (`f438`,
/// `f452`): a `Reader` is built for one tool, and every read it does is
/// checked against that tool's own `args`.
struct Reader<'a> {
    /// Read only by `checked`, which is a `debug_assertions` build only, so
    /// a release build is right to call this unread (`f635`). Unlike
    /// `ArgKind::word` this is not given that condition: `new` sets it
    /// unconditionally, so the field would take a second constructor with
    /// it, and two ways to build a `Reader` is a worse thing to own than one
    /// silenced warning.
    #[allow(dead_code)]
    tool: &'static Tool,
    arguments: &'a Value,
}

impl<'a> Reader<'a> {
    fn new(tool: &'static Tool, params: &'a Value) -> Reader<'a> {
        Reader {
            tool,
            arguments: &params["arguments"],
        }
    }

    /// Debug builds only, and cheap enough there to run on every read: an
    /// argument the tool's schema does not list, or one read as a shape
    /// other than the one declared for it, stops the call on the spot.
    #[cfg(debug_assertions)]
    fn checked(&self, name: &str, kind: ArgKind) {
        match self.tool.args.iter().find(|a| a.name == name) {
            None => panic!(
                "{tool} reads {name}, which its schema does not declare",
                tool = self.tool.name
            ),
            Some(declared) if declared.kind.word() != kind.word() => panic!(
                "{tool} reads {name} as a {read}, and its schema declares a {is}",
                tool = self.tool.name,
                read = kind.word(),
                is = declared.kind.word()
            ),
            Some(_) => {}
        }
    }

    #[cfg(not(debug_assertions))]
    fn checked(&self, _name: &str, _kind: ArgKind) {}

    fn str(&self, name: &str) -> Option<&'a str> {
        self.checked(name, ArgKind::Str);
        self.arguments[name].as_str()
    }

    fn bool(&self, name: &str) -> bool {
        self.checked(name, ArgKind::Bool);
        self.arguments[name].as_bool().unwrap_or(false)
    }

    fn list(&self, name: &str) -> Vec<String> {
        self.checked(name, ArgKind::List);
        self.arguments[name]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The two panics `checked` raises, pinned to their exact wording. Only
/// meaningful in a debug build, the only build where `checked` does
/// anything.
#[cfg(all(test, debug_assertions))]
mod reader_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "vivac_why reads arm_dir, which its schema does not declare")]
    fn reading_an_undeclared_argument_panics() {
        let tool = TOOLS.iter().find(|t| t.name == "vivac_why").unwrap();
        let params = json!({ "arguments": {} });
        Reader::new(tool, &params).str("arm_dir");
    }

    #[test]
    #[should_panic(expected = "vivac_push reads arm as a string, and its schema declares a list")]
    fn reading_a_declared_argument_with_the_wrong_shape_panics() {
        let tool = TOOLS.iter().find(|t| t.name == "vivac_push").unwrap();
        let params = json!({ "arguments": {} });
        Reader::new(tool, &params).str("arm");
    }
}

/// Serialised the same way the three reads that speak JSON already are:
/// `pretty` over a `Value`, plus one field the data alone cannot carry.
/// `text` is what `outcome::to_text` renders for the same write, the way the
/// CLI prints it (`d550`), with the moves it suggests named as tools
/// (`d964`). The warnings only exist as that sentence
/// -- a decision judged against nothing, a stack four deep, a stop with no
/// next step -- and they are meant for the moment of writing, which for an
/// agent is almost always a call to this server.
fn outcome_text(o: outcome::Outcome) -> Result<String, Failure> {
    let mut v = serde_json::to_value(&o).map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    v["text"] = json!(outcome::to_tools_text(&o));
    pretty(v)
}

/// Argument names the call sent that this tool's own schema does not list,
/// sorted the way `Args::unknown` sorts the CLI's, so the two doors answer
/// an unlisted argument with the same shape of refusal (`f438`, `d880`).
/// Reads `tool.args`, the very slice `schema` turns into
/// `inputSchema.properties`, so there is no second hand-kept list for the
/// two to drift apart on.
fn unknown_arguments(tool: &Tool, arguments: &Value) -> Vec<String> {
    let Some(map) = arguments.as_object() else {
        return Vec::new();
    };
    let mut names: Vec<String> = map
        .keys()
        .filter(|k| !tool.args.iter().any(|a| &a.name == k))
        .cloned()
        .collect();
    names.sort_unstable();
    names
}

fn call(project: &mut Project, params: &Value) -> Result<String, Failure> {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return Err(Failure::usage(format!(
            "no such tool: {name}. This server has: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )));
    };
    // `f438`/`d880`: refused before anything is read or written, the same
    // point the CLI refuses an unknown flag at. `arguments` is the only
    // place a caller's own keys live -- `_meta` sits beside it in `params`,
    // never inside it, so a protocol-level field never reaches this check.
    let unknown = unknown_arguments(tool, &params["arguments"]);
    if !unknown.is_empty() {
        let takes = if tool.args.is_empty() {
            "none".to_string()
        } else {
            tool.args
                .iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(" ")
        };
        return Err(Failure::usage(format!(
            "{name} does not take {}.\n\n  It takes: {takes}",
            unknown.join(" ")
        )));
    }
    let a = Reader::new(tool, params);
    let missing = |what: &str| Failure::usage(format!("{name} needs a {what}."));
    match name {
        "vivac_brief" => {
            let empty = Args::default();
            let name = project.name.clone();
            let ctx = project.current()?;
            brief::to_text(
                &ctx.tree,
                &ctx.store.root,
                &ctx.lane_dir,
                &empty,
                &name,
                false,
            )
        }
        "vivac_find" => {
            let query = a.str("query").ok_or_else(|| missing("query"))?.to_string();
            if a.bool("everywhere") {
                // `d916`: the project this server serves is the one asking.
                let own = project.current()?.store.config.project_id.clone();
                pretty(render::find_everywhere_data(&query, Some(&own))?)
            } else {
                pretty(render::find_data(&project.current()?.tree, &query)?)
            }
        }
        "vivac_why" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let full = a.bool("full");
            let only = a.bool("only");
            match a.str("project") {
                Some(spec) => {
                    // `d771`: `full` reads the whole log, and a foreign
                    // project's log is never read that way -- the same
                    // reason the CLI refuses `why --project --full`
                    // (`main.rs`, the `why` arm of `dispatch`).
                    if full {
                        return Err(Failure::usage(
                            "why --project does not take --full: --full reads the whole \
                             log, and a foreign project's log is never read that way.\n\n  \
                             Drop --full or drop --project."
                                .to_string(),
                        ));
                    }
                    let foreign_root = registry::resolve(spec)?;
                    // No `for_lane`: this folder is not a lane of the
                    // foreign tree, so it reads that tree's founding
                    // lane, the same as any tree nobody ran `setup` in.
                    // Defensible and not a lie today; it stops being one
                    // the day a foreign tree has a second lane (`t594`).
                    // No log either, for the same reason `--project` never
                    // reads one on the CLI: `lane` and `where` (`t594`
                    // §5.4) simply have nothing to answer from here.
                    let foreign = store::Store::open_from_elsewhere(foreign_root)?;
                    // `d916`: closed to every project but its own, the same
                    // refusal the CLI gives.
                    let own = project.current()?.store.config.project_id.clone();
                    if foreign.config.closed_to(Some(&own)) {
                        return Err(Failure::Model(format!(
                            "  {} keeps what it knows to itself: other projects cannot read it.",
                            render::project_name(&foreign.root)
                        )));
                    }
                    let tree = index::load(&foreign, false)?;
                    pretty(render::why_data(&tree, &[], &id, false, only)?)
                }
                // The resident log, kept for exactly this (`Project::log`'s
                // own doc): `lane` and `where` answer here the same way
                // they do for the CLI's local `why`, without folding the
                // log a second time.
                None => {
                    let (ctx, log) = project.current_with_log()?;
                    pretty(render::why_data(&ctx.tree, log, &id, full, only)?)
                }
            }
        }
        "vivac_open" => pretty(render::open_data(&project.current()?.tree)),
        "vivac_rules" => pretty(render::rules_data(&project.current()?.tree)),
        "vivac_push" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let why = a.str("why").ok_or_else(|| missing("why"))?.to_string();
            let p = params::Push {
                title,
                why,
                kind: a.str("type").map(str::to_string),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                arms: a.list("arm"),
                arm_dir: a.str("arm_dir").map(str::to_string),
                against: a.list("against"),
                via_mcp: true,
                root: a.bool("root"),
                parent: a.str("parent").map(str::to_string),
            };
            outcome_text(project.write(|ctx| ops::push(ctx, p))?)
        }
        "vivac_pop" => {
            let p = params::Pop {
                outcome: a.str("outcome").unwrap_or("").to_string(),
                next: a.str("next").map(str::to_string),
                force: a.bool("force"),
            };
            outcome_text(project.write(|ctx| ops::pop(ctx, p))?)
        }
        "vivac_done" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let p = params::Done {
                id,
                outcome: a.str("outcome").unwrap_or("").to_string(),
                force: false,
            };
            outcome_text(project.write(|ctx| ops::done(ctx, p))?)
        }
        "vivac_add" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let p = params::Add {
                title,
                parent: a.str("parent").map(str::to_string),
                kind: a.str("type").map(str::to_string),
                why: a.str("why").unwrap_or("").to_string(),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                arms: a.list("arm"),
                arm_dir: a.str("arm_dir").map(str::to_string),
                against: a.list("against"),
                via_mcp: true,
                root: a.bool("root"),
            };
            outcome_text(project.write(|ctx| ops::add(ctx, p))?)
        }
        "vivac_decide" => {
            let title = a.str("title").ok_or_else(|| missing("title"))?.to_string();
            let reason = a
                .str("reason")
                .ok_or_else(|| missing("reason"))?
                .to_string();
            let p = params::Decide {
                title,
                parent: a.str("parent").map(str::to_string),
                reason,
                alternatives: a.list("alternative"),
                supersedes: a.str("supersedes").map(str::to_string),
                refs: a.list("ref"),
                governs: a.list("governs"),
                blocks: a.bool("blocks"),
                against: a.list("against"),
                root: a.bool("root"),
            };
            outcome_text(project.write(|ctx| ops::decide(ctx, p))?)
        }
        // `id` given: the two words are unambiguous, the way `vivac note <id>
        // "<note>"` is. `id` left out: the note text takes the place a lone
        // positional would on the CLI, so `ops::note` attaches it to the
        // focus the same way `vivac note "<note>"` does.
        "vivac_note" => {
            let text = a.str("note").ok_or_else(|| missing("note"))?.to_string();
            let p = match a.str("id") {
                Some(id) => params::Note {
                    node: Some(id.to_string()),
                    note: Some(text),
                },
                None => params::Note {
                    node: Some(text),
                    note: None,
                },
            };
            outcome_text(project.write(|ctx| ops::note(ctx, p))?)
        }
        // Same shape as `vivac_note`: with no `id`, `reason` takes the place
        // of the single word `vivac park "<reason>"` would pass, and
        // `named_or_focus` is what resolves it against the focus.
        "vivac_park" => {
            let until = match a.str("until") {
                Some(u) => Some(params::validate_until(u)?),
                None => None,
            };
            let (node, reason) = match (a.str("id"), a.str("reason")) {
                (Some(id), Some(reason)) => (Some(id.to_string()), Some(reason.to_string())),
                (Some(id), None) => (Some(id.to_string()), None),
                (None, Some(reason)) => (Some(reason.to_string()), None),
                (None, None) => (None, None),
            };
            let p = params::Park {
                node,
                reason,
                until,
            };
            outcome_text(project.write(|ctx| ops::park(ctx, p))?)
        }
        "vivac_arm" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let command = a
                .str("command")
                .ok_or_else(|| missing("command"))?
                .to_string();
            let p = params::Arm {
                id,
                command,
                dir: a.str("dir").map(str::to_string),
                off: a.bool("off"),
                via_mcp: true,
            };
            outcome_text(project.write(|ctx| ops::arm(ctx, p))?)
        }
        "vivac_declare" => {
            let id = a.str("id").ok_or_else(|| missing("id"))?.to_string();
            let p = params::Declare {
                id: Some(id),
                against: a.list("against"),
            };
            outcome_text(project.write(|ctx| ops::declare(ctx, p))?)
        }
        "vivac_save" => {
            let p = params::Save {
                label: a.str("label").unwrap_or("").to_string(),
                next: a.str("next").unwrap_or("").to_string(),
            };
            let mut saved = project.write(|ctx| ops::save(ctx, p))?;
            // After `write` has released the lock: the check starts `git`.
            // A tree that cannot be read again only means no findings, and
            // the stop is written either way.
            if let Ok(ctx) = project.current() {
                ops::check_after_save(ctx, &mut saved);
            }
            outcome_text(saved)
        }
        other => unreachable!("{other} passed the tool lookup but no arm here handles it"),
    }
}

/// `f438`/`d880`: the set `unknown_arguments` checks a call against is
/// `tool.args`, the same slice `schema` reads to build `inputSchema`'s
/// `properties` -- one definition, read twice, rather than a second
/// hand-kept list that could say something else. Walks every tool and
/// checks the two readings still agree, name for name.
#[cfg(test)]
mod unknown_argument_tests {
    use super::*;

    #[test]
    fn every_tools_allowed_arguments_match_its_advertised_schema() {
        for t in TOOLS {
            let advertised_schema = schema(t);
            let mut advertised: Vec<&str> = advertised_schema["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            advertised.sort_unstable();
            let mut declared: Vec<&str> = t.args.iter().map(|a| a.name).collect();
            declared.sort_unstable();
            assert_eq!(
                advertised, declared,
                "{} advertises a schema that does not match its own args",
                t.name
            );
        }
    }
}

/// One line in, at most one line out. `None` is a notification, which by
/// definition is not answered: a reply nobody is waiting for would be read as
/// the answer to whatever comes next.
fn handle(project: &mut Project, line: &str) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(
                &Value::Null,
                -32700,
                &format!("that line is not JSON: {e}"),
            ))
        }
    };
    let id = message.get("id").cloned()?;
    let method = message["method"].as_str().unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => {
            let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            Some(ok(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "vivac", "version": env!("CARGO_PKG_VERSION") },
                }),
            ))
        }
        "ping" => Some(ok(&id, json!({}))),
        "tools/list" => Some(ok(
            &id,
            json!({ "tools": TOOLS.iter().map(schema).collect::<Vec<_>>() }),
        )),
        "tools/call" => Some(match call(project, &params) {
            Ok(text) => tool_ok(&id, text),
            Err(e) => tool_error(&id, e.message()),
        }),
        other => Some(rpc_error(
            &id,
            -32601,
            &format!("this server does not do {other}"),
        )),
    }
}

/// Writes one reply and flushes it, the two calls `serve` used to make
/// inline. Pulled out only so the two call sites below share one place to
/// turn a write failure into the `stderr` line `d883` asks for.
fn write_reply(output: &mut impl Write, reply: &str) -> std::io::Result<()> {
    writeln!(output, "{reply}")?;
    output.flush()
}

pub fn serve(root: PathBuf, located: Option<store::Located>) -> R {
    // Every reply on this channel is JSON-RPC read by a program, never a
    // terminal a person is looking at, even where the harness that spawned
    // this process exported `CLICOLOR_FORCE` or `COLUMNS` for its own
    // reasons. First thing, ahead of anything this server ever writes.
    crate::style::plain_only();
    // A resident server outlives every one of its own calls, and its own
    // `stderr` reaches nobody once it is running headless -- never a
    // terminal a person is reading, never the stream an agent parses
    // either. Declaring that here is what keeps `main`'s own
    // `registry::warn_if_wrote` from echoing the copy warning on `stderr`
    // once this process finally exits, whatever it wrote in between: this
    // server's seat for that warning is the brief, recomputed fresh on
    // every `vivac_brief` call for as long as it lives, not a one-shot
    // echo made for a process that runs once and is gone (`t594`).
    store::mark_resident();
    let mut registry = Registry::open(vec![root.clone()], vec![], located.map(|l| (root, l)))?;
    let project = registry.first();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout();
    // `read_until` instead of `lines()`: `lines()` turns a chunk of bytes
    // that is not valid UTF-8 into an `InvalidData` error indistinguishable
    // from a broken pipe, and the old loop let that one bad line end the
    // whole server (`f882`). A line that fails to parse as JSON already gets
    // a JSON-RPC error and the loop keeps going in `handle` above; a line
    // that fails to parse as UTF-8 at all gets the same treatment here
    // instead of taking the server down with it (`d883`).
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = match input.read_until(b'\n', &mut buf) {
            Ok(n) => n,
            Err(e) => {
                // The reason this server stopped exists nowhere else: this
                // process is headless, so its own `stderr` is the only place
                // left where anyone could ever read why, and the client
                // keeps exactly that stream in its own log (`f882`).
                eprintln!("vivac mcp: reading from the client failed ({e}), stopping.");
                return Err(Failure::Io(e));
            }
        };
        if read == 0 {
            // The client closing its end of the pipe is how this loop is
            // meant to end, not a failure -- but silence about it is what
            // `f882` found in a real session, so it gets a line too.
            eprintln!("vivac mcp: the client closed its end, stopping.");
            return Ok(());
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                let reply = rpc_error(&Value::Null, -32700, "that line is not UTF-8");
                if let Err(e) = write_reply(&mut output, &reply) {
                    eprintln!("vivac mcp: writing to the client failed ({e}), stopping.");
                    return Err(Failure::Io(e));
                }
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(project, line) {
            if let Err(e) = write_reply(&mut output, &reply) {
                eprintln!("vivac mcp: writing to the client failed ({e}), stopping.");
                return Err(Failure::Io(e));
            }
        }
    }
}

/// `t192`: a write used to build a brand new `Ctx` -- store reopened, index
/// reloaded, anchor re-walked -- on every single call, throwing away the
/// fold `Project` already keeps warm. These tests are the other half of
/// that fix: proof that operating on the resident tree instead of a fresh
/// one never leaves it holding something a fresh fold would not.
#[cfg(test)]
mod resident_write_tests {
    use super::*;
    use crate::model::{Node, Tree};
    use crate::store::Store;
    use std::path::Path;

    fn temp_project(name: &str) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!(
            "vivac-mcp-resident-{name}-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Store::create(&root).unwrap();
        let project = Project::open(root.clone(), "t".into(), "t".into(), ops::Whose::Founding)
            .unwrap_or_else(|e| panic!("{}", e.message()));
        (root, project)
    }

    fn cleanup(root: &Path) {
        std::fs::remove_dir_all(root).ok();
    }

    fn call_tool(project: &mut Project, name: &str, arguments: Value) -> Value {
        let params = json!({ "name": name, "arguments": arguments });
        let text = call(project, &params).unwrap_or_else(|e| panic!("{name}: {}", e.message()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} did not reply JSON: {e}"))
    }

    /// A deterministic, order-independent rendering of a whole `Tree`:
    /// `nodes_sorted` fixes the node order and every field is resolved
    /// through the tree that owns it, so two trees folded from the same
    /// log compare equal here even when the `HashMap`s backing them were
    /// built in a different order.
    fn dump_node(tree: &Tree, n: &Node) -> String {
        format!(
            "node num={} id={} kind={:?} state={:?} parent={:?} blocks={} \
             forced_close={} title={:?} why={:?} note={:?} outcome={:?} \
             opened={:?} closed={:?} refs={:?} governs={:?} flags={:?} \
             review_on={:?} notes_since_child={}\n",
            n.num,
            n.id,
            n.kind,
            n.state,
            n.parent,
            n.blocks,
            n.forced_close,
            n.title(tree),
            n.why(tree),
            n.note(tree),
            n.outcome(tree),
            n.opened(tree),
            n.closed(tree),
            n.refs(tree),
            n.governs(tree),
            n.flags,
            n.review_on(tree),
            n.notes_since_child,
        )
    }

    fn dump_tree(tree: &Tree) -> String {
        let mut out = format!(
            "roots={:?} seq={} next_num={} next_vivac_num={} broken={} main_claimed={}\n",
            tree.roots,
            tree.seq,
            tree.next_num,
            tree.next_vivac_num,
            tree.broken_lines,
            tree.main_claimed,
        );
        // Every lane, not only the one this tree is looked at from: the
        // property this dump exists for (`t192`) compares the resident
        // tree against a fresh fold of the same log, and a divergence in a
        // lane nobody is looking from right now would otherwise go unseen.
        for (key, s) in &tree.lanes {
            out.push_str(&format!(
                "lane key={key:?} name={:?} repos={:?} stack={:?} seq_change={} \
                 seq_vivac={} seg_new={} seg_closed={} seg_notes={} seg_events={}\n",
                s.name,
                s.repos,
                s.stack,
                s.seq_change,
                s.seq_vivac,
                s.seg_new,
                s.seg_closed,
                s.seg_notes,
                s.seg_events,
            ));
        }
        for n in tree.nodes_sorted() {
            out.push_str(&dump_node(tree, n));
        }
        for v in &tree.vivacs {
            out.push_str(&format!(
                "vivac num={} id={} seq={} lane={:?} kind={:?} stack={:?} working_set={:?} \
                 next_intent={:?} anchor={:?} node_ref={:?} label={:?} ts={:?}\n",
                v.num,
                v.id,
                v.seq,
                v.lane,
                v.kind,
                v.stack,
                v.working_set,
                v.next_intent,
                v.anchor,
                v.node_ref,
                v.label,
                v.ts,
            ));
        }
        out
    }

    /// The property `t192` exists for: whatever the resident tree holds
    /// after the call, folding the log from scratch has to hold the exact
    /// same thing.
    fn assert_resident_matches_fresh_fold(root: &Path, project: &mut Project) {
        let resident = dump_tree(
            &project
                .current()
                .unwrap_or_else(|e| panic!("{}", e.message()))
                .tree,
        );
        let fresh = dump_tree(
            &ops::Ctx::load(
                Store::open(root.to_path_buf()).unwrap(),
                ops::Whose::Founding,
            )
            .unwrap_or_else(|e| panic!("{}", e.message()))
            .tree,
        );
        assert_eq!(
            resident, fresh,
            "the resident tree diverged from a fresh fold of the same log"
        );
    }

    #[test]
    fn push_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("push");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn pop_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("pop");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(&mut project, "vivac_pop", json!({"outcome": "it shipped"}));
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn add_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("add");
        call_tool(
            &mut project,
            "vivac_add",
            json!({"title": "A finding", "why": "noticed in passing"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn decide_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("decide");
        call_tool(
            &mut project,
            "vivac_decide",
            json!({"title": "Rotate keys", "reason": "the old ones leaked"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `f590`: a write stamped the log and the resident tree from two reads
    /// of the clock, so the two came out a second apart whenever the second
    /// turned in between, which CI saw once in a while. With a clock that
    /// turns on every read it would show every time.
    #[test]
    fn a_clock_that_turns_on_every_read_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let _clock = crate::clock::Ticking::start(1_757_000_000);
        let (root, mut project) = temp_project("ticking");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_note",
            json!({"note": "the rollback plan is untested"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn note_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("note");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_note",
            json!({"note": "the rollback plan is untested"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn park_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("park");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_park",
            json!({"reason": "waiting on the security review"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn save_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("save");
        call_tool(
            &mut project,
            "vivac_save",
            json!({"label": "before the migration", "next": "run the reconcile"}),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    #[test]
    fn declare_leaves_the_resident_tree_equal_to_a_fresh_fold() {
        let (root, mut project) = temp_project("declare");
        call_tool(
            &mut project,
            "vivac_add",
            json!({"title": "Security", "type": "pillar", "why": "vetoes on the spot"}),
        );
        call_tool(
            &mut project,
            "vivac_add",
            json!({
                "title": "Keep the write path local",
                "parent": "1",
                "type": "rule",
                "why": "guard",
            }),
        );
        call_tool(
            &mut project,
            "vivac_decide",
            json!({"title": "Keep it local", "reason": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_declare",
            json!({
                "id": "d3",
                "against": ["r2: nothing on the write path calls the network"],
            }),
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `d783`: a second `vivac_declare` on the same target substitutes the
    /// sentence rather than being refused, and the reply's `against` entry
    /// carries what it replaced.
    #[test]
    fn declare_reports_the_sentence_it_substituted() {
        let (root, mut project) = temp_project("declare-substitute");
        call_tool(
            &mut project,
            "vivac_add",
            json!({"title": "Security", "type": "pillar", "why": "vetoes on the spot"}),
        );
        call_tool(
            &mut project,
            "vivac_add",
            json!({
                "title": "Keep the write path local",
                "parent": "1",
                "type": "rule",
                "why": "guard",
            }),
        );
        call_tool(
            &mut project,
            "vivac_decide",
            json!({"title": "Keep it local", "reason": "because"}),
        );
        call_tool(
            &mut project,
            "vivac_declare",
            json!({
                "id": "d3",
                "against": ["r2: nothing on the write path calls the network"],
            }),
        );
        let reply = call_tool(
            &mut project,
            "vivac_declare",
            json!({"id": "d3", "against": ["r2: a second sentence"]}),
        );
        assert_eq!(reply["against"][0]["node"], "r2", "{reply}");
        assert_eq!(reply["against"][0]["why"], "a second sentence", "{reply}");
        assert_eq!(
            reply["against"][0]["before"], "nothing on the write path calls the network",
            "{reply}"
        );
        assert!(
            reply["text"]
                .as_str()
                .unwrap()
                .contains("before: nothing on the write path calls the network"),
            "{reply}"
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// The other half of `LOADING.md` §4's rule: `load_for_write` exists so
    /// that a write never pays to rewrite the derived index, and the
    /// resident path replacing it must not quietly start doing that.
    #[test]
    fn a_resident_write_never_persists_the_index() {
        let (root, mut project) = temp_project("index");
        call_tool(
            &mut project,
            "vivac_push",
            json!({"title": "Ship it", "why": "because"}),
        );
        assert!(
            !Store::open(root.clone()).unwrap().index_path().exists(),
            "a write through the resident Ctx must never persist the index"
        );
        cleanup(&root);
    }

    fn add(title: &str) -> crate::params::Add {
        crate::params::Add {
            title: title.into(),
            parent: None,
            kind: None,
            why: "t".into(),
            refs: vec![],
            governs: vec![],
            blocks: false,
            arms: vec![],
            arm_dir: None,
            against: vec![],
            via_mcp: false,
            root: true,
        }
    }

    /// `f599`: another process's append reaches the resident tree as a
    /// tail, not as a second fold of the whole log.
    #[test]
    fn a_tail_another_process_appended_is_applied_without_a_full_fold() {
        let (root, mut project) = temp_project("tail");
        project
            .write(|ctx| ops::add(ctx, add("From the server")))
            .unwrap();

        let mut other =
            crate::ops::Ctx::load(Store::open(root.clone()).unwrap(), ops::Whose::Founding)
                .unwrap();
        other.lock_for_write().unwrap();
        ops::add(&mut other, add("From another process")).unwrap();
        other.unlock();

        let folds_before = project.full_folds;
        project.current().unwrap();
        assert_eq!(
            project.full_folds, folds_before,
            "it folded the whole log again"
        );
        assert_resident_matches_fresh_fold(&root, &mut project);

        let (_, log) = project.current_with_log().unwrap();
        assert!(
            log.iter().any(|e| matches!(&e.payload,
                crate::event::Body::NodeCreated { title, .. } if title == "From another process")),
            "what the other process wrote must reach the events this Project keeps beside the tree"
        );
        cleanup(&root);
    }

    /// A log changed underneath, not just grown, is folded whole: there is
    /// no tail to trust.
    #[test]
    fn a_log_rewritten_underneath_is_folded_whole() {
        let (root, mut project) = temp_project("rewritten");
        project
            .write(|ctx| ops::add(ctx, add("Before the edit")))
            .unwrap();
        let log = root.join(".vivac").join("events");
        let text = std::fs::read_to_string(&log).unwrap();
        // One blank line at the front: the log is a byte longer, so it "grew",
        // and every offset after it moved, so the event the last fold read is
        // no longer where it was. A reader skips the blank line, so a fresh
        // fold sees exactly the same events.
        std::fs::write(&log, format!("\n{text}")).unwrap();

        let folds_before = project.full_folds;
        project.current().unwrap();
        assert_eq!(project.full_folds, folds_before + 1);
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `f608`, third time (`t594`): the lane used to live
    /// on the `Store` itself, and `Ctx::refold` opened a fresh one with
    /// `Store::open` -- which always started out signing `main` -- and
    /// never reapplied the lane the `Ctx` around it was actually running
    /// as. A resident project serving a joined lane that reloaded whole
    /// after a torn write kept reading correctly (`adopt` still worked)
    /// but signed every write after that `main`, from the door the agent
    /// actually writes through. The lane is an argument to `append` now,
    /// not a field anything can leave stale.
    #[test]
    fn a_resident_reload_after_a_torn_write_keeps_signing_its_own_lane() {
        let root = std::env::temp_dir().join(format!(
            "vivac-mcp-resident-reload-lane-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Store::create(&root).unwrap();
        let located = crate::store::Located {
            root: root.clone(),
            lane_dir: root.clone(),
            lane: Some(crate::lane::Lane {
                version: 1,
                id: "b".to_string(),
                project: String::new(),
            }),
            worktree: None,
        };
        let mut project = Project::open(
            root.clone(),
            "t".into(),
            "t".into(),
            ops::Whose::Resolved(&located),
        )
        .unwrap_or_else(|e| panic!("{}", e.message()));
        project
            .write(|ctx| ops::add(ctx, add("Before the reload")))
            .unwrap();

        // Same trick `a_log_rewritten_underneath_is_folded_whole` uses to
        // force the next read through a full `refold` rather than a tail.
        let log = root.join(".vivac").join("events");
        let text = std::fs::read_to_string(&log).unwrap();
        std::fs::write(&log, format!("\n{text}")).unwrap();
        project.current().unwrap();

        project
            .write(|ctx| ops::add(ctx, add("After the reload")))
            .unwrap();

        let (_, log) = project.current_with_log().unwrap();
        let after = log
            .iter()
            .find(|e| {
                matches!(&e.payload, crate::event::Body::NodeCreated { title, .. } if title == "After the reload")
            })
            .expect("the write after the reload landed");
        assert_eq!(
            after.lane, "b",
            "the write after refold signed {:?} instead of its own lane",
            after.lane
        );
        cleanup(&root);
    }

    /// What the server itself writes lands in `log` too, beside the tree
    /// it was already applied to.
    #[test]
    fn the_servers_own_writes_reach_its_log() {
        let (root, mut project) = temp_project("own");
        project.write(|ctx| ops::add(ctx, add("Mine"))).unwrap();
        let (_, log) = project.current_with_log().unwrap();
        assert!(log.iter().any(|e| matches!(&e.payload,
            crate::event::Body::NodeCreated { title, .. } if title == "Mine")));
        cleanup(&root);
    }

    /// `f599`: an unterminated tail is not consumed, so a reader that meets
    /// it more than once must not keep adding it to a running total -- the
    /// resident tree would end up with more broken lines than a fresh fold
    /// counts, and stay that way forever, since the tail itself never
    /// becomes a line for anyone to consume.
    #[test]
    fn a_torn_tail_is_counted_once_however_many_times_it_is_read() {
        let (root, mut project) = temp_project("torn");
        project
            .write(|ctx| ops::add(ctx, add("Before the tear")))
            .unwrap();
        let log_path = root.join(".vivac").join("events");
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .unwrap();
            f.write_all(b"{\"seq\":2,\"id\":\"torn").unwrap();
        }

        project.current().unwrap();
        assert_resident_matches_fresh_fold(&root, &mut project);

        let mut other =
            crate::ops::Ctx::load(Store::open(root.clone()).unwrap(), ops::Whose::Founding)
                .unwrap();
        other.lock_for_write().unwrap();
        ops::add(&mut other, add("From another process")).unwrap();
        other.unlock();

        project.current().unwrap();
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `f599`: a write that lands behind a torn tail is not a clean
    /// continuation of what this server already folded -- the tail's own
    /// bytes sit between `fold_end` and where the write's own offsets say
    /// its lines begin, so the next refresh cannot trust them and folds
    /// the whole log instead.
    #[test]
    fn a_write_behind_a_torn_tail_is_folded_whole() {
        let (root, mut project) = temp_project("torn-write");
        project
            .write(|ctx| ops::add(ctx, add("Before the tear")))
            .unwrap();
        let log_path = root.join(".vivac").join("events");
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .unwrap();
            f.write_all(b"{\"seq\":2,\"id\":\"torn").unwrap();
        }

        let folds_before = project.full_folds;
        project
            .write(|ctx| ops::add(ctx, add("Behind the tear")))
            .unwrap();

        project.current().unwrap();
        assert_eq!(
            project.full_folds,
            folds_before + 1,
            "offsets from a write behind an unconsumed torn tail must not be trusted for a tail"
        );
        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `f602`, round two: the full-fold branch of `refresh_if_stale` used to
    /// replace `self.ctx` outright, and `write` had already taken the lock
    /// before calling it. Replacing the whole context dropped that lock on
    /// the floor, so the write that followed inside the same call failed
    /// with "write without the tree's lock" instead of writing. Truncating
    /// the log short of `fold_end` is what forces the full fold from inside
    /// a write that already holds the lock, rather than from a later read.
    #[test]
    fn a_full_fold_forced_inside_a_write_still_writes() {
        let (root, mut project) = temp_project("full-fold-inside-write");
        project
            .write(|ctx| ops::add(ctx, add("Before the tear")))
            .unwrap();

        let log_path = root.join(".vivac").join("events");
        let text = std::fs::read_to_string(&log_path).unwrap();
        // Shorter than `fold_end`: on its own enough to force the full-fold
        // branch, the way a process that died mid-append would leave it.
        // Closed with its own newline, so the write that follows starts a
        // line of its own instead of running on from this one -- that
        // merge is `f599`'s own territory, not this test's.
        std::fs::write(&log_path, format!("{}\n", &text[..text.len() - 6])).unwrap();

        project
            .write(|ctx| ops::add(ctx, add("After the fold was forced")))
            .unwrap();

        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }

    /// `f599`: a log that was replaced, not grown -- a copy restored over
    /// it, a sync client, an edit by hand -- leaves an open handle reading
    /// the old file forever, unless the handle's own fingerprint is
    /// checked against the path's before it is trusted.
    #[test]
    fn a_replaced_log_is_read_fresh_not_through_the_old_handle() {
        let (root, mut project) = temp_project("replaced");
        project
            .write(|ctx| ops::add(ctx, add("Before the swap")))
            .unwrap();

        // Built somewhere else entirely, so its content is simply
        // different from what this `Project` already folded, and the
        // swap onto the log's own path is the one moment that matters.
        let scratch_root = std::env::temp_dir().join(format!(
            "vivac-mcp-resident-replaced-scratch-{}-{}",
            std::process::id(),
            crate::id::ulid()
        ));
        std::fs::create_dir_all(&scratch_root).unwrap();
        Store::create(&scratch_root).unwrap();
        let mut scratch = crate::ops::Ctx::load(
            Store::open(scratch_root.clone()).unwrap(),
            ops::Whose::Founding,
        )
        .unwrap();
        scratch.lock_for_write().unwrap();
        ops::add(&mut scratch, add("After the swap")).unwrap();
        ops::add(&mut scratch, add("Also after the swap")).unwrap();
        scratch.unlock();

        let log_path = root.join(".vivac").join("events");
        let replacement_path = root.join(".vivac").join("events.replacement");
        std::fs::copy(
            scratch_root.join(".vivac").join("events"),
            &replacement_path,
        )
        .unwrap();
        std::fs::rename(&replacement_path, &log_path).unwrap();
        std::fs::remove_dir_all(&scratch_root).ok();

        assert_resident_matches_fresh_fold(&root, &mut project);
        cleanup(&root);
    }
}
