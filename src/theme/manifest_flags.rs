//! Manifest-aware flag derivation — the render-time enforcement of a declared
//! aesthetic-semantic budget, composing the resolved fills (the manifest-free
//! observation of [`crate::theme::flags`]) with a declared
//! [`ThemeManifest`]. Deterministic; no vision model in the verify path.
//!
//! Two surfaces:
//!
//! - [`per_node_flags`] — the per-node fidelity checks (token resolution,
//!   palette membership, the declared contrast floor). An observer with a
//!   manifest wired appends these to each observation.
//! - [`verify_usage_budgets`] — the tree-level area-weighted colour-budget check
//!   (the 60-30-10 enforcement). The caller joins each observation with its
//!   rendered area, because area is a layout fact this tier never measures.
//!
//! **Custom-subtree policy: EXEMPT.** Every per-node check fires only for TONED
//! nodes, so untoned content (a `Custom` host subtree, a domain SVG) is exempt
//! by construction rather than by a carve-out — an absent tone is an absent
//! fact, and an absent fact never fires a flag.

use std::collections::HashMap;

use crate::theme::contrast::Rgba;
use crate::theme::flags::{StyleFlag, StyleObservation, same_rgb, try_parse_hex};
use crate::theme::manifest::{InvariantKind, ManifestToken, ThemeManifest, tone_of_string};

/// The `rgb(r, g, b)` spelling an off-palette finding carries. Channels are
/// rounded half-to-even, matching the sibling hosts' `round()` /
/// `math.RoundToEven`.
fn rgb_string(c: Rgba) -> String {
    format!(
        "rgb({}, {}, {})",
        c.r.round_ties_even() as i64,
        c.g.round_ties_even() as i64,
        c.b.round_ties_even() as i64
    )
}

/// Every `color` token that projects to a resolved colour, paired with its
/// token name. A token whose value is not hex (`oklch(…)`, a reference) is
/// simply absent from the membership set — it cannot be compared, so it does
/// not silently widen the palette.
fn palette_rgba(manifest: &ThemeManifest) -> Vec<(Rgba, &str)> {
    manifest
        .tokens
        .iter()
        .filter(|t| t.token_type == "color")
        .filter_map(|t| try_parse_hex(&t.value).map(|c| (c, t.name.as_str())))
        .collect()
}

/// Resolve an emitted slot: a canonical tone through the tone bindings, anything
/// else as a broader named role.
fn resolve_slot<'a>(manifest: &'a ThemeManifest, slot: &str) -> Option<&'a ManifestToken> {
    match tone_of_string(slot) {
        Some(tone) => manifest.resolve_role(tone),
        None => manifest.resolve_named_role(slot),
    }
}

/// Per-node manifest-aware flags for one observation. Empty for an untoned node.
///
/// The two fill checks are mutually exclusive by construction: a slot the
/// manifest cannot resolve reports [`StyleFlag::TokenResolutionFailed`] and
/// nothing more, because asking whether an unresolvable slot's fill is on the
/// palette is a question about a token that does not exist. The declared
/// contrast floor is checked independently of both — a role can resolve
/// perfectly and still be used below its own floor.
pub fn per_node_flags(manifest: &ThemeManifest, obs: &StyleObservation) -> Vec<StyleFlag> {
    let Some(slot) = obs.emitted_tone.as_deref() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    match resolve_slot(manifest, slot) {
        None => out.push(StyleFlag::TokenResolutionFailed {
            slot: slot.to_string(),
        }),
        Some(_) => {
            let on_palette = palette_rgba(manifest)
                .iter()
                .any(|(c, _)| same_rgb(*c, obs.effective_background));
            if !on_palette {
                out.push(StyleFlag::OffPaletteColour {
                    value: rgb_string(obs.effective_background),
                });
            }
        }
    }

    // A match guard rather than a let-chain: the crate declares MSRV 1.85 in
    // `Cargo.toml`, and `if let … && …` needs 1.88.
    out.extend(
        manifest
            .invariants
            .iter()
            .filter_map(|inv| match &inv.kind {
                InvariantKind::ContrastFloor { role, min_ratio }
                    if role == slot && obs.contrast_ratio < *min_ratio =>
                {
                    Some(StyleFlag::ContrastBelowDeclaredFloor {
                        role: role.clone(),
                        ratio: obs.contrast_ratio,
                        floor: *min_ratio,
                    })
                }
                _ => None,
            }),
    );
    out
}

/// One observation paired with its rendered area (px²) for the tree-level
/// usage-budget check.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeArea {
    /// The node's observation.
    pub obs: StyleObservation,
    /// Its rendered area in px². Area is a layout fact, supplied by whichever
    /// tier measured it.
    pub area: f64,
}

/// Tree-level area-weighted usage-budget verification — the 60-30-10
/// enforcement.
///
/// Returns empty when no area is available at all: a budget is a statement
/// about SHARE, and a share of nothing is not a breach. A node whose fill
/// matches no palette token contributes to the total but to no token's share,
/// which is what makes an off-palette fill dilute every declared budget rather
/// than vanish from the arithmetic.
pub fn verify_usage_budgets(manifest: &ThemeManifest, nodes: &[NodeArea]) -> Vec<StyleFlag> {
    let total_area: f64 = nodes.iter().map(|n| n.area).sum();
    if total_area <= 0.0 {
        return Vec::new();
    }

    let palette = palette_rgba(manifest);
    let mut area_by_token: HashMap<&str, f64> = HashMap::new();
    for node in nodes {
        // First match wins, and the tie-break between two same-valued tokens is
        // RULED (Phase 1727): attribution iterates in canonical token-path
        // order — segment by segment, a shorter prefix first, each segment by
        // Unicode code point; document order plays no part. The rule is stated
        // once, in fuaran-dotnet's `docs/THEME-BRIDGE-GUIDE.md` under "Palette
        // attribution order", and pinned by the corpus's
        // `style-observer/budget-same-valued-tokens-*` vectors. This host meets
        // it through its DECODER, whose DTCG walk visits every group's keys in
        // sorted order (`manifest.rs`, `sorted_keys`), so `palette` already
        // arrives in that order; `tests/theme.rs` keeps the case red if either
        // half stops holding.
        if let Some((_, name)) = palette
            .iter()
            .find(|(c, _)| same_rgb(*c, node.obs.effective_background))
        {
            *area_by_token.entry(name).or_insert(0.0) += node.area;
        }
    }

    let mut out = Vec::new();
    for inv in &manifest.invariants {
        let InvariantKind::UsageBudget {
            token,
            target_pct,
            tolerance_pct,
        } = &inv.kind
        else {
            continue;
        };
        let token_area = area_by_token.get(token.as_str()).copied().unwrap_or(0.0);
        let observed_pct = 100.0 * token_area / total_area;
        if (observed_pct - target_pct).abs() > *tolerance_pct {
            out.push(StyleFlag::UsageBudgetExceeded {
                token: token.clone(),
                declared_pct: *target_pct,
                observed_pct,
            });
        }
    }
    out
}
