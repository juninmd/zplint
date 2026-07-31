# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `LICENSE` (Apache-2.0) and `NOTICE` at the repository root, plus a copy of both inside every
  publishable crate, so each `.crate` source distribution carries the ITB CompuPhase zlib
  notice as required.
- Workspace-level `[workspace.package]` and `[workspace.dependencies]` metadata: `license`,
  `repository`, `homepage`, `keywords`, `categories`, `authors`, `rust-version`.
- CI workflow (clippy `-D warnings`, tests on Linux/Windows/macOS, MSRV check, `cargo package`,
  `cargo audit`) and a tag-driven release workflow that attaches binaries to the GitHub release.
- `CONTRIBUTING.md`, `SECURITY.md` and `docs/PUBLISHING.md` (crates.io publish order and
  dry-run gate).
- `zplint --version` / `-V`. The CLI previously had no version flag.

### Changed
- All `zpc-*` crates and the `zpc` driver move from `0.1.0` to `0.3.0`, matching the `zplint`
  binary. The workspace now ships as a single release train.
- Inter-crate path dependencies now carry `version = "0.3.0"`, which crates.io requires.
- Declared MSRV is **1.87** (was undeclared). `zpc-asm` uses `usize::is_multiple_of`, stable
  since 1.87.
- README: install section points at the real repository and `cargo install zplint`; the
  License section now states Apache-2.0 (it previously claimed MIT, with no `LICENSE` file
  present).

## [0.3.0] - 2026-07-25

First tagged release.

### Added
- Linter over `.sma` sources: `lint`, `watch` and `fix`, rayon-parallel, with Windows-1252
  decoding and `#pragma ctrlchar` support.
- Detector set covering compile errors, runtime-crash patterns, engine/HLDS limits, tag
  mismatches and ZP 5.0 API contracts, each backed by a documented source
  (`docs/KNOWLEDGE.md`).
- `zplint compile`: AMXX 1.10-compatible `.amxx` code generation via the `zpc` toolchain
  (lexer, parser, semantic analysis, codegen, assembler, `.amxx` container writer).
- `zplint disasm`: AMX bytecode disassembler.
- 74/74 acceptance parity with the reference `amxxpc` compiler on the official amxmodx corpus.
- Differential test gate (`scripts/difftest.mjs`) and runtime smoke test
  (`scripts/runtime-test.ps1`).

[Unreleased]: https://github.com/juninmd/zplint/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/juninmd/zplint/releases/tag/v0.3.0
