# Publishing to crates.io

The workspace ships as **one release train**: `zplint` and the nine `zpc*` crates all carry the
same version, inherited from `[workspace.package]` in the root `Cargo.toml`.

## One-time setup

1. Create a crates.io account (GitHub login) at <https://crates.io>.
2. Generate a scoped API token (**Account Settings → API Tokens**). Give it
   `publish-new` + `publish-update` only; do not use a token with `yank`/`change-owners`
   scopes for routine releases.
3. `cargo login` — this writes the token to `~/.cargo/credentials.toml`. Never commit it, never
   paste it into CI logs. For CI, store it as the `CARGO_REGISTRY_TOKEN` secret.

## Name availability

As of the last check, all ten names were unregistered on crates.io:

```
zplint  zpc  zpc-diag  zpc-lex  zpc-ast  zpc-parse  zpc-sema  zpc-codegen  zpc-asm  zpc-amxx
```

Re-verify immediately before the first publish — crates.io has no reservations, and the first
publish claims the name permanently (crates cannot be deleted, only yanked).

## Pre-flight gate

Run everything green before publishing anything:

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
cargo package --workspace --no-verify
node scripts/difftest.mjs --amxxpc path/to/amxxpc.exe `
  --include path/to/amxmodx/scripting/include --corpus path/to/amxmodx/plugins
pwsh scripts/runtime-test.ps1
./target/release/zplint lint
```

Also confirm the working tree is clean and tagged: `cargo publish` refuses a dirty tree unless
you pass `--allow-dirty`, and you should not pass it.

## Publish order

crates.io resolves every dependency from the registry, so a crate can only be published after
all of its dependencies exist there. Publish in dependency order, waiting for the index to
update between steps (`cargo publish` normally blocks until the crate is available):

```
1.  zpc-diag        (no zpc deps)
2.  zpc-ast         -> diag
3.  zpc-lex         -> diag
4.  zpc-amxx        -> diag
5.  zpc-parse       -> diag, lex, ast
6.  zpc-sema        -> diag, ast
7.  zpc-asm         -> diag, amxx
8.  zpc-codegen     -> diag, asm, ast, sema  (dev-deps: lex, parse)
9.  zpc             -> all of the above
10. zplint          -> amxx, asm, ast, lex, parse, codegen, diag
```

### Preferred: publish the whole workspace in one command

Cargo works out the order itself and resolves the not-yet-published siblings locally. Dry run
first — it packages and compiles all ten crates and then refuses to upload:

```powershell
cargo publish --workspace --dry-run
```

Verified on Cargo 1.96: exits 0 with `warning: aborting upload due to dry run` for each of the
ten crates. When it is green, drop the flag:

```powershell
cargo publish --workspace
```

### Fallback: one crate at a time

Only if the workspace publish fails midway. Publish in the order listed above, waiting for the
index between steps:

```powershell
cargo publish -p zpc-diag
```

> **A single-crate `--dry-run` of a dependent crate cannot pass before its dependencies are on
> crates.io.** `cargo publish -p zpc-codegen --dry-run` fails today with
> `no matching package named 'zpc-asm' found — location searched: crates.io index`, because in
> isolation Cargo resolves `zpc-asm = "0.3.0"` from the registry only. This is expected, not a
> misconfiguration. Use `cargo publish --workspace --dry-run` (or `cargo package --workspace
> --no-verify`) for pre-flight validation, and if you do fall back to per-crate publishing, run
> it for real in dependency order rather than dry-running the later crates first.

## After publishing

- Push the `vX.Y.Z` tag; `.github/workflows/release.yml` builds the Linux/Windows/macOS binaries
  and attaches them to the GitHub release.
- Check <https://docs.rs/zplint> built successfully.
- Add a co-owner if the project should outlive a single account:
  `cargo owner --add <user> zplint` (repeat per crate).

## Releasing a new version

1. Bump `version` in `[workspace.package]` **and** every `version = "..."` entry in
   `[workspace.dependencies]` — they must match.
2. Move the `## [Unreleased]` block in `CHANGELOG.md` under the new version heading and update
   the link refs at the bottom.
3. Run the pre-flight gate, commit, tag `vX.Y.Z`, publish in the order above.

## Licence obligations that ride along

Every published `.crate` is an independent source distribution, so each crate directory carries
its own copy of `LICENSE` (Apache-2.0) and `NOTICE` (which reproduces the ITB CompuPhase
zlib notice verbatim). **Do not remove them** — condition 3 of the zlib licence forbids removing
that notice from a source distribution. See [`LICENSING.md`](LICENSING.md) and
[`../ATTRIBUTION.md`](../ATTRIBUTION.md).
