//! The pure-string server-HTML renderer: walks a typed [`Node`] tree and emits
//! a body-fragment HTML string carrying the reference `fuaran-*` class
//! vocabulary — styled byte-for-byte by the shared reference stylesheet
//! (`css/fuaran.css`, a byte-copy of the reference artefact) and structured to
//! hydrate cleanly against a conformant client renderer.
//!
//! Server semantics (no client runtime, no dispatch):
//! - interactivity renders INERT — a Button is a real `<button>`, dead until
//!   hydration; no event handlers are emitted;
//! - a Link renders a real, sanitised `<a href>` — the crawlable no-JS path;
//! - bindings resolve server-side: `Static` to its value, the store-backed
//!   cases from host-supplied [`BindingSources`], the rest to the loading slot
//!   or the em-dash placeholder;
//! - client-library visualisations (Chart / Map) render a deterministic
//!   labelled placeholder, never a blank;
//! - `Custom` renders the inert labelled placeholder (no registry seam);
//! - closure-bearing slots render their decoded-placeholder behaviour — this
//!   host renders decoded trees, where every closure is already a sentinel.
//!
//! **Islands (partial hydration).** [`render_with_islands`] emits the page
//! statically, wraps each designated island subtree in a
//! `<div data-fuaran-island="<id>">` boundary, and appends one
//! `<script type="application/json" id="fuaran-hydrate-island-<id>">` payload
//! per island carrying that subtree's canonical wire JSON. Zero islands ⇒ no
//! wrapper, no script — byte-identical to [`render_to_html`]. Islands are
//! designated by node id (this host's `ExtraAttributes` hatch is wire-omitted
//! by design, so the marker is an explicit render-call argument).
//!
//! The host owns the document shell (`<html>` / `<head>` / the `<link>` to the
//! reference CSS); this renderer emits the body fragment.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::canonical::JVal;
use crate::wire::{
    Action, Binding, BoxLayout, BoxRole, BoxSpec, CellFormat, CellKindErased, ChartSpec,
    ColumnErased, DisclosureSpec, FilterSpec, FormField, FormFieldKind, FormSpec, GridSpec,
    HeadingVariant, ImageVariant, MapSpec, MathDisplay, ModalSpec, Node, NodeKind, Orientation,
    ScrollAreaSpec, ScrollOrientation, SelectOption, SelectSpec, StateBehaviour, StaticRows,
    StaticValue, TabsSpec, TextSource, ToneVariant, encode_node,
};

use super::bindings::{
    BindingSources, EM_DASH, NumberResolution, ResolvedRows, accessibility_attributes,
    display_number, format_number, render_text, resolve_float_pair, resolve_float_seq,
    resolve_number, resolve_options, resolve_rows, resolve_scalar_number, resolve_string_pair,
    static_display_string, try_bool, try_number, try_scalar_number, try_string,
};
use super::class_names::{icon_size_class, node_class_name, tone_var};
use super::html::{Attr, AttrVal, el, entity_encode, escape_attr, escape_text, text_el, void_el};
use super::markdown::to_html as markdown_to_html;
use super::sanitize::sanitize_url_or_blank;

fn s(v: impl Into<String>) -> AttrVal {
    AttrVal::Str(v.into())
}

/// Per-render context: binding sources + the fragment registry + cycle guard +
/// the island designation set. The expansion set is interior-mutable because
/// the walk is single-threaded and the guard scopes push/pop around each
/// fragment expansion.
struct Ctx<'a> {
    sources: &'a BindingSources,
    fragments: HashMap<String, &'a Node>,
    islands: &'a HashSet<String>,
    expanding: std::cell::RefCell<HashSet<String>>,
}

// ─── Fragment collection + namespacing ───────────────────────────────────────

fn collect_fragments<'a>(acc: &mut HashMap<String, &'a Node>, node: &'a Node) {
    match &node.kind {
        NodeKind::FragmentDecl(spec) => {
            acc.insert(spec.name.clone(), &spec.body);
            collect_fragments(acc, &spec.body);
        }
        NodeKind::Box(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::SplitPanel(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::Tabs(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::Stepper(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::SummaryList(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::Disclosure(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::Modal(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::ScrollArea(spec) => spec.children.iter().for_each(|c| collect_fragments(acc, c)),
        NodeKind::ErrorBoundary(spec) => {
            collect_fragments(acc, &spec.child);
            collect_fragments(acc, &spec.fallback);
        }
        NodeKind::Switch(spec) => {
            spec.cases
                .iter()
                .for_each(|c| collect_fragments(acc, &c.child));
            collect_fragments(acc, &spec.default);
        }
        NodeKind::Heading(_)
        | NodeKind::Markdown(_)
        | NodeKind::Metric(_)
        | NodeKind::Badge(_)
        | NodeKind::Sparkline(_)
        | NodeKind::Callout(_)
        | NodeKind::Progress(_)
        | NodeKind::Skeleton(_)
        | NodeKind::Icon(_)
        | NodeKind::LabelValueRow(_)
        | NodeKind::Fact(_)
        | NodeKind::Link(_)
        | NodeKind::Image(_)
        | NodeKind::List(_)
        | NodeKind::Toast(_)
        | NodeKind::CodeBlock(_)
        | NodeKind::Math(_)
        | NodeKind::Form(_)
        | NodeKind::Filters(_)
        | NodeKind::Button(_)
        | NodeKind::FileUpload(_)
        | NodeKind::Select(_)
        | NodeKind::DataGrid(_)
        | NodeKind::Chart(_)
        | NodeKind::Map(_)
        | NodeKind::Custom(_)
        | NodeKind::FragmentRef(_)
        | NodeKind::Drawing(_)
        | NodeKind::Mount(_) => {}
    }
}

fn namespace_node(prefix: &str, node: &Node) -> Node {
    let mut out = node.clone();
    out.id = format!("{prefix}{}", node.id);
    namespace_kind_in_place(prefix, &mut out.kind);
    out
}

fn namespace_kind_in_place(prefix: &str, kind: &mut NodeKind) {
    let namespace_children = |children: &mut Vec<Node>| {
        for child in children.iter_mut() {
            *child = namespace_node(prefix, child);
        }
    };
    match kind {
        NodeKind::Box(spec) => namespace_children(&mut spec.children),
        NodeKind::SplitPanel(spec) => namespace_children(&mut spec.children),
        NodeKind::Tabs(spec) => namespace_children(&mut spec.children),
        NodeKind::Stepper(spec) => namespace_children(&mut spec.children),
        NodeKind::SummaryList(spec) => namespace_children(&mut spec.children),
        NodeKind::Disclosure(spec) => namespace_children(&mut spec.children),
        NodeKind::Modal(spec) => namespace_children(&mut spec.children),
        NodeKind::ScrollArea(spec) => namespace_children(&mut spec.children),
        NodeKind::ErrorBoundary(spec) => {
            *spec.child = namespace_node(prefix, &spec.child);
            *spec.fallback = namespace_node(prefix, &spec.fallback);
        }
        NodeKind::Switch(spec) => {
            for case in spec.cases.iter_mut() {
                case.child = namespace_node(prefix, &case.child);
            }
            *spec.default = namespace_node(prefix, &spec.default);
        }
        NodeKind::FragmentDecl(spec) => {
            *spec.body = namespace_node(prefix, &spec.body);
        }
        _ => {}
    }
}

// ─── Unwired-action detection (UX hint only) ─────────────────────────────────

fn contains_unwired_action(action: &Action) -> bool {
    match action {
        Action::Dispatch
        | Action::CommitLocal { .. }
        | Action::WriteToClipboard { .. }
        | Action::ReadFileBody { .. } => false,
        Action::Chain(actions) => actions.iter().any(contains_unwired_action),
        Action::Call { .. }
        | Action::Notify { .. }
        | Action::Navigate { .. }
        | Action::SetState { .. }
        | Action::AiTool { .. }
        | Action::Invoke { .. } => true,
    }
}

const UNWIRED_TOOLTIP: &str =
    "This action routes through the runtime substrate (Call/Notify/Navigate/SetState/AiTool).";

/// Resolved-value text for Metric / LabelValueRow value slots.
fn resolved_value_text(resolution: &NumberResolution, format: &CellFormat) -> String {
    match resolution {
        NumberResolution::Resolved(value) => format_number(format, *value),
        NumberResolution::NotResolved => EM_DASH.to_string(),
        NumberResolution::I18nUnresolved(key) => format!("[i18n:{key}]"),
        // Phase 632/649 — a scalar-slot Transform that could not yield a single
        // numeric cell renders its didactic loudly (never a silent em-dash),
        // matching the F#/TS reference.
        NumberResolution::Errored(msg) => format!("(error: {msg})"),
    }
}

// ─── The node wrapper + kind dispatch ────────────────────────────────────────

fn render_children(ctx: &Ctx<'_>, nodes: &[Node]) -> String {
    nodes.iter().map(|n| render_node(ctx, n)).collect()
}

fn render_node(ctx: &Ctx<'_>, node: &Node) -> String {
    let inner = render_node_plain(ctx, node);
    if ctx.islands.contains(&node.id) {
        // The island boundary wrapper: its children are exactly the node's
        // plain static render, so the client hydrates mismatch-free.
        el("div", &[("data-fuaran-island", s(node.id.clone()))], &inner)
    } else {
        inner
    }
}

fn render_node_plain(ctx: &Ctx<'_>, node: &Node) -> String {
    let class_name = node_class_name(&node.kind, &node.style);
    let mut attrs: Vec<Attr> = vec![
        ("id", s(node.id.clone())),
        ("data-fuaran-node-id", s(node.id.clone())),
        ("class", s(class_name)),
    ];
    for (name, value) in accessibility_attributes(ctx.sources, node.accessibility.as_ref()) {
        attrs.push((name, s(value)));
    }
    el("div", &attrs, &render_kind(ctx, node))
}

fn render_kind(ctx: &Ctx<'_>, node: &Node) -> String {
    match &node.kind {
        // Layout
        NodeKind::Box(spec) => render_box(ctx, spec),
        NodeKind::SplitPanel(spec) => {
            let weight_left = spec.weight.clamp(0.0, 1.0);
            let weight_right = 1.0 - weight_left;
            let rendered: Vec<String> = spec.children.iter().map(|c| render_node(ctx, c)).collect();
            let left = el(
                "div",
                &[
                    ("class", s("fuaran-split-pane fuaran-split-pane-left")),
                    ("style", s(format!("flex:{weight_left:.6} 1 0"))),
                ],
                rendered.first().map(String::as_str).unwrap_or(""),
            );
            let right = el(
                "div",
                &[
                    ("class", s("fuaran-split-pane fuaran-split-pane-right")),
                    ("style", s(format!("flex:{weight_right:.6} 1 0"))),
                ],
                &rendered.get(1..).unwrap_or(&[]).join(""),
            );
            el(
                "div",
                &[("class", s("fuaran-layout-split-panel"))],
                &format!("{left}{right}"),
            )
        }
        NodeKind::Tabs(spec) => render_tabs(ctx, &node.id, spec),
        NodeKind::Stepper(spec) => {
            let active_index = try_number(ctx.sources, &spec.active_step).unwrap_or(0.0) as usize;
            let steps: String = (0..spec.children.len())
                .map(|i| {
                    let class = if i == active_index {
                        "fuaran-stepper-step fuaran-stepper-step-active"
                    } else {
                        "fuaran-stepper-step"
                    };
                    text_el("li", &[("class", s(class))], &(i + 1).to_string())
                })
                .collect();
            let numbers = el("ol", &[("class", s("fuaran-stepper-numbers"))], &steps);
            let body_inner = spec
                .children
                .get(active_index)
                .map(|child| render_node(ctx, child))
                .unwrap_or_default();
            let body = el("div", &[("class", s("fuaran-stepper-body"))], &body_inner);
            el(
                "div",
                &[("class", s("fuaran-layout-stepper"))],
                &format!("{numbers}{body}"),
            )
        }
        NodeKind::SummaryList(spec) => {
            let header = spec
                .heading
                .as_ref()
                .map(|h| {
                    text_el(
                        "header",
                        &[("class", s("fuaran-summary-list-heading"))],
                        &render_text(ctx.sources, h),
                    )
                })
                .unwrap_or_default();
            let body = el(
                "div",
                &[("class", s("fuaran-summary-list-body"))],
                &render_children(ctx, &spec.children),
            );
            el(
                "section",
                &[("class", s("fuaran-layout-summary-list"))],
                &format!("{header}{body}"),
            )
        }
        NodeKind::Disclosure(spec) => render_disclosure(ctx, spec),
        NodeKind::Modal(spec) => render_modal(ctx, spec),
        NodeKind::ScrollArea(spec) => render_scroll_area(ctx, spec),
        // Display
        NodeKind::Heading(spec) => {
            let variant_suffix = match spec.variant {
                HeadingVariant::Eyebrow => " fuaran-heading-eyebrow",
                HeadingVariant::Caption => " fuaran-heading-caption",
                HeadingVariant::Lead => " fuaran-heading-lead",
                HeadingVariant::Standard => "",
            };
            let tag = if (1..=6).contains(&spec.level) {
                format!("h{}", spec.level)
            } else {
                "h6".to_string()
            };
            text_el(
                &tag,
                &[("class", s(format!("fuaran-heading{variant_suffix}")))],
                &render_text(ctx.sources, &spec.text),
            )
        }
        NodeKind::Markdown(spec) => el(
            "div",
            &[("class", s("fuaran-markdown"))],
            &markdown_to_html(&render_text(ctx.sources, &spec.text)),
        ),
        NodeKind::Metric(spec) => {
            // Phase 632/649 — the Metric value is a scalar slot: a
            // `Binding.Transform` resolves to its 1×1 result cell.
            let resolution = resolve_scalar_number(ctx.sources, &spec.value);
            if matches!(resolution, NumberResolution::NotResolved)
                && let Some(loading) = &node.state.on_loading
            {
                return render_node(ctx, loading);
            }
            let mut parts = vec![
                text_el(
                    "div",
                    &[("class", s("fuaran-metric-label"))],
                    &render_text(ctx.sources, &spec.label),
                ),
                text_el(
                    "div",
                    &[("class", s("fuaran-metric-value"))],
                    &resolved_value_text(&resolution, &spec.format),
                ),
            ];
            if let Some(trend) = &spec.trend {
                let trend_text = try_scalar_number(ctx.sources, trend)
                    .map(|t| {
                        format_number(spec.trend_format.as_ref().unwrap_or(&CellFormat::None), t)
                    })
                    .unwrap_or_default();
                parts.push(text_el(
                    "div",
                    &[("class", s("fuaran-metric-trend"))],
                    &trend_text,
                ));
            }
            if let Some(subtext) = &spec.subtext {
                parts.push(text_el(
                    "div",
                    &[("class", s("fuaran-metric-subtext"))],
                    &render_text(ctx.sources, subtext),
                ));
            }
            el(
                "div",
                &[(
                    "class",
                    s(format!(
                        "fuaran-metric fuaran-metric-{}",
                        tone_var(spec.tone)
                    )),
                )],
                &parts.concat(),
            )
        }
        NodeKind::Badge(spec) => text_el(
            "span",
            &[(
                "class",
                s(format!(
                    "fuaran-badge fuaran-badge-{}",
                    spec.variant.as_str().to_lowercase()
                )),
            )],
            &render_text(ctx.sources, &spec.label),
        ),
        NodeKind::Skeleton(spec) => {
            let rows: String = (0..spec.rows.max(0))
                .map(|_| el("div", &[("class", s("fuaran-skeleton-row"))], ""))
                .collect();
            el("div", &[("class", s("fuaran-skeleton"))], &rows)
        }
        NodeKind::Icon(spec) => {
            // Phase 821 — the standalone icon-only display kind. The glyph
            // NAME rides `data-icon` (the uniform icon-hook contract — no text
            // content, hosts map it to glyphs); size + tone are modifier
            // classes. A11y: decorative (`label` absent) emits
            // `aria-hidden="true"`; labelled emits `role="img"` +
            // `aria-label`. Mirrors the reference SSR renderer byte-for-byte.
            let mut attrs: Vec<Attr> = vec![
                (
                    "class",
                    s(format!(
                        "fuaran-icon fuaran-icon--{} fuaran-icon-{}",
                        icon_size_class(spec.size),
                        tone_var(spec.tone)
                    )),
                ),
                ("data-icon", s(spec.icon.clone())),
            ];
            match &spec.label {
                Some(label) => {
                    attrs.push(("role", s("img")));
                    attrs.push(("aria-label", s(label.clone())));
                }
                None => attrs.push(("aria-hidden", s("true"))),
            }
            el("span", &attrs, "")
        }
        NodeKind::Callout(spec) => {
            let heading = spec
                .heading
                .as_ref()
                .map(|h| {
                    text_el(
                        "div",
                        &[("class", s("fuaran-callout-heading"))],
                        &render_text(ctx.sources, h),
                    )
                })
                .unwrap_or_default();
            let body = text_el(
                "div",
                &[("class", s("fuaran-callout-body"))],
                &render_text(ctx.sources, &spec.body),
            );
            let dismiss = if spec.dismissable {
                text_el(
                    "button",
                    &[
                        ("class", s("fuaran-callout-dismiss")),
                        ("aria-label", s("Dismiss")),
                    ],
                    "×",
                )
            } else {
                String::new()
            };
            el(
                "div",
                &[(
                    "class",
                    s(format!(
                        "fuaran-callout fuaran-callout-{}",
                        tone_var(spec.tone)
                    )),
                )],
                &format!("{heading}{body}{dismiss}"),
            )
        }
        NodeKind::Progress(spec) => {
            let resolution = resolve_number(ctx.sources, &spec.fraction);
            if matches!(resolution, NumberResolution::NotResolved)
                && let Some(loading) = &node.state.on_loading
            {
                return render_node(ctx, loading);
            }
            let fraction = match resolution {
                NumberResolution::Resolved(v) => v,
                _ => 0.0,
            };
            let indeterminate = if spec.indeterminate {
                " fuaran-progress-indeterminate"
            } else {
                ""
            };
            let label = spec
                .label
                .as_ref()
                .map(|l| {
                    text_el(
                        "div",
                        &[("class", s("fuaran-progress-label"))],
                        &render_text(ctx.sources, l),
                    )
                })
                .unwrap_or_default();
            let fill = el(
                "div",
                &[
                    ("class", s("fuaran-progress-fill")),
                    (
                        "style",
                        s(format!("width:{}%", display_number(fraction * 100.0))),
                    ),
                ],
                "",
            );
            let bar = el("div", &[("class", s("fuaran-progress-bar"))], &fill);
            let caveat = spec
                .caveat
                .as_ref()
                .map(|c| {
                    text_el(
                        "div",
                        &[("class", s("fuaran-progress-caveat"))],
                        &render_text(ctx.sources, c),
                    )
                })
                .unwrap_or_default();
            el(
                "div",
                &[(
                    "class",
                    s(format!(
                        "fuaran-progress fuaran-progress-{}{indeterminate}",
                        tone_var(spec.tone)
                    )),
                )],
                &format!("{label}{bar}{caveat}"),
            )
        }
        NodeKind::Fact(spec) => {
            // Server-side Fact mirrors the reference client tile (label /
            // value+icon / help), toned + emphasised via class hooks.
            let emphasis_suffix = if spec.emphasis {
                " fuaran-fact-emphasis"
            } else {
                ""
            };
            let icon = spec
                .icon
                .as_ref()
                .map(|i| text_el("span", &[("class", s("fuaran-fact-icon"))], i))
                .unwrap_or_default();
            let value = el(
                "div",
                &[("class", s("fuaran-fact-value"))],
                &format!(
                    "{icon}{}",
                    text_el("span", &[], &render_text(ctx.sources, &spec.value))
                ),
            );
            let help = spec
                .help
                .as_ref()
                .map(|h| {
                    text_el(
                        "div",
                        &[("class", s("fuaran-fact-help"))],
                        &render_text(ctx.sources, h),
                    )
                })
                .unwrap_or_default();
            el(
                "div",
                &[(
                    "class",
                    s(format!(
                        "fuaran-fact fuaran-fact-{}{emphasis_suffix}",
                        tone_var(spec.tone)
                    )),
                )],
                &format!(
                    "{}{value}{help}",
                    text_el(
                        "div",
                        &[("class", s("fuaran-fact-label"))],
                        &render_text(ctx.sources, &spec.label),
                    )
                ),
            )
        }
        NodeKind::Sparkline(spec) => render_sparkline(ctx, spec),
        NodeKind::Drawing(spec) => render_drawing(ctx, spec),
        NodeKind::LabelValueRow(spec) => {
            // Phase 632/649 — a scalar slot: a `Binding.Transform` resolves to
            // its 1×1 result cell, ambiguity stays loud.
            let resolution = resolve_scalar_number(ctx.sources, &spec.value);
            if matches!(resolution, NumberResolution::NotResolved)
                && let Some(loading) = &node.state.on_loading
            {
                return render_node(ctx, loading);
            }
            let emphasis_suffix = if spec.emphasis {
                " fuaran-label-value-row-emphasis"
            } else {
                ""
            };
            let help = spec
                .help
                .as_ref()
                .map(|h| {
                    text_el(
                        "span",
                        &[("class", s("fuaran-label-value-row-help"))],
                        &render_text(ctx.sources, h),
                    )
                })
                .unwrap_or_default();
            let label_block = el(
                "div",
                &[("class", s("fuaran-label-value-row-label-block"))],
                &format!(
                    "{}{help}",
                    text_el(
                        "span",
                        &[("class", s("fuaran-label-value-row-label"))],
                        &render_text(ctx.sources, &spec.label),
                    )
                ),
            );
            let value = text_el(
                "span",
                &[("class", s("fuaran-label-value-row-value"))],
                &resolved_value_text(&resolution, &spec.format),
            );
            el(
                "div",
                &[(
                    "class",
                    s(format!("fuaran-label-value-row{emphasis_suffix}")),
                )],
                &format!("{label_block}{value}"),
            )
        }
        NodeKind::Link(spec) => {
            let href =
                sanitize_url_or_blank(&try_string(ctx.sources, &spec.href).unwrap_or_default());
            if spec.protection == Some(crate::wire::LinkProtection::Email)
                && href.starts_with("mailto:")
            {
                // Protected email link: every UTF-16 code unit of the sanitised
                // href AND the label is emitted as a decimal HTML entity — the
                // browser decodes entities in both positions, so the anchor is
                // a working `mailto:` with no JavaScript while the raw source
                // carries no scrapeable address. Encoding every character makes
                // the fragment injection-proof by construction, which is why
                // the anchor is built as a raw string below the
                // attribute-escaping floor (`escape_attr` would re-escape the
                // entities). Byte-identical to the sibling hosts' emissions.
                let anchor = format!(
                    "<a class=\"fuaran-link fuaran-link-protected\" href=\"{}\">{}</a>",
                    entity_encode(&href),
                    entity_encode(&render_text(ctx.sources, &spec.label)),
                );
                el(
                    "span",
                    &[("class", s("fuaran-link-protected-wrap"))],
                    &anchor,
                )
            } else {
                let mut attrs: Vec<Attr> = vec![("class", s("fuaran-link")), ("href", s(href))];
                if let Some(rel) = &spec.rel {
                    attrs.push(("rel", s(rel.clone())));
                }
                if let Some(target) = &spec.target {
                    attrs.push(("target", s(target.clone())));
                }
                if spec.download {
                    attrs.push(("download", AttrVal::Flag(true)));
                }
                text_el("a", &attrs, &render_text(ctx.sources, &spec.label))
            }
        }
        NodeKind::Image(spec) => {
            let src =
                sanitize_url_or_blank(&try_string(ctx.sources, &spec.src).unwrap_or_default());
            let variant_class = match spec.variant {
                ImageVariant::Avatar => "fuaran-image fuaran-image-avatar",
                ImageVariant::Rounded => "fuaran-image fuaran-image-rounded",
                ImageVariant::Default => "fuaran-image",
            };
            void_el(
                "img",
                &[
                    ("class", s(variant_class)),
                    ("src", s(src)),
                    ("alt", s(render_text(ctx.sources, &spec.alt))),
                ],
            )
        }
        NodeKind::List(spec) => {
            let items: String = spec
                .items
                .iter()
                .map(|item| {
                    text_el(
                        "li",
                        &[("class", s("fuaran-list-item"))],
                        &render_text(ctx.sources, item),
                    )
                })
                .collect();
            if spec.ordered {
                el(
                    "ol",
                    &[("class", s("fuaran-list fuaran-list-ordered"))],
                    &items,
                )
            } else {
                el(
                    "ul",
                    &[("class", s("fuaran-list fuaran-list-unordered"))],
                    &items,
                )
            }
        }
        NodeKind::Toast(spec) => {
            let is_open = try_bool(ctx.sources, &spec.open) == Some(true);
            let message = text_el(
                "span",
                &[("class", s("fuaran-toast-message"))],
                &render_text(ctx.sources, &spec.message),
            );
            let dismiss = if spec.dismissable {
                text_el(
                    "button",
                    &[
                        ("class", s("fuaran-toast-dismiss")),
                        ("type", s("button")),
                        ("aria-label", s("Dismiss")),
                    ],
                    "×",
                )
            } else {
                String::new()
            };
            let mut attrs: Vec<Attr> = vec![
                (
                    "class",
                    s(format!("fuaran-toast fuaran-toast-{}", tone_var(spec.tone))),
                ),
                ("role", s("status")),
                ("aria-live", s("polite")),
            ];
            if !is_open {
                attrs.push(("hidden", AttrVal::Flag(true)));
            }
            el("div", &attrs, &format!("{message}{dismiss}"))
        }
        NodeKind::CodeBlock(spec) => {
            let container_class = if spec.line_numbers {
                "fuaran-codeblock fuaran-codeblock-numbered"
            } else {
                "fuaran-codeblock"
            };
            let copy = if spec.copyable {
                text_el(
                    "button",
                    &[
                        ("class", s("fuaran-codeblock-copy")),
                        ("type", s("button")),
                        ("aria-label", s("Copy")),
                    ],
                    "Copy",
                )
            } else {
                String::new()
            };
            let code = el(
                "pre",
                &[("class", s("fuaran-codeblock-pre"))],
                &text_el(
                    "code",
                    &[(
                        "class",
                        s(format!("fuaran-codeblock-code language-{}", spec.language)),
                    )],
                    &spec.code,
                ),
            );
            let mut attrs: Vec<Attr> = vec![
                ("class", s(container_class)),
                ("data-language", s(spec.language.clone())),
            ];
            if !spec.highlight_lines.is_empty() {
                let joined = spec
                    .highlight_lines
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                attrs.push(("data-highlight-lines", s(joined)));
            }
            el("div", &attrs, &format!("{copy}{code}"))
        }
        NodeKind::Math(spec) => {
            let source_span = text_el("span", &[("class", s("fuaran-math-source"))], &spec.source);
            match spec.display {
                MathDisplay::Block => el(
                    "div",
                    &[
                        ("class", s("fuaran-math fuaran-math-block")),
                        ("data-math-display", s("block")),
                    ],
                    &source_span,
                ),
                MathDisplay::Inline => el(
                    "span",
                    &[
                        ("class", s("fuaran-math fuaran-math-inline")),
                        ("data-math-display", s("inline")),
                    ],
                    &source_span,
                ),
            }
        }
        // Input (inert)
        NodeKind::Form(spec) => render_form(ctx, spec),
        NodeKind::Filters(specs) => render_filters(ctx, specs),
        NodeKind::Button(spec) => {
            let unwired = contains_unwired_action(&spec.on_click);
            let variant_class = spec.variant.as_str().to_lowercase();
            let class_name = if unwired {
                format!("fuaran-button fuaran-button-{variant_class} fuaran-button-unwired")
            } else {
                format!("fuaran-button fuaran-button-{variant_class}")
            };
            let is_disabled = spec
                .disabled
                .as_ref()
                .is_some_and(|d| try_bool(ctx.sources, d) == Some(true));
            let mut attrs: Vec<Attr> = vec![("class", s(class_name))];
            if unwired {
                attrs.push(("title", s(UNWIRED_TOOLTIP)));
            }
            if is_disabled {
                attrs.push(("disabled", AttrVal::Flag(true)));
            }
            text_el("button", &attrs, &render_text(ctx.sources, &spec.label))
        }
        NodeKind::FileUpload(spec) => {
            let accept = if spec.accept.is_empty() {
                None
            } else {
                Some(spec.accept.join(","))
            };
            let is_disabled = spec
                .disabled
                .as_ref()
                .is_some_and(|d| try_bool(ctx.sources, d) == Some(true));
            let label = text_el(
                "span",
                &[("class", s("fuaran-file-upload-label"))],
                &render_text(ctx.sources, &spec.label),
            );
            let mut attrs: Vec<Attr> = vec![
                ("class", s("fuaran-file-upload-input")),
                ("type", s("file")),
                ("multiple", AttrVal::Flag(spec.multiple)),
            ];
            if is_disabled {
                attrs.push(("disabled", AttrVal::Flag(true)));
            }
            if let Some(accept) = accept {
                attrs.push(("accept", s(accept)));
            }
            el(
                "label",
                &[("class", s("fuaran-file-upload"))],
                &format!("{label}{}", void_el("input", &attrs)),
            )
        }
        NodeKind::Select(spec) => render_select(ctx, spec),
        // Visualisation
        NodeKind::DataGrid(spec) => match &spec.static_rows {
            Some(rows) => render_static_table(ctx, rows),
            None => render_grid(ctx, &node.state, spec),
        },
        NodeKind::Chart(spec) => render_chart(ctx, &node.state, spec),
        NodeKind::Map(spec) => render_map(ctx, &node.state, spec),
        // Structural
        NodeKind::Custom(spec) => {
            let prop_keys = spec
                .props
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let label = text_el(
                "div",
                &[("class", s("fuaran-custom-label"))],
                &format!("Custom {}.{}", spec.module_id, spec.component_id),
            );
            let props_div = text_el(
                "div",
                &[("class", s("fuaran-custom-props"))],
                &format!("props: {prop_keys}"),
            );
            el(
                "div",
                &[("class", s("fuaran-custom-placeholder"))],
                &format!("{label}{props_div}"),
            )
        }
        // No error server-side — render the child inert.
        NodeKind::ErrorBoundary(spec) => render_node(ctx, &spec.child),
        NodeKind::Switch(spec) => {
            // SSR resolves the selector and renders the matching case, else
            // the default — the client's first render reads the same initial
            // state (hydration parity). Phase 768 — the selector is any
            // Binding: `State` keeps the direct state-bag read (with the
            // 768-form defaultValue seeding on an unwritten key); other
            // bindings resolve through the resolver, so an SSR switch on a
            // pre-seeded Selection renders the branch the client will.
            let value_str = match &spec.on {
                Binding::State { key, default_value } => match ctx.sources.state.get(key) {
                    None => static_display_string(default_value).unwrap_or_default(),
                    Some(JVal::Null) => String::new(),
                    Some(JVal::Str(v)) => v.clone(),
                    Some(JVal::Num(n)) => display_number(*n),
                    Some(JVal::Bool(b)) => if *b { "true" } else { "false" }.to_string(),
                    Some(other) => crate::canonical::render_canonical(other),
                },
                on => try_string(ctx.sources, on).unwrap_or_default(),
            };
            let matched = spec.cases.iter().find(|c| c.match_value == value_str);
            match matched {
                Some(case) => render_node(ctx, &case.child),
                None => render_node(ctx, &spec.default),
            }
        }
        // Zero-paint — the decl is a template, not visible output.
        NodeKind::FragmentDecl(_) => String::new(),
        NodeKind::FragmentRef(spec) => render_fragment_ref(ctx, &node.id, &spec.name),
        NodeKind::Mount(spec) => text_el(
            "div",
            &[
                ("class", s("fuaran-mount-placeholder")),
                ("data-fuaran-mount-scope", s(spec.scope_id.clone())),
            ],
            &format!(
                "[fuaran:mount '{}' — guest loader not attached]",
                spec.scope_id
            ),
        ),
    }
}

// ─── Box — the unified container ─────────────────────────────────────────────

fn render_box(ctx: &Ctx<'_>, spec: &BoxSpec) -> String {
    if spec.role == BoxRole::Card {
        let header = spec
            .heading
            .as_ref()
            .map(|h| {
                text_el(
                    "header",
                    &[("class", s("fuaran-card-heading"))],
                    &render_text(ctx.sources, h),
                )
            })
            .unwrap_or_default();
        let body = el(
            "div",
            &[("class", s("fuaran-card-body"))],
            &render_children(ctx, &spec.children),
        );
        return el(
            "section",
            &[("class", s("fuaran-layout-card"))],
            &format!("{header}{body}"),
        );
    }
    if spec.role == BoxRole::Dashboard || matches!(spec.layout, BoxLayout::Auto) {
        return el(
            "div",
            &[("class", s("fuaran-layout-dashboard"))],
            &render_children(ctx, &spec.children),
        );
    }
    if spec.role == BoxRole::Separator {
        return void_el("hr", &[("class", s("fuaran-layout-separator"))]);
    }
    if let BoxLayout::Grid {
        cols,
        gap,
        template_columns,
    } = &spec.layout
    {
        let template = template_columns
            .clone()
            .unwrap_or_else(|| format!("repeat({cols}, 1fr)"));
        let grid_style = match gap {
            Some(gap) => format!("grid-template-columns:{template};gap:{gap}px"),
            None => format!("grid-template-columns:{template}"),
        };
        return el(
            "div",
            &[("class", s("fuaran-layout-grid")), ("style", s(grid_style))],
            &render_children(ctx, &spec.children),
        );
    }
    let (dir, wrap, gap) = match &spec.layout {
        BoxLayout::Flex {
            direction,
            gap,
            wrap,
        } => (
            if *direction == Orientation::Horizontal {
                "fuaran-stack-horizontal"
            } else {
                "fuaran-stack-vertical"
            },
            if *wrap { " fuaran-stack-wrap" } else { "" },
            *gap,
        ),
        BoxLayout::Grid { .. } | BoxLayout::Auto => ("fuaran-stack-vertical", "", None),
    };
    let class = format!("fuaran-layout-stack {dir}{wrap}");
    match gap {
        Some(gap) => el(
            "div",
            &[("class", s(class)), ("style", s(format!("gap:{gap}px")))],
            &render_children(ctx, &spec.children),
        ),
        None => el(
            "div",
            &[("class", s(class))],
            &render_children(ctx, &spec.children),
        ),
    }
}

fn render_disclosure(ctx: &Ctx<'_>, spec: &DisclosureSpec) -> String {
    let resolved_open = try_bool(ctx.sources, &spec.open).unwrap_or(spec.default_open);
    let summary = text_el(
        "summary",
        &[("class", s("fuaran-disclosure-summary"))],
        &render_text(ctx.sources, &spec.heading),
    );
    let body = el(
        "div",
        &[("class", s("fuaran-disclosure-body"))],
        &render_children(ctx, &spec.children),
    );
    el(
        "details",
        &[
            ("class", s("fuaran-layout-disclosure")),
            ("open", AttrVal::Flag(resolved_open)),
        ],
        &format!("{summary}{body}"),
    )
}

fn render_modal(ctx: &Ctx<'_>, spec: &ModalSpec) -> String {
    // Overlay render-fidelity contract: ALWAYS in the DOM; closed = `hidden`;
    // positioned by CSS. Inert server-side.
    let is_open = try_bool(ctx.sources, &spec.open) == Some(true);
    let heading = spec
        .heading
        .as_ref()
        .map(|h| {
            text_el(
                "h2",
                &[("class", s("fuaran-modal-heading"))],
                &render_text(ctx.sources, h),
            )
        })
        .unwrap_or_default();
    let dismiss = if spec.dismissable {
        text_el(
            "button",
            &[
                ("class", s("fuaran-modal-dismiss")),
                ("type", s("button")),
                ("aria-label", s("Close")),
            ],
            "×",
        )
    } else {
        String::new()
    };
    let body = el(
        "div",
        &[("class", s("fuaran-modal-body"))],
        &render_children(ctx, &spec.children),
    );
    let dialog = el(
        "div",
        &[
            ("class", s("fuaran-modal-dialog")),
            ("role", s("dialog")),
            ("aria-modal", s("true")),
        ],
        &format!("{heading}{dismiss}{body}"),
    );
    let mut overlay_attrs: Vec<Attr> = vec![("class", s("fuaran-modal-overlay"))];
    if !is_open {
        overlay_attrs.push(("hidden", AttrVal::Flag(true)));
    }
    el("div", &overlay_attrs, &dialog)
}

fn render_scroll_area(ctx: &Ctx<'_>, spec: &ScrollAreaSpec) -> String {
    let axis_class = match spec.orientation {
        ScrollOrientation::Horizontal => "fuaran-scrollarea fuaran-scrollarea-horizontal",
        ScrollOrientation::Both => "fuaran-scrollarea fuaran-scrollarea-both",
        ScrollOrientation::Vertical => "fuaran-scrollarea fuaran-scrollarea-vertical",
    };
    let mut style_parts: Vec<String> = Vec::new();
    if let Some(max_height) = spec.max_height {
        style_parts.push(format!("max-height:{max_height}px"));
    }
    if let Some(max_width) = spec.max_width {
        style_parts.push(format!("max-width:{max_width}px"));
    }
    let mut attrs: Vec<Attr> = vec![("class", s(axis_class)), ("tabindex", s("0"))];
    if !style_parts.is_empty() {
        attrs.push(("style", s(style_parts.join(";"))));
    }
    el("div", &attrs, &render_children(ctx, &spec.children))
}

// ─── Tabs ────────────────────────────────────────────────────────────────────

fn render_tabs(ctx: &Ctx<'_>, parent_node_id: &str, spec: &TabsSpec) -> String {
    struct PerTab {
        label: String,
        icon: Option<String>,
        disabled: bool,
    }

    let label_from_child = |child: &Node| -> String {
        if let NodeKind::Box(bs) = &child.kind
            && bs.role == BoxRole::Card
            && let Some(heading) = &bs.heading
        {
            return render_text(ctx.sources, heading);
        }
        child.id.clone()
    };

    let per_tab: Vec<PerTab> = match &spec.tab_headers {
        Some(headers) => headers
            .iter()
            .map(|h| PerTab {
                label: render_text(ctx.sources, &h.label),
                icon: h.icon.clone(),
                disabled: h
                    .disabled
                    .as_ref()
                    .and_then(|d| try_bool(ctx.sources, d))
                    .unwrap_or(false),
            })
            .collect(),
        None => spec
            .children
            .iter()
            .map(|child| PerTab {
                label: label_from_child(child),
                icon: None,
                disabled: false,
            })
            .collect(),
    };

    let is_vertical = spec.orientation == Orientation::Vertical;
    let orientation_class = if is_vertical {
        "fuaran-tabs-vertical"
    } else {
        "fuaran-tabs-horizontal"
    };

    let resolved_from_tag: Option<usize> = match (&spec.tab_tags, &spec.active_tag) {
        (Some(tags), Some(active_tag)) => {
            try_string(ctx.sources, active_tag).and_then(|tag| tags.iter().position(|t| *t == tag))
        }
        _ => None,
    };
    let raw_index = resolved_from_tag
        .or_else(|| try_number(ctx.sources, &spec.active_index).map(|n| n.max(0.0) as usize))
        .unwrap_or(0);
    let active_index = raw_index.min(spec.children.len().saturating_sub(1));
    let active_child = spec
        .children
        .get(active_index)
        .or_else(|| spec.children.first());

    let tab_id = |i: usize| format!("{parent_node_id}-tab-{i}");
    let panel_id = |i: usize| format!("{parent_node_id}-panel-{i}");

    let tabs: String = per_tab
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let is_active = i == active_index;
            let mut classes = vec!["fuaran-tab"];
            if is_active {
                classes.push("fuaran-tab-active");
            }
            if t.disabled {
                classes.push("fuaran-tab-disabled");
            }
            let inner = format!(
                "{}{}",
                t.icon
                    .as_ref()
                    .map(|icon| text_el("span", &[("class", s("fuaran-tab-icon"))], icon))
                    .unwrap_or_default(),
                text_el("span", &[("class", s("fuaran-tab-label"))], &t.label)
            );
            let mut attrs: Vec<Attr> = vec![
                ("id", s(tab_id(i))),
                ("class", s(classes.join(" "))),
                ("role", s("tab")),
                ("aria-selected", s(if is_active { "true" } else { "false" })),
                ("aria-controls", s(panel_id(i))),
                ("tabindex", s(if is_active { "0" } else { "-1" })),
                ("data-tab-index", s(i.to_string())),
            ];
            if t.disabled {
                attrs.push(("aria-disabled", s("true")));
                attrs.push(("disabled", AttrVal::Flag(true)));
            }
            el("button", &attrs, &inner)
        })
        .collect();

    let bar = el(
        "div",
        &[
            ("class", s("fuaran-tabs-bar")),
            ("role", s("tablist")),
            (
                "aria-orientation",
                s(if is_vertical {
                    "vertical"
                } else {
                    "horizontal"
                }),
            ),
        ],
        &tabs,
    );

    let panel = active_child
        .map(|child| {
            el(
                "div",
                &[
                    ("id", s(panel_id(active_index))),
                    ("role", s("tabpanel")),
                    ("aria-labelledby", s(tab_id(active_index))),
                    ("tabindex", s("0")),
                    ("class", s("fuaran-tabs-panel")),
                ],
                &render_node(ctx, child),
            )
        })
        .unwrap_or_default();
    let panels = el("div", &[("class", s("fuaran-tabs-panels"))], &panel);
    el(
        "div",
        &[(
            "class",
            s(format!("fuaran-layout-tabs {orientation_class}")),
        )],
        &format!("{bar}{panels}"),
    )
}

// ─── Sparkline ───────────────────────────────────────────────────────────────

fn render_sparkline(ctx: &Ctx<'_>, spec: &crate::wire::SparklineSpec) -> String {
    let series = resolve_float_seq(ctx.sources, &spec.source);
    if series.is_empty() {
        return text_el(
            "div",
            &[("class", s("fuaran-sparkline fuaran-sparkline-empty"))],
            EM_DASH,
        );
    }
    let n = series.len();
    let min_v = series.iter().copied().fold(f64::INFINITY, f64::min);
    let max_v = series.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = if max_v - min_v < 1e-9 {
        1.0
    } else {
        max_v - min_v
    };
    let points: String = series
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if n <= 1 {
                50.0
            } else {
                (i as f64 / (n - 1) as f64) * 100.0
            };
            let y = 30.0 - ((v - min_v) / range) * 28.0 - 1.0;
            format!("{x:.2},{y:.2}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let polyline = el(
        "polyline",
        &[
            ("class", s("fuaran-sparkline-line")),
            ("fill", s("none")),
            ("stroke", s("currentColor")),
            ("stroke-width", s("1.5")),
            ("points", s(points)),
        ],
        "",
    );
    el(
        "svg",
        &[
            ("class", s("fuaran-sparkline")),
            ("viewBox", s("0 0 100 30")),
            ("preserveAspectRatio", s("none")),
        ],
        &polyline,
    )
}

// ─── Drawing (Phase 525) — inline SVG for the server-HTML tier ────────────────
//
// A byte-faithful port of the canonical F# `Renderer.Core.DrawingSvg` builder
// (mirrored by the TS / Python / Go hosts): static geometry lowered to inline
// `<svg>` — same path `d`, coordinate/number form, XML escaping, open-shape
// `fill="none"` defaults, `role="img"` + optional `<title>`/`<desc>` (a11y),
// and the parity-locked `fuaran-drawing*` class vocabulary. Only `DrawStyle`
// carries `Binding`s (resolved Static; the headless baseline omits the rest).

fn draw_num(n: f64) -> String {
    if !n.is_finite() {
        return "0".to_string();
    }
    if n == n.floor() && n.abs() < 1e15 {
        (n as i64).to_string()
    } else {
        format!("{n}")
    }
}

fn draw_static_string(b: &crate::wire::Binding) -> Option<String> {
    match b {
        crate::wire::Binding::Static {
            value: crate::wire::StaticValue::Ast(crate::canonical::JVal::Str(v)),
        } => Some(v.clone()),
        _ => None,
    }
}

fn draw_static_number(b: &crate::wire::Binding) -> Option<f64> {
    match b {
        crate::wire::Binding::Static {
            value: crate::wire::StaticValue::Ast(crate::canonical::JVal::Num(n)),
        } => Some(*n),
        _ => None,
    }
}

fn draw_style_attrs(style: &crate::wire::DrawStyle, default_fill_none: bool) -> String {
    let mut out = String::new();
    match &style.fill {
        Some(b) => {
            if let Some(v) = draw_static_string(b) {
                out.push_str(&format!(" fill=\"{}\"", escape_attr(&v)));
            }
        }
        None => {
            if default_fill_none {
                out.push_str(" fill=\"none\"");
            }
        }
    }
    if let Some(v) = style.opacity.as_ref().and_then(draw_static_number) {
        out.push_str(&format!(" opacity=\"{}\"", draw_num(v)));
    }
    if let Some(v) = style.stroke.as_ref().and_then(draw_static_string) {
        out.push_str(&format!(" stroke=\"{}\"", escape_attr(&v)));
    }
    if let Some(v) = style.stroke_width.as_ref().and_then(draw_static_number) {
        out.push_str(&format!(" stroke-width=\"{}\"", draw_num(v)));
    }
    if let Some(ta) = &style.text_anchor {
        let anchor = match ta {
            crate::wire::TextAnchor::Start => "start",
            crate::wire::TextAnchor::Middle => "middle",
            crate::wire::TextAnchor::End => "end",
        };
        out.push_str(&format!(" text-anchor=\"{anchor}\""));
    }
    if let Some(ff) = &style.font_family {
        out.push_str(&format!(" font-family=\"{}\"", escape_attr(ff)));
    }
    if let Some(fs) = style.font_size {
        out.push_str(&format!(" font-size=\"{}px\"", draw_num(fs)));
    }
    if let Some(em) = &style.emphasis {
        let weight = match em {
            crate::wire::Emphasis::Quiet => "300",
            crate::wire::Emphasis::Normal => "400",
            crate::wire::Emphasis::Loud => "700",
        };
        out.push_str(&format!(" font-weight=\"{weight}\""));
    }
    // Phase 642 — keyed mark identity: a data-bearing shape's derivation-based
    // id rides into the emitted SVG so marks are addressable (object
    // constancy) — last in the fixed attribute order, matching the reference
    // renderer.
    if let Some(m) = &style.mark_id {
        out.push_str(&format!(" data-fuaran-mark=\"{}\"", escape_attr(m)));
    }
    out
}

/// Phase 875 — round line joins + caps on a STROKED path shape (`Polyline` /
/// `Polygon` / `Curve`). A RENDERER default, not a wire field: `DrawStyle`
/// gains nothing, no fixture changes shape, and every host emits the same two
/// attributes from its own builder. SVG's initial `stroke-linejoin` is
/// `miter`, which spikes at the acute vertices a data polyline routinely has
/// — a visible artefact that carries no data.
///
/// Emitted only when the shape actually strokes, so a fill-only polygon (an
/// area band) keeps its minimal attribute set. `Line` is deliberately
/// excluded: a round cap on the axis and gridline rules would overhang each
/// end by half the stroke width, lengthening chrome that is positioned
/// exactly.
fn stroke_join_attrs(style: &crate::wire::DrawStyle) -> &'static str {
    match style.stroke.as_ref().and_then(draw_static_string) {
        Some(_) => " stroke-linejoin=\"round\" stroke-linecap=\"round\"",
        None => "",
    }
}

fn draw_points(points: &[crate::wire::DrawPoint]) -> String {
    points
        .iter()
        .map(|p| format!("{},{}", draw_num(p.x), draw_num(p.y)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn draw_path_d(commands: &[crate::wire::CurveCommand]) -> String {
    use crate::wire::CurveCommand as C;
    let pt = |p: &crate::wire::DrawPoint| format!("{} {}", draw_num(p.x), draw_num(p.y));
    commands
        .iter()
        .map(|c| match c {
            C::MoveTo(to) => format!("M{}", pt(to)),
            C::LineTo(to) => format!("L{}", pt(to)),
            C::CubicTo {
                control1,
                control2,
                to,
            } => format!("C{} {} {}", pt(control1), pt(control2), pt(to)),
            C::QuadraticTo { control, to } => format!("Q{} {}", pt(control), pt(to)),
            C::Close => "Z".to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_shape(ctx: &Ctx<'_>, sh: &crate::wire::Shape) -> String {
    use crate::wire::Shape as S;
    match sh {
        S::Group { children, style } => {
            let inner: String = children.iter().map(|c| render_shape(ctx, c)).collect();
            format!(
                "<g class=\"fuaran-drawing-group\"{}>{inner}</g>",
                draw_style_attrs(style, false)
            )
        }
        S::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius,
            style,
        } => {
            let rx = corner_radius
                .map(|cr| format!(" rx=\"{}\"", draw_num(cr)))
                .unwrap_or_default();
            format!(
                "<rect class=\"fuaran-drawing-rect\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{rx}{}/>",
                draw_num(*x),
                draw_num(*y),
                draw_num(*width),
                draw_num(*height),
                draw_style_attrs(style, false)
            )
        }
        S::Line {
            x1,
            y1,
            x2,
            y2,
            style,
        } => format!(
            "<line class=\"fuaran-drawing-line\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{}/>",
            draw_num(*x1),
            draw_num(*y1),
            draw_num(*x2),
            draw_num(*y2),
            draw_style_attrs(style, false)
        ),
        S::Polyline { points, style } => format!(
            "<polyline class=\"fuaran-drawing-polyline\" points=\"{}\"{}{}/>",
            draw_points(points),
            draw_style_attrs(style, true),
            stroke_join_attrs(style)
        ),
        S::Polygon { points, style } => format!(
            "<polygon class=\"fuaran-drawing-polygon\" points=\"{}\"{}{}/>",
            draw_points(points),
            draw_style_attrs(style, false),
            stroke_join_attrs(style)
        ),
        S::Curve { commands, style } => format!(
            "<path class=\"fuaran-drawing-curve\" d=\"{}\"{}{}/>",
            draw_path_d(commands),
            draw_style_attrs(style, true),
            stroke_join_attrs(style)
        ),
        S::Circle { cx, cy, r, style } => format!(
            "<circle class=\"fuaran-drawing-circle\" cx=\"{}\" cy=\"{}\" r=\"{}\"{}/>",
            draw_num(*cx),
            draw_num(*cy),
            draw_num(*r),
            draw_style_attrs(style, false)
        ),
        S::Ellipse {
            cx,
            cy,
            rx,
            ry,
            style,
        } => format!(
            "<ellipse class=\"fuaran-drawing-ellipse\" cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{}/>",
            draw_num(*cx),
            draw_num(*cy),
            draw_num(*rx),
            draw_num(*ry),
            draw_style_attrs(style, false)
        ),
        // Phase 877 — the rotation transform is built HERE rather than in
        // `draw_style_attrs` because the pivot is the label's own anchor point,
        // which the style record does not carry; `draw_style_attrs` is shared by
        // every shape and stays position-free. Anchoring at (x, y) is what makes
        // the rotation compose with `text_anchor` — the text turns about the
        // point it is aligned to. Degrees, clockwise (SVG's own convention).
        S::Label { x, y, text, style } => format!(
            "<text class=\"fuaran-drawing-label\" x=\"{}\" y=\"{}\"{}{}>{}</text>",
            draw_num(*x),
            draw_num(*y),
            style
                .rotation
                .map(|deg| format!(
                    " transform=\"rotate({} {} {})\"",
                    draw_num(deg),
                    draw_num(*x),
                    draw_num(*y)
                ))
                .unwrap_or_default(),
            draw_style_attrs(style, false),
            escape_text(&render_text(ctx.sources, text))
        ),
    }
}

fn render_drawing(ctx: &Ctx<'_>, spec: &crate::wire::DrawingSpec) -> String {
    let vb = &spec.view_box;
    let view_box = format!(
        "{} {} {} {}",
        draw_num(vb.min_x),
        draw_num(vb.min_y),
        draw_num(vb.width),
        draw_num(vb.height)
    );
    let title = spec
        .title
        .as_ref()
        .map(|t| {
            format!(
                "<title>{}</title>",
                escape_text(&render_text(ctx.sources, t))
            )
        })
        .unwrap_or_default();
    let desc = spec
        .description
        .as_ref()
        .map(|d| format!("<desc>{}</desc>", escape_text(&render_text(ctx.sources, d))))
        .unwrap_or_default();
    let body: String = spec.shapes.iter().map(|s| render_shape(ctx, s)).collect();
    let root_style = draw_style_attrs(&spec.style, false);
    let svg = format!(
        "<svg class=\"fuaran-drawing\" role=\"img\" viewBox=\"{view_box}\"{root_style}>{title}{desc}{body}</svg>"
    );
    format!("<div>{svg}</div>")
}

// ─── Inputs (inert) ──────────────────────────────────────────────────────────

fn render_select(ctx: &Ctx<'_>, spec: &SelectSpec) -> String {
    let options = resolve_options(ctx.sources, &spec.source);
    // The opaque placeholder never reaches the DOM (render contract §5).
    let options: Vec<&SelectOption> = options.iter().filter(|o| o.value != "<opaque>").collect();
    let is_disabled = spec
        .disabled
        .as_ref()
        .is_some_and(|d| try_bool(ctx.sources, d) == Some(true));
    let label = text_el(
        "span",
        &[("class", s("fuaran-select-label"))],
        &render_text(ctx.sources, &spec.label),
    );
    let placeholder = spec
        .placeholder
        .as_ref()
        .map(|p| text_el("option", &[("value", s(""))], &render_text(ctx.sources, p)))
        .unwrap_or_default();
    let opts: String = options
        .iter()
        .map(|o| {
            text_el(
                "option",
                &[("value", s(o.value.clone()))],
                &render_text(ctx.sources, &o.label),
            )
        })
        .collect();
    let control = if spec.multiple {
        el(
            "select",
            &[
                ("class", s("fuaran-select-control")),
                ("multiple", AttrVal::Flag(true)),
                ("disabled", AttrVal::Flag(is_disabled)),
            ],
            &opts,
        )
    } else {
        el(
            "select",
            &[
                ("class", s("fuaran-select-control")),
                ("disabled", AttrVal::Flag(is_disabled)),
            ],
            &format!("{placeholder}{opts}"),
        )
    };
    el(
        "label",
        &[("class", s("fuaran-select"))],
        &format!("{label}{control}"),
    )
}

fn render_form(ctx: &Ctx<'_>, spec: &FormSpec) -> String {
    let fields: String = spec
        .fields
        .iter()
        .map(|f| render_form_field(ctx, f))
        .collect();
    let submit = text_el(
        "button",
        &[("class", s("fuaran-form-submit")), ("type", s("submit"))],
        &render_text(ctx.sources, &spec.submit_label),
    );
    let body = format!("{fields}{submit}");
    let children = match &spec.disabled {
        Some(disabled) => el(
            "fieldset",
            &[
                ("class", s("fuaran-form-fieldset")),
                (
                    "disabled",
                    AttrVal::Flag(try_bool(ctx.sources, disabled) == Some(true)),
                ),
            ],
            &body,
        ),
        None => body,
    };
    el("form", &[("class", s("fuaran-form"))], &children)
}

fn render_form_field(ctx: &Ctx<'_>, field: &FormField) -> String {
    let label_text = render_text(ctx.sources, &field.label);
    let label_with_required = if field.required {
        format!("{label_text} *")
    } else {
        label_text
    };
    let label = text_el(
        "label",
        &[
            ("class", s("fuaran-form-label")),
            ("for", s(field.id.clone())),
        ],
        &label_with_required,
    );
    let control = render_form_control(ctx, field);
    let help = field
        .help
        .as_ref()
        .map(|h| {
            text_el(
                "div",
                &[("class", s("fuaran-form-help"))],
                &render_text(ctx.sources, h),
            )
        })
        .unwrap_or_default();
    el(
        "div",
        &[("class", s("fuaran-form-field"))],
        &format!("{label}{control}{help}"),
    )
}

fn render_form_control(ctx: &Ctx<'_>, field: &FormField) -> String {
    match &field.kind {
        FormFieldKind::Text { value, .. } => {
            let current = try_string(ctx.sources, value).unwrap_or_default();
            void_el(
                "input",
                &[
                    ("class", s("fuaran-form-input")),
                    ("type", s("text")),
                    ("id", s(field.id.clone())),
                    ("required", AttrVal::Flag(field.required)),
                    ("value", s(current)),
                ],
            )
        }
        FormFieldKind::Number { value, .. } => {
            let current = try_number(ctx.sources, value).unwrap_or(0.0);
            void_el(
                "input",
                &[
                    ("class", s("fuaran-form-input")),
                    ("type", s("number")),
                    ("id", s(field.id.clone())),
                    ("required", AttrVal::Flag(field.required)),
                    ("value", s(display_number(current))),
                ],
            )
        }
        FormFieldKind::Range {
            value, min, max, ..
        } => {
            // 0.2.0 dual-thumb range: two bounded number inputs over the
            // resolved (min, max) pair.
            //
            // Class vocabulary reconciled to the reference renderer (Phase 747).
            // This arm emitted `fuaran-form-range*`, which matches NEITHER of the
            // reference's two context-dependent spellings — `fuaran-filter-range*`
            // for a filter chip (this host's filter path already gets that right)
            // and `fuaran-field-range*` for a form field, which is this path. The
            // divergence survived because no gate measured it: the parity oracle
            // this host now has did not exist, and the corpus exercises `Range`
            // only through the Filters carrier, so it is invisible there too.
            let (lo, hi) = resolve_float_pair(ctx.sources, value).unwrap_or((0.0, 0.0));
            let bound_attrs = |v: f64, class: &'static str| {
                let mut attrs: Vec<Attr> = vec![
                    ("type", s("number")),
                    ("class", s(class)),
                    ("value", s(display_number(v))),
                ];
                if let Some(min) = min {
                    attrs.push(("min", s(display_number(*min))));
                }
                if let Some(max) = max {
                    attrs.push(("max", s(display_number(*max))));
                }
                attrs
            };
            el(
                "span",
                &[
                    ("class", s("fuaran-field-range")),
                    ("id", s(field.id.clone())),
                ],
                &format!(
                    "{}{}{}",
                    void_el(
                        "input",
                        &bound_attrs(lo, "fuaran-form-field-control fuaran-field-range-min")
                    ),
                    text_el("span", &[("class", s("fuaran-field-range-sep"))], "–"),
                    void_el(
                        "input",
                        &bound_attrs(hi, "fuaran-form-field-control fuaran-field-range-max")
                    )
                ),
            )
        }
        FormFieldKind::RangedNumber {
            value,
            min,
            max,
            step,
            ..
        } => {
            let current = try_number(ctx.sources, value).unwrap_or(0.0);
            let mut attrs: Vec<Attr> = vec![
                ("class", s("fuaran-form-input")),
                ("type", s("number")),
                ("id", s(field.id.clone())),
                ("required", AttrVal::Flag(field.required)),
                ("value", s(display_number(current))),
            ];
            if let Some(min) = min {
                attrs.push(("min", s(display_number(*min))));
            }
            if let Some(max) = max {
                attrs.push(("max", s(display_number(*max))));
            }
            if let Some(step) = step {
                attrs.push(("step", s(display_number(*step))));
            }
            void_el("input", &attrs)
        }
        FormFieldKind::Checkbox { value, .. } => {
            let current = try_bool(ctx.sources, value).unwrap_or(false);
            void_el(
                "input",
                &[
                    ("class", s("fuaran-form-checkbox")),
                    ("type", s("checkbox")),
                    ("id", s(field.id.clone())),
                    ("checked", AttrVal::Flag(current)),
                ],
            )
        }
        // Phase 766 — the switch affordance. role/aria-checked must be in the
        // SERVER HTML: a switch that only becomes one after hydration is
        // announced wrongly on first paint, and never at all in a static
        // render.
        FormFieldKind::Toggle { value, .. } => {
            let current = try_bool(ctx.sources, value).unwrap_or(false);
            void_el(
                "input",
                &[
                    ("class", s("fuaran-form-toggle")),
                    ("type", s("checkbox")),
                    ("role", s("switch")),
                    ("aria-checked", s(if current { "true" } else { "false" })),
                    ("id", s(field.id.clone())),
                    ("checked", AttrVal::Flag(current)),
                ],
            )
        }
        FormFieldKind::Choice { options, .. } => {
            let opts = resolve_options(ctx.sources, options);
            let options_html = format!(
                "{}{}",
                text_el("option", &[("value", s(""))], EM_DASH),
                opts.iter()
                    .filter(|o| o.value != "<opaque>")
                    .map(|o| text_el(
                        "option",
                        &[("value", s(o.value.clone()))],
                        &render_text(ctx.sources, &o.label),
                    ))
                    .collect::<String>()
            );
            el(
                "select",
                &[
                    ("class", s("fuaran-form-select")),
                    ("id", s(field.id.clone())),
                    ("required", AttrVal::Flag(field.required)),
                ],
                &options_html,
            )
        }
        FormFieldKind::TextArea { value, rows, .. } => {
            let current = try_string(ctx.sources, value).unwrap_or_default();
            text_el(
                "textarea",
                &[
                    ("class", s("fuaran-form-textarea")),
                    ("id", s(field.id.clone())),
                    ("required", AttrVal::Flag(field.required)),
                    ("rows", s(rows.to_string())),
                ],
                &current,
            )
        }
        FormFieldKind::SegmentedChoice {
            options,
            orientation,
            value,
            ..
        } => render_segmented_choice(ctx, &field.id, options, value, *orientation),
        FormFieldKind::Date {
            value,
            variant,
            min,
            max,
            step,
            ..
        } => {
            let input_type = match variant {
                crate::wire::DateVariant::Time => "time",
                crate::wire::DateVariant::DateTime => "datetime-local",
                crate::wire::DateVariant::Date => "date",
            };
            let current = try_string(ctx.sources, value).unwrap_or_default();
            let mut attrs: Vec<Attr> = vec![
                ("class", s("fuaran-form-input fuaran-form-date")),
                ("type", s(input_type)),
                ("id", s(field.id.clone())),
                ("required", AttrVal::Flag(field.required)),
                ("value", s(current)),
            ];
            if let Some(min) = min {
                attrs.push(("min", s(min.clone())));
            }
            if let Some(max) = max {
                attrs.push(("max", s(max.clone())));
            }
            if let Some(step) = step {
                attrs.push(("step", s(display_number(*step))));
            }
            void_el("input", &attrs)
        }
        FormFieldKind::DateRange {
            value,
            variant,
            min,
            max,
            step,
            ..
        } => {
            // 0.7.0 — SSR parity with the client's dual-input range: two native
            // date/time inputs (per variant) over the pair's ends, sharing the
            // min/max/step attributes. Inert like every other server-rendered
            // control; only the FROM end carries `data-fuaran-field` — it is the
            // pair's addressable slot. Class vocabulary follows the reference
            // server renderer (`fuaran-field-range*` + `fuaran-form-field-control`),
            // NOT this file's older `fuaran-form-range*` spellings, which have no
            // counterpart there.
            let input_type = match variant {
                crate::wire::DateVariant::Time => "time",
                crate::wire::DateVariant::DateTime => "datetime-local",
                crate::wire::DateVariant::Date => "date",
            };
            let (from_v, to_v) = resolve_string_pair(ctx.sources, value)
                .unwrap_or_else(|| (String::new(), String::new()));
            let end_attrs = |v: &str, class: &'static str, addressable: bool| {
                let mut attrs: Vec<Attr> = vec![("class", s(class))];
                if addressable {
                    attrs.push(("data-fuaran-field", s(field.id.clone())));
                }
                attrs.push(("type", s(input_type)));
                attrs.push(("value", s(v.to_string())));
                if let Some(min) = min {
                    attrs.push(("min", s(min.clone())));
                }
                if let Some(max) = max {
                    attrs.push(("max", s(max.clone())));
                }
                if let Some(step) = step {
                    attrs.push(("step", s(display_number(*step))));
                }
                attrs
            };
            el(
                "span",
                &[("class", s("fuaran-field-range"))],
                &format!(
                    "{}{}{}",
                    void_el(
                        "input",
                        &end_attrs(
                            &from_v,
                            "fuaran-form-field-control fuaran-field-range-min",
                            true
                        )
                    ),
                    text_el("span", &[("class", s("fuaran-field-range-sep"))], "–"),
                    void_el(
                        "input",
                        &end_attrs(
                            &to_v,
                            "fuaran-form-field-control fuaran-field-range-max",
                            false
                        )
                    )
                ),
            )
        }
    }
}

fn render_segmented_choice(
    ctx: &Ctx<'_>,
    id_namespace: &str,
    options: &Binding,
    value: &Binding,
    orientation: Orientation,
) -> String {
    let opts = resolve_options(ctx.sources, options);
    let opts: Vec<&SelectOption> = opts.iter().filter(|o| o.value != "<opaque>").collect();
    let current = try_string(ctx.sources, value);
    let option_id = |index: usize| format!("{id_namespace}-opt-{index}");

    if orientation == Orientation::Horizontal {
        let active_index: isize = current
            .as_ref()
            .and_then(|c| opts.iter().position(|o| o.value == *c))
            .map(|i| i as isize)
            .unwrap_or(-1);
        let buttons: String = opts
            .iter()
            .enumerate()
            .map(|(index, o)| {
                let is_active = index as isize == active_index;
                let tab_index = if is_active || (active_index < 0 && index == 0) {
                    "0"
                } else {
                    "-1"
                };
                text_el(
                    "button",
                    &[
                        ("class", s("fuaran-segmented-option")),
                        ("type", s("button")),
                        ("id", s(option_id(index))),
                        ("aria-checked", s(if is_active { "true" } else { "false" })),
                        ("role", s("radio")),
                        ("tabindex", s(tab_index)),
                    ],
                    &render_text(ctx.sources, &o.label),
                )
            })
            .collect();
        return el(
            "div",
            &[
                ("class", s("fuaran-segmented-horizontal")),
                ("id", s(id_namespace)),
                ("role", s("radiogroup")),
                ("aria-orientation", s("horizontal")),
            ],
            &buttons,
        );
    }

    let legend = text_el(
        "legend",
        &[("class", s("fuaran-segmented-legend"))],
        id_namespace,
    );
    let rows: String = opts
        .iter()
        .enumerate()
        .map(|(index, o)| {
            let input_id = option_id(index);
            let radio = void_el(
                "input",
                &[
                    ("type", s("radio")),
                    ("id", s(input_id.clone())),
                    ("name", s(id_namespace)),
                    ("value", s(o.value.clone())),
                    (
                        "checked",
                        AttrVal::Flag(current.as_deref() == Some(o.value.as_str())),
                    ),
                ],
            );
            let label = text_el(
                "label",
                &[("for", s(input_id))],
                &render_text(ctx.sources, &o.label),
            );
            el(
                "div",
                &[("class", s("fuaran-segmented-row"))],
                &format!("{radio}{label}"),
            )
        })
        .collect();
    el(
        "fieldset",
        &[
            ("class", s("fuaran-segmented-vertical")),
            ("aria-orientation", s("vertical")),
        ],
        &format!("{legend}{rows}"),
    )
}

fn render_filters(ctx: &Ctx<'_>, specs: &[FilterSpec]) -> String {
    el(
        "div",
        &[("class", s("fuaran-filters"))],
        &specs
            .iter()
            .map(|spec| render_filter(ctx, spec))
            .collect::<String>(),
    )
}

fn render_filter(ctx: &Ctx<'_>, spec: &FilterSpec) -> String {
    // 0.2.0 filters-unification: the chip's control is an ordinary
    // FormFieldKind. The four legacy chip shapes keep their `fuaran-filter-*`
    // class hooks; any other control renders through the shared form-control
    // path keyed by the chip name.
    let label_text = render_text(ctx.sources, &spec.label);
    let control = match &spec.kind {
        FormFieldKind::Text { value, .. } => {
            let current = try_string(ctx.sources, value).unwrap_or_default();
            void_el(
                "input",
                &[
                    ("class", s("fuaran-filter-input")),
                    ("type", s("text")),
                    ("placeholder", s(label_text.clone())),
                    ("value", s(current)),
                ],
            )
        }
        FormFieldKind::Choice { options, .. } => {
            let opts = resolve_options(ctx.sources, options);
            let options_html = format!(
                "{}{}",
                text_el("option", &[("value", s(""))], EM_DASH),
                opts.iter()
                    .filter(|o| o.value != "<opaque>")
                    .map(|o| text_el(
                        "option",
                        &[("value", s(o.value.clone()))],
                        &render_text(ctx.sources, &o.label),
                    ))
                    .collect::<String>()
            );
            el(
                "select",
                &[("class", s("fuaran-filter-select"))],
                &options_html,
            )
        }
        FormFieldKind::Range { value, .. } => {
            let (min, max) = resolve_float_pair(ctx.sources, value).unwrap_or((0.0, 0.0));
            el(
                "span",
                &[("class", s("fuaran-filter-range"))],
                &format!(
                    "{}{}{}",
                    void_el(
                        "input",
                        &[
                            ("type", s("number")),
                            ("class", s("fuaran-filter-range-min")),
                            ("value", s(display_number(min))),
                        ],
                    ),
                    text_el("span", &[("class", s("fuaran-filter-range-sep"))], "–"),
                    void_el(
                        "input",
                        &[
                            ("type", s("number")),
                            ("class", s("fuaran-filter-range-max")),
                            ("value", s(display_number(max))),
                        ],
                    )
                ),
            )
        }
        FormFieldKind::SegmentedChoice {
            options,
            orientation,
            value,
            ..
        } => render_segmented_choice(ctx, &spec.name, options, value, *orientation),
        other => {
            let synthetic = FormField {
                id: spec.name.clone(),
                kind: other.clone(),
                label: spec.label.clone(),
                required: false,
                help: None,
            };
            render_form_control(ctx, &synthetic)
        }
    };
    el(
        "label",
        &[("class", s("fuaran-filter"))],
        &format!(
            "{}{control}",
            text_el("span", &[("class", s("fuaran-filter-label"))], &label_text)
        ),
    )
}

// ─── Visualisations ──────────────────────────────────────────────────────────

/// The projected cell value of a column against a row (the row-field
/// projection contract): the closure wins when present — on a decoded tree it
/// is an inert placeholder producing the empty cell; else the declarative
/// `field` projects the row property; else the cell is empty.
enum CellVal {
    Num(f64),
    Str(String),
    Bool(bool),
    Empty,
}

fn project_cell(col: &ColumnErased, row: &JVal) -> CellVal {
    if col.value.is_some() {
        return CellVal::Empty;
    }
    let Some(field) = &col.field else {
        return CellVal::Empty;
    };
    match row.field(field) {
        Some(JVal::Str(v)) => CellVal::Str(v.clone()),
        Some(JVal::Bool(v)) => CellVal::Bool(*v),
        Some(JVal::Num(v)) => CellVal::Num(*v),
        _ => CellVal::Empty,
    }
}

/// Phase 750 — lower a `CellKindErased::TonedPill` for one row: the named field's
/// text IS the pill's label, and its tone is the map's entry for that text, or
/// `default_tone` for a value the map does not mention.
///
/// The whole of the declarative pill's semantics, in one function, because the same
/// lookup-with-fallback is what every host renders — and a per-surface copy of it is
/// exactly how two hosts come to disagree about an *unmapped* value, which is the
/// case a parity test misses most easily.
///
/// Keyed on the row-field projection's canonical text (`CellFormat::None`), not the
/// column's own display format: the map's keys are the author's raw data values, so
/// running them through a currency or percent format would key on a rendering rather
/// than on the datum. Parity-locked with the reference hosts' `tonedPillOf`.
fn toned_pill_of(
    row: &JVal,
    field: &str,
    map: &BTreeMap<String, ToneVariant>,
    default_tone: ToneVariant,
) -> (String, ToneVariant) {
    let label = cell_value_text(
        &CellFormat::None,
        &match row.field(field) {
            Some(JVal::Str(v)) => CellVal::Str(v.clone()),
            Some(JVal::Bool(v)) => CellVal::Bool(*v),
            Some(JVal::Num(v)) => CellVal::Num(*v),
            _ => CellVal::Empty,
        },
    );
    let tone = map.get(&label).copied().unwrap_or(default_tone);
    (label, tone)
}

fn cell_value_text(format: &CellFormat, value: &CellVal) -> String {
    match value {
        CellVal::Num(v) => format_number(format, *v),
        CellVal::Str(v) => v.clone(),
        CellVal::Bool(v) => if *v { "true" } else { "false" }.to_string(),
        CellVal::Empty => String::new(),
    }
}

fn render_grid_cell(ctx: &Ctx<'_>, col: &ColumnErased, row: &JVal) -> String {
    let value = project_cell(col, row);
    match &col.kind {
        CellKindErased::Text | CellKindErased::Numeric | CellKindErased::Date => {
            text_el("span", &[], &cell_value_text(&col.format, &value))
        }
        CellKindErased::Editable => match &value {
            CellVal::Num(v) => void_el(
                "input",
                &[
                    ("class", s("fuaran-grid-cell-editable")),
                    ("type", s("number")),
                    ("value", s(display_number(*v))),
                ],
            ),
            CellVal::Str(v) => void_el(
                "input",
                &[
                    ("class", s("fuaran-grid-cell-editable")),
                    ("type", s("text")),
                    ("value", s(v.clone())),
                ],
            ),
            CellVal::Bool(_) | CellVal::Empty => {
                text_el("span", &[], &cell_value_text(&col.format, &value))
            }
        },
        // A decoded Checkbox's `get` placeholder reads false.
        CellKindErased::Checkbox => void_el(
            "input",
            &[("type", s("checkbox")), ("checked", AttrVal::Flag(false))],
        ),
        CellKindErased::Button { label } => text_el(
            "button",
            &[("class", s("fuaran-grid-cell-button"))],
            &render_text(ctx.sources, label),
        ),
        CellKindErased::ButtonGroup { labels } => el(
            "span",
            &[("class", s("fuaran-grid-cell-button-group"))],
            &labels
                .iter()
                .map(|label| {
                    text_el(
                        "button",
                        &[("class", s("fuaran-grid-cell-button"))],
                        &render_text(ctx.sources, label),
                    )
                })
                .collect::<String>(),
        ),
        // Decoded Link/Pill placeholders produce the "<closure>" sentinel —
        // rendered faithfully (the decoded-tree parity behaviour).
        CellKindErased::Link => text_el(
            "a",
            &[
                ("class", s("fuaran-grid-cell-link")),
                ("href", s(sanitize_url_or_blank("<closure>"))),
            ],
            "<closure>",
        ),
        CellKindErased::Pill => text_el(
            "span",
            &[("class", s("fuaran-grid-cell-pill fuaran-pill-default"))],
            "<closure>",
        ),
        // Phase 750 — the declarative twin, and the only cell kind this host renders
        // with real per-row content on a decoded tree. Deliberately the SAME element
        // and class vocabulary as the closure `Pill` arm above: the wire variant
        // exists to make the tone rule *expressible*, not to render differently.
        CellKindErased::TonedPill {
            field,
            map,
            default_tone,
        } => {
            let (label, tone) = toned_pill_of(row, field, map, *default_tone);
            text_el(
                "span",
                &[(
                    "class",
                    s(format!(
                        "fuaran-grid-cell-pill fuaran-pill-{}",
                        tone_var(tone)
                    )),
                )],
                &label,
            )
        }
        CellKindErased::Progress => {
            // The decoded fraction placeholder reads 0.
            let fill = el(
                "div",
                &[
                    ("class", s("fuaran-grid-cell-progress-fill")),
                    ("style", s("width:0%")),
                ],
                "",
            );
            el("div", &[("class", s("fuaran-grid-cell-progress"))], &fill)
        }
        CellKindErased::Custom => {
            // The decoded custom cell renders the closure-placeholder node.
            let placeholder = Node {
                id: "<closure>".to_string(),
                kind: NodeKind::Markdown(crate::wire::MarkdownSpec {
                    text: TextSource::Literal("<closure>".to_string()),
                }),
                state: StateBehaviour::default(),
                style: crate::wire::SemanticStyle::default(),
                accessibility: None,
            };
            render_node(ctx, &placeholder)
        }
    }
}

fn render_grid(ctx: &Ctx<'_>, state: &StateBehaviour, spec: &GridSpec) -> String {
    let rows: std::borrow::Cow<'_, [JVal]> = match resolve_rows(ctx.sources, &spec.source) {
        ResolvedRows::NotResolved => {
            if let Some(loading) = &state.on_loading {
                return render_node(ctx, loading);
            }
            std::borrow::Cow::Borrowed(&[])
        }
        ResolvedRows::Rows(rows) => rows,
    };
    let rows: &[JVal] = &rows;
    if rows.is_empty()
        && let Some(empty) = &state.on_empty
    {
        return render_node(ctx, empty);
    }
    // Phase 818 — `sortStateKey`: a host that seeds the named State key with a
    // `{column, direction}` descriptor gets its resolved rows sorted by the
    // addressed column's `field` before rendering (runtime-side sort — the
    // author wires no Transform). No seeded descriptor (the SSR default), a
    // malformed descriptor, an out-of-range column, or a field-less closure
    // column leaves the authored order standing. The interactive affordance
    // (`data-sortable` / live `aria-sort` on the headers) is a client-runtime
    // surface this inert renderer deliberately does not advertise — a table
    // never advertises an interaction it cannot perform.
    let sorted: Option<Vec<JVal>> = grid_sorted_rows(ctx, spec, rows);
    let rows: &[JVal] = sorted.as_deref().unwrap_or(rows);
    let header_cells: String = spec
        .columns
        .iter()
        .map(|col| text_el("th", &[("class", s("fuaran-grid-header"))], &col.label))
        .collect();
    let head = el("thead", &[], &el("tr", &[], &header_cells));
    let body_rows: String = rows
        .iter()
        .map(|row| {
            let cells: String = spec
                .columns
                .iter()
                .map(|col| {
                    el(
                        "td",
                        &[("class", s("fuaran-grid-cell"))],
                        &render_grid_cell(ctx, col, row),
                    )
                })
                .collect();
            el("tr", &[("class", s("fuaran-grid-row"))], &cells)
        })
        .collect();
    let body = el("tbody", &[], &body_rows);
    el(
        "table",
        &[("class", s("fuaran-grid"))],
        &format!("{head}{body}"),
    )
}

/// Phase 818 — read the `{column, direction}` sort descriptor a `sortStateKey`
/// grid's State key carries and sort the resolved rows by the addressed
/// column's `field`. Every part of the descriptor is validated rather than
/// trusted — anything malformed reads as "no sort" so the authored order
/// stands (never an arbitrary one). Empty / missing cells sort LAST in both
/// directions (unmeasured is not zero); ties keep their authored relative
/// order (the sort is stable); string comparison is ordinal over the
/// lower-cased forms — the reference host's `sortRowsByDescriptor` rules.
fn grid_sorted_rows(ctx: &Ctx<'_>, spec: &GridSpec, rows: &[JVal]) -> Option<Vec<JVal>> {
    let sort_key = spec.sort_state_key.as_ref()?;
    let descriptor = ctx.sources.state.get(sort_key)?;
    let fields = match descriptor {
        JVal::Obj(fields) => fields,
        _ => return None,
    };
    let column = fields.iter().find_map(|(k, v)| match (k.as_str(), v) {
        ("column", JVal::Num(n)) if n.fract() == 0.0 && *n >= 0.0 => Some(*n as usize),
        _ => None,
    })?;
    let ascending = fields.iter().find_map(|(k, v)| match (k.as_str(), v) {
        ("direction", JVal::Str(d)) if d == "asc" => Some(true),
        ("direction", JVal::Str(d)) if d == "desc" => Some(false),
        _ => None,
    })?;
    let field = spec.columns.get(column)?.field.as_ref()?;
    let cell = |row: &JVal| -> Option<JVal> { row.field(field).cloned() };
    let rank = |v: &Option<JVal>| -> u8 {
        match v {
            Some(JVal::Num(_)) => 0,
            Some(JVal::Bool(_)) => 1,
            Some(JVal::Str(_)) => 2,
            _ => 3, // null / missing / structured — sorts last in both directions
        }
    };
    let mut out: Vec<JVal> = rows.to_vec();
    out.sort_by(|a, b| {
        use std::cmp::Ordering;
        let (ka, kb) = (cell(a), cell(b));
        let (ra, rb) = (rank(&ka), rank(&kb));
        // Empty cells last in BOTH directions — outside the direction flip.
        if ra == 3 || rb == 3 {
            return ra.cmp(&rb);
        }
        let ord = match (&ka, &kb) {
            (Some(JVal::Num(x)), Some(JVal::Num(y))) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
            (Some(JVal::Bool(x)), Some(JVal::Bool(y))) => x.cmp(y),
            (Some(JVal::Str(x)), Some(JVal::Str(y))) => x.to_lowercase().cmp(&y.to_lowercase()),
            _ => ra.cmp(&rb),
        };
        if ascending { ord } else { ord.reverse() }
    });
    Some(out)
}

/// The static read-only table leg, driven by a grid's `staticRows`.
fn render_static_table(ctx: &Ctx<'_>, spec: &StaticRows) -> String {
    let header_cells: String = spec
        .headers
        .iter()
        .map(|h| {
            text_el(
                "th",
                &[("class", s("fuaran-table-header"))],
                &render_text(ctx.sources, h),
            )
        })
        .collect();
    let head = el("thead", &[], &el("tr", &[], &header_cells));
    let body_rows: String = spec
        .rows
        .iter()
        .map(|row| {
            let cells: String = row
                .iter()
                .map(|cell| {
                    text_el(
                        "td",
                        &[("class", s("fuaran-table-cell"))],
                        &render_text(ctx.sources, cell),
                    )
                })
                .collect();
            el("tr", &[("class", s("fuaran-table-row"))], &cells)
        })
        .collect();
    let body = el("tbody", &[], &body_rows);
    // Phase 801 — the declared sort intent as data attributes, so a
    // progressive-enhancement script honours it without re-parsing the wire. Emitted
    // ONLY when declared (an undeclared table's bytes are unchanged), and in the same
    // order as the F# / TS / Python / Go renderers so the markup stays parity-locked.
    let mut attrs = vec![("class", s("fuaran-table"))];
    if let Some(sortable) = spec.sortable {
        attrs.push((
            "data-fuaran-sortable",
            s(if sortable { "true" } else { "false" }),
        ));
    }
    if let Some(ds) = &spec.default_sort {
        attrs.push(("data-fuaran-sort-column", s(ds.column.to_string())));
        attrs.push(("data-fuaran-sort-direction", s(ds.direction.as_str())));
    }
    el("table", &attrs, &format!("{head}{body}"))
}

// Chart-lowering posture (Phase 551): fuaran-rs is LOWER-IN-HOST. A raw Chart is
// lowered deterministically to a canonical Drawing (`super::chart_lowering::lower_chart`,
// a byte-identical port of the F# reference, pinned by tests/chart_lowering.rs) and
// rendered as first-party inline SVG through the shared Drawing renderer. Because
// the WASM browser client renders through THIS renderer, lowering here brings the
// client to parity with the Chart-as-data demo — the reason this dual-role host
// lowers where the headless fuaran-go host takes the require-pre-lowered posture.
// Lowered arms: Bar (grouped + stacked), Line, Area (overlaid + stacked),
// Scatter, Pie; an un-lowered kind (Heatmap) yields an empty (but titled)
// drawing region, never a silent blank. When the source binding has not yet
// resolved to rows, the on-loading placeholder (or an empty drawing) shows — the
// same not-resolved discipline every data-bound kind follows.
fn render_chart(ctx: &Ctx<'_>, state: &StateBehaviour, spec: &ChartSpec) -> String {
    let rows: std::borrow::Cow<'_, [JVal]> = match resolve_rows(ctx.sources, &spec.source) {
        ResolvedRows::NotResolved => {
            if let Some(loading) = &state.on_loading {
                return render_node(ctx, loading);
            }
            std::borrow::Cow::Borrowed(&[])
        }
        ResolvedRows::Rows(rows) => rows,
    };
    let rows: &[JVal] = &rows;
    if rows.is_empty()
        && let Some(empty) = &state.on_empty
    {
        return render_node(ctx, empty);
    }
    let lowered_rows: Vec<super::chart_lowering::LowerRow> = rows
        .iter()
        .map(|row| super::chart_lowering::project_row(row, &spec.x_field, &spec.y_fields))
        .collect();
    // Phase 876 — the declared value-axis number format travels with the spec
    // into the lowering (the style stays the host's default).
    let drawing = super::chart_lowering::lower_chart_with(
        spec.kind,
        spec.stacked,
        &spec.x_field,
        &spec.y_fields,
        spec.title.as_ref(),
        // Phase 878 — the author's axis names + subtitle travel with the spec;
        // the field-name fallback is the lowering's, not the renderer's.
        &super::chart_lowering::ChartTitles {
            x_title: spec.x_title.as_ref(),
            y_title: spec.y_title.as_ref(),
            subtitle: spec.subtitle.as_ref(),
        },
        spec.value_format.as_ref(),
        &super::chart_lowering::ChartLowerStyle::default(),
        &lowered_rows,
    );
    // Render the lowered Drawing through the shared Drawing renderer (inline SVG),
    // wrapped in the chart class so host CSS + the demo target it consistently.
    el(
        "div",
        &[("class", s("fuaran-chart"))],
        &render_drawing(ctx, &drawing),
    )
}

fn render_map(ctx: &Ctx<'_>, state: &StateBehaviour, spec: &MapSpec) -> String {
    // The typed Static marker payload or a source-supplied array.
    let markers: Vec<(String, f64, f64)> = match super::bindings::resolve(ctx.sources, &spec.source)
    {
        super::bindings::Resolution::Resolved(super::bindings::Value::Static(
            StaticValue::Markers(markers),
        )) => markers
            .iter()
            .map(|m| (render_text(ctx.sources, &m.label), m.latitude, m.longitude))
            .collect(),
        super::bindings::Resolution::Resolved(super::bindings::Value::Json(JVal::Arr(items))) => {
            items
                .iter()
                .filter_map(|item| {
                    let label = match item.field("label") {
                        Some(JVal::Str(v)) => v.clone(),
                        _ => return None,
                    };
                    let lat = match item.field("latitude") {
                        Some(JVal::Num(v)) => *v,
                        _ => return None,
                    };
                    let lng = match item.field("longitude") {
                        Some(JVal::Num(v)) => *v,
                        _ => return None,
                    };
                    Some((label, lat, lng))
                })
                .collect()
        }
        super::bindings::Resolution::NotResolved => {
            if let Some(loading) = &state.on_loading {
                return render_node(ctx, loading);
            }
            vec![]
        }
        _ => vec![],
    };
    let placeholder = text_el(
        "div",
        &[("class", s("fuaran-map-placeholder"))],
        &format!(
            "[Map placeholder: {} markers around ({:.4}, {:.4}) zoom {}. Wire a Leaflet adapter for live rendering.]",
            markers.len(),
            spec.centre_latitude,
            spec.centre_longitude,
            spec.zoom
        ),
    );
    let list = if markers.is_empty() {
        String::new()
    } else {
        el(
            "ul",
            &[("class", s("fuaran-map-marker-list"))],
            &markers
                .iter()
                .map(|(label, lat, lng)| {
                    text_el(
                        "li",
                        &[("class", s("fuaran-map-marker"))],
                        &format!("{label} @ ({lat:.4}, {lng:.4})"),
                    )
                })
                .collect::<String>(),
        )
    };
    el(
        "div",
        &[("class", s("fuaran-map"))],
        &format!("{placeholder}{list}"),
    )
}

// ─── Fragment reference expansion ────────────────────────────────────────────

fn render_fragment_ref(ctx: &Ctx<'_>, parent_node_id: &str, name: &str) -> String {
    if ctx.expanding.borrow().contains(name) {
        return text_el(
            "div",
            &[
                ("class", s("fuaran-fragment-cycle-placeholder")),
                ("data-fuaran-fragment-cycle", s(name)),
            ],
            &format!("[fuaran:fragment cycle '{name}']"),
        );
    }
    let Some(body) = ctx.fragments.get(name) else {
        return text_el(
            "div",
            &[
                ("class", s("fuaran-fragment-unresolved-placeholder")),
                ("data-fuaran-fragment-unresolved", s(name)),
            ],
            &format!("[fuaran:fragment unresolved '{name}']"),
        );
    };
    let prefix = format!("{parent_node_id}.");
    let namespaced = namespace_node(&prefix, body);
    ctx.expanding.borrow_mut().insert(name.to_string());
    let rendered = render_node(ctx, &namespaced);
    ctx.expanding.borrow_mut().remove(name);
    rendered
}

// ─── Public entry points ─────────────────────────────────────────────────────

/// Render a typed [`Node`] tree to a body-fragment HTML string. The host owns
/// the document shell + the `<link>` to the reference CSS (`css/fuaran.css`).
/// With empty `sources`, `Static` bindings resolve and the rest fall back to
/// their loading slot / em-dash placeholder.
pub fn render_to_html(tree: &Node, sources: &BindingSources) -> String {
    let no_islands = HashSet::new();
    let mut fragments = HashMap::new();
    collect_fragments(&mut fragments, tree);
    let ctx = Ctx {
        sources,
        fragments,
        islands: &no_islands,
        expanding: std::cell::RefCell::new(HashSet::new()),
    };
    render_node(&ctx, tree)
}

/// Escape canonical wire JSON for safe embedding inside a `<script>` element:
/// `<` / `>` / `&` become their `\uXXXX` escapes so a `</script>` substring in
/// string data cannot break out; JSON parsers decode them back, so the tree
/// round-trips unchanged.
fn escape_for_script(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// The id of the embedded whole-tree hydrate `<script>` for a render root.
pub fn hydrate_script_id(root_id: &str) -> String {
    format!("fuaran-hydrate-{root_id}")
}

/// The id of an island's embedded hydrate `<script>` (distinct from the
/// whole-tree id, so both can coexist on one page).
pub fn island_script_id(island_id: &str) -> String {
    format!("fuaran-hydrate-island-{island_id}")
}

/// Render a **hydration-ready** HTML string: the body-fragment HTML plus the
/// embedded canonical wire tree as a `<script type="application/json">`
/// payload a conformant client decodes and attaches against the server DOM.
pub fn render_hydratable(tree: &Node, sources: &BindingSources) -> String {
    let html = render_to_html(tree, sources);
    let json = escape_for_script(&encode_node(tree));
    let script = el(
        "script",
        &[
            ("type", s("application/json")),
            ("id", s(hydrate_script_id(&tree.id))),
            ("data-fuaran-hydrate-root", s(tree.id.clone())),
        ],
        &json,
    );
    format!("{html}{script}")
}

/// DFS-collect the designated island nodes in document order.
fn collect_islands<'a>(node: &'a Node, islands: &HashSet<String>, out: &mut Vec<&'a Node>) {
    if islands.contains(&node.id) {
        out.push(node);
    }
    for child in island_children(node) {
        collect_islands(child, islands, out);
    }
}

fn island_children(node: &Node) -> Vec<&Node> {
    match &node.kind {
        NodeKind::Box(spec) => spec.children.iter().collect(),
        NodeKind::SplitPanel(spec) => spec.children.iter().collect(),
        NodeKind::Tabs(spec) => spec.children.iter().collect(),
        NodeKind::Stepper(spec) => spec.children.iter().collect(),
        NodeKind::SummaryList(spec) => spec.children.iter().collect(),
        NodeKind::Disclosure(spec) => spec.children.iter().collect(),
        NodeKind::Modal(spec) => spec.children.iter().collect(),
        NodeKind::ScrollArea(spec) => spec.children.iter().collect(),
        NodeKind::ErrorBoundary(spec) => vec![&spec.child],
        NodeKind::FragmentDecl(spec) => vec![&spec.body],
        _ => vec![],
    }
}

/// Render the page statically with per-island boundary markers (each
/// designated subtree wrapped in `<div data-fuaran-island="<id>">`) plus one
/// embedded hydrate `<script>` per island carrying that subtree's canonical
/// wire JSON. A page with **zero** islands returns exactly
/// [`render_to_html`] — no wrappers, no scripts.
pub fn render_with_islands(tree: &Node, sources: &BindingSources, island_ids: &[&str]) -> String {
    let islands: HashSet<String> = island_ids.iter().map(|id| id.to_string()).collect();
    let mut fragments = HashMap::new();
    collect_fragments(&mut fragments, tree);
    let ctx = Ctx {
        sources,
        fragments,
        islands: &islands,
        expanding: std::cell::RefCell::new(HashSet::new()),
    };
    let static_html = render_node(&ctx, tree);

    let mut island_nodes: Vec<&Node> = Vec::new();
    collect_islands(tree, &islands, &mut island_nodes);
    let scripts: String = island_nodes
        .iter()
        .map(|island| {
            let json = escape_for_script(&encode_node(island));
            el(
                "script",
                &[
                    ("type", s("application/json")),
                    ("id", s(island_script_id(&island.id))),
                    ("data-fuaran-island-payload", s(island.id.clone())),
                ],
                &json,
            )
        })
        .collect();
    format!("{static_html}{scripts}")
}
