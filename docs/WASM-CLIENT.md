# The WASM browser client

Compiled to `wasm32`, `fuaran-rs` is a **browser-native client host**: it decodes
a wire tree, renders it, applies tree-ops against it, and writes the reactive
stores — the whole decode → render → drive loop, client-side, with no JavaScript
framework and no `wasm-bindgen`.

This is the half of the dual role that the other backend hosts do not reach. The
same crate that runs headless on a server runs in the page.

## Build it

```powershell
rustup target add wasm32-unknown-unknown          # once
cargo build --target wasm32-unknown-unknown --release
# -> target/wasm32-unknown-unknown/release/fuaran_rs.wasm   (~0.7 MB)
```

`run.ps1` builds it automatically when that target is installed, so the gate
covers it rather than leaving it to drift.

The size comes from the release profile in `Cargo.toml`, which is tuned for this
artefact specifically: `opt-level = "s"`, `lto = true`, `panic = "abort"` (no
unwinding tables in the browser module) and `strip = true`.

To try the shipped demo, put the module beside the loader and serve the
directory over HTTP — ES modules and WebAssembly both need a real origin, not
`file://`:

```powershell
cp target/wasm32-unknown-unknown/release/fuaran_rs.wasm js/fuaran_rs.wasm
# then serve js/ with any static file server
```

`js/index.html` demonstrates the loop end to end.

## Mount a tree

`js/fuaran-loader.js` is a hand-written, dependency-free loader. Two functions in,
one session object out:

```js
import { loadFuaran, createSession } from './fuaran-loader.js';

const exports = await loadFuaran('./fuaran_rs.wasm');
const session = createSession(exports, tree);   // a JS object or a JSON string

session.mount(document.getElementById('app'));
```

`mount(el)` sets `el.innerHTML` to the current render. The session's other
methods:

| Method | What it does |
|---|---|
| `render()` | the current tree as a body-fragment HTML string |
| `treeJson()` | the current tree, re-encoded to canonical wire JSON |
| `applyOp(op)` | apply a canonical `TreeOp` (object or JSON string) |
| `setState(key, value)` | write a reactive `$state.<key>` slot |
| `setFilter(name, value)` | write a `$filters.<name>` slot |
| `setQuery(name, value)` | seed a `$queries.<name>` result slot |
| `free()` | release the module-side memory; idempotent |

A structured failure throws a `FuaranClientError` carrying `code`, `class`
(`"decode"` or `"apply"`) and the full envelope — so a malformed op is a catchable
JavaScript error with the same canonical code every host reports, not an opaque
trap.

**Call `free()`.** The session lives in the module's linear memory, which nothing
garbage-collects on your behalf. A single-page app that opens a session per view
and never frees them leaks.

## Driving it: write the store, re-render

The module renders **inert** HTML. No event handlers ride the wire — closures are
sentinels, by design — so nothing in the rendered markup calls back into your
code by itself. Interactivity comes from writing the reactive stores and
re-rendering, which is exactly what the wire's *write-back default* prescribes:
an omitted handler over a writable `State` or `Filter` slot means the control
writes its change back to that slot.

```js
appEl.addEventListener('change', (e) => {
  const key = /* your app's mapping from this control to its state key */;
  session.setState(key, e.target.value);
  session.mount(appEl);
});
```

The loader deliberately does **not** wire that up for you. The parity-locked
render carries no slot attribute — adding one would be a wire-format change, not a
loader change — so the control-to-slot mapping is app-specific. `js/index.html`
shows the pattern; copy it and adapt.

Structural change is the other half:

```js
session.applyOp({ $type: 'UpdateProp', target: 'count', path: 'Value', value: 1 });
session.mount(appEl);
```

That runs the same total apply engine the headless host uses. An op that does not
apply throws and the held tree is untouched — there is no partial mutation to
recover from.

## The Rust side, if you are embedding rather than loading

The loader is a thin marshalling shim over `ClientSession`, and everything above
is available natively:

```rust
use fuaran_rs::client::{ClientSession, RowsOutcome};

let mut session = ClientSession::new(wire_json)?;
let html = session.render();
session.set_state("users", "42")?;
session.apply_op(op_json)?;
```

`ClientSession` is **pure, target-agnostic Rust** with no `wasm32`-specific code —
`tests/client.rs` exercises it natively, which is why the browser leg is not a
separately-tested implementation. The WASM tier is a marshalling shim over this
same type.

Two methods are worth knowing about because they exist for consumers this crate
does not contain:

- **`project_resolved()`** returns the tree with every scalar-slot
  `Binding.Transform` folded to the value it evaluates to. It is byte-identical
  to `tree_json()` everywhere else. A **decode-only** consumer — a native render
  surface over this core — can then show computed values without carrying an
  evaluator of its own.
- **`resolved_rows(node_id)`** answers for a row-bearing node (`DataGrid`,
  `Chart`, `Map`, `Sparkline`), and it has **three** outcomes rather than two:

  ```rust
  pub enum RowsOutcome {
      Rows(Vec<JVal>),  // resolved — possibly to zero rows, which is an EMPTY state
      NotResolved,      // a Transform that errored, or a store not yet fed — render LOADING
      NoRowSource,      // no such node, or its kind has no row source — a caller mistake
  }
  ```

  The middle case is the point. Collapsing `NotResolved` into an empty list shows
  "no data" for "not yet", which looks right and is wrong — the failure this whole
  tier exists to avoid.

## Rendering parity with the server

The client renders through the **same server renderer**. One tree plus one set of
sources produces byte-identical HTML whether it came from a native process or the
browser module. That is what makes the third delivery mode — static page,
hydrate the interactive regions — work without a reconciliation mismatch: the
markup the client re-renders into a boundary is the markup that was already there.

`render_with_islands` is the emission side of that, and `render_hydratable`
emits a whole-tree hydration payload. Both live in `fuaran_rs::render`.

## Destinations: deny by default, widen by name

A session holds a tree that **arrived over the wire** — precisely the case the
deny default exists for. So `ClientSession::new` takes `deny_non_local_egress()`,
and widening is a named builder call:

```rust
let session = ClientSession::new(wire_json)?
    .with_egress_policy(my_policy);
```

Named rather than defaulted, deliberately: a grep for the widening call finds
every session that did it.

**The C-ABI surface declares nothing**, so every FFI-driven session — the browser
module included — renders under the deny default. If your page needs a wider
posture, the module you build has to establish it in Rust; there is no JavaScript
switch for it, and that is not an oversight.

## The C-ABI, and the one representational difference

`include/fuaran.h` declares the target-neutral session surface: `fuaran_alloc` /
`fuaran_dealloc`, `fuaran_session_new` / `_free`, `_render`, `_tree_json`,
`_project_resolved`, `_resolved_rows`, `_apply_op`, `_set_state` / `_set_filter` /
`_set_query`, and `fuaran_last_error`. The same symbols are emitted into the
`wasm32` module and into the native `staticlib` the Swift and Kotlin surfaces
link.

Every text-returning function hands back a **Rust-owned** `(ptr, len)` pair. The
caller reads exactly `len` UTF-8 bytes and then frees the buffer with
`fuaran_dealloc(ptr, len)`. The buffer is exact-length: **there is no trailing
NUL**, so always honour `len` and never reach for `strlen`.

The one difference between the two worlds is how that pair is represented, and it
falls out of pointer width:

- **Native (64-bit)** — a two-word `FuaranBuf { uint8_t *ptr; size_t len; }`
  returned by value.
- **`wasm32` (32-bit pointers)** — the same pair packed into a `uint64_t`, `ptr`
  in the high 32 bits and `len` in the low 32.

A full pointer only shares a 64-bit word with a length losslessly when pointers
are 32 bits wide, which is why the packed form is a `wasm32`-only affordance. The
JS loader is written against the packed form and needs no change for it; native
code never sees it. Both are documented in the header so the two ABIs are on the
record together.

## Edge hosting: the same session, evicted and restored

`src/edge/` is the other half of this tier, and it exists because a worker runtime
evicts a session between requests **by design**. An `EdgeSession` owns its store —
so one activation per handle is the compiler's guarantee rather than a review
comment — journals each applied `TreeOp` to a durable store **before** the held
tree moves, and rehydrates from that journal (newest checkpoint plus suffix, chain
verified) on the next activation.

`DurableSessionStore` is a four-method trait with **no vendor in it**: acquire an
activation, read the journal, append a record durably, record a snapshot.
`InMemoryDurableStore` implements the fence fully, so `tests/edge.rs` exercises the
protocol rather than merely describing it; a real host writes its own against the
same four sentences and links nothing from here.

What that tier honestly claims, stated because the boundary matters:

- **A record is durable before the tree moves**, so a crash between the two
  replays to the same tree. `apply` is a total function of the tree and the op and
  reads nothing else — no clock, no id generator, no filesystem.
- **A superseded activation is refused** (`StoreError::NotOwner`) rather than
  interleaved. Ownership cannot reach a second *process* opening the same session
  id, so the store carries a monotonic `ActivationToken` and every write presents
  it.
- **A refused op journals nothing.** The journal is applied history; a record whose
  op does not apply would make replay fail on its own evidence.
- **Reactive slots are not journaled.** `$state` / `$filters` / `$queries` are a
  view's live inputs, not authored history, so they do not survive an eviction and
  a host re-seeds them on activation.

The whole module compiles for `wasm32` with no target-specific code, which is what
makes the browser module and an edge worker the same session type.

## Chart lowering, and why it differs here

This host takes the **lower-in-host** posture: a `Chart` reaching the renderer is
lowered deterministically to a canonical `Drawing` and painted as inline SVG. The
headless-only host takes the opposite posture and emits a hydration placeholder
instead.

The difference is this leg. A browser client that could not paint a chart would
not reach parity with the clients beside it, so the lowering earns its pixels
here. In a host whose output is always handed to some other renderer, it would
not.

## Verifying

```powershell
.\run.ps1                   # cargo fmt --check -> clippy -> build -> test -> wasm build
.\run.ps1 -DriverSemantics  # adds the bounded-loop conformance legs on both targets
```

`tests/client.rs` covers the decode → render → drive loop natively;
`js/driver-semantics.mjs` and `js/placement-abi.mjs` are the `wasm32` legs of the
bounded-loop and placement conformance checks, so the browser target is certified
by **execution**, not merely by the fact that it compiles.
