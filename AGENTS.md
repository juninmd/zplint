# zplint Build Guide

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build (~0.6s lint time)
cargo test                     # Run tests
cargo clippy                   # Lint Rust code
./target/release/zplint lint   # Lint .sma files
```

## Self-Validation

Always run zplint on itself before committing:

```bash
cd /d/Solutions/pessoal/zplint
cargo build --release
./target/release/zplint lint
```

Expected: 0 issues. If issues are found, fix source before commit.

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

## Project Structure

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point (clap) |
| `src/config.rs` | TOML config (serde) |
| `src/engine.rs` | Lint engine (17 detectors) |
| `src/rules.rs` | Helper functions (has_guard, enclosing_body) |
| `src/output.rs` | Biome-style colored output (termcolor) |
| `src/fix.rs` | Auto-fix for safe patterns |
| `src/watch.rs` | Watch mode (notify) |
| `src/discover.rs` | .sma file discovery |

## Hard Rules

1. Zero dependencies on Python/Node — single Rust binary
2. All 17 detectors must pass on the zplague-addons repo before release
3. No unsafe code
4. Test each rule with test .sma fixtures
5. Auto-fix only for 100% safe transforms (if > 0, charsmax)

## Performance Goals

| Target | Current | Status |
|--------|---------|--------|
| < 1s for 300 files | 0.58s | ✅ |
| < 50ms per file | ~2ms | ✅ |
| < 10MB binary | TBD | ⏳ |
