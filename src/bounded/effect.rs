//! The client-effect vocabulary — six closed arms, and the one envelope on this
//! wire that is **not** canonical.
//!
//! A program reaches a rendering surface only through these arms. The
//! specification pins them *as they are emitted*, because their wire form
//! predates the specification that now governs them: the discriminator member is
//! `kind` rather than `$type`, members are ordered by **declaration** rather than
//! Ordinally (so `Download` emits `url` before `name`), and the three common
//! control characters take their short escapes (backslash-n, backslash-r,
//! backslash-t) rather than as six-character backslash-u sequences.
//!
//! **Do not "correct" any of that here.** Re-spelling these bytes canonically is
//! a breaking change to a live contract and is recorded as a migration, not as a
//! tidy-up. The encoder below is therefore deliberately separate from
//! [`crate::canonical`]'s renderer rather than a flag on it: keeping the two
//! apart is what makes encoding an effect canonically fail to compile rather
//! than silently erase the exception.
//!
//! **What conformance fixes, and what it leaves alone.** A host must recognise
//! every arm, derive its capability, and refuse an arm outside the vocabulary.
//! It is *not* obliged to perform any of them in any particular way — a surface
//! with no address bar has nothing for `PushState` to update — so what a claim
//! over this vocabulary fixes is that the arm reached is the arm the program
//! named, with the values it named, in the order it named them. Refusing is
//! conformant; refusing **silently** is not, which is why a declined effect
//! leaves a [`Denial`] rather than nothing at all.

use std::collections::BTreeSet;

/// A client-only effect a bounded program reached. The host performs it (or
/// declines it audibly — see [`Denial`]); this type is what the program *asked
/// for*, never what the world then did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEffect {
    /// A full navigation.
    Navigate { route: String },
    /// Update the address without a reload.
    PushState { route: String },
    /// Write to the clipboard.
    WriteToClipboard { text: String },
    /// Move focus to the addressed node.
    Focus { node_id: String },
    /// Trigger a download with a suggested filename.
    Download { url: String, name: String },
    /// Read the body of a selected file; `encoding` is one of `Text`, `Base64`,
    /// `DataUrl`.
    ReadFileBody { node_id: String, encoding: String },
}

/// Every arm of the closed vocabulary, in declaration order. A host's coverage
/// declaration is a subset of this; an arm outside it is not expressible.
pub const CLIENT_EFFECT_ARMS: &[&str] = &[
    "Navigate",
    "PushState",
    "WriteToClipboard",
    "Focus",
    "Download",
    "ReadFileBody",
];

impl ClientEffect {
    /// The arm's discriminator — and, for this vocabulary, its **capability**
    /// verbatim. The capability is derived here and never carried on a wire: a
    /// wire-carried capability would be a second spelling of a fact the host
    /// computes, under the control of the untrusted side.
    pub fn capability(&self) -> &'static str {
        match self {
            ClientEffect::Navigate { .. } => "Navigate",
            ClientEffect::PushState { .. } => "PushState",
            ClientEffect::WriteToClipboard { .. } => "WriteToClipboard",
            ClientEffect::Focus { .. } => "Focus",
            ClientEffect::Download { .. } => "Download",
            ClientEffect::ReadFileBody { .. } => "ReadFileBody",
        }
    }

    /// Encode the effect in its as-emitted envelope: `kind` first, then the
    /// arm's members in **declaration** order.
    pub fn encode(&self) -> String {
        match self {
            ClientEffect::Navigate { route } => {
                format!("{{\"kind\":\"Navigate\",\"route\":{}}}", quoted(route))
            }
            ClientEffect::PushState { route } => {
                format!("{{\"kind\":\"PushState\",\"route\":{}}}", quoted(route))
            }
            ClientEffect::WriteToClipboard { text } => {
                format!(
                    "{{\"kind\":\"WriteToClipboard\",\"text\":{}}}",
                    quoted(text)
                )
            }
            ClientEffect::Focus { node_id } => {
                format!("{{\"kind\":\"Focus\",\"nodeId\":{}}}", quoted(node_id))
            }
            ClientEffect::Download { url, name } => format!(
                "{{\"kind\":\"Download\",\"url\":{},\"name\":{}}}",
                quoted(url),
                quoted(name)
            ),
            ClientEffect::ReadFileBody { node_id, encoding } => format!(
                "{{\"kind\":\"ReadFileBody\",\"nodeId\":{},\"encoding\":{}}}",
                quoted(node_id),
                quoted(encoding)
            ),
        }
    }
}

/// Why an effect did not run. A denial carries the **capability** and nothing
/// else — never the route, the text, the node id or the filename, every one of
/// which comes off an untrusted wire.
///
/// The two arms are kept apart because they say different things and only one is
/// resolvable by changing policy: `Unregistered` says *this host does not have
/// that capability*, `GateRefused` says *this host has it and refused this use
/// of it*. Collapsing them loses the more useful fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    Unregistered { capability: String },
    GateRefused { capability: String },
}

impl Denial {
    /// The denial's canonical encoding — `$type` first, then `capability`, which
    /// is Ordinal order as well as declaration order.
    pub fn encode(&self) -> String {
        match self {
            Denial::Unregistered { capability } => format!(
                "{{\"$type\":\"Unregistered\",\"capability\":{}}}",
                quoted(capability)
            ),
            Denial::GateRefused { capability } => format!(
                "{{\"$type\":\"GateRefused\",\"capability\":{}}}",
                quoted(capability)
            ),
        }
    }
}

/// A host's declaration of which client-effect arms it serves, and whether a
/// served arm is permitted for a given use.
///
/// **Two independent facts, and a conformant host keeps them so:** an
/// unregistered arm is unreachable however permissive the policy, and a
/// registered one is still subject to the gate. Both default to deny — a host
/// that has enabled nothing declines everything, audibly.
pub struct EffectPolicy {
    registered: BTreeSet<String>,
    gate: Box<dyn Fn(&ClientEffect) -> bool>,
}

impl std::fmt::Debug for EffectPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectPolicy")
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

impl Default for EffectPolicy {
    /// Nothing registered, and a gate that permits nothing — the honest default
    /// for a host that has declared no capability at all.
    fn default() -> Self {
        EffectPolicy {
            registered: BTreeSet::new(),
            gate: Box::new(|_| false),
        }
    }
}

impl EffectPolicy {
    /// Register a performer for one arm (builder-style). An unrecognised arm
    /// name is refused rather than stored: a host cannot widen a closed
    /// vocabulary by naming something new, and silently keeping the string would
    /// let it believe it had.
    pub fn register(mut self, arm: &str) -> Result<Self, String> {
        if !CLIENT_EFFECT_ARMS.contains(&arm) {
            return Err(format!(
                "'{arm}' is not an arm of the client-effect vocabulary; a host extends its reach by \
                 registering a performer for a declared arm, never by naming a new one"
            ));
        }
        self.registered.insert(arm.to_string());
        Ok(self)
    }

    /// **The named opt-in back to an allow-everything host**: every arm
    /// registered, every use permitted. Named rather than implied, so "this host
    /// serves the whole vocabulary" is a statement in the code.
    pub fn permissive() -> Self {
        EffectPolicy {
            registered: CLIENT_EFFECT_ARMS.iter().map(|s| s.to_string()).collect(),
            gate: Box::new(|_| true),
        }
    }

    /// Replace the policy gate (builder-style). Registration is unaffected — the
    /// two facts stay independent.
    pub fn with_gate(mut self, gate: impl Fn(&ClientEffect) -> bool + 'static) -> Self {
        self.gate = Box::new(gate);
        self
    }

    /// The arms this host has registered — its declared coverage.
    pub fn registered(&self) -> impl Iterator<Item = &str> {
        self.registered.iter().map(|s| s.as_str())
    }

    /// Decide one effect. `None` permits it; `Some(denial)` declines it and says
    /// which capability was declined — the reporting half of "refusal is
    /// conformant, silence is not".
    pub fn decide(&self, effect: &ClientEffect) -> Option<Denial> {
        let capability = effect.capability().to_string();
        if !self.registered.contains(&capability) {
            return Some(Denial::Unregistered { capability });
        }
        if !(self.gate)(effect) {
            return Some(Denial::GateRefused { capability });
        }
        None
    }
}

/// String escaping for this family's envelope: `"` and `\`, the three common
/// control characters as their short escapes, and every remaining control
/// character as `\u00xx` with lower-case hex. Nothing else is escaped — a
/// non-ASCII character passes through as its literal UTF-8 sequence.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_emits_url_before_name_which_is_not_ordinal_order() {
        let encoded = ClientEffect::Download {
            url: "https://example.invalid/report.csv".into(),
            name: "report.csv".into(),
        }
        .encode();
        assert_eq!(
            encoded,
            "{\"kind\":\"Download\",\"url\":\"https://example.invalid/report.csv\",\"name\":\"report.csv\"}"
        );
        // The whole point of the exception: Ordinal order would put `name` first.
        assert!(encoded.find("\"url\"") < encoded.find("\"name\""));
    }

    #[test]
    fn the_three_common_controls_take_short_escapes_and_the_rest_do_not() {
        let encoded = ClientEffect::WriteToClipboard {
            text: "a\nb\tc\u{1}d".into(),
        }
        .encode();
        assert_eq!(
            encoded,
            "{\"kind\":\"WriteToClipboard\",\"text\":\"a\\nb\\tc\\u0001d\"}"
        );
    }

    #[test]
    fn both_default_to_deny_and_the_two_facts_are_independent() {
        let effect = ClientEffect::Navigate {
            route: "/next".into(),
        };

        // Nothing registered, nothing permitted.
        assert_eq!(
            EffectPolicy::default().decide(&effect),
            Some(Denial::Unregistered {
                capability: "Navigate".into()
            })
        );

        // Registered but gated: a different fact, and a different denial.
        let gated = EffectPolicy::default()
            .register("Navigate")
            .expect("Navigate is a declared arm");
        assert_eq!(
            gated.decide(&effect),
            Some(Denial::GateRefused {
                capability: "Navigate".into()
            })
        );

        // Permissive registers and permits; the gate alone cannot reach an
        // unregistered arm.
        assert_eq!(EffectPolicy::permissive().decide(&effect), None);
        let permissive_gate_only = EffectPolicy::default().with_gate(|_| true);
        assert_eq!(
            permissive_gate_only.decide(&effect),
            Some(Denial::Unregistered {
                capability: "Navigate".into()
            })
        );
    }

    #[test]
    fn a_host_cannot_widen_the_closed_vocabulary_by_registering_a_new_arm() {
        assert!(EffectPolicy::default().register("LaunchMissiles").is_err());
    }
}
