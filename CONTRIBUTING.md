# Contributing to zplint

Thanks for helping. This project lints and compiles Pawn/AMXX plugins, so a wrong rule or a
wrong opcode breaks live game servers — the bar for evidence is high.

## Licence of contributions

By submitting a contribution you agree it is licensed under the
[Apache License, Version 2.0](LICENSE), without additional terms.

**Before porting any code from upstream AMX Mod X or the Pawn compiler, read
[`docs/LICENSING.md`](docs/LICENSING.md).** Short version: code under the CompuPhase zlib
licence may be ported; GPL-licensed AMX Mod X code may **never** be transcribed, translated, or
rewritten line-by-line. Binary formats are reimplemented from facts (magic values, offsets,
field order), not from GPL sources. Files derived from upstream carry a header comment naming
the exact upstream file.

## Development

Requires Rust **1.88+** (edition 2024).

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`cargo clippy -D warnings` is the CI gate and must be clean. The tree predates rustfmt and is
not currently `cargo fmt`-clean; do not reformat files you are not otherwise touching.

## Full release gate

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
node scripts/difftest.mjs --amxxpc path/to/amxxpc.exe `
  --include path/to/amxmodx/scripting/include --corpus path/to/amxmodx/plugins
pwsh scripts/runtime-test.ps1
./target/release/zplint lint
```

The differential gate compares zplint's accept/reject decisions against the reference `amxxpc`
compiler and disassembles every artifact zplint produces.

## Adding a detector

A rule lands only with a citation. Every detector must:

1. Have a documented source — AMXX compiler sources, AlliedModders wiki/forums, amxmodx.org API
   docs, or the official ZP 5.0 sources — recorded in [`docs/KNOWLEDGE.md`](docs/KNOWLEDGE.md).
2. Produce **zero** findings on the official `alliedmodders/amxmodx` bundled plugins. That
   corpus is canonical code; anything it trips is a false positive.
3. Pick the right severity: **errors** are crash or compile-failure patterns and set exit code
   1; **warnings** are style/perf/modernization signals and never fail CI.
4. Ship with tests covering both the positive case and a near-miss that must not fire.

## Compiler changes

Divergences from the reference compiler are tracked in
[`docs/DIVERGENCES.md`](docs/DIVERGENCES.md); migration status is in
[`docs/COMPILER_MIGRATION.md`](docs/COMPILER_MIGRATION.md). If you change codegen, run the
differential gate and the runtime smoke test — unit tests alone do not prove a `.amxx` loads.

## Commits and pull requests

- Commits (and PR titles, which become the squash-commit message) must follow
  [Conventional Commits](https://www.conventionalcommits.org): `feat(rules): ...`,
  `fix: ...`, `docs: ...`. The `Semantic PR title` check enforces it.
- Releases are automatic. On every push to `master`, `.github/workflows/release.yml` reads the
  commits since the last tag: `feat` -> minor, `fix`/`perf` -> patch, `type!:` or a
  `BREAKING CHANGE:` footer -> major (minor while the version is 0.x). It bumps the version,
  writes the `CHANGELOG.md` section, tags `vX.Y.Z` and publishes the GitHub release.
  `docs`/`chore`/`ci`/`test`/`refactor`/`style`/`build` commits alone do not release.
  Never bump versions or edit the generated changelog section by hand.
- Keep the diff surgical: no unrelated refactors or reformatting.
- crates.io publishing is still manual; see [`docs/PUBLISHING.md`](docs/PUBLISHING.md).

## Reporting bugs

Include the `.sma` that reproduces it (minimized if possible), the zplint version
(`zplint --version`), your OS, and the full command line. For compiler bugs, say what `amxxpc`
does with the same input — a divergence report is far more actionable than a bare failure.

## Security

Do not open a public issue for a vulnerability. See [`SECURITY.md`](SECURITY.md).
