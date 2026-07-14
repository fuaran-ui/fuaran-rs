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
| `teleport` (FT1 deflate+base64url+SHA-256 envelope, §17) | ✅ | `teleport/` | byte-exact string round-trip + digest-tamper/version/oversize rejects; hand-written RFC 1951 DEFLATE (self round-trip + stored/fixed/dynamic inflate) |
| `action-gate` (default-deny dispatch/decode gate) | ✅ | `gate/` + `wire/` + `validator/` | decode-reject is the structural gate; `gate/` adds the default-deny capability allowlist (per-mount + per-Invoke) |
| `tree-diff` (before→after op-script) | ✅ | `diff/` | `apply(diff(a,b), a) == b` over changed-leaf / heading / root-swap / child add+remove |
| `email-safe render` (Send Me digest projection) | ✅ | `render/email.rs` | plain-text digest of content nodes; interactive/structural omitted; no HTML surface |
| `versioning-envelope` (§15 profile/Unknown tolerance) | ✅ | `envelope.rs` | negotiation (Current/Behind/Foreign) + degrade-and-preserve — Phase 553; byte-exact |
| `elicitation` (§18 question-as-UI + typed answer contract + outcome codes) | ✅ | `elicitation.rs` | shared `elicitation/` corpus family — Phase 553; answer accept/reject + outcome round-trip |

## Demo → capabilities → status

| Demo | Pillar | Required | Status |
|---|---|---|---|
| Rosetta | Wire | codec, cross-host hash | ✅ (codec byte-identical; SHA-256 shipped) |
| Degradation | Wire | render, codec | ✅ (server render = the no-JS tier) |
| Pandas | Wire | codec, apply, render, transform-eval | ✅ (host decodes+applies the wire; the dataframe pipeline evaluates as data) |
| Send Me | Wire | render, codec, email-safe render | ✅ (crawlable + live + email-safe plain-text digest) |
| Bouncer | Machine | action-gate, codec, validator | ✅ (decode-reject bounces hostile payloads with typed codes) |
| Notarised | Value | opstream-hashchain, apply, codec | ✅ |
| Relay | Wire | opstream-hashchain, apply, codec, cross-host hash | ✅ |
| Time Machine | Value | apply, codec, opstream (light) | ✅ (fold apply over an op prefix; chain provenance) |
| Bazaar | Value | codec, mount-isolation, action-gate | ✅ (Mount decodes+renders; default-deny capability gate enforces per-mount grants) |
| Git for Interfaces | Value | merge-dag, apply | ✅ |
| Counterfactual | Value | merge-dag, apply | ✅ |
| What-If | Value | apply, tree-diff | ✅ (before→after tree-diff generates a replayable op-script) |
| Living Sheet | Wire | transform-eval | ✅ (dataframe pipeline evaluated as data) |
| Pattern Bank | Machine | search-pattern, transform-eval | ✅ (structural search + computed-metric transform-eval) |
| Grep Apps | Value | search-pattern, render | ✅ |
| Kintsugi | Machine | introspection, theme-contrast, layout-observe, apply | ✅ (introspection + layout-observe + apply + the contrast sense all ✅) |
| Infinite Skins | Intent | theme-contrast, render | ✅ (render + WCAG contrast auditor shipped) |
| Every Screen | Intent | render, responsive-layout | ✅ (reference-CSS breakpoints fold the grid — no host code) |
| Blind Surveyor | Machine | layout-observe | ✅ |
| Unit Test | Machine | introspection, layout-observe | ✅ |
| Teleport | Value | teleport, codec, validator, versioning-envelope | ✅ (FT1 bundle: serialise running app → string → resume, digest-verified) |

## Agent-engagement surface (Phases 465–466)

Beyond the four-pillar demos, the combined demo/live surface stages the
**agent-engagement** mechanics — an agent asks and expresses through live Fuaran UI,
not prose. The Rust host carries both:

| Feature | Required | Status |
|---|---|---|
| **Elicitation** *(question-as-UI + typed answer, Ph 465)* | `elicitation` (§18 envelope + answer contract), `codec`, `apply`, `action-gate` | ✅ (elicitation envelope + closed outcome set decode/encode byte-exact; the answer resolves through the session's write-back + apply, gated default-deny) |
| **Agent expression turns** *(live emitted panels, Ph 466)* | `codec`, `apply` (streamed tree-ops), `opstream` (per-panel scope), `client`/`render` (live), interaction | ✅ (a streamed tree + subsequent ops render and stay drivable through the shipped client session; each panel is an op-stream scope — all mechanics ✅) |

Both are **compositions of shipped mechanics** — 466 adds no new wire vocabulary
(no `NodeKind`/`Action`/`Binding`), and 465 wraps a tree in a typed envelope rather
than extending the tree. The Phase 548 cross-host kind-set attestation guard would
fail if either had introduced an unpropagated kind; it is green.

## Build order (leverage-first)

1. **opstream-hashchain** ✅ — Notarised, Relay, Time Machine.
2. **dag-record + merge-dag** ✅ — Git for Interfaces, Counterfactual (corpus-certified).
3. **introspection + search-pattern + layout-observe** ✅ — Unit Test, Grep Apps, Blind Surveyor, + partial Kintsugi / Pattern Bank.
4. **theme-contrast** ✅ — Infinite Skins, Kintsugi (the contrast sense).
5. **transform-eval** ✅ — Living Sheet, Pattern Bank (the computed metric), Pandas.
6. **teleport** ✅ — Teleport (hand-written RFC 1951 DEFLATE + base64url + FT1 envelope).
7. **email-safe render** ✅ — Send Me (plain-text digest projection).
8. **capability-gate** ✅ — Bazaar (default-deny per-mount capability allowlist).
9. **tree-diff op-script** ✅ — What-If (before→after replayable op-script).

## Where parity stands

**Demo parity is reached: the host runs the mechanic of all 21 demos.** The spine
(`codec` + `apply` + `render` + `validator` + `client` + `opstream` +
`dag/merge` + `introspect` + `theme` + `transform` + `teleport` + `gate` +
`diff` + `render/email`) covers every demo on the site — a Rust service, edge
worker, or in-browser `wasm32` client can decode, apply, render, verify, merge,
evaluate a transform, introspect, contrast-audit, teleport, capability-gate, and
diff any Fuaran UI wire tree.

The `versioning-envelope` (§15) and `elicitation` (§18) families — previously the
open depth items — **shipped in Phase 553**, so every capability in the matrix is now
✅. Beyond the four-pillar demos, the host also carries the **agent-engagement**
mechanics (Ph 465 elicitation + Ph 466 agent expression turns; see the section above)
as compositions of those shipped capabilities. The remaining conformance-family depth
is the `lenient-accept` (§16 shorthand normalisation) family only — a codec-tolerance
tier, not demo coverage.

_Updated 2026-07-14: `versioning-envelope` + `elicitation` flipped to ✅ (Phase 553,
`envelope.rs` / `elicitation.rs`); agent-engagement section added for Phases 465/466._
