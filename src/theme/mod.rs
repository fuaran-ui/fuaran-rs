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
    MotionBudget, RoleBinding, TONES, ThemeManifest, decode, merge, of_json,
    project_from_css_custom_properties, project_from_dtcg, project_from_fuaran_tone_vars,
    scan_css_blocks, tone_contrast, tone_of_string, tone_rgba,
};
