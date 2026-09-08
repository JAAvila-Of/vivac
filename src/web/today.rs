//! `WEB.md` §3.1 -- the Today page, and the index of projects it is reached
//! through.
//!
//! `today_page` is the surface `mod.rs` docs as "the real page": everything
//! below builds it, section by section, out of the same functions the CLI
//! reads the tree with. This module knows nothing about a socket; `mod.rs`
//! is what turns its output into a response.

use super::{alias_link, escape};
use crate::changes::{self, Boundary, Changed};
use crate::event::Kind;
use crate::event::{Event, State};
use crate::model::{Node, Tree};

/// One project as the index sees it: the four fields `d200` admitted, and
/// the link to reach it by.
///
/// Four and no more, and each one was argued against the promise: the name
/// so you know which it is, the focus so you know what it was doing, the
/// blocked count because a blocked front is the commonest reason a project
/// stops without anyone noticing, and the silence because that is the whole
/// question. `d200` says a counter that cannot answer the promise does not
/// get in, so nothing else does.
struct Line {
    href: String,
    name: String,
    focus: Option<String>,
    blocked: usize,
    quiet: Option<i64>,
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
        };
    };
    let href = log.first().map(|e| e.id.clone()).unwrap_or(href);
    let tree = &ctx.tree;
    let focus = tree
        .focus()
        .map(|n| format!("{}  {}", n.alias(), n.title(tree)));
    let blocked = tree
        .nodes_iter()
        .filter(|n| n.kind == Kind::Question && n.state.is_open() && n.blocks)
        .count();
    let quiet = log
        .last()
        .and_then(|e| crate::clock::days_between(&e.ts, &crate::clock::now_rfc3339()));
    Line {
        href,
        name,
        focus,
        blocked,
        quiet,
    }
}

/// How long the silence reads. Days and not hours: the promise is about a
/// project that has been still for *days*, and an hour count would invite
/// reading this page for movement it is not measuring.
fn silence(days: Option<i64>) -> String {
    match days {
        None => "never written to".to_string(),
        Some(d) if d <= 0 => "moved today".to_string(),
        Some(1) => "moved yesterday".to_string(),
        Some(d) => format!("moved {d} days ago"),
    }
}

/// One row, shared by the index and by the page that asks which of two
/// projects you meant. **No path ever appears here.** The security pillar
/// allows a project's name across this boundary and nothing else, and a real
/// path carries the name of whoever owns the machine.
fn project_row(l: &Line) -> String {
    let focus = match &l.focus {
        Some(f) => format!("<p class=\"title\">{}</p>", escape(f)),
        None => "<p class=\"note\">no focus</p>".to_string(),
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
    format!(
        "<li><span class=\"alias\"><a href=\"/p/{href}/\">{name}</a></span>
         {focus}<p class=\"note\">{quiet}{blocked}</p></li>
",
        href = escape(&l.href),
        name = escape(&l.name),
        quiet = silence(l.quiet),
    )
}

/// The page shell both listings share.
fn listing(title: &str, promise: &str, rows: String, footer: &str) -> String {
    format!(
        "<!doctype html>
         <html lang=\"en\"><head><meta charset=\"utf-8\">
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
         <title>{t}</title>
         <style>
{WEB_CSS}</style></head>
         <body><div class=\"page\">
         <header><h1>{t}</h1>
         <p class=\"promise\">{promise}</p></header>
         <main><ul class=\"nodes\">
{rows}</ul></main>
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
/// Reached when the working directory is not inside a project (`d199`). It
/// used to be a bare `<ul>` of links with no stylesheet, no title and no
/// viewport, which answered nothing a `cd` did not.
pub(super) fn index_page(projects: &mut [crate::project::Project]) -> String {
    let rows: String = projects.iter_mut().map(|p| project_row(&line(p))).collect();
    listing(
        "Projects",
        "Which project moved, and which one has been sitting still.",
        rows,
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
            .map(|n| row(project, tree, n, "", ""))
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
    if body.is_empty() {
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
fn stack_section(project: &str, tree: &Tree) -> String {
    let stack: Vec<&Node> = tree
        .stack
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
                "<span class=\"here-mark\">you are here</span>"
            } else {
                ""
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
pub(super) fn today_page(project: &str, name: &str, tree: &Tree, log: &[Event]) -> String {
    let boundary = changes::manual_boundary(tree);
    let mut changed = changes::collect(tree, log, boundary.seq());
    changed.since = boundary;

    format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Today - {name_t}</title>\n\
         <style>\n{WEB_CSS}</style></head>\n\
         <body><div class=\"page\">\n\
         <header><h1>{name_t}</h1>\n\
         <p class=\"promise\">What moved while you were not looking.</p>\n\
         <p class=\"onward\"><a href=\"/p/{p}/tree\">The whole tree, drawn</a></p></header>\n\
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
    use super::today_page;
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
}
