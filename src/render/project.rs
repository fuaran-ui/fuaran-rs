//! The resolved projection (Phase 650): a tree rewrite that folds render-time
//! Transform resolution into the wire tree itself, so a **decode-only** consumer
//! (a native render surface over this core) renders already-resolved compute
//! values without carrying an evaluator.
//!
//! The core already resolves `Binding.Transform` at render time (Phase 649,
//! `render::bindings`). But a consumer that reads the session's *tree* JSON
//! (rather than the rendered HTML) sees the raw, unresolved `Transform` in every
//! scalar slot — and, having no evaluator, renders it empty. This module walks
//! the tree and, at exactly the scalar slots the renderer resolves through the
//! Phase 632/649 1×1 law, replaces a `Binding.Transform` with the concrete value
//! it evaluates to:
//!
//!   • every `TextSource::Bound(Transform)` → `TextSource::Literal(resolved)`
//!     (the [`render_text`](super::bindings::render_text) Bound arm — Badge
//!     label, Callout body, Fact value, Heading/Markdown text, List items, …);
//!   • the numeric scalar slots (`Metric.value` / `Metric.trend`,
//!     `LabelValueRow.value`) → `Binding::Static` carrying the resolved number,
//!     so the consumer still applies the slot's `CellFormat`.
//!
//! The rewrite is **deliberately scalar-only**. Row-context Transforms
//! (`DataGrid` / `Chart` / `Map` / `Sparkline` sources) resolve to *collections*,
//! which the wire's `Static` slot erases to `"<opaque>"` (§2 rule 11) — they
//! cannot ride a projected literal and are left as `Transform`. Every
//! non-`Transform` binding (`Static` / `State` / `Filter` / `Selection` / `Query`
//! / …) is left byte-identical, so a consumer's own reactive/selection seeding
//! and write-back interaction model are untouched: the projection resolves *only*
//! what a decode-only consumer cannot, and nothing else.
//!
//! A scalar `Transform` that does not cleanly resolve (ambiguous, errored, or
//! not-yet-resolvable) is left in place — the projection never fabricates a
//! value; the consumer renders its own unresolved floor there, exactly as it
//! would for any other unresolved binding.

use crate::canonical::JVal;
use crate::render::BindingSources;
use crate::render::bindings::{try_scalar_number, try_scalar_string};
use crate::wire::{
    Binding, BoxSpec, CalloutSpec, ChartSpec, DisclosureSpec, DrawingSpec, ErrorBoundarySpec,
    FactSpec, FilterSpec, FormField, FormSpec, FragmentArg, FragmentDeclSpec, FragmentRefSpec,
    GridSpec, HeadingSpec, LabelValueRowSpec, LinkSpec, ListSpec, MarkdownSpec, MetricSpec,
    ModalSpec, MountSpec, Node, NodeKind, ProgressSpec, ScrollAreaSpec, SelectSpec, Shape,
    SplitPanelSpec, StaticRows, StaticValue, StepperSpec, SummaryListSpec, SwitchSpec, TabHeader,
    TabsSpec, TextSource, ToastSpec,
};

/// Project a resolved copy of `tree` against the live `sources` — the entry
/// point [`ClientSession::project_resolved`](crate::client::ClientSession::project_resolved)
/// encodes.
pub fn project_resolved(sources: &BindingSources, tree: &Node) -> Node {
    project_node(sources, tree)
}

fn project_node(sources: &BindingSources, node: &Node) -> Node {
    Node {
        id: node.id.clone(),
        kind: project_kind(sources, &node.kind),
        state: project_state(sources, &node.state),
        style: node.style,
        accessibility: node.accessibility.clone(),
    }
}

fn project_state(
    sources: &BindingSources,
    state: &crate::wire::StateBehaviour,
) -> crate::wire::StateBehaviour {
    crate::wire::StateBehaviour {
        on_loading: state
            .on_loading
            .as_ref()
            .map(|n| Box::new(project_node(sources, n))),
        on_empty: state
            .on_empty
            .as_ref()
            .map(|n| Box::new(project_node(sources, n))),
        on_error: state.on_error,
    }
}

/// Resolve a `TextSource::Bound(Transform)` to a literal; every other text source
/// (including `Bound` over a non-Transform binding) passes through unchanged.
fn map_text(sources: &BindingSources, text: &TextSource) -> TextSource {
    match text {
        TextSource::Bound(binding) if matches!(**binding, Binding::Transform { .. }) => {
            TextSource::Literal(try_scalar_string(sources, binding).unwrap_or_default())
        }
        other => other.clone(),
    }
}

fn map_opt_text(sources: &BindingSources, text: &Option<TextSource>) -> Option<TextSource> {
    text.as_ref().map(|t| map_text(sources, t))
}

/// Resolve a numeric scalar-slot `Binding::Transform` to a `Static` number so the
/// consumer still applies the slot's format; a Transform that does not cleanly
/// resolve, and every non-Transform binding, passes through unchanged.
fn map_scalar_number(sources: &BindingSources, binding: &Binding) -> Binding {
    match binding {
        Binding::Transform { .. } => match try_scalar_number(sources, binding) {
            Some(n) => Binding::Static {
                value: StaticValue::Ast(JVal::Num(n)),
            },
            None => binding.clone(),
        },
        other => other.clone(),
    }
}

fn map_opt_scalar_number(sources: &BindingSources, binding: &Option<Binding>) -> Option<Binding> {
    binding.as_ref().map(|b| map_scalar_number(sources, b))
}

fn project_children(sources: &BindingSources, nodes: &[Node]) -> Vec<Node> {
    nodes.iter().map(|n| project_node(sources, n)).collect()
}

fn project_kind(sources: &BindingSources, kind: &NodeKind) -> NodeKind {
    match kind {
        // ── Layout (recurse children; map headings) ──────────────────────────
        NodeKind::Box(spec) => NodeKind::Box(BoxSpec {
            children: project_children(sources, &spec.children),
            heading: map_opt_text(sources, &spec.heading),
            layout: spec.layout.clone(),
            role: spec.role,
        }),
        NodeKind::SplitPanel(spec) => NodeKind::SplitPanel(SplitPanelSpec {
            children: project_children(sources, &spec.children),
            weight: spec.weight,
        }),
        NodeKind::Tabs(spec) => NodeKind::Tabs(TabsSpec {
            children: project_children(sources, &spec.children),
            orientation: spec.orientation,
            active_index: spec.active_index.clone(),
            on_select: spec.on_select,
            tab_headers: spec.tab_headers.as_ref().map(|hs| {
                hs.iter()
                    .map(|h| TabHeader {
                        label: map_text(sources, &h.label),
                        icon: h.icon.clone(),
                        disabled: h.disabled.clone(),
                    })
                    .collect()
            }),
            tab_tags: spec.tab_tags.clone(),
            active_tag: spec.active_tag.clone(),
            on_select_tag: spec.on_select_tag,
        }),
        NodeKind::Stepper(spec) => NodeKind::Stepper(StepperSpec {
            active_step: spec.active_step.clone(),
            children: project_children(sources, &spec.children),
        }),
        NodeKind::SummaryList(spec) => NodeKind::SummaryList(SummaryListSpec {
            children: project_children(sources, &spec.children),
            heading: map_opt_text(sources, &spec.heading),
        }),
        NodeKind::Disclosure(spec) => NodeKind::Disclosure(DisclosureSpec {
            children: project_children(sources, &spec.children),
            default_open: spec.default_open,
            heading: map_text(sources, &spec.heading),
            open: spec.open.clone(),
            on_toggle: spec.on_toggle,
        }),
        NodeKind::Modal(spec) => NodeKind::Modal(ModalSpec {
            children: project_children(sources, &spec.children),
            dismissable: spec.dismissable,
            open: spec.open.clone(),
            on_dismiss: spec.on_dismiss.clone(),
            heading: map_opt_text(sources, &spec.heading),
        }),
        NodeKind::ScrollArea(spec) => NodeKind::ScrollArea(ScrollAreaSpec {
            children: project_children(sources, &spec.children),
            orientation: spec.orientation,
            max_height: spec.max_height,
            max_width: spec.max_width,
        }),
        // ── Display (map scalar text / numeric slots) ────────────────────────
        NodeKind::Heading(spec) => NodeKind::Heading(HeadingSpec {
            level: spec.level,
            text: map_text(sources, &spec.text),
            variant: spec.variant,
        }),
        NodeKind::Markdown(spec) => NodeKind::Markdown(MarkdownSpec {
            text: map_text(sources, &spec.text),
        }),
        NodeKind::Metric(spec) => NodeKind::Metric(MetricSpec {
            label: map_text(sources, &spec.label),
            value: map_scalar_number(sources, &spec.value),
            format: spec.format.clone(),
            tone: spec.tone,
            weight: spec.weight,
            emphasis: spec.emphasis,
            trend: map_opt_scalar_number(sources, &spec.trend),
            trend_format: spec.trend_format.clone(),
            icon: spec.icon.clone(),
            subtext: map_opt_text(sources, &spec.subtext),
        }),
        NodeKind::Badge(spec) => NodeKind::Badge(crate::wire::BadgeSpec {
            label: map_text(sources, &spec.label),
            variant: spec.variant,
        }),
        // Sparkline.source is a float-sequence (collection) slot — left as-is.
        NodeKind::Sparkline(spec) => NodeKind::Sparkline(spec.clone()),
        NodeKind::Callout(spec) => NodeKind::Callout(CalloutSpec {
            body: map_text(sources, &spec.body),
            dismissable: spec.dismissable,
            tone: spec.tone,
            heading: map_opt_text(sources, &spec.heading),
            icon: spec.icon.clone(),
        }),
        NodeKind::Progress(spec) => NodeKind::Progress(ProgressSpec {
            fraction: spec.fraction.clone(),
            indeterminate: spec.indeterminate,
            tone: spec.tone,
            label: map_opt_text(sources, &spec.label),
            caveat: map_opt_text(sources, &spec.caveat),
        }),
        NodeKind::Skeleton(spec) => NodeKind::Skeleton(spec.clone()),
        // Icon carries only scalar fields — no TextSource / scalar-number slot
        // to resolve.
        NodeKind::Icon(spec) => NodeKind::Icon(spec.clone()),
        NodeKind::LabelValueRow(spec) => NodeKind::LabelValueRow(LabelValueRowSpec {
            label: map_text(sources, &spec.label),
            value: map_scalar_number(sources, &spec.value),
            format: spec.format.clone(),
            emphasis: spec.emphasis,
            help: map_opt_text(sources, &spec.help),
        }),
        NodeKind::Fact(spec) => NodeKind::Fact(FactSpec {
            label: map_text(sources, &spec.label),
            // Fact.value is a TextSource — a Bound(Transform) resolves here too.
            value: map_text(sources, &spec.value),
            emphasis: spec.emphasis,
            tone: spec.tone,
            help: map_opt_text(sources, &spec.help),
            icon: spec.icon.clone(),
        }),
        NodeKind::Link(spec) => NodeKind::Link(LinkSpec {
            href: spec.href.clone(),
            label: map_text(sources, &spec.label),
            download: spec.download,
            rel: spec.rel.clone(),
            target: spec.target.clone(),
            protection: spec.protection,
        }),
        NodeKind::Image(spec) => NodeKind::Image(crate::wire::ImageSpec {
            alt: map_text(sources, &spec.alt),
            src: spec.src.clone(),
            variant: spec.variant,
        }),
        NodeKind::List(spec) => NodeKind::List(ListSpec {
            items: spec.items.iter().map(|t| map_text(sources, t)).collect(),
            ordered: spec.ordered,
        }),
        NodeKind::Toast(spec) => NodeKind::Toast(ToastSpec {
            message: map_text(sources, &spec.message),
            tone: spec.tone,
            open: spec.open.clone(),
            dismissable: spec.dismissable,
        }),
        NodeKind::CodeBlock(spec) => NodeKind::CodeBlock(spec.clone()),
        NodeKind::Math(spec) => NodeKind::Math(spec.clone()),
        NodeKind::Drawing(spec) => NodeKind::Drawing(DrawingSpec {
            view_box: spec.view_box.clone(),
            shapes: spec
                .shapes
                .iter()
                .map(|s| project_shape(sources, s))
                .collect(),
            style: spec.style.clone(),
            title: map_opt_text(sources, &spec.title),
            description: map_opt_text(sources, &spec.description),
        }),
        // ── Input (map labels / prompts) ─────────────────────────────────────
        NodeKind::Form(spec) => NodeKind::Form(FormSpec {
            fields: spec
                .fields
                .iter()
                .map(|f| FormField {
                    id: f.id.clone(),
                    kind: f.kind.clone(),
                    label: map_text(sources, &f.label),
                    required: f.required,
                    help: map_opt_text(sources, &f.help),
                })
                .collect(),
            on_submit: spec.on_submit.clone(),
            submit_label: map_text(sources, &spec.submit_label),
            disabled: spec.disabled.clone(),
        }),
        NodeKind::Filters(specs) => NodeKind::Filters(
            specs
                .iter()
                .map(|f| FilterSpec {
                    kind: f.kind.clone(),
                    label: map_text(sources, &f.label),
                    name: f.name.clone(),
                })
                .collect(),
        ),
        NodeKind::Button(spec) => NodeKind::Button(crate::wire::ButtonSpec {
            label: map_text(sources, &spec.label),
            on_click: spec.on_click.clone(),
            variant: spec.variant,
            icon: spec.icon.clone(),
            disabled: spec.disabled.clone(),
        }),
        NodeKind::FileUpload(spec) => NodeKind::FileUpload(crate::wire::FileUploadSpec {
            accept: spec.accept.clone(),
            label: map_text(sources, &spec.label),
            multiple: spec.multiple,
            disabled: spec.disabled.clone(),
        }),
        NodeKind::Select(spec) => NodeKind::Select(SelectSpec {
            label: map_text(sources, &spec.label),
            source: spec.source.clone(),
            value: spec.value.clone(),
            on_change: spec.on_change,
            placeholder: map_opt_text(sources, &spec.placeholder),
            disabled: spec.disabled.clone(),
            multiple: spec.multiple,
            values: spec.values.clone(),
            on_change_multi: spec.on_change_multi,
        }),
        // ── Visualisation (map chrome text; collection sources left as-is) ───
        NodeKind::DataGrid(spec) => NodeKind::DataGrid(GridSpec {
            columns: spec.columns.clone(),
            editable: spec.editable,
            source: spec.source.clone(),
            on_row_click: spec.on_row_click,
            row_key: spec.row_key,
            row_key_field: spec.row_key_field.clone(),
            // Phase 818 — a state-key name, not text; projects through untouched.
            sort_state_key: spec.sort_state_key.clone(),
            static_rows: spec.static_rows.as_ref().map(|sr| StaticRows {
                headers: sr.headers.iter().map(|t| map_text(sources, t)).collect(),
                rows: sr
                    .rows
                    .iter()
                    .map(|r| r.iter().map(|t| map_text(sources, t)).collect())
                    .collect(),
                // Phase 801 — the sort declaration is not text, so it projects through
                // untouched (this pass maps TextSource chrome only).
                sortable: sr.sortable,
                default_sort: sr.default_sort.clone(),
            }),
        }),
        NodeKind::Chart(spec) => NodeKind::Chart(ChartSpec {
            kind: spec.kind,
            source: spec.source.clone(),
            stacked: spec.stacked,
            x_field: spec.x_field.clone(),
            y_fields: spec.y_fields.clone(),
            title: map_opt_text(sources, &spec.title),
            value_format: spec.value_format.clone(),
            x_title: map_opt_text(sources, &spec.x_title),
            y_title: map_opt_text(sources, &spec.y_title),
            subtitle: map_opt_text(sources, &spec.subtitle),
            // Phase 880 — a placement, not text; projects through untouched.
            legend_position: spec.legend_position,
            // Phase 881 — whether the values are written onto the picture.
            data_labels: spec.data_labels,
            // Phase 882 — a scale declaration, not text; projects through untouched.
            x_scale: spec.x_scale,
            on_point_click: spec.on_point_click,
        }),
        NodeKind::Map(spec) => NodeKind::Map(spec.clone()),
        // ── Structural (recurse subtrees) ────────────────────────────────────
        NodeKind::Custom(spec) => NodeKind::Custom(spec.clone()),
        NodeKind::ErrorBoundary(spec) => NodeKind::ErrorBoundary(ErrorBoundarySpec {
            child: Box::new(project_node(sources, &spec.child)),
            fallback: Box::new(project_node(sources, &spec.fallback)),
        }),
        NodeKind::Switch(spec) => NodeKind::Switch(SwitchSpec {
            on: spec.on.clone(),
            cases: spec
                .cases
                .iter()
                .map(|c| crate::wire::SwitchCase {
                    match_value: c.match_value.clone(),
                    child: project_node(sources, &c.child),
                })
                .collect(),
            default: Box::new(project_node(sources, &spec.default)),
        }),
        NodeKind::FragmentDecl(spec) => NodeKind::FragmentDecl(FragmentDeclSpec {
            name: spec.name.clone(),
            body: Box::new(project_node(sources, &spec.body)),
            holes: spec.holes.clone(),
            effect: spec.effect.clone(),
        }),
        NodeKind::FragmentRef(spec) => NodeKind::FragmentRef(FragmentRefSpec {
            name: spec.name.clone(),
            args: spec
                .args
                .iter()
                .map(|(k, a)| (k.clone(), project_fragment_arg(sources, a)))
                .collect(),
        }),
        NodeKind::Mount(spec) => NodeKind::Mount(MountSpec {
            scope_id: spec.scope_id.clone(),
            inputs: spec
                .inputs
                .iter()
                .map(|(k, a)| (k.clone(), project_fragment_arg(sources, a)))
                .collect(),
            channel: spec.channel.clone(),
            capabilities: spec.capabilities.clone(),
        }),
    }
}

fn project_fragment_arg(sources: &BindingSources, arg: &FragmentArg) -> FragmentArg {
    match arg {
        FragmentArg::Slot { tree } => FragmentArg::Slot {
            tree: Box::new(project_node(sources, tree)),
        },
        FragmentArg::Value(v) => FragmentArg::Value(v.clone()),
    }
}

fn project_shape(sources: &BindingSources, shape: &Shape) -> Shape {
    match shape {
        Shape::Group { children, style } => Shape::Group {
            children: children.iter().map(|c| project_shape(sources, c)).collect(),
            style: style.clone(),
        },
        Shape::Label { x, y, text, style } => Shape::Label {
            x: *x,
            y: *y,
            text: map_text(sources, text),
            style: style.clone(),
        },
        other => other.clone(),
    }
}
