//! The §15 wire versioning envelope. An artefact may be wrapped as
//! `{"$payload":<Node|TreeOp>,"$profile":"<name>@<major>.<minor>"}`. A consumer
//! negotiates the authored profile against its own `core@1.0`:
//!
//! - [`Negotiation::Current`] (same name+major, authored minor ≤ ours) — decode
//!   fully;
//! - [`Negotiation::Behind`] (same name+major, authored minor > ours) — tolerate:
//!   an unknown kind becomes a transport-only preserved payload whose verbatim
//!   bytes re-encode identically (must-ignore-but-preserve);
//! - [`Negotiation::Foreign`] (different name, or different major) — hard-refuse
//!   with [`EnvelopeErrorCode::ForeignProfile`] — never silently mis-decode.
//!
//! The transport-only preserve is decode-only: there is no encoder entry point
//! that mints an unknown kind, so the closed authoring surface stays intact.
//! Mirrors the shipped `fuaran-go` / `fuaran-ts` shapes case-for-case; the three
//! negotiation outcomes are a native `enum` with an exhaustive `match`.

use crate::canonical::{JVal, escape_string, parse, render_canonical, render_object};
use crate::wire::{DecodeError, DecodeErrorCode, Node, decode_node, encode_node};

/// The profile this host implements.
pub const HOST_PROFILE: &str = "core@1.0";

/// The outcome of comparing an authored profile against the host — the closed
/// three-case §15 negotiation DU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Negotiation {
    /// The host is at or ahead of the authored profile (same name+major, minor ≤ ours).
    Current,
    /// The host is behind (authored minor ahead) — tolerate unknown kinds by preserving.
    Behind,
    /// An incompatible namespace / major — refuse.
    Foreign,
}

impl Negotiation {
    /// The wire-stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Negotiation::Current => "Current",
            Negotiation::Behind => "Behind",
            Negotiation::Foreign => "Foreign",
        }
    }
}

/// The error code an envelope decode surfaces: either one of the core six wire
/// codes (reused for structural faults) or the §15-only `FOREIGN_PROFILE`
/// extension (kept out of the core six, like the reference hosts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeErrorCode {
    /// One of the six canonical wire codes.
    Core(DecodeErrorCode),
    /// `FOREIGN_PROFILE` — a different namespace or major version, hard-refused.
    ForeignProfile,
}

impl EnvelopeErrorCode {
    /// The wire-stable string every host emits verbatim.
    pub fn as_str(self) -> &'static str {
        match self {
            EnvelopeErrorCode::Core(c) => c.as_str(),
            EnvelopeErrorCode::ForeignProfile => "FOREIGN_PROFILE",
        }
    }
}

/// A structured, recoverable envelope-decode error — the same `{code, path,
/// message}` envelope the codec floor uses, with the §15 code superset.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvelopeError {
    pub code: EnvelopeErrorCode,
    pub path: String,
    pub message: String,
}

impl EnvelopeError {
    fn new(code: EnvelopeErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        EnvelopeError {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.code.as_str(),
            self.path,
            self.message
        )
    }
}

impl std::error::Error for EnvelopeError {}

/// A decoded §15 payload: a fully-decoded [`Node`] (Current, or a known kind on
/// a Behind envelope) or the verbatim-preserved value (a Behind unknown kind,
/// must-ignore-but-preserve).
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// Boxed to keep the enum small — a decoded `Node` dwarfs the preserved `JVal`.
    Node(Box<Node>),
    Preserved(JVal),
}

/// A decoded §15 versioned artefact: the payload, the authored profile, and the
/// negotiation outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub payload: Payload,
    pub profile: String,
    pub negotiation: Negotiation,
}

fn parse_profile(p: &str) -> Option<(&str, i64, i64)> {
    let at = p.find('@')?;
    if at == 0 {
        return None;
    }
    let name = &p[..at];
    let ver = &p[at + 1..];
    let dot = ver.find('.')?;
    let major: i64 = ver[..dot].parse().ok()?;
    let minor: i64 = ver[dot + 1..].parse().ok()?;
    Some((name, major, minor))
}

/// Compares an authored profile against the host's `core@1.0`.
pub fn negotiate(profile: &str) -> Negotiation {
    match parse_profile(profile) {
        Some(("core", 1, minor)) => {
            if minor <= 0 {
                Negotiation::Current
            } else {
                Negotiation::Behind
            }
        }
        _ => Negotiation::Foreign,
    }
}

fn reroot_payload_err(e: DecodeError) -> EnvelopeError {
    // "$…" → "$.$payload…" (the payload's own `$` becomes `$.$payload`).
    let rest = &e.path[1..];
    EnvelopeError::new(
        EnvelopeErrorCode::Core(e.code),
        format!("$.$payload{rest}"),
        e.message,
    )
}

/// Decode a §15 versioned artefact: negotiate the profile, then decode the
/// payload (Current → strict node decode; Behind → tolerate an unknown kind by
/// preserving it verbatim). A Foreign profile is refused with `FOREIGN_PROFILE`
/// at `$.$profile`.
pub fn decode_envelope(canonical_json: &str) -> Result<Envelope, EnvelopeError> {
    let raw = parse(canonical_json).map_err(|e| {
        EnvelopeError::new(
            EnvelopeErrorCode::Core(DecodeErrorCode::InvalidJson),
            "$",
            format!("input is not valid JSON: {}", e.message),
        )
    })?;
    let JVal::Obj(_) = raw else {
        return Err(EnvelopeError::new(
            EnvelopeErrorCode::Core(DecodeErrorCode::WrongType),
            "$",
            "expected an object at $",
        ));
    };
    let profile = match raw.field("$profile") {
        None => {
            return Err(EnvelopeError::new(
                EnvelopeErrorCode::Core(DecodeErrorCode::MissingField),
                "$.$profile",
                "missing required field '$profile'",
            ));
        }
        Some(JVal::Str(s)) => s.clone(),
        Some(_) => {
            return Err(EnvelopeError::new(
                EnvelopeErrorCode::Core(DecodeErrorCode::WrongType),
                "$.$profile",
                "$profile must be a string",
            ));
        }
    };
    let Some(payload_raw) = raw.field("$payload") else {
        return Err(EnvelopeError::new(
            EnvelopeErrorCode::Core(DecodeErrorCode::MissingField),
            "$.$payload",
            "missing required field '$payload'",
        ));
    };

    let negotiation = negotiate(&profile);
    if negotiation == Negotiation::Foreign {
        return Err(EnvelopeError::new(
            EnvelopeErrorCode::ForeignProfile,
            "$.$profile",
            format!(
                "foreign profile '{profile}' — a different namespace or major version, hard-refused"
            ),
        ));
    }

    // Decode the payload through the certified node codec (the payload is
    // re-rendered canonically first so the shared `$`-rooted decoder applies;
    // its faults are re-rooted under `$.$payload`).
    let payload = match decode_node(&render_canonical(payload_raw)) {
        Ok(node) => Payload::Node(Box::new(node)),
        Err(e)
            if negotiation == Negotiation::Behind && e.code == DecodeErrorCode::WrongNodeKind =>
        {
            // Must-ignore-but-preserve: an unknown kind a behind consumer meets
            // is preserved verbatim, so re-encoding reproduces the producer's bytes.
            Payload::Preserved(payload_raw.clone())
        }
        Err(e) => return Err(reroot_payload_err(e)),
    };

    Ok(Envelope {
        payload,
        profile,
        negotiation,
    })
}

/// Re-encode an envelope to canonical wire JSON:
/// `{"$payload":<payload>,"$profile":<profile>}` (`$payload` sorts before
/// `$profile` under the Ordinal key order).
pub fn encode_envelope(env: &Envelope) -> String {
    let payload_json = match &env.payload {
        Payload::Node(n) => encode_node(n),
        Payload::Preserved(v) => render_canonical(v),
    };
    let mut fields = vec![
        ("$payload".to_string(), payload_json),
        ("$profile".to_string(), escape_string(&env.profile)),
    ];
    render_object(&mut fields)
}
