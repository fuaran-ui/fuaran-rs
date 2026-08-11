//! The pre-emit structural validator: a default-deny-posture walk over the
//! decoded typed tree, surfacing the canonical `FUARAN###` defect codes the
//! sibling hosts share so a Rust service can validate a tree *before* it emits
//! or drives it.
//!
//! Two of the sibling validators' rule families are enforced **by
//! construction** in this host and therefore have no runtime rule here:
//! unrecognised kinds and missing required slots cannot exist on a decoded
//! tree (the typed model's exhaustive decode refuses them with the six-code
//! `DecodeError` envelope). What remains is the tree-decidable rule subset —
//! node identity, bounded primitives, shape coherence, and the
//! wire-survivability / write-back lints:
//!
//! | Code | Severity | Rule |
//! |---|---|---|
//! | `EMPTY_NODE_ID` | Error | a node id is the empty string (the §8 identity invariant, re-checked pre-emit) |
//! | `FUARAN001` | Error | duplicate NodeId within the tree (op-target stability) |
//! | `FUARAN047` | Error | Tabs `tabHeaders` length ≠ children length |
//! | `FUARAN048` | Error | Tabs `tabTags` length ≠ children length |
//! | `FUARAN049` | Warning | Tabs `activeTag` set without `tabTags` |
//! | `FUARAN050` | Warning | Progress fraction static literal outside `[0, 1]` |
//! | `FUARAN061` | Error | `Binding.Format` Currency with a blank ISO-4217 code |
//! | `FUARAN063` | Warning | Link with a blank static href |
//! | `FUARAN064` | Warning | Button `disabled` bound to `Static(false)` (no-op) |
//! | `FUARAN069` | Warning | inert control: omitted handler over a non-writable value binding (write-back default cannot arm; `Local` exempt) |
//! | `FUARAN073` | Warning | fire-and-forget `Action.Call` (no `into`, no `onResult`) |
//! | `FUARAN082` | Warning | duplicate `Switch` case match values (first-match-wins shadows the rest) |
//! | `FUARAN083` | Warning | ungrounded `Switch` selector: an empty-key `State` binding can never resolve a case (any other `Binding` names its source and is grounded by construction) |
//! | `FUARAN086` | Error | chart field reference absent from the statically-known data schema (Phase 640) |
//! | `FUARAN087` | Error | grounded chart value field of a non-numeric column type (silently flat series; Phase 640) |
//! | `FUARAN088` | Error | Pie with other than exactly one `yFields` series (the lowering refuses; Phase 638/640) |
//! | `FUARAN089` | Warning | `stacked` on a kind where stacking is meaningless (Line/Scatter/Pie; Phase 637/640) |
//! | `FUARAN084` | Warning / Error | `Binding.Computed` — host-only, erases on the wire (Error in orchestrated runs) |

use crate::canonical::JVal;
use crate::wire::{Action, Binding, FormFieldKind, Node, NodeKind, StaticValue, TreeOp};

/// Finding severity, matching the sibling validators' two-level surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// One validator finding: a stable code + the node it anchors to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// The stable, cross-host defect code (`FUARAN###`, or `EMPTY_NODE_ID` for
    /// the §8 identity re-check).
    pub code: &'static str,
    /// The id of the node the finding anchors to.
    pub node_id: String,
    pub message: String,
}

/// Validation options. `orchestrated` matches the sibling validators'
/// `--orchestrated` mode: wire-survivability escapes (`FUARAN084`) harden from
/// advisory to Error in an AI-emitted / orchestrated run.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidateOptions {
    pub orchestrated: bool,
}

struct Walker {
    orchestrated: bool,
    findings: Vec<Finding>,
    seen_ids: std::collections::HashMap<String, usize>,
}

impl Walker {
    fn push(&mut self, severity: Severity, code: &'static str, node_id: &str, message: String) {
        self.findings.push(Finding {
            severity,
            code,
            node_id: node_id.to_string(),
            message,
        });
    }

    fn walk(&mut self, n: &Node) {
        // ── EMPTY_NODE_ID / FUARAN001 — node identity ──
        if n.id.is_empty() {
            self.push(
                Severity::Error,
                "EMPTY_NODE_ID",
                &n.id,
                "Node id is the empty string — every node carries a non-empty stable id."
                    .to_string(),
            );
        }
        *self.seen_ids.entry(n.id.clone()).or_insert(0) += 1;

        self.check_kind(&n.id, &n.kind);

        for child in child_nodes(n) {
            self.walk(child);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn check_kind(&mut self, id: &str, kind: &NodeKind) {
        match kind {
            NodeKind::Tabs(s) => {
                if let Some(headers) = &s.tab_headers
                    && headers.len() != s.children.len()
                {
                    self.push(
                        Severity::Error,
                        "FUARAN047",
                        id,
                        format!(
                            "Tabs declares tabHeaders with {} entries but {} children — headers align 1:1 with children by index.",
                            headers.len(),
                            s.children.len()
                        ),
                    );
                }
                if let Some(tags) = &s.tab_tags
                    && tags.len() != s.children.len()
                {
                    self.push(
                        Severity::Error,
                        "FUARAN048",
                        id,
                        format!(
                            "Tabs declares tabTags with {} entries but {} children — the tag overlay maps tags to children by index.",
                            tags.len(),
                            s.children.len()
                        ),
                    );
                }
                if s.active_tag.is_some() && s.tab_tags.is_none() {
                    self.push(
                        Severity::Warning,
                        "FUARAN049",
                        id,
                        "Tabs sets activeTag but has no tabTags — the tag binding has nothing to resolve against; the renderer falls back to activeIndex.".to_string(),
                    );
                }
                // Write-back: an omitted onSelect needs a writable activeIndex.
                if s.on_select.is_none() {
                    self.check_writable(id, "activeIndex", &s.active_index);
                }
                self.check_binding(id, &s.active_index);
                if let Some(active_tag) = &s.active_tag {
                    self.check_binding(id, active_tag);
                }
                if let Some(headers) = &s.tab_headers {
                    for h in headers {
                        if let Some(disabled) = &h.disabled {
                            self.check_binding(id, disabled);
                        }
                    }
                }
            }
            NodeKind::Progress(s) => {
                if let Binding::Static {
                    value: StaticValue::Ast(JVal::Num(fraction)),
                } = &s.fraction
                    && !(0.0..=1.0).contains(fraction)
                {
                    self.push(
                        Severity::Warning,
                        "FUARAN050",
                        id,
                        format!(
                            "Progress fraction literal {fraction} is outside the bounded domain [0, 1] — declare an honest 0..1 value, or set indeterminate with a caveat."
                        ),
                    );
                }
                self.check_binding(id, &s.fraction);
            }
            NodeKind::Link(s) => {
                if let Binding::Static {
                    value: StaticValue::Ast(JVal::Str(href)),
                } = &s.href
                    && href.trim().is_empty()
                {
                    self.push(
                        Severity::Warning,
                        "FUARAN063",
                        id,
                        "Link has a blank href — an empty href navigates to the current page; provide a real destination URL.".to_string(),
                    );
                }
                self.check_binding(id, &s.href);
            }
            NodeKind::Button(s) => {
                if let Some(Binding::Static {
                    value: StaticValue::Ast(JVal::Bool(false)),
                }) = &s.disabled
                {
                    self.push(
                        Severity::Warning,
                        "FUARAN064",
                        id,
                        "Button disabled is bound to Static(false) — a constant-false disabled binding never disables the button; point it at live state or remove it.".to_string(),
                    );
                }
                self.check_action(id, &s.on_click);
                if let Some(disabled) = &s.disabled {
                    self.check_binding(id, disabled);
                }
            }
            NodeKind::Form(s) => {
                for field in &s.fields {
                    self.check_form_field(id, &field.kind);
                }
                self.check_action(id, &s.on_submit);
                if let Some(disabled) = &s.disabled {
                    self.check_binding(id, disabled);
                }
            }
            NodeKind::Filters(specs) => {
                // 0.2.0 filters-unification: a chip's control is an ordinary
                // FormFieldKind; its auto/explicit Filter binding writes the
                // filter store itself when the handler is omitted, so the
                // same control walk applies.
                for spec in specs {
                    self.check_form_field(id, &spec.kind);
                }
            }
            NodeKind::Select(s) => {
                if s.on_change.is_none() && !s.multiple {
                    self.check_writable(id, "value", &s.value);
                }
                if let Some(values) = &s.values
                    && s.on_change_multi.is_none()
                {
                    self.check_writable(id, "values", values);
                }
                self.check_binding(id, &s.source);
                self.check_binding(id, &s.value);
                if let Some(values) = &s.values {
                    self.check_binding(id, values);
                }
                if let Some(disabled) = &s.disabled {
                    self.check_binding(id, disabled);
                }
            }
            NodeKind::Disclosure(s) => {
                if s.on_toggle.is_none() {
                    self.check_writable(id, "open", &s.open);
                }
                self.check_binding(id, &s.open);
            }
            NodeKind::Modal(s) => {
                if s.on_dismiss.is_none() && s.dismissable {
                    self.check_writable(id, "open", &s.open);
                }
                if let Some(on_dismiss) = &s.on_dismiss {
                    self.check_action(id, on_dismiss);
                }
                self.check_binding(id, &s.open);
            }
            NodeKind::Switch(s) => {
                // FUARAN083 — an empty-key State selector is ungrounded: the
                // switch can never resolve a case, so it is stuck on its
                // `default` forever. Any other Binding selector names its
                // source and is grounded by construction.
                if let Binding::State { key, .. } = &s.on
                    && key.is_empty()
                {
                    self.push(
                        Severity::Warning,
                        "FUARAN083",
                        id,
                        "Switch has an empty stateKey — it can never resolve a case and is stuck on its default; name the state key the switch selects on.".to_string(),
                    );
                }
                let mut seen = std::collections::HashSet::new();
                for case in &s.cases {
                    if !seen.insert(&case.match_value) {
                        self.push(
                            Severity::Warning,
                            "FUARAN082",
                            id,
                            format!(
                                "Switch declares duplicate case match \"{}\" — first-match-wins shadows the later case.",
                                case.match_value
                            ),
                        );
                    }
                }
            }
            NodeKind::Metric(s) => {
                self.check_binding(id, &s.value);
                self.check_text(id, &s.label);
                if let Some(trend) = &s.trend {
                    self.check_binding(id, trend);
                }
                if let Some(subtext) = &s.subtext {
                    self.check_text(id, subtext);
                }
            }
            NodeKind::Sparkline(s) => self.check_binding(id, &s.source),
            NodeKind::LabelValueRow(s) => {
                self.check_binding(id, &s.value);
                self.check_text(id, &s.label);
                if let Some(help) = &s.help {
                    self.check_text(id, help);
                }
            }
            NodeKind::Fact(s) => {
                self.check_text(id, &s.label);
                self.check_text(id, &s.value);
                if let Some(help) = &s.help {
                    self.check_text(id, help);
                }
            }
            NodeKind::Image(s) => {
                self.check_binding(id, &s.src);
                self.check_text(id, &s.alt);
            }
            NodeKind::Toast(s) => {
                self.check_binding(id, &s.open);
                self.check_text(id, &s.message);
            }
            NodeKind::DataGrid(s) => self.check_binding(id, &s.source),
            NodeKind::Chart(s) => {
                self.check_binding(id, &s.source);
                self.check_chart(id, s);
            }
            NodeKind::Map(s) => self.check_binding(id, &s.source),
            NodeKind::Stepper(s) => self.check_binding(id, &s.active_step),
            NodeKind::FileUpload(s) => {
                if let Some(disabled) = &s.disabled {
                    self.check_binding(id, disabled);
                }
            }
            // Text-slot-only kinds: bound text sources still carry bindings
            // worth linting.
            NodeKind::Heading(s) => self.check_text(id, &s.text),
            NodeKind::Markdown(s) => self.check_text(id, &s.text),
            NodeKind::Badge(s) => self.check_text(id, &s.label),
            NodeKind::Callout(s) => {
                self.check_text(id, &s.body);
                if let Some(heading) = &s.heading {
                    self.check_text(id, heading);
                }
            }
            NodeKind::List(s) => {
                for item in &s.items {
                    self.check_text(id, item);
                }
            }
            NodeKind::Box(s) => {
                if let Some(heading) = &s.heading {
                    self.check_text(id, heading);
                }
            }
            NodeKind::SummaryList(s) => {
                if let Some(heading) = &s.heading {
                    self.check_text(id, heading);
                }
            }
            // No bounded-primitive / binding surface to lint (children are
            // walked by the caller).
            NodeKind::SplitPanel(_)
            | NodeKind::ScrollArea(_)
            | NodeKind::Skeleton(_)
            | NodeKind::CodeBlock(_)
            | NodeKind::Math(_)
            | NodeKind::Custom(_)
            | NodeKind::ErrorBoundary(_)
            | NodeKind::FragmentDecl(_)
            | NodeKind::FragmentRef(_)
            | NodeKind::Drawing(_)
            | NodeKind::Mount(_) => {}
        }
    }

    fn check_form_field(&mut self, id: &str, kind: &FormFieldKind) {
        match kind {
            FormFieldKind::Text { value, on_change }
            | FormFieldKind::Number { value, on_change }
            | FormFieldKind::TextArea {
                value, on_change, ..
            }
            | FormFieldKind::RangedNumber {
                value, on_change, ..
            }
            | FormFieldKind::Date {
                value, on_change, ..
            }
            | FormFieldKind::DateRange {
                value, on_change, ..
            } => {
                if on_change.is_none() {
                    self.check_writable(id, "value", value);
                }
                self.check_binding(id, value);
            }
            FormFieldKind::Range {
                value, on_change, ..
            } => {
                if on_change.is_none() {
                    self.check_writable(id, "value", value);
                }
                self.check_binding(id, value);
            }
            FormFieldKind::Checkbox { value, on_toggle }
            | FormFieldKind::Toggle { value, on_toggle } => {
                if on_toggle.is_none() {
                    self.check_writable(id, "value", value);
                }
                self.check_binding(id, value);
            }
            FormFieldKind::Choice {
                options,
                value,
                on_change,
            }
            | FormFieldKind::SegmentedChoice {
                options,
                value,
                on_change,
                ..
            } => {
                if on_change.is_none() {
                    self.check_writable(id, "value", value);
                }
                self.check_binding(id, options);
                self.check_binding(id, value);
            }
        }
    }

    /// FUARAN086–089 (Phase 640) — schema-grounded ChartSpec validation. An
    /// ungrounded field reference is the language's defect to catch before
    /// lowering — a wrong field name otherwise lowers to a silently
    /// flat/empty chart. Grounding fires only where the schema is statically
    /// known: a `Binding.Transform` over an `Embedded` table with an EMPTY
    /// pipeline (a non-empty pipeline changes the column set; a `Ref` /
    /// `Query` / host `Static` source is unknowable pre-emit and deliberately
    /// passes ungrounded — only refuse what is PROVABLY wrong).
    fn check_chart(&mut self, id: &str, s: &crate::wire::ChartSpec) {
        use crate::wire::{ChartKind, ColumnType, DataSource};

        // FUARAN088 — pie needs exactly one series (the lowering refuses
        // multi-series geometry rather than truncating).
        if matches!(s.kind, ChartKind::Pie) && s.y_fields.len() != 1 {
            self.push(
                Severity::Error,
                "FUARAN088",
                id,
                format!(
                    "pie chart '{id}' declares {} series — the pie lowering refuses anything but exactly one (no silent truncation; Phase 638/640)",
                    s.y_fields.len()
                ),
            );
        }

        // FUARAN089 — Stacked is dead intent outside Bar/Area.
        if s.stacked
            && matches!(
                s.kind,
                ChartKind::Line | ChartKind::Scatter | ChartKind::Pie
            )
        {
            self.push(
                Severity::Warning,
                "FUARAN089",
                id,
                format!(
                    "chart '{id}' sets Stacked=true on kind {} — the lowering ignores it (dead intent; Phase 637/640)",
                    s.kind.as_str()
                ),
            );
        }

        // FUARAN086/087 — grounding, only where the schema is statically known.
        let Binding::Transform {
            source: DataSource::Embedded { schema, .. },
            pipeline,
            ..
        } = &s.source
        else {
            return;
        };
        if !pipeline.is_empty() {
            return;
        }
        let col_type = |name: &str| -> Option<ColumnType> {
            schema
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.column_type)
        };
        let numeric = |t: ColumnType| -> bool {
            matches!(t, ColumnType::Int | ColumnType::Float | ColumnType::Bool)
        };
        let ungrounded = |v: &mut Self, field: &str| {
            v.push(
                Severity::Error,
                "FUARAN086",
                id,
                format!(
                    "chart '{id}' references field '{field}' absent from its statically-known data schema — it would lower silently flat/empty (Phase 640)"
                ),
            );
        };
        let mismatch = |v: &mut Self, field: &str, t: ColumnType| {
            v.push(
                Severity::Error,
                "FUARAN087",
                id,
                format!(
                    "chart '{id}' plots field '{field}' of type '{}' — the lowering reads non-numeric cells as 0.0, a silently flat series (Phase 640)",
                    t.as_str()
                ),
            );
        };
        match col_type(&s.x_field) {
            None => ungrounded(self, &s.x_field),
            Some(t) => {
                if matches!(s.kind, ChartKind::Scatter) && !numeric(t) {
                    mismatch(self, &s.x_field, t);
                }
            }
        }
        for yf in &s.y_fields {
            match col_type(yf) {
                None => ungrounded(self, yf),
                Some(t) => {
                    if !numeric(t) {
                        mismatch(self, yf, t);
                    }
                }
            }
        }
    }

    /// FUARAN069 — the write-back default only arms when the control's own
    /// value binding is directly `State` or `Filter`; `Local` is exempt (its
    /// commit pipeline carries the change). Anything else is an inert control.
    fn check_writable(&mut self, id: &str, slot: &str, binding: &Binding) {
        match binding {
            Binding::State { .. } | Binding::Filter { .. } | Binding::Local { .. } => {}
            Binding::Static { .. }
            | Binding::Query { .. }
            | Binding::Selection { .. }
            | Binding::Computed
            | Binding::Now
            | Binding::I18n { .. }
            | Binding::Format { .. }
            | Binding::Transform { .. }
            | Binding::Invoke { .. } => {
                self.push(
                    Severity::Warning,
                    "FUARAN069",
                    id,
                    format!(
                        "Inert control: the '{slot}' handler is omitted but the binding is not a writable State/Filter slot — the write-back default cannot arm, so the control parses, renders, and does nothing. Bind '{slot}' to Binding.State / Binding.Filter, or supply a handler."
                    ),
                );
            }
        }
    }

    /// A text slot: a `Bound` text source carries a binding worth linting.
    fn check_text(&mut self, id: &str, t: &crate::wire::TextSource) {
        if let crate::wire::TextSource::Bound(b) = t {
            self.check_binding(id, b);
        }
    }

    /// Binding-level lints, recursing through composite bindings.
    fn check_binding(&mut self, id: &str, b: &Binding) {
        match b {
            Binding::Computed => {
                let severity = if self.orchestrated {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                self.push(
                    severity,
                    "FUARAN084",
                    id,
                    "Binding.Computed is host-only — the whole case erases to \"<closure>\" on the wire, so a decoded/replayed tree sees an inert placeholder. Use Binding.State / Binding.Filter for reactive values, Binding.Transform for derivation, or Binding.Format for formatting.".to_string(),
                );
            }
            Binding::Format { format, source, .. } => {
                if let crate::wire::Format::Currency { iso_code } = format
                    && iso_code.trim().is_empty()
                {
                    self.push(
                        Severity::Error,
                        "FUARAN061",
                        id,
                        "Binding.Format Currency has a blank ISO-4217 currency code — the renderer's locale formatter throws at render time. Supply a valid code (e.g. \"GBP\", \"USD\").".to_string(),
                    );
                }
                self.check_binding(id, source);
            }
            Binding::Local { initial_from, .. } => self.check_binding(id, initial_from),
            Binding::I18n { args, .. } => {
                if let Some(args) = args {
                    for (_, arg) in args {
                        self.check_binding(id, arg);
                    }
                }
            }
            Binding::Transform { params, .. } => {
                if let Some(params) = params {
                    for p in params {
                        self.check_binding(id, &p.from);
                    }
                }
            }
            Binding::Static { .. }
            | Binding::Query { .. }
            | Binding::Filter { .. }
            | Binding::Selection { .. }
            | Binding::State { .. }
            | Binding::Now
            | Binding::Invoke { .. } => {}
        }
    }

    /// Action-level lints, recursing through Chain.
    fn check_action(&mut self, id: &str, a: &Action) {
        match a {
            Action::Call {
                into, on_result, ..
            } => {
                if into.is_none() && on_result.is_none() {
                    self.push(
                        Severity::Warning,
                        "FUARAN073",
                        id,
                        "Fire-and-forget Action.Call: no 'into' result target and no onResult — the response is discarded. Add into: State/Query so the result lands in a readable slot, or supply an onResult.".to_string(),
                    );
                }
            }
            Action::Chain(actions) => {
                for inner in actions {
                    self.check_action(id, inner);
                }
            }
            Action::Dispatch
            | Action::Notify { .. }
            | Action::Navigate { .. }
            | Action::SetState { .. }
            | Action::AiTool { .. }
            | Action::CommitLocal { .. }
            | Action::WriteToClipboard { .. }
            | Action::ReadFileBody { .. }
            | Action::Invoke { .. } => {}
        }
    }
}

/// Every immediate sub-node the validator descends into — the same traversal
/// the apply engine's structural checks run.
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
    if let Some(b) = &n.state.on_loading {
        out.push(b);
    }
    if let Some(b) = &n.state.on_empty {
        out.push(b);
    }
    out
}

/// Validate a decoded tree pre-emit. Findings come back in walk order; an
/// empty list means the tree passes the structural surface.
pub fn validate_with(tree: &Node, options: ValidateOptions) -> Vec<Finding> {
    let mut walker = Walker {
        orchestrated: options.orchestrated,
        findings: Vec::new(),
        seen_ids: std::collections::HashMap::new(),
    };
    walker.walk(tree);
    // FUARAN001 — one finding per duplicated id (not per occurrence past the
    // first: the id names the collision).
    let mut duplicate_ids: Vec<&String> = walker
        .seen_ids
        .iter()
        .filter(|&(_, &count)| count > 1)
        .map(|(id, _)| id)
        .collect();
    duplicate_ids.sort();
    let mut findings = walker.findings.clone();
    for id in duplicate_ids {
        findings.push(Finding {
            severity: Severity::Error,
            code: "FUARAN001",
            node_id: id.clone(),
            message: format!(
                "Duplicate NodeId \"{id}\" within the tree — every NodeId must be unique (op-target stability)."
            ),
        });
    }
    findings
}

/// Validate with default options (hand-authored posture: `FUARAN084` warns).
pub fn validate(tree: &Node) -> Vec<Finding> {
    validate_with(tree, ValidateOptions::default())
}

/// Validate a tree-op's payload trees pre-apply: `InsertChild.child` and
/// `ReplaceRoot.node` carry whole subtrees; `Batch` recurses.
pub fn validate_op(op: &TreeOp) -> Vec<Finding> {
    match op {
        TreeOp::InsertChild { child, .. } => validate(child),
        TreeOp::ReplaceRoot { node } => validate(node),
        TreeOp::Batch(ops) => ops.iter().flat_map(validate_op).collect(),
        TreeOp::EditNode { .. }
        | TreeOp::UpdateProp { .. }
        | TreeOp::ReplaceBinding { .. }
        | TreeOp::UpdateStyle { .. }
        | TreeOp::UpdateState { .. }
        | TreeOp::RemoveNode { .. }
        | TreeOp::MoveNode { .. }
        | TreeOp::ReorderChildren { .. } => vec![],
    }
}
