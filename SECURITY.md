# Security Policy

## Supported versions

Only the latest released version is supported. Fixes ship in a new release; older versions are
not patched.

| Version | Supported |
| ------- | --------- |
| 0.3.x   | ✅        |
| < 0.3   | ❌        |

## Reporting a vulnerability

**Do not open a public issue.** Report privately via GitHub Security Advisories:

<https://github.com/juninmd/zplint/security/advisories/new>

Please include the affected version, a reproducer (the `.sma` or `.amxx` input, ideally
minimized), and the impact you observed. Expect an acknowledgement within 7 days.

## Threat model

zplint parses and compiles **untrusted input**: `.sma` sources and `.amxx` binaries downloaded
from plugin forums. The following are in scope:

- Memory-unsafety, panics, or unbounded allocation reachable from a crafted `.sma` or `.amxx`
  (a panic that aborts a lint run is a bug; a hang or OOM is a security issue).
- Path traversal through `#include` resolution or the file discovery walker.
- Code generation that emits a `.amxx` violating AMX bounds checks in a way that could
  compromise a game server.
- Writing outside the intended output directory during `compile` or `fix`.

Out of scope:

- False positives and false negatives in lint rules (open a normal issue).
- Divergences from the reference `amxxpc` compiler that do not cross a trust boundary — see
  `docs/DIVERGENCES.md`.
- Vulnerabilities in AMX Mod X, the HLDS engine, or third-party plugins themselves.

## Dependencies

`cargo audit` runs on every push and pull request via `.github/workflows/ci.yml`. Advisories
affecting a dependency are treated as issues against zplint.
