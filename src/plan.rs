//! A plan a person is asked to confirm, shared by `setup`, `init`, `codex`
//! and `update` (`d792`, `t789`, `d653`, `d815`).
//!
//! A plan is data first -- one [`PlanItem`] per row, its verb and its "what"
//! already apart -- and rendered second, so every column's width comes from
//! the items actually in this plan rather than a fixed guess (`t565` §7.8's
//! old 41 named no file at all once a label like `.vivac/config` grew past
//! it). `setup/claude_code.rs` wrote this first; `setup/codex.rs` and
//! `setup/tree.rs` built the same `PlanItem`s and rendered them through the
//! same two functions rather than fixing the columns a second time
//! (`d653`); `update.rs` reuses it in turn for its own plan, cargo install
//! or release archive, rather than hard-wrapping prose the way `d792`
//! already rejected (`d815`).

use crate::style::{self, Stream};

/// One row of a plan: a verb, the path or label it acts on, what it does
/// in plain words, and the value lines (a hook's command, a tree's own
/// path) that sit under it, dim and indented to the path column.
pub(crate) struct PlanItem {
    pub(crate) verb: &'static str,
    pub(crate) path: String,
    pub(crate) what: String,
    pub(crate) sub: Vec<(String, String)>,
}

impl PlanItem {
    pub(crate) fn new(
        verb: &'static str,
        path: impl Into<String>,
        what: impl Into<String>,
    ) -> PlanItem {
        PlanItem {
            verb,
            path: path.into(),
            what: what.into(),
            sub: Vec::new(),
        }
    }

    pub(crate) fn with_sub(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
    ) -> PlanItem {
        self.sub.push((label.into(), value.into()));
        self
    }
}

/// The line every plan opens with: the command in bold, the folder it acts
/// on dim, never a verb of its own -- each row below names its own.
pub(crate) fn heading(stream: Stream, cmd: &str, here: &std::path::Path) -> String {
    format!(
        "{} will, in {}:\n\n",
        style::bold(stream, cmd),
        style::dim(stream, &here.display().to_string())
    )
}

/// `items`, rendered: every column padded to the widest plain text in that
/// column across the whole plan, two spaces of gap after each, so a run
/// with one long path never drags every other row's own width up with it
/// column by column but *does* keep its own row's columns lined up with
/// the rest. Never wrapped (`d792`): a plan item is one line, and the
/// terminal is what wraps it if it has to.
pub(crate) fn render_items(stream: Stream, items: &[PlanItem]) -> String {
    let verb_width = items.iter().map(|i| i.verb.len()).max().unwrap_or(0);
    let path_width = items.iter().map(|i| i.path.len()).max().unwrap_or(0);
    let sub_label_width = items
        .iter()
        .flat_map(|i| i.sub.iter().map(|(label, _)| label.len()))
        .max()
        .unwrap_or(0);
    let sub_indent = " ".repeat(2 + verb_width + 2);
    let mut s = String::new();
    for item in items {
        s.push_str("  ");
        s.push_str(&style::verb(stream, item.verb));
        s.push_str(&" ".repeat(verb_width - item.verb.len() + 2));
        s.push_str(&style::path(stream, &item.path));
        s.push_str(&" ".repeat(path_width - item.path.len() + 2));
        s.push_str(&item.what);
        s.push('\n');
        for (label, value) in &item.sub {
            let line = format!(
                "{label}{}  {value}",
                " ".repeat(sub_label_width - label.len())
            );
            s.push_str(&sub_indent);
            s.push_str(&style::dim(stream, &line));
            s.push('\n');
        }
    }
    s
}
