# Contributing to fuaran-rs

This repo is licensed **Apache-2.0** (see [`LICENSE`](LICENSE)). Contributions are welcome under
the same licence. The conventions below keep the tree green and the wire format stable.

## Contribution licensing — Developer Certificate of Origin

Every commit must be signed off under the [Developer Certificate of Origin 1.1](https://developercertificate.org/)
to certify you have the right to contribute the code under Apache-2.0. Add a `Signed-off-by:`
trailer to each commit:

```
git commit -s -m "feat: your change"
```

A pull request without DCO sign-off on every commit will not be merged.

## Per-commit hard requirements

1. **`pwsh ./run.ps1` is green** — the one-command gate: format check, build, and the full test
   suite in one pass.
2. **Formatting** — run `cargo fmt` before every commit. Unformatted code is not mergeable.
3. **Conformance** — wire-surface changes must keep the bundled conformance corpus green (round-trip, reject, and lenient-accept families). The corpus is canonical upstream — corpus updates arrive as corpus-sync changes, never hand-edits to fixtures.

## Pull request flow

1. Branch from `main` with a descriptive name (`feat/<short-name>`, `fix/<short-name>`,
   `docs/<short-name>`).
2. Make focused, DCO-signed commits. Group related changes; do not bundle unrelated cleanups.
3. Run the per-commit hard requirements above.
4. Open a PR describing the change and its wire-format impact.
5. A maintainer reviews and merges.
