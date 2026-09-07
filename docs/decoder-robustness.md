# Decoder robustness: the long-run record

`decoder-robustness.json` beside this file is **generated** by this host's own fuzz leg and
rewritten in full by the run that produces it; do not hand-edit it. It records one long run:
the seed, the iteration count, how many generated inputs each decode entry point saw, the
refusal codes they answered with, the largest single decode time, the worst allocation
amplification, and the number of counterexamples (an escaping panic, a hang, a budget breach,
or an accepted input whose canonical form does not re-decode). Zero counterexamples is the
claim under test: decoding is total over hostile input.

Regenerate it with the command below from the repository root, replacing `<seed>` (the
committed record names the seed it was produced with, so the same stream can be replayed):

```
FUARAN_FUZZ_LONG=1 FUARAN_FUZZ_ITERATIONS=50000 FUARAN_FUZZ_SEED=<seed> FUARAN_FUZZ_EVIDENCE=docs/decoder-robustness.json cargo test --test decoder_fuzz the_refusal_contract_holds -- --nocapture
```

The bounded form of the same leg runs on every pull request with a fixed seed. The five
input families and the four invariants are those of the reference host's harness; the
generator is a sibling per host rather than one shared byte stream, so two hosts' records
are comparable by classification and not by identical inputs. This host measures allocation
exactly, with a per-thread counting allocator, which the other hosts approximate.

The first run on this seed found six wide-object inputs decoding in fifteen to sixteen
seconds, the duplicate-member scan being quadratic in width; the committed record is the
run after that fix, on the same seed.
