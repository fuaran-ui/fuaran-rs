//! The declared theme-token contract — the machine-readable theme the AI reasons
//! against and the contrast tier verifies resolved style against. DTCG-compatible
//! (a vanilla DTCG file decodes cleanly) extended with the two things DTCG lacks:
//! a per-token role→tone binding (so `Tone::Brand` is known to resolve to the
//! manifest's brand token) and an invariant block (contrast floors, colour-usage
//! budgets, motion voice, each soft-weighted). Tones are their wire strings
//! (`"Default"`…`"Info"`). A sibling of the F#/TS/Python/Go ThemeManifest tiers,
//! built to the same shapes; the closed role + invariant DUs are native `enum`s.

use crate::canonical::{JVal, parse, render_canonical};
use crate::theme::contrast::{ContrastVerdict, Rgba, verdict};

/// The canonical `ToneVariant` palette, as wire strings.
pub const TONES: &[&str] = &[
    "Default", "Subdued", "Brand", "Success", "Warning", "Critical", "Info",
];

/// Validate a wire string as a tone, returning `None` for an unrecognised token.
pub fn tone_of_string(s: &str) -> Option<&'static str> {
    TONES.iter().copied().find(|&t| t == s)
}

/// The soft weight an invariant carries unless overridden.
pub const DEFAULT_WEIGHT: f64 = 1.0;

/// The `meta` block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestMeta {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

/// One token — DTCG-compatible (`type`/`value`/`description` round-trip the DTCG
/// `$type`/`$value`/`$description`) plus the dual-field `role` tag. `name` is the
/// dotted path (`"color.brand.base"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestToken {
    pub name: String,
    pub token_type: String,
    pub value: String,
    pub description: Option<String>,
    pub role: Option<String>,
}

/// A role binding target — a tone variant or a broader named role. Closed DU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestRole {
    /// Binds a role to one of the canonical tone variants.
    Tone(String),
    /// Binds a role to a broader named semantic role (body text, divider…).
    Named(String),
}

fn role_key(r: &ManifestRole) -> String {
    match r {
        ManifestRole::Tone(t) => format!("tone:{t}"),
        ManifestRole::Named(n) => format!("named:{n}"),
    }
}

/// Binds a role onto a manifest token by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleBinding {
    pub role: ManifestRole,
    pub token_name: String,
}

/// The motion-voice budget — the payload of [`InvariantKind::MotionVoice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionBudget {
    pub max_duration_ms: i64,
    pub easing: Option<String>,
}

/// A declared invariant's payload (closed DU).
#[derive(Debug, Clone, PartialEq)]
pub enum InvariantKind {
    /// A named role's resolved contrast must be at least `min_ratio`.
    ContrastFloor { role: String, min_ratio: f64 },
    /// A token's share of visible surface must stay within `target_pct ± tolerance_pct`.
    UsageBudget {
        token: String,
        target_pct: f64,
        tolerance_pct: f64,
    },
    /// The theme's motion must stay within the declared budget.
    MotionVoice { budget: MotionBudget },
}

impl InvariantKind {
    /// The stable discriminator string for an invariant.
    pub fn kind_name(&self) -> &'static str {
        match self {
            InvariantKind::ContrastFloor { .. } => "ContrastFloor",
            InvariantKind::UsageBudget { .. } => "UsageBudget",
            InvariantKind::MotionVoice { .. } => "MotionVoice",
        }
    }
}

/// One declared invariant + its soft weight.
#[derive(Debug, Clone, PartialEq)]
pub struct Invariant {
    pub kind: InvariantKind,
    pub weight: f64,
}

impl Invariant {
    /// Construct an invariant with the default weight.
    pub fn new(kind: InvariantKind) -> Self {
        Invariant {
            kind,
            weight: DEFAULT_WEIGHT,
        }
    }
}

/// The declared theme contract: metadata + tokens + role bindings + invariants.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThemeManifest {
    pub meta: ManifestMeta,
    pub tokens: Vec<ManifestToken>,
    pub roles: Vec<RoleBinding>,
    pub invariants: Vec<Invariant>,
}

impl ThemeManifest {
    /// Look up a token by its dotted name.
    pub fn try_get_token(&self, name: &str) -> Option<&ManifestToken> {
        self.tokens.iter().find(|t| t.name == name)
    }

    /// Resolve a tone to its declared manifest token, or `None`.
    pub fn resolve_role(&self, tone: &str) -> Option<&ManifestToken> {
        self.roles.iter().find_map(|b| match &b.role {
            ManifestRole::Tone(t) if t == tone => self.try_get_token(&b.token_name),
            _ => None,
        })
    }

    /// Resolve a named (non-tone) role to its declared manifest token.
    pub fn resolve_named_role(&self, role: &str) -> Option<&ManifestToken> {
        self.roles.iter().find_map(|b| match &b.role {
            ManifestRole::Named(n) if n == role => self.try_get_token(&b.token_name),
            _ => None,
        })
    }

    /// Every colour value declared in the palette — the off-palette check's
    /// membership set.
    pub fn palette_colours(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .tokens
            .iter()
            .filter(|t| t.token_type == "color")
            .map(|t| t.value.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

// ─── JVal helpers ────────────────────────────────────────────────────────────

fn as_obj(v: &JVal) -> Option<&[(String, JVal)]> {
    match v {
        JVal::Obj(f) => Some(f),
        _ => None,
    }
}

fn get<'a>(fields: &'a [(String, JVal)], key: &str) -> Option<&'a JVal> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn as_str(v: Option<&JVal>) -> Option<String> {
    match v {
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn as_num(v: Option<&JVal>) -> Option<f64> {
    match v {
        Some(JVal::Num(n)) => Some(*n),
        _ => None,
    }
}

fn str_or(v: Option<&JVal>, def: &str) -> String {
    as_str(v).unwrap_or_else(|| def.to_string())
}

fn num_or(v: Option<&JVal>, def: f64) -> f64 {
    as_num(v).unwrap_or(def)
}

fn sorted_keys(fields: &[(String, JVal)]) -> Vec<&str> {
    let mut keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    keys
}

// ─── DTCG token-tree walk ────────────────────────────────────────────────────

fn walk_tokens(prefix: &str, node: &JVal) -> Vec<ManifestToken> {
    let Some(fields) = as_obj(node) else {
        return vec![];
    };
    if get(fields, "$value").is_some() {
        let role = get(fields, "$extensions")
            .and_then(as_obj)
            .and_then(|ext| get(ext, "fuaran"))
            .and_then(as_obj)
            .and_then(|f| as_str(get(f, "role")));
        return vec![ManifestToken {
            name: prefix.to_string(),
            token_type: str_or(get(fields, "$type"), ""),
            value: str_or(get(fields, "$value"), ""),
            description: as_str(get(fields, "$description")),
            role,
        }];
    }
    let mut out = vec![];
    for k in sorted_keys(fields) {
        if k.starts_with('$') {
            continue;
        }
        let next = if prefix.is_empty() {
            k.to_string()
        } else {
            format!("{prefix}.{k}")
        };
        out.extend(walk_tokens(&next, get(fields, k).expect("key present")));
    }
    out
}

// ─── roles + invariants ──────────────────────────────────────────────────────

fn parse_role(j: &JVal) -> ManifestRole {
    let Some(fields) = as_obj(j) else {
        return ManifestRole::Named(String::new());
    };
    if let Some(tone) = as_str(get(fields, "tone")) {
        return match tone_of_string(&tone) {
            Some(validated) => ManifestRole::Tone(validated.to_string()),
            None => ManifestRole::Named(tone),
        };
    }
    ManifestRole::Named(str_or(get(fields, "named"), ""))
}

fn parse_role_binding(j: &JVal) -> Option<RoleBinding> {
    let fields = as_obj(j)?;
    let token = as_str(get(fields, "token"))?;
    let role = get(fields, "role")
        .map(parse_role)
        .unwrap_or_else(|| ManifestRole::Named(String::new()));
    Some(RoleBinding {
        role,
        token_name: token,
    })
}

fn parse_invariant(j: &JVal) -> Option<Invariant> {
    let fields = as_obj(j)?;
    let weight = num_or(get(fields, "weight"), DEFAULT_WEIGHT);
    let kind = match str_or(get(fields, "kind"), "").as_str() {
        "ContrastFloor" => InvariantKind::ContrastFloor {
            role: str_or(get(fields, "role"), ""),
            min_ratio: num_or(get(fields, "minRatio"), 0.0),
        },
        "UsageBudget" => InvariantKind::UsageBudget {
            token: str_or(get(fields, "token"), ""),
            target_pct: num_or(get(fields, "targetPct"), 0.0),
            tolerance_pct: num_or(get(fields, "tolerancePct"), 0.0),
        },
        "MotionVoice" => InvariantKind::MotionVoice {
            budget: MotionBudget {
                max_duration_ms: num_or(get(fields, "maxDurationMs"), 0.0) as i64,
                easing: as_str(get(fields, "easing")),
            },
        },
        _ => return None,
    };
    Some(Invariant { kind, weight })
}

fn parse_meta(fields: &[(String, JVal)]) -> ManifestMeta {
    ManifestMeta {
        name: str_or(get(fields, "name"), ""),
        version: str_or(get(fields, "version"), ""),
        description: as_str(get(fields, "description")),
    }
}

fn as_array(v: Option<&JVal>) -> &[JVal] {
    match v {
        Some(JVal::Arr(a)) => a,
        _ => &[],
    }
}

/// Build a manifest from a parsed JSON value. Two top-level shapes are accepted:
/// a Fuaran manifest wrapper (`{meta, tokens, roles, invariants}` — selected by a
/// top-level `tokens` key) or a vanilla DTCG token tree (decodes to tokens only).
pub fn of_json(root: &JVal) -> ThemeManifest {
    let Some(fields) = as_obj(root) else {
        return ThemeManifest::default();
    };
    if let Some(tokens_node) = get(fields, "tokens") {
        let meta = get(fields, "meta")
            .and_then(as_obj)
            .map(parse_meta)
            .unwrap_or_default();
        let roles = as_array(get(fields, "roles"))
            .iter()
            .filter_map(parse_role_binding)
            .collect();
        let invariants = as_array(get(fields, "invariants"))
            .iter()
            .filter_map(parse_invariant)
            .collect();
        ThemeManifest {
            meta,
            tokens: walk_tokens("", tokens_node),
            roles,
            invariants,
        }
    } else {
        ThemeManifest {
            meta: ManifestMeta::default(),
            tokens: walk_tokens("", root),
            roles: vec![],
            invariants: vec![],
        }
    }
}

/// Decode a manifest from JSON; `None` on a parse failure.
pub fn decode(json: &str) -> Option<ThemeManifest> {
    parse(json).ok().map(|root| of_json(&root))
}

/// Project a DTCG / tokens.json file into a manifest (values lossless; roles
/// unmined). The decoder *is* the projection for a DTCG source.
pub fn project_from_dtcg(json: &str) -> Option<ThemeManifest> {
    decode(json)
}

// ─── Encode (the inverse of `of_json` / `decode`) ────────────────────────────

/// Push a string member only when it is non-empty — the omit-at-default rule for
/// every member the decoder reads through `str_or(_, "")`, which cannot tell an
/// absent key from an empty one.
fn put_str(fields: &mut Vec<(String, JVal)>, key: &str, value: &str) {
    if !value.is_empty() {
        fields.push((key.to_string(), JVal::Str(value.to_string())));
    }
}

/// Push a numeric member only when it differs from the value `num_or` supplies
/// in its absence.
fn put_num(fields: &mut Vec<(String, JVal)>, key: &str, value: f64, default: f64) {
    if value != default {
        fields.push((key.to_string(), JVal::Num(value)));
    }
}

/// One DTCG token node. `$value` is the leaf discriminator `walk_tokens` stops
/// on, so it is emitted unconditionally — an empty value included.
fn token_leaf(t: &ManifestToken) -> JVal {
    let mut fields = vec![];
    put_str(&mut fields, "$type", &t.token_type);
    fields.push(("$value".to_string(), JVal::Str(t.value.clone())));
    if let Some(d) = &t.description {
        fields.push(("$description".to_string(), JVal::Str(d.clone())));
    }
    if let Some(role) = &t.role {
        fields.push((
            "$extensions".to_string(),
            JVal::Obj(vec![(
                "fuaran".to_string(),
                JVal::Obj(vec![("role".to_string(), JVal::Str(role.clone()))]),
            )]),
        ));
    }
    JVal::Obj(fields)
}

/// Does the decoder's token walk stop at this node?
fn is_leaf(v: &JVal) -> bool {
    matches!(v, JVal::Obj(f) if get(f, "$value").is_some())
}

/// Place one token at its dotted path, replacing whatever occupies that path.
///
/// A DTCG path addresses a group **or** a token, never both, so two tokens whose
/// names collide (equal, or one a strict prefix of the other) are not jointly
/// representable. The later write wins — the same precedence [`dedupe_tokens`]
/// and [`merge`] already apply throughout this module — which is why descending
/// past a leaf clears it: leaving its `$value` in place would hide every
/// descendant from the decoder and make the *earlier* token win instead.
fn insert_at(tree: &mut Vec<(String, JVal)>, segments: &[&str], leaf: JVal) {
    let Some((head, rest)) = segments.split_first() else {
        return;
    };
    let slot = match tree.iter().position(|(k, _)| k == head) {
        Some(i) => i,
        None => {
            tree.push(((*head).to_string(), JVal::Obj(vec![])));
            tree.len() - 1
        }
    };
    if rest.is_empty() {
        tree[slot].1 = leaf;
        return;
    }
    if is_leaf(&tree[slot].1) {
        tree[slot].1 = JVal::Obj(vec![]);
    }
    if let JVal::Obj(children) = &mut tree[slot].1 {
        insert_at(children, rest, leaf);
    }
}

fn tokens_tree(tokens: &[ManifestToken]) -> JVal {
    let mut root = vec![];
    for t in tokens {
        let segments: Vec<&str> = t.name.split('.').collect();
        insert_at(&mut root, &segments, token_leaf(t));
    }
    JVal::Obj(root)
}

fn role_json(r: &ManifestRole) -> JVal {
    let (key, value) = match r {
        ManifestRole::Tone(t) => ("tone", t),
        ManifestRole::Named(n) => ("named", n),
    };
    JVal::Obj(vec![(key.to_string(), JVal::Str(value.clone()))])
}

fn role_binding_json(b: &RoleBinding) -> JVal {
    // `token` is required — `parse_role_binding` drops a binding without it.
    let mut fields = vec![("token".to_string(), JVal::Str(b.token_name.clone()))];
    // An absent `role` decodes to `Named("")`, so that one value is omitted.
    if b.role != ManifestRole::Named(String::new()) {
        fields.push(("role".to_string(), role_json(&b.role)));
    }
    JVal::Obj(fields)
}

fn invariant_json(inv: &Invariant) -> JVal {
    // `kind` is the discriminator — an unrecognised or absent one drops the
    // invariant at decode, so it is never omitted.
    let mut fields = vec![(
        "kind".to_string(),
        JVal::Str(inv.kind.kind_name().to_string()),
    )];
    match &inv.kind {
        InvariantKind::ContrastFloor { role, min_ratio } => {
            put_str(&mut fields, "role", role);
            put_num(&mut fields, "minRatio", *min_ratio, 0.0);
        }
        InvariantKind::UsageBudget {
            token,
            target_pct,
            tolerance_pct,
        } => {
            put_str(&mut fields, "token", token);
            put_num(&mut fields, "targetPct", *target_pct, 0.0);
            put_num(&mut fields, "tolerancePct", *tolerance_pct, 0.0);
        }
        InvariantKind::MotionVoice { budget } => {
            put_num(
                &mut fields,
                "maxDurationMs",
                budget.max_duration_ms as f64,
                0.0,
            );
            if let Some(e) = &budget.easing {
                fields.push(("easing".to_string(), JVal::Str(e.clone())));
            }
        }
    }
    put_num(&mut fields, "weight", inv.weight, DEFAULT_WEIGHT);
    JVal::Obj(fields)
}

fn meta_json(meta: &ManifestMeta) -> Option<JVal> {
    if meta == &ManifestMeta::default() {
        return None;
    }
    let mut fields = vec![];
    put_str(&mut fields, "name", &meta.name);
    put_str(&mut fields, "version", &meta.version);
    if let Some(d) = &meta.description {
        fields.push(("description".to_string(), JVal::Str(d.clone())));
    }
    Some(JVal::Obj(fields))
}

/// Build the JSON value for a manifest — the inverse of [`of_json`], for a host
/// embedding a manifest in a larger document rather than emitting it alone.
///
/// Always the Fuaran wrapper shape, never a bare DTCG tree: a top-level `tokens`
/// key is what selects the wrapper branch in [`of_json`], so omitting it at empty
/// would decode the document as vanilla DTCG and silently discard meta, roles and
/// invariants.
pub fn to_json(m: &ThemeManifest) -> JVal {
    let mut fields = vec![];
    if let Some(meta) = meta_json(&m.meta) {
        fields.push(("meta".to_string(), meta));
    }
    fields.push(("tokens".to_string(), tokens_tree(&m.tokens)));
    if !m.roles.is_empty() {
        let items: Vec<JVal> = m.roles.iter().map(role_binding_json).collect();
        fields.push(("roles".to_string(), JVal::Arr(items)));
    }
    if !m.invariants.is_empty() {
        let items: Vec<JVal> = m.invariants.iter().map(invariant_json).collect();
        fields.push(("invariants".to_string(), JVal::Arr(items)));
    }
    JVal::Obj(fields)
}

/// Encode a manifest as canonical JSON — the round trip the projectors and
/// [`merge`] had no way to emit, so a host that merged a brand override over a
/// base can hand the result back over the wire.
///
/// Canonical in the crate's one sense (`canonical::render_canonical`): Ordinal-
/// sorted keys, no whitespace, the §2 rule-5 number layout, the rule-6 escapes.
/// Every member the decoder tolerates the absence of is omitted at its default,
/// so a projected manifest does not carry a page of empty strings.
///
/// **The round trip, stated precisely.** `encode(decode(bytes)) == bytes` for
/// canonical bytes already in this shape, and `decode(encode(m)) == m` for any
/// manifest whose tokens are in the wire's own order — every manifest [`decode`]
/// produces is one. A projector or [`merge`] result carries tokens in
/// first-appearance order instead, and the wire's order is sorted, so the round
/// trip there preserves the token *set* and normalises the order; the total
/// statement that covers every manifest is that `encode` is a fixpoint through
/// it — `encode(decode(encode(m))) == encode(m)`.
///
/// **Three model states the wire cannot carry**, recorded rather than hidden
/// because each is reachable only by hand-building a `ThemeManifest` — no
/// decoder or projector in this module produces one. Two token names that
/// collide (equal, or one a strict prefix of the other) resolve last-write-wins,
/// per `insert_at`. A name whose first segment starts with `$` is emitted but is
/// unreachable to the decoder, which skips `$`-prefixed keys as DTCG metadata. A
/// `ManifestRole::Tone` holding a string outside [`TONES`] decodes back as
/// `ManifestRole::Named`, since `parse_role` validates the tone. Widening any of
/// these is a wire-format question for every host at once, not a change this host
/// makes alone.
pub fn encode(m: &ThemeManifest) -> String {
    render_canonical(&to_json(m))
}

// ─── CSS token-surface projectors ────────────────────────────────────────────

/// One selector block — its selector text + the `--name → value` custom-property
/// declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssBlock {
    pub selector: String,
    pub declarations: Vec<(String, String)>,
}

/// Strip `/* … */` comments (stdlib-only — no regex dependency).
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Scan flat `selector { … }` blocks, keeping only custom-property (`--`)
/// declarations.
pub fn scan_css_blocks(css: &str) -> Vec<CssBlock> {
    let cleaned = strip_comments(css);
    let mut blocks = vec![];
    for chunk in cleaned.split('}') {
        let Some(brace) = chunk.find('{') else {
            continue;
        };
        let selector = chunk[..brace].trim().to_string();
        let body = &chunk[brace + 1..];
        let mut decls = vec![];
        for decl in body.split(';') {
            let Some(colon) = decl.find(':') else {
                continue;
            };
            let name = decl[..colon].trim();
            let value = decl[colon + 1..].trim();
            if name.starts_with("--") {
                decls.push((name.to_string(), value.to_string()));
            }
        }
        if !decls.is_empty() {
            blocks.push(CssBlock {
                selector,
                declarations: decls,
            });
        }
    }
    blocks
}

fn infer_type(value: &str) -> String {
    let v = value.trim().to_lowercase();
    for p in ["#", "rgb", "hsl", "oklch", "oklab", "color("] {
        if v.starts_with(p) {
            return "color".to_string();
        }
    }
    for s in ["px", "rem", "em", "%"] {
        if v.ends_with(s) {
            return "dimension".to_string();
        }
    }
    for s in ["ms", "s"] {
        if v.ends_with(s) {
            return "duration".to_string();
        }
    }
    String::new()
}

fn token(name: &str, value: &str) -> ManifestToken {
    ManifestToken {
        name: name.to_string(),
        token_type: infer_type(value),
        value: value.to_string(),
        description: None,
        role: None,
    }
}

/// Keep the last write for each name, preserving first-appearance order of
/// surviving names (last-write-wins).
fn dedupe_tokens(tokens: Vec<ManifestToken>) -> Vec<ManifestToken> {
    let mut out: Vec<ManifestToken> = vec![];
    for t in tokens {
        if let Some(slot) = out.iter_mut().find(|e| e.name == t.name) {
            *slot = t;
        } else {
            out.push(t);
        }
    }
    out
}

fn cap1(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
    }
}

/// Project a `--fuaran-tone-{tone}-{slot}` set into tokens + Tone role bindings.
/// Token names are `tone.{tone}.{slot}`; each tone whose `bg` slot is present
/// gets a tone role binding to that token.
pub fn project_from_fuaran_tone_vars(css: &str) -> ThemeManifest {
    let mut raw = vec![];
    for b in scan_css_blocks(css) {
        for (name, value) in &b.declarations {
            let stripped = name.trim_start_matches('-');
            let Some(rest) = stripped.strip_prefix("fuaran-tone-") else {
                continue;
            };
            let parts: Vec<&str> = rest.split('-').collect();
            if parts.len() != 2 {
                continue;
            }
            let (tone, slot) = (parts[0], parts[1]);
            if tone_of_string(&cap1(tone)).is_none() {
                continue;
            }
            raw.push(token(
                &format!("tone.{}.{}", tone.to_lowercase(), slot.to_lowercase()),
                value,
            ));
        }
    }
    let tokens = dedupe_tokens(raw);
    let mut roles = vec![];
    for t in &tokens {
        let name_parts: Vec<&str> = t.name.split('.').collect();
        if name_parts.len() == 3 && name_parts[0] == "tone" && name_parts[2] == "bg" {
            if let Some(bound) = tone_of_string(&cap1(name_parts[1])) {
                roles.push(RoleBinding {
                    role: ManifestRole::Tone(bound.to_string()),
                    token_name: t.name.clone(),
                });
            }
        }
    }
    ThemeManifest {
        meta: ManifestMeta::default(),
        tokens,
        roles,
        invariants: vec![],
    }
}

fn is_dark_selector(selector: &str) -> bool {
    let s = selector.to_lowercase();
    s.contains("data-theme=dark") || s.contains("data-theme=\"dark\"") || s.contains(".dark")
}

/// Project a generic `:root` block (+ optional dark block) into tokens; roles are
/// left unbound. Dark tokens carry an `@dark` suffix.
pub fn project_from_css_custom_properties(css: &str) -> ThemeManifest {
    let blocks = scan_css_blocks(css);
    let mut all = vec![];
    for b in &blocks {
        if is_dark_selector(&b.selector) {
            continue;
        }
        for (name, value) in &b.declarations {
            all.push(token(name.trim_start_matches('-'), value));
        }
    }
    for b in &blocks {
        if !is_dark_selector(&b.selector) {
            continue;
        }
        for (name, value) in &b.declarations {
            all.push(token(
                &format!("{}@dark", name.trim_start_matches('-')),
                value,
            ));
        }
    }
    ThemeManifest {
        meta: ManifestMeta::default(),
        tokens: dedupe_tokens(all),
        roles: vec![],
        invariants: vec![],
    }
}

/// Combine base + override with last-write-wins precedence (the CSS cascade).
pub fn merge(base: &ThemeManifest, over: &ThemeManifest) -> ThemeManifest {
    let over_names: Vec<&str> = over.tokens.iter().map(|t| t.name.as_str()).collect();
    let mut tokens: Vec<ManifestToken> = base
        .tokens
        .iter()
        .filter(|t| !over_names.contains(&t.name.as_str()))
        .cloned()
        .collect();
    tokens.extend(over.tokens.iter().cloned());

    let over_roles: Vec<String> = over.roles.iter().map(|r| role_key(&r.role)).collect();
    let mut roles: Vec<RoleBinding> = base
        .roles
        .iter()
        .filter(|r| !over_roles.contains(&role_key(&r.role)))
        .cloned()
        .collect();
    roles.extend(over.roles.iter().cloned());

    let mut seen: Vec<String> = vec![];
    let mut invariants = vec![];
    for inv in over.invariants.iter().chain(base.invariants.iter()) {
        let k = invariant_key(inv);
        if !seen.contains(&k) {
            seen.push(k);
            invariants.push(inv.clone());
        }
    }

    let meta = if over.meta != ManifestMeta::default() {
        over.meta.clone()
    } else {
        base.meta.clone()
    };
    ThemeManifest {
        meta,
        tokens,
        roles,
        invariants,
    }
}

fn invariant_key(inv: &Invariant) -> String {
    let name = inv.kind.kind_name();
    match &inv.kind {
        InvariantKind::ContrastFloor { role, min_ratio } => {
            format!("{name}|{role}|{min_ratio}|{}", inv.weight)
        }
        InvariantKind::UsageBudget {
            token,
            target_pct,
            tolerance_pct,
        } => format!("{name}|{token}|{target_pct}|{tolerance_pct}|{}", inv.weight),
        InvariantKind::MotionVoice { budget } => format!(
            "{name}|{}|{}|{}",
            budget.max_duration_ms,
            budget.easing.as_deref().unwrap_or(""),
            inv.weight
        ),
    }
}

// ─── Contrast bridge (projected token → resolved contrast) ───────────────────

/// Parse a `#rgb` / `#rrggbb` hex colour token into an opaque [`Rgba`]. Other
/// colour forms (`rgb(…)`, `oklch(…)`, …) are not parsed here — the manifest
/// carries them verbatim; only hex projects to a resolved colour.
pub fn parse_hex(value: &str) -> Option<Rgba> {
    let hex = value.trim().strip_prefix('#')?;
    let (r, g, b) = match hex.len() {
        3 => {
            let d = |i: usize| {
                let c = hex.as_bytes()[i] as char;
                let v = c.to_digit(16)?;
                Some((v * 16 + v) as f64)
            };
            (d(0)?, d(1)?, d(2)?)
        }
        6 => {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(f64::from);
            (byte(0)?, byte(2)?, byte(4)?)
        }
        _ => return None,
    };
    Some(Rgba::rgb(r, g, b))
}

/// Resolve a tone to the opaque colour its manifest token declares (hex-parsed).
pub fn tone_rgba(manifest: &ThemeManifest, tone: &str) -> Option<Rgba> {
    parse_hex(&manifest.resolve_role(tone)?.value)
}

/// The WCAG contrast verdict between a foreground tone and a background tone,
/// resolving both through the manifest's role bindings — the contrast tier
/// consuming projected tokens (`InvariantKind::ContrastFloor` reasons against
/// this verdict).
pub fn tone_contrast(
    manifest: &ThemeManifest,
    foreground_tone: &str,
    background_tone: &str,
) -> Option<ContrastVerdict> {
    let fg = tone_rgba(manifest, foreground_tone)?;
    let bg = tone_rgba(manifest, background_tone)?;
    Some(verdict(fg, &[bg]))
}
