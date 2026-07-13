//! The emission tier: the deterministic cross-host markdown renderer
//! ([`markdown::to_html`]) and the render-time sanitisation floor
//! ([`sanitize`]). The server-HTML walk + islands emission build on these.

pub mod markdown;
pub mod sanitize;
