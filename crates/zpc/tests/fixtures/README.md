# zpc test fixtures

Hand-written Pawn sources exercising the constructs a naive implementation gets wrong.
Every phase of the compiler is validated against these, so a regression in one phase
shows up as a failing fixture rather than as a silently wrong `.amxx`.

| Fixture | Exercises |
|---------|-----------|
| `lexer_edge_cases.sma` | `^` escapes (not `\`), `//*` as a line comment, operator greediness (`>>>=`), char/hex/binary/rational literals, multi-line string continuation, `..` ranges |
| `preproc_edge_cases.sma` | Pawn `%1` function-like macros, nested `#if`/`#elseif`, `defined()`, `#pragma ctrlchar` switching the escape char mid-file, macro names inside strings not expanding |
| `decl_edge_cases.sma` | Tagged enums, `(<<= 1)` step enums, enum members with array sizes, every variable form, default/by-ref/const/rest arguments, operator overloads |
| `stmt_expr_edge_cases.sma` | Full statement set, braceless bodies, `case 1..5` and multi-value cases, the whole precedence ladder, `goto`/labels, tag casts |

## End-to-end gates

`scripts/difftest.mjs` compiles a selected corpus with real `amxxpc` and zplint,
compares acceptance, checks output invariants, and disassembles every successful
zplint artifact. `--strict-diagnostics` also compares diagnostics.

`../runtime/compiler_smoke.sma` is compiled by both compilers and executed through
`scripts/runtime-test.ps1` in AMX Mod X 1.10/HLDS. Its PASS marker is emitted only
after forwards, callback dispatch, native calls, arrays, recursion, by-reference
arguments, floats, strings, and control flow complete successfully.
