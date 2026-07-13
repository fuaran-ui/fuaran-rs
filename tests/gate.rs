//! Capability gate — default-deny dispatch over mounted mini-apps (Bazaar).
//! A mount is live only when the host grants every capability it declares.

use fuaran_rs::gate::{CapabilityGate, GateDecision};
use fuaran_rs::wire::decode_node;

// A marketplace: two mounted mini-apps with different capability demands.
const BAZAAR: &str = r#"{"id":"market","kind":{"$type":"Box","children":[
    {"id":"weather","kind":{"$type":"Mount","capabilities":["net.fetch"],"channel":{"direction":"OutOnly"},"onBubble":"<closure>","scopeId":"weather"}},
    {"id":"notes","kind":{"$type":"Mount","capabilities":["fs.read","fs.write"],"channel":{"direction":"OutOnly"},"onBubble":"<closure>","scopeId":"notes"}},
    {"id":"clock","kind":{"$type":"Mount","capabilities":[],"channel":{"direction":"OutOnly"},"onBubble":"<closure>","scopeId":"clock"}}
],"layout":{"$type":"Flex","direction":"Vertical","wrap":false},"role":"Group"}}"#;

fn tree() -> fuaran_rs::wire::Node {
    decode_node(BAZAAR).expect("bazaar decodes")
}

#[test]
fn a_default_gate_denies_every_capability() {
    let gate = CapabilityGate::default();
    assert!(!gate.allows("net.fetch"));
    assert_eq!(
        gate.decide_capability("net.fetch"),
        GateDecision::Deny {
            missing: vec!["net.fetch".to_string()]
        }
    );
}

#[test]
fn a_mount_is_live_only_when_all_its_capabilities_are_granted() {
    // Host grants read + network, but not write.
    let gate = CapabilityGate::granting(["net.fetch", "fs.read"]);
    let audit = gate.audit_mounts(&tree());
    assert_eq!(
        audit,
        vec![
            ("weather".to_string(), GateDecision::Allow), // net.fetch granted
            (
                "notes".to_string(),
                GateDecision::Deny {
                    missing: vec!["fs.write".to_string()] // read ok, write missing
                }
            ),
            ("clock".to_string(), GateDecision::Allow), // declares nothing
        ]
    );
}

#[test]
fn granting_the_missing_capability_makes_the_mount_live() {
    let gate = CapabilityGate::granting(["net.fetch", "fs.read"]).grant("fs.write");
    let live: Vec<String> = gate
        .audit_mounts(&tree())
        .into_iter()
        .filter(|(_, d)| d.is_allowed())
        .map(|(id, _)| id)
        .collect();
    assert_eq!(live, vec!["weather", "notes", "clock"]);
}

#[test]
fn a_chained_action_denies_if_any_link_is_ungranted() {
    use fuaran_rs::wire::{Action, InvokeArg};
    let chain = Action::Chain(vec![
        Action::Invoke {
            capability_id: "fs.read".to_string(),
            args: vec![],
        },
        Action::Invoke {
            capability_id: "net.post".to_string(),
            args: vec![InvokeArg {
                addr: "url".to_string(),
                value: "x".to_string(),
            }],
        },
    ]);
    let gate = CapabilityGate::granting(["fs.read"]);
    assert_eq!(
        gate.decide_action(&chain),
        GateDecision::Deny {
            missing: vec!["net.post".to_string()]
        }
    );
    // Grant the second, and the whole chain is admitted.
    assert!(gate.grant("net.post").decide_action(&chain).is_allowed());
}
