//! The emission tier: the pure-string server-HTML renderer ([`server`] —
//! parity-locked `fuaran-*` class vocabulary, inert interactivity, islands
//! partial-hydration emission), the deterministic cross-host markdown renderer
//! ([`markdown::to_html`]), the render-time sanitisation floor
//! ([`sanitize`]), and the destination policy layered over it
//! ([`egress`] — `WIRE_FORMAT.md` §14.1: where a destination may GO, as
//! against what a URL may BE). The reference stylesheet ships as a byte-copy at
//! `css/fuaran.css`; the host serves it alongside the emitted body fragment.

pub mod bindings;
pub mod chart_lowering;
pub mod class_names;
pub mod egress;
pub mod email;
pub mod html;
pub mod markdown;
pub mod project;
pub mod sanitize;
pub mod seeds;
pub mod server;
pub mod sparkline_lowering;

pub use bindings::BindingSources;
pub use project::project_resolved;
pub use seeds::{HOST_RESERVED_STATE_PREFIX, collect_state_seeds, with_state_seeds};
pub use server::{
    render_hydratable, render_hydratable_with_egress, render_to_html, render_to_html_with_egress,
    render_with_islands, render_with_islands_with_egress,
};
