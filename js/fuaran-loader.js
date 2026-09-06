// fuaran-loader.js — the thin, dependency-free JS loader for the fuaran-rs
// `wasm32` client module.
//
// This is a HAND-WRITTEN generic loader, NOT an npm dependency: it instantiates
// the WebAssembly module, marshals UTF-8 strings across the linear-memory
// boundary per the C-ABI memory contract (src/client/wasm.rs), and hands back a
// `FuaranSession` that decodes → renders → drives a Fuaran UI wire tree entirely
// client-side. No wasm-bindgen, no framework.
//
// The emission surface is the canonical JSON wire format for every host; this
// loader mounts the module's own server-parity HTML render into the DOM. The
// module renders inert HTML (no event handlers ride the wire — §4); interactivity
// is driven by writing the reactive stores (setState / setFilter) and
// re-rendering, exactly as the wire's write-back default prescribes. Auto-wiring
// a control's DOM event to its store slot is app-specific (the parity-locked
// render carries no slot attribute), so this loader stays generic and leaves the
// event→store mapping to the app — see js/index.html for the pattern.

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

/** Read the module's (possibly grown) linear memory as a byte view. */
function mem(exports) {
  return new Uint8Array(exports.memory.buffer);
}

/** Copy a JS string into a fresh module-owned input buffer; returns {ptr, len}.
 *
 *  The NULL check is load-bearing, not defensive noise. `fuaran_alloc` is
 *  fallible and returns 0 when the request cannot be satisfied — which is what
 *  the C header has always promised and what the module now actually does. In
 *  linear memory, address 0 is a VALID offset: writing there would silently
 *  scribble over the module's own low memory and corrupt the session rather
 *  than fail. So an unsatisfiable request is refused here, loudly, at the one
 *  place that would otherwise do the writing. */
function writeString(exports, str) {
  const bytes = ENCODER.encode(str);
  const ptr = exports.fuaran_alloc(bytes.length);
  if (ptr === 0 && bytes.length > 0) {
    throw new Error(
      `fuaran: could not allocate ${bytes.length} bytes in the module's memory`,
    );
  }
  mem(exports).set(bytes, ptr);
  return { ptr, len: bytes.length };
}

/** Read a packed (ptr<<32 | len) return, copy the UTF-8 out, then free it. */
function readPacked(exports, packed) {
  const p = BigInt.asUintN(64, packed);
  const ptr = Number(p >> 32n);
  const len = Number(p & 0xffffffffn);
  if (len === 0) {
    if (ptr !== 0) exports.fuaran_dealloc(ptr, len);
    return '';
  }
  // Copy before dealloc — the freed buffer may be reused by the next call.
  const bytes = mem(exports).slice(ptr, ptr + len);
  exports.fuaran_dealloc(ptr, len);
  return DECODER.decode(bytes);
}

/** A recoverable client error surfaced by the module's JSON envelope. */
export class FuaranClientError extends Error {
  constructor(envelope) {
    super(envelope?.error?.message ?? 'fuaran client error');
    this.name = 'FuaranClientError';
    this.envelope = envelope;
    this.code = envelope?.error?.code;
    this.class = envelope?.error?.class;
  }
}

/** A live client session over one decoded wire tree. */
export class FuaranSession {
  constructor(exports, handle) {
    this._x = exports;
    this._handle = handle;
  }

  /** The current tree rendered to a body-fragment HTML string. */
  render() {
    return readPacked(this._x, this._x.fuaran_session_render(this._handle));
  }

  /** The current tree, re-encoded to canonical wire JSON. */
  treeJson() {
    return readPacked(this._x, this._x.fuaran_session_tree_json(this._handle));
  }

  /** Apply a canonical wire `TreeOp` (a JS object or JSON string). Throws a
   *  `FuaranClientError` on a structured apply / decode failure. */
  applyOp(op) {
    return this._mutate('fuaran_session_apply_op', typeof op === 'string' ? op : JSON.stringify(op));
  }

  /** Write a reactive `$state.<key>` slot from a JSON value (object/primitive). */
  setState(key, value) {
    return this._store('fuaran_session_set_state', key, value);
  }

  /** Write a `$filters.<name>` slot. */
  setFilter(name, value) {
    return this._store('fuaran_session_set_filter', name, value);
  }

  /** Seed a `$queries.<name>` result slot (host-fed data). */
  setQuery(name, value) {
    return this._store('fuaran_session_set_query', name, value);
  }

  /** Render into a DOM element (sets innerHTML to the current render). */
  mount(el) {
    el.innerHTML = this.render();
  }

  /** Free the session's module-side memory. Idempotent. */
  free() {
    if (this._handle !== 0) {
      this._x.fuaran_session_free(this._handle);
      this._handle = 0;
    }
  }

  _mutate(fnName, json) {
    // The dealloc is in a `finally`. Without it, a module-side trap — the exact
    // case the guest is untrusted for — unwound past the free and leaked the
    // input buffer inside the module's linear memory, which nothing on the JS
    // side can reclaim afterwards. A page driving a session in a loop lost that
    // memory permanently, once per trapping call.
    const { ptr, len } = writeString(this._x, json);
    let result;
    try {
      result = readPacked(this._x, this._x[fnName](this._handle, ptr, len));
    } finally {
      this._x.fuaran_dealloc(ptr, len);
    }
    const parsed = JSON.parse(result);
    if (parsed.error) throw new FuaranClientError(parsed);
    return parsed;
  }

  _store(fnName, key, value) {
    // JSON.stringify on both branches of the ternary: the condition decided
    // nothing, so it read as a deliberate distinction where none existed, and a
    // reader had to prove that to themselves before touching the line. A string
    // value IS stringified — the module expects a JSON document, so a bare
    // string must arrive quoted — which is what the dead branch was accidentally
    // right about.
    const k = writeString(this._x, key);
    const v = writeString(this._x, JSON.stringify(value));
    let result;
    try {
      result = readPacked(this._x, this._x[fnName](this._handle, k.ptr, k.len, v.ptr, v.len));
    } finally {
      this._x.fuaran_dealloc(k.ptr, k.len);
      this._x.fuaran_dealloc(v.ptr, v.len);
    }
    const parsed = JSON.parse(result);
    if (parsed.error) throw new FuaranClientError(parsed);
    return parsed;
  }
}

/** Instantiate the fuaran-rs client module from a `.wasm` URL. Returns the raw
 *  exports; use `createSession` to open a session over a wire tree. */
export async function loadFuaran(wasmUrl) {
  let instance;
  try {
    ({ instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), {}));
  } catch {
    // Fallback for servers without the correct `application/wasm` MIME type.
    const bytes = await fetch(wasmUrl).then((r) => r.arrayBuffer());
    ({ instance } = await WebAssembly.instantiate(bytes, {}));
  }
  return instance.exports;
}

/** Decode a canonical wire `Node` JSON (a JS object or JSON string) into a live
 *  session. Throws a `FuaranClientError` when the tree fails to decode. */
export function createSession(exports, tree) {
  const json = typeof tree === 'string' ? tree : JSON.stringify(tree);
  const { ptr, len } = writeString(exports, json);
  const handle = exports.fuaran_session_new(ptr, len);
  exports.fuaran_dealloc(ptr, len);
  if (handle === 0) {
    const envelope = JSON.parse(readPacked(exports, exports.fuaran_last_error()) || '{}');
    throw new FuaranClientError(envelope);
  }
  return new FuaranSession(exports, handle);
}
