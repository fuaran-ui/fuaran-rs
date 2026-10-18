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
//! - the UNIFORM half of the codec (Phase 2333): one encoder and one decoder per
//!   owner, and the discriminator dispatch, written from the IDL type, the
//!   optionality and the generated members (`gen_encode` / `gen_decode`).
//!
//! **What stays hand-written, deliberately.** The behaviour over the types:
//! decode policy, refusal wording, leniency, relational and limit checks,
//! rendering and the C ABI, plus the few host-modelled types the dialect names
//! (`StaticValue`, `I18nArg`, `TransformSource`, `SwitchCondition`,
//! `FragmentArg`). Codec policy is a hook the dialect declares, naming the hand
//! function that handles it, and the generator refuses a hook naming nothing and
//! a `Shape` hook the rules now cover. A plain field the IDL gains is therefore
//! coded with no hand edit; it meets the compiler only where behaviour builds its
//! owner (the renderer's projection), and a new kind only at the exhaustive
//! `match`es over `NodeKind` that decide what a kind DOES.
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

use dialect::{
    CATEGORIES, Check, DECODE_ALIASES, DECODE_CHECKS, DECODE_ENTER, DECODE_HOOKS, DECODE_OWNERS,
    DERIVES, ENCODE_HOOKS, ENCODE_OWNERS, ENUM_ALIASES, FIELDS, HAND_TYPES, Hook, INLINED,
    KIND_PAYLOADS, NOTES, TYPE_NAMES, UNION_PRE,
};

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
    /// `(union, case)`: a case whose bare value is its canonical spelling.
    transparent: Vec<(String, String)>,
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
    let mut transparent = Vec::new();
    if let Some(JVal::Arr(items)) = j.field("harden").and_then(|h| h.field("transparentUnions")) {
        for t in items {
            transparent.push((
                str_of(t, "union", "transparentUnions[]")?.to_string(),
                str_of(t, "case", "transparentUnions[]")?.to_string(),
            ));
        }
    }
    Ok(Idl {
        version,
        kinds,
        records,
        unions,
        enums,
        ops,
        node_fields,
        transparent,
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

    write_codec(w, &idl)?;

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

// ── The generated codec (Phase 2333) ───────────────────────────────────────
//
// The UNIFORM half of the codec: one encoder and one decoder per owner (a kind's
// spec, a record, a union case, a tree op, the node envelope), written from the
// IDL type, the optionality and the generated Rust members. Every position whose
// handling is policy is a hook the dialect declares, naming the hand function in
// `encode.rs` / `decode.rs` that handles it. A hook receives every Rust member its
// field (or owner) carries, so a field the IDL gains is either written by the
// generated encoder or changes a hook's arity — never silently absent.

/// One owner the codec covers.
struct CodecOwner<'a> {
    /// The dialect key: a kind's tag, a record's name, `Union.Case`, `TreeOp.Tag`
    /// or `Node`.
    key: String,
    /// The `defaults` constant prefix (empty where the table carries none).
    prefix: String,
    /// The path the members are bound under: `BadgeSpec` or `Action::Navigate`.
    path: String,
    /// The IDL fields, host-only ones included (they carry no member).
    fields: Vec<&'a Field>,
    /// Each field's Rust members, parallel to `fields`.
    per_field: Vec<Vec<Member>>,
    /// Every member, in declaration order.
    members: Vec<Member>,
}

impl CodecOwner<'_> {
    fn positional(&self) -> bool {
        !self.members.is_empty()
            && self
                .members
                .iter()
                .all(|m| m.ident.chars().all(|c| c.is_ascii_digit()))
    }
}

/// The name a member is bound to in a generated pattern.
/// Prefixed, so a member never shadows a codec function of the same name (a
/// `field` member beside the `field` helper, a `view_box` beside its encoder).
fn bind(m: &Member) -> String {
    format!("m_{}", key_ident(&m.ident))
}

/// The pattern binding every member of an owner (also the expression building
/// it from the same names).
fn pattern(o: &CodecOwner) -> String {
    if o.members.is_empty() {
        // A unit case is its path; a struct with no members is `Path {}`.
        if o.path.contains("::") {
            o.path.clone()
        } else {
            format!("{} {{}}", o.path)
        }
    } else if o.positional() {
        let binds: Vec<String> = o.members.iter().map(bind).collect();
        format!("{}({})", o.path, binds.join(", "))
    } else {
        let binds: Vec<String> = o
            .members
            .iter()
            .map(|m| format!("{}: {}", m.ident, bind(m)))
            .collect();
        format!("{} {{ {} }}", o.path, binds.join(", "))
    }
}

/// A Rust function name for a type: `TextSource` → `text_source`.
fn fn_name(rust_ty: &str) -> String {
    snake(rust_ty).trim_start_matches('_').to_string()
}

fn strip<'a>(prefix: &str, ty: &'a str) -> Option<&'a str> {
    ty.strip_prefix(prefix).and_then(|t| t.strip_suffix('>'))
}

/// The IDL type a hosted position carries on the wire.
fn wire_type(ty: &JVal) -> &JVal {
    if ty_tag(ty) == "hosted"
        && let Some(wire @ JVal::Obj(_)) = ty.field("wire")
    {
        return wire;
    }
    ty
}

fn is_json(ty: &JVal) -> bool {
    match ty_tag(ty) {
        "json" => true,
        "hosted" => matches!(ty.field("wire"), Some(JVal::Str(s)) if s == "json"),
        _ => false,
    }
}

/// Rule: the `Static` payload slot a `Binding<T>` position decodes through.
fn static_slot(arg: &JVal) -> Option<&'static str> {
    if is_json(arg) {
        return Some("Untyped");
    }
    Some(match ty_tag(arg) {
        "str" => "Str",
        "bool" => "Bool",
        "float" => "Float",
        "int" => "Int",
        "list" => {
            let of = arg.field("of")?;
            match (ty_tag(of), of.field("name")) {
                ("str", _) => "StringList",
                ("float", _) => "FloatSeq",
                ("record", Some(JVal::Str(n))) if n == "SelectOption" => "Options",
                ("record", Some(JVal::Str(n))) if n == "MapMarker" => "Markers",
                _ => return None,
            }
        }
        _ => return None,
    })
}

enum Default {
    Const(String),
    EmptyList,
}

struct Codec<'a> {
    enums: std::collections::BTreeSet<String>,
    idl: &'a Idl,
}

impl Codec<'_> {
    /// Whether a non-container Rust type is the uniform rendering of an IDL
    /// position: a scalar of its scalar, the named type its name maps to, or a
    /// host-modelled type at a hosted or type-variable position. Anything else is
    /// a departure, which only a hook may code.
    fn agrees(idl_ty: &JVal, rust_ty: &str) -> bool {
        let tag = ty_tag(idl_ty);
        match rust_ty {
            "String" | "NodeId" => tag == "str",
            "bool" => tag == "bool",
            "i64" => tag == "int",
            "f64" => tag == "float",
            "Closure" => tag == "fn",
            "Node" => tag == "node",
            "NodeKind" => tag == "kind",
            "TreeOp" => tag == "op",
            t => match tag {
                "hosted" | "var" => true,
                "enum" | "union" | "record" => {
                    matches!(idl_ty.field("name"), Some(JVal::Str(n)) if rust_type_name(n) == t)
                }
                _ => false,
            },
        }
    }

    /// Rule: the encoder of one value. `x` is an expression of type `&rust_ty`.
    fn enc_value(&self, idl_ty: &JVal, rust_ty: &str, x: &str) -> Option<String> {
        let idl_ty = wire_type(idl_ty);
        if let Some(inner) = strip("Box<", rust_ty) {
            return self.enc_value(idl_ty, inner, x);
        }
        if let Some(inner) = strip("Vec<(String, ", rust_ty).and_then(|t| t.strip_suffix(')')) {
            if ty_tag(idl_ty) != "map" {
                return None;
            }
            let v = self.enc_value(idl_ty.field("values")?, inner, "v")?;
            return Some(format!("enc_map({x}, {})", closure(&v)));
        }
        if let Some(inner) = strip("Vec<", rust_ty) {
            if ty_tag(idl_ty) != "list" {
                return None;
            }
            let v = self.enc_value(idl_ty.field("of")?, inner, "v")?;
            return Some(format!("enc_list({x}, {})", closure(&v)));
        }
        if !Self::agrees(idl_ty, rust_ty) && !(rust_ty == "JVal" && is_json(idl_ty)) {
            return None;
        }
        Some(match rust_ty {
            "String" | "NodeId" => format!("s({x})"),
            "bool" => format!("boolean(*{x})"),
            "i64" => format!("int(*{x})"),
            "f64" => format!("num(*{x})"),
            "Closure" => "CLOSURE.to_string()".into(),
            "JVal" if is_json(idl_ty) => format!("json_value({x})"),
            _ if rust_ty.contains('<') || rust_ty == "JVal" => return None,
            t if self.enums.contains(t) => format!("s({x}.as_str())"),
            t => format!("{}({x})", fn_name(t)),
        })
    }

    /// Rule: the decoder of one value, as an expression callable as
    /// `(path: &str, j: &JVal) -> DResult<rust_ty>`.
    fn dec_value(&self, idl_ty: &JVal, rust_ty: &str) -> Option<String> {
        let idl_ty = wire_type(idl_ty);
        if let Some(inner) = strip("Box<", rust_ty) {
            let d = self.dec_value(idl_ty, inner)?;
            return Some(format!("|p, j| ({d})(p, j).map(Box::new)"));
        }
        if let Some(inner) = strip("Vec<(String, ", rust_ty).and_then(|t| t.strip_suffix(')')) {
            if ty_tag(idl_ty) != "map" {
                return None;
            }
            let d = self.dec_value(idl_ty.field("values")?, inner)?;
            return Some(format!("|p, j| dec_map(p, j, {d})"));
        }
        if let Some(inner) = strip("Vec<", rust_ty) {
            if ty_tag(idl_ty) != "list" {
                return None;
            }
            let d = self.dec_value(idl_ty.field("of")?, inner)?;
            return Some(format!("|p, j| dec_list(p, j, {d})"));
        }
        if !Self::agrees(idl_ty, rust_ty) && !(rust_ty == "JVal" && is_json(idl_ty)) {
            return None;
        }
        Some(match rust_ty {
            "String" | "NodeId" => "dec_string".into(),
            "bool" => "as_bool".into(),
            "i64" => "as_int".into(),
            "f64" => "as_float".into(),
            "Closure" => "dec_closure".into(),
            "JVal" if is_json(idl_ty) => "decode_jval".into(),
            "Binding" if ty_tag(idl_ty) == "union" => match idl_ty.field("args") {
                Some(JVal::Arr(args)) if !args.is_empty() => match static_slot(&args[0])? {
                    "Untyped" => "decode_binding".into(),
                    slot => format!("|p, j| decode_binding_slot(p, j, StaticSlot::{slot})"),
                },
                _ => "decode_binding".into(),
            },
            "Node" => "decode_node_ast".into(),
            "TreeOp" => "decode_tree_op_ast".into(),
            _ if rust_ty.contains('<') || rust_ty == "JVal" => return None,
            t => format!("decode_{}", fn_name(t)),
        })
    }

    /// The `defaults` constant an omit-at-default field compares against, or the
    /// emptiness test of a list whose default is empty.
    fn default_of(&self, o: &CodecOwner, f: &Field) -> Option<Default> {
        let d = f.default.as_ref()?;
        if ty_tag(d) == "list" && matches!(d.field("items"), Some(JVal::Arr(i)) if i.is_empty()) {
            return Some(Default::EmptyList);
        }
        if o.prefix.is_empty() {
            return None;
        }
        match default_expr(self.idl, &f.ty, d) {
            Ok(Some(_)) => Some(Default::Const(format!(
                "defaults::{}_{}",
                screaming(&o.prefix),
                screaming(&f.name)
            ))),
            _ => None,
        }
    }

    /// Rule: the uniform encoder statement for one field, or `None` where the
    /// rules have no codec for it.
    fn enc_field(&self, o: &CodecOwner, i: usize) -> Option<String> {
        let f = o.fields[i];
        let ms = &o.per_field[i];
        if ms.is_empty() && ty_tag(&f.ty) == "fn" {
            // Rule: a closure slot this host carries no member for is always
            // written as its sentinel (the closure itself never rides the wire).
            return Some(format!(
                "f.push(field(\"{}\", CLOSURE.to_string()));",
                f.name
            ));
        }
        if ms.len() != 1 {
            return None;
        }
        let m = &ms[0];
        let b = bind(m);
        let key = &f.name;
        match (f.optionality.as_str(), strip("Option<", &m.ty)) {
            ("required", None) => {
                let e = self.enc_value(&f.ty, &m.ty, &b)?;
                Some(format!("f.push(field(\"{key}\", {e}));"))
            }
            ("optional", Some("Closure")) if ty_tag(&f.ty) == "fn" => Some(format!(
                "if {b}.is_some() {{
        f.push(field(\"{key}\", CLOSURE.to_string()));
    }}"
            )),
            ("optional", Some(inner)) => {
                let e = self.enc_value(&f.ty, inner, "x")?;
                Some(format!(
                    "if let Some(x) = {b} {{\n        f.push(field(\"{key}\", {e}));\n    }}"
                ))
            }
            ("omitDefault", None) => {
                let e = self.enc_value(&f.ty, &m.ty, &b)?;
                let test = match self.default_of(o, f)? {
                    Default::Const(c) => format!("*{b} != {c}"),
                    Default::EmptyList => format!("!{b}.is_empty()"),
                };
                Some(format!(
                    "if {test} {{\n        f.push(field(\"{key}\", {e}));\n    }}"
                ))
            }
            _ => None,
        }
    }

    /// Rule: the uniform decoder statement for one field, or `None` where the
    /// rules have no codec for it.
    fn dec_field(&self, o: &CodecOwner, i: usize) -> Option<String> {
        let f = o.fields[i];
        let ms = &o.per_field[i];
        if ms.is_empty() && ty_tag(&f.ty) == "fn" {
            // Rule: nothing reads a closure slot this host carries no member for.
            return Some(String::new());
        }
        if ms.len() != 1 {
            return None;
        }
        let m = &ms[0];
        let b = bind(m);
        let key = &f.name;
        let aliases = DECODE_ALIASES
            .iter()
            .find(|(ow, fd, _, _)| *ow == o.key && *fd == f.name)
            .map_or_else(String::new, |(_, _, a, _)| {
                a.iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        match (f.optionality.as_str(), strip("Option<", &m.ty)) {
            ("required", None) => {
                let d = self.dec_value(&f.ty, &m.ty)?;
                Some(format!(
                    "let {b} = field_req(path, fields, \"{key}\", &[{aliases}], \"{}.{key}: {}\", {d})?;",
                    o.key,
                    type_text(&f.ty)
                ))
            }
            ("optional", Some(inner)) => {
                let d = self.dec_value(&f.ty, inner)?;
                Some(format!(
                    "let {b} = field_opt(path, fields, \"{key}\", &[{aliases}], {d})?;"
                ))
            }
            ("omitDefault", None) => {
                let d = self.dec_value(&f.ty, &m.ty)?;
                let default = match self.default_of(o, f)? {
                    Default::Const(c) => c,
                    Default::EmptyList => "Vec::new()".into(),
                };
                Some(format!(
                    "let {b} = field_or(path, fields, \"{key}\", &[{aliases}], {default}, {d})?;"
                ))
            }
            _ => None,
        }
    }
}

/// The element encoder `|v| f(v)` as the function `f` itself when it is one.
fn closure(body: &str) -> String {
    match body.strip_suffix("(v)") {
        Some(f)
            if f != "s"
                && f.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') =>
        {
            f.to_string()
        }
        _ => format!("|v| {body}"),
    }
}

/// A short description of an IDL type, for a decoder's `MISSING_FIELD` hint.
fn type_text(ty: &JVal) -> String {
    let ty = wire_type(ty);
    match ty_tag(ty) {
        "str" => "string".into(),
        "bool" => "boolean".into(),
        "int" => "integer".into(),
        "float" => "number".into(),
        "json" | "hosted" => "JSON value".into(),
        "fn" => "closure sentinel".into(),
        "node" => "Node".into(),
        "kind" => "NodeKind".into(),
        "op" => "TreeOp".into(),
        "list" => format!(
            "list of {}",
            ty.field("of").map_or_else(String::new, type_text)
        ),
        "map" => format!(
            "map of {}",
            ty.field("values").map_or_else(String::new, type_text)
        ),
        _ => match ty.field("name") {
            Some(JVal::Str(n)) => match ty.field("args") {
                Some(JVal::Arr(args)) if !args.is_empty() => format!(
                    "{n}<{}>",
                    args.iter().map(type_text).collect::<Vec<_>>().join(", ")
                ),
                _ => n.clone(),
            },
            _ => "value".into(),
        },
    }
}

/// The Rust members one IDL field maps to, under the rules or the dialect.
fn field_members(g: &Graph, key: &str, rust_owner: &str, f: &Field) -> Result<Vec<Member>, String> {
    match FIELDS.iter().find(|(o, n, _, _)| *o == key && *n == f.name) {
        Some((_, _, rust, _)) => Ok(rust
            .iter()
            .map(|(i, t)| Member {
                ident: (*i).to_string(),
                ty: (*t).to_string(),
            })
            .collect()),
        None => uniform_members(g, rust_owner, f),
    }
}

fn codec_owner<'a>(
    g: &Graph,
    key: String,
    prefix: String,
    path: String,
    rust_owner: &str,
    fields: Vec<&'a Field>,
) -> Result<CodecOwner<'a>, String> {
    let mut per_field = Vec::new();
    let mut members: Vec<Member> = Vec::new();
    for f in &fields {
        let ms = field_members(g, &key, rust_owner, f)?;
        for m in &ms {
            if !members.iter().any(|x| x.ident == m.ident) {
                members.push(m.clone());
            }
        }
        per_field.push(ms);
    }
    Ok(CodecOwner {
        key,
        prefix,
        path,
        fields,
        per_field,
        members,
    })
}

type HookUse = std::collections::BTreeSet<(String, String)>;
type OwnerUse = std::collections::BTreeSet<String>;
type FieldHook = (&'static str, &'static str, &'static str, Hook, &'static str);
type OwnerHook = (&'static str, &'static str, Hook, &'static str);

fn field_hook<'a>(
    table: &'a [FieldHook],
    o: &CodecOwner,
    f: &Field,
    used: &mut HookUse,
) -> Option<&'a FieldHook> {
    let hit = table.iter().find(|h| h.0 == o.key && h.1 == f.name);
    if hit.is_some() {
        used.insert((o.key.clone(), f.name.clone()));
    }
    hit
}

fn owner_hook<'a>(table: &'a [OwnerHook], key: &str, used: &mut OwnerUse) -> Option<&'a OwnerHook> {
    let hit = table.iter().find(|h| h.0 == key);
    if hit.is_some() {
        used.insert(key.to_string());
    }
    hit
}

/// The argument list passing a field's (or an owner's) members to a hook.
fn args(ms: &[Member]) -> String {
    ms.iter().map(bind).collect::<Vec<_>>().join(", ")
}

fn tys(ms: &[Member]) -> String {
    ms.iter()
        .map(|m| m.ty.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn case_tag(o: &CodecOwner) -> String {
    o.path.rsplit("::").next().unwrap_or_default().to_string()
}

struct Emit {
    /// Every position no rule and no hook covers, reported together.
    missing: Vec<String>,
    enc: String,
    dec: String,
    enc_used: HookUse,
    dec_used: HookUse,
    enc_owners: OwnerUse,
    dec_owners: OwnerUse,
}

impl Emit {
    /// The encoder body of an owner: its owner hook, or one statement per field
    /// then `wrap` over the collected fields.
    fn enc_body(&mut self, c: &Codec, o: &CodecOwner, wrap: &str) -> Result<String, String> {
        if let Some((_, hook, kind, _)) = owner_hook(ENCODE_OWNERS, &o.key, &mut self.enc_owners) {
            if *kind == Hook::Shape && (0..o.fields.len()).all(|i| c.enc_field(o, i).is_some()) {
                return Err(format!(
                    "dialect encode owner hook {} is stale: the uniform rules now encode every \
                     field; delete the entry",
                    o.key
                ));
            }
            return Ok(format!("{hook}({})", args(&o.members)));
        }
        let mut stmts = Vec::new();
        for (i, fd) in o.fields.iter().enumerate() {
            if fd.optionality == "hostOnly" {
                continue;
            }
            let uniform = c.enc_field(o, i);
            let stmt = match field_hook(ENCODE_HOOKS, o, fd, &mut self.enc_used) {
                Some((_, _, hook, kind, _)) => {
                    if *kind == Hook::Shape && uniform.is_some() {
                        return Err(format!(
                            "dialect encode hook {}.{} is stale: the uniform rules now encode it; \
                             delete the entry",
                            o.key, fd.name
                        ));
                    }
                    format!(
                        "if let Some(j) = {hook}({}) {{\n        f.push(field(\"{}\", j));\n    }}",
                        args(&o.per_field[i]),
                        fd.name
                    )
                }
                None => uniform.unwrap_or_else(|| {
                    self.missing.push(format!(
                        "encode {}.{} ({})",
                        o.key,
                        fd.name,
                        tys(&o.per_field[i])
                    ));
                    String::new()
                }),
            };
            stmts.push(stmt);
        }
        // The fields every value writes open the list; the conditional ones push.
        let lead = stmts
            .iter()
            .take_while(|s| s.starts_with("f.push(") && !s.contains('\n'))
            .count();
        let opening: Vec<&str> = stmts[..lead]
            .iter()
            .map(|s| &s["f.push(".len()..s.len() - ");".len()])
            .collect();
        let list = format!("vec![{}]", opening.join(", "));
        if lead == stmts.len() {
            return Ok(wrap.replace("f)", &format!("{list})")));
        }
        let mut body = format!("let mut f: Vec<Field> = {list};\n");
        for s in &stmts[lead..] {
            let _ = writeln!(body, "    {s}");
        }
        let _ = write!(body, "    {wrap}");
        Ok(body)
    }

    /// The decoder statements of an owner's fields, ending in its construction.
    fn dec_body(&mut self, c: &Codec, o: &CodecOwner) -> Result<String, String> {
        let mut body = String::new();
        if let Some((_, hook, _, _)) = DECODE_CHECKS
            .iter()
            .find(|(k, _, when, _)| *k == o.key && *when == Check::Pre)
        {
            let _ = writeln!(body, "    {hook}(path, fields)?;");
        }
        let mut bound: Vec<String> = Vec::new();
        for (i, fd) in o.fields.iter().enumerate() {
            if fd.optionality == "hostOnly" {
                continue;
            }
            let fresh: Vec<Member> = o.per_field[i]
                .iter()
                .filter(|m| !bound.contains(&m.ident))
                .cloned()
                .collect();
            let folded = fresh.len() != o.per_field[i].len();
            let uniform = if folded { None } else { c.dec_field(o, i) };
            let stmt = match field_hook(DECODE_HOOKS, o, fd, &mut self.dec_used) {
                Some((_, _, hook, kind, _)) => {
                    if *kind == Hook::Shape && uniform.is_some() {
                        return Err(format!(
                            "dialect decode hook {}.{} is stale: the uniform rules now decode it; \
                             delete the entry",
                            o.key, fd.name
                        ));
                    }
                    match fresh.len() {
                        0 => format!("{hook}(path, fields)?;"),
                        1 => format!("let {} = {hook}(path, fields)?;", bind(&fresh[0])),
                        _ => format!("let ({}) = {hook}(path, fields)?;", args(&fresh)),
                    }
                }
                None => uniform.unwrap_or_else(|| {
                    self.missing.push(format!(
                        "decode {}.{} ({})",
                        o.key,
                        fd.name,
                        tys(&o.per_field[i])
                    ));
                    String::new()
                }),
            };
            bound.extend(fresh.iter().map(|m| m.ident.clone()));
            if !stmt.is_empty() {
                let _ = writeln!(body, "    {stmt}");
            }
        }
        let built = pattern(o);
        match DECODE_CHECKS
            .iter()
            .find(|(k, _, when, _)| *k == o.key && *when == Check::Post)
        {
            Some((_, hook, _, _)) => {
                let _ = write!(
                    body,
                    "    let v = {built};\n    {hook}(path, fields, &v)?;\n    Ok(v)"
                );
            }
            None => {
                let _ = write!(body, "    Ok({built})");
            }
        }
        Ok(body)
    }

    /// A struct owner: one encoder, one decoder.
    fn struct_owner(&mut self, c: &Codec, o: &CodecOwner, rust: &str) -> Result<(), String> {
        let name = fn_name(rust);
        let bind_all = if o.members.is_empty() {
            "let _ = v;".to_string()
        } else {
            format!("let {} = v;", pattern(o))
        };
        let body = self.enc_body(c, o, "obj(f)")?;
        let _ = writeln!(
            self.enc,
            "\npub(in super::super) fn {name}(v: &{rust}) -> String {{\n    {bind_all}\n    {body}\n}}"
        );
        let dec_name = if rust == "Node" {
            "decode_node_ast".to_string()
        } else {
            format!("decode_{name}")
        };
        if let Some((_, hook, kind, _)) = owner_hook(DECODE_OWNERS, &o.key, &mut self.dec_owners) {
            if *kind == Hook::Shape && (0..o.fields.len()).all(|i| c.dec_field(o, i).is_some()) {
                return Err(format!(
                    "dialect decode owner hook {} is stale: the uniform rules now decode every \
                     field; delete the entry",
                    o.key
                ));
            }
            let _ = writeln!(
                self.dec,
                "\npub(in super::super) fn {dec_name}(path: &str, j: &JVal) -> DResult<{rust}> {{\n    {hook}(path, j)\n}}"
            );
            return Ok(());
        }
        let enter = DECODE_ENTER
            .iter()
            .find(|(k, _, _)| *k == o.key)
            .map_or_else(String::new, |(_, hook, _)| {
                format!("let _guard = {hook}(path)?;\n    ")
            });
        let pre = UNION_PRE.iter().find(|(k, _, _)| *k == o.key).map_or_else(
            String::new,
            |(_, hook, _)| {
                format!("if let Some(r) = {hook}(path, j) {{\n        return r;\n    }}\n    ")
            },
        );
        let body = self.dec_body(c, o)?;
        let _ = writeln!(
            self.dec,
            "\npub(in super::super) fn {dec_name}(path: &str, j: &JVal) -> DResult<{rust}> {{\n    {enter}{pre}let fields = as_obj(path, j)?;\n{body}\n}}"
        );
        Ok(())
    }

    /// A union (or `TreeOp`): an encoder matching every case, and a decoder
    /// dispatching on the discriminator to one function per case.
    fn union_owner(
        &mut self,
        c: &Codec,
        name: &str,
        rust: &str,
        tags_const: &str,
        cases: &[CodecOwner],
    ) -> Result<(), String> {
        let f = fn_name(rust);
        let transparent = c
            .idl
            .transparent
            .iter()
            .find(|(u, _)| u == name)
            .map(|(_, case)| case.clone());
        let transparent_value = |o: &CodecOwner| -> Option<(String, String)> {
            if transparent.as_deref() != Some(case_tag(o).as_str())
                || o.fields.len() != 1
                || o.members.len() != 1
            {
                return None;
            }
            let m = &o.members[0];
            Some((
                c.enc_value(&o.fields[0].ty, &m.ty, &bind(m))?,
                c.dec_value(&o.fields[0].ty, &m.ty)?,
            ))
        };
        if owner_hook(ENCODE_OWNERS, name, &mut self.enc_owners).is_none() {
            let mut arms = String::new();
            for o in cases {
                let tag = case_tag(o);
                let body = match transparent_value(o) {
                    // Rule: a transparent case is written as its bare value.
                    Some((enc, _)) => enc,
                    None => self.enc_body(c, o, &format!("case_obj(\"{tag}\", f)"))?,
                };
                let _ = writeln!(arms, "    {} => {{\n    {body}\n    }}", pattern(o));
            }
            let _ = writeln!(
                self.enc,
                "\npub(in super::super) fn {f}(v: &{rust}) -> String {{\n    match v {{\n{}    }}\n}}",
                indent(&arms)
            );
        }
        let dec_name = if rust == "TreeOp" {
            "decode_tree_op_ast".to_string()
        } else {
            format!("decode_{f}")
        };
        if let Some((_, hook, _, _)) = owner_hook(DECODE_OWNERS, name, &mut self.dec_owners) {
            // A union decoded by hand dispatches over this tag enum, so a case the
            // IDL gains is a non-exhaustive match in the hand decoder.
            let variants: Vec<String> = cases.iter().map(case_tag).collect();
            let _ = writeln!(
                self.dec,
                "\n/// The discriminators of `{rust}`, for its hand decoder's exhaustive dispatch.\n#[derive(Clone, Copy)]\npub(in super::super) enum {rust}Tag {{\n{}}}\n\nimpl {rust}Tag {{\n    pub(in super::super) fn of(tag: &str) -> Option<Self> {{\n        match tag {{\n{}            _ => None,\n        }}\n    }}\n}}",
                variants
                    .iter()
                    .map(|v| format!("    {v},\n"))
                    .collect::<String>(),
                variants
                    .iter()
                    .map(|v| format!("            \"{v}\" => Some(Self::{v}),\n"))
                    .collect::<String>()
            );
            // An empty hook name: the hand decoder takes a context argument and is
            // called by name from the hooks that supply it.
            if !hook.is_empty() {
                let _ = writeln!(
                    self.dec,
                    "\npub(in super::super) fn {dec_name}(path: &str, j: &JVal) -> DResult<{rust}> {{\n    {hook}(path, j)\n}}"
                );
            }
            return Ok(());
        }
        let enter = DECODE_ENTER
            .iter()
            .find(|(k, _, _)| *k == name)
            .map_or_else(String::new, |(_, hook, _)| {
                format!("let _guard = {hook}(path)?;\n    ")
            });
        let mut pre = UNION_PRE.iter().find(|(k, _, _)| *k == name).map_or_else(
            String::new,
            |(_, hook, _)| {
                format!("if let Some(r) = {hook}(path, j) {{\n        return r;\n    }}\n    ")
            },
        );
        // Rule: a value that is not an object is the transparent case's bare
        // spelling.
        if let Some(o) = cases.iter().find(|o| transparent_value(o).is_some())
            && let Some((_, dec)) = transparent_value(o)
        {
            let build = if o.positional() {
                o.path.clone()
            } else {
                format!("|{}| {}", bind(&o.members[0]), pattern(o))
            };
            let _ = write!(
                pre,
                "if !matches!(j, JVal::Obj(_)) {{\n        return ({dec})(path, j).map({build});\n    }}\n    "
            );
        }
        let mut dispatch = String::new();
        let mut case_fns = String::new();
        for o in cases {
            let tag = case_tag(o);
            let case_fn = format!("{dec_name}_{}", fn_name(&tag));
            let _ = writeln!(dispatch, "        \"{tag}\" => {case_fn}(path, fields),");
            let body = match owner_hook(DECODE_OWNERS, &o.key, &mut self.dec_owners) {
                Some((_, hook, _, _)) => format!("    {hook}(path, fields)"),
                None => self.dec_body(c, o)?,
            };
            // A case that reads nothing names its parameters unused.
            let (p, fs) = if body.contains("path") {
                ("path", "fields")
            } else {
                ("_path", "_fields")
            };
            let _ = writeln!(
                case_fns,
                "\npub(in super::super) fn {case_fn}({p}: &str, {fs}: &Fields) -> DResult<{rust}> {{\n{body}\n}}"
            );
        }
        let _ = writeln!(
            self.dec,
            "\npub(in super::super) fn {dec_name}(path: &str, j: &JVal) -> DResult<{rust}> {{\n    {enter}{pre}let fields = as_obj(path, j)?;\n    match disc(path, fields)? {{\n{dispatch}        other => Err(unknown_du_case(path, other, {tags_const})),\n    }}\n}}\n{case_fns}"
        );
        Ok(())
    }
}

/// The generated codec: `gen_encode` and `gen_decode`, appended to the module.
fn write_codec(w: &mut String, idl: &Idl) -> Result<(), String> {
    let g = containment(idl);
    let c = Codec {
        enums: wire_enums(idl).iter().map(|e| rust_type_name(e)).collect(),
        idl,
    };
    let mut e = Emit {
        missing: Vec::new(),
        enc: String::new(),
        dec: String::new(),
        enc_used: HookUse::new(),
        dec_used: HookUse::new(),
        enc_owners: OwnerUse::new(),
        dec_owners: OwnerUse::new(),
    };
    let hand =
        |n: &str| INLINED.iter().any(|(i, _)| *i == n) || HAND_TYPES.iter().any(|(h, _)| *h == n);

    // Kinds: each spec, then NodeKind over them.
    let mut kind_arms = String::new();
    let mut kind_dispatch = String::new();
    let mut kind_fns = String::new();
    for k in &idl.kinds {
        let tag = &k.owner.name;
        let variant_fn = format!("kind_{}", fn_name(tag));
        if let Some((_, payload, _)) = KIND_PAYLOADS.iter().find(|(t, _, _)| t == tag) {
            let mut hook = |table: &[OwnerHook], used: &mut OwnerUse, what: &str| {
                owner_hook(table, tag, used).map_or_else(
                    || {
                        e.missing
                            .push(format!("{what} owner {tag} (carries {payload})"));
                        String::new()
                    },
                    |h| h.1.to_string(),
                )
            };
            let enc_hook = hook(ENCODE_OWNERS, &mut e.enc_owners, "encode");
            let dec_hook = hook(DECODE_OWNERS, &mut e.dec_owners, "decode");
            let _ = writeln!(
                kind_arms,
                "        NodeKind::{tag}(m_0) => {enc_hook}(m_0),"
            );
            let _ = writeln!(
                kind_fns,
                "\nfn {variant_fn}(path: &str, j: &JVal) -> DResult<NodeKind> {{\n    Ok(NodeKind::{tag}({dec_hook}(path, j)?))\n}}"
            );
        } else {
            let rust = kind_type_name(tag);
            let o = codec_owner(
                &g,
                tag.clone(),
                tag.clone(),
                rust.clone(),
                &rust,
                k.owner.fields.iter().collect(),
            )?;
            e.struct_owner(&c, &o, &rust)?;
            let _ = writeln!(
                kind_arms,
                "        NodeKind::{tag}(m_0) => case_obj_hoisted(\"{tag}\", {}(m_0)),",
                fn_name(&rust)
            );
            let _ = writeln!(
                kind_fns,
                "\nfn {variant_fn}(path: &str, j: &JVal) -> DResult<NodeKind> {{\n    Ok(NodeKind::{tag}(decode_{}(path, j)?))\n}}",
                fn_name(&rust)
            );
        }
        let _ = writeln!(kind_dispatch, "        \"{tag}\" => {variant_fn}(path, j),");
    }
    let _ = writeln!(
        e.enc,
        "\npub(in super::super) fn node_kind(v: &NodeKind) -> String {{\n    match v {{\n{kind_arms}    }}\n}}"
    );
    let _ = writeln!(
        e.dec,
        "\n/// Each kind decodes in its own frame, so the dispatch's frame holds one result
/// however many kinds there are (the §21 depth bound is measured on the stack).
pub(in super::super) fn decode_node_kind(path: &str, j: &JVal) -> DResult<NodeKind> {{\n    let fields = as_obj(path, j)?;\n    match disc(path, fields)? {{\n{kind_dispatch}        other => Err(wrong_node_kind(path, other)),\n    }}\n}}\n{kind_fns}"
    );

    // Records.
    for r in idl.records.iter().filter(|r| !hand(&r.name)) {
        let rust = rust_type_name(&r.name);
        let o = codec_owner(
            &g,
            r.name.clone(),
            r.name.clone(),
            rust.clone(),
            &rust,
            r.fields.iter().collect(),
        )?;
        e.struct_owner(&c, &o, &rust)?;
    }

    // Unions.
    for u in idl.unions.iter().filter(|u| !hand(&u.name)) {
        let rust = rust_type_name(&u.name);
        let mut cases = Vec::new();
        for case in &u.cases {
            cases.push(codec_owner(
                &g,
                format!("{}.{}", u.name, case.name),
                format!("{}{}", u.name, case.name),
                format!("{rust}::{}", case.name),
                &rust,
                case.fields.iter().collect(),
            )?);
        }
        let tags = if u.name == "FormFieldKind" {
            "CANONICAL_FORM_FIELD_KINDS".to_string()
        } else {
            format!("{}_TAGS", screaming(&u.name))
        };
        e.union_owner(&c, &u.name, &rust, &tags, &cases)?;
    }

    // The node envelope: its identity and kind, then the IDL's node fields.
    let id = Field {
        name: "id".into(),
        optionality: "required".into(),
        ty: JVal::Obj(vec![("$type".into(), JVal::Str("str".into()))]),
        default: None,
    };
    let kind = Field {
        name: "kind".into(),
        optionality: "required".into(),
        ty: JVal::Obj(vec![("$type".into(), JVal::Str("kind".into()))]),
        default: None,
    };
    let mut node_fields: Vec<&Field> = vec![&id, &kind];
    node_fields.extend(idl.node_fields.iter());
    let mut node = codec_owner(
        &g,
        "Node".into(),
        String::new(),
        "Node".into(),
        "Node",
        node_fields,
    )?;
    node.per_field[0] = vec![Member {
        ident: "id".into(),
        ty: "NodeId".into(),
    }];
    node.per_field[1] = vec![Member {
        ident: "kind".into(),
        ty: "NodeKind".into(),
    }];
    let mut members = vec![node.per_field[0][0].clone(), node.per_field[1][0].clone()];
    members.extend(
        node.members
            .iter()
            .filter(|m| m.ident != "id" && m.ident != "kind")
            .cloned(),
    );
    node.members = members;
    e.struct_owner(&c, &node, "Node")?;

    // Tree ops.
    let mut ops = Vec::new();
    for o in &idl.ops {
        ops.push(codec_owner(
            &g,
            format!("TreeOp.{}", o.name),
            String::new(),
            format!("TreeOp::{}", o.name),
            "TreeOp",
            o.fields.iter().collect(),
        )?);
    }
    e.union_owner(&c, "TreeOp", "TreeOp", "TREE_OP_TAGS", &ops)?;

    // Bare-string enum decoders, with their foreign spellings as data.
    for name in &c.enums {
        if owner_hook(DECODE_OWNERS, name, &mut e.dec_owners).is_some() {
            continue;
        }
        let aliases = ENUM_ALIASES.iter().find(|(n, _, _)| n == name).map_or_else(
            String::new,
            |(_, a, _)| {
                a.iter()
                    .map(|(wire, case)| format!("(\"{wire}\", {name}::{case})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        let _ = writeln!(
            e.dec,
            "\npub(in super::super) fn decode_{}(path: &str, j: &JVal) -> DResult<{name}> {{\n    decode_enum(path, j, \"{name}\", {name}::from_wire, {name}::WIRE_NAMES, &[{aliases}])\n}}",
            fn_name(name)
        );
    }

    if !e.missing.is_empty() {
        return Err(format!(
            "no uniform codec rule and no dialect hook for: {}",
            e.missing.join("; ")
        ));
    }
    check_codec_dialect(idl, &e)?;

    let _ = writeln!(
        w,
        "
// ─── The generated codec (Phase 2333) ────────────────────────────────────────
//
// The uniform half: one encoder and one decoder per owner, from the IDL type, the
// optionality and the generated members. Policy is the hand hooks the dialect
// names, in encode.rs and decode.rs.

pub(super) mod gen_encode {{
    use super::*;
    use super::super::encode::*;
{}}}

pub(super) mod gen_decode {{
    use super::*;
    use super::super::decode::*;
{}}}",
        indent(&e.enc),
        indent(&e.dec)
    );
    Ok(())
}

/// Every codec entry of the dialect names something the IDL defines, and was used.
fn check_codec_dialect(idl: &Idl, e: &Emit) -> Result<(), String> {
    for (owner, field, _, _, _) in ENCODE_HOOKS {
        if !e
            .enc_used
            .contains(&((*owner).to_string(), (*field).to_string()))
        {
            return Err(format!(
                "dialect encode hook {owner}.{field} names a field the IDL does not have; delete it"
            ));
        }
    }
    for (owner, field, _, _, _) in DECODE_HOOKS {
        if !e
            .dec_used
            .contains(&((*owner).to_string(), (*field).to_string()))
        {
            return Err(format!(
                "dialect decode hook {owner}.{field} names a field the IDL does not have; delete it"
            ));
        }
    }
    for (owner, _, _, _) in ENCODE_OWNERS {
        if !e.enc_owners.contains(*owner) {
            return Err(format!(
                "dialect encode owner hook {owner} names nothing the IDL defines; delete it"
            ));
        }
    }
    for (owner, _, _, _) in DECODE_OWNERS {
        if !e.dec_owners.contains(*owner) {
            return Err(format!(
                "dialect decode owner hook {owner} names nothing the IDL defines; delete it"
            ));
        }
    }
    for (owner, field, _, _) in DECODE_ALIASES {
        if !owner_has_field(idl, owner, field) {
            return Err(format!(
                "dialect alias entry {owner}.{field} names a field the IDL does not have; delete it"
            ));
        }
    }
    for (owner, _, _, _) in DECODE_CHECKS {
        if !owner_has_field(idl, owner, "") {
            return Err(format!(
                "dialect decode check {owner} names nothing the IDL defines; delete it"
            ));
        }
    }
    for (owner, _, _) in DECODE_ENTER.iter().chain(UNION_PRE) {
        if !owner_has_field(idl, owner, "") {
            return Err(format!(
                "dialect decode entry {owner} names nothing the IDL defines; delete it"
            ));
        }
    }
    for (name, aliases, _) in ENUM_ALIASES {
        let en = enum_by_name(idl, name)
            .ok_or_else(|| format!("dialect enum alias entry {name} names no IDL enum"))?;
        for (wire, case) in *aliases {
            if en.cases.iter().any(|c| c == wire) {
                return Err(format!(
                    "dialect enum alias {name}.{wire} is stale: it is now a case; delete it"
                ));
            }
            if !en.host_cases.iter().any(|h| case_ident(h) == *case) {
                return Err(format!(
                    "dialect enum alias {name}.{wire} maps to no case {case}"
                ));
            }
        }
    }
    Ok(())
}

/// Whether an owner key (and, when `field` is non-empty, its field) names
/// something the IDL defines.
fn owner_has_field(idl: &Idl, owner: &str, field: &str) -> bool {
    let has = |fs: &[Field]| field.is_empty() || fs.iter().any(|f| f.name == field);
    if owner == "Node" {
        return field.is_empty() || field == "id" || field == "kind" || has(&idl.node_fields);
    }
    if owner == "TreeOp" {
        return field.is_empty();
    }
    if let Some((u, case)) = owner.split_once('.') {
        if u == "TreeOp" {
            return idl.ops.iter().any(|o| o.name == case && has(&o.fields));
        }
        return idl
            .unions
            .iter()
            .any(|x| x.name == u && x.cases.iter().any(|c| c.name == case && has(&c.fields)));
    }
    idl.kinds
        .iter()
        .any(|k| k.owner.name == owner && has(&k.owner.fields))
        || idl
            .records
            .iter()
            .any(|r| r.name == owner && has(&r.fields))
        || (field.is_empty() && idl.unions.iter().any(|u| u.name == owner))
}

fn indent(code: &str) -> String {
    code.lines()
        .map(|l| {
            if l.is_empty() {
                "\n".to_string()
            } else {
                format!("    {l}\n")
            }
        })
        .collect()
}
