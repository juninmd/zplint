# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- `compile` now writes the **public-variable table**. `public` globals (and `@`-prefixed ones)
  were compiled with storage but never exported, so AMX Mod X could not bind `MaxClients`,
  `MapName`, `NULL_STRING` or `NULL_VECTOR` through `amx_FindPubVar`: a plugin reading one of
  them silently saw 0, with no load error. All 307 artifacts of the ZP corpus now carry the
  same pubvar table as `amxxpc`.
- The **native table no longer exports every native the includes declare**. Codegen numbers
  natives at declaration (single pass); the emitted `sysreq.c` operands are now renumbered into
  first-call order and the uncalled entries dropped, as `ffcall()` does. `zp50_flashlight.sma`
  went from 758 entries to 54 (`amxxpc`: 49), and the average `.amxx` is now 0.57x the
  `amxxpc` artifact instead of 1.5x. Declarations that share an exported name (float.inc
  aliases several `operator+` overloads to `floatadd`) now share one entry.

### Added
- 8 detectors covering hook re-entry, dynamic-handle ownership and inclusive bounds, each
  documented in `docs/KNOWLEDGE.md` and validated at 0 findings on the official amxmodx
  plugins: `ham_recursion` (`ExecuteHamB` of the very `Ham_` being hooked — hamsandwich.inc's
  "be very careful about recursion"), `message_recursion` (`message_begin` of the message its
  own `register_message` handler is hooking; `emessage_begin` is the fix), `kill_in_killed_hook`
  (`user_kill` inside `Ham_Killed`), `handle_leak` (local `ArrayCreate`/`TrieCreate`/
  `CreateDataPack` never destroyed), `handle_use_after_destroy`, `task_no_remove` (repeating
  per-player `set_task` in a file with no `remove_task`), `random_num_bound` (`random_num`'s
  upper bound is inclusive, so a bare `sizeof arr` is off by one) and `allocstring_hotpath`
  (`EngFunc_AllocString` per frame → "Hunk_Alloc: failed").
- `sizeof_string_len`: `sizeof buf` where a native's max-chars length is expected writes one
  cell past the buffer (`set_amxstring` emits `max` characters **and** a terminator). Driven by
  a curated `(native, buffer, length)` table so cell-count natives like `ArrayGetArray`, where
  `sizeof` is correct, stay quiet — and **auto-fixable**: `zplint fix` rewrites it to
  `charsmax(buf)`.
- `self_recursion` (amxxpc warning 234 against the 4096-cell AMX stack), `msg_arg_index`
  (message arguments are 1-based — amxmodx/messages.cpp rejects anything below 1) and
  `write_literal_range` (`write_byte(300)` reaches the client as 44).
- `mixed_return` (amxxpc warning 209, narrowed to leading `return` statements) and
  `task_entity_not_validated` (`set_task` keyed by an entity whose callback touches entity
  fields without `pev_valid`).
- 3 more detectors closing gaps the knowledge base already documented without a rule:
  `say_hook_handled` (a say/say_team handler whose every exit is `PLUGIN_HANDLED` eats all
  server chat), `zp_infect_pre_handled` (unconditional `PLUGIN_HANDLED` in
  `zp_fw_core_infect_pre`/`_cure_pre` deadlocks round start) and `cs_model_hotpath`
  (`cs_set_user_model` per frame).
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
- Declared MSRV is **1.88** (was undeclared). `zpc-lex` uses let-chains, stable since 1.88;
  `zpc-asm` uses `usize::is_multiple_of`, stable since 1.87.
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
