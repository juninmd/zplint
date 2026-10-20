# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Biome-parity workflow**: comment suppressions (`// zplint-ignore`,
  `-next-line`, `-file`, optional rule list, `-- reason`), `--only`/`--skip`
  rule filtering, `--error-on-warnings`, `--max-diagnostics`, a `--json`
  reporter and `zplint explain <rule>` (reads the curated `docs/KNOWLEDGE.md`
  entry plus severity). See `docs/BIOME_PARITY.md` for the research, gap matrix
  and prioritized backlog.
- 5 new detectors: `menu_handler_handled` (newmenus.inc: a `menu_create` handler
  must return `PLUGIN_HANDLED`), `get_user_msgid_in_precache` (amxmodx.inc: id 0
  on the first server start), `duplicate_include`, `unknown_pragma` (amxxpc
  warning 207) and `sizeof_scalar` (warning 223).
- `callback_not_defined` and `registered_callback_public` now cover
  `register_event`, `register_forward`, `RegisterHam*`, `register_think`,
  `register_touch`, `register_impulse` and `register_logevent`; registrations in
  commented-out code are ignored.
- `handle_leak` and `handle_use_after_destroy` now cover 24 constructor/
  destructor pairs from the bundled includes (array, trie, stack, datapack, tr2,
  kvd, forward, SQL tuple/connection, DBI, nvault, regex, socket, SMC/INI parser,
  JSON). Globals remain exempt; locals are checked.
- Cross-mod include aliases for `api_missing_include`: the same native is
  declared by `csstats`/`dodstats`/`tfcstats`/`tsstats` and
  `csx`/`dodx`/`tfcx`/`tsx`. This removes 71 false errors on the official
  DOD/TFC/TS plugins.

### Added
- New rule `velocity_overwrite_no_add` (warning): flags `set_pev(id, pev_velocity, var)` that
  replaces the whole velocity vector instead of combining it with the player's current velocity.
  Production incident: a knockback/speed-boost ability that overwrites `pev_velocity` outright
  silently erases fall damage and jump/bhop momentum. Warning-level: a full overwrite is also the
  correct idiom for launch pads/rocket-jump abilities, so it can't be proven a bug from source alone.
- New rule `cmd_injection_read_argv` (warning): flags `client_cmd`/`engclient_cmd`/`server_cmd`
  forwarding a raw `read_argv`/`read_args` buffer, which lets a caller chain `;` to inject
  console commands. Warning-level because the caller is often an admin-gated command
  (e.g. the bundled amxmodx `admincmd.sma`), not arbitrary player input.
- New rule `zp_maxhealth_arg_order` (warning): flags `zp_class_zombie_get_max_health`/
  `zp_class_human_get_max_health` called as `(classid, id)` with the args swapped. Both natives
  are `(id, classid)` per zp50_class_zombie.inc/zp50_class_human.inc; the reversed call still
  compiles (both are plain cells) but silently reads the wrong table row. 0 hits on both corpora
  (14 real call sites already use the correct order) — a preventive rule for a documented
  porting footgun, same family as `sql_fieldname_truthy`.
- New rule `zp_menu_text_add_outside_select_pre` (warning): flags `zp_class_zombie_menu_text_add`/
  `zp_class_human_menu_text_add`/`zp_items_menu_text_add` called outside their matching
  `select_pre` forward (per each native's doc comment). Resolved one level deep like
  `task_entity_not_validated` — a local helper called from the matching `select_pre` forward
  doesn't trigger it. 0 hits on both corpora after that fix (4 initial hits in the real corpus
  were all this exact helper-function pattern, i.e. false positives caught before shipping).
- New rule `zp_id_get_truthy` (warning): flags a bare `if (zp_items_get_id(...))` /
  `zp_class_zombie_get_id`/`zp_class_human_get_id`/`zp_gamemodes_get_id` check (direct or via a
  variable assigned earlier in the same function). All four natives return their `ZP_INVALID_*`
  constant (confirmed `-1` in each `*_const.inc`) on failure, which is truthy in Pawn, while a
  valid id `0` is falsy — same inverted-truthiness trap as `func_id_truthy`/`sql_fieldname_truthy`.
  0 hits on both corpora (89 real call sites already assign-then-compare with `!= -1`).

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
