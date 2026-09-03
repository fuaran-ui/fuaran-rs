// placement-abi.mjs — the `wasm32` leg of the placement C-ABI.
//
// The native leg lives in `tests/placement_abi.rs`. This one certifies the same
// verbs on the target they are also built for, by EXECUTING them there rather
// than by observing that the module compiles.
//
// The placement algebra itself is target-agnostic Rust, so what a second target
// genuinely adds is the ABI: the `(ptr, len)` return form is pointer-width
// dependent — a packed `uint64` here, a two-word struct natively — and that
// split is exactly the thing a native-only gate cannot see. Running the verbs
// here exercises the packed form on every call.
//
// What this script does NOT do is decide anything. There is no expected-order
// arithmetic here, no tree walking, no second implementation of the algebra: it
// reads the requests and the recorded envelopes out of the shared fixture
// (`tests/fixtures/placement-abi.json`, the same file the native leg reads),
// hands each request to the module, and compares the answer byte for byte. A
// comparison written in JavaScript would certify the JavaScript.
//
// Prerequisite, which `run.ps1` arranges:
//
//   cargo build --target wasm32-unknown-unknown --release

import { readFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const WASM = join(REPO, 'target', 'wasm32-unknown-unknown', 'release', 'fuaran_rs.wasm');
const FIXTURE = join(REPO, 'tests', 'fixtures', 'placement-abi.json');

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

// ─── The C-ABI memory contract (see src/ffi/) ────────────────────────────────

const mem = (x) => new Uint8Array(x.memory.buffer);

function writeString(x, str) {
  const bytes = ENCODER.encode(str);
  const ptr = x.fuaran_alloc(bytes.length);
  mem(x).set(bytes, ptr);
  return { ptr, len: bytes.length };
}

/** Read a packed (ptr<<32 | len) return, copy the UTF-8 out, then free it. */
function readPacked(x, packed) {
  const p = BigInt.asUintN(64, BigInt(packed));
  const ptr = Number(p >> 32n);
  const len = Number(p & 0xffffffffn);
  if (len === 0) {
    if (ptr !== 0) x.fuaran_dealloc(ptr, len);
    return '';
  }
  // Copy before dealloc — the freed buffer may be reused by the next call.
  const bytes = mem(x).slice(ptr, ptr + len);
  x.fuaran_dealloc(ptr, len);
  return DECODER.decode(bytes);
}

const VERBS = {
  place: 'fuaran_session_place',
  nudge: 'fuaran_session_nudge',
  duplicate: 'fuaran_session_duplicate',
  paste: 'fuaran_session_paste',
};

function callVerb(x, session, verb, request) {
  const symbol = VERBS[verb];
  if (symbol === undefined) throw new Error(`the fixture names an unknown verb '${verb}'`);
  const { ptr, len } = writeString(x, request);
  const out = readPacked(x, x[symbol](session, ptr, len));
  x.fuaran_dealloc(ptr, len);
  return out;
}

function openSession(x, treeJson) {
  const { ptr, len } = writeString(x, treeJson);
  const session = x.fuaran_session_new(ptr, len);
  x.fuaran_dealloc(ptr, len);
  if (session === 0) {
    throw new Error(`the fixture tree did not decode: ${readPacked(x, x.fuaran_last_error())}`);
  }
  return session;
}

// ─── The run ─────────────────────────────────────────────────────────────────

let fixture;
try {
  fixture = JSON.parse(readFileSync(FIXTURE, 'utf8'));
} catch (e) {
  console.error(`placement-abi (wasm32): the shared fixture could not be read: ${e.message}`);
  process.exit(1);
}

let bytes;
try {
  bytes = readFileSync(WASM);
} catch {
  console.error(
    `placement-abi (wasm32): no module at '${WASM}'. Build it first:\n` +
      '  cargo build --target wasm32-unknown-unknown --release',
  );
  process.exit(1);
}
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
for (const symbol of Object.values(VERBS)) {
  if (typeof x[symbol] !== 'function') {
    console.error(`placement-abi (wasm32): the module exports no ${symbol}.`);
    process.exit(1);
  }
}

const failures = [];
const ran = [];
const session = openSession(x, fixture.tree);
for (const testCase of fixture.cases) {
  ran.push(testCase.name);
  const observed = callVerb(x, session, testCase.verb, testCase.request);
  if (observed !== testCase.expect) {
    failures.push(
      `${testCase.name}:\n  expected ${testCase.expect}\n  observed ${observed}`,
    );
  }
}

// The session ADOPTED every successful edit, and every refusal left the held
// tree untouched.
const finalTree = readPacked(x, x.fuaran_session_tree_json(session));
if (finalTree !== fixture.finalTree) {
  failures.push(`the final tree diverges:\n  expected ${fixture.finalTree}\n  observed ${finalTree}`);
}
x.fuaran_session_free(session);

// The harness's own obligations. A green result means nothing without them.
//
// (1) It ran what the fixture enumerates.
if (ran.length !== fixture.cases.length) {
  failures.push(`ran ${ran.length} case(s); the fixture carries ${fixture.cases.length}`);
}

// (2) A perturbed request makes it go red — proved against a real case, here,
//     on this target, every run.
const probeSession = openSession(x, fixture.tree);
const probeSource = fixture.cases[0];
const probeRequest = probeSource.request.replaceAll('"left"', '"ghost"');
if (probeRequest === probeSource.request) {
  failures.push('the probe perturbation changed nothing, so this probe proves nothing.');
} else {
  const probeVerdict = callVerb(x, probeSession, probeSource.verb, probeRequest);
  if (probeVerdict === probeSource.expect) {
    failures.push(
      'the probe passed: this leg could not detect a request naming an absent parent, so its ' +
        'green means nothing.',
    );
  } else if (!probeVerdict.includes('"code":"ParentNotFound"')) {
    failures.push(`the probe went red for something other than the absent parent: ${probeVerdict}`);
  }
}
x.fuaran_session_free(probeSession);

console.error(`placement-abi (wasm32): ${ran.length} case(s): ${ran.join(', ')}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} failure(s):\n\n${failures.join('\n\n')}`);
  process.exit(1);
}
console.error('placement-abi (wasm32): every recorded envelope reproduced, and the probe went red.');
