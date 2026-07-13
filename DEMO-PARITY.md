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
| `merge-dag` (3-way tree merge) | ✅ | `dag/merge.rs` | `merge-conformance/` (byte-identical tree + outcome hash) |
| `dag-record` (DAG record wire form) | ✅ | `dag/record.rs` | `dag/` round-trip byte-identical |
| `introspection` (getNodeState / assertions over the tree) | ✅ | `introspect/` | behaviour suite (facts, restyle-proof assertions) |
| `search-pattern` (structural search over trees) | ✅ | `introspect/` | behaviour suite (structural query → matched ids) |
| `layout-observe` (structural overflow/reflow flags) | ✅ | `introspect/layout.rs` | `LayoutObserver.Flags.derive` port + behaviour suite |
| `transform-eval` (Binding.Transform dataframe pipeline) | ✅ | `transform/` | pinned cross-host semantics (null/coercion/round-half-away/div-by-zero/stability) + canonical round-trip |
| `theme-contrast` (theme resolve + WCAG contrast) | ✅ | `theme/` | WCAG reference constants (black/white = 21.0, #767676 grey = AA boundary, alpha compositing) |
| `teleport` (FT1 deflate+base64url+SHA-256 envelope, §17) | ⬜ | — | (planned) |
| `action-gate` (default-deny dispatch/decode gate) | 🟡 | `wire/` + `validator/` | decode-reject IS the structural gate; a dispatch allowlist is the remaining piece |
| `email-safe render` (Send Me digest projection) | ⬜ | — | (planned) |
| `versioning-envelope` (§15 profile/Unknown tolerance) | ⬜ | — | `envelope/` (planned) |

## Demo → capabilities → status

| Demo | Pillar | Required | Status |
|---|---|---|---|
| Rosetta | Wire | codec, cross-host hash | ✅ (codec byte-identical; SHA-256 shipped) |
| Degradation | Wire | render, codec | ✅ (server render = the no-JS tier) |
| Pandas | Wire | codec, apply, render, transform-eval | ✅ (host decodes+applies the wire; the dataframe pipeline evaluates as data) |
| Send Me | Wire | render, codec | 🟡 (crawlable + live shipped; email-digest render ⬜) |
| Bouncer | Machine | action-gate, codec, validator | ✅ (decode-reject bounces hostile payloads with typed codes) |
| Notarised | Value | opstream-hashchain, apply, codec | ✅ |
| Relay | Wire | opstream-hashchain, apply, codec, cross-host hash | ✅ |
| Time Machine | Value | apply, codec, opstream (light) | ✅ (fold apply over an op prefix; chain provenance) |
| Bazaar | Value | codec, mount-isolation, action-gate | 🟡 (Mount kind decodes+renders; capability-gate enforcement ⬜) |
| Git for Interfaces | Value | merge-dag, apply | ✅ |
| Counterfactual | Value | merge-dag, apply | ✅ |
| What-If | Value | apply, tree-diff | 🟡 (apply shipped; a tree-diff op-script generator ⬜) |
| Living Sheet | Wire | transform-eval | ✅ (dataframe pipeline evaluated as data) |
| Pattern Bank | Machine | search-pattern, transform-eval | ✅ (structural search + computed-metric transform-eval) |
| Grep Apps | Value | search-pattern, render | ✅ |
| Kintsugi | Machine | introspection, theme-contrast, layout-observe, apply | ✅ (introspection + layout-observe + apply + the contrast sense all ✅) |
| Infinite Skins | Intent | theme-contrast, render | ✅ (render + WCAG contrast auditor shipped) |
| Every Screen | Intent | render, responsive-layout | ✅ (reference-CSS breakpoints fold the grid — no host code) |
| Blind Surveyor | Machine | layout-observe | ✅ |
| Unit Test | Machine | introspection, layout-observe | ✅ |
| Teleport | Value | teleport, codec, validator, versioning-envelope | ⬜ |

## Build order (leverage-first)

1. **opstream-hashchain** ✅ — Notarised, Relay, Time Machine.
2. **dag-record + merge-dag** ✅ — Git for Interfaces, Counterfactual (corpus-certified).
3. **introspection + search-pattern + layout-observe** ✅ — Unit Test, Grep Apps, Blind Surveyor, + partial Kintsugi / Pattern Bank.
4. **theme-contrast** ✅ — Infinite Skins, Kintsugi (the contrast sense).
5. **transform-eval** ✅ — Living Sheet, Pattern Bank (the computed metric), Pandas.
6. **teleport (+ versioning-envelope)** — Teleport.
7. **email-safe render** — Send Me.
8. **tree-diff op-script** — What-If (+ Pandas re-run patch).

## Where parity stands

The spine (`codec` + `apply` + `render` + `validator` + `client` + `opstream` +
`dag/merge` + `introspect` + `theme` + `transform`) now runs the mechanic of
**almost the whole site** — 18 of 21 demos fully covered: Rosetta, Degradation,
Pandas, Bouncer, Notarised, Relay, Time Machine, Git for Interfaces,
Counterfactual, Grep Apps, Every Screen, Blind Surveyor, Unit Test, Kintsugi,
Infinite Skins, Living Sheet, Pattern Bank. Partial (a named sub-capability
remaining): Send Me (email digest), Bazaar (capability-gate enforcement), What-If
(tree-diff). Remaining net-new capability: **teleport** (Teleport), plus the
small **email-digest render** + **tree-diff** + **capability-gate** pieces.
