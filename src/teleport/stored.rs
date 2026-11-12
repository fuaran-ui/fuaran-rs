//! The stored-block teleport encoder (Phase 2452) — a §17 bundle written with no
//! compressor, `no_std` + `alloc`, for a device.
//!
//! RFC 1951 lets a deflate stream be a run of STORED blocks (block type 00: a
//! header bit pair, then the length, its complement and the bytes verbatim), so a
//! valid `FT1.` bundle needs no Huffman coder and no LZ77 window — only framing.
//! Every conformant inflater accepts it, this host's included
//! ([`super::deflate::inflate`]), so a stored bundle decodes on every host exactly
//! as a compressed one does; it is larger, which a panel's few hundred bytes can
//! afford.
//!
//! The envelope is assembled through the Core spine's canonical writer from
//! documents the caller already holds as canonical text (the emitter's constant
//! panel document, its op documents and canonical state values), and the §17.3
//! digest is taken over that canonical envelope without its digest member — the
//! same preimage [`super::decode`] recomputes.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use fuaran_core::json::{render_array, render_object};
use fuaran_core::sha256::sha256_hex;

use super::{BUNDLE_VERSION, DIGEST_PREFIX, MAX_ENCODED, PREFIX, base64url};

/// The most bytes one stored block carries (its length field is 16 bits).
pub const STORED_BLOCK_MAX: usize = 65_535;

/// A raw RFC 1951 stream of stored blocks over `input`: every block but the last
/// carries [`STORED_BLOCK_MAX`] bytes, and an empty input is one empty final
/// block. Deterministic, and byte-aligned by construction (each header is a whole
/// byte: `BFINAL`, `BTYPE = 00`, five padding bits).
pub fn deflate_stored(input: &[u8]) -> Vec<u8> {
    let blocks = input.len().div_ceil(STORED_BLOCK_MAX).max(1);
    let mut out = Vec::with_capacity(input.len() + blocks * 5);
    let mut chunks = input.chunks(STORED_BLOCK_MAX).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        return out;
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        let len = chunk.len() as u16;
        out.push(u8::from(last));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// What a device bundles: documents it holds as canonical text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope<'a> {
    /// The tree: a canonical node document.
    pub tree: &'a str,
    /// The `Binding.State` values: key and canonical JSON value.
    pub state: &'a [(&'a str, &'a str)],
    /// A bounded window of recent op documents (provenance, §17.2).
    pub history: &'a [&'a str],
    /// The op-chain head at bundle time.
    pub chain_head: Option<&'a str>,
}

/// Why a bundle was not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredError {
    /// The encoded string would pass [`MAX_ENCODED`], so every decoder would
    /// refuse it before reading it.
    Oversize { encoded: usize },
}

fn envelope_json(env: &Envelope<'_>, digest: Option<&str>) -> String {
    let quoted = |s: &str| fuaran_core::json::escape_string(s);
    let mut fields: Vec<(String, String)> = Vec::with_capacity(6);
    fields.push(("bundle".to_string(), quoted(BUNDLE_VERSION)));
    fields.push(("tree".to_string(), env.tree.to_string()));
    if !env.state.is_empty() {
        let mut state: Vec<(String, String)> = env
            .state
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        fields.push(("state".to_string(), render_object(&mut state)));
    }
    if !env.history.is_empty() {
        let ops: Vec<String> = env.history.iter().map(|o| (*o).to_string()).collect();
        fields.push(("history".to_string(), render_array(&ops)));
    }
    if let Some(head) = env.chain_head {
        fields.push(("chainHead".to_string(), quoted(head)));
    }
    if let Some(d) = digest {
        fields.push(("digest".to_string(), quoted(d)));
    }
    render_object(&mut fields)
}

/// The §17.3 digest over the canonical envelope without its digest member.
pub fn digest(env: &Envelope<'_>) -> String {
    let mut preimage = String::from(DIGEST_PREFIX);
    preimage.push_str(&envelope_json(env, None));
    sha256_hex(&preimage)
}

/// Encode a bundle to its `FT1.` string over stored blocks.
pub fn encode(env: &Envelope<'_>) -> Result<String, StoredError> {
    let d = digest(env);
    let json = envelope_json(env, Some(&d));
    let framed = base64url::encode(&deflate_stored(json.as_bytes()));
    let encoded = PREFIX.len() + framed.len();
    if encoded > MAX_ENCODED {
        return Err(StoredError::Oversize { encoded });
    }
    let mut out = String::with_capacity(encoded);
    out.push_str(PREFIX);
    out.push_str(&framed);
    Ok(out)
}
