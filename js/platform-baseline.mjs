// platform-baseline.mjs — the `wasm32` leg of the platform-baseline capability
// wave: Media text tracks, Embed, the node-level tooltip trait, Combobox, Tree.
//
// The native leg lives in `tests/platform_baseline_abi.rs`. This one certifies the
// same behaviour on the target this crate's OTHER host runs on, by EXECUTING it
// there rather than by observing that the module compiles.
//
// Why a second target matters here, when the vocabulary itself is target-agnostic
// Rust: this crate is two hosts in one, and the browser-native client is the one
// whose answer the native Swift and Kotlin surfaces inherit — they are decode-only
// render projections and never decode a document or emit markup themselves. So
// "the server path renders a Tree correctly" is not the claim that needs making;
// "the client path, reached over the ABI, produces the same bytes" is. The
// `(ptr, len)` return form is pointer-width dependent (a packed `uint64` here, a
// two-word struct natively) and that split is exactly what a native-only gate
// cannot see.
//
// What this script does NOT do is decide anything. There is no markup knowledge
// here, no second renderer, no expectation computed in JavaScript: it reads the
// trees, the store writes and the recorded expectations out of the shared fixture
// (`tests/fixtures/platform-baseline.json`, the same file the native leg reads),
// hands each to the module, and compares. A comparison written in JavaScript would
// certify the JavaScript.
//
// Prerequisite, which `run.ps1` arranges:
//
//   cargo build --target wasm32-unknown-unknown --release

import { readFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const WASM = join(REPO, 'target', 'wasm32-unknown-unknown', 'release', 'fuaran_rs.wasm');
const FIXTURE = join(REPO, 'tests', 'fixtures', 'platform-baseline.json');

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

const SYMBOLS = [
  'fuaran_alloc',
  'fuaran_dealloc',
  'fuaran_session_new',
  'fuaran_session_free',
  'fuaran_session_set_state',
  'fuaran_session_render',
  'fuaran_session_tree_json',
  'fuaran_last_error',
];

function openSession(x, treeJson) {
  const { ptr, len } = writeString(x, treeJson);
  const session = x.fuaran_session_new(ptr, len);
  x.fuaran_dealloc(ptr, len);
  if (session === 0) {
    throw new Error(
      `the tree did not decode over the ABI — this vocabulary does not reach a ` +
        `native surface at all: ${readPacked(x, x.fuaran_last_error())}`,
    );
  }
  return session;
}

function setState(x, session, key, value) {
  const k = writeString(x, key);
  const v = writeString(x, value);
  const out = readPacked(x, x.fuaran_session_set_state(session, k.ptr, k.len, v.ptr, v.len));
  x.fuaran_dealloc(k.ptr, k.len);
  x.fuaran_dealloc(v.ptr, v.len);
  if (out !== '{"ok":true}') throw new Error(`the store write failed: ${out}`);
}

/**
 * One case, start to finish over the ABI: open a session on the document, write
 * any declared store slots, then take BOTH observations — the re-encoded tree
 * and the rendered markup.
 *
 * Both, because they answer different questions and a native surface needs each:
 * the tree JSON is what a projection re-serialises (so it is where a dropped
 * field shows up), and the render is what it paints (so it is where a dropped
 * OBLIGATION shows up). A wave that landed the codec and not the renderer passes
 * the first and fails the second.
 */
function runCase(x, testCase) {
  const session = openSession(x, testCase.tree);
  try {
    for (const write of testCase.state ?? []) {
      setState(x, session, write.key, write.value);
    }
    return {
      tree: readPacked(x, x.fuaran_session_tree_json(session)),
      html: readPacked(x, x.fuaran_session_render(session)),
    };
  } finally {
    x.fuaran_session_free(session);
  }
}

/** Assert `needles` appear in `haystack` in the given ORDER. */
function orderFailure(haystack, needles) {
  let last = 0;
  for (const needle of needles) {
    const at = haystack.indexOf(needle);
    if (at < 0) return `'${needle}' is absent`;
    if (at < last) return `'${needle}' appears out of the required order`;
    last = at;
  }
  return null;
}

// ─── The run ─────────────────────────────────────────────────────────────────

let fixture;
try {
  fixture = JSON.parse(readFileSync(FIXTURE, 'utf8'));
} catch (e) {
  console.error(`platform-baseline (wasm32): the shared fixture could not be read: ${e.message}`);
  process.exit(1);
}

let bytes;
try {
  bytes = readFileSync(WASM);
} catch {
  console.error(
    `platform-baseline (wasm32): no module at '${WASM}'. Build it first:\n` +
      '  cargo build --target wasm32-unknown-unknown --release',
  );
  process.exit(1);
}
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
for (const symbol of SYMBOLS) {
  if (typeof x[symbol] !== 'function') {
    console.error(`platform-baseline (wasm32): the module exports no ${symbol}.`);
    process.exit(1);
  }
}

const failures = [];
const ran = [];
for (const testCase of fixture.cases) {
  ran.push(testCase.name);
  const { tree, html } = runCase(x, testCase);

  if (tree !== testCase.expectTree) {
    failures.push(
      `${testCase.name}: the tree a native surface re-serialises diverges\n` +
        `  expected ${testCase.expectTree}\n  observed ${tree}`,
    );
  }
  for (const needle of testCase.expectRenderContains ?? []) {
    if (!html.includes(needle)) {
      failures.push(`${testCase.name}: the rendered markup is missing '${needle}'\n  ${html}`);
    }
  }
  for (const needle of testCase.expectRenderAbsent ?? []) {
    if (html.includes(needle)) {
      failures.push(`${testCase.name}: the rendered markup must NOT carry '${needle}'\n  ${html}`);
    }
  }
  const bad = orderFailure(html, testCase.expectOrder ?? []);
  if (bad !== null) failures.push(`${testCase.name}: ${bad}\n  ${html}`);
}

// The harness's own obligations. A green result means nothing without them.
//
// (1) It ran what the fixture enumerates, and the fixture covers the wave.
if (ran.length !== fixture.cases.length) {
  failures.push(`ran ${ran.length} case(s); the fixture carries ${fixture.cases.length}`);
}
if (fixture.cases.length < 5) {
  failures.push(
    `the wave has five capabilities; the fixture carries ${fixture.cases.length} case(s)`,
  );
}

// (2) Every case declared enough to certify anything. A mis-keyed expectation
//     list reads as an EMPTY list and every loop above becomes vacuously green —
//     the failure shape a fixture-driven gate is most prone to and least likely
//     to be noticed for.
for (const testCase of fixture.cases) {
  const n = (testCase.expectRenderContains ?? []).length;
  if (n < 3) {
    failures.push(`${testCase.name}: only ${n} render expectation(s) — too few to certify anything`);
  }
}

// (3) A perturbed expectation makes it go red — proved against a real case, here,
//     on this target, every run. A recorded-envelope comparison whose recorder is
//     the code under test is worth nothing without one.
const first = fixture.cases[0];
const perturbed = `${first.expectTree} `;
{
  const { tree } = runCase(x, first);
  if (tree === perturbed) {
    failures.push('the go-red probe perturbation changed nothing, so this probe proves nothing.');
  } else if (tree !== first.expectTree) {
    failures.push(`the go-red probe found the unperturbed case already failing: ${first.name}`);
  }
}

if (failures.length > 0) {
  console.error('platform-baseline (wasm32): FAILED\n');
  for (const f of failures) console.error(`  ${f}\n`);
  process.exit(1);
}

console.log(
  `platform-baseline (wasm32): OK — ${ran.length} case(s) over the C-ABI ` +
    `(${ran.join(', ')}), tree bytes + rendered markup certified on wasm32.`,
);
