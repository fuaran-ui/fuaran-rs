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

/** Copy a JS string into a fresh module-owned input buffer; returns {ptr, len}. */
function writeString(exports, str) {
  const bytes = ENCODER.encode(str);
  const ptr = exports.fuaran_alloc(bytes.length);
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
    const { ptr, len } = writeString(this._x, json);
    const result = readPacked(this._x, this._x[fnName](this._handle, ptr, len));
    this._x.fuaran_dealloc(ptr, len);
    const parsed = JSON.parse(result);
    if (parsed.error) throw new FuaranClientError(parsed);
    return parsed;
  }

  _store(fnName, key, value) {
    const k = writeString(this._x, key);
    const v = writeString(this._x, typeof value === 'string' ? JSON.stringify(value) : JSON.stringify(value));
    const result = readPacked(this._x, this._x[fnName](this._handle, k.ptr, k.len, v.ptr, v.len));
    this._x.fuaran_dealloc(k.ptr, k.len);
    this._x.fuaran_dealloc(v.ptr, v.len);
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
