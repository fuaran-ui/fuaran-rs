//! The tree-op apply engine: `apply(tree, op)` reduces a [`TreeOp`] against a
//! [`Node`] tree, returning either the new tree (plus per-leaf-op telemetry) or
//! a structured [`ApplyError`] — total, never a panic, and never a partial
//! mutation (the input tree is untouched on every error path; wrap an op list
//! in `TreeOp::Batch` for all-or-nothing atomicity).
//!
//! Structural child ops (Insert / Remove / Move / Reorder) address the
//! child-bearing layout kinds only; other kinds surface `ChildlessKind` with a
//! hint. `UpdateProp` implements the wire spec's nested-path grammar (§3.4) and
//! the v1 typed-traversal legs; grammar violations surface at apply time as
//! `PathInvalid` / `PathNotSupportedYet` / `FieldNotFound` /
//! `PositionOutOfRange` / `KindMismatch`, never at decode time.
//!
//! The dry-run entry [`can_apply`] obeys the apply-envelope law shared by the
//! sibling hosts: `can_apply(tree, op) ≡ apply(tree, op).is_ok()`.

use crate::canonical::{JVal, ordinal_cmp};
use crate::wire::coerce;
use crate::wire::{Binding, Node, NodeKind, TreeOp};

// ─── Error + result surface ──────────────────────────────────────────────────

/// The canonical apply-failure codes (`ERROR_CODES.md` apply family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyErrorCode {
    NodeNotFound,
    ParentNotFound,
    ChildlessKind,
    PositionOutOfRange,
    DuplicateNodeId,
    FieldNotFound,
    SlotNotFound,
    KindMismatch,
    PathInvalid,
    PathNotSupportedYet,
    OrderingMismatch,
    BatchAborted,
}

impl ApplyErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyErrorCode::NodeNotFound => "NodeNotFound",
            ApplyErrorCode::ParentNotFound => "ParentNotFound",
            ApplyErrorCode::ChildlessKind => "ChildlessKind",
            ApplyErrorCode::PositionOutOfRange => "PositionOutOfRange",
            ApplyErrorCode::DuplicateNodeId => "DuplicateNodeId",
            ApplyErrorCode::FieldNotFound => "FieldNotFound",
            ApplyErrorCode::SlotNotFound => "SlotNotFound",
            ApplyErrorCode::KindMismatch => "KindMismatch",
            ApplyErrorCode::PathInvalid => "PathInvalid",
            ApplyErrorCode::PathNotSupportedYet => "PathNotSupportedYet",
            ApplyErrorCode::OrderingMismatch => "OrderingMismatch",
            ApplyErrorCode::BatchAborted => "BatchAborted",
        }
    }
}

/// The structured, recoverable failure an apply returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyError {
    pub code: ApplyErrorCode,
    pub message: String,
    /// Inner-op index when `code == BatchAborted`.
    pub batch_index: Option<usize>,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApplyError {}

/// One record per applied leaf op (the telemetry contract the sibling hosts share).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpApplyTelemetryRecord {
    /// The op's wire discriminator (`"EditNode"`, `"InsertChild"`, …).
    pub op: &'static str,
    pub target_id: String,
}

/// A successful apply: the new tree + the emitted telemetry records.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOutcome {
    pub new_tree: Node,
    pub emitted_telemetry: Vec<OpApplyTelemetryRecord>,
}

type AResult<T> = Result<T, ApplyError>;

fn fail<T>(code: ApplyErrorCode, message: impl Into<String>) -> AResult<T> {
    Err(ApplyError {
        code,
        message: message.into(),
        batch_index: None,
    })
}

// ─── Tree walkers ────────────────────────────────────────────────────────────

/// The ordered `children` list of a child-bearing layout kind, or `None` — the
/// discriminator every structural child op checks. Exhaustive over `NodeKind`
/// so a future child-bearing kind cannot be silently skipped.
fn layout_children(n: &Node) -> Option<&Vec<Node>> {
    match &n.kind {
        NodeKind::Box(s) => Some(&s.children),
        NodeKind::SplitPanel(s) => Some(&s.children),
        NodeKind::Tabs(s) => Some(&s.children),
        NodeKind::Stepper(s) => Some(&s.children),
        NodeKind::SummaryList(s) => Some(&s.children),
        NodeKind::Disclosure(s) => Some(&s.children),
        NodeKind::Modal(s) => Some(&s.children),
        NodeKind::ScrollArea(s) => Some(&s.children),
        NodeKind::Heading(_)
        | NodeKind::Markdown(_)
        | NodeKind::Metric(_)
        | NodeKind::Badge(_)
        | NodeKind::Sparkline(_)
        | NodeKind::Callout(_)
        | NodeKind::Progress(_)
        | NodeKind::Skeleton(_)
        | NodeKind::LabelValueRow(_)
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
        | NodeKind::ErrorBoundary(_)
        | NodeKind::Switch(_)
        | NodeKind::FragmentDecl(_)
        | NodeKind::FragmentRef(_)
        | NodeKind::Mount(_) => None,
    }
}

fn layout_children_mut(n: &mut Node) -> Option<&mut Vec<Node>> {
    match &mut n.kind {
        NodeKind::Box(s) => Some(&mut s.children),
        NodeKind::SplitPanel(s) => Some(&mut s.children),
        NodeKind::Tabs(s) => Some(&mut s.children),
        NodeKind::Stepper(s) => Some(&mut s.children),
        NodeKind::SummaryList(s) => Some(&mut s.children),
        NodeKind::Disclosure(s) => Some(&mut s.children),
        NodeKind::Modal(s) => Some(&mut s.children),
        NodeKind::ScrollArea(s) => Some(&mut s.children),
        _ => None,
    }
}

/// A kind's short label for error messages (`ChildlessKind` hints).
fn kind_label(n: &Node) -> &'static str {
    match n.kind.category() {
        crate::wire::NodeCategory::Layout => "Layout",
        crate::wire::NodeCategory::Display => "Display",
        crate::wire::NodeCategory::Input => "Input",
        crate::wire::NodeCategory::Visualisation => "Visualisation",
        crate::wire::NodeCategory::Structural => "Structural",
    }
}

/// Every immediate sub-node, in the shared traversal order: layout children /
/// structural sub-trees first, then the `state` surfaces.
fn child_nodes(n: &Node) -> Vec<&Node> {
    let mut out: Vec<&Node> = Vec::new();
    match &n.kind {
        NodeKind::Box(s) => out.extend(&s.children),
        NodeKind::SplitPanel(s) => out.extend(&s.children),
        NodeKind::Tabs(s) => out.extend(&s.children),
        NodeKind::Stepper(s) => out.extend(&s.children),
        NodeKind::SummaryList(s) => out.extend(&s.children),
        NodeKind::Disclosure(s) => out.extend(&s.children),
        NodeKind::Modal(s) => out.extend(&s.children),
        NodeKind::ScrollArea(s) => out.extend(&s.children),
        NodeKind::ErrorBoundary(s) => {
            out.push(&s.child);
            out.push(&s.fallback);
        }
        NodeKind::Switch(s) => {
            out.extend(s.cases.iter().map(|c| &c.child));
            out.push(&s.default);
        }
        NodeKind::FragmentDecl(s) => out.push(&s.body),
        NodeKind::Heading(_)
        | NodeKind::Markdown(_)
        | NodeKind::Metric(_)
        | NodeKind::Badge(_)
        | NodeKind::Sparkline(_)
        | NodeKind::Callout(_)
        | NodeKind::Progress(_)
        | NodeKind::Skeleton(_)
        | NodeKind::LabelValueRow(_)
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
        | NodeKind::Mount(_) => {}
    }
    if let Some(b) = &n.state.on_loading {
        out.push(b);
    }
    if let Some(b) = &n.state.on_empty {
        out.push(b);
    }
    out
}

fn child_nodes_mut(n: &mut Node) -> Vec<&mut Node> {
    let mut out: Vec<&mut Node> = Vec::new();
    match &mut n.kind {
        NodeKind::Box(s) => out.extend(s.children.iter_mut()),
        NodeKind::SplitPanel(s) => out.extend(s.children.iter_mut()),
        NodeKind::Tabs(s) => out.extend(s.children.iter_mut()),
        NodeKind::Stepper(s) => out.extend(s.children.iter_mut()),
        NodeKind::SummaryList(s) => out.extend(s.children.iter_mut()),
        NodeKind::Disclosure(s) => out.extend(s.children.iter_mut()),
        NodeKind::Modal(s) => out.extend(s.children.iter_mut()),
        NodeKind::ScrollArea(s) => out.extend(s.children.iter_mut()),
        NodeKind::ErrorBoundary(s) => {
            out.push(&mut s.child);
            out.push(&mut s.fallback);
        }
        NodeKind::Switch(s) => {
            out.extend(s.cases.iter_mut().map(|c| &mut c.child));
            out.push(&mut s.default);
        }
        NodeKind::FragmentDecl(s) => out.push(&mut s.body),
        _ => {}
    }
    // `state` is a distinct field from `kind`; the borrows are disjoint.
    if let Some(b) = n.state.on_loading.as_deref_mut() {
        out.push(b);
    }
    if let Some(b) = n.state.on_empty.as_deref_mut() {
        out.push(b);
    }
    out
}

fn find_node<'a>(n: &'a Node, target: &str) -> Option<&'a Node> {
    if n.id == target {
        return Some(n);
    }
    child_nodes(n)
        .into_iter()
        .find_map(|c| find_node(c, target))
}

fn find_node_mut<'a>(n: &'a mut Node, target: &str) -> Option<&'a mut Node> {
    if n.id == target {
        return Some(n);
    }
    child_nodes_mut(n)
        .into_iter()
        .find_map(|c| find_node_mut(c, target))
}

/// Every NodeId in `n`'s subtree, DFS order — the traversal the structural-op
/// duplicate checks run.
pub fn all_node_ids(n: &Node) -> Vec<String> {
    let mut out = vec![n.id.clone()];
    for c in child_nodes(n) {
        out.extend(all_node_ids(c));
    }
    out
}

/// The nearest child-bearing layout node whose `children` contain `target`.
fn find_layout_parent<'a>(n: &'a Node, target: &str) -> Option<&'a Node> {
    if let Some(children) = layout_children(n)
        && children.iter().any(|c| c.id == target)
    {
        return Some(n);
    }
    child_nodes(n)
        .into_iter()
        .find_map(|c| find_layout_parent(c, target))
}

fn is_ancestor(ancestor_id: &str, descendant_id: &str, root: &Node) -> bool {
    match find_node(root, ancestor_id) {
        None => false,
        Some(a) => child_nodes(a)
            .into_iter()
            .any(|c| all_node_ids(c).iter().any(|id| id == descendant_id)),
    }
}

// ─── UpdateProp path parser (WIRE_FORMAT.md §3.4 grammar) ────────────────────
//
//   path     := segment ( "." segment )*
//   segment  := field ( "[" index "]" )?
//   field    := [A-Za-z_][A-Za-z0-9_]*
//   index    := "0" | [1-9][0-9]*

struct PathSeg {
    field: String,
    index: Option<usize>,
}

fn is_field_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_field_char(c: char) -> bool {
    is_field_start(c) || c.is_ascii_digit()
}

fn parse_segment(raw: &str) -> Result<PathSeg, String> {
    if raw.is_empty() {
        return Err("empty segment".to_string());
    }
    let (field_part, index_part) = match raw.find('[') {
        None => (raw, None),
        Some(pos) => (&raw[..pos], Some(&raw[pos..])),
    };
    let mut chars = field_part.chars();
    let valid_field = match chars.next() {
        Some(first) => is_field_start(first) && chars.all(is_field_char),
        None => false,
    };
    if !valid_field {
        return Err(format!("segment '{raw}' is not a field name"));
    }
    let Some(index_part) = index_part else {
        return Ok(PathSeg {
            field: field_part.to_string(),
            index: None,
        });
    };
    if index_part.len() < 3 || !index_part.ends_with(']') {
        return Err(format!("malformed index in segment '{raw}'"));
    }
    let digits = &index_part[1..index_part.len() - 1];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "index in segment '{raw}' must be a non-negative decimal integer"
        ));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(format!("index in segment '{raw}' has a leading zero"));
    }
    let index: usize = digits
        .parse()
        .map_err(|_| format!("index in segment '{raw}' is out of range"))?;
    Ok(PathSeg {
        field: field_part.to_string(),
        index: Some(index),
    })
}

fn parse_path(path: &str) -> Result<Vec<PathSeg>, String> {
    if path.trim().is_empty() {
        return Err("empty path".to_string());
    }
    path.split('.').map(parse_segment).collect()
}

// ─── UpdateProp field dispatch (top-level paths) ─────────────────────────────

enum UpdateOutcome {
    Updated(Box<NodeKind>),
    UnknownField,
    NotSupported,
    TypeMismatch(String),
}

/// Thread a coercion result into a spec patch.
fn patch<T>(r: Result<T, String>, build: impl FnOnce(T) -> NodeKind) -> UpdateOutcome {
    match r {
        Ok(v) => UpdateOutcome::Updated(Box::new(build(v))),
        Err(detail) => UpdateOutcome::TypeMismatch(detail),
    }
}

#[allow(clippy::too_many_lines)]
fn update_field(field: &str, value: &JVal, kind: &NodeKind) -> UpdateOutcome {
    use UpdateOutcome::{NotSupported, UnknownField};
    match kind {
        NodeKind::Metric(s) => match field {
            "Label" => patch(coerce::text_source(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    label: x,
                    ..s.clone()
                })
            }),
            "Source" => patch(coerce::binding(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    source: x,
                    ..s.clone()
                })
            }),
            "Format" => patch(coerce::cell_format(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    format: x,
                    ..s.clone()
                })
            }),
            "Tone" => patch(coerce::tone(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    tone: x,
                    ..s.clone()
                })
            }),
            "Weight" => patch(coerce::weight(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    weight: x,
                    ..s.clone()
                })
            }),
            "Emphasis" => patch(coerce::emphasis(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    emphasis: x,
                    ..s.clone()
                })
            }),
            "Trend" => patch(coerce::binding(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    trend: Some(x),
                    ..s.clone()
                })
            }),
            "TrendFormat" => patch(coerce::cell_format(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    trend_format: Some(x),
                    ..s.clone()
                })
            }),
            "Icon" => patch(coerce::icon_source(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    icon: Some(x),
                    ..s.clone()
                })
            }),
            "Subtext" => patch(coerce::text_source(value), |x| {
                NodeKind::Metric(crate::wire::MetricSpec {
                    subtext: Some(x),
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Heading(s) => match field {
            "Level" => patch(coerce::int(value), |x| {
                NodeKind::Heading(crate::wire::HeadingSpec {
                    level: x,
                    ..s.clone()
                })
            }),
            "Text" => patch(coerce::text_source(value), |x| {
                NodeKind::Heading(crate::wire::HeadingSpec {
                    text: x,
                    ..s.clone()
                })
            }),
            "Variant" => patch(coerce::heading_variant(value), |x| {
                NodeKind::Heading(crate::wire::HeadingSpec {
                    variant: x,
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Markdown(_) => match field {
            "Text" => patch(coerce::text_source(value), |x| {
                NodeKind::Markdown(crate::wire::MarkdownSpec { text: x })
            }),
            _ => UnknownField,
        },
        NodeKind::Badge(s) => match field {
            "Label" => patch(coerce::text_source(value), |x| {
                NodeKind::Badge(crate::wire::BadgeSpec {
                    label: x,
                    ..s.clone()
                })
            }),
            "Variant" => patch(coerce::badge_variant(value), |x| {
                NodeKind::Badge(crate::wire::BadgeSpec {
                    variant: x,
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Skeleton(_) => match field {
            "Rows" => patch(coerce::int(value), |x| {
                NodeKind::Skeleton(crate::wire::SkeletonSpec { rows: x })
            }),
            _ => UnknownField,
        },
        NodeKind::Callout(s) => match field {
            "Tone" => patch(coerce::tone(value), |x| {
                NodeKind::Callout(crate::wire::CalloutSpec {
                    tone: x,
                    ..s.clone()
                })
            }),
            "Body" => patch(coerce::text_source(value), |x| {
                NodeKind::Callout(crate::wire::CalloutSpec {
                    body: x,
                    ..s.clone()
                })
            }),
            "Dismissable" => patch(coerce::boolean(value), |x| {
                NodeKind::Callout(crate::wire::CalloutSpec {
                    dismissable: x,
                    ..s.clone()
                })
            }),
            "Heading" => patch(coerce::text_source(value), |x| {
                NodeKind::Callout(crate::wire::CalloutSpec {
                    heading: Some(x),
                    ..s.clone()
                })
            }),
            "Icon" => patch(coerce::icon_source(value), |x| {
                NodeKind::Callout(crate::wire::CalloutSpec {
                    icon: Some(x),
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Progress(s) => match field {
            "Fraction" => patch(coerce::binding(value), |x| {
                NodeKind::Progress(crate::wire::ProgressSpec {
                    fraction: x,
                    ..s.clone()
                })
            }),
            "Indeterminate" => patch(coerce::boolean(value), |x| {
                NodeKind::Progress(crate::wire::ProgressSpec {
                    indeterminate: x,
                    ..s.clone()
                })
            }),
            "Tone" => patch(coerce::tone(value), |x| {
                NodeKind::Progress(crate::wire::ProgressSpec {
                    tone: x,
                    ..s.clone()
                })
            }),
            "Label" => patch(coerce::text_source(value), |x| {
                NodeKind::Progress(crate::wire::ProgressSpec {
                    label: Some(x),
                    ..s.clone()
                })
            }),
            "Caveat" => patch(coerce::text_source(value), |x| {
                NodeKind::Progress(crate::wire::ProgressSpec {
                    caveat: Some(x),
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::LabelValueRow(s) => match field {
            "Label" => patch(coerce::text_source(value), |x| {
                NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                    label: x,
                    ..s.clone()
                })
            }),
            "Source" => patch(coerce::binding(value), |x| {
                NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                    source: x,
                    ..s.clone()
                })
            }),
            "Format" => patch(coerce::cell_format(value), |x| {
                NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                    format: x,
                    ..s.clone()
                })
            }),
            "Emphasis" => patch(coerce::boolean(value), |x| {
                NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                    emphasis: x,
                    ..s.clone()
                })
            }),
            "Help" => patch(coerce::text_source(value), |x| {
                NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                    help: Some(x),
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Link(s) => match field {
            "Href" => patch(coerce::binding(value), |x| {
                NodeKind::Link(crate::wire::LinkSpec {
                    href: x,
                    ..s.clone()
                })
            }),
            "Label" => patch(coerce::text_source(value), |x| {
                NodeKind::Link(crate::wire::LinkSpec {
                    label: x,
                    ..s.clone()
                })
            }),
            "Rel" => patch(coerce::string(value), |x| {
                NodeKind::Link(crate::wire::LinkSpec {
                    rel: Some(x),
                    ..s.clone()
                })
            }),
            "Target" => patch(coerce::string(value), |x| {
                NodeKind::Link(crate::wire::LinkSpec {
                    target: Some(x),
                    ..s.clone()
                })
            }),
            "Download" => patch(coerce::boolean(value), |x| {
                NodeKind::Link(crate::wire::LinkSpec {
                    download: x,
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::Sparkline(_) => NotSupported,
        // Remaining Display kinds carry no field-level surface yet.
        NodeKind::Image(_)
        | NodeKind::List(_)
        | NodeKind::Toast(_)
        | NodeKind::CodeBlock(_)
        | NodeKind::Math(_) => UnknownField,
        // The unified container: the field surface is layout-mode-dependent,
        // preserving the retired kinds' updatable fields.
        NodeKind::Box(s) => {
            use crate::wire::BoxLayout;
            match (field, &s.layout) {
                ("Orientation", BoxLayout::Flex { gap, wrap, .. }) => {
                    let (gap, wrap) = (*gap, *wrap);
                    patch(coerce::orientation(value), |x| {
                        NodeKind::Box(crate::wire::BoxSpec {
                            layout: BoxLayout::Flex {
                                direction: x,
                                gap,
                                wrap,
                            },
                            ..s.clone()
                        })
                    })
                }
                ("Wrap", BoxLayout::Flex { direction, gap, .. }) => {
                    let (direction, gap) = (*direction, *gap);
                    patch(coerce::boolean(value), |x| {
                        NodeKind::Box(crate::wire::BoxSpec {
                            layout: BoxLayout::Flex {
                                direction,
                                gap,
                                wrap: x,
                            },
                            ..s.clone()
                        })
                    })
                }
                (
                    "Cols",
                    BoxLayout::Grid {
                        gap,
                        template_columns,
                        ..
                    },
                ) => {
                    let (gap, template_columns) = (*gap, template_columns.clone());
                    patch(coerce::int(value), |x| {
                        NodeKind::Box(crate::wire::BoxSpec {
                            layout: BoxLayout::Grid {
                                cols: x,
                                gap,
                                template_columns,
                            },
                            ..s.clone()
                        })
                    })
                }
                ("TemplateColumns", BoxLayout::Grid { cols, gap, .. }) => {
                    let (cols, gap) = (*cols, *gap);
                    patch(coerce::string(value), |x| {
                        NodeKind::Box(crate::wire::BoxSpec {
                            layout: BoxLayout::Grid {
                                cols,
                                gap,
                                template_columns: Some(x),
                            },
                            ..s.clone()
                        })
                    })
                }
                ("Heading", _) => patch(coerce::text_source(value), |x| {
                    NodeKind::Box(crate::wire::BoxSpec {
                        heading: Some(x),
                        ..s.clone()
                    })
                }),
                ("Children", _) => NotSupported,
                _ => UnknownField,
            }
        }
        NodeKind::SplitPanel(s) => match field {
            "Weight" => patch(coerce::float(value), |x| {
                NodeKind::SplitPanel(crate::wire::SplitPanelSpec {
                    weight: x,
                    ..s.clone()
                })
            }),
            "Children" => NotSupported,
            _ => UnknownField,
        },
        NodeKind::Tabs(s) => match field {
            "Orientation" => patch(coerce::orientation(value), |x| {
                NodeKind::Tabs(crate::wire::TabsSpec {
                    orientation: x,
                    ..s.clone()
                })
            }),
            "Children" => NotSupported,
            _ => UnknownField,
        },
        NodeKind::Stepper(s) => match field {
            "ActiveStep" => patch(coerce::binding(value), |x| {
                NodeKind::Stepper(crate::wire::StepperSpec {
                    active_step: x,
                    ..s.clone()
                })
            }),
            "Children" => NotSupported,
            _ => UnknownField,
        },
        NodeKind::SummaryList(s) => match field {
            "Heading" => patch(coerce::text_source(value), |x| {
                NodeKind::SummaryList(crate::wire::SummaryListSpec {
                    heading: Some(x),
                    ..s.clone()
                })
            }),
            "Children" => NotSupported,
            _ => UnknownField,
        },
        NodeKind::Disclosure(s) => match field {
            "Heading" => patch(coerce::text_source(value), |x| {
                NodeKind::Disclosure(crate::wire::DisclosureSpec {
                    heading: x,
                    ..s.clone()
                })
            }),
            "Open" => patch(coerce::binding(value), |x| {
                NodeKind::Disclosure(crate::wire::DisclosureSpec {
                    open: x,
                    ..s.clone()
                })
            }),
            "DefaultOpen" => patch(coerce::boolean(value), |x| {
                NodeKind::Disclosure(crate::wire::DisclosureSpec {
                    default_open: x,
                    ..s.clone()
                })
            }),
            "Children" => NotSupported,
            _ => UnknownField,
        },
        NodeKind::Modal(_) | NodeKind::ScrollArea(_) => UnknownField,
        NodeKind::FragmentDecl(s) => match field {
            "Name" => patch(coerce::string(value), |x| {
                NodeKind::FragmentDecl(crate::wire::FragmentDeclSpec {
                    name: x,
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        NodeKind::FragmentRef(s) => match field {
            "Name" => patch(coerce::string(value), |x| {
                NodeKind::FragmentRef(crate::wire::FragmentRefSpec {
                    name: x,
                    ..s.clone()
                })
            }),
            _ => UnknownField,
        },
        // Input / Visualisation / Custom / ErrorBoundary / Switch / Mount have
        // no top-level field surface.
        NodeKind::Form(_)
        | NodeKind::Filters(_)
        | NodeKind::Button(_)
        | NodeKind::FileUpload(_)
        | NodeKind::Select(_)
        | NodeKind::DataGrid(_)
        | NodeKind::Chart(_)
        | NodeKind::Map(_)
        | NodeKind::Custom(_)
        | NodeKind::ErrorBoundary(_)
        | NodeKind::Switch(_)
        | NodeKind::Mount(_) => NotSupported,
    }
}

// ─── UpdateProp nested dispatch — the v1 typed-traversal legs (§3.4) ─────────

enum NestedOutcome {
    Updated(Box<NodeKind>),
    FieldNotFound {
        segment: String,
        available: &'static [&'static str],
    },
    MissingIndex {
        list_field: &'static str,
        count: usize,
    },
    IndexOutOfRange {
        list_field: &'static str,
        count: usize,
        requested: usize,
    },
    NotSupported,
    TypeMismatch(String),
}

fn nested_patch<T>(r: Result<T, String>, build: impl FnOnce(T) -> NodeKind) -> NestedOutcome {
    match r {
        Ok(v) => NestedOutcome::Updated(Box::new(build(v))),
        Err(detail) => NestedOutcome::TypeMismatch(detail),
    }
}

/// The sole leaf shape the v1 legs accept: exactly one trailing un-indexed field.
fn leaf_of(rest: &[PathSeg]) -> Option<&str> {
    match rest {
        [seg] if seg.index.is_none() => Some(&seg.field),
        _ => None,
    }
}

fn update_nested(segs: &[PathSeg], value: &JVal, kind: &NodeKind) -> NestedOutcome {
    use NestedOutcome::{FieldNotFound, IndexOutOfRange, MissingIndex, NotSupported};
    let Some((head, rest)) = segs.split_first() else {
        return NotSupported;
    };

    match kind {
        NodeKind::DataGrid(s) => {
            if head.field != "Columns" {
                return FieldNotFound {
                    segment: head.field.clone(),
                    available: &["Columns"],
                };
            }
            let Some(i) = head.index else {
                return MissingIndex {
                    list_field: "Columns",
                    count: s.columns.len(),
                };
            };
            if i >= s.columns.len() {
                return IndexOutOfRange {
                    list_field: "Columns",
                    count: s.columns.len(),
                    requested: i,
                };
            }
            let rebuild = |col: crate::wire::ColumnErased| {
                let mut spec = s.clone();
                spec.columns[i] = col;
                NodeKind::DataGrid(spec)
            };
            let col = &s.columns[i];
            match leaf_of(rest) {
                None => NotSupported,
                Some("Label") => nested_patch(coerce::string(value), |x| {
                    rebuild(crate::wire::ColumnErased {
                        label: x,
                        ..col.clone()
                    })
                }),
                Some("Format") => nested_patch(coerce::cell_format(value), |x| {
                    rebuild(crate::wire::ColumnErased {
                        format: x,
                        ..col.clone()
                    })
                }),
                Some("Width") => nested_patch(coerce::column_width(value), |x| {
                    rebuild(crate::wire::ColumnErased {
                        width: x,
                        ..col.clone()
                    })
                }),
                // Closure-bearing — never addressable.
                Some("Value") | Some("Kind") => NotSupported,
                Some(other) => FieldNotFound {
                    segment: other.to_string(),
                    available: &["Label", "Format", "Width"],
                },
            }
        }
        NodeKind::Chart(s) => {
            if head.field != "YFields" {
                return FieldNotFound {
                    segment: head.field.clone(),
                    available: &["YFields"],
                };
            }
            let Some(i) = head.index else {
                return MissingIndex {
                    list_field: "YFields",
                    count: s.y_fields.len(),
                };
            };
            if i >= s.y_fields.len() {
                return IndexOutOfRange {
                    list_field: "YFields",
                    count: s.y_fields.len(),
                    requested: i,
                };
            }
            if !rest.is_empty() {
                return NotSupported;
            }
            nested_patch(coerce::string(value), |x| {
                let mut spec = s.clone();
                spec.y_fields[i] = x;
                NodeKind::Chart(spec)
            })
        }
        NodeKind::Tabs(s) => {
            // An absent header list addresses like an empty one.
            let headers = s.tab_headers.clone().unwrap_or_default();
            if head.field != "TabHeaders" {
                return FieldNotFound {
                    segment: head.field.clone(),
                    available: &["TabHeaders"],
                };
            }
            let Some(i) = head.index else {
                return MissingIndex {
                    list_field: "TabHeaders",
                    count: headers.len(),
                };
            };
            if i >= headers.len() {
                return IndexOutOfRange {
                    list_field: "TabHeaders",
                    count: headers.len(),
                    requested: i,
                };
            }
            let hdr = headers[i].clone();
            let rebuild = |h: crate::wire::TabHeader| {
                let mut hs = headers.clone();
                hs[i] = h;
                NodeKind::Tabs(crate::wire::TabsSpec {
                    tab_headers: Some(hs),
                    ..s.clone()
                })
            };
            match leaf_of(rest) {
                None => NotSupported,
                Some("Label") => nested_patch(coerce::text_source(value), |x| {
                    rebuild(crate::wire::TabHeader {
                        label: x,
                        ..hdr.clone()
                    })
                }),
                Some("Icon") => nested_patch(coerce::icon_source(value), |x| {
                    rebuild(crate::wire::TabHeader {
                        icon: Some(x),
                        ..hdr.clone()
                    })
                }),
                // Optional typed binding; replacing it installs the value.
                Some("Disabled") => nested_patch(coerce::binding(value), |x| {
                    rebuild(crate::wire::TabHeader {
                        disabled: Some(x),
                        ..hdr.clone()
                    })
                }),
                Some(other) => FieldNotFound {
                    segment: other.to_string(),
                    available: &["Label", "Icon", "Disabled"],
                },
            }
        }
        NodeKind::Form(s) => {
            if head.field != "Fields" {
                return FieldNotFound {
                    segment: head.field.clone(),
                    available: &["Fields"],
                };
            }
            let Some(i) = head.index else {
                return MissingIndex {
                    list_field: "Fields",
                    count: s.fields.len(),
                };
            };
            if i >= s.fields.len() {
                return IndexOutOfRange {
                    list_field: "Fields",
                    count: s.fields.len(),
                    requested: i,
                };
            }
            let fld = &s.fields[i];
            let rebuild = |f: crate::wire::FormField| {
                let mut spec = s.clone();
                spec.fields[i] = f;
                NodeKind::Form(spec)
            };
            match leaf_of(rest) {
                None => NotSupported,
                Some("Label") => nested_patch(coerce::text_source(value), |x| {
                    rebuild(crate::wire::FormField {
                        label: x,
                        ..fld.clone()
                    })
                }),
                Some("Required") => nested_patch(coerce::boolean(value), |x| {
                    rebuild(crate::wire::FormField {
                        required: x,
                        ..fld.clone()
                    })
                }),
                Some("Help") => nested_patch(coerce::text_source(value), |x| {
                    rebuild(crate::wire::FormField {
                        help: Some(x),
                        ..fld.clone()
                    })
                }),
                // Id is the form-store key; Kind is closure-bearing.
                Some("Id") | Some("Kind") => NotSupported,
                Some(other) => FieldNotFound {
                    segment: other.to_string(),
                    available: &["Label", "Required", "Help"],
                },
            }
        }
        _ => NotSupported,
    }
}

// ─── ReplaceBinding slot dispatch ────────────────────────────────────────────

fn replace_binding(slot: &str, b: &Binding, kind: &NodeKind) -> Option<NodeKind> {
    match (kind, slot) {
        (NodeKind::Metric(s), "Source") => Some(NodeKind::Metric(crate::wire::MetricSpec {
            source: b.clone(),
            ..s.clone()
        })),
        (NodeKind::Metric(s), "Trend") => Some(NodeKind::Metric(crate::wire::MetricSpec {
            trend: Some(b.clone()),
            ..s.clone()
        })),
        (NodeKind::Sparkline(_), "Source") => {
            Some(NodeKind::Sparkline(crate::wire::SparklineSpec {
                source: b.clone(),
            }))
        }
        (NodeKind::Progress(s), "Fraction") => {
            Some(NodeKind::Progress(crate::wire::ProgressSpec {
                fraction: b.clone(),
                ..s.clone()
            }))
        }
        (NodeKind::LabelValueRow(s), "Source") => {
            Some(NodeKind::LabelValueRow(crate::wire::LabelValueRowSpec {
                source: b.clone(),
                ..s.clone()
            }))
        }
        (NodeKind::Stepper(s), "ActiveStep") => Some(NodeKind::Stepper(crate::wire::StepperSpec {
            active_step: b.clone(),
            ..s.clone()
        })),
        (NodeKind::Button(s), "Disabled") => Some(NodeKind::Button(crate::wire::ButtonSpec {
            disabled: Some(b.clone()),
            ..s.clone()
        })),
        (NodeKind::Select(s), "Disabled") => Some(NodeKind::Select(crate::wire::SelectSpec {
            disabled: Some(b.clone()),
            ..s.clone()
        })),
        (NodeKind::Form(s), "Disabled") => Some(NodeKind::Form(crate::wire::FormSpec {
            disabled: Some(b.clone()),
            ..s.clone()
        })),
        (NodeKind::FileUpload(s), "Disabled") => {
            Some(NodeKind::FileUpload(crate::wire::FileUploadSpec {
                disabled: Some(b.clone()),
                ..s.clone()
            }))
        }
        (NodeKind::DataGrid(s), "Source") => Some(NodeKind::DataGrid(crate::wire::GridSpec {
            source: b.clone(),
            ..s.clone()
        })),
        (NodeKind::Chart(s), "Source") => Some(NodeKind::Chart(crate::wire::ChartSpec {
            source: b.clone(),
            ..s.clone()
        })),
        (NodeKind::Map(s), "Source") => Some(NodeKind::Map(crate::wire::MapSpec {
            source: b.clone(),
            ..s.clone()
        })),
        _ => None,
    }
}

// ─── Single-op apply ─────────────────────────────────────────────────────────

fn set_kind(root: &Node, target: &str, kind: NodeKind) -> Option<Node> {
    let mut new_tree = root.clone();
    let node = find_node_mut(&mut new_tree, target)?;
    node.kind = kind;
    Some(new_tree)
}

fn apply_one(op: &TreeOp, root: &Node, telem: &mut Vec<OpApplyTelemetryRecord>) -> AResult<Node> {
    match op {
        TreeOp::EditNode { target, new_kind } => {
            let Some(new_tree) = set_kind(root, target, new_kind.clone()) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            telem.push(OpApplyTelemetryRecord {
                op: "EditNode",
                target_id: target.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::UpdateProp {
            target,
            path,
            value,
        } => {
            let segs = parse_path(path).map_err(|e| ApplyError {
                code: ApplyErrorCode::PathInvalid,
                message: format!("Path '{path}' is structurally invalid: {e}."),
                batch_index: None,
            })?;
            let Some(target_node) = find_node(root, target) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            let finish = |kind: NodeKind, telem: &mut Vec<OpApplyTelemetryRecord>| {
                let Some(new_tree) = set_kind(root, target, kind) else {
                    return fail(
                        ApplyErrorCode::NodeNotFound,
                        format!("Node '{target}' not found in tree."),
                    );
                };
                telem.push(OpApplyTelemetryRecord {
                    op: "UpdateProp",
                    target_id: target.clone(),
                });
                Ok(new_tree)
            };
            if segs.len() == 1 && segs[0].index.is_none() {
                // Top-level path — the per-kind field dispatch.
                match update_field(path, value, &target_node.kind) {
                    UpdateOutcome::Updated(kind) => finish(*kind, telem),
                    UpdateOutcome::UnknownField => fail(
                        ApplyErrorCode::FieldNotFound,
                        format!("Field '{path}' not found on node '{target}'."),
                    ),
                    UpdateOutcome::NotSupported => fail(
                        ApplyErrorCode::PathNotSupportedYet,
                        format!(
                            "Path '{path}' on node '{target}' is not yet supported by the apply engine."
                        ),
                    ),
                    UpdateOutcome::TypeMismatch(detail) => fail(
                        ApplyErrorCode::KindMismatch,
                        format!(
                            "UpdateProp value for '{path}' on node '{target}' does not match the field's expected type: {detail}"
                        ),
                    ),
                }
            } else {
                // Nested path — the per-kind typed traversal.
                match update_nested(&segs, value, &target_node.kind) {
                    NestedOutcome::Updated(kind) => finish(*kind, telem),
                    NestedOutcome::MissingIndex { list_field, count } => fail(
                        ApplyErrorCode::PathInvalid,
                        format!(
                            "Field '{list_field}' on node '{target}' is a list — address an element with a 0-based index (the list has {count} element(s))."
                        ),
                    ),
                    NestedOutcome::IndexOutOfRange {
                        list_field,
                        count,
                        requested,
                    } => {
                        let range = if count == 0 {
                            "the list is empty".to_string()
                        } else {
                            format!("valid: 0..{}", count - 1)
                        };
                        fail(
                            ApplyErrorCode::PositionOutOfRange,
                            format!(
                                "Index {requested} is out of range for '{list_field}' on node '{target}' ({range})."
                            ),
                        )
                    }
                    NestedOutcome::FieldNotFound { segment, available } => fail(
                        ApplyErrorCode::FieldNotFound,
                        format!(
                            "Field '{segment}' (in path '{path}') not found on node '{target}'. Available at this segment: {}.",
                            available.join(", ")
                        ),
                    ),
                    NestedOutcome::NotSupported => fail(
                        ApplyErrorCode::PathNotSupportedYet,
                        format!(
                            "Path '{path}' on node '{target}' is not yet supported by the apply engine."
                        ),
                    ),
                    NestedOutcome::TypeMismatch(detail) => fail(
                        ApplyErrorCode::KindMismatch,
                        format!(
                            "UpdateProp value for '{path}' on node '{target}' does not match the field's expected type: {detail}"
                        ),
                    ),
                }
            }
        }
        TreeOp::ReplaceBinding {
            target,
            slot,
            binding,
        } => {
            let Some(target_node) = find_node(root, target) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            let Some(new_kind) = replace_binding(slot, binding, &target_node.kind) else {
                return fail(
                    ApplyErrorCode::SlotNotFound,
                    format!("Binding slot '{slot}' not found on node '{target}'."),
                );
            };
            let Some(new_tree) = set_kind(root, target, new_kind) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            telem.push(OpApplyTelemetryRecord {
                op: "ReplaceBinding",
                target_id: target.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::UpdateStyle { target, style } => {
            let mut new_tree = root.clone();
            let Some(node) = find_node_mut(&mut new_tree, target) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            node.style = *style;
            telem.push(OpApplyTelemetryRecord {
                op: "UpdateStyle",
                target_id: target.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::UpdateState { target, state } => {
            let mut new_tree = root.clone();
            let Some(node) = find_node_mut(&mut new_tree, target) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            node.state = state.clone();
            telem.push(OpApplyTelemetryRecord {
                op: "UpdateState",
                target_id: target.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::InsertChild {
            parent_id,
            position,
            child,
        } => {
            let Some(parent) = find_node(root, parent_id) else {
                return fail(
                    ApplyErrorCode::ParentNotFound,
                    format!("Parent node '{parent_id}' not found in tree."),
                );
            };
            let Some(children) = layout_children(parent) else {
                return fail(
                    ApplyErrorCode::ChildlessKind,
                    format!(
                        "Node '{parent_id}' (kind={}) has no children field — only child-bearing layout kinds accept structural child ops.",
                        kind_label(parent)
                    ),
                );
            };
            let position = *position;
            if position < 0 || position as usize > children.len() {
                return fail(
                    ApplyErrorCode::PositionOutOfRange,
                    format!(
                        "Position {position} is out of range for parent '{parent_id}' (valid: 0..{}).",
                        children.len()
                    ),
                );
            }
            let existing: std::collections::HashSet<String> =
                all_node_ids(root).into_iter().collect();
            if let Some(duplicate) = all_node_ids(child)
                .into_iter()
                .find(|id| existing.contains(id))
            {
                return fail(
                    ApplyErrorCode::DuplicateNodeId,
                    format!(
                        "NodeId '{duplicate}' is already present in the tree; ids must be unique."
                    ),
                );
            }
            let mut new_tree = root.clone();
            let parent = find_node_mut(&mut new_tree, parent_id)
                .expect("parent located above; the clone preserves it");
            layout_children_mut(parent)
                .expect("children located above; the clone preserves them")
                .insert(position as usize, child.clone());
            telem.push(OpApplyTelemetryRecord {
                op: "InsertChild",
                target_id: parent_id.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::RemoveNode { target } => {
            if root.id == *target {
                return fail(ApplyErrorCode::KindMismatch, "Cannot RemoveNode the root.");
            }
            let Some(parent) = find_layout_parent(root, target) else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            let parent_id = parent.id.clone();
            let mut new_tree = root.clone();
            let parent = find_node_mut(&mut new_tree, &parent_id)
                .expect("parent located above; the clone preserves it");
            layout_children_mut(parent)
                .expect("layout parent by construction")
                .retain(|c| c.id != *target);
            telem.push(OpApplyTelemetryRecord {
                op: "RemoveNode",
                target_id: target.clone(),
            });
            Ok(new_tree)
        }
        TreeOp::MoveNode {
            target,
            new_parent_id,
            new_position,
        } => {
            if target == new_parent_id {
                return fail(
                    ApplyErrorCode::KindMismatch,
                    "Cannot move a node into itself.",
                );
            }
            if is_ancestor(target, new_parent_id, root) {
                return fail(
                    ApplyErrorCode::KindMismatch,
                    "Cannot move a node into its own descendant (would create a cycle).",
                );
            }
            let Some(moving) = find_node(root, target).cloned() else {
                return fail(
                    ApplyErrorCode::NodeNotFound,
                    format!("Node '{target}' not found in tree."),
                );
            };
            let Some(new_parent) = find_node(root, new_parent_id) else {
                return fail(
                    ApplyErrorCode::ParentNotFound,
                    format!("Parent node '{new_parent_id}' not found in tree."),
                );
            };
            if layout_children(new_parent).is_none() {
                return fail(
                    ApplyErrorCode::ChildlessKind,
                    format!(
                        "Node '{new_parent_id}' (kind={}) has no children field.",
                        kind_label(new_parent)
                    ),
                );
            }
            let after_remove = apply_one(
                &TreeOp::RemoveNode {
                    target: target.clone(),
                },
                root,
                &mut vec![],
            )?;
            let inserted = apply_one(
                &TreeOp::InsertChild {
                    parent_id: new_parent_id.clone(),
                    position: *new_position,
                    child: moving,
                },
                &after_remove,
                &mut vec![],
            )?;
            telem.push(OpApplyTelemetryRecord {
                op: "MoveNode",
                target_id: target.clone(),
            });
            Ok(inserted)
        }
        TreeOp::ReorderChildren {
            parent_id,
            new_order,
        } => {
            let Some(parent) = find_node(root, parent_id) else {
                return fail(
                    ApplyErrorCode::ParentNotFound,
                    format!("Parent node '{parent_id}' not found in tree."),
                );
            };
            let Some(children) = layout_children(parent) else {
                return fail(
                    ApplyErrorCode::ChildlessKind,
                    format!(
                        "Node '{parent_id}' (kind={}) has no children field.",
                        kind_label(parent)
                    ),
                );
            };
            let mut sorted_current: Vec<&str> = children.iter().map(|c| c.id.as_str()).collect();
            sorted_current.sort_by(|a, b| ordinal_cmp(a, b));
            let mut sorted_new: Vec<&str> = new_order.iter().map(String::as_str).collect();
            sorted_new.sort_by(|a, b| ordinal_cmp(a, b));
            if sorted_current != sorted_new {
                return fail(
                    ApplyErrorCode::OrderingMismatch,
                    format!(
                        "ReorderChildren for '{parent_id}' did not list exactly the current child ids."
                    ),
                );
            }
            let reordered: Vec<Node> = new_order
                .iter()
                .filter_map(|id| children.iter().find(|c| &c.id == id).cloned())
                .collect();
            let mut new_tree = root.clone();
            let parent = find_node_mut(&mut new_tree, parent_id)
                .expect("parent located above; the clone preserves it");
            *layout_children_mut(parent).expect("layout parent by construction") = reordered;
            telem.push(OpApplyTelemetryRecord {
                op: "ReorderChildren",
                target_id: parent_id.clone(),
            });
            Ok(new_tree)
        }
        // The whole-tree swap: the only op that legally changes the root id.
        TreeOp::ReplaceRoot { node } => Ok(node.clone()),
        TreeOp::Batch(ops) => {
            let mut state = root.clone();
            for (i, inner) in ops.iter().enumerate() {
                match apply_one(inner, &state, telem) {
                    Ok(next) => state = next,
                    Err(e) => {
                        // All-or-nothing: the caller discards partial telemetry
                        // with the error.
                        return Err(ApplyError {
                            code: ApplyErrorCode::BatchAborted,
                            message: format!("Batch aborted at inner op #{i}: {}", e.message),
                            batch_index: Some(i),
                        });
                    }
                }
            }
            Ok(state)
        }
    }
}

// ─── Public entry ────────────────────────────────────────────────────────────

/// Apply a single tree-op against `tree`, returning either the updated tree
/// plus emitted telemetry, or a structured [`ApplyError`]. `tree` is never
/// mutated. Fold across an op list to apply many; wrap in [`TreeOp::Batch`]
/// for atomic all-or-nothing application.
pub fn apply(tree: &Node, op: &TreeOp) -> Result<ApplyOutcome, ApplyError> {
    let mut telem = Vec::new();
    let new_tree = apply_one(op, tree, &mut telem)?;
    Ok(ApplyOutcome {
        new_tree,
        emitted_telemetry: telem,
    })
}

/// Dry-run: whether `op` would apply cleanly against `tree`. By construction
/// this obeys the apply-envelope law the sibling hosts certify:
/// `can_apply(tree, op) ≡ apply(tree, op).is_ok()`.
pub fn can_apply(tree: &Node, op: &TreeOp) -> bool {
    apply(tree, op).is_ok()
}
