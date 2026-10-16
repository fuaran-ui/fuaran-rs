//! The host's own generator over the wire corpus's published `idl.json`
//! (Phase 2298). It reads the IDL — the language-neutral data rendering of the
//! wire vocabulary that the corpus publishes beside `manifest.json` — and writes
//! `src/wire/generated.rs`, the STRUCTURAL layer of this host:
//!
//! - every bare-string enum a wire position references, as the `bare_enum!`
//!   invocation the rest of the crate already uses (host case names from the
//!   IDL's `hostCases` where the wire token differs);
//! - the kind set: [`NodeKind::type_name`](super::NodeKind::type_name),
//!   [`NodeKind::category`](super::NodeKind::category),
//!   `CANONICAL_NODE_KINDS` and the unknown-kind hint, all from the IDL's kind
//!   rows;
//! - the discriminator sets of every tagged union and of `TreeOp`, as tag lists
//!   the decoder's refusal hints read, each beside an exhaustive `match` over the
//!   Rust enum that names every IDL case;
//! - a field pin per modelled kind spec, record and op: an exhaustive
//!   destructuring pattern naming exactly the IDL's wire fields;
//! - the omit-at-default table as typed constants (`defaults`), which the decoder
//!   fills absent fields from and the encoder omits by.
//!
//! **Why the pins are code and not tests.** Rust has no reflection over a type's
//! variants or fields, so a hand-kept list beside an enum can only be checked by a
//! test that compares it with something else. A generated `match` with no
//! catch-all arm, and a generated struct pattern with no `..`, are checked by the
//! COMPILER: an IDL kind, case or field this host does not model is a build error
//! naming it, and so is a Rust variant or field the IDL does not have. The kind
//! set the corpus declares is therefore no longer pinned by a test — the
//! regeneration drift test in `tests/conformance.rs` plus the compiler make the
//! drift it caught impossible.
//!
//! **What stays hand-written, deliberately.** The spec structs themselves, decode
//! policy, refusal wording, leniency, relational and limit checks, the encoder's
//! behaviour-bearing paths, rendering and the C ABI. The structs stay hand-written
//! because this host's representation is not a uniform function of the IDL type:
//! a handful of slots are modelled differently on purpose (a predicate pair folded
//! into one `SwitchCondition`, a `stateKey` folded into the `on` binding, a
//! closure slot whose presence is implied). Those are declared in
//! [`FIELD_OVERLAY`], once, with the reason, and the pins hold every other field
//! to the IDL by name.
//!
//! **Ruling (2026-10-16).** The generator is host-owned and
//! reads the published IDL; it is not a target of any other tier, and no codec
//! code is shared with another host. Reading a language-neutral data artefact with
//! the host's own small generator is not a transpile — the posture in this
//! repository's `CLAUDE.md` says so.
//!
//! **Placement.** No `build.rs`: the published crate never needs the corpus to
//! compile. The generator is compiled only under the off-by-default
//! `idl-codegen` feature, which the crate's self dev-dependency turns on for its
//! own test suite. It is driven by `tests/conformance.rs`, which locates the
//! corpus the same way every other corpus leg does (`FUARAN_WIRE_FIXTURES`, else
//! the nearest `wire-format-fixtures/`):
//!
//! ```text
//! FUARAN_IDL_REGENERATE=1 cargo test --test conformance generated_module
//! ```
//!
//! rewrites `src/wire/generated.rs`; without the variable the same test is the
//! drift check, red when `idl.json` has moved and the module has not.

use std::fmt::Write as _;

use crate::canonical::{JVal, parse};
use crate::opstream::sha256_hex;

/// The relative path, from the crate root, of the module this generator writes.
pub const GENERATED_PATH: &str = "src/wire/generated.rs";

/// The node category spellings of the IDL, mapped to this host's
/// [`NodeCategory`](super::NodeCategory) variants and the article the
/// unknown-kind hint uses, in the order the hint lists them. The IDL calls the
/// structural kinds `Meta`; this host's public enum has always called them
/// `Structural`, and a public variant name is kept. An IDL category missing here
/// is a generator error, never a silent bucket.
const CATEGORIES: &[(&str, &str, &str)] = &[
    ("Layout", "Layout", "a"),
    ("Display", "Display", "a"),
    ("Input", "Input", "an"),
    ("Visualisation", "Visualisation", "a"),
    ("Meta", "Structural", "a"),
];

/// IDL type name to this host's Rust type name, where the two differ.
const TYPE_NAMES: &[(&str, &str)] = &[
    ("DataGrid", "GridSpec"),
    ("GuestChannel", "MountChannel"),
    ("Scalar", "FragmentScalar"),
];

/// IDL kinds, records and unions this host does not model as one Rust type of
/// the same shape, so no field or case pin is generated for them. Each carries
/// its reason.
const UNPINNED: &[(&str, &str)] = &[
    (
        "Filters",
        "the NodeKind variant holds Vec<FilterSpec> directly; FilterSpec is pinned",
    ),
    (
        "ButtonGroupItem",
        "inlined into CellKindErased::ButtonGroup",
    ),
    (
        "DateTimeRangePair",
        "inlined into the DateTimeRange control's value",
    ),
    ("RangePair", "inlined into the Range control's value"),
    (
        "FragmentArg",
        "modelled as Value(FragmentScalar) | Slot; its tag set is still generated",
    ),
];

/// Per-field representation choices this host makes on purpose. `None` means
/// the IDL field has no Rust field at all; `Some(name)` names the Rust field
/// that carries it.
const FIELD_OVERLAY: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "FileUpload",
        "onSelect",
        None,
        "closure slot whose presence is implied: the encoder always writes the sentinel",
    ),
    (
        "Stepper",
        "onSelect",
        None,
        "closure slot whose presence is implied: the encoder always writes the sentinel",
    ),
    (
        "Mount",
        "onBubble",
        None,
        "closure slot whose presence is implied: the encoder always writes the sentinel",
    ),
    (
        "Switch",
        "stateKey",
        Some("on"),
        "the state-keyed form is one of the `on` binding's shapes",
    ),
    (
        "SwitchCase",
        "match",
        Some("condition"),
        "the match/when pair is one SwitchCondition",
    ),
    (
        "SwitchCase",
        "when",
        Some("condition"),
        "the match/when pair is one SwitchCondition",
    ),
];

/// Rust keywords an IDL field or case name can collide with, and how this host
/// spells them: a raw identifier for a field, a trailing underscore for an enum
/// case (the `bare_enum!` macro cannot take a raw identifier as a case name).
const FIELD_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct",
    "trait", "type", "unsafe", "use", "where", "while", "async", "await", "dyn",
];
const CASE_KEYWORDS: &[&str] = &["Self"];

#[derive(Debug)]
struct Field {
    name: String,
    optionality: String,
    ty: JVal,
    default: Option<JVal>,
}

#[derive(Debug)]
struct Owner {
    name: String,
    fields: Vec<Field>,
}

#[derive(Debug)]
struct Kind {
    owner: Owner,
    category: String,
}

#[derive(Debug)]
struct Union {
    name: String,
    cases: Vec<Owner>,
}

#[derive(Debug)]
struct Enum {
    name: String,
    cases: Vec<String>,
    host_cases: Vec<String>,
}

struct Idl {
    version: f64,
    kinds: Vec<Kind>,
    records: Vec<Owner>,
    unions: Vec<Union>,
    enums: Vec<Enum>,
    ops: Vec<Owner>,
    node_fields: Vec<Field>,
}

fn str_of<'a>(j: &'a JVal, key: &str, at: &str) -> Result<&'a str, String> {
    match j.field(key) {
        Some(JVal::Str(s)) => Ok(s),
        _ => Err(format!("{at}: expected a string '{key}'")),
    }
}

fn arr_of<'a>(j: &'a JVal, key: &str, at: &str) -> Result<&'a [JVal], String> {
    match j.field(key) {
        Some(JVal::Arr(items)) => Ok(items),
        _ => Err(format!("{at}: expected an array '{key}'")),
    }
}

fn read_fields(j: &JVal, at: &str) -> Result<Vec<Field>, String> {
    let items = match j.field("fields") {
        Some(JVal::Arr(items)) => items.as_slice(),
        None => &[],
        Some(_) => return Err(format!("{at}: 'fields' is not an array")),
    };
    items
        .iter()
        .map(|f| {
            let name = str_of(f, "name", at)?.to_string();
            let opt = f
                .field("optionality")
                .ok_or_else(|| format!("{at}.{name}: no optionality"))?;
            Ok(Field {
                optionality: str_of(opt, "$type", &format!("{at}.{name}"))?.to_string(),
                default: opt.field("default").cloned(),
                ty: f
                    .field("type")
                    .cloned()
                    .ok_or_else(|| format!("{at}.{name}: no type"))?,
                name,
            })
        })
        .collect()
}

fn read_idl(text: &str) -> Result<Idl, String> {
    let j = parse(text).map_err(|e| format!("idl.json does not parse: {e:?}"))?;
    let version = match j.field("version") {
        Some(JVal::Num(n)) => *n,
        _ => return Err("idl.json: no numeric 'version'".into()),
    };
    let mut kinds = Vec::new();
    for k in arr_of(&j, "kinds", "idl")? {
        let tag = str_of(k, "tag", "kinds[]")?.to_string();
        kinds.push(Kind {
            category: str_of(k, "category", &tag)?.to_string(),
            owner: Owner {
                fields: read_fields(k, &tag)?,
                name: tag,
            },
        });
    }
    let mut records = Vec::new();
    for r in arr_of(&j, "records", "idl")? {
        let name = str_of(r, "name", "records[]")?.to_string();
        records.push(Owner {
            fields: read_fields(r, &name)?,
            name,
        });
    }
    let mut unions = Vec::new();
    for u in arr_of(&j, "unions", "idl")? {
        let name = str_of(u, "name", "unions[]")?.to_string();
        let mut cases = Vec::new();
        for c in arr_of(u, "cases", &name)? {
            let tag = str_of(c, "tag", &name)?.to_string();
            cases.push(Owner {
                fields: read_fields(c, &format!("{name}.{tag}"))?,
                name: tag,
            });
        }
        unions.push(Union { name, cases });
    }
    let mut enums = Vec::new();
    for e in arr_of(&j, "enums", "idl")? {
        let name = str_of(e, "name", "enums[]")?.to_string();
        let strs = |key: &str| -> Result<Vec<String>, String> {
            match e.field(key) {
                None => Ok(Vec::new()),
                Some(JVal::Arr(items)) => items
                    .iter()
                    .map(|c| match c {
                        JVal::Str(s) => Ok(s.clone()),
                        _ => Err(format!("{name}.{key}: a non-string case")),
                    })
                    .collect(),
                Some(_) => Err(format!("{name}.{key}: not an array")),
            }
        };
        let cases = strs("cases")?;
        let mut host_cases = strs("hostCases")?;
        if host_cases.is_empty() {
            host_cases = cases.clone();
        }
        if host_cases.len() != cases.len() {
            return Err(format!("{name}: hostCases and cases differ in length"));
        }
        enums.push(Enum {
            name,
            cases,
            host_cases,
        });
    }
    let mut ops = Vec::new();
    for o in arr_of(&j, "ops", "idl")? {
        let tag = str_of(o, "tag", "ops[]")?.to_string();
        ops.push(Owner {
            fields: read_fields(o, &tag)?,
            name: tag,
        });
    }
    let node_fields = read_fields(
        &JVal::Obj(vec![(
            "fields".to_string(),
            j.field("nodeFields")
                .cloned()
                .ok_or("idl.json: no 'nodeFields'")?,
        )]),
        "nodeFields",
    )?;
    Ok(Idl {
        version,
        kinds,
        records,
        unions,
        enums,
        ops,
        node_fields,
    })
}

fn rust_type_name(idl: &str) -> String {
    TYPE_NAMES
        .iter()
        .find(|(i, _)| *i == idl)
        .map_or_else(|| idl.to_string(), |(_, r)| (*r).to_string())
}

fn is_unpinned(name: &str) -> bool {
    UNPINNED.iter().any(|(n, _)| *n == name)
}

fn snake(camel: &str) -> String {
    let mut out = String::new();
    for c in camel.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn screaming(camel: &str) -> String {
    let mut out = String::new();
    for (i, c) in camel.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

fn field_ident(idl_name: &str) -> String {
    let s = snake(idl_name);
    if FIELD_KEYWORDS.contains(&s.as_str()) {
        format!("r#{s}")
    } else {
        s
    }
}

fn case_ident(host_case: &str) -> String {
    if CASE_KEYWORDS.contains(&host_case) {
        format!("{host_case}_")
    } else {
        host_case.to_string()
    }
}

fn ty_tag(ty: &JVal) -> &str {
    match ty.field("$type") {
        Some(JVal::Str(s)) => s,
        _ => "",
    }
}

/// Every enum a wire-visible position names, transitively through nothing: the
/// IDL's type positions name enums directly, so one walk over every non-host-only
/// field type is the reachable set. An enum only a host-only slot names (the
/// in-process `motion` closure's `Motion`) has no wire spelling to round-trip.
fn wire_enums(idl: &Idl) -> std::collections::BTreeSet<String> {
    fn walk(ty: &JVal, out: &mut std::collections::BTreeSet<String>) {
        if ty_tag(ty) == "enum"
            && let Some(JVal::Str(n)) = ty.field("name")
        {
            out.insert(n.clone());
        }
        if let JVal::Obj(fields) = ty {
            for (_, v) in fields {
                walk(v, out);
            }
        }
        if let JVal::Arr(items) = ty {
            for v in items {
                walk(v, out);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    let owners = idl
        .kinds
        .iter()
        .map(|k| &k.owner)
        .chain(idl.records.iter())
        .chain(idl.ops.iter())
        .chain(idl.unions.iter().flat_map(|u| u.cases.iter()));
    let fields = owners
        .flat_map(|o| o.fields.iter())
        .chain(idl.node_fields.iter());
    for f in fields {
        if f.optionality != "hostOnly" {
            walk(&f.ty, &mut out);
        }
    }
    out
}

/// The wire fields of an owner as this host names them, after the overlay.
/// Deduplicated, in IDL order.
fn rust_fields(owner: &str, fields: &[Field]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in fields {
        if f.optionality == "hostOnly" {
            continue;
        }
        let ident = match FIELD_OVERLAY
            .iter()
            .find(|(o, n, _, _)| *o == owner && *n == f.name)
        {
            Some((_, _, None, _)) => continue,
            Some((_, _, Some(r), _)) => (*r).to_string(),
            None => field_ident(&f.name),
        };
        if !out.contains(&ident) {
            out.push(ident);
        }
    }
    out
}

fn enum_by_name<'a>(idl: &'a Idl, name: &str) -> Option<&'a Enum> {
    idl.enums.iter().find(|e| e.name == name)
}

/// The typed Rust expression for an omit-at-default value, or `None` for a
/// default the generator does not render as a constant (an empty list, a record,
/// a union case that carries fields) — those stay as the codec's emptiness or
/// structural checks.
fn default_expr(idl: &Idl, ty: &JVal, default: &JVal) -> Result<Option<(String, String)>, String> {
    match ty_tag(default) {
        "bool" => match default.field("value") {
            Some(JVal::Bool(b)) => Ok(Some(("bool".into(), b.to_string()))),
            _ => Err("a bool default without a boolean value".into()),
        },
        "enum" => {
            let name = str_of(ty, "name", "enum default type")?;
            let case = str_of(default, "case", name)?;
            let e = enum_by_name(idl, name).ok_or_else(|| format!("unknown enum {name}"))?;
            let i = e
                .cases
                .iter()
                .position(|c| c == case)
                .ok_or_else(|| format!("{name}: default case {case} is not a case"))?;
            let rust = rust_type_name(name);
            Ok(Some((
                rust.clone(),
                format!("{rust}::{}", case_ident(&e.host_cases[i])),
            )))
        }
        "union" => {
            let fields = arr_of(default, "fields", "union default")?;
            if !fields.is_empty() {
                return Ok(None);
            }
            let name = str_of(ty, "name", "union default type")?;
            let tag = str_of(default, "tag", name)?;
            let rust = rust_type_name(name);
            Ok(Some((rust.clone(), format!("{rust}::{tag}"))))
        }
        _ => Ok(None),
    }
}

fn str_list(items: &[String]) -> String {
    let mut out = String::from("&[\n");
    for i in items {
        let _ = writeln!(out, "    \"{i}\",");
    }
    out.push(']');
    out
}

/// Generate `src/wire/generated.rs` from the bytes of `idl.json`.
///
/// # Errors
///
/// A message naming the IDL position the generator cannot render — an unknown
/// category, an enum default naming no case, a malformed row. The generator never
/// emits a partial module.
pub fn generate(idl_text: &str) -> Result<String, String> {
    let idl = read_idl(idl_text)?;
    let mut out = String::new();
    let w = &mut out;

    let _ = writeln!(
        w,
        "// GENERATED by src/wire/idl_codegen.rs from the wire corpus's idl.json. DO NOT EDIT.
//
// Regenerate (the drift test in tests/conformance.rs is red until you do):
//
//     FUARAN_IDL_REGENERATE=1 cargo test --test conformance generated_module
//
// Everything structural about the wire vocabulary that this host does not choose
// for itself lives here; behaviour stays in model.rs, decode.rs and encode.rs.

use super::model::*;
use crate::core::bare_enum;

/// SHA-256 of the exact `idl.json` bytes this module was generated from.
pub const IDL_SHA256: &str = \"{}\";

/// The `version` the IDL declared when this module was generated.
pub const IDL_VERSION: u32 = {};",
        sha256_hex(idl_text),
        idl.version
    );

    // ── Enums ───────────────────────────────────────────────────────────────
    let _ = writeln!(
        w,
        "\n// ─── Bare-string enums (§3.5) ───────────────────────────────────────────────\n"
    );
    let reachable = wire_enums(&idl);
    for e in idl.enums.iter().filter(|e| reachable.contains(&e.name)) {
        let cases: Vec<String> = e
            .cases
            .iter()
            .zip(&e.host_cases)
            .map(|(wire, host)| format!("{} => \"{wire}\"", case_ident(host)))
            .collect();
        let _ = writeln!(
            w,
            "bare_enum!({} {{ {} }});",
            rust_type_name(&e.name),
            cases.join(", ")
        );
    }

    // ── Kinds ───────────────────────────────────────────────────────────────
    let _ = writeln!(
        w,
        "\n// ─── The kind set (§3.2) ────────────────────────────────────────────────────\n"
    );
    for k in &idl.kinds {
        if !CATEGORIES.iter().any(|(c, _, _)| *c == k.category) {
            return Err(format!(
                "kind {}: category {} has no NodeCategory mapping",
                k.owner.name, k.category
            ));
        }
    }
    let tags: Vec<String> = idl.kinds.iter().map(|k| k.owner.name.clone()).collect();
    let _ = writeln!(
        w,
        "/// The emittable NodeKind vocabulary: one entry per IDL kind, in IDL order.
pub const CANONICAL_NODE_KINDS: &[&str] = {};
",
        str_list(&tags)
    );
    let _ = writeln!(
        w,
        "impl NodeKind {{
    /// The wire `$type` discriminator name of this kind (e.g. `\"Metric\"`,
    /// `\"DataGrid\"`, `\"Box\"`) — the stable structural name introspection and
    /// structural search key off.
    pub fn type_name(&self) -> &'static str {{
        match self {{"
    );
    for t in &tags {
        let _ = writeln!(w, "            NodeKind::{t}(_) => \"{t}\",");
    }
    let _ = writeln!(
        w,
        "        }}
    }}

    /// The host-side behavioural classification of this primitive (§3.2).
    pub fn category(&self) -> NodeCategory {{
        match self {{"
    );
    for k in &idl.kinds {
        let (_, rust, _) = CATEGORIES
            .iter()
            .find(|(c, _, _)| *c == k.category)
            .expect("checked above");
        let _ = writeln!(
            w,
            "            NodeKind::{}(_) => NodeCategory::{rust},",
            k.owner.name
        );
    }
    let _ = writeln!(w, "        }}\n    }}\n}}\n");
    let mut groups = Vec::new();
    for (idl_cat, rust, article) in CATEGORIES {
        let members: Vec<&str> = idl
            .kinds
            .iter()
            .filter(|k| k.category == *idl_cat)
            .map(|k| k.owner.name.as_str())
            .collect();
        if !members.is_empty() {
            groups.push(format!(
                "{article} {rust} primitive ({})",
                members.join(" | ")
            ));
        }
    }
    let hint = match groups.split_last() {
        Some((last, init)) if !init.is_empty() => format!("{}, or {last}", init.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    };
    let _ = writeln!(
        w,
        "/// The `WRONG_NODE_KIND` refusal hint: every kind, grouped by category.
pub(crate) const WRONG_NODE_KIND_HINT: &str = \"{hint}\";"
    );

    // ── Discriminator sets ──────────────────────────────────────────────────
    let _ = writeln!(
        w,
        "\n// ─── Discriminator sets (§3.3) ──────────────────────────────────────────────
//
// One tag list per tagged union, in IDL order, which the decoder's
// UNKNOWN_DU_CASE hints read; beside each, a match over the Rust enum with no
// catch-all arm, so a case on either side and not the other is a build error.\n"
    );
    for u in &idl.unions {
        let tags: Vec<String> = u.cases.iter().map(|c| c.name.clone()).collect();
        let konst = format!("{}_TAGS", screaming(&u.name));
        if u.name == "FormFieldKind" {
            let _ = writeln!(
                w,
                "/// The emittable FormFieldKind control vocabulary, in IDL order. One control
/// vocabulary covers both the `Form.fields` and `Filters.items` carriers.
pub const CANONICAL_FORM_FIELD_KINDS: &[&str] = {};
",
                str_list(&tags)
            );
        } else {
            let _ = writeln!(
                w,
                "pub(crate) const {konst}: &[&str] = {};\n",
                str_list(&tags)
            );
        }
        if is_unpinned(&u.name) {
            continue;
        }
        let rust = rust_type_name(&u.name);
        let _ = writeln!(
            w,
            "#[allow(clippy::unneeded_struct_pattern)]
fn _cases_{}(v: &{rust}) -> &'static str {{
    match v {{",
            snake(&u.name).trim_start_matches('_')
        );
        for t in &tags {
            let _ = writeln!(w, "        {rust}::{t} {{ .. }} => \"{t}\",");
        }
        let _ = writeln!(w, "    }}\n}}\n");
    }
    let op_tags: Vec<String> = idl.ops.iter().map(|o| o.name.clone()).collect();
    let _ = writeln!(
        w,
        "pub(crate) const TREE_OP_TAGS: &[&str] = {};

fn _cases_tree_op(v: &TreeOp) -> &'static str {{
    match v {{",
        str_list(&op_tags)
    );
    for t in &op_tags {
        let _ = writeln!(w, "        TreeOp::{t} {{ .. }} => \"{t}\",");
    }
    let _ = writeln!(w, "    }}\n}}");

    // ── Field pins ──────────────────────────────────────────────────────────
    let _ = writeln!(
        w,
        "\n// ─── Field pins ─────────────────────────────────────────────────────────────
//
// An exhaustive pattern per modelled spec, record and op, naming the IDL's wire
// fields as this host spells them: a field on either side and not the other is
// a build error. Representation choices are declared in idl_codegen.rs's
// FIELD_OVERLAY, never here.\n"
    );
    let pinned_owners = idl
        .kinds
        .iter()
        .filter(|k| !is_unpinned(&k.owner.name))
        .map(|k| (format!("{}Spec", k.owner.name), &k.owner))
        .map(|(spec, o)| {
            let ty = TYPE_NAMES
                .iter()
                .find(|(i, _)| *i == o.name)
                .map_or(spec, |(_, r)| (*r).to_string());
            (ty, o)
        })
        .chain(
            idl.records
                .iter()
                .filter(|r| !is_unpinned(&r.name))
                .map(|r| (rust_type_name(&r.name), r)),
        );
    for (ty, owner) in pinned_owners {
        let fields = rust_fields(&owner.name, &owner.fields);
        let pattern: Vec<String> = fields.iter().map(|f| format!("{f}: _")).collect();
        let _ = writeln!(
            w,
            "fn _fields_{}(v: &{ty}) {{
    let {ty} {{ {} }} = v;
}}
",
            snake(&ty).trim_start_matches('_'),
            pattern.join(", ")
        );
    }
    for op in &idl.ops {
        let fields = rust_fields(&op.name, &op.fields);
        // `Batch` is a tuple variant over its op list; every other op names its
        // fields.
        if op.name == "Batch" {
            continue;
        }
        let pattern: Vec<String> = fields.iter().map(|f| format!("{f}: _")).collect();
        let _ = writeln!(
            w,
            "fn _fields_op_{}(v: &TreeOp) {{
    if let TreeOp::{} {{ {} }} = v {{}}
}}
",
            snake(&op.name).trim_start_matches('_'),
            op.name,
            pattern.join(", ")
        );
    }

    // ── Defaults ────────────────────────────────────────────────────────────
    let _ = writeln!(
        w,
        "// ─── Omit-at-default (§3.6) ─────────────────────────────────────────────────

/// The IDL's omit-at-default values, one typed constant per field whose default
/// is a scalar, an enum case or a field-less union case. The decoder fills an
/// absent field from these and the encoder omits a field equal to them, so a
/// default moved in the IDL moves both boundaries in one regeneration. Defaults
/// that are an empty list or a record stay as the codec's structural checks.
pub(crate) mod defaults {{
    use super::*;"
    );
    let default_owners = idl
        .kinds
        .iter()
        .map(|k| (k.owner.name.clone(), &k.owner))
        .chain(idl.records.iter().map(|r| (r.name.clone(), r)))
        .chain(idl.unions.iter().flat_map(|u| {
            u.cases
                .iter()
                .map(move |c| (format!("{}{}", u.name, c.name), c))
        }));
    for (prefix, owner) in default_owners {
        for f in &owner.fields {
            if f.optionality != "omitDefault" {
                continue;
            }
            let default = f
                .default
                .as_ref()
                .ok_or_else(|| format!("{prefix}.{}: omitDefault with no default", f.name))?;
            if let Some((ty, expr)) = default_expr(&idl, &f.ty, default)? {
                let _ = writeln!(
                    w,
                    "    pub(crate) const {}_{}: {ty} = {expr};",
                    screaming(&prefix),
                    screaming(&f.name)
                );
            }
        }
    }
    let _ = writeln!(w, "}}");

    Ok(out)
}
