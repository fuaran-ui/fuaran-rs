# Security Policy

## Supported versions

The `fuaran-rs` crate is pre-1.0. Security fixes are applied to the latest released `0.x` version on the
`main` branch. Older pre-releases are not maintained.

## Reporting a vulnerability

Please report suspected vulnerabilities privately — do **not** open a public issue.

- **Preferred:** GitHub's private vulnerability reporting (the repository's **Security** tab →
  **Report a vulnerability**).
- **Or email:** security@fuaran.com — include a description, the affected version, and steps
  to reproduce.

We aim to acknowledge a report within five business days and to agree a disclosure timeline with
you. Please allow a reasonable window to ship a fix before any public disclosure.

## Scope

This repo is the Rust host of the Fuaran UI wire format: a headless/edge tier and a
browser-native WASM client. It decodes wire JSON — often AI-emitted — and renders markup.

- **Wire decoding:** a decode path that admits malformed wire as valid, or parser resource
  exhaustion (unbounded depth or size), is in scope.
- **Emitted-markup injection safety:** tree content must never escape into markup as script or
  active content, in either the server-side or the WASM-client render path.
- **C-ABI surface:** the exported `staticlib` C-ABI is consumed by the native Swift/Kotlin
  surfaces — memory-safety defects reachable through it (use-after-free, buffer over-read on
  malformed input) are in scope.
