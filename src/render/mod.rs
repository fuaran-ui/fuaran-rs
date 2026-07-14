//! The emission tier: the pure-string server-HTML renderer ([`server`] —
//! parity-locked `fuaran-*` class vocabulary, inert interactivity, islands
//! partial-hydration emission), the deterministic cross-host markdown renderer
//! ([`markdown::to_html`]), and the render-time sanitisation floor
//! ([`sanitize`]). The reference stylesheet ships as a byte-copy at
//! `css/fuaran.css`; the host serves it alongside the emitted body fragment.

pub mod bindings;
pub mod chart_lowering;
pub mod class_names;
pub mod email;
pub mod html;
pub mod markdown;
pub mod sanitize;
pub mod server;

pub use bindings::BindingSources;
pub use server::{render_hydratable, render_to_html, render_with_islands};
