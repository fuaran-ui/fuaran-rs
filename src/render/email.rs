//! Email-safe digest projection (the Send Me mechanic) — render a tree to a
//! plain-text digest that survives any mail client: no HTML, no JS, no external
//! references, no interactivity. It is a *reading* projection of the content
//! nodes (headings, prose, metrics, badges, callouts, lists, links), skipping
//! layout scaffolding and interactive controls that carry no static meaning in
//! an inbox. Deterministic and side-effect-free — the same tree always yields
//! the same digest.

use crate::introspect::all_nodes;
use crate::wire::{Node, NodeKind, TextSource};

fn literal(t: &TextSource) -> Option<String> {
    match t {
        TextSource::Literal(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

// The digest line(s) a single node contributes, or empty for structural /
// interactive nodes that carry no static reading content.
fn node_lines(node: &Node) -> Vec<String> {
    match &node.kind {
        NodeKind::Heading(s) => literal(&s.text)
            .map(|t| vec![format!("# {t}")])
            .unwrap_or_default(),
        NodeKind::Markdown(s) => literal(&s.text).map(|t| vec![t]).unwrap_or_default(),
        NodeKind::Metric(s) => {
            // The value is a live binding; a static digest carries the label and
            // marks the value as read at send time.
            literal(&s.label)
                .map(|l| vec![format!("{l}: (live value)")])
                .unwrap_or_default()
        }
        NodeKind::LabelValueRow(s) => literal(&s.label)
            .map(|l| vec![format!("{l}: (live value)")])
            .unwrap_or_default(),
        NodeKind::Badge(s) => literal(&s.label)
            .map(|l| vec![format!("[{l}]")])
            .unwrap_or_default(),
        NodeKind::Callout(s) => {
            let mut out = Vec::new();
            if let Some(h) = s.heading.as_ref().and_then(literal) {
                out.push(format!("! {h}"));
            }
            if let Some(b) = literal(&s.body) {
                out.push(format!("  {b}"));
            }
            out
        }
        NodeKind::List(s) => s
            .items
            .iter()
            .filter_map(literal)
            .map(|it| format!("- {it}"))
            .collect(),
        NodeKind::Link(s) => {
            // A link's label carries the meaning; the href is inert text.
            literal(&s.label)
                .map(|l| vec![format!("{l}")])
                .unwrap_or_default()
        }
        NodeKind::CodeBlock(s) if !s.code.trim().is_empty() => {
            vec![format!("```\n{}\n```", s.code)]
        }
        _ => Vec::new(),
    }
}

/// Render a tree to a plain-text, email-safe digest — one block per content
/// node, in document order, joined by blank lines. Interactive and structural
/// nodes contribute nothing; the result is pure text (no injection surface).
pub fn email_digest(tree: &Node) -> String {
    let blocks: Vec<String> = all_nodes(tree).into_iter().flat_map(node_lines).collect();
    blocks.join("\n")
}
