//! `WEB.md` §3.2 -- the lineage, drawn.
//!
//! Its promise (`d144`): you see the shape of the path **at a glance**
//! --how deep, where it branched, what was still open at each step-- where
//! the text hands it to you one node at a time and in order.
//!
//! `d187` decided the form on a measurement, not on taste. A real lineage
//! carries around twenty-two standing decisions and thirty siblings spread
//! over six or seven steps: drawing all of it is fifty rows, which is
//! `why --full` with a stylesheet, and that is precisely what the promise
//! says the text already does. So the counts carry the silhouette and the
//! detail stays one gesture away, in `<details>` -- plain HTML, no build
//! step (§5), nothing off this machine (§7.4), and `curl` still returns the
//! content inside the page.
//!
//! The page picks no nodes of its own: every one of the three things it
//! draws per step comes from the same function `why --full` calls
//! (`WEB.md` §2).

use super::{alias_link, escape};
use crate::event::Event;
use crate::model::{Aggregates, Node, Tree};
use crate::render::{anchor_of, open_then_of, standing_of, Full};

/// One node as a line inside a `<details>`: the alias links to its own
/// lineage, so the drawing is also the way you walk the tree.
fn link(project: &str, n: &Node) -> String {
    format!(
        "<li><span class=\"alias\"><a href=\"/p/{p}/why/{a}\">{a}</a></span>\
         <p class=\"title\">{t}</p></li>\n",
        p = escape(project),
        a = escape(&n.alias()),
        t = escape(&n.title)
    )
}

/// The two lists that hang off a step, and the summary that stands in for
/// them when it is closed.
///
/// A step with nothing to expand gets no `<details>` at all: the absence is
/// the answer --nothing was decided here, nothing was left open here-- and
/// a disclosure triangle that opens onto nothing teaches that the shape
/// cannot be trusted.
fn weight(project: &str, standing: &[&Node], open_then: &[&Node]) -> String {
    if standing.is_empty() && open_then.is_empty() {
        return String::new();
    }
    let counts = [
        (standing.len(), "governing here"),
        (open_then.len(), "open then"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("<span class=\"count\">{n} {w}</span>"))
    .collect::<Vec<_>>()
    .join(" ");

    let mut body = String::new();
    if !standing.is_empty() {
        body.push_str("<h3>Decided here, still standing</h3>\n<ul class=\"nodes\">\n");
        for n in standing {
            body.push_str(&link(project, n));
        }
        body.push_str("</ul>\n");
    }
    if !open_then.is_empty() {
        body.push_str("<h3>Still open at that moment</h3>\n<ul class=\"nodes\">\n");
        for n in open_then {
            body.push_str(&link(project, n));
        }
        body.push_str("</ul>\n");
    }
    format!(
        "<details><summary>{counts}</summary>\n<div class=\"detail\">\n{body}</div>\n</details>\n"
    )
}

/// The facts that are always visible: what this step is, what state it is
/// in, when it opened, the anchor if there is one, and how much is still
/// open underneath.
///
/// The state is a word before it is anything else. The DX pillar does not
/// allow a meaning that only a colour or a border carries, so `superseded`
/// and `parked` say so in text on a step that is neither of them by shape.
///
/// The anchor is drawn only when there is one (`f186`). Measured against
/// all three real trees on 4-Sep-2026, every stop carries an empty one,
/// because none of the three directories is under version control; a row
/// reading `anchor: --` on every step of every page would say nothing about
/// the path, only about how the directory is set up.
fn facts(tree: &Tree, ag: &Aggregates, full: &Full, n: &Node) -> String {
    let mut parts = vec![
        format!("<span class=\"word\">{}</span>", n.kind.word()),
        format!("<span class=\"word\">{}</span>", n.state.word(n.kind)),
        format!("<span class=\"when\">opened {}</span>", escape(&n.opened)),
    ];
    let anchor = anchor_of(tree, full, n);
    if !anchor.is_empty_tree() {
        parts.push(format!(
            "<span class=\"anchor\">{} {}</span>",
            escape(&anchor.kind),
            escape(anchor.short())
        ));
    }
    let open_below = ag.counts(&n.id).open_count;
    if open_below > 0 {
        parts.push(format!(
            "<span class=\"count\">{open_below} open below</span>"
        ));
    }
    format!("<p class=\"facts\">{}</p>\n", parts.join(" "))
}

/// One step of the spine.
///
/// Every step but the one you are on links to its own lineage, so the
/// drawing is also how you walk up the path. The step you are on does not:
/// a link back to the page you are reading teaches nothing.
fn step(project: &str, tree: &Tree, ag: &Aggregates, full: &Full, n: &Node, here: bool) -> String {
    let mark = if here {
        "<span class=\"here-mark\">you are here</span>"
    } else {
        ""
    };
    let alias = if here {
        escape(&n.alias())
    } else {
        alias_link(project, &n.alias())
    };
    format!(
        "<li{cls}>\n<span class=\"alias\">{alias}</span>\n<div class=\"what\">\n\
         <p class=\"title\">{title}{mark}</p>\n{facts}{weight}</div>\n</li>\n",
        cls = if here { " class=\"here\"" } else { "" },
        title = escape(&n.title),
        facts = facts(tree, ag, full, n),
        weight = weight(project, &standing_of(tree, n), &open_then_of(tree, full, n)),
    )
}

/// The lineage of `id`, drawn. `None` when nothing resolves, which the
/// caller turns into a `404`: a page that renders an empty spine for a
/// typo teaches that the node exists.
pub(super) fn why_page(
    project: &str,
    name: &str,
    tree: &Tree,
    log: &[Event],
    id: &str,
) -> Option<String> {
    let target = tree.resolve(id)?;
    let full = Full::from_log(log);
    // Once for the page, not once per step: `aggregates` walks the whole
    // tree, and a seven-step spine would otherwise walk it seven times for
    // an answer that does not change between them.
    let ag = tree.aggregates();
    let path = tree.ancestors(&target.id);
    let last = path.len().saturating_sub(1);
    let spine: String = path
        .iter()
        .enumerate()
        .map(|(i, n)| step(project, tree, &ag, &full, n, i == last))
        .collect();

    Some(format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Why {alias} - {name_t}</title>\n\
         <style>\n{css}</style></head>\n\
         <body><div class=\"page\">\n\
         <header><p class=\"crumb\"><a href=\"/p/{p}/\">{name_t}</a></p>\n\
         <h1>Why we are here</h1>\n\
         <p class=\"promise\">The shape of the path at a glance: how deep it goes, \
         where it branched, and what was still open at each step.</p></header>\n\
         <main>\n<ol class=\"spine\">\n{spine}</ol>\n</main>\n\
         <footer>The same reading in a terminal: \
         <code>vivac why {alias} --full</code></footer>\n\
         </div></body></html>\n",
        alias = escape(&target.alias()),
        name_t = escape(name),
        p = escape(project),
        css = super::WEB_CSS,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Body, Event, Kind, State};
    use crate::model::fold;

    /// A lineage needs a path to draw, so these build one: a goal, a
    /// decision under it, and a finding under that.
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

    fn born(seq: u64, num: u64, kind: Kind, title: &str, parent: Option<&str>) -> Event {
        ev(
            seq,
            Body::NodeCreated {
                node: format!("n{num}"),
                num,
                kind,
                title: title.to_string(),
                why: "it is needed".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
            },
        )
    }

    fn closed(seq: u64, num: u64) -> Event {
        ev(
            seq,
            Body::StateChanged {
                node: format!("n{num}"),
                state: State::Done,
                outcome: String::new(),
                forced: false,
            },
        )
    }

    /// goal 1 -> decision 2 -> finding 3, with a sibling of 3 that was
    /// still open when 3 was born and closed afterwards.
    fn lineage() -> Vec<Event> {
        vec![
            born(1, 1, Kind::Goal, "ship it", None),
            born(2, 2, Kind::Decision, "the face is web", Some("n1")),
            born(3, 3, Kind::Task, "the sibling", Some("n2")),
            born(4, 4, Kind::Finding, "the anchor is empty", Some("n2")),
            closed(5, 3),
        ]
    }

    /// The promise of this page is the shape of the path, so the spine is
    /// the path: every node from the root down, and no others.
    #[test]
    fn the_spine_has_one_step_per_node_of_the_path() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        // Counted on the one element a step always has and a listed node
        // never does. Counting `<li>` would count the nodes inside an open
        // `<details>` too.
        assert_eq!(
            page.matches("class=\"facts\"").count(),
            3,
            "one step per node of g1 > d2 > f4"
        );
        assert!(page.contains("ship it"));
        assert!(page.contains("the face is web"));
        assert!(page.contains("the anchor is empty"));
    }

    #[test]
    fn the_last_step_is_the_node_that_was_asked_for() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        let here = page.find("you are here").expect("the step is marked");
        let target = page
            .find("the anchor is empty")
            .expect("the target is drawn");
        let middle = page
            .find("the face is web")
            .expect("the middle step is drawn");
        assert!(middle < target, "the spine runs from the root down");
        assert!(target < here, "the mark sits on the step it belongs to");
    }

    /// The DX pillar: a meaning is never carried by a colour or a class
    /// alone. Strip every attribute and the page still says where you are.
    #[test]
    fn the_step_you_are_on_is_marked_by_a_word_and_not_only_by_a_class() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        assert!(page.contains("you are here"));
        assert!(page.contains("class=\"here\""));
    }

    /// The same rule the whole surface rests on: a title is prose somebody
    /// wrote, and a tree is allowed to hold a node called `<script>`.
    #[test]
    fn a_title_that_looks_like_markup_reaches_the_lineage_escaped() {
        let events = vec![born(1, 1, Kind::Goal, "<script>alert(1)</script>", None)];
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "g1").unwrap();
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("&lt;script&gt;"));
    }

    /// `f186`: every stop in all three real trees carries an empty anchor,
    /// because none of the three directories is under version control. A
    /// row saying so on every step of every page describes the directory,
    /// not the path.
    #[test]
    fn a_step_with_no_anchor_draws_no_anchor_row() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        assert!(!page.contains("class=\"anchor\""));
    }

    /// `d187`: a disclosure that opens onto nothing teaches that the shape
    /// cannot be trusted, so a step with nothing to expand has none.
    #[test]
    fn a_step_with_nothing_to_expand_has_no_disclosure() {
        let events = vec![born(1, 1, Kind::Goal, "ship it", None)];
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "g1").unwrap();
        assert!(!page.contains("<details>"));
    }

    /// `d173` renounced the list of what closed below and kept the counts.
    /// The page keeps that bargain: a number, never the nodes.
    ///
    /// The node buried here is under the target, so it is none of the three
    /// things a step does draw -- not a standing decision born on the path,
    /// not a sibling that was open at the time. If it reaches the page at
    /// all, the renounced list came back in.
    #[test]
    fn what_closed_below_a_step_is_a_count_and_never_a_list() {
        let mut events = lineage();
        events.push(born(6, 5, Kind::Task, "the buried one", Some("n4")));
        events.push(closed(7, 5));
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        assert!(page.contains("open below"), "the counts are drawn");
        assert!(
            !page.contains("the buried one"),
            "what closed below is counted, not listed"
        );
    }

    /// `d147` kept the tree only as the way you reach a lineage, so the
    /// lineage has to be a way of walking too: every step above you links
    /// to its own. The step you are on does not -- a link back to the page
    /// you are reading teaches nothing.
    #[test]
    fn every_step_but_the_one_you_are_on_links_to_its_own_lineage() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        assert!(page.contains("href=\"/p/vivac/why/g1\""), "{page}");
        assert!(page.contains("href=\"/p/vivac/why/d2\""), "{page}");
        assert!(!page.contains("href=\"/p/vivac/why/f4\""), "{page}");
    }

    /// A typo must not render an empty spine: that would teach that the
    /// node exists.
    #[test]
    fn an_alias_that_resolves_to_nothing_gives_no_page() {
        let events = lineage();
        let tree = fold(&events, 0);
        assert!(why_page("vivac", "vivac", &tree, &events, "f999").is_none());
    }

    /// `WEB.md` §7.4: the page loads with no internet and makes not one
    /// request off `127.0.0.1`.
    #[test]
    fn the_lineage_asks_for_nothing_off_this_machine() {
        let events = lineage();
        let tree = fold(&events, 0);
        let page = why_page("vivac", "vivac", &tree, &events, "f4").unwrap();
        assert!(!page.contains("http://"));
        assert!(!page.contains("https://"));
        assert!(!page.contains("//cdn"));
    }
}
