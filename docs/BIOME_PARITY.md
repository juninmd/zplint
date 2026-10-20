# Biome parity: research, gap analysis and backlog

This document is the result of a second research pass (October 2026) over:

- Biome's analyzer model: rules, groups, recommended sets, safe/unsafe fixes,
  suppressions, CLI reporters, `explain`, LSP.
- The AMX Mod X compiler sources (`compiler/libpc300/sc2.c`, `sc5-in.scp`) and
  every bundled include under `plugins/include`.
- The official `alliedmodders/amxmodx` plugin corpus (74 `.sma` files) and the
  local 542+ file Zombie Plague corpus under `D:\Solutions\pessoal\zplague-addons`.

The goal is not "copy Biome". It is to identify the parts of Biome's *product*
that make it feel better than a bag of regexes, and map them onto Pawn/AMXX.

---

## 1. What "as good as Biome" means

Biome is four things at once. Measuring zplint only by rule count misses three
of them.

| Pillar | What Biome does | Why it matters |
|---|---|---|
| **Analyzer breadth** | 568 rules in groups (`correctness`, `suspicious`, `style`, `complexity`, `performance`, `security`, `a11y`, `nursery`), with a `recommended` set | Users get a defensible default, not a flat list |
| **One toolchain** | Formatter + linter + import sorting in one binary, one config, one pass | No Prettier/ESLint/plugin matrix |
| **Diagnostics as teaching** | Every rule explains the error, why it happens, and the code action; safe vs unsafe fixes | A diagnostic is actionable, not a puzzle |
| **Workflow integration** | `--write`, `--unsafe`, `--only`/`--skip`, `--error-on-warnings`, `--max-diagnostics`, JSON/SARIF/GitHub/JUnit reporters, `ci`, `--changed`, `--stdin-file-path`, suppressions, `explain`, LSP | Works in a terminal, an editor, and CI without glue code |

Biome also separates **safe fixes** (semantics preserved, applied on save) from
**unsafe fixes** (opt-in). Pawn has the same distinction: `sizeof buf` →
`charsmax(buf)` is safe in the specific native contract, while `if (mode)` →
`if (mode > 0)` is semantically narrowing and should be reviewed.

Sources:

- https://biomejs.dev/linter/
- https://biomejs.dev/analyzer/suppressions/
- https://biomejs.dev/reference/cli/
- https://biomejs.dev/internals/architecture/

---

## 2. Where zplint already matches or beats Biome

These are real strengths and should not be traded away for surface parity.

| Area | zplint today |
|---|---|
| Domain depth | 106+ detectors, including ZP 5.0 API contracts, HLDS engine limits, `svc_bad`/RTE 3/4/10/11 patterns, tag-bit reinterpretation, hook re-entry |
| Compiler | `zpc` front-end (lexer, preprocessor, parser, sema, codegen, assembler, disassembler) with a differential gate against `amxxpc` |
| Speed | ~2 ms/file, ~1.7 s for 542 files, rayon-parallel |
| Corpus discipline | Official plugins are the false-positive baseline; every rule is validated on two corpora |
| Sources | Every rule is documented in `docs/KNOWLEDGE.md` with problem → consequence → sources |
| Legacy input | Windows-1252 decoding, `#pragma ctrlchar`, braceless bodies, `//*` line comments, backslash continuation |
| API surface | Generated table of 1631 signatures + 3194 constants from 66 includes, with arity/tag/include/enum checks |

Biome has more rules and a formatter; zplint has deeper AMXX semantics. The gap
is mostly in **breadth of correctness analysis**, **tooling UX**, and **the
formatter/LSP half of the product**.

---

## 3. What shipped in this batch

Implemented and tested in this change:

| Item | Type | Notes |
|---|---|---|
| `// zplint-ignore`, `-next-line`, `-file` | suppression | Biome-style comments, optional rule list, `-- reason` |
| `--only`, `--skip` | CLI | Repeatable/comma-separated, filters every rule including legacy boolean fields |
| `--error-on-warnings` | CLI | Warnings still print as warnings; exit code becomes 1 |
| `--max-diagnostics N` | CLI | `0` = unlimited; reports how many were hidden |
| `--json` | reporter | Stable machine-readable output for CI/editor bridges |
| `zplint explain <rule>` | CLI | Reads the curated `docs/KNOWLEDGE.md` entry + severity |
| `menu_handler_handled` | rule | `menu_create` handler must return `PLUGIN_HANDLED` (newmenus.inc) |
| `get_user_msgid_in_precache` | rule | `get_user_msgid` returns 0 in `plugin_precache` on first start |
| Wider callback families | rule fix | `register_event`, `register_forward`, `RegisterHam*`, `register_think/touch/impulse`, `register_logevent` |
| `duplicate_include` | rule | Duplicate `#include` in either quote style |
| `unknown_pragma` | rule | amxxpc warning 207; typo silently loses the setting |
| `sizeof_scalar` | rule | amxxpc warning 223 (`sizeof` on a scalar is 1) |
| Handle family table | rule fix | 24 constructor/destructor pairs: array/trie/stack/datapack/tr2/kvd/forward/SQL/DBI/nvault/regex/socket/SMC/INI/JSON |
| Cross-mod include aliases | rule fix | `csstats`/`dodstats`/`tfcstats`/`tsstats`, `csx`/`dodx`/`tfcx`/`tsx`; fixes 71 false errors on official DOD/TFC/TS plugins |

Validation: 0 errors on the official plugins (excluding `plugins/testsuite`, which
intentionally exercises invalid paths), 187 unit tests green.

### 3.1 P0/P1 status

| Item | Status |
|---|---|
| P0.1 `zpc-sema`/AST semantic pass (203/204/210/212) | done (`src/sema_lint.rs`) |
| P0.2 rule categories, `recommended`, `zplint rules` | done (`src/rule_meta.rs`) |
| P0.3 safe/unsafe fixes | done (`FixSafety`, `fix --unsafe-fixes`) |
| P0.4 `ci`, `--changed`, `--staged`, `--since` | done |
| P0.5 SQL injection | done (`sql_injection`, user-controlled args only) |
| P1.6 formatter | done (`zplint fmt --check/--write`, conservative) |
| P1.7 LSP | done (`zplint lsp`: diagnostics, hover, quick fixes, format) |
| P1.8 SARIF/GitHub/JUnit reporters | done (`--reporter`) |
| P1.9 path-aware lifecycle | done (`resource_leak_on_return`, early-return + guards) |
| P1.10 `get_players`/`find_player` flags | done |
| P1.11 `read_data` bounds | done for the stable event table |
| P1.12 `set_pdata_*` offset family | deferred: no verified per-mod/platform offset table in the bundled includes; see `docs/KNOWLEDGE.md` §10.15 |

---

## 4. Gap analysis

### 4.1 Correctness / compiler-parity gaps

`amxxpc` already warns about these, but zplint should explain them before the
compiler runs and with a rule id that can be suppressed/disabled.

| # | Gap | Detector idea | Source | FP risk | Signal |
|---|---|---|---|---|---|
| C1 | Unused local/global (203) | Scope-aware symbol table; don't flag `public`, `stock`, `native`, or write-only | `sc5-in.scp` 203 | Medium | Compiler already warns |
| C2 | Assigned but never read (204) | Read/write usage flags per symbol | `sc5-in.scp` 204 | Medium | Common dead state |
| C3 | Use before initialization (210) | Definite-assignment walk per function | `sc5-in.scp` 210 | Medium | Real runtime bug |
| C4 | Unintended bitwise op (212) | `if (x \| FLAG)` / `x & FLAG` where a boolean test was meant | `sc5-in.scp` 212 | Medium | Rare but costly |
| C5 | Nested comment (216) | `/* /* */` depth tracking | `sc5-in.scp` 216 | Low | Parser confusion |
| C6 | Tag override without parens (220) | `new x = Float:y` / return tag override | `sc5-in.scp` 220 | Low | Compiler warning |
| C7 | Redundant `sizeof` (223) | **Implemented** as `sizeof_scalar` | `sc5-in.scp` 223 | Low | - |
| C8 | Enum initializer count/length (227/228) | Count initializers vs enum fields; literal length vs field size | `sc5-in.scp` 227/228 | Low | Data-table bugs |
| C9 | Index tag mismatch (229) | Array indexed by a different enum tag | `sc5-in.scp` 229 | Medium | Wrong table row |
| C10 | Wrong arity for in-file functions (202) | Build a local function signature table; check calls | `sc5-in.scp` 202; `api_check` already does natives | Low | Compiler warning |
| C11 | Duplicate global (021) | Depth-0 declarations, enum/const aware | `sc5-in.scp` 021 | Low | Merge accidents |
| C12 | Multi-dim initializer mismatch (052) | Join the statement across lines; compare rows | `sc5-in.scp` 052 | Medium | Hand-edited tables |
| C13 | Variable array size (008/009) | Require `const`/`#define`/enum for dimensions | `sc5-in.scp` 008/009 | Medium | Beginner bug |
| C14 | Invalid subscript / too many dimensions (028) | Declared dimensions vs `a[i][j][k]` | `sc5-in.scp` 028 | Low | Compiler error |
| C15 | Extra characters on line (038) | After a complete statement, only `;`/comment allowed | `sc5-in.scp` 038 | Medium | Parser cascade |
| C16 | `sizeof` on a function (072) | Function-name table | `sc5-in.scp` 072 | Low | Compiler error |
| C17 | Mixed array/non-array return (079) | Return statement shape per function | `sc5-in.scp` 079 | Low | Compiler error |
| C18 | Missing include for a native prefix (017) | **Mostly covered** by `api_missing_include`; add prefix table for `zp_*`, `cs_*`, etc. | `sc5-in.scp` 017 | Medium | Most-asked error |

The cheapest path to C1-C9, C14-C17 is **not** more regexes: `zpc-sema` already
implements `SymbolTable` usage flags, tag checks, constant folding and unused
reports. Wiring `zpc-sema` into `ast_lint` would add a whole warning family in
one pass (see §6).

### 4.2 Lifecycle / resource gaps

Implemented now: the 24 handle families above, local-only, with escape
analysis and global exclusion.

Still open:

| # | Gap | Detector idea | Source |
|---|---|---|---|
| L1 | SQL result not freed | `dbi_query` → `dbi_free_result` (added); also `SQL_Execute` + `SQL_MoreResults` loop without `SQL_FreeHandle` on early return | `sqlx.inc`, `dbi.inc` |
| L2 | `SQL_ThreadQuery` handler signature | 7 params `(failstate, Handle:query, error[], errnum, data[], size, &maxlen)`; wrong signature reads stack | `sqlx.inc` |
| L3 | Menu handler destroyed on every path | Existing `menu_handler_destroy` only checks "contains menu_destroy"; add path-aware check | `newmenus.inc` |
| L4 | `create_tr2` in a static/global | Plugin-lifetime; free in `plugin_end`; currently excluded by local-only rule | `fakemeta.inc` |
| L5 | File handle opened but not closed on early return | Existing `fopen_close` checks the function, not paths | `file.inc` |
| L6 | `CreateMultiForward` / `CreateOneForward` not destroyed | Now detected locally; globals need `plugin_end` check | `amxmodx.inc` |
| L7 | `nvault_open` handle reused after close | Now covered by `handle_use_after_destroy` | `nvault.inc` |
| L8 | `regex_compile` error code ignored | `regex_compile` returns a handle and writes `&ret`; a compile failure is a null handle | `regex.inc` |
| L9 | `socket_open` error ignored | `&_error` out-param; connect/send on an invalid socket | `sockets.inc` |
| L10 | `json_parse` returns `Invalid_JSON` (-1) | Check before every `json_get_*` | `json.inc` |

### 4.3 API contract gaps

| # | Gap | Detector idea | Source |
|---|---|---|---|
| A1 | `get_players` flags | `get_players(..., "a")` = exclude dead; `"b"` = exclude alive; `"c"` = exclude bots; `"d"` = exclude humans; `"e"` = dead only (deprecated); `"h"` = exclude HLTV. Detect contradictory flags (`ab`, `cd`) and `find_player` userid + exclusion flags | `amxmodx.inc` |
| A2 | `find_player` userid flags | Doc: "If matching by userid, do not also specify the 'a', 'b' or 'c' flags" | `amxmodx.inc` |
| A3 | `register_event` args | `read_data(N)` beyond the event's known arg count | `messages.inc` / event docs |
| A4 | `set_msg_arg_*` type | Setter family vs `register_message` argument type | `messages.inc` |
| A5 | `write_string` buffer | Non-null-terminated string / `sizeof` vs `charsmax` | `amxmodx.inc` |
| A6 | `set_task` callback flags | Existing `set_task_flags`; extend to `set_task_ex` named constants and repeat/after/before combinations | `amxmodx.inc` |
| A7 | `create_cvar` vs `register_cvar` | `register_cvar` is documented deprecated; `create_cvar` supports bounds/callbacks | `cvars.inc` |
| A8 | `get_pcvar_*` pointer null check | `get_cvar_pointer` returns 0 on miss | `cvars.inc` |
| A9 | `entity_set_*` vs `EV_*` | **Implemented** (`entity_ev_type_mismatch`); extend to `set_pdata_*` offsets | `engine.inc` |
| A10 | `pev`/`set_pev` field family | **Implemented** for floats; extend to vector/string/edict fields | `fakemeta.inc` |
| A11 | `cs_get_user_*` vs `get_user_*` | `cs_get_user_money` vs `get_user_money` (different semantics); wrong native family | `cstrike.inc` |
| A12 | `get_user_origin` precision | **Implemented**; also `set_user_origin` loses float precision | `amxmodx.inc` |
| A13 | `menu_additem` count > 10 | New menus paginate; old `show_menu` does not | `newmenus.inc` |
| A14 | `show_menu` vs `menu_create` | New code should use `menu_create`; old API has no pagination in the same way | `newmenus.inc` |

### 4.4 Security gaps

| # | Gap | Detector idea | Source |
|---|---|---|---|
| S1 | SQL injection | `format`/`format_ex` with `%s` fed into `SQL_Query*`/`SQL_PrepareQuery*`/`SQL_ThreadQuery*`; recommend `SQL_QuoteString` or `SQL_QuoteStringFmt` | `sqlx.inc` |
| S2 | Command injection | Existing `cmd_injection_read_argv` covers `read_argv`/`read_args`; extend to any user-controlled buffer passed to `client_cmd`/`server_cmd`/`engclient_cmd` | `amxmodx.inc` |
| S3 | Path traversal | `fopen`/`read_file`/`write_file` with a path built from `read_argv`/`get_user_authid` | `file.inc` |
| S4 | Log injection | `log_message`/`log_amx` with raw user text (`%`/newlines) | `amxmodx.inc` |
| S5 | `setinfo`/`set_user_info` injection | Raw user text with `\` or newlines | `amxmodx.inc` |
| S6 | `register_cvar` from user input | `server_cmd("amx_cvar ...")` with user text | `amxmodx.inc` |
| S7 | Authid spoofing | `get_user_authid` used for authorization without `get_user_ip`/`is_user_bot` checks | `amxmodx.inc` |

### 4.5 Performance gaps

Existing: `get_cvar_hotpath`, `library_exists_hotpath`, `allocstring_hotpath`,
`strlen_in_loop`, `buffer_in_loop`, `read_file_loop`, `precache_in_loop`,
`cs_model_hotpath`.

Open:

| # | Gap | Detector idea |
|---|---|---|
| P1 | `ArraySize()` in loop condition | Hoist to a local |
| P2 | `get_players()` inside a loop | Collect once |
| P3 | `get_user_name`/`get_user_authid` in a loop | Cache per client |
| P4 | `cs_get_user_team` in a loop | Cache |
| P5 | `format`/`format_ex` in a loop | Reuse a buffer / build once |
| P6 | `engfunc`/`pev` in a loop | Cache fields |
| P7 | `get_user_msgid` in a hot path | Cache in `plugin_init` |
| P8 | `set_hudmessage`/`ShowSyncHudMsg` per player | Use one sync object |
| P9 | `get_pcvar_*` per call in a hot path | Cache the pcvar handle |
| P10 | `ArrayGetString` with a fixed large buffer in a loop | Reuse/trim |

### 4.6 Style / complexity gaps (Biome's `style` + `complexity`)

These are mostly formatter/linter-style and should be opt-in or warning-level,
exactly like Biome's `style` group.

| # | Gap | Detector idea |
|---|---|---|
| Y1 | Naming conventions | Globals `g_`, constants `UPPER_SNAKE`, functions `snake_case`, enums `PascalCase`; configurable |
| Y2 | Magic numbers | `32`, `33`, `MAX_PLAYERS`, time constants; whitelist `0/1/-1` |
| Y3 | Deep nesting / long functions | Configurable thresholds; report cyclomatic complexity |
| Y4 | Redundant `else` after `return` | `if (c) return a; else return b;` |
| Y5 | Boolean return simplification | `if (c) return true; else return false;` |
| Y6 | Redundant `return` at end | `return;` at the end of a `void`-equivalent |
| Y7 | `if (x == true)` / `x != false` | Prefer `if (x)` |
| Y8 | `while (true)` vs `for (;;)` | Pick one |
| Y9 | Unnecessary parentheses | `if ((x))` |
| Y10 | Trailing whitespace / tabs vs spaces | Formatter territory |
| Y11 | `#pragma semicolon 1` consistency | Project style |
| Y12 | Include order / grouping | `<amxmodx>` first, then alphabetical; `organize-includes` equivalent |

### 4.7 Tooling gaps (the other half of Biome)

| # | Biome feature | zplint today | Proposal |
|---|---|---|---|
| T1 | Formatter | none | `zplint fmt`: indentation, spaces around operators, brace style, line wrapping, `#pragma` normalization. Reuse `zpc-parse` + a printer; make it opt-in and idempotent. |
| T2 | LSP | none | `zplint lsp`: diagnostics, code actions (safe fixes), hover (rule doc), formatting. The JSON reporter and `explain` are the first pieces. |
| T3 | Safe/unsafe fixes | `auto_fixable: bool` | Add `FixSafety::{Safe, Unsafe, None}`, `--write` (safe) and `--unsafe` (all), matching Biome. |
| T4 | Rule groups + recommended | flat list | Add `category` metadata and `rules.recommended`; `--only=correctness`, `--skip=style`. |
| T5 | Range suppressions | added inline/file/next-line | Add `zplint-ignore-start`/`-end` pairs and an "unused suppression" diagnostic. |
| T6 | Reporters | default + JSON | Add `github`, `sarif`, `junit`, `checkstyle`, `rdjson` (Biome's set). |
| T7 | `ci` command | `lint` | Alias with no color, `--error-on-warnings`, `--max-diagnostics`, exit 1 on any diagnostic. |
| T8 | `--changed` / `--staged` | none | Git-aware file selection; very useful for pre-commit. |
| T9 | `--stdin-file-path` | none | Editor/CI piping without temp files. |
| T10 | Config overrides | single `zplint.toml` | `[[overrides]]` by path glob; per-directory config. |
| T11 | VCS ignore | `exclude` list | Read `.gitignore`/`.ignore`. |
| T12 | Rule docs URL | `explain` | Add `docs/rules/<id>.md` or a generated site; link from diagnostics. |
| T13 | Type-aware rules | `api_check` + zpc tags | Wire `zpc-sema` for real tag/symbol analysis (see §6). |
| T14 | Daemon / watch | `watch` | Keep simple polling/watch; a daemon is optional. |

---

## 5. Prioritized backlog

Ordered by (user impact × confidence) / effort. Each item lists the acceptance
gate.

### P0 - next sprint

1. **Wire `zpc-sema` into `ast_lint`** for 203/204/210/212/223/225/226.
   `SymbolTable::finish()` already batches unused diagnostics; `Folder`/`Tags`
   already model constants and tags. Acceptance: official plugins 0 errors; new
   fixtures for each warning; `ast-compare` shows no divergence.
2. **Rule metadata registry**: `id → { category, recommended, severity, fix }`.
   Acceptance: `zplint rules --json`, `--only=correctness`, `--skip=style`.
3. **Safe/unsafe fixes**: replace `auto_fixable: bool` with a `Fix` enum.
   Acceptance: `zplint fix` applies only safe fixes; `zplint fix --unsafe` applies
   all; `sizeof_string_len` stays safe, `zp_gamemode_if` becomes unsafe.
4. **`zplint ci` + `--changed`/`--staged`**. Acceptance: a pre-commit hook can
   run `zplint ci --changed` and fail on new diagnostics only.
5. **SQL/format injection rule** (S1). Acceptance: 0 findings on official
   plugins; fixtures for `format` + `SQL_Query` and `SQL_QuoteString`.

### P1 - next month

6. **Formatter (`zplint fmt`)**: start with whitespace/indentation/brace style,
   `#pragma` normalization, and include sorting. Acceptance: idempotent on the
   official plugins; `--check` mode; no semantic changes (compare `zpc` output
   before/after).
7. **LSP**: diagnostics + `explain` hover + safe fixes. Acceptance: VS Code
   client shows inline diagnostics on a real `.sma`.
8. **Reporters**: SARIF + GitHub + JUnit. Acceptance: `--reporter=sarif` passes a
   SARIF validator and GitHub annotations appear in a workflow.
9. **Path-aware lifecycle**: early returns, `plugin_end`, and ownership transfer
   for `fopen`, `menu_create`, `SQL_*`, `create_tr2`, `CreateMultiForward`.
10. **`get_players`/`find_player` flag rules** (A1/A2). Acceptance: fixtures for
    contradictory flags; 0 findings on official plugins.
11. **`read_data` event-arg bounds** (A3). Build a curated event table from
    `messages.inc` + `amxmodx.inc` docs. Acceptance: `DeathMsg` has 3 args;
    `read_data(4)` flagged.
12. **`set_pdata_*` offset family** (A9). Build a small offset table for the
    common HLSDK fields; avoid a full reverse-engineering project.

### P2 - later

13. **Naming/magic-number/complexity rules** (Y1-Y8), `style` group, warning by
    default.
14. **Baseline file**: `zplint-baseline.json` to suppress existing diagnostics
    in legacy code, with `--baseline`.
15. **`zplint-ignore-start`/`-end` ranges** + unused-suppression diagnostics.
16. **Cross-file analysis**: include resolution, call graph (mutual recursion,
    unused functions), project-level rules.
17. **Rule docs site**: generate from `docs/KNOWLEDGE.md`, link each diagnostic.
18. **Daemon**: optional; only if editor latency demands it.

---

## 6. Architecture note: reuse `zpc-sema` before writing more regexes

`crates/zpc-sema` already models the compiler's semantic layer:

- `SymbolTable` with `Scope`, `Usage::{DEFINED, READ, WRITTEN, CONST, PUBLIC,
  NATIVE, STOCK, ...}`, `finish()` → batched 203/204 diagnostics.
- `Tags`/`TagCheck` for 213 and 229; `Folder` for constant folding, array sizes,
  enum fields, `sizeof`.
- `RefKind::{Read, Write, ReadWrite, Call}` for use-before-init and dead stores.

The linter currently uses `zpc-ast` + `zpc-lex` only. A `sema_lint` pass that
walks the parsed `Program`, declares symbols, resolves references and runs the
existing checks would add C1-C9/C14-C17 in one place, with the compiler's own
semantics rather than new heuristics. Keep the existing regex rules as the
fallback for files that do not parse.

Recommended shape:

```
ast_lint::lint()
  ├─ existing AST rules (empty_statement, string_literal_compare, ...)
  ├─ sema_lint::run(&program)
  │    ├─ symbols: unused / write-only / use-before-init
  │    ├─ tags: mismatch / index tag / mixed return
  │    └─ fold: redundant sizeof / enum initializers / constant conditions
  └─ return issues; regex engine runs only on parse failure
```

Do not turn parse diagnostics into lint issues. The current "parse failure is
never a lint error" rule is what keeps the official baseline clean; preserve it.

---

## 7. Concrete rule proposals

The following are ready to implement, with the source and the expected false
positive posture. "0/0" means 0 findings on both official plugins and the
non-archive real corpus.

| Rule id | Severity | Detection | Source | Expected |
|---|---|---|---|---|
| `unused_local` | warning | Local declared and never read (203) | `sc5-in.scp` | compiler parity |
| `write_only_local` | warning | Assigned but never read (204) | `sc5-in.scp` | real dead state |
| `use_before_init` | error | Read before definite assignment (210) | `sc5-in.scp` | rare, high value |
| `unintended_bitwise` | warning | `if (x \| FLAG)` / `x & FLAG` in a boolean context (212) | `sc5-in.scp` | rare |
| `nested_comment` | warning | Nested `/*` (216) | `sc5-in.scp` | low |
| `tag_override_parens` | warning | Tag override without parentheses (220) | `sc5-in.scp` | low |
| `enum_init_count` | warning | More initializers than fields (227) | `sc5-in.scp` | data tables |
| `enum_init_length` | warning | String literal longer than enum field (228) | `sc5-in.scp` | data tables |
| `index_tag_mismatch` | warning | Array indexed by a different enum tag (229) | `sc5-in.scp` | wrong row |
| `in_file_arity` | warning | Call to an in-file function with wrong arity (202) | `sc5-in.scp` | compiler parity |
| `duplicate_global` | error | Two depth-0 declarations of the same name (021) | `sc5-in.scp` | merge accidents |
| `enum_dim_mismatch` | error | Multi-dim initializer rows ≠ declared (052) | `sc5-in.scp` | hand-edited tables |
| `variable_array_size` | error | Array dimension not a constant (008/009) | `sc5-in.scp` | beginner bug |
| `invalid_subscript` | error | Too many subscripts for declared dims (028) | `sc5-in.scp` | compiler error |
| `extra_characters` | error | Tokens after a complete statement (038) | `sc5-in.scp` | parser cascade |
| `sizeof_function` | error | `sizeof` on a function name (072) | `sc5-in.scp` | compiler error |
| `mixed_array_return` | error | Function returns both array and scalar (079) | `sc5-in.scp` | compiler error |
| `sql_injection` | warning | `format` `%s` into `SQL_*Query*` without `SQL_QuoteString` | `sqlx.inc` | security |
| `sql_thread_signature` | error | `SQL_ThreadQuery` handler not 7 params | `sqlx.inc` | RTE 10 |
| `get_players_flags` | warning | Contradictory `get_players` flags (`ab`, `cd`) | `amxmodx.inc` | logic bug |
| `find_player_userid_flags` | warning | `find_player` userid match + exclusion flags | `amxmodx.inc` | logic bug |
| `read_data_bounds` | error | `read_data(N)` past the event's arg count | `messages.inc` | RTE 4 |
| `cvar_pointer_null` | warning | `get_cvar_pointer` result used without 0 check | `cvars.inc` | RTE 10 |
| `menu_additem_over_10` | warning | Old `show_menu` with >10 items (no pagination) | `newmenus.inc` | UI bug |
| `array_size_in_loop` | warning | `ArraySize()` in a loop condition | perf | hot path |
| `get_players_in_loop` | warning | `get_players()` inside a loop | perf | O(n²) |
| `get_user_name_in_loop` | warning | name/authid in a loop | perf | O(n²) |
| `format_in_loop` | warning | `format`/`format_ex` in a loop | perf | repeated alloc |
| `naming_convention` | warning | Configurable naming rules | style | opt-in |
| `magic_number` | warning | Non-whitelisted literal in a condition | style | opt-in |
| `deep_nesting` | warning | Nesting depth > N | complexity | opt-in |
| `redundant_else` | warning | `else` after an unconditional `return` | style | opt-in |
| `boolean_return` | warning | `if (c) return true; else return false;` | style | opt-in |
| `if_equals_true` | warning | `if (x == true)` | style | opt-in |

---

## 8. Validation protocol

Every new rule must pass these gates before it ships:

1. **Fixture pair**: a bad `.sma` that triggers it and a good `.sma` that does
   not, in `src/*.rs` tests.
2. **Official corpus**: 0 errors (warnings allowed) on the 74 official plugins,
   excluding `plugins/testsuite` (which intentionally exercises invalid paths).
3. **Real corpus**: inspect every finding; 0 unexplained findings on the
   non-archive corpus.
4. **Severity**: crash/compile-failure = error; style/perf/contract smell =
   warning; add the id to `engine::WARNING_RULES`.
5. **Docs first**: add the problem/consequence/sources entry to
   `docs/KNOWLEDGE.md` before the detector (hard rule #6).
6. **Self-lint**: `cargo build --release && ./target/release/zplint lint` is
   clean.

---

## 9. References

Biome:

- https://biomejs.dev/linter/
- https://biomejs.dev/analyzer/suppressions/
- https://biomejs.dev/reference/cli/
- https://biomejs.dev/internals/architecture/
- https://biomejs.dev/linter/javascript/rules

AMXX compiler:

- https://github.com/alliedmodders/amxmodx/blob/master/compiler/libpc300/sc5-in.scp
- https://github.com/alliedmodders/amxmodx/blob/master/compiler/libpc300/sc2.c
- https://github.com/alliedmodders/amxmodx/tree/master/plugins/include
- https://wiki.alliedmods.net/Pawn_tutorial
- https://www.compuphase.com/pawn/pawnfeatures.htm

AMXX API/docs:

- https://www.amxmodx.org/api
- https://www.amxmodx.org/doc/source/scripting/primer.htm
- https://www.amxmodx.org/doc/source/scripting/advanced.htm
- https://wiki.alliedmods.net/Debugging_Plugins_(AMX_Mod_X)
- https://wiki.alliedmods.net/HamSandwich_General_Usage_(AMX_Mod_X)
- https://wiki.alliedmods.net/AMX_Mod_X_1.9_API_Changes

Includes with the contracts cited above:

- `newmenus.inc` (menu_create/menu_destroy/menu_display)
- `amxmodx.inc` (get_user_msgid, get_players, find_player, register_event, set_task)
- `cellarray.inc`, `celltrie.inc`, `cellstack.inc`, `datapack.inc`
- `sqlx.inc`, `dbi.inc`, `nvault.inc`, `regex.inc`, `sockets.inc`
- `textparse_smc.inc`, `textparse_ini.inc`, `json.inc`
- `fakemeta.inc`, `hamsandwich.inc`, `engine.inc`, `cstrike.inc`, `fun.inc`
