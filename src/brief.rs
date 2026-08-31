//! The `brief`: a deterministic render bounded in tokens.
//!
//! `BRIEF-SPEC.md`. It answers three questions in order of importance: where
//! we are and how we got here, what governs this point, and **what is out of
//! scope right now**. The third is the one no other tool emits: every memory
//! tool dumps what is relevant, and the problem in agentic development is the
//! opposite one, bounding.
//!
//! Two rules override everything else:
//!
//! - **Same log + same `--now` + same anchor state -> same bytes.** Without
//!   `--now` determinism would be impossible, because ages are relative to
//!   the moment.
//! - **The spine is never truncated.** If it does not fit, the budget is
//!   wrong and it says so, but it comes out whole: it is the answer to
//!   question 1, and without it the brief has no reason to exist.

use crate::anchor::Anchor;
use crate::args::Args;
use crate::event::{State, Kind};
use crate::failure::R;
use crate::model::{Tree, Node};

const PRESUPUESTO: usize = 1500;
/// The whole brief is pure ASCII.
///
/// `BRIEF-SPEC.md` §7 draws the spine with box-drawing characters, but the DX
/// pillar demands it degrade without breaking "in cmd.exe as well as Windows
/// Terminal", and there any code page that is not UTF-8 turns them into
/// garbage. What is normative in §7 are the markers --that the focus be
/// visible, that a flag carry its reason, that an empty section not show--
const RULE: &str = "------------------------------------------------------------";

/// One section of the brief. The vector order is the one in §3, which is both
/// render order and priority order: truncation starts from the bottom.
struct Section {
    lines: Vec<String>,
    truncable: bool,
}

impl Section {
    fn fixed(lines: Vec<String>) -> Section {
        Section {
            lines,
            truncable: false,
        }
    }
    fn loose(lines: Vec<String>) -> Section {
        Section {
            lines,
            truncable: true,
        }
    }
}

/// Token estimator. It is an estimate and the ceiling is indicative: what
/// matters is that it be **deterministic**, so two runs of the same log
/// truncate the same way.
fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn tokens_of(secciones: &[Section]) -> usize {
    secciones
        .iter()
        .flat_map(|s| s.lines.iter())
        .map(|l| tokens(l) + 1)
        .sum()
}

/// Truncates a list keeping the first `n`. An item from the middle is never
/// dropped in silence.
fn trim_list(mut v: Vec<String>, n: usize, which: &str) -> Vec<String> {
    if v.len() > n {
        let left_over = v.len() - n;
        v.truncate(n);
        v.push(format!("      ... and {left_over} more (vivac {which})"));
    }
    v
}

fn heading(title: &str, cuerpo: Vec<String>) -> Vec<String> {
    // Empty sections are omitted whole, heading included: a brief with nothing
    // parked does not say "DO NOT TOUCH NOW: (empty)".
    if cuerpo.is_empty() {
        return vec![];
    }
    let mut v = vec![String::new(), format!(" {title}")];
    v.extend(cuerpo);
    v
}

/// Constraints that govern the path.
///
/// **By `spawns` only.** Inheriting through `depends_on` as well would turn
/// the computation from O(depth) into O(graph), and would lose the property
/// that inheritance is legible by looking at the stack on screen.
fn constraints<'a>(a: &'a Tree, camino: &[&Node]) -> Vec<&'a Node> {
    let en_camino: std::collections::HashSet<&str> = camino.iter().map(|n| n.id.as_str()).collect();
    let mut v: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Constraint && n.state.is_open())
        .filter(|n| {
            // Project-wide (hangs off a root), or reachable from the path.
            let del_proyecto = n
                .parent
                .as_ref()
                .and_then(|p| a.node(p))
                .is_some_and(|p| p.parent.is_none());
            del_proyecto
                || a.ancestors(&n.id)
                    .iter()
                    .any(|p| en_camino.contains(p.id.as_str()))
        })
        .collect();
    // At risk first --the ones carrying a flag-- and then by alias.
    v.sort_by_key(|n| (n.flags.is_empty(), n.num));
    v
}

fn spine(camino: &[&Node]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, n) in camino.iter().enumerate() {
        let primero = i == 0;
        let is_last = i == camino.len() - 1;
        // Continuation: the trunk carries on while anything is left below.
        let cont = if is_last { "        " } else { "  |     " };

        let branch = if primero {
            " GOAL ".to_string()
        } else if is_last {
            "  `-- ".to_string()
        } else {
            "  |-- ".to_string()
        };
        let flags: Vec<&str> = n.flags.keys().map(|b| b.word()).collect();
        let flag = if flags.is_empty() {
            String::new()
        } else {
            format!("  ! {}", flags.join(" "))
        };
        let here_mark = if is_last { "   <== HERE" } else { "" };
        v.push(format!(
            "{branch}{:<6} {}{flag}{here_mark}",
            n.alias(),
            clip(&n.title, 44)
        ));
        if !primero && !n.why.is_empty() {
            v.push(format!("{cont}why: {}", clip(&n.why, 52)));
        }
        if !n.governs.is_empty() {
            v.push(format!("{cont}governs: {}", n.governs.join(" ")));
        }
        if !is_last {
            v.push("  |".to_string());
        }
    }
    v
}

/// Cuts on a word boundary without exceeding `n`, **counting the ellipsis**.
/// Budgeting for it matters: otherwise the cut overruns on exactly the
/// tightest lines of the brief, which are the ones being truncated.
pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let t: String = s.chars().take(n.saturating_sub(3)).collect();
    match t.rsplit_once(' ') {
        Some((a, _)) if !a.is_empty() => format!("{a}..."),
        _ => format!("{t}..."),
    }
}

pub fn brief(a: &Tree, anchor_of: &dyn Anchor, args: &Args, project: &str) -> R {
    print!("{}", to_text(a, anchor_of, args, project)?);
    Ok(())
}

/// The brief as text. `session start --hook` needs it whole to put in the
/// envelope: whatever falls outside the envelope, the agent never sees.
pub fn to_text(
    a: &Tree,
    anchor_of: &dyn Anchor,
    args: &Args,
    project: &str,
) -> Result<String, crate::failure::Failure> {
    let hoy = args.opt("now").unwrap_or("").to_string();
    let hoy = if hoy.is_empty() {
        crate::clock::now_rfc3339()
    } else {
        hoy
    };
    let date = crate::clock::date_of(&hoy).to_string();
    let budget: usize = args
        .opt("budget")
        .and_then(|s| s.parse().ok())
        .unwrap_or(PRESUPUESTO);

    let camino: Vec<&Node> = match a.stack.last() {
        Some(id) => a.ancestors(id),
        None => vec![],
    };

    if camino.is_empty() {
        return no_focus(a, project, &date);
    }
    let focus = camino[camino.len() - 1];

    let mut s: Vec<Section> = Vec::new();

    // 1. Header. 2. Spine, which is never truncated.
    s.push(Section::fixed(vec![
        format!("vivac · project: {project} · lane: main · {date}"),
        RULE.to_string(),
        String::new(),
    ]));
    s.push(Section::fixed(spine(&camino)));

    // 3. Focus: what hangs off it unclosed. Standing decisions do not go in
    //    --they are not pending work and they have their own section (8)--,
    //    and whatever hangs further down is counted without being listed.
    let mut children: Vec<String> = a
        .children(&focus.id)
        .into_iter()
        .filter(|c| c.is_front())
        .map(|c| {
            format!(
                "  {} {:<6} {}",
                if c.blocks { '*' } else { ' ' },
                c.alias(),
                c.title
            )
        })
        .collect();
    // Closing a parent cannot make its open children invisible. They are
    // counted and the place to look is named; listing them here would drag in
    // the whole tree, which is exactly the noise the focus exists to keep
    // out.
    let directos: std::collections::HashSet<&str> =
        a.children(&focus.id).iter().map(|c| c.id.as_str()).collect();
    let deep = a
        .descendants(&focus.id)
        .into_iter()
        .filter(|n| n.is_front() && !directos.contains(n.id.as_str()))
        .filter(|n| !a.children(&n.id).iter().any(|c| c.is_front()))
        .count();
    if deep > 0 {
        children.push(format!(
            "    + {deep} further down, outside this level   vivac open"
        ));
    }
    s.push(Section::fixed(heading("BORN FROM HERE", children)));

    // 4. Invariants.
    let invariants: Vec<String> = constraints(a, &camino)
        .iter()
        .map(|c| {
            let riesgo = if c.flags.is_empty() {
                ""
            } else {
                "   AT RISK"
            };
            format!("  {:<6} {}{riesgo}", c.alias(), c.title)
        })
        .collect();
    s.push(Section::fixed(heading("INVARIANTS", invariants)));

    // 5. Blocking questions: all of them, untruncated.
    let en_camino: std::collections::HashSet<&str> = camino.iter().map(|n| n.id.as_str()).collect();
    let questions: Vec<String> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Question && n.state.is_open() && n.blocks)
        .filter(|n| {
            a.ancestors(&n.id)
                .iter()
                .any(|p| en_camino.contains(p.id.as_str()))
        })
        .map(|n| format!("  {:<6} {}", n.alias(), n.title))
        .collect();
    let mut questions = questions;
    questions.sort();
    s.push(Section::fixed(heading("BLOCKS", questions)));

    // 6. Flags on the path, or one hop off it.
    let mut marcados: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| !n.flags.is_empty())
        .filter(|n| {
            en_camino.contains(n.id.as_str())
                || n.parent
                    .as_ref()
                    .is_some_and(|p| en_camino.contains(p.as_str()))
        })
        .collect();
    marcados.sort_by_key(|n| n.num);
    let flag_lines: Vec<String> = marcados
        .iter()
        .flat_map(|n| {
            n.flags.iter().map(move |(b, reason)| {
                format!(
                    "  {:<6} {:<10} {}",
                    n.alias(),
                    b.word(),
                    clip(reason, 44)
                )
            })
        })
        .collect();
    s.push(Section::loose(heading(
        "FLAGGED",
        trim_list(flag_lines, 3, "stats"),
    )));

    // 7. Out of scope. **This is the product's differentiator**, and it only
    // has content if `park` costs the same as `pop`.
    let mut parked_nodes: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state == State::Suspended)
        .filter(|n| {
            a.ancestors(&n.id)
                .iter()
                .rev()
                .skip(1)
                .any(|p| en_camino.contains(p.id.as_str()))
        })
        .collect();
    parked_nodes.sort_by_key(|n| n.num);
    let fuera: Vec<String> = parked_nodes
        .iter()
        .flat_map(|n| {
            let colgado = n
                .parent
                .as_ref()
                .and_then(|p| a.node(p))
                .map(|p| format!("hangs off {}", p.alias()))
                .unwrap_or_default();
            let mut v = vec![format!(
                "  {:<6} {:<40} {colgado}",
                n.alias(),
                clip(&n.title, 40)
            )];
            if !n.outcome.is_empty() {
                v.push(format!("         \"{}\"", clip(&n.outcome, 56)));
            }
            v
        })
        .collect();
    s.push(Section::loose(heading(
        "DO NOT TOUCH NOW",
        trim_list(fuera, 6, "parked"),
    )));

    // 8. Standing decisions: on the path, or with a `governs` overlapping the
    // focus's own. Superseded ones never appear.
    let mut dec: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.kind == Kind::Decision && n.state.is_open())
        .filter(|n| {
            en_camino.contains(n.id.as_str())
                || n.parent
                    .as_ref()
                    .is_some_and(|p| en_camino.contains(p.as_str()))
                || n.governs
                    .iter()
                    .any(|g| focus.governs.iter().any(|f| crate::glob::covers(g, f)))
        })
        .collect();
    dec.sort_by_key(|n| n.num);
    let decisions: Vec<String> = dec
        .iter()
        .map(|n| format!("  {:<6} {}", n.alias(), clip(&n.title, 52)))
        .collect();
    s.push(Section::loose(heading(
        "STANDING DECISIONS",
        trim_list(decisions, 3, "tree"),
    )));

    // 9. Last vivac. Restoring is always restore + diff: a vivac is never
    // presented without saying what changed since.
    let vv: Vec<String> = match a.last_vivac() {
        None => vec![],
        Some(v) => {
            let mut l = vec![format!(
                "  {} · {} · {}{}",
                v.alias(),
                v.kind.word(),
                crate::clock::date_of(&v.ts),
                if v.anchor.is_empty_tree() {
                    String::new()
                } else {
                    format!(" · {}", v.anchor.corto())
                }
            )];
            if !v.next_intent.is_empty() {
                l.push(format!("         you were about to: {}", clip(&v.next_intent, 52)));
            }
            // With no anchor no diff lines are invented: they are omitted, and
            // the date above stands in, which is the plain age there really is.
            if !v.anchor.is_empty_tree() {
                let changes = anchor_of.changed_since(&v.anchor);
                if !changes.is_empty() {
                    let touching = changes
                        .iter()
                        .filter(|c| v.working_set.iter().any(|g| crate::glob::covers(g, &c.path_arg)))
                        .count();
                    l.push(format!(
                        "         {} changes since, {touching} touching what it governs",
                        changes.len()
                    ));
                }
            }
            l
        }
    };
    s.push(Section::loose(heading("LAST VIVAC", vv)));

    // 10. Freshness.
    let stale_ones: Vec<String> = camino
        .iter()
        .filter(|n| n.flags.contains_key(&crate::event::Flag::Stale))
        .map(|n| format!("  {:<6} {}", n.alias(), n.title))
        .collect();
    s.push(Section::loose(heading("UNTOUCHED FOR A WHILE", stale_ones)));

    emit(s, budget, a)
}

/// Assembles under budget. It is a **soft ceiling**: truncatable sections are
/// dropped from the bottom up until it fits; if it still does not fit, it is
/// emitted anyway with a warning. Going over budget is a sign the tree needs
/// pruning, not that the brief should lie by silent omission.
fn emit(
    mut s: Vec<Section>,
    budget: usize,
    a: &Tree,
) -> Result<String, crate::failure::Failure> {
    let requested = tokens_of(&s);
    while tokens_of(&s) > budget {
        match s.iter().rposition(|x| x.truncable && !x.lines.is_empty()) {
            Some(i) => s[i].lines.clear(),
            None => break,
        }
    }
    let spent = tokens_of(&s);

    let mut o = String::new();
    for l in s.iter().flat_map(|x| x.lines.iter()) {
        o.push_str(l);
        o.push('\n');
    }
    let parked_nodes = a.nodes_iter().filter(|n| n.state == State::Suspended).count();
    o.push_str(&format!(
        "
{RULE}
 {spent} tokens · depth {} · {parked_nodes} parked
",
        a.stack_depth()
    ));
    if spent > budget {
        o.push_str(&format!(
            "
 ! the brief is over budget ({spent}/{budget}).
   The spine is never truncated: what is left over is tree, not render.
   What can be pruned:  vivac triage
"
        ));
    } else if requested > budget {
        o.push_str(&format!(
            "
 ! {} tokens trimmed to fit in {budget}.
",
            requested - spent
        ));
    }
    Ok(o)
}

/// Empty stack. **It never comes out empty**: it shows the open goals and one
/// concrete action.
fn no_focus(a: &Tree, project: &str, date: &str) -> Result<String, crate::failure::Failure> {
    let mut o = format!(
        "vivac · project: {project} · lane: main · {date}
{RULE}

 No active focus.
"
    );
    let mut goals: Vec<&Node> = a
        .nodes_iter()
        .filter(|n| n.state.is_open() && (n.kind == Kind::Goal || n.parent.is_none()))
        .collect();
    goals.sort_by_key(|n| n.num);
    if !goals.is_empty() {
        o.push_str(
            "
 OPEN GOALS
",
        );
        for m in goals {
            o.push_str(&format!(
                "  {:<6} {:<40} {} open below
",
                m.alias(),
                clip(&m.title, 40),
                a.counts(&m.id).open_count
            ));
        }
    }
    o.push('\n');
    if a.is_empty_tree() {
        o.push_str(
            " Start with:  vivac push \"<title>\" --why \"<reason>\"
",
        );
    } else {
        o.push_str(
            " Pick up with:  vivac focus <id>
",
        );
        o.push_str(
            " Or open another:  vivac push \"<title>\" --why \"<reason>\"
",
        );
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimator_is_deterministic() {
        assert_eq!(tokens("same"), 1);
        assert_eq!(tokens("same tokens"), 3);
        assert_eq!(tokens(""), 0);
        // Same text, same number, always.
        assert_eq!(tokens("abcdefgh"), tokens("12345678"));
    }

    #[test]
    fn trimming_says_what_is_missing() {
        let v: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let r = trim_list(v, 3, "parked");
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], "l0");
        assert!(r[3].contains("7 more"), "{}", r[3]);
    }

    #[test]
    fn an_empty_section_leaves_no_heading() {
        assert!(heading("DO NOT TOUCH NOW", vec![]).is_empty());
        assert_eq!(heading("X", vec!["  a".into()]).len(), 3);
    }

    #[test]
    fn clipping_respects_words() {
        assert_eq!(clip("hello world", 20), "hello world");
        assert!(clip("a fairly long sentence that does not fit", 20).ends_with("..."));
        assert!(
            clip("a fairly long sentence that does not fit", 20)
                .chars()
                .count()
                <= 20
        );
    }
}
