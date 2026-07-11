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
