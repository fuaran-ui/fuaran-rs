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

/// Why an effect did not run. A denial carries the **capability**, and — where
/// the refusal's ground was a destination — that destination's **origin**. Never
/// the route, the text, the node id or the filename, every one of which comes
/// off an untrusted wire; and never the URL, only the host or the class of
/// destination where there is no host.
///
/// The two arms are kept apart because they say different things and only one is
/// resolvable by changing policy: `Unregistered` says *this host does not have
/// that capability*, `GateRefused` says *this host has it and refused this use
/// of it*. Collapsing them loses the more useful fact.
///
/// **`origin` is a member rather than a third arm, deliberately.** A refusal on
/// the ground of the destination is still the host having the capability and
/// refusing this use of it — `GateRefused`'s own sentence — and what was missing
/// was *which use*. A third arm would have been a breaking change to a closed
/// vocabulary for a fact one of its existing arms already carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    Unregistered {
        capability: String,
    },
    GateRefused {
        capability: String,
        /// Present only where the ground of the refusal was the destination the
        /// effect named. Its ABSENCE is not a claim that the destination was
        /// permitted — it is the statement that no destination was consulted,
        /// which the ordering in [`EffectPolicy::decide`] is what makes true.
        origin: Option<String>,
    },
}

impl Denial {
    /// The denial's canonical encoding — `$type`, `capability`, then `origin`
    /// where there is one, which is Ordinal order as well as declaration order
    /// (`$` U+0024 < `c` < `o`).
    pub fn encode(&self) -> String {
        match self {
            Denial::Unregistered { capability } => format!(
                "{{\"$type\":\"Unregistered\",\"capability\":{}}}",
                quoted(capability)
            ),
            Denial::GateRefused {
                capability,
                origin: None,
            } => format!(
                "{{\"$type\":\"GateRefused\",\"capability\":{}}}",
                quoted(capability)
            ),
            Denial::GateRefused {
                capability,
                origin: Some(origin),
            } => format!(
                "{{\"$type\":\"GateRefused\",\"capability\":{},\"origin\":{}}}",
                quoted(capability),
                quoted(origin)
            ),
        }
    }

    /// Read a denial back from the three positions the specification declares.
    ///
    /// **Reading rather than diffing bytes is the point.** A harness holding a
    /// recorded denial's bytes beside its own would compare two strings and
    /// assert nothing about whether this host RECOGNISES the vocabulary — so an
    /// arm outside it, or an `origin` on an arm that never consulted a
    /// destination, fails here rather than passing through as an opaque value
    /// somebody expected to be honoured.
    pub fn from_wire(
        arm: &str,
        capability: &str,
        origin: Option<String>,
    ) -> Result<Denial, String> {
        match (arm, origin) {
            ("Unregistered", None) => Ok(Denial::Unregistered {
                capability: capability.to_string(),
            }),
            ("Unregistered", Some(_)) => Err(
                "an Unregistered denial carries an origin: the capability was never reachable, so \
                 no destination was ever consulted"
                    .to_string(),
            ),
            ("GateRefused", origin) => Ok(Denial::GateRefused {
                capability: capability.to_string(),
                origin,
            }),
            (other, _) => Err(format!("'{other}' is not an arm of the denial vocabulary")),
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
/// Where a host will let an effect send its payload — the SECOND question,
/// consulted after the discriminator gate has answered the first.
///
/// The gate sees the effect and can therefore see its payload, so a host could
/// in principle express this as a gate closure. It must not, and the reason is
/// the record rather than the decision: a gate refusal and a destination refusal
/// are both `GateRefused`, and only the second one has an origin to name. Folded
/// into the gate they would be indistinguishable in the denial, which is the one
/// place the difference is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressFloor {
    /// Every destination permitted. The named opt-in, paired with a permissive
    /// gate: a host that believes it permitted everything and quietly kept a
    /// local-only floor would be wrong about itself.
    AnyOrigin,
    /// Only destinations that have not left this host's own origin. A relative
    /// route or a fragment has not left; an absolute network destination has,
    /// and so has a hostless scheme, which is an egress channel with no host for
    /// a rule to name. An effect naming no destination at all is unaffected —
    /// the discriminator gate is the whole policy for those, and inventing a
    /// destination for one would only make the record dishonest.
    LocalOnly,
}

pub struct EffectPolicy {
    registered: BTreeSet<String>,
    gate: Box<dyn Fn(&ClientEffect) -> bool>,
    egress: EgressFloor,
}

/// The destination an arm sends its payload to.
///
/// `ReadFileBody` is local rather than absent, and the distinction is the honest
/// one rather than a convenience: the arm carries a node id, not a URL, so there
/// is no origin to allowlist — but the body it reads travels back to the host
/// driving the loop, which is the local origin by construction.
fn destination_of(effect: &ClientEffect) -> Option<crate::render::egress::Destination> {
    use crate::render::egress::{Destination, classify_destination};
    match effect {
        ClientEffect::Navigate { route } | ClientEffect::PushState { route } => {
            Some(classify_destination(route))
        }
        ClientEffect::Download { url, .. } => Some(classify_destination(url)),
        ClientEffect::ReadFileBody { .. } => Some(Destination::Local),
        ClientEffect::WriteToClipboard { .. } | ClientEffect::Focus { .. } => None,
    }
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
            egress: EgressFloor::LocalOnly,
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
            egress: EgressFloor::AnyOrigin,
        }
    }

    /// **The named host policy the scenario corpus's `local-egress-only`
    /// denotes**: every arm registered and permitted by the discriminator gate,
    /// and a destination that leaves this host's own origin declined.
    ///
    /// Constructed here rather than carried by a fixture. A denial is a fact
    /// about a policy, and a corpus that carried the policy as data would be
    /// specifying one — so the corpus names it and every host builds what the
    /// name denotes.
    pub fn local_egress_only() -> Self {
        EffectPolicy {
            egress: EgressFloor::LocalOnly,
            ..EffectPolicy::permissive()
        }
    }

    /// Construct the policy a scenario's declared host-policy name denotes, or
    /// `None` for a scenario that names none.
    ///
    /// **An unrecognised name is an error, never a fallback.** Falling back to
    /// this host's own default would report a scenario this host could not
    /// evaluate as one it passed — the vacuous green every other obligation in
    /// the conformance family is shaped to refuse, arriving on precisely the
    /// scenarios whose whole content is what a policy declines.
    pub fn named(name: Option<&str>) -> Result<Self, String> {
        match name {
            None => Ok(EffectPolicy::permissive()),
            Some("local-egress-only") => Ok(EffectPolicy::local_egress_only()),
            Some(other) => Err(format!(
                "'{other}' is not a host policy this implementation constructs; a scenario naming                  one it cannot build is out of scope, not passed"
            )),
        }
    }

    /// Replace the destination floor (builder-style).
    #[must_use]
    pub fn with_egress(mut self, egress: EgressFloor) -> Self {
        self.egress = egress;
        self
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
    /// The ordering is load-bearing at two points, and both are decisions.
    /// Registration before the gate, so an unregistered arm is unreachable
    /// however permissive the policy; and the DISCRIMINATOR gate before the
    /// DESTINATION floor, so an effect this host does not perform at all is
    /// refused before its payload is ever parsed — which is what makes an
    /// absent `origin` mean *no destination was consulted* rather than *the
    /// destination was fine*.
    pub fn decide(&self, effect: &ClientEffect) -> Option<Denial> {
        use crate::render::egress::Destination;
        let capability = effect.capability().to_string();
        if !self.registered.contains(&capability) {
            return Some(Denial::Unregistered { capability });
        }
        if !(self.gate)(effect) {
            return Some(Denial::GateRefused {
                capability,
                origin: None,
            });
        }
        if self.egress == EgressFloor::AnyOrigin {
            return None;
        }
        // The origin, and never the URL: a denial record outlives the session
        // that produced it, and the query string of a refused exfiltration
        // attempt IS the payload.
        let origin = match destination_of(effect) {
            None | Some(Destination::Local) => return None,
            Some(Destination::Remote(host)) => host,
            Some(Destination::NonNetwork(scheme)) => format!("{scheme}:"),
            Some(Destination::Rejected) => "unparseable".to_string(),
        };
        Some(Denial::GateRefused {
            capability,
            origin: Some(origin),
        })
    }
}

/// String escaping for this family's envelope: `"` and `\`, the three common
/// control characters as their short escapes, and every remaining control
/// character as `\u00xx` with lower-case hex. Nothing else is escaped — a
/// non-ASCII character passes through as its literal UTF-8 sequence.
///
/// The `\u00xx` half is the part that is easy to omit, and §5.2 now says so in
/// as many words: the envelope's exception narrows three code points and no
/// more, and an encoder emitting any other control raw has produced text that is
/// not JSON. A substitution list is complete for the three it names and silently
/// empty for the rest, which no printable-string corpus can catch.
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
                capability: "Navigate".into(),
                origin: None
            }),
            "a gate refusal names no origin: it declined the ARM, before any payload was parsed"
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

    #[test]
    fn the_destination_floor_declines_a_non_local_route_and_names_its_origin_only() {
        let policy = EffectPolicy::local_egress_only();

        // A route that has not left the origin passes the floor untouched.
        assert_eq!(
            policy.decide(&ClientEffect::Navigate {
                route: "/orders".into()
            }),
            None
        );

        // One that has is declined, and the record carries the HOST — not the
        // path and not the query, which is where the payload of an
        // exfiltration attempt would be sitting.
        assert_eq!(
            policy.decide(&ClientEffect::Navigate {
                route: "https://exfil.example/collect?session=secret".into()
            }),
            Some(Denial::GateRefused {
                capability: "Navigate".into(),
                origin: Some("exfil.example".into())
            })
        );

        // An arm with no destination at all is unaffected: the discriminator
        // gate is the whole policy for those, and inventing a destination
        // would only make the record dishonest.
        assert_eq!(
            policy.decide(&ClientEffect::WriteToClipboard {
                text: "https://exfil.example/collect".into()
            }),
            None
        );
    }

    #[test]
    fn the_two_refusal_grounds_are_distinguishable_in_the_record() {
        // The whole reason the floor is a second dimension rather than a gate
        // closure: both refusals are GateRefused, and only one has an origin.
        let effect = ClientEffect::Navigate {
            route: "https://exfil.example/x".into(),
        };
        let by_gate = EffectPolicy::permissive().with_gate(|_| false);
        let by_destination = EffectPolicy::local_egress_only();
        assert_ne!(by_gate.decide(&effect), by_destination.decide(&effect));
    }

    #[test]
    fn a_recorded_denial_round_trips_and_an_ill_formed_one_does_not_load() {
        for denial in [
            Denial::Unregistered {
                capability: "Focus".into(),
            },
            Denial::GateRefused {
                capability: "Navigate".into(),
                origin: None,
            },
            Denial::GateRefused {
                capability: "Download".into(),
                origin: Some("cdn.example".into()),
            },
        ] {
            let (arm, capability, origin) = match &denial {
                Denial::Unregistered { capability } => ("Unregistered", capability.clone(), None),
                Denial::GateRefused { capability, origin } => {
                    ("GateRefused", capability.clone(), origin.clone())
                }
            };
            assert_eq!(
                Denial::from_wire(arm, &capability, origin),
                Ok(denial.clone()),
                "{}",
                denial.encode()
            );
        }

        // An origin on the arm that consulted no destination is a member that
        // cannot describe the fact beside it.
        assert!(Denial::from_wire("Unregistered", "Navigate", Some("x.example".into())).is_err());
        assert!(Denial::from_wire("DestinationRefused", "Navigate", None).is_err());
    }

    #[test]
    fn an_unrecognised_host_policy_name_is_refused_rather_than_defaulted() {
        assert!(EffectPolicy::named(None).is_ok());
        assert!(EffectPolicy::named(Some("local-egress-only")).is_ok());
        assert!(
            EffectPolicy::named(Some("anything-goes")).is_err(),
            "a fallback would report a scenario this host could not evaluate as one it passed"
        );
    }
}
