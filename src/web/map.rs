//! `WEB.md` §3.6 -- the map: the whole tree drawn as a transit map, with the
//! detail of any stop readable without letting go of the drawing.
//!
//! Its promise (`d391`, signed by the owner on 9-Sep-2026 **before** this
//! was built, which is what the UX pillar asks for): *you read why a node
//! exists without letting go of the tree you found it in, and you see at a
//! glance how much of what surrounds it is already closed.* `vivac why`
//! answers the first half and charges your place for it -- one node at a
//! time, in a terminal, with the tree around it gone while you read.
//!
//! It **replaces** the spine-and-comb this page used to draw (`d194`,
//! `d387`) rather than sitting beside it, because two drawings of the same
//! tree is exactly the second map `d125` refuses. `t191`'s promise -- how
//! branched the work really is, at a glance -- is not dropped in the trade:
//! it rides on the radius of each station, which *is* its fan-out. A map
//! that stopped showing that would be a regression, not a change of
//! subject, and `the_real_shape_does_not_render_like_an_even_tree` is still
//! here to catch it.
//!
//! ## What the drawing says
//!
//! A **line** is a route, not a branch: it follows the heaviest child at
//! every step, and every sibling that turns off starts a line of its own.
//! On the real tree that is 262 lines of which only 8 reach five stations,
//! so eight earn a colour and a name and the rest stay grey -- which is the
//! whole reason the colours read at all. Lines are threaded over the
//! **whole** tree, never over what is on screen: fold a branch away and
//! `g1` is still `g1`, in the same colour. A line that ends carries a
//! terminus cap: provenance is a tree (`MODEL.md` §9, invariant 11: at most
//! one incoming edge), so no line ever rejoins another, and the cap says
//! the line finished rather than that the drawing got cut off.
//!
//! A **lane** is one ancestor. Lanes alive at any row are exactly the
//! ancestor chain, so the gutter is as wide as the tree is deep -- 15 --
//! and not as wide as it is branched -- 125. That is the measurement the
//! whole form rests on: 318 px of gutter on a desktop, 158 on a phone,
//! fold controls included.
//!
//! Nothing here is carried by colour alone, which the DX pillar forbids:
//! state is the fill of the station *and* a strike through the title *and*
//! the word in the detail; blocking is a ring *and* the same `*` glyph
//! `vivac tree` prints; the line is a colour *and* its name in the legend
//! and in the detail.
//!
//! ## Why this page has script, when no other one does
//!
//! The promise is "without letting go". A link that navigates is letting
//! go, so the detail has to arrive without one -- and that is interaction,
//! which is script. It is inline (the CSP admits `script-src
//! 'unsafe-inline'` and no external script at all) and it reaches for
//! nothing: the page holds every byte it will ever need.
//!
//! The page still works with script off, and that is not an accident: the
//! rails, the stations and the rows are all rendered here, on the server,
//! and every alias is a real link to its own lineage (`d147`). Script
//! *intercepts* that link to open the panel instead. With no script you get
//! the old bargain -- a click costs you your place -- which is exactly the
//! product before today.
//!
//! ## Why the whole detail travels with the page
//!
//! Every node's `why`, outcome and notes are embedded, as JSON, in a
//! `<script type="application/json">` the panel reads. The alternative --
//! fetching one node at a time -- would need `connect-src` added to a CSP
//! that today says `default-src 'none'`, so the page could not talk to
//! anything at all. Measured on the real tree, the detail is 821 KB of a
//! 1 MB page: the cost of the promise, paid once, on `127.0.0.1`.
//!
//! It does not scale, and saying so here is cheaper than letting somebody
//! find out: the page grows with the *prose* in the tree, so a ten thousand
//! node tree would ship tens of megabytes. `f393` holds that.
//!
//! ## Folding, and what folding is not
//!
//! A tree drawn in one column puts a node's siblings as far apart as the
//! work between them is deep, so on the real tree "the next one along" is
//! the hardest thing on the page to reach. Folding answers that: `?fold=`
//! names the nodes whose subtrees are not drawn, and the server recomputes
//! the map over what is left. `Fold` says why it is a URL and not a
//! gesture.
//!
//! Both controls are **in the drawing**, because what they fold is a branch
//! and the branch is what the gutter draws. A node with children carries a
//! triangle in a column of its own before lane zero, drawn against the same
//! row as its station so the two cannot drift apart; and the head of each
//! lane says how many levels to show, sitting over the column it stops at
//! -- a lane *is* a depth, so nobody has to translate a number into a
//! position. Both are links, so both work with no script at all.
//!
//! The two compose in one direction only, and on purpose. `fold=` is a set
//! of nodes and `depth=` is **one number**: clicking a depth says how many
//! levels, whatever was clicked before it, so two clicks in any order leave
//! the same view as the second one alone. The first version spelled a depth
//! out as the aliases of every node at it and piled those into `fold=`,
//! which meant folding three depths took three undos in reverse order.
//!
//! What a fold does **not** do is take a node out of the page. The map holds
//! every node and gives a row only to what is drawn, so the search still
//! reads the whole tree: folding is a way of looking, not a claim that the
//! rest stopped existing.
//!
//! What folding is **not** is an "only what is open" filter. That one is
//! still not here and still not authorised: it is not navigation, it is a
//! claim about what matters, and `d391`'s sentence does not make it. The
//! mechanism it would need now exists, which is the whole of what changed
//! -- `f394` is where it gets decided, not here.
//!
//! Nor does folding settle the page's height (`f395`): 395 rows at 32 px is
//! still twelve thousand pixels when nothing is folded, which is what the
//! page is for.

use super::escape;
use crate::event::State;
use crate::model::{Aggregates, Node, Tree};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

/// The script the page carries, in its own file for the reason `web.css` is
/// in its own file: nobody maintains two hundred lines of anything inside a
/// string literal.
const MAP_JS: &str = include_str!("map.js");

/// One row of the map, in pixels. Everything vertical is a multiple of it,
/// on the server and in the script, because a row's index *is* its position:
/// the drawing and the list are two views of one ordered walk, and the one
/// bug that shape can have is the two disagreeing.
const ROW: u32 = 32;

/// A lane's width and where the first one starts, in pixels. Two of them,
/// because the geometry of an SVG is attributes and not styles: the same
/// drawing cannot be 18 px wide per level on a desktop and 9 on a phone
/// without being drawn twice. It costs 79 KB of a 1 MB page, which is the
/// cheapest of the three ways to do it and the only one that needs no
/// browser to cooperate.
struct Lane {
    step: u32,
    origin: u32,
    /// Where the fold control sits, before lane zero. It has a column of the
    /// gutter to itself: a control drawn on top of the rails would have to
    /// share space with a station of radius 9.
    control: u32,
    /// The class that decides which of the two is on screen. Both are
    /// selected by class and never by element, because the first version of
    /// this drew both at once: `svg.g{display:block}` is (0,1,1) and beat
    /// the `.g-m{display:none}` that was meant to hide one.
    when: &'static str,
}

const WIDE: Lane = Lane {
    step: 18,
    origin: 34,
    control: 11,
    when: "wide",
};

/// A step narrower than the desktop's half, so that the column the control
/// took does not cost the phone any width: 15 levels at 8 px plus the
/// control is the 159 px the gutter measured before it had one.
const NARROW: Lane = Lane {
    step: 8,
    origin: 24,
    control: 9,
    when: "narrow",
};

impl Lane {
    fn x(&self, depth: usize) -> u32 {
        self.origin + self.step * depth as u32
    }
}

/// A line earns a colour at five stations, and eight of them earn one at
/// all. Both numbers are measurements, not taste: the real tree has 261
/// lines and exactly 8 of five stations or more, so a lower bar would hand
/// out colour to two-stop stubs and there would be no colour left to read.
const NAMED_MIN: usize = 5;
const NAMED_MAX: usize = 8;

/// How many direct children make a node a hub. The same threshold the
/// radius uses to jump, so the bold title on the row and the big circle in
/// the gutter always mean the same thing.
const HUB: usize = 10;

/// What the reader has folded away, read off the query string.
///
/// **Folding is a URL and not a gesture**, and that is the whole design.
/// The gutter is drawn by row index, so a row that goes missing takes its
/// rail's meaning with it (`f394`): folding has to *recompute* the map, not
/// hide part of one. Recomputing in the browser would put a second
/// implementation of the same drawing next to this one, which is the defect
/// `f380` is named after -- so the server recomputes, the fold rides in the
/// query, and every fold control on the page is a plain link.
///
/// What that buys, beyond one implementation: it works with script off, a
/// folded view can be bookmarked and sent to somebody, and the back button
/// unfolds.
#[derive(Default)]
struct Fold {
    /// Nodes whose children are not drawn.
    shut: BTreeSet<u64>,
    /// How many levels are drawn at all. `None` is the whole tree.
    ///
    /// **A limit and not a pile of folds**, which is the difference between
    /// a control that composes and one that does not. The first version
    /// expanded "fold to depth 6" into the aliases of every node at that
    /// depth and added them to `shut`, so folding 6, then 5, then 3 left
    /// three sets that had to be undone one at a time and in reverse. The
    /// owner called that strange, and it was. A depth is one number:
    /// clicking 3 says three levels, whatever was said before it.
    depth: Option<usize>,
}

impl Fold {
    /// **No percent-decoding**, for the reason the router gives for paths: an
    /// alias is letters and digits and never needs escaping, so a value that
    /// still carries a `%` resolves to no node and is dropped. Anything that
    /// does not resolve is dropped the same way, which is what keeps the
    /// page from ever echoing back a string a reader put in the address bar.
    fn parse(query: &str, tree: &Tree) -> Fold {
        let mut shut = BTreeSet::new();
        let mut depth = None;
        for field in query.split('&') {
            match field.split_once('=') {
                Some(("fold", value)) => {
                    for alias in value.split(',') {
                        if let Some(n) = tree.resolve(alias) {
                            shut.insert(n.num);
                        }
                    }
                }
                // Zero levels is a view of nothing, so it is not a number
                // this accepts.
                Some(("depth", value)) => depth = value.parse().ok().filter(|&d| d > 0),
                _ => {}
            }
        }
        Fold { shut, depth }
    }

    /// Whether this node's children are folded away by name.
    fn hides(&self, n: &Node) -> bool {
        self.shut.contains(&n.num)
    }

    /// Whether the depth limit is what stops a node at `depth` drawing its
    /// children. Not something a reader can undo one node at a time, which
    /// is why the control on such a row says so instead of offering.
    fn cuts(&self, depth: usize) -> bool {
        self.depth.is_some_and(|d| depth + 1 >= d)
    }

    /// A query for a fold, with both parts spelled out and `standing` as the
    /// fragment so the browser lands back where the reader was.
    fn url(tree: &Tree, shut: &BTreeSet<u64>, depth: Option<usize>, standing: &str) -> String {
        let mut parts = Vec::new();
        if let Some(d) = depth {
            parts.push(format!("depth={d}"));
        }
        let mut aliases: Vec<String> = shut
            .iter()
            .filter_map(|&num| tree.node_by_num(num).map(|x| x.alias()))
            .collect();
        aliases.sort();
        if !aliases.is_empty() {
            parts.push(format!("fold={}", aliases.join(",")));
        }
        let query = match parts.is_empty() {
            true => "?".to_string(),
            false => format!("?{}", parts.join("&")),
        };
        match standing.is_empty() {
            true => query,
            false => format!("{query}#{standing}"),
        }
    }

    /// The query for this fold with one node's state flipped, and the node
    /// itself as the fragment so the browser lands back where the reader was
    /// standing when they folded it.
    fn toggled(&self, tree: &Tree, n: &Node) -> String {
        let mut shut = self.shut.clone();
        if !shut.remove(&n.num) {
            shut.insert(n.num);
        }
        Fold::url(tree, &shut, self.depth, &n.alias())
    }

    /// The query for this fold at another depth, keeping whatever is folded
    /// by name. `None` is the whole tree.
    fn at_depth(&self, tree: &Tree, depth: Option<usize>) -> String {
        Fold::url(tree, &self.shut, depth, "")
    }
}

/// One node's place in the map. **Every** node has one, drawn or not: a
/// stop's index is its handle in the payload, and the payload is what the
/// search reads. Folding used to drop a node out of the list altogether,
/// which took it out of the search too -- fold `g1` and two hundred nodes
/// stopped being findable, while `vivac find` still found them.
struct Stop<'t> {
    node: &'t Node,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    /// How many direct children the node really has, folded or not. The
    /// radius reads this and never `children.len()`: folding is a way of
    /// looking at the tree, and it is not allowed to change what the drawing
    /// says about how branched the work is.
    fan: usize,
    /// Which row this stop is drawn on, or `None` when it is folded away.
    /// Everything vertical is computed from this and never from the index.
    row: Option<usize>,
    /// The last row anything in this subtree is drawn on: the far end of the
    /// rail this stop draws.
    last_row: Option<usize>,
    /// How many of this stop's descendants are drawn.
    drawn_below: usize,
    /// How many nodes this stop is holding out of sight.
    hidden: usize,
    line: usize,
}

/// The whole drawing, computed once: every node that is not folded away as
/// a stop, in tree order, over the lines of the whole tree.
struct Map<'t> {
    stops: Vec<Stop<'t>>,
    lines: Lines,
    /// Which stop a node is, by `num` -- `None` for a node that is folded
    /// away and therefore has no row to point at.
    index: HashMap<u64, usize>,
    /// The deepest lane *drawn*, which is what the stats report.
    depth: usize,
    /// The deepest lane the tree has, drawn or not. The gutter is always
    /// this wide: the ruler across its head measures the tree, so it has to
    /// fit whatever is folded -- and a gutter that changed width on every
    /// fold would shove the whole page sideways as well.
    lanes: usize,
}

/// `n` and everything under it, pre-order, appended to `stops`. Pre-order is
/// what makes a parent's row sit above its children's and its rail reach
/// exactly to the last of them.
///
/// `drawn` comes down from the parent: a node is drawn when its parent drew
/// its children, and a parent draws its children unless it is folded or the
/// depth limit stops there. So a subtree that is not drawn is still walked,
/// counted and carried -- it just has no row.
#[allow(clippy::too_many_arguments)]
fn walk<'t>(
    tree: &'t Tree,
    ag: &Aggregates,
    fold: &Fold,
    n: &'t Node,
    depth: usize,
    parent: Option<usize>,
    drawn: bool,
    stops: &mut Vec<Stop<'t>>,
    next_row: &mut usize,
) -> usize {
    let me = stops.len();
    let row = drawn.then(|| {
        let r = *next_row;
        *next_row += 1;
        r
    });
    stops.push(Stop {
        node: n,
        depth,
        parent,
        children: Vec::new(),
        fan: tree.children(n.num).len(),
        row,
        last_row: row,
        drawn_below: 0,
        hidden: 0,
        line: 0,
    });

    let shows = drawn && !fold.hides(n) && !fold.cuts(depth);
    let mut children = Vec::new();
    let mut drawn_below = 0;
    let mut last_row = row;
    for c in tree.children(n.num) {
        let at = walk(
            tree,
            ag,
            fold,
            c,
            depth + 1,
            Some(me),
            shows,
            stops,
            next_row,
        );
        drawn_below += stops[at].drawn_below + usize::from(stops[at].row.is_some());
        if stops[at].last_row.is_some() {
            last_row = stops[at].last_row;
        }
        children.push(at);
    }
    stops[me].children = children;
    stops[me].last_row = last_row;
    stops[me].drawn_below = drawn_below;
    // What folding costs, counted rather than implied: the whole subtree
    // minus what of it is on the page.
    stops[me].hidden = ag.counts(n.num).total.saturating_sub(drawn_below);
    me
}

/// Every line of the whole tree, threaded once.
///
/// **Threaded over the whole tree and never over what is on screen.** A line
/// is a property of the work, not of the view: fold a branch away and `g1`
/// has to still be `g1`, in the same colour, or the legend means something
/// different on every page. The first version threaded the visible stops and
/// the eight colours changed hands whenever anything was folded.
struct Lines {
    /// Which line each node is on, by `num`.
    of: HashMap<u64, usize>,
    /// The nodes of each line, in order, by `num`.
    chains: Vec<Vec<u64>>,
    /// The lines that earned a colour, best first. A line's position here is
    /// its colour: `named[0]` is `l0`.
    named: Vec<usize>,
    /// What a named line is called: the alias of the node it starts at,
    /// which is the oldest one on it.
    names: HashMap<usize, String>,
}

/// Thread the lines. A line follows the heaviest child at every step and is
/// followed to its end before any of the branches it left behind is started;
/// those wait in `pending`, which is a stack rather than recursion because a
/// line can be as long as the tree has nodes and a stack frame per station
/// is a stack frame too many.
fn thread(tree: &Tree, ag: &Aggregates) -> Lines {
    let mut of: HashMap<u64, usize> = HashMap::new();
    let mut chains: Vec<Vec<u64>> = Vec::new();
    let mut pending: Vec<u64> = tree.roots().iter().rev().map(|r| r.num).collect();

    while let Some(start) = pending.pop() {
        let id = chains.len();
        chains.push(Vec::new());
        let mut at = start;
        loop {
            of.insert(at, id);
            chains[id].push(at);
            let children = tree.children(at);
            let Some(first) = children.first() else {
                break;
            };
            // The one carrying the most work, ties to the first born, so the
            // drawing is the same every time it is drawn.
            let mut next = first.num;
            let mut best = ag.counts(first.num).total;
            for c in children.iter().skip(1) {
                let weight = ag.counts(c.num).total;
                if weight > best {
                    next = c.num;
                    best = weight;
                }
            }
            for c in children.iter().rev() {
                if c.num != next {
                    pending.push(c.num);
                }
            }
            at = next;
        }
    }

    // Longest first, and the tie broken by where the line starts so the
    // colours do not shuffle between two runs on the same tree.
    let mut rank: Vec<usize> = (0..chains.len()).collect();
    rank.sort_by_key(|&i| (std::cmp::Reverse(chains[i].len()), chains[i][0]));
    let named: Vec<usize> = rank
        .into_iter()
        .filter(|&i| chains[i].len() >= NAMED_MIN)
        .take(NAMED_MAX)
        .collect();
    let names: HashMap<usize, String> = named
        .iter()
        .filter_map(|&i| tree.node_by_num(chains[i][0]).map(|n| (i, n.alias())))
        .collect();

    Lines {
        of,
        chains,
        named,
        names,
    }
}

impl<'t> Map<'t> {
    fn of(tree: &'t Tree, ag: &Aggregates, fold: &Fold) -> Map<'t> {
        let lines = thread(tree, ag);
        let mut stops = Vec::new();
        let mut next_row = 0;
        for root in tree.roots() {
            walk(
                tree,
                ag,
                fold,
                root,
                0,
                None,
                true,
                &mut stops,
                &mut next_row,
            );
        }
        for s in stops.iter_mut() {
            s.line = *lines.of.get(&s.node.num).unwrap_or(&usize::MAX);
        }
        let index = stops
            .iter()
            .enumerate()
            .map(|(i, s)| (s.node.num, i))
            .collect();
        let depth = stops
            .iter()
            .filter(|s| s.row.is_some())
            .map(|s| s.depth)
            .max()
            .unwrap_or(0);
        Map {
            stops,
            lines,
            index,
            depth,
            lanes: levels(tree).saturating_sub(1),
        }
    }

    /// The class that colours one stop. `l0`..`l7` for a line that earned a
    /// colour, `lx` for everything else -- one class rather than a hex value
    /// in the markup, so the palette can differ between the light and the
    /// dark ground and neither is baked into the drawing.
    fn colour(&self, line: usize) -> String {
        match self.lines.named.iter().position(|&l| l == line) {
            Some(i) => format!("l{i}"),
            None => "lx".to_string(),
        }
    }

    /// A named line is called after the node it starts at, which is the
    /// oldest one on it. An unnamed line is not called anything.
    fn line_name(&self, line: usize) -> String {
        self.lines.names.get(&line).cloned().unwrap_or_default()
    }

    fn width(&self, lane: &Lane) -> u32 {
        lane.x(self.lanes) + 14
    }

    fn height(&self) -> u32 {
        self.shown() as u32 * ROW
    }

    /// How many stops are drawn. Never `stops.len()`, which counts the
    /// whole tree since the payload started carrying it.
    fn shown(&self) -> usize {
        self.drawn().count()
    }

    /// The stops with a row, in row order, which is the order they were
    /// walked in.
    fn drawn(&self) -> impl Iterator<Item = (usize, &Stop<'t>)> {
        self.stops
            .iter()
            .enumerate()
            .filter(|(_, s)| s.row.is_some())
    }
}

/// The radius of a station is its fan-out. This is the whole of `t191`'s
/// promise, carried without spending a pixel of width on it: the two nodes
/// almost everything hangs off are the two big circles, and a reader who
/// never counts anything still sees them.
fn radius(children: usize) -> f32 {
    match children {
        0 => 2.4,
        1..=4 => 3.6,
        5..=9 => 5.0,
        10..=29 => 6.5,
        _ => 9.0,
    }
}

/// The middle of row `i`.
fn y(i: usize) -> u32 {
    i as u32 * ROW + ROW / 2
}

/// One gutter, at one lane width. Rails first, then elbows, then caps, then
/// the stations on top of all of them -- SVG paints in document order and a
/// station is what the reader aims at.
///
/// The whole drawing is `aria-hidden`: it is a picture of the list next to
/// it, and a reader who cannot see it is not served by hearing 389 circles
/// announced. The list carries the text, the links and the keyboard.
fn gutter(tree: &Tree, map: &Map, lane: &Lane, fold: &Fold) -> String {
    // The drawing is `aria-hidden`: it is a picture of the list next to it,
    // and a reader who cannot see it is not served by hearing 395 circles
    // announced. The fold controls at the end are outside that group, and
    // labelled, because they are the one part of this that does something.
    let mut out = format!(
        "<svg class=\"rails {when}\" width=\"{w}\" height=\"{h}\" \
         data-step=\"{step}\" data-origin=\"{origin}\" data-row=\"{ROW}\">\n\
         <g aria-hidden=\"true\">\n",
        when = lane.when,
        w = map.width(lane),
        h = map.height(),
        step = lane.step,
        origin = lane.origin,
    );

    for (_, s) in map.drawn() {
        let (x, mid) = (lane.x(s.depth), y(s.row.unwrap_or(0)));
        let c = map.colour(s.line);
        if s.drawn_below > 0 {
            // The rail runs from this stop down to the last row anything
            // below it is drawn on.
            out.push_str(&format!(
                "<path class=\"rail {c}\" d=\"M{x} {mid} V{end}\"/>\n",
                end = y(s.last_row.unwrap_or(0)),
            ));
        }
        if let Some(p) = s.parent.filter(|&p| map.stops[p].row.is_some()) {
            // Down the parent's lane, then a quarter turn into this one.
            // The turn is always exactly one lane wide, because a child is
            // always exactly one level deeper.
            let px = lane.x(map.stops[p].depth);
            out.push_str(&format!(
                "<path class=\"elbow {c}\" d=\"M{px} {top} V{turn} Q{px} {mid} {corner} {mid} H{x}\"/>\n",
                top = mid - ROW / 2,
                turn = mid - 7,
                corner = px + 7,
            ));
        }
    }

    // A cap is drawn where a line really ends, and only if that station is
    // on the page. Folding cuts a line short, and a cap on the last station
    // still showing would say the line finished there when it did not.
    for &line in &map.lines.named {
        let end = *map.lines.chains[line]
            .last()
            .expect("a line in `named` has at least NAMED_MIN stations");
        let Some(last) = map
            .index
            .get(&end)
            .map(|&at| &map.stops[at])
            .filter(|s| s.row.is_some())
        else {
            continue;
        };
        let (x, mid) = (lane.x(last.depth), y(last.row.unwrap_or(0)));
        out.push_str(&format!(
            "<path class=\"terminus {c}\" d=\"M{left} {below} H{right}\"/>\n",
            c = map.colour(line),
            left = x - 5,
            right = x + 5,
            below = mid + 9,
        ));
    }

    for (i, s) in map.drawn() {
        let (x, mid) = (lane.x(s.depth), y(s.row.unwrap_or(0)));
        let r = radius(s.fan);
        out.push_str(&format!(
            "<circle class=\"station {c}{state}\" data-stop=\"{i}\" \
             cx=\"{x}\" cy=\"{mid}\" r=\"{r}\"/>\n",
            c = map.colour(s.line),
            state = if s.node.state.is_open() {
                " open"
            } else {
                " shut"
            },
        ));
        if s.node.blocks {
            out.push_str(&format!(
                "<circle class=\"waits\" cx=\"{x}\" cy=\"{mid}\" r=\"{ring}\"/>\n",
                ring = r + 3.6,
            ));
        }
    }

    // Where the script draws the route to the selected stop. Empty on the
    // server: nothing is selected when the page lands, which is the whole
    // fix for a first version that arrived with everything but one node
    // dimmed to a third.
    out.push_str("<g class=\"route\"></g></g>\n");

    // The fold controls, in a column of their own before lane zero, and
    // *inside* the drawing rather than at the end of the row. They belong to
    // the rails: what a reader folds is a branch of the tree, and the branch
    // is the thing the gutter draws. Being in the same SVG also means they
    // are placed by the same `y(i)` as the station they sit beside, so they
    // cannot drift out of step with it the way a third parallel list would.
    //
    // Real links, in SVG, so they work with no script at all -- and outside
    // the `aria-hidden` above, with a label each, because these are the one
    // part of the drawing that is not a picture of the list.
    out.push_str("<g class=\"folds\">\n");
    for (_, s) in map.drawn() {
        if s.fan == 0 {
            continue;
        }
        let n = s.node;
        let shut = fold.hides(n);
        let mid = y(s.row.unwrap_or(0));
        let (left, top, text) = (lane.control.saturating_sub(9), mid - 9, mid + 5);
        // A triangle that points down is open and one that points right is
        // shut: the shape a disclosure has had since before the web, and a
        // shape rather than a colour. A node the depth limit is cutting has
        // no offer to make -- one node cannot come back from a limit that is
        // set for the whole map -- so it says so instead of pretending.
        if fold.cuts(s.depth) {
            out.push_str(&format!(
                "<g class=\"fold off\"><title>{id} is at the last level the \
                 depth is set to</title>\
                 <text x=\"{x}\" y=\"{text}\">&#9656;</text></g>\n",
                id = escape(&n.alias()),
                x = lane.control,
            ));
            continue;
        }
        out.push_str(&format!(
            "<a class=\"fold{on}\" href=\"{href}\" aria-label=\"{label}\">\
             <rect x=\"{left}\" y=\"{top}\" width=\"18\" height=\"18\"/>\
             <text x=\"{x}\" y=\"{text}\">{glyph}</text></a>\n",
            on = if shut { " on" } else { "" },
            href = escape(&fold.toggled(tree, n)),
            label = escape(&if shut {
                format!("unfold {} nodes under {}", s.hidden, n.alias())
            } else {
                format!("fold everything under {}", n.alias())
            }),
            x = lane.control,
            glyph = if shut { "&#9656;" } else { "&#9662;" },
        ));
        // And the drawing says it too, where the rail would have gone: a cut
        // rail, which is the mark a drawing uses for "this carries on and is
        // not shown". An 11 px triangle in a column of its own turned out to
        // be too small to read a state off, and the state is the point.
        if shut {
            let x = lane.x(s.depth);
            out.push_str(&format!(
                "<path class=\"cut {c}\" d=\"M{a} {up} L{b} {down} M{a} {below} L{b} {rest}\"/>\n",
                c = map.colour(s.line),
                a = x - 5,
                b = x + 5,
                up = mid + 11,
                down = mid + 5,
                below = mid + 15,
                rest = mid + 9,
            ));
        }
    }
    out.push_str("</g></svg>\n");
    out
}

/// The list beside the drawing: one row per node, in the same order, at the
/// same offset. Flat, and `d387`'s reason still holds -- a row's meaning is
/// its rail, and an indent would say the same thing twice and worse.
fn rows(project: &str, tree: &Tree, map: &Map, ag: &Aggregates, fold: &Fold) -> String {
    let mut out = String::from("<ol class=\"stops\">\n");
    for (i, s) in map.drawn() {
        let n = s.node;
        let alias = n.alias();
        let mut class = String::from("stop");
        if !n.state.is_open() {
            class.push_str(" shut");
        }
        if n.state == State::Suspended {
            class.push_str(" parked");
        }
        if s.fan >= HUB {
            class.push_str(" hub");
        }
        if n.state == State::Done && ag.blockers(n.num) > 0 {
            class.push_str(" false-close");
        }
        if fold.hides(n) || (s.fan > 0 && fold.cuts(s.depth)) {
            class.push_str(" folded");
        }
        let notes = match n.notes.len() {
            0 | 1 => String::new(),
            many => format!("<span class=\"notes\">{many} notes</span>"),
        };
        let fan = match s.fan {
            0 => String::new(),
            c => format!("<span class=\"fan\">{c}</span>"),
        };
        // How many this row is holding out of sight. The control that put
        // them there lives in the gutter, beside the rail it cut; what
        // belongs on the row is the count, which is the one thing a reader
        // cannot see for themselves once the subtree is gone.
        // Only the row that is doing the hiding says how much. Every
        // ancestor of a folded node also has descendants off the page --
        // 198 of `g1`'s, when `g132` is folded -- and a count on each of
        // them read as an offer to unfold something they do not hold.
        let control = match s.fan > 0 && (fold.hides(n) || fold.cuts(s.depth)) {
            true => format!("<span class=\"held\">+{}</span>", s.hidden),
            false => String::new(),
        };
        // The `*` goes *inside* the alias, which is where `vivac tree` puts
        // it and where the old tile put it. Outside, it was a fifth child of
        // a four-column grid: a row whose node blocks pushed its title into
        // the next column and its fan-out off the end, so the whole list
        // went crooked wherever one of the 31 blockers landed.
        out.push_str(&format!(
            "<li class=\"{class}\" id=\"{id}\" data-stop=\"{i}\">\
             <a class=\"alias {c}\" href=\"/p/{p}/why/{id}\" \
             title=\"{id} · {w}{b} · {t}\">{id}{mark}</a>\
             <span class=\"title\">{t}</span>{notes}{fan}{control}</li>\n",
            c = map.colour(s.line),
            id = escape(&alias),
            p = escape(project),
            w = escape(n.state.word(n.kind)),
            b = if n.blocks { " · blocking" } else { "" },
            t = escape(n.title(tree)),
            mark = if n.blocks {
                "<span class=\"mark\" aria-hidden=\"true\">*</span>"
            } else {
                ""
            },
        ));
    }
    out.push_str("</ol>\n");
    out
}

/// The eight named lines, as the legend and as a highlight. A line's name is
/// its oldest station, which is also where a reader can go to see what the
/// line is about, and its count is its length in the whole tree -- folding
/// changes what is drawn and never what a line is.
fn legend(map: &Map) -> String {
    let mut out = String::from("<div class=\"legend\">\n");
    for (i, &line) in map.lines.named.iter().enumerate() {
        out.push_str(&format!(
            "<button class=\"line l{i}\" data-line=\"{name}\" type=\"button\">\
             <b></b>{name} · {n}</button>\n",
            name = escape(&map.line_name(line)),
            n = map.lines.chains[line].len(),
        ));
    }
    out.push_str("</div>\n");
    out
}

/// Everything the panel says about every node, as one JSON array in tree
/// order -- **every** node, folded away or not.
///
/// A folded node has no row and no station, and it used to have no entry
/// either, which quietly took it out of the page's own search: fold `g1` and
/// two hundred nodes stopped being findable here while `vivac find` went on
/// finding them. `r` is the row a stop is drawn on, or `null`, and it is the
/// only thing that separates the two.
///
/// `<` is escaped even though `serde_json` has already made the value a
/// legal JSON string: legal JSON can still contain the four characters that
/// end a `<script>` element, and a tree is allowed to hold a node called
/// `</script>`.
fn payload(project: &str, tree: &Tree, map: &Map, ag: &Aggregates, fold: &Fold) -> String {
    let stops: Vec<serde_json::Value> = map
        .stops
        .iter()
        .map(|s| {
            let hides_here = s.fan > 0 && (fold.hides(s.node) || fold.cuts(s.depth));
            let n = s.node;
            let below = ag.counts(n.num);
            let blocking = crate::render::blocking_of(tree, n);
            json!({
                "a": n.alias(),
                "t": n.title(tree),
                "k": n.kind.word(),
                "s": n.state.word(n.kind),
                "b": n.blocks,
                "w": n.why(tree),
                "o": n.outcome(tree),
                "nt": n.notes(tree)
                    .iter()
                    .map(|(at, text)| json!({"at": at, "n": text}))
                    .collect::<Vec<_>>(),
                "rf": n.refs(tree),
                "gv": n.governs(tree),
                "op": n.opened(tree),
                "cl": n.closed(tree),
                "fc": n.state == State::Done && ag.blockers(n.num) > 0,
                "d": s.depth,
                "p": s.parent,
                "c": s.fan,
                "ob": below.open_count,
                "tb": below.total,
                "ln": map.line_name(s.line),
                // The row it is drawn on, and `null` when it is folded
                // away: everything the script measures reads this and never
                // the index.
                "r": s.row,
                "hd": if hides_here { s.hidden } else { 0 },
                // What holds this one open, as stop indices: the same
                // filter `tree` and `why` read, so the three surfaces
                // cannot disagree about what a debt is (`f380`).
                //
                // Every node has an entry now, folded or not, so this is
                // the whole list and `bn` is its length. Whether one of them
                // is on the page is a question about `r`, which the panel
                // asks for itself.
                "bn": blocking.len(),
                "bl": blocking
                    .iter()
                    .filter_map(|b| map.index.get(&b.num))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    let folded: Vec<String> = fold
        .shut
        .iter()
        .filter_map(|&num| tree.node_by_num(num).map(|n| n.alias()))
        .collect();
    let raw = json!({
        "project": project,
        "fold": folded,
        "depth": fold.depth,
        "stops": stops,
    })
    .to_string();
    raw.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// What the panel holds while nothing is selected: how to read the
/// drawing.
///
/// Rendered here rather than by the script for two reasons. It is the one
/// part of the panel that is true before any click, so a reader with no
/// script still gets it; and the page used to land with a node already
/// chosen and everything else dimmed, which read as the whole page having
/// gone dark. Landing on the key instead is the fix.
///
/// It is a key to a drawing, and every line of it names something the
/// drawing does *and* something a word already says, which is the DX
/// pillar's rule about colour applied to shape.
fn key(tree: &Tree) -> String {
    format!(
        "<div class=\"resting\">\n<p class=\"code\">nothing selected</p>\n\
         <h2>The whole tree, {n} nodes</h2>\n\
         <p class=\"prose\">Every station is a node and every rail a line of \
         provenance. Click a row or a station and the route from the root is \
         drawn across the map and lit along the titles.</p>\n\
         <h3>How to read it</h3>\n<dl>\n\
         <dt>size</dt><dd>direct children: the biggest circles are the places \
         nearly all the work hangs from</dd>\n\
         <dt>fill</dt><dd>solid is open, hollow is closed</dd>\n\
         <dt>ring</dt><dd>blocks its parent, which cannot close until this \
         one does</dd>\n\
         <dt>cap</dt><dd>the end of a line. Provenance is a tree, so no line \
         ever rejoins another</dd>\n\
         <dt>struck out</dt><dd>closed</dd>\n\
         <dt>triangle</dt><dd>folds everything under that node, so its \
         neighbours come within reach</dd>\n\
         <dt>numbers above</dt><dd>fold a whole depth at once, one per lane</dd>\n\
         <dt>keys</dt><dd>up and down walk the list, / finds, Esc closes</dd>\n\
         </dl>\n</div>\n",
        n = tree.total(),
    )
}

/// The line under the title: what the page is, in numbers, before anybody
/// has clicked anything.
fn stats(map: &Map, tree: &Tree) -> String {
    let open = map.drawn().filter(|(_, s)| s.node.state.is_open()).count();
    let blocking = map.drawn().filter(|(_, s)| s.node.blocks).count();
    // Every number here counts what is drawn, except the total, which counts
    // the tree. Folding is a way of looking and the two have to be told
    // apart, so the gap between them is spelled out rather than left for the
    // reader to notice.
    let folded = tree.total().saturating_sub(map.shown());
    let away = match folded {
        0 => String::new(),
        n => format!(" · {n} folded away"),
    };
    format!(
        "{drawn} of {n} nodes{away} · {open} open · {blocking} blocking · \
         depth {d} · {lines} lines, {named} of them named",
        drawn = map.shown(),
        n = tree.total(),
        d = map.depth,
        lines = map.lines.chains.len(),
        named = map.lines.named.len(),
    )
}

/// Fold everything at one depth, as a link each. The deepest level worth
/// offering is the one below the shallowest node that has children -- past
/// that a link would fold nothing.
///
/// Each of these is the same mechanism as one row's control, only spelled
/// out over every node at that depth: there is one way to fold and the URL
/// always says exactly what is folded, node by node. A `depth=` of its own
/// would have been shorter and would have left two mechanisms that answer
/// to each other, with a row's unfold unable to undo it.
/// How many levels deep the whole tree goes. The ruler is drawn from this
/// and never from what is on screen: a ruler that loses its far end when
/// something is folded is a ruler you cannot use to get back.
fn levels(tree: &Tree) -> usize {
    fn deepest(tree: &Tree, n: &Node, depth: usize) -> usize {
        tree.children(n.num)
            .iter()
            .map(|c| deepest(tree, c, depth + 1))
            .max()
            .unwrap_or(depth)
    }
    tree.roots()
        .iter()
        .map(|r| deepest(tree, r, 0))
        .max()
        .map(|d| d + 1)
        .unwrap_or(0)
}

/// One control per lane, at the head of the column it measures.
///
/// A lane **is** a depth, so the head of a lane is where "show me this many
/// levels" belongs: over the column it stops at, with nobody having to
/// translate a number into a position on the drawing.
///
/// Exactly one is marked, because a depth is one number and not a pile. Two
/// clicks in any order leave the same view as the second one alone, and
/// clicking the marked one gives the whole tree back.
///
/// Wide layout only. A lane is 8 px across on a phone and no control fits in
/// 8 px, so the narrow one keeps the same links as a row of chips in the
/// tools. Same links and the same mechanism, in two shapes.
fn heads(tree: &Tree, fold: &Fold, lane: &Lane) -> String {
    let deep = levels(tree);
    if deep == 0 {
        return String::new();
    }
    let mut out = format!("<div class=\"heads {when}\">", when = lane.when);
    for level in 1..=deep {
        let here = fold.depth == Some(level);
        out.push_str(&format!(
            "<a class=\"head{on}\" style=\"left:{left}px\" href=\"{href}\" \
             title=\"{what}\">{level}</a>",
            on = if here { " on" } else { "" },
            left = lane.x(level - 1).saturating_sub(9),
            href = escape(&fold.at_depth(tree, if here { None } else { Some(level) })),
            what = if here {
                "showing this many levels -- click for the whole tree".to_string()
            } else {
                format!(
                    "show {level} level{s}",
                    s = if level == 1 { "" } else { "s" }
                )
            },
        ));
    }
    out.push_str("</div>\n");
    out
}

/// The same ruler as a row of chips: what the phone gets, where a lane is
/// too narrow to carry a control of its own.
fn depths(tree: &Tree, fold: &Fold) -> String {
    let deep = levels(tree);
    if deep == 0 {
        return String::new();
    }
    let mut out = String::from("<span class=\"depths\">levels ");
    for level in 1..=deep {
        let here = fold.depth == Some(level);
        out.push_str(&format!(
            "<a class=\"depth{on}\" href=\"{href}\">{level}</a>",
            on = if here { " on" } else { "" },
            href = escape(&fold.at_depth(tree, if here { None } else { Some(level) })),
        ));
    }
    out.push_str(&format!(
        "<a class=\"depth{on}\" href=\"{href}\">all</a></span>",
        on = if fold.depth.is_none() { " on" } else { "" },
        href = escape(&fold.at_depth(tree, None)),
    ));
    out
}

/// The way back to the whole tree, and it is only drawn when there is
/// something to come back from.
///
/// It used to live inside the depth chips, which the wide layout hides --
/// so on a desktop the page had no visible way out of a fold at all, and
/// the reader had to undo each level in the order they folded it. The
/// escape hatch is now its own control, on both layouts, and it says how
/// much it will bring back.
fn unfold_all(map: &Map, tree: &Tree) -> String {
    let folded = tree.total().saturating_sub(map.shown());
    match folded {
        0 => String::new(),
        n => format!("<a class=\"tool\" href=\"?\">Unfold everything · {n}</a>\n"),
    }
}

/// The other end of the same pair: everything but the roots, in one click.
///
/// It is `depth=1` and not a fold of every node with children, and those are
/// the same view by two roads. The road matters: one number comes undone
/// with one click, and a set of a hundred and twenty-five aliases does not.
fn fold_all(map: &Map, tree: &Tree) -> String {
    if map.shown() <= tree.roots().len() {
        return String::new();
    }
    "<a class=\"tool\" href=\"?depth=1\">Fold everything</a>\n".to_string()
}

/// The whole tree of `project`, drawn as a map, minus whatever `query` says
/// the reader has folded away. Never `None`: an empty tree still gets a
/// page, it just has nothing to draw.
pub(super) fn map_page(project: &str, name: &str, tree: &Tree, query: &str) -> String {
    let ag = tree.aggregates();
    let fold = Fold::parse(query, tree);
    let map = Map::of(tree, &ag, &fold);

    if tree.is_empty_tree() {
        return shell(
            project,
            name,
            "<p class=\"empty\">Empty tree.</p>\n".to_string(),
            String::new(),
            String::new(),
            String::new(),
        );
    }

    // Where "where am I" goes. The focus is a node, and a node is a stop;
    // if the stack is empty there is nowhere to go and the button is not
    // drawn at all rather than drawn dead.
    let focus = tree
        .focus()
        .and_then(|f| map.stops.iter().position(|s| s.node.num == f.num));
    let here = match focus {
        Some(i) => format!(
            "<button class=\"tool\" id=\"here\" type=\"button\" data-stop=\"{i}\">\
             Where am I?</button>\n"
        ),
        None => String::new(),
    };

    let body = format!(
        "<div class=\"map\">\n{heads}<div class=\"gutter\">{wide}{narrow}</div>\n\
         {rows}<aside id=\"detail\" class=\"detail\">{key}</aside>\n</div>\n",
        heads = heads(tree, &fold, &WIDE),
        wide = gutter(tree, &map, &WIDE, &fold),
        narrow = gutter(tree, &map, &NARROW, &fold),
        rows = rows(project, tree, &map, &ag, &fold),
        key = key(tree),
    );

    shell(
        project,
        name,
        body,
        format!(
            "<p class=\"stats\">{stats}</p>\n{legend}<div class=\"tools\">{here}{shut}{back}\
             <input id=\"find\" type=\"search\" \
             placeholder=\"Find in alias, title and why…\" \
             aria-label=\"Find a station\">\
             <span class=\"hits\" id=\"hits\" role=\"status\"></span>{depths}</div>\n",
            stats = escape(&stats(&map, tree)),
            shut = fold_all(&map, tree),
            back = unfold_all(&map, tree),
            legend = legend(&map),
            depths = depths(tree, &fold),
        ),
        payload(project, tree, &map, &ag, &fold),
        MAP_JS.to_string(),
    )
}

/// The page around the drawing. Split out so the empty tree takes exactly
/// the same route through here as a full one.
fn shell(
    project: &str,
    name: &str,
    body: String,
    head: String,
    data: String,
    script: String,
) -> String {
    let carried = if data.is_empty() {
        String::new()
    } else {
        format!(
            "<script type=\"application/json\" id=\"map-data\">{data}</script>\n\
             <script>{script}</script>\n"
        )
    };
    format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Map - {name_t}</title>\n\
         <style>\n{css}</style></head>\n\
         <body class=\"wide-page\"><div class=\"page\">\n\
         <header><p class=\"crumb\"><a href=\"/p/{p}/\">{name_t}</a></p>\n\
         <h1>The map</h1>\n\
         <p class=\"promise\">Why a node exists, read without letting go of the \
         tree you found it in -- and how much of what surrounds it is already \
         closed, at a glance.</p>\n{head}</header>\n\
         <main>\n{body}</main>\n\
         <footer>The same readings in a terminal: <code>vivac tree --all</code> \
         and <code>vivac why &lt;id&gt;</code></footer>\n\
         </div>\n{carried}</body></html>\n",
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
    /// 48 with children.
    ///
    /// **Dated on purpose, and not a mirror of the live tree.** If it
    /// followed the tree `vivac-project/.vivac` holds today, the §7.7
    /// harness below would change on its own every time somebody writes a
    /// node, and a judge that moves judges nothing.
    const REAL_DEGREES: &[usize] = &[
        56, 30, 12, 11, 8, 7, 6, 4, 3, 3, 3, //
        2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, //
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, //
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, //
        1, 1, 1, 1, 1, //
    ];

    /// One node, applied straight to `tree` rather than folded from a log
    /// of `Event`s: `Tree::apply` is the same function the fold uses.
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
                arms: vec![],
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
    /// `REAL_DEGREES` rather than by hand.
    fn real_shape() -> Tree {
        let mut tree = Tree::default();
        let mut seq = 0u64;
        let mut num = 0u64;

        let root_a = fixture_node(&mut tree, &mut seq, &mut num, None);
        let root_b = fixture_node(&mut tree, &mut seq, &mut num, None);
        fixture_node(&mut tree, &mut seq, &mut num, None);

        let root_a_children = fixture_children(&mut tree, &mut seq, &mut num, &root_a, 56);
        let p11_children = fixture_children(&mut tree, &mut seq, &mut num, &root_a_children[0], 11);
        let p12_children = fixture_children(&mut tree, &mut seq, &mut num, &p11_children[0], 12);
        let p6_children = fixture_children(&mut tree, &mut seq, &mut num, &p12_children[0], 6);
        let p30_children = fixture_children(&mut tree, &mut seq, &mut num, &p6_children[0], 30);

        let root_b_children = fixture_children(&mut tree, &mut seq, &mut num, &root_b, 4);
        let p7_children = fixture_children(&mut tree, &mut seq, &mut num, &root_b_children[0], 7);
        let p8_children = fixture_children(&mut tree, &mut seq, &mut num, &p7_children[0], 8);

        let mut slots: Vec<String> = Vec::new();
        slots.extend(root_a_children[1..].iter().cloned());
        slots.extend(root_b_children[1..].iter().cloned());
        slots.extend(p11_children[1..].iter().cloned());
        slots.extend(p7_children[1..].iter().cloned());
        slots.extend(p12_children[1..].iter().cloned());
        slots.extend(p8_children.iter().cloned());
        slots.extend(p6_children[1..].iter().cloned());

        let mut remaining: Vec<usize> = REAL_DEGREES.to_vec();
        for placed in [56usize, 11, 12, 6, 30, 4, 7, 8] {
            let at = remaining
                .iter()
                .position(|&d| d == placed)
                .expect("every placed degree is in REAL_DEGREES");
            remaining.remove(at);
        }

        let one_at = remaining
            .iter()
            .position(|&d| d == 1)
            .expect("REAL_DEGREES carries a 1 to spend at depth 6");
        remaining.remove(one_at);
        fixture_node(&mut tree, &mut seq, &mut num, Some(&p30_children[0]));

        for (slot, degree) in slots.iter().zip(remaining.iter()) {
            for _ in 0..*degree {
                fixture_node(&mut tree, &mut seq, &mut num, Some(slot));
            }
        }

        tree.sort_nodes();
        tree
    }

    /// The control: the same total node count as `real_shape`, the same
    /// max depth, and at most three children per node.
    fn even_shape() -> Tree {
        let mut tree = Tree::default();
        let mut seq = 0u64;
        let mut num = 0u64;

        let root = fixture_node(&mut tree, &mut seq, &mut num, None);
        let mut levels: Vec<Vec<String>> = vec![vec![root]];
        let branching = [1usize, 3, 9, 15, 27, 9];
        for parents_with_children in branching {
            let mut next = Vec::new();
            for parent in levels.last().unwrap().iter().take(parents_with_children) {
                next.extend(fixture_children(&mut tree, &mut seq, &mut num, parent, 3));
            }
            levels.push(next);
        }
        for parent in levels[5].iter().skip(9).take(2) {
            fixture_node(&mut tree, &mut seq, &mut num, Some(parent));
        }

        tree.sort_nodes();
        tree
    }

    /// A tree with one blocker and one node closed on top of it: the two
    /// readings `f380` found missing from the whole web.
    fn blocked_shape() -> Tree {
        let mut tree = Tree::default();
        let (mut seq, mut num) = (0u64, 0u64);
        let root = fixture_node(&mut tree, &mut seq, &mut num, None);
        let branch = fixture_node(&mut tree, &mut seq, &mut num, Some(&root));
        seq += 1;
        num += 1;
        tree.apply(
            seq,
            "2026-09-09T10:00:00Z",
            &Body::NodeCreated {
                node: "n3".to_string(),
                num,
                kind: Kind::Question,
                title: "the blocker".to_string(),
                why: "fixture".to_string(),
                parent: Some(branch.clone()),
                blocks: true,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        );
        seq += 1;
        tree.apply(
            seq,
            "2026-09-09T11:00:00Z",
            &Body::StateChanged {
                node: branch,
                state: State::Done,
                outcome: "closed with a condition still open".to_string(),
                forced: true,
            },
        );
        tree.sort_nodes();
        tree
    }

    /// The maximum depth under `tree`'s roots, root itself at 0.
    fn max_depth(tree: &Tree) -> usize {
        fn under(tree: &Tree, n: &Node, depth: usize) -> usize {
            tree.children(n.num)
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

    #[test]
    fn real_shape_matches_the_measured_degree_sequence() {
        let tree = real_shape();
        assert_eq!(tree.total(), 195, "node count");
        assert_eq!(tree.roots().len(), 3, "root count");

        let mut degrees: Vec<usize> = tree
            .nodes_iter()
            .map(|n| tree.children(n.num).len())
            .filter(|&d| d > 0)
            .collect();
        degrees.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(degrees, REAL_DEGREES, "the degree sequence must match");
        assert_eq!(tree.total() - degrees.len(), 147, "leaf count");
        assert_eq!(max_depth(&tree), 6, "max depth");
    }

    /// The one bug this shape can have: the list and the drawing disagreeing
    /// about which row a node is on. Every node is one row and one station
    /// in each of the two gutters, and nothing is drawn that has no row.
    #[test]
    fn every_node_is_one_row_and_one_station_in_each_gutter() {
        let tree = real_shape();
        let page = map_page("vivac", "vivac", &tree, "");
        assert_eq!(page.matches("<li class=\"stop").count(), tree.total());
        assert_eq!(
            page.matches("class=\"station ").count(),
            tree.total() * 2,
            "one station per node in each of the two gutters"
        );
    }

    /// Two gutters, told apart by class alone. The first version of this
    /// drew both at once because the rule meant to hide one was less
    /// specific than the rule that showed it.
    #[test]
    fn the_page_carries_one_gutter_per_lane_width() {
        let page = map_page("vivac", "vivac", &real_shape(), "");
        assert_eq!(page.matches("<svg class=\"rails").count(), 2);
        assert_eq!(page.matches("class=\"rails wide\"").count(), 1);
        assert_eq!(page.matches("class=\"rails narrow\"").count(), 1);
        assert!(
            !super::super::WEB_CSS.contains("svg.rails"),
            "selecting the gutter by element is what broke it the first time"
        );
    }

    /// Every stop belongs to exactly one line, and a line is a walk down
    /// the tree: each station after the first is a child of the one before.
    #[test]
    fn every_stop_is_on_exactly_one_line_and_a_line_walks_downwards() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let map = Map::of(&tree, &ag, &Fold::default());
        let mut seen = vec![0usize; map.stops.len()];
        for line in map.lines.chains.iter().map(|c| {
            c.iter()
                .filter_map(|num| map.index.get(num).copied())
                .collect::<Vec<usize>>()
        }) {
            let line = &line;
            for &s in line {
                seen[s] += 1;
            }
            for pair in line.windows(2) {
                assert_eq!(
                    map.stops[pair[1]].parent,
                    Some(pair[0]),
                    "a line only ever continues into a child"
                );
            }
        }
        assert!(
            seen.iter().all(|&c| c == 1),
            "every stop on exactly one line"
        );
    }

    /// A colour is earned, not handed out: at most eight lines have one,
    /// and none of them is shorter than five stations.
    #[test]
    fn only_the_long_lines_earn_a_colour() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let map = Map::of(&tree, &ag, &Fold::default());
        assert!(map.lines.named.len() <= NAMED_MAX);
        for &line in &map.lines.named {
            assert!(map.lines.chains[line].len() >= NAMED_MIN);
        }
        assert!(
            map.lines.chains.len() > map.lines.named.len(),
            "the real shape has more lines than colours, which is the point"
        );
    }

    /// The line follows the work. Given a fork where one side carries more
    /// nodes, the line continues into that side and the lighter sibling
    /// starts one of its own.
    #[test]
    fn the_line_continues_into_the_heaviest_child() {
        let mut tree = Tree::default();
        let (mut seq, mut num) = (0u64, 0u64);
        let root = fixture_node(&mut tree, &mut seq, &mut num, None);
        // Born first, and lighter: birth order must not decide this.
        let light = fixture_node(&mut tree, &mut seq, &mut num, Some(&root));
        let heavy = fixture_node(&mut tree, &mut seq, &mut num, Some(&root));
        fixture_children(&mut tree, &mut seq, &mut num, &heavy, 3);
        tree.sort_nodes();

        let ag = tree.aggregates();
        let map = Map::of(&tree, &ag, &Fold::default());
        let at = |id: &str| {
            map.stops
                .iter()
                .position(|s| s.node.id == id)
                .expect("the fixture node is a stop")
        };
        assert_eq!(
            map.stops[at(&root)].line,
            map.stops[at(&heavy)].line,
            "the line goes where the work is"
        );
        assert_ne!(
            map.stops[at(&root)].line,
            map.stops[at(&light)].line,
            "the sibling that turns off starts its own line"
        );
    }

    /// The promise, as far as a machine can hold it: the detail of every
    /// node is on the page before anybody clicks, so no click can fail to
    /// find it.
    #[test]
    fn the_detail_of_every_node_travels_with_the_page() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let map = Map::of(&tree, &ag, &Fold::default());
        let data = payload("vivac", &tree, &map, &ag, &Fold::default());
        let parsed: serde_json::Value =
            serde_json::from_str(&data.replace("\\u003c", "<").replace("\\u003e", ">"))
                .expect("the payload is JSON");
        let stops = parsed["stops"].as_array().expect("an array of stops");
        assert_eq!(stops.len(), tree.total());
        assert!(
            stops.iter().all(|s| s["w"] == "fixture"),
            "every why is here"
        );
    }

    /// `f389` again, on the surface it was found from: a node given two
    /// notes carries both, and the panel is not allowed to quietly show the
    /// last one.
    #[test]
    fn every_note_travels_with_the_page() {
        let mut tree = Tree::default();
        let (mut seq, mut num) = (0u64, 0u64);
        let id = fixture_node(&mut tree, &mut seq, &mut num, None);
        for (at, text) in [
            ("2026-09-08T10:00:00Z", "the first note"),
            ("2026-09-09T10:00:00Z", "the correction"),
        ] {
            seq += 1;
            tree.apply(
                seq,
                at,
                &Body::NodeNoted {
                    node: id.clone(),
                    note: text.to_string(),
                },
            );
        }
        tree.sort_nodes();

        let page = map_page("vivac", "vivac", &tree, "");
        assert!(page.contains("the first note"), "the covered note is here");
        assert!(page.contains("the correction"));
        assert!(page.contains("2 notes"), "and the row says there are two");
    }

    /// The `*` is on the row, the word is in the `title`, and neither of
    /// them is a colour: the DX pillar does not allow a meaning that only a
    /// colour carries, and `vivac tree` prints the same glyph.
    #[test]
    fn a_node_that_blocks_its_parent_is_marked_in_glyph_and_in_word() {
        let page = map_page("vivac", "vivac", &blocked_shape(), "");
        assert_eq!(page.matches("class=\"mark\"").count(), 1);
        assert!(page.contains("· blocking ·"), "the word, not only the mark");
        // Inside the alias, not beside it. A row is a four-column grid and
        // the mark was a fifth child of it, so every one of the 31 blockers
        // on the real tree shunted its own title one column to the right.
        assert!(
            page.contains("*</span></a>"),
            "the mark has to close before the alias does:\n{page}"
        );
        assert_eq!(
            page.matches("class=\"waits\"").count(),
            2,
            "the ring, once in each gutter"
        );
    }

    /// A node closed while something it waits on is still open. `vivac tree`
    /// says so in those words, and so does the row's own class.
    #[test]
    fn a_node_closed_over_an_open_condition_is_marked_on_its_row() {
        let page = map_page("vivac", "vivac", &blocked_shape(), "");
        assert!(page.contains("false-close"), "{page}");
        assert!(page.contains("\"fc\":true"), "and it travels in the detail");
    }

    /// Nothing is selected when the page lands. The first version arrived
    /// with one node chosen and everything else dimmed to a third, which
    /// the owner read -- correctly -- as the page having gone dark.
    #[test]
    fn nothing_is_selected_when_the_page_lands() {
        let page = map_page("vivac", "vivac", &real_shape(), "");
        assert!(!page.contains("class=\"stop on"), "no row arrives selected");
        assert!(
            page.contains("<g class=\"route\"></g>"),
            "and no route arrives drawn"
        );
    }

    /// The escaping test every page here carries, on this page's own new
    /// hole: the detail is JSON inside a `<script>`, and a tree is allowed
    /// to hold a node called `</script>`.
    #[test]
    fn a_node_title_cannot_close_the_script_it_travels_in() {
        let mut tree = Tree::default();
        let (mut seq, mut num) = (0u64, 0u64);
        let id = format!("n{}", num + 1);
        seq += 1;
        num += 1;
        tree.apply(
            seq,
            "2026-09-09T10:00:00Z",
            &Body::NodeCreated {
                node: id,
                num,
                kind: Kind::Task,
                title: "</script><script>alert(1)</script>".to_string(),
                why: "</script>".to_string(),
                parent: None,
                blocks: false,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        );
        tree.sort_nodes();

        let page = map_page("vivac", "vivac", &tree, "");
        assert!(!page.contains("<script>alert(1)"), "{page}");
        assert_eq!(
            page.matches("</script>").count(),
            2,
            "the two this page opens, and no third one a title smuggled in"
        );
    }

    /// The biggest hub on a fixture, which is the one worth folding.
    fn biggest_hub(tree: &Tree, ag: &Aggregates) -> String {
        let map = Map::of(tree, ag, &Fold::default());
        map.stops
            .iter()
            .max_by_key(|s| s.fan)
            .expect("the fixture has a node with children")
            .node
            .alias()
    }

    /// Folding takes a subtree off the page -- rows, stations and detail --
    /// and says how many it took, because that count is the one thing a
    /// reader cannot see for themselves once it is folded.
    #[test]
    fn folding_a_node_takes_its_subtree_off_the_page_and_counts_it() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let alias = biggest_hub(&tree, &ag);
        let under = ag
            .counts(tree.resolve(&alias).expect("the hub resolves").num)
            .total;

        let page = map_page("vivac", "vivac", &tree, &format!("fold={alias}"));
        assert_eq!(
            page.matches("<li class=\"stop").count(),
            tree.total() - under,
            "the subtree should be off the page"
        );
        assert_eq!(
            page.matches("class=\"station ").count(),
            (tree.total() - under) * 2,
            "and off both gutters, or a rail points at the wrong row"
        );
        assert!(
            page.contains(&format!("<span class=\"held\">+{under}</span>")),
            "{page}"
        );
    }

    /// Only the node doing the folding offers to unfold. Every ancestor of
    /// a folded node also has descendants off the page -- 198 of `g1`'s,
    /// when `g132` is folded -- and the first version read that number off
    /// each of them, so `g1` carried a control that said "+198" and folded
    /// `g1` when it was clicked.
    #[test]
    fn only_the_folded_node_offers_to_unfold() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let alias = biggest_hub(&tree, &ag);

        let page = map_page("vivac", "vivac", &tree, &format!("fold={alias}"));
        assert_eq!(
            page.matches("class=\"fold on\"").count(),
            2,
            "one node is folded, so one control per gutter says so:\n{page}"
        );
        assert_eq!(
            page.matches("class=\"stop hub folded\"").count()
                + page.matches("class=\"stop folded\"").count(),
            1,
            "and one row is marked folded:\n{page}"
        );
    }

    /// The ruler is one number and not a pile. Exactly one mark is on,
    /// whatever was clicked before, and clicking the marked one gives the
    /// whole tree back.
    ///
    /// The first version added every node at a depth to the fold set, so
    /// folding 6, then 5, then 3 left three sets that had to be undone one
    /// at a time in reverse -- which is what the owner met.
    #[test]
    fn the_depth_ruler_is_one_number_and_not_a_pile() {
        let tree = real_shape();
        let deep = levels(&tree);
        assert!(deep > 3, "the fixture is deep enough to test this");

        let page = map_page("vivac", "vivac", &tree, "");
        assert_eq!(page.matches("class=\"head\"").count(), deep);
        assert_eq!(page.matches("class=\"head on\"").count(), 0);

        // Three levels, then two: the second click decides on its own.
        let page = map_page("vivac", "vivac", &tree, "depth=3");
        assert_eq!(page.matches("class=\"head on\"").count(), 1, "{page}");
        let three = page.matches("<li class=\"stop").count();

        let page = map_page("vivac", "vivac", &tree, "depth=3&depth=2");
        assert_eq!(
            page.matches("<li class=\"stop").count(),
            map_page("vivac", "vivac", &tree, "depth=2")
                .matches("<li class=\"stop")
                .count(),
            "the last number wins, and nothing accumulates"
        );
        assert!(
            page.matches("<li class=\"stop").count() < three,
            "two levels draw fewer rows than three"
        );
    }

    /// A depth shows that many levels, and one is the roots.
    #[test]
    fn a_depth_of_one_draws_the_roots_and_nothing_else() {
        let tree = real_shape();
        let page = map_page("vivac", "vivac", &tree, "depth=1");
        assert_eq!(
            page.matches("<li class=\"stop").count(),
            tree.roots().len(),
            "{page}"
        );
        // And the way back is right there, because it always is.
        assert!(page.contains("class=\"tool\" href=\"?\""), "{page}");
    }

    /// A row the depth limit is cutting has no offer to make: one node
    /// cannot come back from a limit set for the whole map. It says so
    /// rather than offering a click that does nothing.
    #[test]
    fn a_row_the_depth_cuts_says_so_instead_of_offering_to_fold() {
        let tree = real_shape();
        let page = map_page("vivac", "vivac", &tree, "depth=2");
        assert!(page.contains("class=\"fold off\""), "{page}");
        assert!(
            page.contains("is at the last level the depth is set to"),
            "{page}"
        );
    }

    /// Whatever is folded, the whole tree is one click away. The way back
    /// used to live inside the depth chips, and the wide layout hides those,
    /// so on a desktop a folded page had no visible way out at all.
    #[test]
    fn a_folded_page_always_shows_the_way_back() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let alias = biggest_hub(&tree, &ag);
        let under = ag
            .counts(tree.resolve(&alias).expect("the hub resolves").num)
            .total;

        let whole = map_page("vivac", "vivac", &tree, "");
        assert!(
            !whole.contains("class=\"tool\" href=\"?\""),
            "nothing is folded, so there is nothing to come back from"
        );

        let folded = map_page("vivac", "vivac", &tree, &format!("fold={alias}"));
        assert!(folded.contains("class=\"tool\" href=\"?\""), "{folded}");
        assert!(
            folded.contains(&format!("Unfold everything · {under}")),
            "and it says how much it brings back:\n{folded}"
        );
    }

    /// A line is a property of the work and not of the view. Fold a branch
    /// away and the eight colours have to stay where they were, or the
    /// legend means something different on every page.
    #[test]
    fn folding_does_not_change_which_lines_have_a_colour() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let alias = biggest_hub(&tree, &ag);

        let whole = Map::of(&tree, &ag, &Fold::default());
        let folded = Map::of(&tree, &ag, &Fold::parse(&format!("fold={alias}"), &tree));

        let names =
            |m: &Map| -> Vec<String> { m.lines.named.iter().map(|&l| m.line_name(l)).collect() };
        assert_eq!(names(&whole), names(&folded), "the legend must not move");

        // And a stop that is still drawn keeps the colour it had.
        for s in &folded.stops {
            let before = whole.index[&s.node.num];
            assert_eq!(
                folded.colour(s.line),
                whole.colour(whole.stops[before].line),
                "{} changed colour when something else was folded",
                s.node.alias()
            );
        }
    }

    /// Folding is a way of looking at the tree, so it is not allowed to
    /// change what the drawing says about how branched the work is. The
    /// radius of a folded station is still its real fan-out, and so is the
    /// number on its row.
    #[test]
    fn a_folded_node_still_says_how_branched_it_is() {
        let tree = real_shape();
        let ag = tree.aggregates();
        let alias = biggest_hub(&tree, &ag);
        let fan = tree
            .children(tree.resolve(&alias).expect("the hub resolves").num)
            .len();

        let page = map_page("vivac", "vivac", &tree, &format!("fold={alias}"));
        assert!(
            page.contains(&format!("<span class=\"fan\">{fan}</span>")),
            "{page}"
        );
        assert!(
            page.contains("r=\"9\""),
            "the folded hub keeps the largest radius:\n{page}"
        );
    }

    /// Nothing a reader types into the address bar reaches the page. An
    /// alias that resolves to no node is dropped, which is the same answer
    /// the router gives a path it does not know.
    #[test]
    fn a_fold_that_names_nothing_folds_nothing_and_echoes_nothing() {
        let tree = real_shape();
        let page = map_page("vivac", "vivac", &tree, "fold=not-a-node,%2e%2e&x=1");
        assert_eq!(page.matches("<li class=\"stop").count(), tree.total());
        assert!(!page.contains("not-a-node"), "{page}");
        assert!(!page.contains("%2e"), "{page}");
    }

    /// A blocker is always a descendant, so folding a node hides its own
    /// debts. The count has to survive that: a panel that listed only the
    /// survivors would report a node as waiting on nothing while it waits.
    #[test]
    fn folding_hides_a_blocker_without_hiding_that_it_is_waiting() {
        let mut tree = Tree::default();
        let (mut seq, mut num) = (0u64, 0u64);
        let root = fixture_node(&mut tree, &mut seq, &mut num, None);
        seq += 1;
        num += 1;
        tree.apply(
            seq,
            "2026-09-09T10:00:00Z",
            &Body::NodeCreated {
                node: "n2".to_string(),
                num,
                kind: Kind::Question,
                title: "the blocker".to_string(),
                why: "fixture".to_string(),
                parent: Some(root.clone()),
                blocks: true,
                refs: vec![],
                governs: vec![],
                arms: vec![],
            },
        );
        tree.sort_nodes();

        let whole = map_page("vivac", "vivac", &tree, "");
        assert!(whole.contains("\"bn\":1"), "{whole}");
        assert!(whole.contains("\"bl\":[1]"), "{whole}");

        let folded = map_page("vivac", "vivac", &tree, "fold=g1");
        assert!(
            folded.contains("\"bn\":1"),
            "still waiting on one:\n{folded}"
        );
        assert!(
            folded.contains("\"bl\":[]"),
            "and none of them has a row to link to:\n{folded}"
        );
    }

    /// The control has to weigh the same as the tree it is a control for.
    #[test]
    fn even_shape_has_the_same_total_as_real_shape() {
        assert_eq!(even_shape().total(), real_shape().total());
        assert_eq!(max_depth(&even_shape()), 6, "max depth");
    }

    /// `WEB.md` §7.7: the acceptance harness for the promise `t191` made and
    /// `d391` inherited -- that the drawing itself carries the fan-out, with
    /// no number and no caption stating the conclusion.
    ///
    /// This test cannot decide whether the drawing works. It renders both
    /// shapes with the same function and writes them side by side so a
    /// person can look. **The verdict is the owner's, per §7.7.**
    #[test]
    fn the_real_shape_does_not_render_like_an_even_tree() {
        let real = map_page("vivac", "vivac", &real_shape(), "");
        let even = map_page("vivac", "vivac", &even_shape(), "");

        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/shape");
        std::fs::create_dir_all(&dir).expect("target/shape can be created");
        let real_path = dir.join("real.html");
        let even_path = dir.join("even.html");
        std::fs::write(&real_path, &real).expect("real.html can be written");
        std::fs::write(&even_path, &even).expect("even.html can be written");

        eprintln!("real shape:  {}", real_path.display());
        eprintln!("even shape:  {}", even_path.display());
    }
}
