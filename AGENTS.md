# zplint Build Guide

## Commands

```bash
cargo build --workspace                                # Debug build
cargo build --release                                  # Release build (~0.6s lint time)
cargo test --workspace                                 # Run tests (CI gate)
cargo clippy --workspace --all-targets -- -D warnings  # Lint Rust code (CI gate)
./target/release/zplint lint <file.sma|dir>            # Lint .sma files
```

No `cargo fmt --check` gate: the tree predates rustfmt, do not reformat files you are not touching.

## CI Gates

`.github/workflows/`: `ci.yml` = clippy (`-D warnings`), `cargo test --workspace` on
ubuntu/windows/macos, MSRV 1.88 (`cargo check --workspace --all-targets`),
`cargo package --workspace --no-verify`, `cargo audit`. `semantic-pr.yml` = PR title must be a
Conventional Commit (it becomes the squash commit and drives `release.yml` versioning; `docs:`
never releases). `security.yml` = shared `juninmd/base-actions` scan.

## README is compiled into the binary

`include_str!("../README.md")` in `src/rule_meta.rs` and `src/main.rs` feeds `zplint rules` and
`zplint explain`. Never reformat the rule table rows (lines starting with ``| `rule_id` ``) and do
not put a backticked rule id in prose above the tables (`explain` falls back to the first README
line that contains it). After touching README.md, rebuild and compare `zplint rules` output.

## Self-Validation

```bash
cargo build --release
./target/release/zplint lint crates/zpc/tests/runtime/compiler_smoke.sma
```

Plain `zplint lint` in this repo prints `No .sma files found` (default `paths` is the author-local
`meus_plugins_organizados`), so it validates nothing here. The release gate in README.md
(`difftest.mjs`, `runtime-test.ps1`) needs `amxxpc`, AMXX includes and Docker.

## AMXX Rule References

Use these primary references when adding or validating Pawn/AMXX lint rules:

| Reference | Use |
|-----------|-----|
| `https://www.amxmodx.org/api/amxmodx` | Core AMXX natives and callback contracts |
| `https://www.amxmodx.org/api/messages` | Message API scope, `message_begin/message_end`, `write_*`, and `get_msg_arg*/set_msg_arg*` rules |
| `https://www.amxmodx.org/api/newmenus/menu_create` | Dynamic menu creation contracts |
| `https://www.amxmodx.org/api/newmenus/menu_destroy` | Required cleanup for dynamic menu resources |
| `https://www.amxmodx.org/api/file/fopen` | File handle lifecycle; pair `fopen` with `fclose` |
| `https://www.amxmodx.org/api/cellarray/ArrayGetString` | Array index validity; guard `ArraySize() > 0` before random access |
| `https://amxmodx.org/doc/index.html?page=source%2Ffunctions%2Fcore%2Fset_task.htm` | Legacy `set_task` flags/repeat semantics |
| `https://github.com/alliedmodders/amxmodx` | Source-level validation for docs ambiguity and edge behavior |

Before adding a detector, confirm it against at least one primary AMXX reference and one real `.sma` fixture/pattern from `D:\Solutions\pessoal\zplague-addons` when possible.

## Regenerating the API table

`src/api.rs` is generated, never hand-edited. To refresh it against upstream AMX Mod X:

```bash
git clone --depth 1 --filter=blob:none --sparse https://github.com/alliedmodders/amxmodx
cd amxmodx && git sparse-checkout set plugins
node scripts/genapi.mjs <clone>/plugins/include src/api.rs
cargo test          # includes a table-sorted invariant check
```

Node is required only for this regeneration step; building and running zplint never needs it.

## Project Structure

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point (clap), rayon-parallel lint |
| `src/config.rs` | TOML config (serde); `rules.disable` list for detectors.rs rules |
| `src/engine.rs` | Lint engine (original 37 detectors), comment stripping, severity table |
| `src/detectors.rs` | Research-driven detectors (67 rules, see docs/KNOWLEDGE.md) |
| `src/sema_lint.rs` | AST semantic rules: 203/204/210/212 via zpc-sema |
| `src/fmt.rs` | Conservative formatter (`zplint fmt --check/--write`) |
| `src/lsp.rs` | LSP server over stdio (diagnostics, hover, quick fixes, format) |
| `src/rule_meta.rs` | Rule categories, recommended set, `zplint rules` |
| `docs/BIOME_PARITY.md` | Research, Biome gap matrix and prioritized rule/tooling backlog |
| `src/api.rs` | GENERATED: 1631 signatures + 3194 constants from the 66 bundled includes |
| `src/api_check.rs` | Whole-API rules (arity, tags, includes, typos, forwards, enum constants) |
| `scripts/genapi.mjs` | Dev-time generator for `src/api.rs` (not needed to build or run zplint) |
| `src/rules.rs` | Helper functions (has_guard, enclosing_body, squash) |
| `src/output.rs` | Biome-style colored output (termcolor), prints rule ids |
| `src/fix.rs` | Auto-fix for safe patterns |
| `src/watch.rs` | Watch mode (notify) |
| `src/self_update.rs` | `zplint self-update`: GitHub release download, SHA-256 check, binary swap |
| `scripts/release.sh` | Conventional-commit version bump + release notes (used by release.yml) |
| `src/discover.rs` | .sma file discovery |
| `docs/KNOWLEDGE.md` | Knowledge base: every rule's problem/consequence/sources from web research |

## Hard Rules

1. Zero dependencies on Python/Node — single Rust binary
2. The official `alliedmodders/amxmodx` bundled plugins must lint with **0 errors**
   (warnings allowed) — canonical code is the false-positive baseline
3. No unsafe code
4. Test each rule with test .sma fixtures (bad flagged + good passes)
5. Auto-fix only for 100% safe transforms (if > 0, charsmax)
6. New detectors: document problem/consequence/sources in docs/KNOWLEDGE.md first;
   add the rule id to WARNING_RULES in engine.rs if it is style/perf (not crash)
7. Pawn parsing gotchas learned from the corpora: escape char is `^` unless
   `#pragma ctrlchar '\'`; `//*` is a line comment (not a block open); braceless
   single-statement function bodies exist; `if (..) message_begin(A) else
   message_begin(B)` is one message, not nesting; multi-line continuation is a
   trailing **backslash only** — hardcoded in `readline()` (`if (*ptr=='\\')`),
   independent of `#pragma ctrlchar`, so a trailing `^` is just an xor;
   literal-to-array assignment is legal when the literal fits

## Performance Goals

| Target | Current | Status |
|--------|---------|--------|
| < 1s for 300 files | 0.58s | ✅ |
| < 50ms per file | ~2ms | ✅ |
| < 10MB binary | TBD | ⏳ |
