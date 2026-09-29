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
    pub arms: Vec<String>,
    pub arm_dir: Option<String>,
    /// Only on a decision: what it was judged against. `t426` §2.1.
    pub against: Vec<String>,
    /// Whether this call arrived over MCP rather than the CLI: the only
    /// thing it changes is which vocabulary a missing-folder message uses
    /// -- `arm_dir` there, `--arm-dir` here (`t411` §21).
    pub via_mcp: bool,
    /// Born at the root, with no parent, instead of under the focus. `t533`
    /// §1: the stack is left holding only the new node.
    pub root: bool,
    /// Born under this node instead of the focus: the stack is rebuilt to
    /// its own path first. Refused together with `root`, and on a node
    /// that is closed, abandoned or parked (`d757`).
    pub parent: Option<String>,
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
            arms: a.list("arm"),
            arm_dir: a.opt("arm-dir").map(str::to_string),
            against: a.list("against"),
            via_mcp: false,
            root: a.has("root"),
            parent: a.opt("parent").map(str::to_string),
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
    /// `--until`'s own civil date, already validated: real, strictly
    /// `YYYY-MM-DD`, and strictly after today's local date. Validated here
    /// rather than in the operation because it needs only the clock, never
    /// the tree -- the same reason `Flag::from_args` above checks its own
    /// `why` is given unless `--off`. `d899`.
    pub until: Option<String>,
}

impl Park {
    pub fn from_args(a: &Args) -> Result<Park, Failure> {
        let until = match a.opt("until") {
            Some(s) => Some(validate_until(s)?),
            None => None,
        };
        Ok(Park {
            node: a.positional(0).map(str::to_string),
            reason: a.positional(1).map(str::to_string),
            until,
        })
    }
}

/// `--until`'s own validation. Relative forms such as `+7d` or `tomorrow`
/// are refused on purpose: a park is read back long after it is written, and
/// a relative date only ever meant something at the moment it was typed.
/// `pub(crate)` so `mcp.rs`'s own `vivac_park` can share it rather than
/// re-implement the same check against `until` given as JSON instead of a
/// flag. `d899`.
pub(crate) fn validate_until(s: &str) -> Result<String, Failure> {
    validate_day(s, "--until", "a park comes back on a day still ahead.")
}

/// `--on`'s own validation, the same check as [`validate_until`] under its
/// own name and with its own closing sentence. `d906`.
fn validate_on(s: &str) -> Result<String, Failure> {
    validate_day(s, "--on", "a review is set for a day still ahead.")
}

/// What `--until` and `--on` share: a real civil date, written exactly as
/// `YYYY-MM-DD`, strictly after today's local date. `flag` is the option's
/// own name as the message quotes it, and `closing` the sentence that closes a
/// refusal for a day that is not ahead.
fn validate_day(s: &str, flag: &str, closing: &str) -> Result<String, Failure> {
    if !crate::clock::is_civil_date(s) {
        return Err(Failure::usage(format!(
            "{flag} takes a date as YYYY-MM-DD."
        )));
    }
    let today = crate::clock::today_local();
    if s <= today.as_str() {
        let why = if s == today {
            "is today"
        } else {
            "has already passed"
        };
        return Err(Failure::usage(format!("{flag} {s} {why}; {closing}")));
    }
    Ok(s.to_string())
}

pub struct Add {
    pub title: String,
    pub parent: Option<String>,
    pub kind: Option<String>,
    pub why: String,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub blocks: bool,
    pub arms: Vec<String>,
    pub arm_dir: Option<String>,
    /// Only on a decision: what it was judged against. `t426` §2.1.
    pub against: Vec<String>,
    /// See `Push::via_mcp`.
    pub via_mcp: bool,
    /// See `Push::root`. Refused together with `parent`.
    pub root: bool,
}

impl Add {
    pub fn from_args(a: &Args) -> Result<Add, Failure> {
        let title = a.positional(0).ok_or_else(|| {
            Failure::usage(
                "usage: vivac add \"<title>\" [--parent N | --root] [--why \"<reason>\"]",
            )
        })?;
        Ok(Add {
            title: title.to_string(),
            parent: a.opt("parent").map(str::to_string),
            kind: a.opt("type").map(str::to_string),
            why: a.opt_or("why"),
            refs: a.list("ref"),
            governs: a.list("governs"),
            blocks: a.has("blocks"),
            arms: a.list("arm"),
            arm_dir: a.opt("arm-dir").map(str::to_string),
            against: a.list("against"),
            via_mcp: false,
            root: a.has("root"),
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
    /// `--on`'s own review date, already validated: real, strictly
    /// `YYYY-MM-DD`, strictly after today's local date, and never beside
    /// `--off`. That it goes with `review` alone is checked in the
    /// operation, next to where the flag is interpreted. `d906`.
    pub on: Option<String>,
}

impl Flag {
    pub fn from_args(a: &Args) -> Result<Flag, Failure> {
        let (Some(sid), Some(sb)) = (a.positional(0), a.positional(1)) else {
            return Err(Failure::usage(
                "usage: vivac flag <id> <flag> --why \"<reason>\" [--on YYYY-MM-DD]  |  --off\n\n  \
                 Flags: suspect, review, stale",
            ));
        };
        let on = match a.opt("on") {
            Some(s) => Some(validate_on(s)?),
            None => None,
        };
        if on.is_some() && a.has("off") {
            return Err(Failure::usage(
                "--on and --off do not go together: --off clears the flag.",
            ));
        }
        Ok(Flag {
            id: sid.to_string(),
            flag: sb.to_string(),
            off: a.has("off"),
            why: a.opt("why").map(str::to_string),
            on,
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
    /// What it was judged against. `t426` §2.1.
    pub against: Vec<String>,
    /// See `Push::root`. Refused together with `parent`.
    pub root: bool,
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
            against: a.list("against"),
            root: a.has("root"),
        })
    }
}

/// `declare <decision> --against "<id>: <why>"`. `t426` §2.2.
pub struct Declare {
    pub id: Option<String>,
    pub against: Vec<String>,
}

impl Declare {
    pub fn from_args(a: &Args) -> Result<Declare, Failure> {
        Ok(Declare {
            id: a.positional(0).map(str::to_string),
            against: a.list("against"),
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

pub struct Arm {
    pub id: String,
    pub command: String,
    pub dir: Option<String>,
    pub off: bool,
    /// See `Push::via_mcp`.
    pub via_mcp: bool,
}

impl Arm {
    pub fn from_args(a: &Args) -> Result<Arm, Failure> {
        let (Some(id), Some(command)) = (a.positional(0), a.positional(1)) else {
            return Err(Failure::usage(
                "usage: vivac arm <rule> \"<command>\" --dir <dir> [--off]",
            ));
        };
        Ok(Arm {
            id: id.to_string(),
            command: command.to_string(),
            dir: a.opt("dir").map(str::to_string),
            off: a.has("off"),
            via_mcp: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Ticking;

    fn args(words: &[&str]) -> Args {
        Args::parse(words.iter().map(|s| s.to_string())).unwrap()
    }

    /// `2026-08-31` is `20_696` days since the epoch (`clock::tests::known_dates`),
    /// so a `Ticking` started there reads today as `2026-08-31` for the rest
    /// of this test.
    fn today_is_2026_08_31() -> Ticking {
        Ticking::start(20_696 * 86_400)
    }

    #[test]
    fn no_until_is_none() {
        let p = Park::from_args(&args(&["t1", "waiting"])).unwrap();
        assert_eq!(p.until, None);
    }

    #[test]
    fn a_date_strictly_after_today_is_accepted() {
        let _t = today_is_2026_08_31();
        let p = Park::from_args(&args(&["t1", "waiting", "--until", "2026-09-01"])).unwrap();
        assert_eq!(p.until, Some("2026-09-01".to_string()));
    }

    /// `Park::from_args`'s own refusal message, or a panic if it did not
    /// refuse -- every refusal it raises is a usage error, and `Park` is not
    /// `Debug`, so `unwrap_err` is not an option here.
    fn refusal(r: Result<Park, Failure>) -> String {
        match r {
            Ok(_) => panic!("expected a refusal, got Ok"),
            Err(Failure::Usage(m)) => m,
            Err(other) => panic!("expected a usage failure, got {other:?}"),
        }
    }

    #[test]
    fn a_past_date_is_refused() {
        let _t = today_is_2026_08_31();
        let m = refusal(Park::from_args(&args(&[
            "t1",
            "waiting",
            "--until",
            "2026-08-30",
        ])));
        assert!(m.contains("has already passed"), "{m}");
    }

    #[test]
    fn a_same_day_date_is_refused() {
        let _t = today_is_2026_08_31();
        let m = refusal(Park::from_args(&args(&[
            "t1",
            "waiting",
            "--until",
            "2026-08-31",
        ])));
        assert!(m.contains("is today"), "{m}");
    }

    #[test]
    fn a_short_form_is_refused() {
        let _t = today_is_2026_08_31();
        let m = refusal(Park::from_args(&args(&[
            "t1", "waiting", "--until", "2026-9-1",
        ])));
        assert!(m.contains("YYYY-MM-DD"), "{m}");
    }

    #[test]
    fn an_invalid_calendar_date_is_refused() {
        let _t = today_is_2026_08_31();
        let m = refusal(Park::from_args(&args(&[
            "t1",
            "waiting",
            "--until",
            "2026-02-30",
        ])));
        assert!(m.contains("YYYY-MM-DD"), "{m}");
    }

    #[test]
    fn a_relative_form_is_refused() {
        let _t = today_is_2026_08_31();
        let m = refusal(Park::from_args(&args(&[
            "t1", "waiting", "--until", "tomorrow",
        ])));
        assert!(m.contains("YYYY-MM-DD"), "{m}");
    }

    // `d906`: `flag --on`, the same check `--until` gets under its own name.

    /// [`refusal`] for `Flag`, which is not `Debug` either.
    fn flag_refusal(r: Result<Flag, Failure>) -> String {
        match r {
            Ok(_) => panic!("expected a refusal, got Ok"),
            Err(Failure::Usage(m)) => m,
            Err(other) => panic!("expected a usage failure, got {other:?}"),
        }
    }

    #[test]
    fn no_on_is_none() {
        let p = Flag::from_args(&args(&["t1", "review", "--why", "look again"])).unwrap();
        assert_eq!(p.on, None);
    }

    #[test]
    fn an_on_date_strictly_after_today_is_accepted() {
        let _t = today_is_2026_08_31();
        let p = Flag::from_args(&args(&[
            "t1",
            "review",
            "--why",
            "look again",
            "--on",
            "2026-09-01",
        ]))
        .unwrap();
        assert_eq!(p.on, Some("2026-09-01".to_string()));
    }

    #[test]
    fn an_on_date_that_has_passed_is_refused_in_its_own_words() {
        let _t = today_is_2026_08_31();
        let m = flag_refusal(Flag::from_args(&args(&[
            "t1",
            "review",
            "--why",
            "x",
            "--on",
            "2026-08-30",
        ])));
        assert_eq!(
            m.trim(),
            "--on 2026-08-30 has already passed; a review is set for a day still ahead."
        );
    }

    #[test]
    fn an_on_date_of_today_is_refused_in_its_own_words() {
        let _t = today_is_2026_08_31();
        let m = flag_refusal(Flag::from_args(&args(&[
            "t1",
            "review",
            "--why",
            "x",
            "--on",
            "2026-08-31",
        ])));
        assert_eq!(
            m.trim(),
            "--on 2026-08-31 is today; a review is set for a day still ahead."
        );
    }

    #[test]
    fn a_malformed_on_date_is_refused() {
        let _t = today_is_2026_08_31();
        let m = flag_refusal(Flag::from_args(&args(&[
            "t1", "review", "--why", "x", "--on", "2026-9-1",
        ])));
        assert_eq!(m.trim(), "--on takes a date as YYYY-MM-DD.");
    }

    #[test]
    fn on_beside_off_is_refused() {
        let _t = today_is_2026_08_31();
        let m = flag_refusal(Flag::from_args(&args(&[
            "t1",
            "review",
            "--off",
            "--on",
            "2026-09-01",
        ])));
        assert_eq!(
            m.trim(),
            "--on and --off do not go together: --off clears the flag."
        );
    }
}
