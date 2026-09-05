//! `WEB.md` §3.6 -- the global graph: the whole tree, drawn so its aggregate
//! *shape* is visible at a glance.
//!
//! Its promise (`t191`, written before anything was built): you see **at a
//! glance** how branched the work really is, which the same lines
//! `vivac tree` prints hold without showing.
//!
//! `d194` decided the form on the measurement the promise rests on: 193
//! nodes, max depth 6, 146 leaves (76%), one node with 56 direct children,
//! another with 30, and only seven with five or more. **Spine and comb**: a
//! row for every node with at least one child, nested by depth the way the
//! spine of §3.2 nests a path, and hanging off each row a dense band -- the
//! comb -- with one tile per direct child, in tree order. The width of the
//! band *is* the fan-out, so nothing here counts it in a number, ranks the
//! biggest hubs, or captions the tree as lopsided: the drawing carries that
//! or it fails §7.7.
//!
//! A leaf is therefore only ever a tile, in its parent's comb. A node with
//! children is both: a tile where its parent lists it, and a row of its
//! own. That is intended, not a duplicate.
//!
//! `d196`: each row is a `<details open>`/`<summary>`, the idiom `d187`
//! already fixed for the lineage. Open by default, because the promise is
//! the shape at landing, not after a gesture; and the one place a bare
//! fan-out number is allowed to exist, in the `<summary>`, hidden by CSS
//! for as long as the comb it counts is visible and shown the moment the
//! reader closes it.
//!
//! Plain HTML, the same as `why.rs`: no SVG, no JS, nothing off this
//! machine. Every tile links to its own lineage (§3.2), which is why this
//! surface was built after that one.
//!
//! The page picks no nodes of its own: it reads `Tree::roots` and
//! `Tree::children`, the same structures `vivac tree --all` reads
//! (`WEB.md` §2).

use super::{alias_link, escape};
use crate::event::State;
use crate::model::{Node, Tree};

/// The class a tile carries. State is never only a class -- `tile` also
/// spells it into `title` -- but the DX pillar asks for a second, silent cue
/// too: closed strikes through and parked dashes and italicises.
fn tile_class(n: &Node) -> &'static str {
    if n.state == State::Suspended {
        "tile parked"
    } else if n.state.is_open() {
        "tile"
    } else {
        "tile closed"
    }
}

/// One child, as a tile in its parent's comb. The click is the one every
/// alias on this product carries: its own lineage (`d147`).
///
/// The tile's own text is only its alias, and a page of 192 of them has no
/// other way to say what a node is without following the link. `title`
/// carries the rest -- alias, state, title -- in the separator `brief.rs`
/// already uses, so hovering or reading the raw HTML answers "what is
/// this" without a click.
fn tile(project: &str, n: &Node) -> String {
    format!(
        "<a class=\"{cls}\" href=\"/p/{p}/why/{a}\" title=\"{a} · {w} · {t}\">{a}</a>\n",
        cls = tile_class(n),
        p = escape(project),
        a = escape(&n.alias()),
        w = escape(n.state.word(n.kind)),
        t = escape(&n.title),
    )
}

/// The band hanging off one row: one tile per direct child, in tree order.
/// Its width is the fan-out -- that is the whole of `d194`'s argument, so
/// this draws it and never counts it.
fn comb(project: &str, children: &[&Node]) -> String {
    let tiles: String = children.iter().map(|c| tile(project, c)).collect();
    format!("<div class=\"comb\">\n{tiles}</div>\n")
}

/// One row: the node's own alias and title inside a `<summary>`, and --
/// once opened -- its comb and, nested one level deeper, a row for each of
/// its children that itself has children. `d196`.
///
/// **Open by default.** `t191`'s promise is the shape *at landing*, and a
/// page that opens onto 48 triangles and no comb delivers nothing: closing
/// a block is a prune the reader makes once already looking, not the state
/// the page arrives in.
///
/// **The fan-out is a number, but only once the comb it counts is
/// hidden.** `d194` forbids counting it while the band is visible -- the
/// width already says it -- and `d187` counts exactly what a closed
/// disclosure hides. Both hold at once because the number always sits in
/// the markup and only ever disappears in CSS, on `details[open]`.
///
/// The alias inside `<summary>` is a real link and still navigates: a
/// nested interactive element is its own click target under the HTML
/// activation model, the same reason a `<button>` inside a `<label>` does
/// not also fire the label. Clicking anywhere else in the summary -- the
/// title, the count, the triangle -- toggles as normal.
fn row(project: &str, tree: &Tree, n: &Node, children: &[&Node]) -> String {
    let comb_html = comb(project, children);
    let nested_html = rows(project, tree, children);
    let alias = alias_link(project, &n.alias());
    let title = escape(&n.title);

    // The same rule `why.rs` applies to its own disclosure: a triangle
    // that opens onto nothing teaches that the shape cannot be trusted.
    // `rows` never calls this with an empty `children`, so the comb below
    // is never empty either -- this is what keeps that true if it changes.
    if comb_html.is_empty() && nested_html.is_empty() {
        return format!(
            "<li class=\"row\"><span class=\"alias\">{alias}</span><p class=\"title\">{title}</p></li>\n"
        );
    }

    let count = match children.len() {
        1 => "1 child".to_string(),
        n => format!("{n} children"),
    };
    format!(
        "<li class=\"row\">\n<details open>\n<summary><span class=\"alias\">{alias}</span>\
         <p class=\"title\">{title}</p><span class=\"count\">{count}</span></summary>\n\
         {comb_html}{nested_html}</details>\n</li>\n"
    )
}

/// A row for every one of `siblings` that has at least one child, in tree
/// order, nested inside its own list so depth reads as indentation. A
/// sibling with no children draws no row here: it already has its tile, in
/// the comb above.
fn rows(project: &str, tree: &Tree, siblings: &[&Node]) -> String {
    let body: String = siblings
        .iter()
        .filter_map(|n| {
            let children = tree.children(&n.id);
            if children.is_empty() {
                None
            } else {
                Some(row(project, tree, n, &children))
            }
        })
        .collect();
    if body.is_empty() {
        String::new()
    } else {
        format!("<ol class=\"rows\">\n{body}</ol>\n")
    }
}

/// The whole tree of `project`, drawn. Never `None`: an empty tree still
/// gets a page, it just has nothing to draw.
pub(super) fn tree_page(project: &str, name: &str, tree: &Tree) -> String {
    let total = tree.total();
    let body = if tree.is_empty_tree() {
        "<p class=\"empty\">Empty tree.</p>\n".to_string()
    } else {
        rows(project, tree, &tree.roots())
    };
    format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Tree - {name_t}</title>\n\
         <style>\n{css}</style></head>\n\
         <body><div class=\"page\">\n\
         <header><p class=\"crumb\"><a href=\"/p/{p}/\">{name_t}</a></p>\n\
         <h1>The whole tree</h1>\n\
         <p class=\"promise\">How branched the work really is -- what the {total} \
         lines of <code>vivac tree</code> hold but do not show.</p></header>\n\
         <main>\n{body}</main>\n\
         <footer>The same reading in a terminal: \
         <code>vivac tree --all</code></footer>\n\
         </div></body></html>\n",
        name_t = escape(name),
        p = escape(project),
        css = super::WEB_CSS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Body, Kind};

    /// The degree of every node of the real tree that has at least one
    /// child, highest first -- what `vivac tree --all` measured on
    /// 4-Sep-2026: 195 nodes, 3 roots, 147 leaves, max depth 6, and these
    /// 48 with children. Eleven are five or more, twelve are two, and
    /// twenty-five are a single child.
    ///
    /// **Dated on purpose, and not a mirror of the live tree.** This is the
    /// shape the tree had at one moment, kept as a fixture; it does not
    /// follow the tree `vivac-project/.vivac` holds today. If it did, the
    /// §7.7 harness below would change on its own every time somebody
    /// writes a node, and a judge that moves judges nothing. `WEB.md` §3.6
    /// cites 193 because it was measured a few hours before this count --
    /// two nodes were written in between, and that gap is not worth
    /// chasing.
    const REAL_DEGREES: &[usize] = &[
        56, 30, 12, 11, 8, 7, 6, 4, 3, 3, 3, //
        2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, //
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, //
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, //
        1, 1, 1, 1, 1, //
    ];

    /// One node, applied straight to `tree` rather than folded from a log
    /// of `Event`s: `Tree::apply` is the same function the fold uses, and
    /// building 195 of them through a full `Event` (with an `id`, an
    /// `actor`, a `lane`) adds nothing a fixture needs to see.
    fn fixture_node(tree: &mut Tree, seq: &mut u64, num: &mut u64, parent: Option<&str>) -> String {
        *seq += 1;
        *num += 1;
        let id = format!("n{num}");
        tree.apply(
            *seq,
            "2026-09-04T10:00:00Z",
            &Body::NodeCreated {
                node: id.clone(),
                num: *num,
                kind: Kind::Task,
                title: format!("node {num}"),
                why: "fixture".to_string(),
                parent: parent.map(str::to_string),
                blocks: false,
                refs: vec![],
                governs: vec![],
            },
        );
        id
    }

    /// `count` children of `parent`, in the order they are born.
    fn fixture_children(
        tree: &mut Tree,
        seq: &mut u64,
        num: &mut u64,
        parent: &str,
        count: usize,
    ) -> Vec<String> {
        (0..count)
            .map(|_| fixture_node(tree, seq, num, Some(parent)))
            .collect()
    }

    /// A tree the size and shape `WEB.md` §7.7 asks for, built from
    /// `REAL_DEGREES` rather than by hand: two chains carry the measured
    /// depths exactly --56 at depth 0 down to 30 at depth 4 on one root,
    /// 4 down to 8 on another-- and the smaller degrees left over fill in
    /// under them, one of them one level deeper to reach depth 6.
    fn real_shape() -> Tree {
        let mut tree = Tree::default();
        let mut seq = 0u64;
        let mut num = 0u64;

        // Three roots. The third gets no children of its own, which makes
        // it one of the 147 leaves even though it is also a root.
        let root_a = fixture_node(&mut tree, &mut seq, &mut num, None);
        let root_b = fixture_node(&mut tree, &mut seq, &mut num, None);
        fixture_node(&mut tree, &mut seq, &mut num, None);

        // The chain the owner measured on one root: 56 at depth 0, 11 at
        // depth 1, 12 at depth 2, 6 at depth 3, 30 at depth 4.
        let root_a_children = fixture_children(&mut tree, &mut seq, &mut num, &root_a, 56);
        let p11_children = fixture_children(&mut tree, &mut seq, &mut num, &root_a_children[0], 11);
        let p12_children = fixture_children(&mut tree, &mut seq, &mut num, &p11_children[0], 12);
        let p6_children = fixture_children(&mut tree, &mut seq, &mut num, &p12_children[0], 6);
        let p30_children = fixture_children(&mut tree, &mut seq, &mut num, &p6_children[0], 30);

        // The other chain, on the second root: 4 at depth 0, 7 at depth 1,
        // 8 at depth 2.
        let root_b_children = fixture_children(&mut tree, &mut seq, &mut num, &root_b, 4);
        let p7_children = fixture_children(&mut tree, &mut seq, &mut num, &root_b_children[0], 7);
        let p8_children = fixture_children(&mut tree, &mut seq, &mut num, &p7_children[0], 8);

        // Everything the two chains left as a plain child -- the rest of
        // each root's own children, and every other child of the five
        // hubs above -- is a slot the smaller degrees left in
        // `REAL_DEGREES` can fill.
        let mut slots: Vec<String> = Vec::new();
        slots.extend(root_a_children[1..].iter().cloned());
        slots.extend(root_b_children[1..].iter().cloned());
        slots.extend(p11_children[1..].iter().cloned());
        slots.extend(p7_children[1..].iter().cloned());
        slots.extend(p12_children[1..].iter().cloned());
        slots.extend(p8_children.iter().cloned());
        slots.extend(p6_children[1..].iter().cloned());

        // The eight degrees already placed above, at the depths the owner
        // measured, spent from the sequence.
        let mut remaining: Vec<usize> = REAL_DEGREES.to_vec();
        for placed in [56usize, 11, 12, 6, 30, 4, 7, 8] {
            let at = remaining
                .iter()
                .position(|&d| d == placed)
                .expect("every placed degree is in REAL_DEGREES");
            remaining.remove(at);
        }

        // One of what is left has to reach depth 6, or the shape falls
        // short of the measurement. `p30`'s children sit at depth 5, so
        // spending a single leftover degree there -- the smallest one, a
        // lone child -- reaches depth 6 without going past it.
        let one_at = remaining
            .iter()
            .position(|&d| d == 1)
            .expect("REAL_DEGREES carries a 1 to spend at depth 6");
        remaining.remove(one_at);
        fixture_node(&mut tree, &mut seq, &mut num, Some(&p30_children[0]));

        // The rest fill the slots left over from the two chains, one
        // degree per slot; there are more slots than degrees left, so most
        // slots stay plain leaves, which is the same mix `WEB.md` §3.6
        // describes.
        for (slot, degree) in slots.iter().zip(remaining.iter()) {
            for _ in 0..*degree {
                fixture_node(&mut tree, &mut seq, &mut num, Some(slot));
            }
        }

        tree.sort_nodes();
        tree
    }

    /// The control: the same total node count as `real_shape`, the same
    /// max depth, and at most three children per node -- what `WEB.md`
    /// §7.7 calls an even tree.
    fn even_shape() -> Tree {
        let mut tree = Tree::default();
        let mut seq = 0u64;
        let mut num = 0u64;

        let root = fixture_node(&mut tree, &mut seq, &mut num, None);
        let mut levels: Vec<Vec<String>> = vec![vec![root]];
        // How many nodes of the current level get exactly three children,
        // one level at a time. The rest of that level stays a leaf. Chosen
        // so each new level's total is the largest multiple of three the
        // cap allows, which is what keeps every node at or under three
        // children while still reaching depth 6.
        let branching = [1usize, 3, 9, 15, 27, 9];
        for parents_with_children in branching {
            let mut next = Vec::new();
            for parent in levels.last().unwrap().iter().take(parents_with_children) {
                next.extend(fixture_children(&mut tree, &mut seq, &mut num, parent, 3));
            }
            levels.push(next);
        }
        // The branching above totals 193, two short of `real_shape`'s 195
        // (`WEB.md` §3.6 measured a few hours earlier than the count
        // `REAL_DEGREES` carries). Two more leaves close the gap, each a
        // single child of an otherwise childless depth-5 node: still at
        // most three children, and still depth 6.
        for parent in levels[5].iter().skip(9).take(2) {
            fixture_node(&mut tree, &mut seq, &mut num, Some(parent));
        }

        tree.sort_nodes();
        tree
    }

    /// The maximum depth under `tree`'s roots, root itself at 0.
    fn max_depth(tree: &Tree) -> usize {
        fn under(tree: &Tree, n: &Node, depth: usize) -> usize {
            tree.children(&n.id)
                .iter()
                .map(|c| under(tree, c, depth + 1))
                .max()
                .unwrap_or(depth)
        }
        tree.roots()
            .iter()
            .map(|r| under(tree, r, 0))
            .max()
            .unwrap_or(0)
    }

    /// The shape `real_shape` has to reproduce exactly, not approximately:
    /// the node count, the root count, the leaf count, the max depth, and
    /// the degree of every node that has at least one child. A fixture that
    /// merely resembles the measurement is a judge that can be fooled.
    #[test]
    fn real_shape_matches_the_measured_degree_sequence() {
        let tree = real_shape();
        assert_eq!(tree.total(), 195, "node count");
        assert_eq!(tree.roots().len(), 3, "root count");

        let mut degrees: Vec<usize> = tree
            .nodes_iter()
            .map(|n| tree.children(&n.id).len())
            .filter(|&d| d > 0)
            .collect();
        degrees.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(
            degrees, REAL_DEGREES,
            "the degree sequence must match the measurement exactly"
        );

        assert_eq!(tree.total() - degrees.len(), 147, "leaf count");
        assert_eq!(max_depth(&tree), 6, "max depth");
    }

    /// `d196` wrapped every row in a `<details>`; it must not have moved
    /// which tiles exist. 195 nodes minus 3 roots is 192 non-root nodes,
    /// and every one of them is still exactly one tile.
    #[test]
    fn wrapping_a_row_in_details_does_not_change_which_tiles_exist() {
        let tree = real_shape();
        let page = tree_page("vivac", "vivac", &tree);
        assert_eq!(
            page.matches("class=\"tile").count(),
            192,
            "192 non-root nodes should still be 192 tiles"
        );
    }

    /// The control has to weigh the same as the tree it is a control for:
    /// a judge shown a bigger tree next to a smaller one is judging size,
    /// not shape.
    #[test]
    fn even_shape_has_the_same_total_as_real_shape() {
        assert_eq!(even_shape().total(), real_shape().total());
        assert_eq!(max_depth(&even_shape()), 6, "max depth");
    }

    /// `WEB.md` §7.7: the acceptance harness for the promise `t191` and
    /// `d194` make -- that the drawing itself carries the fan-out, with no
    /// number and no caption stating the conclusion.
    ///
    /// This test cannot decide whether the drawing works. No automatic check
    /// can tell a broom from a bush by reading bytes; it renders both shapes
    /// with the same function and writes them side by side so a person can
    /// look and say whether they are obviously different. **The verdict is
    /// the owner's, per §7.7.**
    #[test]
    fn the_real_shape_does_not_render_like_an_even_tree() {
        let real = tree_page("vivac", "vivac", &real_shape());
        let even = tree_page("vivac", "vivac", &even_shape());

        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/shape");
        std::fs::create_dir_all(&dir).expect("target/shape can be created");
        let real_path = dir.join("real.html");
        let even_path = dir.join("even.html");
        std::fs::write(&real_path, &real).expect("real.html can be written");
        std::fs::write(&even_path, &even).expect("even.html can be written");

        assert!(std::fs::metadata(&real_path).unwrap().len() > 0);
        assert!(std::fs::metadata(&even_path).unwrap().len() > 0);
        println!("real shape:  {}", real_path.display());
        println!("even shape:  {}", even_path.display());
    }
}
