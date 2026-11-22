# AGENTS.md

`zplint`: a single Rust binary that lints AMX Mod X Pawn (`.sma`) plugins, with an LSP server, a conservative formatter and a Pawn compiler workspace (`crates/zpc*`).

## Stack

- Rust, MSRV 1.88, Cargo workspace (`crates/*`), zero Python or Node dependencies at build or run time
- CLI (clap), rayon, termcolor, notify (watch mode), LSP over stdio

## Commands

```bash
cargo build --workspace                                # Debug build
cargo build --release                                  # Release build
cargo test --workspace                                 # Tests (CI gate)
cargo clippy --workspace --all-targets -- -D warnings  # Lint (CI gate)
cargo check --workspace --all-targets                  # MSRV check (CI, Rust 1.88)
./target/release/zplint lint <file.sma|dir>            # Lint .sma files
```

- No `cargo fmt --check` gate: the tree predates rustfmt. Do not reformat files you do not touch.
- Self-validation: `cargo build --release && ./target/release/zplint lint crates/zpc/tests/runtime/compiler_smoke.sma`
- Release gate (README): `scripts/difftest.mjs` and `scripts/runtime-test.ps1` need `amxxpc`, AMXX includes and Docker.

## Structure

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point (clap), rayon-parallel lint |
| `src/config.rs` | TOML config (serde); `rules.disable` list |
| `src/engine.rs` | Lint engine (original 37 detectors), comment stripping, severity table |
| `src/detectors.rs` | Research-driven detectors (67 rules, see `docs/KNOWLEDGE.md`) |
| `src/sema_lint.rs` | AST semantic rules 203/204/210/212 via `zpc-sema` |
| `src/fmt.rs` | Conservative formatter (`zplint fmt --check/--write`) |
| `src/lsp.rs` | LSP server over stdio (diagnostics, hover, quick fixes, format) |
| `src/rule_meta.rs` | Rule categories, recommended set, `zplint rules` |
| `src/api.rs` | GENERATED: 1631 signatures and 3194 constants from 66 bundled includes |
| `src/api_check.rs` | Whole-API rules (arity, tags, includes, typos, forwards, enum constants) |
| `src/rules.rs`, `src/output.rs`, `src/fix.rs`, `src/watch.rs`, `src/discover.rs` | helpers, colored output, auto-fix, watch mode, `.sma` discovery |
| `src/self_update.rs` | `zplint self-update`: GitHub release download, SHA-256 check, binary swap |
| `crates/zpc*` | Pawn compiler workspace (lex, parse, sema, codegen, asm, amxx) |
| `scripts/` | `genapi.mjs`, `gendiag.mjs`, `genopcodes.mjs`, `difftest.mjs`, `runtime-test.ps1`, `release.sh` |
| `docs/` | `KNOWLEDGE.md` (rule sources), `BIOME_PARITY.md` (gap matrix), `DIVERGENCES.md`, `COMPILER_MIGRATION.md`, `PUBLISHING.md` |

## Conventions

- Zero dependencies on Python or Node. A single Rust binary.
- No `unsafe` code.
- The official `alliedmodders/amxmodx` bundled plugins must lint with 0 errors (warnings allowed). Canonical code is the false-positive baseline.
- Test each rule with `.sma` fixtures: a bad case is flagged and a good case passes.
- Auto-fix only for 100% safe transforms (e.g. `charsmax`).
- New detectors: document problem, consequence and sources in `docs/KNOWLEDGE.md` first. Add the rule id to `WARNING_RULES` in `engine.rs` if it is style or performance, not a crash.
- Confirm a new detector against at least one primary AMXX reference and one real `.sma` fixture or pattern when possible.
- Performance budgets (not yet measured in CI): under 1 s for 300 files, under 50 ms per file, under 10 MB binary.
- PR titles are Conventional Commits (`semantic-pr.yml`). `docs:` never releases.

### Primary AMXX references

| Reference | Use |
|-----------|-----|
| `https://www.amxmodx.org/api/amxmodx` | Core AMXX natives and callback contracts |
| `https://www.amxmodx.org/api/messages` | Message API scope, `message_begin/message_end`, `write_*`, `get_msg_arg*/set_msg_arg*` |
| `https://www.amxmodx.org/api/newmenus/menu_create` | Dynamic menu creation contracts |
| `https://www.amxmodx.org/api/newmenus/menu_destroy` | Required cleanup for dynamic menus |
| `https://www.amxmodx.org/api/file/fopen` | File handle lifecycle; pair `fopen` with `fclose` |
| `https://www.amxmodx.org/api/cellarray/ArrayGetString` | Array index validity; guard `ArraySize() > 0` before random access |
| `https://amxmodx.org/doc/index.html?page=source%2Ffunctions%2Fcore%2Fset_task.htm` | Legacy `set_task` flags and repeat semantics |
| `https://github.com/alliedmodders/amxmodx` | Source-level validation for doc ambiguity and edge behavior |

## Gotchas

- `README.md` is compiled into the binary (`include_str!` in `src/rule_meta.rs` and `src/main.rs`) and feeds `zplint rules` and `zplint explain`. Do not reformat the rule-table rows (lines starting with ``| `rule_id` ``). Do not put a backticked rule id in prose above the tables. After editing the README, rebuild and compare `zplint rules`.
- `src/api.rs` is generated. Do not hand-edit. To refresh: `git clone --depth 1 --filter=blob:none --sparse https://github.com/alliedmodders/amxmodx`, `git sparse-checkout set plugins` inside it, then `node scripts/genapi.mjs <clone>/plugins/include src/api.rs` and `cargo test` (includes a sorted-table invariant check). Node is needed only for this step.
- Plain `zplint lint` in this repo prints `No .sma files found`, because the default `paths` points to an author-local folder. Use the explicit path shown in Commands.
- Pawn parsing gotchas (from the corpora): escape char is `^` unless `#pragma ctrlchar '\'`; `//*` is a line comment, not a block open; braceless single-statement function bodies exist; `if (..) message_begin(A) else message_begin(B)` is one message, not nesting; multi-line continuation is a trailing backslash only (hardcoded in `readline()`, independent of `#pragma ctrlchar`), so a trailing `^` is just an xor; literal-to-array assignment is legal when the literal fits.

<!-- base-actions-main-policy -->
## GitHub Actions — padrão obrigatório (@main)

- Referencie as actions internas sempre pelo branch, nunca por SHA:
  - `uses: juninmd/base-actions/<action>@main`
  - `uses: juninmd/base-actions/.github/workflows/<arquivo>.yml@main`
- Não fixe `@<sha>` em `juninmd/base-actions`: a atualização é centralizada no
  `main` e passa a valer no próximo run.
- PRs cujos checks não iniciam por billing/spending limit são mesclados pelo bot central
  `juninmd/base-actions/.github/workflows/auto-merge-billing-blocked.yml`.
  Para impedir o merge automático de um PR, aplique a label `do-not-merge`.
