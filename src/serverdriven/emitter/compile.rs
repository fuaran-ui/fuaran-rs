//! The desktop tree compiler (Phase 2452, `std` only): certify a panel and turn it
//! into the constant [`Image`] a device compiles in.
//!
//! Certification is everything the device cannot do for itself, done once, with
//! this host's full codec, validator and apply engine as the oracles:
//!
//! 1. the panel and every fragment encode to canonical bytes that re-encode
//!    byte-identical (`canonical-bytes`);
//! 2. the panel, with every fragment inserted, has no validator error;
//! 3. every node kind, control kind and op kind lies inside the declared subset
//!    (the `kind.$type` at a node position against [`SUBSET_KINDS`], at a control
//!    position — a `Form`'s `fields[]` — against [`SUBSET_FORM_FIELD_KINDS`]);
//! 4. no number token carries a fraction or an exponent (`integer-only`, read on
//!    the token as §11.4 says, not on the value);
//! 5. every handler the tree registers is one the server-driven driver admits for
//!    its kind (the same legitimacy table);
//! 6. every write the device will make — each fragment's insert and removal, each
//!    reading, each property — is written by the device's own op writers and
//!    APPLIES to the panel, and re-encodes byte-identical.
//!
//! The output is [`CompiledPanel`]: the image as owned values
//! ([`CompiledPanel::with_image`]) and as Rust source ([`CompiledPanel::to_rust_source`]).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::{
    Fragment, Handler, Image, PROFILE_REVISION, SUBSET_FORM_FIELD_KINDS, SUBSET_KINDS, SUBSET_OPS,
    Value, ValueKind, Write, insert_child, remove_node, replace_binding, replace_root, update_prop,
};
use crate::canonical::{JVal, parse};
use crate::ops::{all_node_ids, apply};
use crate::serverdriven::driver::legitimate_event;
use crate::validator::{Severity, validate};
use crate::wire::{Node, decode_node, decode_op, encode_node, encode_op};

/// A declared write: the node, the field (slot or property path) and the value kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteSpec {
    pub target: String,
    pub field: String,
    pub kind: ValueKind,
}

impl WriteSpec {
    pub fn new(target: &str, field: &str, kind: ValueKind) -> Self {
        WriteSpec {
            target: target.to_string(),
            field: field.to_string(),
            kind,
        }
    }
}

/// What the desktop author hands the compiler.
#[derive(Debug, Clone)]
pub struct PanelSpec {
    /// The panel.
    pub root: Node,
    /// The subtrees that come and go: (parent id, subtree).
    pub fragments: Vec<(String, Node)>,
    /// The readings — `ReplaceBinding` targets (field = binding slot).
    pub readings: Vec<WriteSpec>,
    /// The properties — `UpdateProp` targets (field = property path).
    pub props: Vec<WriteSpec>,
}

/// A certified panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPanel {
    pub revision: String,
    pub root: String,
    pub nodes: Vec<String>,
    /// (parent id, fragment root id, canonical node document).
    pub fragments: Vec<(String, String, String)>,
    pub readings: Vec<WriteSpec>,
    pub props: Vec<WriteSpec>,
    /// (node id, event name).
    pub handlers: Vec<(String, String)>,
    pub state_keys: Vec<String>,
}

/// Why a panel was refused, with a stable code and the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub code: &'static str,
    pub detail: String,
}

fn refuse<T>(code: &'static str, detail: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError {
        code,
        detail: detail.into(),
    })
}

/// Encode `node` and prove the bytes canonical.
fn canonical_bytes(node: &Node, what: &str) -> Result<String, CompileError> {
    let bytes = encode_node(node);
    match decode_node(&bytes) {
        Ok(back) if encode_node(&back) == bytes => Ok(bytes),
        Ok(_) => refuse(
            "NOT_CANONICAL",
            format!("{what} does not re-encode byte-identical"),
        ),
        Err(e) => refuse("NOT_CANONICAL", format!("{what} does not decode: {e:?}")),
    }
}

/// The `integer-only` axis over a document's bytes: no number token outside a
/// string carries `.`, `e` or `E`.
fn integer_only(bytes: &str, what: &str) -> Result<(), CompileError> {
    let b = bytes.as_bytes();
    let (mut i, mut in_string) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
        } else if c == b'"' {
            in_string = true;
            i += 1;
        } else if c == b'-' || c.is_ascii_digit() {
            let start = i;
            while i < b.len() && matches!(b[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                i += 1;
            }
            let token = &bytes[start..i];
            if token.contains(['.', 'e', 'E']) {
                return refuse(
                    "NOT_INTEGER_ONLY",
                    format!("{what} carries the number token {token}"),
                );
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// What one walk of a document collects.
#[derive(Default)]
struct Walk {
    handlers: Vec<(String, String)>,
    state_keys: BTreeSet<String>,
}

/// The node-borne handler members and the event each registers.
const HANDLER_MEMBERS: &[(&str, &str)] = &[
    ("onClick", "click"),
    ("onSubmit", "submit"),
    ("onChange", "change"),
];

fn type_of(kind: &JVal) -> Option<&str> {
    match kind.field("$type") {
        Some(JVal::Str(t)) => Some(t),
        _ => None,
    }
}

/// A node position: `{"id":…,"kind":{"$type":…}}`.
fn as_node(v: &JVal) -> Option<(&str, &JVal)> {
    match (v.field("id"), v.field("kind")) {
        (Some(JVal::Str(id)), Some(kind @ JVal::Obj(_))) if type_of(kind).is_some() => {
            Some((id, kind))
        }
        _ => None,
    }
}

fn walk_node(id: &str, kind: &JVal, w: &mut Walk) -> Result<(), CompileError> {
    let t = type_of(kind).unwrap_or_default();
    if !SUBSET_KINDS.contains(&t) {
        return refuse(
            "OUTSIDE_SUBSET",
            format!("node '{id}' is a {t}, outside the declared kinds"),
        );
    }
    for (member, event) in HANDLER_MEMBERS {
        if kind.field(member).is_some() {
            if !legitimate_event(t, event) {
                return refuse(
                    "ILLEGITIMATE_HANDLER",
                    format!("node '{id}' ({t}) registers '{event}', which the driver refuses"),
                );
            }
            w.handlers.push((id.to_string(), event.to_string()));
        }
    }
    if let JVal::Obj(members) = kind {
        for (k, v) in members {
            if k == "fields" && t == "Form" {
                walk_controls(id, v, w)?;
            } else {
                walk_value(v, w)?;
            }
        }
    }
    Ok(())
}

fn walk_controls(form: &str, fields: &JVal, w: &mut Walk) -> Result<(), CompileError> {
    let JVal::Arr(items) = fields else {
        return Ok(());
    };
    for item in items {
        if let Some(kind) = item.field("kind") {
            let t = type_of(kind).unwrap_or_default();
            if !SUBSET_FORM_FIELD_KINDS.contains(&t) {
                return refuse(
                    "OUTSIDE_SUBSET",
                    format!("form '{form}' carries a {t} control, outside the declared controls"),
                );
            }
        }
        // A control has a node's shape and is not a node: walk its members.
        if let JVal::Obj(members) = item {
            for (_, m) in members {
                walk_value(m, w)?;
            }
        }
    }
    Ok(())
}

fn walk_value(v: &JVal, w: &mut Walk) -> Result<(), CompileError> {
    if let Some((id, kind)) = as_node(v) {
        return walk_node(id, kind, w);
    }
    match v {
        JVal::Obj(members) => {
            if let (Some(JVal::Str(t)), Some(JVal::Str(key))) = (v.field("$type"), v.field("key")) {
                if t == "State" {
                    w.state_keys.insert(key.clone());
                }
            }
            for (_, m) in members {
                walk_value(m, w)?;
            }
        }
        JVal::Arr(items) => {
            for m in items {
                walk_value(m, w)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn walk_document(bytes: &str, w: &mut Walk) -> Result<(), CompileError> {
    let v = parse(bytes).map_err(|e| CompileError {
        code: "NOT_CANONICAL",
        detail: e.message,
    })?;
    walk_value(&v, w)
}

/// Prove one op the device will write: canonical, inside the op subset, and
/// applying to `tree`. Answers the tree after it.
fn certify_op(tree: &Node, op: &str, what: &str) -> Result<Node, CompileError> {
    let decoded = match decode_op(op) {
        Ok(d) if encode_op(&d) == op => d,
        Ok(_) => return refuse("NOT_CANONICAL", format!("{what} is not canonical: {op}")),
        Err(e) => return refuse("NOT_CANONICAL", format!("{what} does not decode: {e:?}")),
    };
    let tag = op
        .strip_prefix("{\"$type\":\"")
        .and_then(|s| s.split('"').next())
        .unwrap_or_default();
    if !SUBSET_OPS.contains(&tag) {
        return refuse("OUTSIDE_SUBSET", format!("{what} is a {tag} op"));
    }
    integer_only(op, what)?;
    match apply(tree, &decoded) {
        Ok(outcome) => Ok(outcome.new_tree),
        Err(e) => refuse("WRITE_DOES_NOT_APPLY", format!("{what}: {e:?}")),
    }
}

fn representative(kind: ValueKind) -> Value<'static> {
    match kind {
        ValueKind::Int => Value::Int(0),
        ValueKind::Str => Value::Str("-"),
        ValueKind::Bool => Value::Bool(false),
    }
}

/// Certify a panel and compile it.
pub fn compile(spec: &PanelSpec) -> Result<CompiledPanel, CompileError> {
    let root = canonical_bytes(&spec.root, "the panel")?;
    integer_only(&root, "the panel")?;
    let mut walk = Walk::default();
    walk_document(&root, &mut walk)?;

    let mut nodes = all_node_ids(&spec.root);
    let mut fragments = Vec::with_capacity(spec.fragments.len());
    for (parent, frag) in &spec.fragments {
        let what = format!("fragment '{}'", frag.id);
        let bytes = canonical_bytes(frag, &what)?;
        integer_only(&bytes, &what)?;
        walk_document(&bytes, &mut walk)?;
        nodes.extend(all_node_ids(frag));
        fragments.push((parent.clone(), frag.id.clone(), bytes));
    }
    let mut seen = BTreeSet::new();
    for id in &nodes {
        if !seen.insert(id.as_str()) {
            return refuse("DUPLICATE_ID", format!("node id '{id}' appears twice"));
        }
    }

    let compiled = CompiledPanel {
        revision: PROFILE_REVISION.to_string(),
        root,
        nodes,
        fragments,
        readings: spec.readings.clone(),
        props: spec.props.clone(),
        handlers: walk.handlers,
        state_keys: walk.state_keys.into_iter().collect(),
    };

    // Every write, through the device's own writers, against the panel with
    // every fragment present.
    compiled.with_image(|image| -> Result<(), CompileError> {
        let mut tree = spec.root.clone();
        for i in 0..image.fragments.len() {
            let op = insert_child(image, i).expect("index in range");
            tree = certify_op(&tree, &op, &format!("insert of fragment {i}"))?;
        }
        for f in validate(&tree) {
            if f.severity == Severity::Error {
                return refuse(
                    "TREE_INVALID",
                    format!("{} at '{}': {}", f.code, f.node_id, f.message),
                );
            }
        }
        for (i, w) in image.readings.iter().enumerate() {
            let op = replace_binding(image, i, representative(w.kind)).expect("index in range");
            certify_op(&tree, &op, &format!("reading {i}"))?;
        }
        for (i, w) in image.props.iter().enumerate() {
            let op = update_prop(image, i, representative(w.kind)).expect("index in range");
            certify_op(&tree, &op, &format!("property {i}"))?;
        }
        for i in 0..image.fragments.len() {
            let op = remove_node(image, i).expect("index in range");
            certify_op(&tree, &op, &format!("removal of fragment {i}"))?;
        }
        certify_op(&tree, &replace_root(image), "the first frame")?;
        Ok(())
    })?;
    Ok(compiled)
}

fn write_of(w: &WriteSpec) -> Write<'_> {
    Write {
        target: &w.target,
        field: &w.field,
        kind: w.kind,
    }
}

fn kind_source(k: ValueKind) -> &'static str {
    match k {
        ValueKind::Int => "ValueKind::Int",
        ValueKind::Str => "ValueKind::Str",
        ValueKind::Bool => "ValueKind::Bool",
    }
}

impl CompiledPanel {
    /// Lend the compiled panel as an [`Image`] — what the device holds as
    /// constants, here borrowed from owned values.
    pub fn with_image<R>(&self, f: impl FnOnce(&Image<'_>) -> R) -> R {
        let nodes: Vec<&str> = self.nodes.iter().map(String::as_str).collect();
        let fragments: Vec<Fragment<'_>> = self
            .fragments
            .iter()
            .map(|(parent, id, node)| Fragment { parent, id, node })
            .collect();
        let readings: Vec<Write<'_>> = self.readings.iter().map(write_of).collect();
        let props: Vec<Write<'_>> = self.props.iter().map(write_of).collect();
        let handlers: Vec<Handler<'_>> = self
            .handlers
            .iter()
            .map(|(node, event)| Handler { node, event })
            .collect();
        let state_keys: Vec<&str> = self.state_keys.iter().map(String::as_str).collect();
        f(&Image {
            revision: &self.revision,
            root: &self.root,
            nodes: &nodes,
            fragments: &fragments,
            readings: &readings,
            props: &props,
            handlers: &handlers,
            state_keys: &state_keys,
        })
    }

    /// The image as Rust source: `const <name>: Image<'static> = Image { … };`,
    /// naming `Image`, `Fragment`, `Write`, `Handler` and `ValueKind` unqualified.
    /// One member per line and every list vertical, so a device crate can keep it
    /// verbatim under `#[rustfmt::skip]`.
    pub fn to_rust_source(&self, name: &str) -> String {
        let mut s = String::new();
        let list = |s: &mut String, member: &str, items: Vec<String>| {
            if items.is_empty() {
                let _ = writeln!(s, "    {member}: &[],");
            } else {
                let _ = writeln!(s, "    {member}: &[");
                for it in items {
                    let _ = writeln!(s, "        {it},");
                }
                let _ = writeln!(s, "    ],");
            }
        };
        let write_items = |ws: &[WriteSpec]| {
            ws.iter()
                .map(|w| {
                    format!(
                        "Write {{ target: {:?}, field: {:?}, kind: {} }}",
                        w.target,
                        w.field,
                        kind_source(w.kind)
                    )
                })
                .collect::<Vec<_>>()
        };
        let _ = writeln!(s, "const {name}: Image<'static> = Image {{");
        let _ = writeln!(s, "    revision: {:?},", self.revision);
        let _ = writeln!(s, "    root: {:?},", self.root);
        list(
            &mut s,
            "nodes",
            self.nodes.iter().map(|n| format!("{n:?}")).collect(),
        );
        list(
            &mut s,
            "fragments",
            self.fragments
                .iter()
                .map(|(p, i, n)| format!("Fragment {{ parent: {p:?}, id: {i:?}, node: {n:?} }}"))
                .collect(),
        );
        list(&mut s, "readings", write_items(&self.readings));
        list(&mut s, "props", write_items(&self.props));
        list(
            &mut s,
            "handlers",
            self.handlers
                .iter()
                .map(|(n, e)| format!("Handler {{ node: {n:?}, event: {e:?} }}"))
                .collect(),
        );
        list(
            &mut s,
            "state_keys",
            self.state_keys.iter().map(|k| format!("{k:?}")).collect(),
        );
        s.push_str("};\n");
        s
    }
}
