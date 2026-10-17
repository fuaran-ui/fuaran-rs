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
//!   the decoder's refusal hints read;
//! - every WIRE TYPE (Phase 2315): each kind's spec struct, each record and
//!   union, `NodeKind`, the node envelope `Node` and `TreeOp`, written by the
//!   uniform mapping rules below — one function per rule: type naming, field
//!   naming, optionality, value types, boxing of recursive slots, derives, and
//!   record and union representation — except where this host's dialect
//!   (`dialect.rs`) declares a departure;
//! - the omit-at-default table as typed constants (`defaults`), which the decoder
//!   fills absent fields from and the encoder omits by;
//! - the node-bearing slot enumeration a walk over a wire tree descends through.
//!
//! **The dialect is checked, so it cannot become a mirror.** It lists only the
//! positions the rules map differently from this host's public types, each with
//! its reason; a field the IDL gains needs no entry. Generation refuses an entry
//! naming a field the IDL does not have, an entry the rules would now reproduce,
//! and a note naming nothing generated — `tests/conformance.rs` provokes all
//! three. Because the types are generated, a kind, case or field on one side and
//! not the other cannot be written, which retired Phase 2298's compile-time pins.
//!
//! **What stays hand-written, deliberately.** The behaviour over the types:
//! decode policy, refusal wording, leniency, relational and limit checks, the
//! encoder, rendering and the C ABI, plus the few host-modelled types the dialect
//! names (`StaticValue`, `I18nArg`, `TransformSource`, `SwitchCondition`,
//! `FragmentArg`). A new field therefore still meets the compiler at every struct
//! literal that builds its owner (the decoder's among them), and a new kind at
//! every exhaustive `match` over `NodeKind`: those sites decide behaviour.
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

// The host's dialect: every place this host's representation departs from the
// uniform rules below, one entry each with its reason, checked against the IDL at
// generation. Compiled with the generator only.
#[path = "dialect.rs"]
mod dialect;

use dialect::{CATEGORIES, DERIVES, FIELDS, HAND_TYPES, INLINED, KIND_PAYLOADS, NOTES, TYPE_NAMES};

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

// ── The uniform mapping rules ──────────────────────────────────────────────
//
// One function per rule. Every Rust type the generator writes is the product of
// these, except where `dialect.rs` declares a departure; the generator refuses a
// departure the rules would now produce anyway.

/// Rule: the Rust type name of a kind's spec — `<Tag>Spec`, unless the dialect
/// renames it.
fn kind_type_name(tag: &str) -> String {
    TYPE_NAMES
        .iter()
        .find(|(i, _)| *i == tag)
        .map_or_else(|| format!("{tag}Spec"), |(_, r)| (*r).to_string())
}

/// Rule: optionality. A required field and a field omitted at its default are the
/// plain type (the default is a generated constant in `defaults`); an optional
/// field is `Option<T>`; a host-only field has no Rust member.
fn optionality_type(optionality: &str, inner: String) -> Result<Option<String>, String> {
    match optionality {
        "required" | "omitDefault" => Ok(Some(inner)),
        "optional" => Ok(Some(format!("Option<{inner}>"))),
        "hostOnly" => Ok(None),
        other => Err(format!("unknown optionality '{other}'")),
    }
}

/// The by-value containment graph over the Rust types the IDL defines: an edge
/// from an owner to every named type one of its wire fields holds directly (not
/// through a list or a map, which allocate).
type Graph = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

/// The named Rust type a position holds by value, if any.
fn named_type(ty: &JVal) -> Option<String> {
    match ty_tag(ty) {
        "enum" | "union" | "record" => match ty.field("name") {
            Some(JVal::Str(n)) => Some(rust_type_name(n)),
            _ => None,
        },
        "node" => Some("Node".into()),
        "kind" => Some("NodeKind".into()),
        "op" => Some("TreeOp".into()),
        "hosted" => ty.field("wire").and_then(named_type),
        _ => None,
    }
}

fn containment(idl: &Idl) -> Graph {
    let mut g = Graph::new();
    let mut add = |owner: &str, fields: &[Field]| {
        for f in fields.iter().filter(|f| f.optionality != "hostOnly") {
            if let Some(n) = named_type(&f.ty) {
                g.entry(owner.to_string()).or_default().insert(n);
            }
        }
    };
    for k in &idl.kinds {
        add(&kind_type_name(&k.owner.name), &k.owner.fields);
    }
    for r in &idl.records {
        add(&rust_type_name(&r.name), &r.fields);
    }
    for u in &idl.unions {
        for c in &u.cases {
            add(&rust_type_name(&u.name), &c.fields);
        }
    }
    for o in &idl.ops {
        add("TreeOp", &o.fields);
    }
    add("Node", &idl.node_fields);
    let specs: Vec<String> = idl
        .kinds
        .iter()
        .filter(|k| !KIND_PAYLOADS.iter().any(|(t, _, _)| *t == k.owner.name))
        .map(|k| kind_type_name(&k.owner.name))
        .collect();
    g.entry("NodeKind".into()).or_default().extend(specs);
    g.entry("Node".into())
        .or_default()
        .insert("NodeKind".into());
    g
}

/// Whether `from` reaches `to` by value. With `through_nodes` false the walk does
/// not pass through `Node`, whose own recursion the node-slot rule breaks.
fn reaches(g: &Graph, from: &str, to: &str, through_nodes: bool) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(at) = stack.pop() {
        if at == to {
            return true;
        }
        if !seen.insert(at.clone()) {
            continue;
        }
        for next in g.get(&at).into_iter().flatten() {
            if through_nodes || next != "Node" || next == to {
                stack.push(next.clone());
            }
        }
    }
    false
}

/// Rule: boxing of recursive slots. A node slot is boxed when its owner is held
/// by value inside a node; any other named slot is boxed when its type holds the
/// owner by value without passing through a node. Every other position is plain.
fn boxed(g: &Graph, target: &str, owner: &str) -> bool {
    if target == "Node" {
        reaches(g, "Node", owner, true)
    } else {
        reaches(g, target, owner, false)
    }
}

/// Rule: the Rust type of a wire position. Scalars map to the host's scalar types;
/// a list is a `Vec`, a map an ordered `Vec` of name-value pairs; a union's type
/// arguments and a type variable are erased; a hosted position is its wire type
/// (`json` is `JVal`); a closure is the presence marker `Closure`. `by_value` is
/// false under a list or a map, where no slot is boxed.
fn value_type(g: &Graph, ty: &JVal, owner: &str, by_value: bool) -> Result<String, String> {
    Ok(match ty_tag(ty) {
        "str" => "String".into(),
        "bool" => "bool".into(),
        "int" => "i64".into(),
        "float" => "f64".into(),
        "json" | "var" => "JVal".into(),
        "fn" => "Closure".into(),
        "list" => format!(
            "Vec<{}>",
            value_type(
                g,
                ty.field("of").ok_or("a list with no 'of'")?,
                owner,
                false
            )?
        ),
        "map" => format!(
            "Vec<(String, {})>",
            value_type(
                g,
                ty.field("values").ok_or("a map with no 'values'")?,
                owner,
                false
            )?
        ),
        "hosted" => match ty.field("wire") {
            Some(JVal::Str(s)) if s == "json" => "JVal".into(),
            Some(wire @ JVal::Obj(_)) => value_type(g, wire, owner, by_value)?,
            _ => return Err("a hosted position with no wire type".into()),
        },
        "enum" | "union" | "record" | "node" | "kind" | "op" => {
            let n = named_type(ty).ok_or("a named position with no name")?;
            if by_value && boxed(g, &n, owner) {
                format!("Box<{n}>")
            } else {
                n
            }
        }
        other => return Err(format!("no mapping rule for a '{other}' position")),
    })
}

/// One Rust member: a named field, or a positional one (`"0"`, `"1"`, …).
#[derive(Debug, Clone, PartialEq)]
struct Member {
    ident: String,
    ty: String,
}

/// Rule: one IDL field as Rust members under the uniform rules — none for a
/// host-only field, otherwise one, named by [`field_ident`].
fn uniform_members(g: &Graph, owner: &str, f: &Field) -> Result<Vec<Member>, String> {
    if f.optionality == "hostOnly" {
        return Ok(Vec::new());
    }
    let inner = value_type(g, &f.ty, owner, true)?;
    Ok(optionality_type(&f.optionality, inner)?
        .map(|ty| Member {
            ident: field_ident(&f.name),
            ty,
        })
        .into_iter()
        .collect())
}

type Used = std::collections::BTreeSet<(String, String)>;
type Noted = std::collections::BTreeSet<String>;

/// The Rust members of an owner: the uniform rules, except where the dialect
/// declares a departure. An entry that the rules now reproduce is refused, and
/// every entry used is recorded so an entry naming nothing can be refused after.
fn members(
    g: &Graph,
    key: &str,
    rust_owner: &str,
    fields: &[Field],
    used: &mut Used,
) -> Result<Vec<Member>, String> {
    let mut out: Vec<Member> = Vec::new();
    for f in fields {
        let uniform = uniform_members(g, rust_owner, f)?;
        let rep = match FIELDS.iter().find(|(o, n, _, _)| *o == key && *n == f.name) {
            Some((_, _, rust, _)) => {
                used.insert((key.to_string(), f.name.clone()));
                let declared: Vec<Member> = rust
                    .iter()
                    .map(|(i, t)| Member {
                        ident: (*i).to_string(),
                        ty: (*t).to_string(),
                    })
                    .collect();
                if declared == uniform {
                    return Err(format!(
                        "dialect entry {key}.{} is stale: the uniform rules now map it the \
                         same; delete the entry",
                        f.name
                    ));
                }
                declared
            }
            None => uniform,
        };
        for m in rep {
            match out.iter().find(|o| o.ident == m.ident) {
                Some(o) if o.ty == m.ty => {}
                Some(o) => {
                    return Err(format!(
                        "{key}: member {} declared as both {} and {}",
                        m.ident, o.ty, m.ty
                    ));
                }
                None => out.push(m),
            }
        }
    }
    Ok(out)
}

/// The doc comment for a generated item, from the dialect's notes.
fn write_doc(w: &mut String, indent: &str, key: &str, noted: &mut Noted) {
    if let Some((_, text)) = NOTES.iter().find(|(k, _)| *k == key) {
        noted.insert(key.to_string());
        for line in text.lines() {
            if line.is_empty() {
                let _ = writeln!(w, "{indent}///");
            } else {
                let _ = writeln!(w, "{indent}/// {line}");
            }
        }
    }
}

/// Rule: derives. Every generated type derives `Debug, Clone, PartialEq`; the
/// dialect adds to that per type.
fn write_derives(w: &mut String, ty: &str) {
    let extra: &[&str] = DERIVES
        .iter()
        .find(|(t, _, _)| *t == ty)
        .map_or(&[], |(_, d, _)| d);
    let all: Vec<&str> = ["Debug", "Clone", "Copy", "PartialEq", "Eq", "Default"]
        .into_iter()
        .filter(|d| matches!(*d, "Debug" | "Clone" | "PartialEq") || extra.contains(d))
        .collect();
    let _ = writeln!(w, "#[derive({})]", all.join(", "));
}

fn key_ident(ident: &str) -> &str {
    ident.trim_start_matches("r#")
}

/// Rule: record representation — a struct with one public field per member.
fn write_struct(w: &mut String, ty: &str, ms: &[Member], noted: &mut Noted) {
    write_doc(w, "", ty, noted);
    write_derives(w, ty);
    let _ = writeln!(w, "pub struct {ty} {{");
    for m in ms {
        write_doc(w, "    ", &format!("{ty}.{}", key_ident(&m.ident)), noted);
        let _ = writeln!(w, "    pub {}: {},", m.ident, m.ty);
    }
    let _ = writeln!(w, "}}\n");
}

/// Rule: union representation — an enum with one variant per case: a unit
/// variant for a case with no members, a tuple variant when every member is
/// positional, otherwise a struct variant.
fn write_enum(
    w: &mut String,
    ty: &str,
    cases: &[(String, Vec<Member>)],
    noted: &mut Noted,
) -> Result<(), String> {
    write_doc(w, "", ty, noted);
    write_derives(w, ty);
    let _ = writeln!(w, "pub enum {ty} {{");
    for (case, ms) in cases {
        write_doc(w, "    ", &format!("{ty}::{case}"), noted);
        let positional = ms
            .iter()
            .filter(|m| m.ident.chars().all(|c| c.is_ascii_digit()))
            .count();
        if ms.is_empty() {
            let _ = writeln!(w, "    {case},");
        } else if positional == ms.len() {
            let tys: Vec<&str> = ms.iter().map(|m| m.ty.as_str()).collect();
            let _ = writeln!(w, "    {case}({}),", tys.join(", "));
        } else if positional == 0 {
            let _ = writeln!(w, "    {case} {{");
            for m in ms {
                let key = format!("{ty}::{case}.{}", key_ident(&m.ident));
                write_doc(w, "        ", &key, noted);
                let _ = writeln!(w, "        {}: {},", m.ident, m.ty);
            }
            let _ = writeln!(w, "    }},");
        } else {
            return Err(format!("{ty}::{case} mixes positional and named members"));
        }
    }
    let _ = writeln!(w, "}}\n");
    Ok(())
}

/// The wire types: every kind's spec, every record and union, `NodeKind`, the
/// node envelope and `TreeOp`, through the rules and the dialect, with the
/// dialect checked against the IDL as it is read.
fn write_types(w: &mut String, idl: &Idl) -> Result<(), String> {
    check_dialect_names(idl)?;
    let g = containment(idl);
    let mut used = Used::new();
    let mut noted = Noted::new();
    let mut emitted: Vec<String> = Vec::new();
    let _ = writeln!(
        w,
        "\n// ─── Wire types ─────────────────────────────────────────────────────────────
//
// Every type the IDL defines, through the uniform rules in idl_codegen.rs and the
// departures declared in dialect.rs. Behaviour impls stay in model.rs.\n"
    );
    for k in &idl.kinds {
        if KIND_PAYLOADS.iter().any(|(t, _, _)| *t == k.owner.name) {
            continue;
        }
        let ty = kind_type_name(&k.owner.name);
        let ms = members(&g, &k.owner.name, &ty, &k.owner.fields, &mut used)?;
        write_struct(w, &ty, &ms, &mut noted);
        emitted.push(ty);
    }
    let hand =
        |n: &str| INLINED.iter().any(|(i, _)| *i == n) || HAND_TYPES.iter().any(|(h, _)| *h == n);
    for r in idl.records.iter().filter(|r| !hand(&r.name)) {
        let ty = rust_type_name(&r.name);
        let ms = members(&g, &r.name, &ty, &r.fields, &mut used)?;
        write_struct(w, &ty, &ms, &mut noted);
        emitted.push(ty);
    }
    for u in idl.unions.iter().filter(|u| !hand(&u.name)) {
        let ty = rust_type_name(&u.name);
        let mut cases = Vec::new();
        for c in &u.cases {
            let key = format!("{}.{}", u.name, c.name);
            let ms = members(&g, &key, &ty, &c.fields, &mut used)?;
            cases.push((c.name.clone(), ms));
        }
        write_enum(w, &ty, &cases, &mut noted)?;
        emitted.push(ty);
    }
    let kinds: Vec<(String, Vec<Member>)> = idl
        .kinds
        .iter()
        .map(|k| {
            let payload = KIND_PAYLOADS
                .iter()
                .find(|(t, _, _)| *t == k.owner.name)
                .map_or_else(
                    || kind_type_name(&k.owner.name),
                    |(_, p, _)| (*p).to_string(),
                );
            let member = Member {
                ident: "0".into(),
                ty: payload,
            };
            (k.owner.name.clone(), vec![member])
        })
        .collect();
    write_enum(w, "NodeKind", &kinds, &mut noted)?;
    // The node envelope (§3.1): its identity and its kind, then the IDL's node
    // fields.
    let mut envelope = vec![
        Member {
            ident: "id".into(),
            ty: "NodeId".into(),
        },
        Member {
            ident: "kind".into(),
            ty: "NodeKind".into(),
        },
    ];
    envelope.extend(members(&g, "Node", "Node", &idl.node_fields, &mut used)?);
    write_struct(w, "Node", &envelope, &mut noted);
    let mut ops = Vec::new();
    for o in &idl.ops {
        let key = format!("TreeOp.{}", o.name);
        let ms = members(&g, &key, "TreeOp", &o.fields, &mut used)?;
        ops.push((o.name.clone(), ms));
    }
    write_enum(w, "TreeOp", &ops, &mut noted)?;
    emitted.extend(["NodeKind", "Node", "TreeOp"].map(String::from));

    for (owner, field, _, _) in FIELDS {
        if !used.contains(&((*owner).to_string(), (*field).to_string())) {
            return Err(format!(
                "dialect entry {owner}.{field} names a field the IDL does not have; delete it"
            ));
        }
    }
    for (key, _) in NOTES {
        if !noted.contains(*key) {
            return Err(format!(
                "dialect note '{key}' names nothing the generator emits"
            ));
        }
    }
    for (ty, _, _) in DERIVES {
        if !emitted.iter().any(|e| e == ty) {
            return Err(format!(
                "dialect derive entry '{ty}' names no generated type"
            ));
        }
    }
    Ok(())
}

/// The dialect's type-level entries each name something the IDL defines.
fn check_dialect_names(idl: &Idl) -> Result<(), String> {
    let kind = |n: &str| idl.kinds.iter().any(|k| k.owner.name == n);
    let record = |n: &str| idl.records.iter().any(|r| r.name == n);
    let union = |n: &str| idl.unions.iter().any(|u| u.name == n);
    for (n, _) in TYPE_NAMES {
        if !(kind(n) || record(n) || union(n)) {
            return Err(format!(
                "dialect type name '{n}' names no IDL kind, record or union"
            ));
        }
    }
    for (n, _) in INLINED {
        if !record(n) {
            return Err(format!("dialect inlined record '{n}' names no IDL record"));
        }
    }
    for (n, _) in HAND_TYPES {
        if !(record(n) || union(n)) {
            return Err(format!(
                "dialect hand type '{n}' names no IDL record or union"
            ));
        }
    }
    for (n, _, _) in KIND_PAYLOADS {
        if !kind(n) {
            return Err(format!("dialect kind payload '{n}' names no IDL kind"));
        }
    }
    Ok(())
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

/// The type a list element or a bare position holds: `list<T>` is `T`.
fn element(ty: &JVal) -> &JVal {
    if ty_tag(ty) == "list" {
        ty.field("of").unwrap_or(ty)
    } else {
        ty
    }
}

fn is_node_slot(ty: &JVal) -> bool {
    ty_tag(element(ty)) == "node"
}

/// The records that carry a node somewhere inside them, transitively: a record
/// with a node slot, or a record slot holding one of these.
fn node_bearing_records(idl: &Idl) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    loop {
        let before = out.len();
        for r in &idl.records {
            let bears = r.fields.iter().any(|f| {
                f.optionality != "hostOnly" && (is_node_slot(&f.ty) || is_record_slot(&f.ty, &out))
            });
            if bears {
                out.insert(r.name.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

fn is_record_slot(ty: &JVal, bearing: &std::collections::BTreeSet<String>) -> bool {
    let el = element(ty);
    ty_tag(el) == "record" && matches!(el.field("name"), Some(JVal::Str(n)) if bearing.contains(n))
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
// Everything structural about the wire vocabulary lives here: the IDL through the
// uniform rules in idl_codegen.rs and the departures declared in dialect.rs.
// Behaviour stays in model.rs, decode.rs and encode.rs.

use super::model::*;
use crate::core::bare_enum;
@@IMPORTS@@
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
    }
    let op_tags: Vec<String> = idl.ops.iter().map(|o| o.name.clone()).collect();
    let _ = writeln!(
        w,
        "pub(crate) const TREE_OP_TAGS: &[&str] = {};",
        str_list(&op_tags)
    );

    write_types(w, &idl)?;

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

    // ── Slots ───────────────────────────────────────────────────────────────
    let bearing = node_bearing_records(&idl);
    let mut node_slots: Vec<String> = Vec::new();
    let mut record_slots: Vec<String> = Vec::new();
    let slot_owners = idl
        .kinds
        .iter()
        .map(|k| &k.owner)
        .chain(idl.records.iter())
        .chain(idl.unions.iter().flat_map(|u| u.cases.iter()))
        .flat_map(|o| o.fields.iter())
        .chain(idl.node_fields.iter());
    for f in slot_owners.filter(|f| f.optionality != "hostOnly") {
        let list = if is_node_slot(&f.ty) {
            &mut node_slots
        } else if is_record_slot(&f.ty, &bearing) {
            &mut record_slots
        } else {
            continue;
        };
        if !list.contains(&f.name) {
            list.push(f.name.clone());
        }
    }
    node_slots.sort();
    record_slots.sort();
    let _ = writeln!(
        w,
        "
// ─── Node-bearing slots ─────────────────────────────────────────────────────

/// Every wire field name, on the node envelope, a kind, a record or a union
/// case, whose value is a node or a list of nodes.
pub const NODE_SLOTS: &[&str] = {};

/// Every wire field name whose value is a record, or a list of records, that
/// carries a node somewhere inside it. With [`NODE_SLOTS`] these are the
/// positions a walk over a wire tree descends through.
pub const NODE_RECORD_SLOTS: &[&str] = {};",
        str_list(&node_slots),
        str_list(&record_slots)
    );
    // The imports the wire types need, and only those: a dialect entry that
    // stops naming a map type must not leave an unused import behind.
    let mut imports = String::from(
        "use crate::canonical::JVal;
",
    );
    if out.contains("BTreeMap<") {
        imports.push_str(
            "use std::collections::BTreeMap;
",
        );
    }
    Ok(out.replacen("@@IMPORTS@@", &imports, 1))
}
