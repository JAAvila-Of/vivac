use super::escape;
use crate::event::Body;
use crate::model::Tree;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const PER_NODE: usize = 20;
const TOTAL: usize = 2_000;

#[derive(Clone, Serialize)]
pub(super) struct Link<'a> {
    pub artifact: &'a str,
    pub revision: &'a str,
    pub source: &'a str,
}

#[derive(Default, Serialize)]
pub(super) struct Links<'a> {
    pub produced: Vec<Link<'a>>,
    pub referenced: Vec<Link<'a>>,
    pub count: usize,
    pub truncated: bool,
}

pub(super) fn project(tree: &Tree) -> HashMap<u64, Links<'_>> {
    let mut out = HashMap::new();
    if !tree
        .artifact_projection
        .is_valid(tree.artifact_history.len())
    {
        return out;
    }
    let mut revisions = HashMap::new();
    let mut latest = HashMap::new();
    for (_, body) in &tree.artifact_history {
        if let Body::ArtifactCaptured {
            artifact,
            revision,
            producer,
            source,
            ..
        }
        | Body::ArtifactBundleCaptured {
            artifact,
            revision,
            producer,
            source,
            ..
        } = body
        {
            let link = Link {
                artifact,
                revision,
                source,
            };
            revisions.insert(revision.as_str(), link.clone());
            latest.insert((producer.as_str(), artifact.as_str()), link);
        }
    }
    let mut produced: HashMap<&str, Vec<Link<'_>>> = HashMap::new();
    for ((producer, _), link) in latest {
        produced.entry(producer).or_default().push(link);
    }
    let mut total = 0;
    for node in tree.nodes_iter() {
        let mut direct = produced.remove(node.id.as_str()).unwrap_or_default();
        direct.sort_by_key(|link| (link.artifact, link.revision));
        let mut referenced = Vec::new();
        let mut seen = HashSet::new();
        for reference in node.refs(tree) {
            let Some((artifact, revision)) = reference
                .strip_prefix("artifact:")
                .and_then(|s| s.split_once('@'))
            else {
                continue;
            };
            if let Some(link) = revisions
                .get(revision)
                .filter(|link| link.artifact == artifact)
            {
                if seen.insert((artifact, revision)) {
                    referenced.push(link.clone());
                }
            }
        }
        let count = direct
            .iter()
            .chain(&referenced)
            .map(|link| link.artifact)
            .collect::<HashSet<_>>()
            .len();
        if count == 0 {
            continue;
        }
        let original = direct.len() + referenced.len();
        let limit = PER_NODE.min(TOTAL.saturating_sub(total));
        direct.truncate(limit);
        referenced.truncate(limit.saturating_sub(direct.len()));
        total += direct.len() + referenced.len();
        out.insert(
            node.num,
            Links {
                count,
                truncated: original > direct.len() + referenced.len(),
                produced: direct,
                referenced,
            },
        );
    }
    out
}

pub(super) fn badge(count: usize, artifact: bool) -> String {
    if count == 0 {
        return String::new();
    }
    let word = if artifact { "artifact" } else { "note" };
    let label = format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    format!("<span class=\"metadata-badge {word}-badge\" role=\"img\" aria-label=\"{label}\" title=\"{label}\">{}{count}</span>", icon(artifact))
}

pub(super) fn icon(artifact: bool) -> &'static str {
    if artifact {
        "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\" focusable=\"false\"><path d=\"M3 1h6l4 4v10H3z M9 1v4h4\"/></svg>"
    } else {
        "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\" focusable=\"false\"><path d=\"M3 1h10v14H3z M5 5h6 M5 8h6 M5 11h4\"/></svg>"
    }
}

pub(super) fn html(project: &str, links: &Links<'_>) -> String {
    let mut out = String::new();
    for (heading, items) in [
        ("Produced here", &links.produced),
        ("Referenced here", &links.referenced),
    ] {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("<h3>{heading}</h3><ul class=\"artifact-links\">"));
        for item in items {
            out.push_str(&format!("<li><a href=\"/p/{}/artifacts#artifact={}&amp;revision={}\">{} - revision {}</a></li>", escape(project), escape(item.artifact), escape(item.revision), escape(item.source), escape(item.revision)));
        }
        out.push_str("</ul>");
    }
    if links.truncated {
        out.push_str(&format!("<p>{} deliveries. This list is limited. <a href=\"/p/{}/artifacts\">See all project artifacts</a>.</p>", links.count, escape(project)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Kind;

    fn id(n: u32) -> String {
        format!("{n:026}")
    }
    fn node(tree: &mut Tree, num: u64, refs: Vec<String>) {
        tree.apply(
            num,
            "2026-10-13T00:00:00Z",
            "main",
            &Body::NodeCreated {
                node: id(num as u32),
                num,
                kind: Kind::Task,
                title: "Fixture".into(),
                why: "Test".into(),
                parent: None,
                blocks: false,
                refs,
                governs: vec![],
                arms: vec![],
                against: None,
            },
        );
    }
    fn receipt(tree: &mut Tree, revision: u32, previous: Option<String>) {
        receipt_at(tree, revision, previous, 1);
    }

    fn receipt_at(tree: &mut Tree, revision: u32, previous: Option<String>, producer: u32) {
        tree.apply(
            revision as u64,
            "2026-10-13T00:00:00Z",
            "main",
            &Body::ArtifactCaptured {
                artifact: id(90),
                revision: id(revision),
                operation: id(revision + 200),
                producer: id(producer),
                previous,
                source: "mockup<&\".html".into(),
                site: id(500),
                size: 17,
                digest: "a".repeat(64),
                object: format!("objects/{}.blob", id(revision)),
            },
        );
    }
    #[test]
    fn bundle_receipts_keep_exact_revision_links_on_producer_and_reference() {
        let mut tree = Tree::default();
        node(&mut tree, 1, vec![]);
        node(
            &mut tree,
            2,
            vec![format!("artifact:{}@{}", id(90), id(102))],
        );
        for (revision, previous) in [(102, None), (103, Some(id(102)))] {
            let body: Body = serde_json::from_value(serde_json::json!({
                "type": "artifact.bundle_captured", "artifact": id(90),
                "revision": id(revision), "operation": id(revision + 200),
                "producer": id(1), "previous": previous, "source": "delivery-root",
                "selection": "selection.json", "site": id(500), "size": 170,
                "digest": "a".repeat(64), "object": format!("objects/{}.bundle", id(revision)),
                "entrypoint": "index.html", "member_count": 3, "manifest_bytes": 100
            }))
            .unwrap();
            tree.apply(revision.into(), "2026-10-14T00:00:00Z", "main", &body);
        }
        let links = project(&tree);
        assert_eq!(links[&1].produced[0].revision, id(103));
        assert_eq!(links[&2].referenced[0].revision, id(102));
        assert_eq!(links[&2].count, 1);
    }

    #[test]
    fn exact_references_do_not_follow_later_revisions_or_count_twice() {
        let mut tree = Tree::default();
        let reference = format!("artifact:{}@{}", id(90), id(102));
        node(&mut tree, 1, vec![reference.clone(), reference.clone()]);
        node(&mut tree, 2, vec![reference]);
        receipt(&mut tree, 102, None);
        receipt(&mut tree, 103, Some(id(102)));
        let links = project(&tree);
        assert_eq!(links[&1].count, 1);
        assert_eq!(links[&1].produced[0].revision, id(103));
        assert_eq!(links[&1].referenced.len(), 1);
        assert_eq!(links[&1].referenced[0].revision, id(102));
        assert!(links[&2].produced.is_empty());
        assert_eq!(links[&2].referenced[0].revision, id(102));
        let html = html("fixture", &links[&1]);
        assert!(html.contains("mockup&lt;&amp;&quot;.html"));
        assert!(!html.contains("mockup<&"));
        let why = super::super::why::why_page("fixture", "Fixture", &tree, &[], "t1").unwrap();
        assert!(why.contains("Produced here"));
        assert!(why.contains(&format!("revision={}", id(102))));
        assert!(why.contains(&format!("revision={}", id(103))));
    }
    #[test]
    fn a_later_revision_from_another_producer_keeps_the_original_provenance() {
        let mut tree = Tree::default();
        node(
            &mut tree,
            1,
            vec![format!("artifact:{}@{}", id(90), id(102))],
        );
        node(&mut tree, 2, vec![]);
        receipt(&mut tree, 102, None);
        receipt_at(&mut tree, 103, Some(id(102)), 2);
        let links = project(&tree);
        assert_eq!(links[&1].count, 1);
        assert_eq!(links[&1].produced[0].revision, id(102));
        assert_eq!(links[&1].referenced[0].revision, id(102));
        assert_eq!(links[&2].produced[0].revision, id(103));
        assert!(links[&2].referenced.is_empty());
        let html = html("fixture", &links[&1]);
        assert!(!html.contains(&format!("revision={}", id(103))));
    }
    #[test]
    fn map_rows_show_single_counts_without_empty_badge_groups() {
        let mut tree = Tree::default();
        node(&mut tree, 1, vec![]);
        node(&mut tree, 2, vec![]);
        tree.apply(
            3,
            "2026-10-13T00:00:00Z",
            "main",
            &Body::NodeNoted {
                node: id(1),
                note: "A note".into(),
            },
        );
        receipt(&mut tree, 102, None);
        let page = super::super::map::map_page("fixture", "Fixture", &tree, "");
        let first = page
            .split("id=\"t1\" data-stop=")
            .nth(1)
            .unwrap()
            .split("</li>")
            .next()
            .unwrap();
        assert!(first.contains("aria-label=\"1 note\""));
        assert!(first.contains("aria-label=\"1 artifact\""));
        assert!(!first.contains(">1 note<"));
        let second = page
            .split("id=\"t2\" data-stop=")
            .nth(1)
            .unwrap()
            .split("</li>")
            .next()
            .unwrap();
        assert!(!second.contains("metadata-badges"));
    }
    #[test]
    fn bounded_lists_keep_exact_counts_and_an_inventory_escape() {
        let mut tree = Tree::default();
        for n in 1..=101 {
            node(&mut tree, n, vec![]);
        }
        for producer in 1..=101 {
            for artifact in 1..=21 {
                let revision = 1_000 + producer * 21 + artifact;
                let body = Body::ArtifactCaptured {
                    artifact: id(revision),
                    revision: id(revision + 10_000),
                    operation: id(revision + 20_000),
                    producer: id(producer),
                    previous: None,
                    source: "mockup.html".into(),
                    site: id(500),
                    size: 17,
                    digest: "a".repeat(64),
                    object: format!("objects/{}.blob", id(revision + 10_000)),
                };
                tree.apply(revision.into(), "2026-10-13T00:00:00Z", "main", &body);
            }
        }
        let links = project(&tree);
        assert!(links
            .values()
            .all(|links| links.count == 21 && links.truncated));
        assert_eq!(
            links
                .values()
                .map(|links| links.produced.len() + links.referenced.len())
                .sum::<usize>(),
            TOTAL
        );
        assert!(links.values().all(|links| links.produced.len() <= PER_NODE));
        assert!(html("fixture", &links[&101]).contains("/p/fixture/artifacts"));
    }
    #[test]
    fn one_note_is_visible_with_an_accessible_icon_and_no_visible_word() {
        let markup = badge(1, false);
        assert!(markup.contains("role=\"img\""));
        assert!(markup.contains("aria-label=\"1 note\""));
        assert!(markup.contains("aria-hidden=\"true\""));
        assert!(markup.ends_with("</svg>1</span>"));
        assert!(!markup.contains(">1 note<"));
        assert!(badge(0, true).is_empty());
    }
}
