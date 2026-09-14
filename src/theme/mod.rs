//! Theme-observable accessibility — WCAG contrast derivation over resolved
//! colours. The host reads legibility from the resolved foreground/background
//! (alpha compositing → effective background → relative luminance → contrast
//! ratio → AA/AAA verdict), the same structural sense the Infinite Skins
//! contrast auditor and Kintsugi surface without inspecting pixels.

pub mod contrast;
pub mod manifest;

pub use contrast::{
    AA_LARGE, AA_NORMAL, AAA_LARGE, AAA_NORMAL, ContrastVerdict, Rgba, composite, contrast_ratio,
    effective_background, foreground_contrast, relative_luminance, verdict,
};
pub use manifest::{
    DEFAULT_WEIGHT, Invariant, InvariantKind, ManifestMeta, ManifestRole, ManifestToken,
    MotionBudget, RoleBinding, TONES, ThemeManifest, decode, encode, merge, of_json,
    project_from_css_custom_properties, project_from_dtcg, project_from_fuaran_tone_vars,
    scan_css_blocks, to_json, tone_contrast, tone_of_string, tone_rgba,
};

// The style-observer pure tier — resolved-style FACTS in, typed `StyleFlag`s
// out, byte-identical to the sibling hosts. "Style is read, not looked at": no
// DOM and no pixels reach the derivation, and the live `getComputedStyle`
// read-back stays outside the crate (see `observer`).
pub mod flags;
pub mod manifest_flags;
pub mod observer;

pub use flags::{
    FontRole, StyleFlag, StyleInput, StyleObservation, StyleObserverOptions, baseline_style_input,
    contrast as style_contrast, derive_style_flags, encode_rgba, encode_style_flag,
    encode_style_observation, flags_equal, font_role_of, resolved_background, resolved_foreground,
    same_rgb, to_style_observation, try_parse_hex,
};
pub use manifest_flags::{NodeArea, per_node_flags, verify_usage_budgets};
pub use observer::{InMemoryStyleObserver, SubscriptionId};
