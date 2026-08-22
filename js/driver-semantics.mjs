// driver-semantics.mjs — the `wasm32` leg of the bounded loop's conformance
// check.
//
// The native leg lives in `tests/driver_semantics.rs`. This one certifies the
// same loop on the target it is also built for, by EXECUTING it there rather
// than by observing that it compiles.
//
// What this script does NOT do is compare anything. There is no second
// comparison here — no tree normalisation, no effect matching, no divergence
// arithmetic. It reads a scenario's three documents off disk, hands them to the
// module, and reports the verdict the module returns; the comparison is the
// crate's own `bounded::trace` code compiled for `wasm32`. A comparison written
// in JavaScript would certify the JavaScript, which is not the thing under test.
//
// Prerequisites, both of which `run.ps1 -DriverSemantics` arranges:
//
//   cargo build --target wasm32-unknown-unknown --release --features driver-semantics-abi
//   FUARAN_PROGRAM_SPEC=<the program wire specification's directory>
//
// The corpus is not a sibling this repository's public workflow checks out, so
// this is a local, operator-invoked gate. Where a corpus is claimed and cannot
// be read, this script FAILS: a conformance check that passes without its oracle
// reports the same green as one that ran.

import { readFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const WASM = join(REPO, 'target', 'wasm32-unknown-unknown', 'release', 'fuaran_rs.wasm');
const FAMILY = 'driver-semantics';
const DECLARED_OBLIGATION = 'bounded-loop';

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

/** Resolve the scenario corpus: declared, or found beside this repository. */
function fixturesRoot() {
  const declared = process.env.FUARAN_PROGRAM_SPEC;
  if (declared && declared.trim() !== '') {
    return join(declared, 'wire-fixtures');
  }
  let dir = REPO;
  for (;;) {
    const candidate = join(dir, 'fuaran-program-spec', 'wire-fixtures');
    try {
      readFileSync(join(candidate, 'manifest.json'));
      return candidate;
    } catch {
      const parent = dirname(dir);
      if (parent === dir) return null;
      dir = parent;
    }
  }
}

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

function checkScenario(x, request) {
  const { ptr, len } = writeString(x, JSON.stringify(request));
  const result = readPacked(x, x.fuaran_bounded_check_scenario(ptr, len));
  x.fuaran_dealloc(ptr, len);
  return JSON.parse(result);
}

// ─── The run ─────────────────────────────────────────────────────────────────

const fixtures = fixturesRoot();
if (fixtures === null) {
  console.error(
    'driver-semantics (wasm32): NOT RUN — no scenario corpus was claimed or found, so this leg ' +
      'asserted nothing. Set FUARAN_PROGRAM_SPEC to the program wire specification\'s directory, ' +
      'or check it out beside this repository, to run it.',
  );
  process.exit(0);
}

let manifest;
try {
  manifest = JSON.parse(readFileSync(join(fixtures, 'manifest.json'), 'utf8'));
} catch (e) {
  console.error(
    `driver-semantics (wasm32): the scenario corpus is claimed at '${fixtures}' but its manifest ` +
      `could not be read: ${e.message}. This leg fails rather than skipping, deliberately.`,
  );
  process.exit(1);
}

if (!(manifest.scenarioFamilies ?? []).includes(FAMILY)) {
  console.error(`driver-semantics (wasm32): the corpus enumerates no '${FAMILY}' scenario family.`);
  process.exit(1);
}

// The manifest is the authoritative enumeration, never a directory listing: a
// scenario on disk but absent from it is behaviour nobody is required to
// reproduce, while every host still reports full conformance.
const scenarios = (manifest.scenarios ?? []).filter((s) => s.family === FAMILY);
if (scenarios.length === 0) {
  console.error(`driver-semantics (wasm32): the corpus enumerates no ${FAMILY} scenario.`);
  process.exit(1);
}

let bytes;
try {
  bytes = readFileSync(WASM);
} catch {
  console.error(
    `driver-semantics (wasm32): no module at '${WASM}'. Build it first:\n` +
      '  cargo build --target wasm32-unknown-unknown --release --features driver-semantics-abi',
  );
  process.exit(1);
}
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
if (typeof x.fuaran_bounded_check_scenario !== 'function') {
  console.error(
    'driver-semantics (wasm32): the module exports no fuaran_bounded_check_scenario — it was ' +
      'built without --features driver-semantics-abi.',
  );
  process.exit(1);
}

const failures = [];
const ran = [];
for (const scenario of scenarios) {
  // A scenario presuming an obligation this host has not declared is refused by
  // name rather than silently run: the field exists so a second obligation
  // ENUMERATES rather than renumbers.
  if (scenario.requires !== DECLARED_OBLIGATION) {
    failures.push(
      `${scenario.name}: presumes the '${scenario.requires}' obligation, which this host has not declared.`,
    );
    continue;
  }
  const request = {
    name: scenario.name,
    tree: readFileSync(join(fixtures, scenario.files.tree), 'utf8'),
    events: readFileSync(join(fixtures, scenario.files.events), 'utf8'),
    expectation: readFileSync(join(fixtures, scenario.files.expectation), 'utf8'),
  };
  ran.push(scenario.name);
  const verdict = checkScenario(x, request);
  if (verdict.divergence !== undefined) failures.push(verdict.divergence);
  else if (verdict.error !== undefined) failures.push(`${scenario.name}: ${verdict.error}`);
  else if (verdict.ok === undefined) failures.push(`${scenario.name}: unreadable verdict ${JSON.stringify(verdict)}`);
}

// The harness's own obligations. A green result means nothing without them.
//
// (1) It ran what the manifest enumerates.
if (ran.length !== scenarios.length) {
  failures.push(`ran ${ran.length} scenario(s); the manifest enumerates ${scenarios.length}`);
}

// (2) A mutated trace makes it go red — proved against a real scenario, here,
//     on this target, every run.
const probeSource = scenarios[0];
const probe = {
  name: `${probeSource.name} (probe)`,
  tree: readFileSync(join(fixtures, probeSource.files.tree), 'utf8'),
  events: readFileSync(join(fixtures, probeSource.files.events), 'utf8'),
  // A change the decoder ACCEPTS — renaming a node, rather than naming a case
  // that does not exist. A perturbation that failed to decode would be caught by
  // the decoder, which is a different check passing under this one's name.
  expectation: readFileSync(join(fixtures, probeSource.files.expectation), 'utf8').replaceAll(
    '"id": "root"',
    '"id": "rooted"',
  ).replaceAll('"id":"root"', '"id":"rooted"'),
};
const probeVerdict = checkScenario(x, probe);
if (probeVerdict.divergence === undefined) {
  failures.push(
    'the probe passed: this leg could not detect a mutated trace, so its green means nothing.',
  );
}

console.error(
  `driver-semantics (wasm32): ${ran.length} scenario(s) from '${fixtures}': ${ran.join(', ')}`,
);
if (failures.length > 0) {
  console.error(`\n${failures.length} failure(s):\n\n${failures.join('\n\n')}`);
  process.exit(1);
}
console.error('driver-semantics (wasm32): every scenario reproduced, and the probe went red.');
