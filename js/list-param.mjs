// list-param.mjs — the `wasm32` leg of the list-valued Transform param rule.
//
// The native leg lives in `tests/list_param_abi.rs`. This one certifies the same
// behaviour on the target this crate's OTHER host runs on, by EXECUTING it there
// rather than by observing that the module compiles.
//
// Why a second target matters here, when the rule itself is target-agnostic Rust:
// this crate is two hosts in one, and the browser-native client is the one whose
// answer the native Swift and Kotlin surfaces inherit — they are decode-only
// render projections and never evaluate a pipeline themselves. So "the server
// path does the right thing" is not the claim that needs making; "the client
// path, reached over the ABI, does the same right thing" is. The `(ptr, len)`
// return form is pointer-width dependent (a packed `uint64` here, a two-word
// struct natively) and that split is exactly what a native-only gate cannot see.
//
// What this script does NOT do is decide anything. There is no expected-row
// arithmetic here, no pipeline evaluation, no second implementation of the rule:
// it reads the selections and the recorded envelopes out of the shared fixture
// (`tests/fixtures/list-param.json`, the same file the native leg reads), hands
// each to the module, and compares byte for byte. A comparison written in
// JavaScript would certify the JavaScript.
//
// Prerequisite, which `run.ps1` arranges:
//
//   cargo build --target wasm32-unknown-unknown --release

import { readFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const WASM = join(REPO, 'target', 'wasm32-unknown-unknown', 'release', 'fuaran_rs.wasm');
const FIXTURE = join(REPO, 'tests', 'fixtures', 'list-param.json');

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
  'fuaran_session_set_filter',
  'fuaran_session_resolved_rows',
  'fuaran_last_error',
];

function openSession(x, treeJson) {
  const { ptr, len } = writeString(x, treeJson);
  const session = x.fuaran_session_new(ptr, len);
  x.fuaran_dealloc(ptr, len);
  if (session === 0) {
    throw new Error(`the fixture tree did not decode: ${readPacked(x, x.fuaran_last_error())}`);
  }
  return session;
}

function setFilter(x, session, name, value) {
  const k = writeString(x, name);
  const v = writeString(x, value);
  const out = readPacked(x, x.fuaran_session_set_filter(session, k.ptr, k.len, v.ptr, v.len));
  x.fuaran_dealloc(k.ptr, k.len);
  x.fuaran_dealloc(v.ptr, v.len);
  if (out !== '{"ok":true}') throw new Error(`the store write failed: ${out}`);
}

function resolvedRows(x, session, node) {
  const { ptr, len } = writeString(x, node);
  const out = readPacked(x, x.fuaran_session_resolved_rows(session, ptr, len));
  x.fuaran_dealloc(ptr, len);
  return out;
}

/**
 * One case: a fresh session, an optional selection written to the filter store,
 * then the grid's resolved rows. Fresh per case deliberately — a carried-over
 * selection would make the cases order-dependent, and "nothing selected" is a
 * state a shared session could never return to.
 */
function runCase(x, fixture, selection) {
  const session = openSession(x, fixture.tree);
  try {
    if (selection !== null && selection !== undefined) {
      setFilter(x, session, fixture.filter, selection);
    }
    return resolvedRows(x, session, fixture.node);
  } finally {
    x.fuaran_session_free(session);
  }
}

// ─── The run ─────────────────────────────────────────────────────────────────

let fixture;
try {
  fixture = JSON.parse(readFileSync(FIXTURE, 'utf8'));
} catch (e) {
  console.error(`list-param (wasm32): the shared fixture could not be read: ${e.message}`);
  process.exit(1);
}

let bytes;
try {
  bytes = readFileSync(WASM);
} catch {
  console.error(
    `list-param (wasm32): no module at '${WASM}'. Build it first:\n` +
      '  cargo build --target wasm32-unknown-unknown --release',
  );
  process.exit(1);
}
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
for (const symbol of SYMBOLS) {
  if (typeof x[symbol] !== 'function') {
    console.error(`list-param (wasm32): the module exports no ${symbol}.`);
    process.exit(1);
  }
}

const failures = [];
const ran = [];
for (const testCase of fixture.cases) {
  ran.push(testCase.name);
  const observed = runCase(x, fixture, testCase.selection);
  if (observed !== testCase.expect) {
    failures.push(`${testCase.name}:\n  expected ${testCase.expect}\n  observed ${observed}`);
  }
}

// The harness's own obligations. A green result means nothing without them.
//
// (1) It ran what the fixture enumerates.
if (ran.length !== fixture.cases.length) {
  failures.push(`ran ${ran.length} case(s); the fixture carries ${fixture.cases.length}`);
}

// (2) A perturbed selection makes it go red — proved against a real case, here,
//     on this target, every run. The perturbation is a selection naming NO
//     department, whose answer is the EMPTY table: the one comparison that
//     separates "nothing selected" (no constraint, every row) from "a constraint
//     no row satisfies" (no rows), which is the whole of behaviour 2.
const probe = fixture.probe;
const probed = fixture.cases.find((c) => c.name === probe.case);
if (probed === undefined) {
  failures.push(`the probe names case '${probe.case}', which the fixture does not carry.`);
} else if (probe.selection === probed.selection) {
  failures.push('the probe perturbation changed nothing, so this probe proves nothing.');
} else {
  const verdict = runCase(x, fixture, probe.selection);
  if (verdict === probed.expect) {
    failures.push(
      'the probe passed: this leg could not detect a selection naming no department, so its ' +
        'green means nothing.',
    );
  } else if (verdict !== probe.expect) {
    failures.push(
      `the probe went red for the wrong reason:\n  expected ${probe.expect}\n  observed ${verdict}`,
    );
  }
}

console.error(`list-param (wasm32): ${ran.length} case(s): ${ran.join(', ')}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} failure(s):\n\n${failures.join('\n\n')}`);
  // `process.exitCode` rather than `process.exit(1)`: node exits naturally once
  // this module settles, where an immediate exit while the WebAssembly instance
  // is still live aborts in libuv on Windows and reports 127 instead of 1 — a
  // failure that is harder to read than the failure it is reporting.
  process.exitCode = 1;
} else {
  console.error('list-param (wasm32): every recorded envelope reproduced, and the probe went red.');
}
