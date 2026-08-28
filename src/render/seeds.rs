//! The `Binding.State` SEEDING pass — `WIRE_FORMAT.md` §24.4.
//!
//! §24.1 says what a declared default resolves to *for the reader that carries
//! it*. §24.4 says what it means for every OTHER reader of the same slot: a
//! `Binding.State` carrying a `defaultValue` DECLARES the value of its slot, so
//! a grid bound to `$state.members` and carrying the rows, beside a badge whose
//! `Transform` derives over the same key and carries nothing, read the same
//! rows.
//!
//! It is a RENDER-parity obligation, not a codec one (§24.6): the bytes
//! round-trip identically with or without the rule, which is exactly why no
//! codec family catches a host that has not adopted it.
//!
//! The five rules, ported from the SPECIFICATION rather than from either
//! reference implementation, each answering a question two readers of one key
//! raise that one does not:
//!
//! 1. **Who declares** — any `Binding.State` with a PRESENT `defaultValue`, in
//!    any slot. There is no separate declaration form and no new namespace.
//! 2. **Precedence: host value > written value > seed.** A seed is the value of
//!    a slot before anything else has said anything, never an override. It is
//!    laid UNDER `BindingSources::state`, so a host value and a `set_state`
//!    write both win.
//! 3. **Order-independence** — seeding happens over the WHOLE tree before any
//!    binding resolves, so a badge that appears before the grid declaring the
//!    rows is not a special case.
//! 4. **Two declarations of one key** — a disagreement is `FUARAN106` (a
//!    validator concern, not this module's), but a renderer must still be
//!    deterministic and takes the FIRST declaration in tree order. An EMPTY
//!    declaration declares nothing: it is the value an unseeded slot already
//!    has.
//! 5. **A host-reserved key is never seeded** — a seed is a tree-originated
//!    write, and §12's reserved `host.` namespace refuses those on every path.
//!    Such a declaration still resolves for its OWN reader exactly as §24.1
//!    says, because the reader's own `default_value` is what `resolve` falls
//!    back to; it simply fills nothing for anyone else.
//!
//! ## Why the walk is over the CANONICAL JSON rather than the typed tree
//!
//! This host models the wire DUs as `enum`s with compile-time-exhaustive
//! `match`, which is its great advantage everywhere else and a liability here: a
//! typed walk that must reach a `Binding` in *every* slot of *every* `NodeKind`
//! and `Spec` carries a standing forward-coupling duty, and a new
//! binding-bearing field added tomorrow would silently stop being seeded rather
//! than failing to compile — the walk would still be exhaustive, just wrong.
//!
//! A structural descent over the tree's own canonical JSON has no such duty: it
//! finds a `State` binding in any slot, including one added later. And it fixes
//! the seed to what the DOCUMENT declares, which is the definition every host
//! can agree on by construction — the corpus round-trip law makes
//! `encode_node(decode(x)) == x`, so the bytes walked here are the bytes that
//! arrived. The cost is one encode + parse per render pass, paid once and not
//! per binding.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::canonical::{JVal, parse};
use crate::render::BindingSources;
use crate::wire::{Node, encode_node};

/// The HOST-OWNED state namespace (§12). A tree-originated write naming one of
/// these keys is refused, so a tree-originated SEED naming one must be too.
pub const HOST_RESERVED_STATE_PREFIX: &str = "host.";

/// The value each `$state.<key>` slot carries before anything else has said
/// anything — rule 1 filtered by rules 4 and 5.
///
/// Empty when the tree declares nothing.
pub fn collect_state_seeds(tree: &Node) -> HashMap<String, JVal> {
    let mut seeds = HashMap::new();
    // A canonical re-encode of a decoded tree is the document it was decoded
    // from (the corpus round-trip law), so this parse cannot fail on a tree this
    // crate produced. A failure is treated as "declares nothing" rather than
    // panicking: a renderer is total, and refusing to draw a page because a
    // seeding pass could not read it would be a worse answer than drawing it
    // unseeded.
    if let Ok(doc) = parse(&encode_node(tree)) {
        walk(&doc, &mut seeds);
    }
    seeds
}

/// Lay a tree's seeds UNDER a caller's own binding sources (rule 2: the caller
/// wins every key it names).
///
/// Returns the caller's own sources BORROWED when the tree declares nothing, so
/// an unseeded tree costs no clone.
pub fn with_state_seeds<'a>(tree: &Node, sources: &'a BindingSources) -> Cow<'a, BindingSources> {
    let seeds = collect_state_seeds(tree);
    if seeds.is_empty() {
        return Cow::Borrowed(sources);
    }
    let mut seeded = sources.clone();
    for (key, value) in seeds {
        seeded.state.entry(key).or_insert(value);
    }
    Cow::Owned(seeded)
}

/// Descend one canonical JSON value, recording the first declaration of each
/// key.
///
/// Object members are visited in the order the canonical encoder wrote them
/// (`JVal::Obj` is an ordered association list, and the encoder emits the
/// ordinal member sort), so "first in tree order" is a property of the BYTES
/// rather than of any host's map iteration. Array order is the wire's own.
fn walk(value: &JVal, seeds: &mut HashMap<String, JVal>) {
    match value {
        JVal::Arr(items) => {
            for item in items {
                walk(item, seeds);
            }
        }
        JVal::Obj(fields) => {
            if matches!(field(fields, "$type"), Some(JVal::Str(t)) if t == "State") {
                record(fields, seeds);
            }
            // Keep descending whatever the tag: a `Local` re-sync source, an
            // `I18n` argument, or a `Transform` param's `from` can nest another
            // binding underneath this one.
            for (_, v) in fields {
                walk(v, seeds);
            }
        }
        _ => {}
    }
}

fn field<'a>(fields: &'a [(String, JVal)], name: &str) -> Option<&'a JVal> {
    fields.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// Apply rules 1, 4 and 5 to one `Binding.State` object.
fn record(fields: &[(String, JVal)], seeds: &mut HashMap<String, JVal>) {
    let Some(JVal::Str(key)) = field(fields, "key") else {
        return;
    };
    // Rule 1 — a reader that declares nothing declares nothing. The field
    // aliases the decoder accepts (`initialValue` / `default`) are normalised
    // away by the canonical re-encode this walk reads, so only the canonical
    // spelling can appear here.
    let Some(declared) = field(fields, "defaultValue") else {
        return;
    };
    if key.starts_with(HOST_RESERVED_STATE_PREFIX) {
        return; // rule 5.
    }
    if is_empty_declaration(declared) {
        return; // rule 4 — an empty declaration declares nothing.
    }
    // Rule 4 — the FIRST declaration in tree order wins.
    seeds.entry(key.clone()).or_insert_with(|| declared.clone());
}

/// The EMPTY table, which is what a seed must not be.
///
/// `"defaultValue": []` is the identity of the seeding lattice, not a claim
/// about content: an unseeded slot already resolves to the empty table, so an
/// empty declaration adds nothing an absent one does not already say. Both
/// consequences are load-bearing rather than tidy. It must not WIN the
/// first-declaration race — `{"$type":"State","key":k,"defaultValue":[]}` is how
/// a `Transform` source slot says "I read this key and carry no data of my own",
/// so a badge spelling it before the grid that carries the rows would otherwise
/// seed the slot EMPTY and make rule 3 false. And it must not CONFLICT, or that
/// same pair would raise `FUARAN106` against the grid beside it — an Error on
/// the very document the seeding rule exists to make work.
fn is_empty_declaration(value: &JVal) -> bool {
    match value {
        JVal::Arr(items) => items.is_empty(),
        // The canonical columnar spelling of the same nothing.
        JVal::Obj(fields) => match field(fields, "columns") {
            Some(JVal::Obj(columns)) => columns.is_empty(),
            _ => false,
        },
        _ => false,
    }
}
