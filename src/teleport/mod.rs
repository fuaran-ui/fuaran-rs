//! The **teleport state bundle** (`§17`) — serialise a *running* application
//! (its tree, `Binding.State` values, an optional bounded op-history window, and
//! the op-chain head hash) into one self-identifying string small enough to ride
//! a URL fragment or QR code, and resume it exactly on any device. The string
//! form is `FT1.<base64url(deflate(canonical-JSON envelope))>`; a SHA-256 digest
//! over the whole envelope makes any tamper fail before a payload is decoded.
//!
//! The Node/TreeOp payloads inside the envelope ride the standard codec
//! unchanged (closures are `"<closure>"` sentinels, §4), so a teleported app is
//! interactive to precisely the bounded, gate-checked extent any decoded tree
//! is. Encode + decode assemble the digest preimage through the **one** canonical
//! renderer (re-parsing their own sub-documents), so verification cannot drift
//! from production.

pub mod base64url;
pub mod deflate;

use crate::canonical::{JVal, parse, render_canonical};
use crate::opstream::sha256_hex;
use crate::validator::{Severity, validate};
use crate::wire::{DecodeError, Node, TreeOp, decode_node, decode_op, encode_node, encode_op};

/// The self-identifying format tag — Fuaran Teleport, format 1.
pub const PREFIX: &str = "FT1.";
/// The envelope-shape version. An unrecognised version is refused by name.
pub const BUNDLE_VERSION: &str = "teleport@1";
/// The domain-separated digest preimage prefix (`§17.3`).
const DIGEST_PREFIX: &str = "fuaran-teleport:v1|";

/// A size gate on the *encoded* string, applied before any decompression work
/// (`§17.4` step 1). The URL-fragment practical budget (`§17.5`) — generous, so
/// a legitimate fragment bundle is never refused, while a pathological input is
/// rejected before it can drive the inflater.
pub const MAX_ENCODED: usize = 16_384;

/// A running application captured for teleport. On decode, `tree`/`history` are
/// the standard wire forms; `state` is the raw JSON value map (rule-12 discipline
/// — no `null`); `chain_head` anchors the bundle to a position in the op-stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub tree: Node,
    /// The `Binding.State` value map (state key → canonical JSON value).
    pub state: Vec<(String, JVal)>,
    /// A bounded window of recent TreeOps — provenance, not resume material.
    pub history: Vec<TreeOp>,
    /// The op-chain head hash at bundle time, when present.
    pub chain_head: Option<String>,
}

/// A typed, recoverable teleport failure (`§17.4` — never a panic).
#[derive(Debug, Clone, PartialEq)]
pub enum TeleportError {
    /// Input over the size gate, or inflate output over the cap (a bomb).
    Oversize,
    /// Prefix / base64url / inflate / UTF-8 failure.
    InvalidFormat(String),
    /// The inner text is not JSON.
    InvalidJson(String),
    /// The envelope object is structurally wrong (missing required field, …).
    InvalidEnvelope(String),
    /// The `bundle` version is not recognised.
    UnsupportedVersion(String),
    /// The recomputed digest does not match the envelope's.
    DigestMismatch,
    /// The `tree` payload failed the standard wire decode.
    TreeDecode(DecodeError),
    /// A `history` op failed the standard wire decode.
    HistoryDecode(DecodeError),
    /// The decoded tree has a node-identity defect (duplicate / empty NodeId) —
    /// state re-seat is keyed on stable identity, so the bundle is refused.
    TreeInvalid(String),
}

// ─── envelope assembly (shared canonical path) ───────────────────────────────

// Build the envelope object (optionally including the digest). Sub-documents are
// re-parsed so the whole envelope renders through the one canonical path.
fn envelope_obj(bundle: &Bundle, digest: Option<&str>) -> Result<JVal, TeleportError> {
    let tree =
        parse(&encode_node(&bundle.tree)).map_err(|e| TeleportError::InvalidJson(e.message))?;
    let mut fields: Vec<(String, JVal)> = vec![
        ("bundle".to_string(), JVal::Str(BUNDLE_VERSION.to_string())),
        ("tree".to_string(), tree),
    ];
    if !bundle.state.is_empty() {
        fields.push(("state".to_string(), JVal::Obj(bundle.state.clone())));
    }
    if !bundle.history.is_empty() {
        let mut ops = Vec::with_capacity(bundle.history.len());
        for op in &bundle.history {
            ops.push(parse(&encode_op(op)).map_err(|e| TeleportError::InvalidJson(e.message))?);
        }
        fields.push(("history".to_string(), JVal::Arr(ops)));
    }
    if let Some(head) = &bundle.chain_head {
        fields.push(("chainHead".to_string(), JVal::Str(head.clone())));
    }
    if let Some(d) = digest {
        fields.push(("digest".to_string(), JVal::Str(d.to_string())));
    }
    Ok(JVal::Obj(fields))
}

// The §17.3 digest over the canonical envelope *without* its digest field.
fn compute_digest(bundle: &Bundle) -> Result<String, TeleportError> {
    let obj = envelope_obj(bundle, None)?;
    let preimage = format!("{DIGEST_PREFIX}{}", render_canonical(&obj));
    Ok(sha256_hex(&preimage))
}

/// Encode a running-app [`Bundle`] to its `FT1.` string. Deterministic: the same
/// bundle always yields the same string (canonical render + deterministic
/// deflate + base64url).
pub fn encode(bundle: &Bundle) -> Result<String, TeleportError> {
    let digest = compute_digest(bundle)?;
    let obj = envelope_obj(bundle, Some(&digest))?;
    let json = render_canonical(&obj);
    let compressed = deflate::deflate(json.as_bytes());
    Ok(format!("{PREFIX}{}", base64url::encode(&compressed)))
}

/// Decode–validate–resume (`§17.4`): size gate → unwrap → envelope+version →
/// digest → standard wire decode → identity re-check. Every failure is typed.
pub fn decode(s: &str) -> Result<Bundle, TeleportError> {
    // 1. Size gate — before any decompression work.
    if s.len() > MAX_ENCODED {
        return Err(TeleportError::Oversize);
    }
    // 2. Unwrap — prefix, base64url, inflate, UTF-8, JSON.
    let body = s
        .strip_prefix(PREFIX)
        .ok_or_else(|| TeleportError::InvalidFormat("missing FT1. prefix".to_string()))?;
    let compressed = base64url::decode(body)
        .ok_or_else(|| TeleportError::InvalidFormat("invalid base64url".to_string()))?;
    let bytes = deflate::inflate(&compressed).map_err(|e| {
        if e.0.contains("cap") {
            TeleportError::Oversize
        } else {
            TeleportError::InvalidFormat(e.0.to_string())
        }
    })?;
    let text = String::from_utf8(bytes)
        .map_err(|_| TeleportError::InvalidFormat("inflated bytes are not UTF-8".to_string()))?;
    let env = parse(&text).map_err(|e| TeleportError::InvalidJson(e.message))?;

    // 3. Envelope shape + version.
    let JVal::Obj(_) = &env else {
        return Err(TeleportError::InvalidEnvelope(
            "envelope is not an object".to_string(),
        ));
    };
    match env.field("bundle") {
        Some(JVal::Str(v)) if v == BUNDLE_VERSION => {}
        Some(JVal::Str(v)) => return Err(TeleportError::UnsupportedVersion(v.clone())),
        _ => {
            return Err(TeleportError::InvalidEnvelope(
                "missing 'bundle' field".to_string(),
            ));
        }
    }
    let claimed_digest = match env.field("digest") {
        Some(JVal::Str(d)) => d.clone(),
        _ => {
            return Err(TeleportError::InvalidEnvelope(
                "missing 'digest' field".to_string(),
            ));
        }
    };
    let tree_json = match env.field("tree") {
        Some(t) => render_canonical(t),
        None => {
            return Err(TeleportError::InvalidEnvelope(
                "missing 'tree' field".to_string(),
            ));
        }
    };

    // 4. Digest verification — before decoding any payload. Rebuild the preimage
    //    through the same canonical renderer, dropping only the digest field.
    let JVal::Obj(fields) = &env else {
        unreachable!()
    };
    let without_digest: Vec<(String, JVal)> = fields
        .iter()
        .filter(|(k, _)| k != "digest")
        .cloned()
        .collect();
    let recomputed = sha256_hex(&format!(
        "{DIGEST_PREFIX}{}",
        render_canonical(&JVal::Obj(without_digest))
    ));
    if recomputed != claimed_digest {
        return Err(TeleportError::DigestMismatch);
    }

    // 5. Standard wire decode of tree + each history op.
    let tree = decode_node(&tree_json).map_err(TeleportError::TreeDecode)?;
    let history = match env.field("history") {
        Some(JVal::Arr(items)) => {
            let mut ops = Vec::with_capacity(items.len());
            for it in items {
                ops.push(decode_op(&render_canonical(it)).map_err(TeleportError::HistoryDecode)?);
            }
            ops
        }
        _ => Vec::new(),
    };
    let state = match env.field("state") {
        Some(JVal::Obj(kv)) => kv.clone(),
        _ => Vec::new(),
    };
    let chain_head = match env.field("chainHead") {
        Some(JVal::Str(h)) => Some(h.clone()),
        _ => None,
    };

    // 6. Pre-emit identity re-check — duplicate / empty NodeId refuses the bundle.
    for f in validate(&tree) {
        if f.severity == Severity::Error && (f.code == "EMPTY_NODE_ID" || f.code == "FUARAN001") {
            return Err(TeleportError::TreeInvalid(format!(
                "{}: {}",
                f.code, f.node_id
            )));
        }
    }

    Ok(Bundle {
        tree,
        state,
        history,
        chain_head,
    })
}
