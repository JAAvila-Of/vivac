//! `WEB.md` §3.1 -- the Today page, and the index of projects it is reached
//! through.
//!
//! `today_page` is the surface `mod.rs` docs as "the real page": everything
//! below builds it, section by section, out of the same functions the CLI
//! reads the tree with. This module knows nothing about a socket; `mod.rs`
//! is what turns its output into a response.

use super::{alias_link, escape, FAVICON};
use crate::changes::{self, Boundary, Changed};
use crate::event::Kind;
use crate::event::{Event, State};
use crate::model::{Node, Tree};

/// One project as the index sees it: the four fields `d200` admitted, the
/// one `d817` did, and the link to reach it by.
///
/// Each was argued against the promise: the name so you know which it is,
/// the focus so you know what it was doing, the blocked count because a
/// blocked front is the commonest reason a project stops without anyone
/// noticing, and the silence because that is the whole question. `d817`
/// added the last stop you made and whether anything moved after it: a
/// project left with work past its last save point is where picking up
/// costs the most, and `changes --since manual` already answered it one
/// project at a time. `d200` says a counter that cannot answer the promise
/// does not get in, so nothing else does.
struct Line {
    href: String,
    name: String,
    focus: Option<Focus>,
    blocked: usize,
    quiet: Option<i64>,
    stop: Option<LastStop>,
}

/// What a project's focus is, and where: `lane` carries the multi-lane
/// sentence `line` used to glue onto the title with two spaces, now a line
/// of its own.
struct Focus {
    alias: String,
    title: String,
    lane: Option<String>,
}

/// The last stop made by hand, and whether the stretch since it moved
/// anything (`d817`). Distinct from `Boundary`: this is the shape a page
/// prints, not the shape `changes` measures from.
enum LastStop {
    /// The log holds work and no stop in it was made by hand.
    NeverByHand,
    /// The last stop made by hand: how many days ago (`None` if its date
    /// does not parse), its date as a fallback, and whether anything moved
    /// after it.
    Made {
        days: Option<i64>,
        date: String,
        work_since: bool,
    },
}

/// Reads one project into a [`Line`], re-folding it if the log moved.
///
/// The link prefers the ULID (`d374`): it is the form that keeps working
/// when a second project of the same name joins the registry, and the index
/// is exactly the page that shows both of them at once. A tree with no first
/// event has no ULID yet, and falls back to the readable form -- which is
/// unambiguous or it would not be reachable from here anyway.
fn line(p: &mut crate::project::Project) -> Line {
    // Both read before the fold is borrowed, which is what the refresh below
    // needs `p` mutably for.
    let name = p.name.clone();
    let href = p.slug.clone();
    let Ok((ctx, log)) = p.current_with_log() else {
        // A store that cannot be read is still a project the reader has:
        // saying so is the answer, and dropping the row would hide it.
        return Line {
            href,
            name,
            focus: None,
            blocked: 0,
            quiet: None,
            stop: None,
        };
    };
    let href = log.first().map(|e| e.id.clone()).unwrap_or(href);
    let tree = &ctx.tree;
    let lanes = crate::brief::lanes_with_a_stack(tree);
    // `t594` §5.6: the same substitution `stack_section` makes, for the
    // same reason -- this project's own `tree.lane()` is a guess for every
    // caller `Registry::open` did not start in, so once a second lane has
    // something to name, the lane that wrote most recently is worth more
    // than whichever one the guess landed on.
    let focus = if lanes.len() > 1 {
        crate::brief::last_writer(tree).map(|w| Focus {
            alias: w.focus.alias(),
            title: w.focus.title(tree).to_string(),
            lane: Some(format!("in lane {}, 1 of {} lanes", w.name, lanes.len())),
        })
    } else {
        tree.focus().map(|n| Focus {
            alias: n.alias(),
            title: n.title(tree).to_string(),
            lane: None,
        })
    };
    let blocked = tree
        .nodes_iter()
        .filter(|n| n.kind == Kind::Question && n.state.is_open() && n.blocks)
        .count();
    let quiet = log
        .last()
        .and_then(|e| crate::clock::days_between(&e.ts, &crate::clock::now_rfc3339()));
    let stop = last_stop(tree, log);
    Line {
        href,
        name,
        focus,
        blocked,
        quiet,
        stop,
    }
}

/// The last stop made by hand, and whether the stretch since it moved
/// anything (`d817`). `None` for a project whose log has nothing in it yet
/// -- there is no "since" to measure on a project that never wrote.
fn last_stop(tree: &Tree, log: &[Event]) -> Option<LastStop> {
    if log.is_empty() {
        return None;
    }
    Some(match changes::manual_boundary(tree) {
        Boundary::Beginning { .. } => LastStop::NeverByHand,
        Boundary::Stop { vivac, .. } => LastStop::Made {
            days: crate::clock::days_between(&vivac.ts, &crate::clock::now_rfc3339()),
            date: crate::clock::date_of(&vivac.ts),
            work_since: !changes::collect(tree, log, vivac.seq).nothing_moved(),
        },
    })
}

/// How long ago a day count reads: `today`, `yesterday`, or a count of days.
/// Shared by `silence`, about the whole project, and `stop_line`, about the
/// one stop made by hand -- the same distance read the same way twice would
/// drift the moment only one of them changed.
fn ago(days: i64) -> String {
    match days {
        d if d <= 0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d => format!("{d} days ago"),
    }
}

/// How long the silence reads. Days and not hours: the promise is about a
/// project that has been still for *days*, and an hour count would invite
/// reading this page for movement it is not measuring.
fn silence(days: Option<i64>) -> String {
    match days {
        None => "never written to".to_string(),
        Some(d) => format!("moved {}", ago(d)),
    }
}

/// The last-stop line the state block prints (`d817`): a project a person
/// has never sat down and stopped, or the last time they did and whether
/// the tree moved after it.
fn stop_line(s: &LastStop) -> String {
    match s {
        LastStop::NeverByHand => "no stop made by hand".to_string(),
        LastStop::Made {
            days,
            date,
            work_since,
        } => {
            let when = days.map(ago).unwrap_or_else(|| date.clone());
            if *work_since {
                format!("last stop you made: {when}, work since")
            } else {
                format!("last stop you made: {when}, nothing since")
            }
        }
    }
}

/// One card, shared by the index and by the page that asks which of two
/// projects you meant. **No path ever appears here.** The security pillar
/// allows a project's name across this boundary and nothing else, and a real
/// path carries the name of whoever owns the machine.
fn project_row(l: &Line) -> String {
    let (focus, lane) = match &l.focus {
        Some(f) => (
            format!(
                "<p class=\"focus\"><span class=\"alias\">{}</span> {}</p>",
                escape(&f.alias),
                escape(&f.title)
            ),
            match &f.lane {
                Some(lane) => format!("<p class=\"lane\">{}</p>", escape(lane)),
                None => String::new(),
            },
        ),
        None => (
            "<p class=\"focus none\">no focus</p>".to_string(),
            String::new(),
        ),
    };
    // Zero blocked fronts is said by not saying it. The count earns its
    // place on the row when it is the reason a project stopped; printed as
    // `0` on every row it would only be furniture.
    let blocked = if l.blocked == 0 {
        String::new()
    } else if l.blocked == 1 {
        " &middot; 1 blocked".to_string()
    } else {
        format!(" &middot; {} blocked", l.blocked)
    };
    let stop = match &l.stop {
        Some(s) => {
            let class = match s {
                LastStop::NeverByHand => "never",
                LastStop::Made { work_since, .. } if *work_since => "work",
                LastStop::Made { .. } => "still",
            };
            format!("<p class=\"stop {class}\">{}</p>", escape(&stop_line(s)))
        }
        None => String::new(),
    };
    format!(
        "<li class=\"project\"><p class=\"name\"><a href=\"/p/{href}/\">{name}</a></p>
         {focus}
         {lane}<div class=\"state\"><p class=\"quiet\">{quiet}{blocked}</p>
         {stop}</div></li>
",
        href = escape(&l.href),
        name = escape(&l.name),
        quiet = silence(l.quiet),
    )
}

/// The page shell both listings share. `note`, when there is one, sits right
/// after the list, inside `<main>` -- `index_page`'s way of saying a root the
/// registry named did not open (`d810`); `choose_page` never has one.
fn listing(title: &str, promise: &str, rows: String, note: Option<&str>, footer: &str) -> String {
    let note = match note {
        Some(n) => format!("<p class=\"note\">{}</p>\n", escape(n)),
        None => String::new(),
    };
    format!(
        "<!doctype html>
         <html lang=\"en\"><head><meta charset=\"utf-8\">
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
         <title>{t}</title>
         {FAVICON}
         <style>
{WEB_CSS}</style></head>
         <body><div class=\"page\">
         <header><h1>{t}</h1></header>
         <p class=\"promise\">{promise}</p>
         <main><ul class=\"projects\">
{rows}</ul>{note}</main>
         <footer>{footer}</footer>
         </div></body></html>
",
        t = escape(title),
    )
}

/// The index of projects (`d199`, `d200`).
///
/// Its promise, written before it was built and kept here where it can be
/// read against what the page does: **which project moved, and which one has
/// been sitting still, without going in to ask.** It passes the burden of
/// proof the same way the web did as a whole -- the CLI answers this already,
/// project by project, and still does not arrive, because the cost is not
/// reading but knowing where to look. Eight `cd` is what "being available is
/// not arriving" looks like.
///
/// Reached when the working directory is not inside a project (`d199`), and
/// now -- `d809` -- every other time too: `/` always answers this page.
/// It used to be a bare `<ul>` of links with no stylesheet, no title and no
/// viewport, which answered nothing a `cd` did not.
///
/// `unreachable` names every root the machine's registry could not open
/// (`d810`), fixed at startup: a note under the list when there is at least
/// one, in the same words `find --everywhere` prints for the same shape of
/// answer.
pub(super) fn index_page(
    projects: &mut [crate::project::Project],
    unreachable: &[String],
) -> String {
    let rows: String = projects.iter_mut().map(|p| project_row(&line(p))).collect();
    let note = (!unreachable.is_empty()).then(|| super::unreachable_sentence(unreachable));
    listing(
        "Projects",
        "Which project moved, and which one has been sitting still.",
        rows,
        note.as_deref(),
        "The same reading in a terminal, one project at a time:          <code>vivac open</code>",
    )
}

/// The answer to a name more than one project carries (`d374`).
///
/// It refuses rather than guesses, which is what `registry::resolve` already
/// does for `--project` on the CLI. What the web can do and the command
/// cannot is *say which is which without naming a path*: the CLI concluded
/// that two candidates sharing a name have no path-free way to tell apart,
/// and that is true of a one-line error and false of a page -- the focus and
/// the silence separate them, and neither is a path.
pub(super) fn choose_page(projects: &mut [crate::project::Project], which: &[usize]) -> String {
    let rows: String = which
        .iter()
        .map(|&i| project_row(&line(&mut projects[i])))
        .collect();
    listing(
        "Which one?",
        "More than one project goes by that name, so this page picks none of them.",
        rows,
        None,
        "The link under each name is permanent: it carries the project's own          id, so it keeps opening this tree even when another of the same name          joins.",
    )
}

/// `WEB.md` §5: embedded, because the CSP admits an inline `<style>` and no
/// external stylesheet at all, and in its own file because nobody maintains
/// two hundred lines of CSS inside a string literal.
use super::WEB_CSS;

/// One node as a row: its alias, its title, and at most one line under them.
///
/// Everything here goes through `escape`. A title is prose somebody wrote,
/// and a tree is allowed to hold a node called `<script>`.
fn row(project: &str, tree: &Tree, n: &Node, note: &str, note_class: &str) -> String {
    let mut s = format!(
        "<li><span class=\"alias\">{}</span><p class=\"title\">{}</p>",
        alias_link(project, &n.alias()),
        escape(n.title(tree))
    );
    if !note.is_empty() {
        let class = if note_class.is_empty() {
            "note".to_string()
        } else {
            format!("note {note_class}")
        };
        s.push_str(&format!("<p class=\"{class}\">{}</p>", escape(note)));
    }
    s.push_str("</li>\n");
    s
}

/// A named list, or nothing at all when there is nothing in it. An empty
/// heading is a line that says only that you have to read on.
fn group(title: &str, rows: String) -> String {
    if rows.is_empty() {
        return String::new();
    }
    format!("<h3>{title}</h3>\n<ul class=\"nodes\">\n{rows}</ul>\n")
}

/// Where the stretch is measured from, in a sentence. The facts come from
/// the same `Boundary` the CLI prints; only the wording is this page's.
fn since_line(since: &Boundary, stops: usize) -> String {
    match since {
        Boundary::Beginning {
            asked_for_manual: false,
        } => "Since the beginning: no stops yet.".to_string(),
        Boundary::Beginning {
            asked_for_manual: true,
        } => "Since the beginning: no stop here was made by hand.".to_string(),
        Boundary::Stop { vivac, .. } => {
            let date = crate::clock::date_of(&vivac.ts);
            let tail = match stops {
                0 => String::new(),
                1 => ", 1 stop since".to_string(),
                n => format!(", {n} stops since"),
            };
            format!(
                "Since {}, the last stop you made, {date}{tail}.",
                escape(&vivac.alias())
            )
        }
    }
}

/// The block this page exists for. It is rendered **whether or not anything
/// moved**: the promise is that you find out without having had to ask, and
/// a section that vanishes when the answer is "nothing" is that question put
/// straight back.
fn moved_section(project: &str, tree: &Tree, changed: &Changed) -> String {
    let mut body = String::new();
    body.push_str(&group(
        "Opened",
        changed
            .opened
            .iter()
            .map(|o| row(project, tree, o.node, "", ""))
            .collect(),
    ));
    body.push_str(&group(
        "Closed",
        changed
            .closed
            .iter()
            .map(|c| {
                let note = if c.forced && c.outcome.is_empty() {
                    "forced".to_string()
                } else if c.forced {
                    format!("forced: {}", c.outcome)
                } else {
                    c.outcome.clone()
                };
                row(
                    project,
                    tree,
                    c.node,
                    &note,
                    if c.forced { "forced" } else { "" },
                )
            })
            .collect(),
    ));
    body.push_str(&group(
        "Flagged",
        changed
            .flagged
            .iter()
            .map(|f| {
                let note = if f.reason.is_empty() {
                    f.flag.word().to_string()
                } else {
                    format!("{}: {}", f.flag.word(), f.reason)
                };
                row(project, tree, f.node, &note, "flag")
            })
            .collect(),
    ));
    body.push_str(&group(
        "Moved",
        changed
            .moved
            .iter()
            .map(|m| row(project, tree, m.node, m.state.word(m.node.kind), ""))
            .collect(),
    ));

    if let Some(tail) = changes::tail_phrase(&changed.tail) {
        body.push_str(&format!("<p class=\"note\">{}</p>\n", escape(&tail)));
    }
    if changed.nothing_moved() {
        body.push_str("<p class=\"empty\">Nothing has moved.</p>\n");
    }

    format!(
        "<section id=\"moved\">\n<h2>What moved</h2>\n<p class=\"since\">{}</p>\n{body}</section>\n",
        since_line(&changed.since, changed.tail.stops)
    )
}

/// The stack, top to bottom, with the focus marked by a word and not only by
/// a rule beside it: the DX pillar does not allow a meaning that only a
/// colour or a border carries.
///
/// With more than one lane, `tree.stack()` only ever reads `tree.lane()`'s
/// own -- the one this page happened to be resolved for, which the web asks
/// for far less often than it serves a project it was not started in
/// (`project::Registry::open`'s own doc). Showing that lane's stack as "the"
/// stack would say something false the moment it is not the one anybody
/// wrote to last, so once a second lane has something to name at all, this
/// shows the one that wrote most recently instead and says whose it is
/// (`t594` §5.6).
fn stack_section(project: &str, tree: &Tree) -> String {
    let lanes = crate::brief::lanes_with_a_stack(tree);
    let elsewhere = if lanes.len() > 1 {
        crate::brief::last_writer(tree)
    } else {
        None
    };
    let (raw_stack, here_mark): (&[u64], String) = match &elsewhere {
        Some(w) => (
            tree.lanes
                .get(w.id)
                .map(|s| s.stack.as_slice())
                .unwrap_or(&[]),
            format!("in lane {}, 1 of {} lanes", w.name, lanes.len()),
        ),
        None => (tree.stack(), "you are here".to_string()),
    };
    let stack: Vec<&Node> = raw_stack
        .iter()
        .filter_map(|&num| tree.node_by_num(num))
        .collect();
    if stack.is_empty() {
        return "<section id=\"focus\">\n<h2>Where you are</h2>\n\
                <p class=\"empty\">Empty stack.</p>\n</section>\n"
            .to_string();
    }
    let last = stack.len() - 1;
    let items: String = stack
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let here = if i == last {
                // `f807`: a leading space, or the mark reads glued to the
                // title beside it -- "...Fix the cache adapteryou are here".
                format!(" <span class=\"here-mark\">{}</span>", escape(&here_mark))
            } else {
                String::new()
            };
            format!(
                "<li{}><span class=\"alias\">{}</span><p class=\"title\">{}{here}</p></li>\n",
                if i == last { " class=\"here\"" } else { "" },
                alias_link(project, &n.alias()),
                escape(n.title(tree))
            )
        })
        .collect();
    format!(
        "<section id=\"focus\">\n<h2>Where you are</h2>\n<ol class=\"stack\">\n{items}</ol>\n</section>\n"
    )
}

/// What reaches this point: the standing decisions and the invariants, both
/// picked by the same functions the brief picks them with (`WEB.md` §2).
fn governs_section(project: &str, tree: &Tree) -> String {
    let focus = tree.focus();
    let lineage: Vec<&Node> = focus.map(|f| tree.ancestors(f.num)).unwrap_or_default();
    let on_lineage: std::collections::HashSet<u64> = lineage.iter().map(|n| n.num).collect();

    let decisions: String = match focus {
        Some(f) => crate::brief::standing(tree, f, &on_lineage)
            .iter()
            .map(|n| row(project, tree, n, "", ""))
            .collect(),
        // No focus, no path, so nothing reaches "this point" except what
        // governs the whole project -- which is what the invariants below
        // already carry. The brief answers the same way.
        None => String::new(),
    };
    let invariants: String = crate::brief::constraints(tree, &lineage)
        .iter()
        .map(|n| {
            row(
                project,
                tree,
                n,
                if n.flags.is_empty() { "" } else { "at risk" },
                "flag",
            )
        })
        .collect();

    let mut body = String::new();
    body.push_str(&group("Standing decisions", decisions));
    body.push_str(&group("Invariants", invariants));
    if body.is_empty() {
        body.push_str("<p class=\"empty\">Nothing governs this point yet.</p>\n");
    }
    format!("<section id=\"governs\">\n<h2>What governs this point</h2>\n{body}</section>\n")
}

/// The product's differentiator, and it only ever has content if parking
/// costs what popping costs.
fn parked_section(project: &str, tree: &Tree) -> String {
    let mut ps: Vec<&Node> = tree
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .collect();
    ps.sort_by_key(|n| n.num);
    let body = if ps.is_empty() {
        "<p class=\"empty\">Nothing parked.</p>\n".to_string()
    } else {
        format!(
            "<ul class=\"nodes\">\n{}</ul>\n",
            ps.iter()
                .map(|n| row(project, tree, n, n.outcome(tree), ""))
                .collect::<String>()
        )
    };
    format!("<section id=\"parked\">\n<h2>Do not touch now</h2>\n{body}</section>\n")
}

/// `WEB.md` §3.1 -- Today.
///
/// > You find out what moved while you were not looking **without having had
/// > to ask whether anything did**.
///
/// The four blocks come in the order that sentence implies rather than the
/// brief's: what moved is why this page exists, and a page whose reason for
/// existing sits below the fold does not keep it.
///
/// Every one of the four is answered by a function the CLI calls too. This
/// page picks no nodes of its own.
///
/// The crumb above the title is the way back to the index (`d809`): the
/// same form `why` and `tree` already carry, pointed at `/` instead of at
/// this project, since `/` is where every project is listed.
pub(super) fn today_page(project: &str, name: &str, tree: &Tree, log: &[Event]) -> String {
    let boundary = changes::manual_boundary(tree);
    let mut changed = changes::collect(tree, log, boundary.seq());
    changed.since = boundary;

    format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Today - {name_t}</title>\n\
         {FAVICON}\n\
         <style>\n{WEB_CSS}</style></head>\n\
         <body><div class=\"page\">\n\
         <p class=\"crumb\"><a href=\"/\">All projects</a></p>\n\
         <header><h1>{name_t}</h1></header>\n\
         <p class=\"promise\">What moved while you were not looking.</p>\n\
         <p class=\"onward\"><a href=\"/p/{p}/tree\">The whole tree, as a map</a></p>\n\
         <main>\n{moved}{focus}{governs}{parked}</main>\n\
         <footer>The same reading in a terminal: \
         <code>vivac changes --since manual</code></footer>\n\
         </div></body></html>\n",
        name_t = escape(name),
        p = escape(project),
        moved = moved_section(project, tree, &changed),
        focus = stack_section(project, tree),
        governs = governs_section(project, tree),
        parked = parked_section(project, tree),
    )
}

#[cfg(test)]
mod tests {
    use super::{last_stop, silence, stop_line, today_page, LastStop};
    use crate::event::{Body, Event, Kind, VivacKind};
    use crate::model::fold;

    /// Four events are enough for a page: a goal to stand on, a stop to
    /// measure from, and a node born after it.
    fn ev(seq: u64, payload: Body) -> Event {
        Event {
            seq,
            id: format!("e{seq}"),
            ts: "2026-09-03T10:00:00Z".to_string(),
            actor: "a".to_string(),
            lane: "main".to_string(),
            payload,
        }
    }

    fn born(seq: u64, num: u64, title: &str, parent: Option<&str>) -> Event {
        ev(
            seq,
            Body::NodeCreated {
                node: format!("n{num}"),
                num,
                kind: Kind::Goal,
                title: title.to_string(),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
                against: None,
            },
        )
    }

    fn stop(seq: u64, num: u64, kind: VivacKind) -> Event {
        ev(
            seq,
            Body::VivacCreated {
                vivac: format!("v{num}"),
                num,
                kind,
                stack: vec![],
                working_set: vec![],
                next_intent: String::new(),
                anchor: crate::anchor::AnchorRef::default(),
                anchors: vec![],
                node_ref: None,
                label: String::new(),
            },
        )
    }

    /// The one rule the whole page rests on: a title is prose somebody
    /// wrote, and a tree is allowed to hold a node called `<script>`.
    #[test]
    fn a_title_that_looks_like_markup_reaches_the_page_escaped() {
        let events = vec![born(1, 1, "<script>alert(1)</script>", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(!page.contains("<script>alert(1)"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
    }

    /// The order is the page's argument. What moved is why this page exists,
    /// so it comes before the three blocks that say where you are.
    #[test]
    fn what_moved_comes_before_the_rest() {
        let events = vec![born(1, 1, "A goal", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        let at = |id: &str| page.find(id).unwrap_or_else(|| panic!("no {id}:\n{page}"));
        assert!(at("id=\"moved\"") < at("id=\"focus\""), "{page}");
        assert!(at("id=\"focus\"") < at("id=\"governs\""), "{page}");
        assert!(at("id=\"governs\"") < at("id=\"parked\""), "{page}");
    }

    /// The promise is that you find out **without having had to ask whether
    /// anything did**. A block that disappears when the answer is "nothing"
    /// is that question put straight back, so it is rendered either way.
    #[test]
    fn the_moved_block_is_there_even_when_nothing_moved() {
        let events = vec![born(1, 1, "A goal", None), stop(2, 1, VivacKind::Manual)];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(page.contains("id=\"moved\""), "{page}");
        assert!(page.contains("the last stop you made"), "{page}");
        assert!(page.contains("Nothing has moved."), "{page}");
    }

    /// The boundary is the last stop **made by hand**, the same one
    /// `changes --since manual` measures from. A stop the hook wrote sits
    /// inside the stretch and does not end it.
    #[test]
    fn the_boundary_is_the_last_stop_made_by_hand() {
        let events = vec![
            born(1, 1, "A goal", None),
            stop(2, 1, VivacKind::Manual),
            born(3, 2, "Born after the stop", Some("n1")),
            stop(4, 2, VivacKind::Auto),
        ];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(page.contains("Born after the stop"), "{page}");
        assert!(page.contains("1 stop since"), "{page}");
    }

    /// DX pillar: a meaning never rides on a colour or a rule alone. Every
    /// state the page shows is also a word on the page.
    #[test]
    fn a_state_is_carried_by_a_word_and_not_only_by_a_class() {
        let events = vec![
            born(1, 1, "A goal", None),
            born(2, 2, "Parked work", Some("n1")),
            ev(
                3,
                Body::StateChanged {
                    node: "n2".to_string(),
                    state: crate::event::State::Suspended,
                    outcome: "waiting on day 14".to_string(),
                    forced: false,
                },
            ),
        ];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(page.contains("Do not touch now"), "{page}");
        assert!(page.contains("waiting on day 14"), "{page}");
        assert!(page.contains("parked"), "{page}");
    }

    /// `d809`: the page a project's own `id` routes to still carries a way
    /// back to the index, in the same crumb `why` and `map` already use.
    #[test]
    fn the_header_carries_a_crumb_back_to_the_index() {
        let events = vec![born(1, 1, "A goal", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(
            page.contains("<p class=\"crumb\"><a href=\"/\">All projects</a></p>"),
            "{page}"
        );
    }

    /// `f807`: `why.rs`'s `step()` is not the only place `.here-mark` is
    /// emitted -- this page's own stack section glued it to the title the
    /// same way. A stack needs a push to show anything at all (`f156`'s
    /// stack, not `stop`'s -- a stop's own `stack` field is `working_set`'s
    /// counterpart, not what `tree.stack()` reads).
    #[test]
    fn the_here_mark_on_the_stack_is_set_off_from_the_title_by_a_space() {
        let events = vec![
            born(1, 1, "A goal", None),
            ev(
                2,
                Body::Pushed {
                    node: "n1".to_string(),
                },
            ),
        ];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(
            page.contains(" <span class=\"here-mark\">"),
            "the mark is glued to the title:\n{page}"
        );
    }

    /// `WEB.md` §7.4: the page loads with no internet at all. Nothing here
    /// may reach past `127.0.0.1`, and the cheapest proof is that no absolute
    /// url is written into it in the first place.
    #[test]
    fn the_page_asks_for_nothing_off_this_machine() {
        let events = vec![born(1, 1, "A goal", None)];
        let tree = fold(&events, 0);
        let page = today_page("demo", "demo", &tree, &events);
        assert!(!page.contains("http://"), "{page}");
        assert!(!page.contains("https://"), "{page}");
        assert!(!page.contains("//fonts."), "{page}");
    }

    /// A project with nothing in its log yet has no last stop to report.
    #[test]
    fn last_stop_on_an_empty_log_is_none() {
        let events: Vec<Event> = vec![];
        let tree = fold(&events, 0);
        assert!(last_stop(&tree, &events).is_none());
    }

    /// A log where every stop was written by the hook has never been
    /// stopped by hand.
    #[test]
    fn a_log_with_only_hook_stops_is_never_by_hand() {
        let events = vec![born(1, 1, "A goal", None), stop(2, 1, VivacKind::Auto)];
        let tree = fold(&events, 0);
        assert!(matches!(
            last_stop(&tree, &events),
            Some(LastStop::NeverByHand)
        ));
    }

    /// A node born after the stop made by hand is work since it.
    #[test]
    fn a_node_born_after_the_manual_stop_reads_work_since() {
        let events = vec![
            born(1, 1, "A goal", None),
            stop(2, 1, VivacKind::Manual),
            born(3, 2, "Born after the stop", Some("n1")),
        ];
        let tree = fold(&events, 0);
        match last_stop(&tree, &events) {
            Some(LastStop::Made { work_since, .. }) => assert!(work_since),
            _ => panic!("expected Made with work since"),
        }
    }

    /// A stop made by hand with nothing after it reads nothing since.
    #[test]
    fn the_manual_stop_as_the_last_event_reads_nothing_since() {
        let events = vec![born(1, 1, "A goal", None), stop(2, 1, VivacKind::Manual)];
        let tree = fold(&events, 0);
        match last_stop(&tree, &events) {
            Some(LastStop::Made { work_since, .. }) => assert!(!work_since),
            _ => panic!("expected Made with nothing since"),
        }
    }

    /// A stop the hook wrote after the one made by hand is still not work:
    /// a stop is not a change to the tree.
    #[test]
    fn a_hook_stop_after_the_manual_one_is_still_nothing_since() {
        let events = vec![
            born(1, 1, "A goal", None),
            stop(2, 1, VivacKind::Manual),
            stop(3, 2, VivacKind::Auto),
        ];
        let tree = fold(&events, 0);
        match last_stop(&tree, &events) {
            Some(LastStop::Made { work_since, .. }) => assert!(!work_since),
            _ => panic!("expected Made with nothing since"),
        }
    }

    /// The three sentences `stop_line` can read, and the fallback to the
    /// bare date when `days` does not parse.
    #[test]
    fn stop_line_reads_the_three_sentences_and_the_date_fallback() {
        assert_eq!(stop_line(&LastStop::NeverByHand), "no stop made by hand");
        assert_eq!(
            stop_line(&LastStop::Made {
                days: Some(0),
                date: "2026-09-03".to_string(),
                work_since: true,
            }),
            "last stop you made: today, work since"
        );
        assert_eq!(
            stop_line(&LastStop::Made {
                days: Some(3),
                date: "2026-09-03".to_string(),
                work_since: false,
            }),
            "last stop you made: 3 days ago, nothing since"
        );
        assert_eq!(
            stop_line(&LastStop::Made {
                days: None,
                date: "2026-09-03".to_string(),
                work_since: true,
            }),
            "last stop you made: 2026-09-03, work since"
        );
    }

    /// `silence` is `ago` with "moved" in front, and its four outputs must
    /// not change now that it is built on top of it.
    #[test]
    fn silence_keeps_its_four_outputs() {
        assert_eq!(silence(None), "never written to");
        assert_eq!(silence(Some(0)), "moved today");
        assert_eq!(silence(Some(1)), "moved yesterday");
        assert_eq!(silence(Some(5)), "moved 5 days ago");
    }
}
