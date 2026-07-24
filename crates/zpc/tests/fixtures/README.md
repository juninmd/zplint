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

## Why these are not compiled by CI yet

The differential oracle (running the real `amxxpc` over the same inputs and diffing
its diagnostics and bytecode against ours) is **not** wired up: `amxxpc` is not
installed on this machine. Until it is, these fixtures are used as *parser/lexer*
inputs — they must lex and parse without spurious diagnostics — rather than as
end-to-end compile comparisons.

When an `amxxpc` binary becomes available, point the harness at it and these same
files become the first differential cases. See `docs/COMPILER_MIGRATION.md` §2.
