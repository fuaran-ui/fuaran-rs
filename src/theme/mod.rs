//! Theme-observable accessibility — WCAG contrast derivation over resolved
//! colours. The host reads legibility from the resolved foreground/background
//! (alpha compositing → effective background → relative luminance → contrast
//! ratio → AA/AAA verdict), the same structural sense the Infinite Skins
//! contrast auditor and Kintsugi surface without inspecting pixels.

pub mod contrast;

pub use contrast::{
    AA_LARGE, AA_NORMAL, AAA_LARGE, AAA_NORMAL, ContrastVerdict, Rgba, composite, contrast_ratio,
    effective_background, foreground_contrast, relative_luminance, verdict,
};
