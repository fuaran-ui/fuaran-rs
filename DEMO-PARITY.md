# Demo parity — capability matrix

The public Fuaran demo site (`demo-site/`) stages 21 live demos across four
pillars. "Demo parity" means: **every mechanic a demo exercises can run in this
host** — not the F# UI shell, but the underlying wire/substrate operation.

This matrix maps each demo to the host capability its mechanic requires and
tracks `fuaran-rs` status. The demo *shell* is F#/Fable; what matters here is
whether the Rust host can perform the demo's substrate operation (decode, apply,
render, verify a chain, merge, evaluate a transform, introspect, …).

**Status legend:** ✅ shipped & tested · 🟡 partial · ⬜ not yet.

## Capability → status

| Capability | Status | Module | Certified against |
|---|---|---|---|
| `codec` (decode/encode wire trees + ops) | ✅ | `wire/` | `wire-format-fixtures` node/op round-trip + reject |
| `apply` (tree-op apply engine + dry-run) | ✅ | `ops/` | apply-envelope law + behaviour suite |
| `render` (server-HTML + islands + markdown) | ✅ | `render/` | markdown corpus + CSS byte-parity + islands laws |
| `validator` (pre-emit structural defects) | ✅ | `validator/` | per-rule fire/stay-silent |
| `client` (wasm32 decode→render→drive) | ✅ | `client/` | native session suite + in-browser ABI verify |
| `opstream-hashchain` (SHA-256 chain, verify, tamper) | ✅ | `opstream/` | `chain/chain-corpus.json` golden (byte-identical) + NIST SHA-256 vectors |
| `merge-dag` (3-way tree merge) | ⬜ | — | `merge-conformance/` (planned) |
| `dag-record` (DAG record wire form) | ⬜ | — | `dag/` (planned) |
| `transform-eval` (Binding.Transform dataframe pipeline) | ⬜ | — | behavioural (planned) |
| `layout-observe` (structural overflow/reflow flags) | ⬜ | — | (planned) |
| `introspection` (getNodeState / assertions over the tree) | ⬜ | — | (planned) |
| `theme-contrast` (theme resolve + WCAG contrast) | ⬜ | — | (planned) |
| `search-pattern` (structural search over trees) | ⬜ | — | (planned) |
| `teleport` (FT1 deflate+base64url+SHA-256 envelope, §17) | ⬜ | — | (planned) |
| `action-gate` (default-deny dispatch/decode gate) | 🟡 | `wire/` + `validator/` | decode-reject IS the structural gate; a dispatch allowlist is the remaining piece |
| `email-safe render` (Send Me digest projection) | ⬜ | — | (planned) |
| `versioning-envelope` (§15 profile/Unknown tolerance) | ⬜ | — | `envelope/` (planned) |

## Demo → capabilities → status

| Demo | Pillar | Required | Status |
|---|---|---|---|
| Rosetta | Wire | codec, cross-host hash | ✅ (codec byte-identical; SHA-256 shipped) |
| Degradation | Wire | render, codec | ✅ (server render = the no-JS tier) |
| Pandas | Wire | codec, apply, render | ✅ (host decodes+applies the Python-authored wire) |
| Send Me | Wire | render, codec | 🟡 (crawlable + live shipped; email-digest render ⬜) |
| Bouncer | Machine | action-gate, codec, validator | ✅ (decode-reject bounces hostile payloads with typed codes) |
| Notarised | Value | opstream-hashchain, apply, codec | ✅ |
| Relay | Wire | opstream-hashchain, apply, codec, cross-host hash | ✅ |
| Time Machine | Value | apply, codec, opstream (light) | ✅ (fold apply over an op prefix; chain provenance) |
| Bazaar | Value | codec, mount-isolation, action-gate | 🟡 (Mount kind decodes+renders; capability-gate enforcement ⬜) |
| Git for Interfaces | Value | merge-dag, apply | ⬜ (needs 3-way merge) |
| Counterfactual | Value | merge-dag, apply | ⬜ (needs 3-way merge) |
| What-If | Value | apply, tree-diff | 🟡 (apply shipped; a tree-diff op-script generator ⬜) |
| Living Sheet | Wire | transform-eval | ⬜ (needs dataframe evaluation) |
| Pattern Bank | Machine | search-pattern, transform-eval | ⬜ |
| Grep Apps | Value | search-pattern, render | ⬜ (needs structural search) |
| Kintsugi | Machine | introspection, theme-contrast, layout-observe, apply | ⬜ (needs the observers) |
| Infinite Skins | Intent | theme-contrast, render | 🟡 (render shipped; contrast auditor ⬜) |
| Every Screen | Intent | render, responsive-layout | ✅ (reference-CSS breakpoints fold the grid — no host code) |
| Blind Surveyor | Machine | layout-observe | ⬜ |
| Unit Test | Machine | introspection, layout-observe | ⬜ |
| Teleport | Value | teleport, codec, validator, versioning-envelope | ⬜ |

## Build order (leverage-first)

1. **opstream-hashchain** ✅ — Notarised, Relay, Time Machine.
2. **dag-record + merge-dag** — Git for Interfaces, Counterfactual (corpus-certified).
3. **transform-eval** — Living Sheet, Pattern Bank.
4. **layout-observe** — Kintsugi, Blind Surveyor, Unit Test (3 demos).
5. **introspection** — Unit Test, Kintsugi.
6. **theme-contrast** — Infinite Skins, Kintsugi.
7. **search-pattern** — Grep Apps, Pattern Bank.
8. **teleport (+ versioning-envelope)** — Teleport.
9. **email-safe render** — Send Me.

The spine (`codec` + `apply` + `render` + `validator`) already runs ~15 of the
21 demos; the remaining work is the finite observer/integrity/compute set above.
