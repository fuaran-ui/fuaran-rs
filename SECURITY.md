# Security Policy

## Supported versions

The `fuaran-rs` crate is pre-1.0. Security fixes are applied to the latest released `0.x` version on the
`main` branch. Older pre-releases are not maintained.

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

## Reporting a vulnerability

Please report suspected vulnerabilities privately — do **not** open a public issue.

- **Preferred:** GitHub's private vulnerability reporting for this repository (the repository's
  **Security** tab → **Report a vulnerability**). It is visible only to the maintainers, and it is
  where we reply, share a draft fix, and publish the advisory from.
- **Or email:** andrew@fuaran.com — include a description, the affected version, and steps to
  reproduce.

A useful report names the version you tested, the input or sequence that triggers the behaviour,
and what you believe the impact is. A proof of concept helps and is never required.

We aim to acknowledge a report within five business days. There is no bounty programme, and
nothing to sign: we do not ask reporters to accept terms in exchange for a response.

## What happens after you report

The same process applies in every repository of this project.

1. **Acknowledgement** within five business days, saying whether we have reproduced it yet.
2. **Triage.** A maintainer reproduces the report and settles two questions: whether it is a
   defect in this project's own code, and which of this project's published packages are affected.
   The second is not answered from the reporting repository alone — see the cross-host note below.
3. **Fix**, landing with a regression test that fails without it. Where the defect is in a
   guarantee this project documents, the document stating that guarantee is corrected in the same
   change.
4. **Release.** Every affected package gets a released version carrying the fix.
5. **Advisory**, published on each affected repository, requesting a CVE where one is warranted,
   crediting the reporter by whatever name they choose — or not at all, if they prefer.

**How affected versions are stated.** These packages are released independently of one another,
and consumers pin exact versions rather than floating ranges, so an advisory that says "upgrade to
the latest" is not actionable here. Each advisory therefore states, per published package:

- the registry id of the package, and the affected versions as an explicit range of versions that
  were actually published to that public registry — never "all earlier versions", and never a
  version that exists only in a development build, since no consumer can be on one;
- the first released version that carries the fix;
- whether the package is affected **directly** (the defect is in its own code) or **transitively**
  (it pins an affected version of another package in this project). A pinned consumer does not
  pick up a fixed dependency by upgrading nothing, so a transitively affected package gets its own
  fixed release and its own entry in the advisory, rather than a note telling the reader to go and
  upgrade something else.

**One defect can affect several of these repositories at once.** This project ships parallel
implementations of one wire format in several languages, written against a shared specification
and a shared conformance corpus rather than transpiled from one another. A defect in how one host
decodes, renders, or gates may therefore exist in the others or may not, and neither can be
assumed. Before an advisory is published, the same defect is looked for in every host, and the
advisory names every affected package across every language. Where a host is **not** affected, the
advisory says so explicitly: silence about a host reads as "unknown", which is the one thing an
advisory must never leave a consumer holding.

## Reports about a dependency or another project

Not every report is about code this project owns, and the handling differs.

- **A defect in one of our dependencies.** We do not publish it. It belongs to that project's own
  disclosure channel, and we will forward it there with your consent, or ask you to report it
  there yourself if you would rather hold the relationship. We honour that project's embargo. If
  the impact on our side can be mitigated without revealing the defect, we ship that mitigation
  during the embargo and describe it in neutral terms; if any honest mitigation would disclose the
  defect, we wait — and we tell you that we are waiting, and why.
- **A defect in an application built on these packages.** Host-supplied code runs with the host's
  own trust, so its issues belong with that application rather than here. If the host was
  following our documentation and our documentation was wrong, that is our defect and we take it.
- **A report that is already public when it reaches us.** The embargo question is then moot, and
  we will say so: we ship and publish as fast as we can, rather than ask anyone to un-say
  something.
- **Our own default window.** Where the defect is ours we propose a disclosure window at the
  acknowledgement rather than leaving it open — 90 days from that acknowledgement unless we agree
  something else with you, and sooner if the fix ships sooner. If we go quiet, or miss the window
  we proposed, publishing is your call and we will not treat it as a breach of anything.

## What is out of scope

- Findings that require an already-compromised operating system, browser binary, build machine, or
  package-registry account.
- Issues in an application that consumes these packages, including custom code that a host
  registers and that runs with the host's own trust — see the section above.
- Vulnerabilities in a third-party dependency: we will forward them, but the advisory is that
  project's to publish.
- Reports against a site or deployment this project does not operate.
- Automated scanner output with no demonstrated impact on this project's code.
- Missing hardening that is a documented deployment choice left to the host rather than a defect
  in the code here.
