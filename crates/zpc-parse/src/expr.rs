//! Expression parsing: the `hier14` .. `hier1` / `primary` / `constant` ladder.
//!
//! Ported from `compiler/libpc300/sc3.c` (Pawn compiler, (c) ITB CompuPhase,
//! zlib-style licence - see ATTRIBUTION.md). The original is a single-pass code
//! generator, so each `hier` function both parses *and* emits; here every level
//! only builds a [`Expr`] node, but the shape of the ladder is preserved
//! one-for-one:
//!
//! | `sc3.c`      | operators                | this file                 |
//! |--------------|--------------------------|---------------------------|
//! | `expression` | -                        | [`Parser::parse_expr`]    |
//! | `hier14`     | `=` `+=` .. `>>>=`       | `hier14`                  |
//! | `hier13`     | `?:`                     | `hier13`                  |
//! | `hier12`     | `\|\|`                   | `hier12`                  |
//! | `hier11`     | `&&`                     | `hier11`                  |
//! | `hier10`     | `==` `!=`                | `hier10`                  |
//! | `hier9`      | `<` `<=` `>` `>=`        | `hier9` (`plnge_rel`)     |
//! | `hier8`      | `\|`                     | `hier8`                   |
//! | `hier7`      | `^`                      | `hier7`                   |
//! | `hier6`      | `&`                      | `hier6`                   |
//! | `hier5`      | `<<` `>>` `>>>`          | `hier5`                   |
//! | `hier4`      | `+` `-`                  | `hier4`                   |
//! | `hier3`      | `*` `/` `%`              | `hier3`                   |
//! | `hier2`      | prefix ops, `sizeof`, .. | `hier2`                   |
//! | `hier1`      | `[]` `{}` `()`           | `hier1`                   |
//! | `primary`    | `( .. )`, names          | `primary`                 |
//! | `constant`   | literals, `{1,2}`        | `constant`                |
//!
//! # Two pieces of state the original keeps in globals
//!
//! `sc_allowtags` and `sc_allowproccall` are file-scope variables in the
//! compiler, saved and restored around sub-expressions. They are carried here in
//! [`Ctx`], threaded through the ladder by value, because a `Parser` field would
//! have to be saved/restored by hand at exactly the same points anyway.

use zpc_ast::{
    Ident, Span,
    expr::{
        Arg, ArgValue, BinOp, Expr, ExprKind, Fixity, IncDecOp, IndexKind, RationalLit,
        SizeOfLevel, StringLit, TagOfTarget, UnOp,
    },
};
use zpc_lex::TokenKind;

use crate::Parser;

/// The two context flags `sc3.c` keeps in globals.
#[derive(Clone, Copy, Debug)]
struct Ctx {
    /// `sc_allowtags`. False inside the "then" arm of `?:`, where a `:` closes
    /// the conditional and so `Name:` cannot be a tag override (`sc3.c:1041`).
    allow_tags: bool,
    /// `sc_allowproccall`. Only ever true at the very start of an expression
    /// *statement* (`sc1.c:4800`); cleared by the first operator (`nextop()`),
    /// by an assignment (`sc3.c:830`) and inside parentheses (`sc3.c:1702`).
    allow_proccall: bool,
}

impl Ctx {
    /// The context `expression()` starts from.
    const fn top() -> Self {
        Self { allow_tags: true, allow_proccall: false }
    }

    const fn no_tags(self) -> Self {
        Self { allow_tags: false, ..self }
    }

    const fn no_proccall(self) -> Self {
        Self { allow_proccall: false, ..self }
    }
}

/// One rung of a plain left-associative level (`plnge()`).
type Rung<'a> = fn(&mut Parser<'a>, Ctx) -> Expr;

impl<'a> Parser<'a> {
    // ================================================================ entry

    /// Parse one expression: `expression()` in `sc3.c`, which is `hier14()` plus
    /// bookkeeping the tree-building parser does not need.
    ///
    /// Note that the comma operator is *not* part of this: `hier14` is the entry
    /// point, and a comma expression only exists inside parentheses (`primary()`),
    /// which is what lets `new a = 1, b = 2` and argument lists work.
    pub(crate) fn parse_expr(&mut self) -> Expr {
        self.hier14(Ctx::top())
    }

    /// Parse one expression in *statement* position, where Pawn's
    /// parenthesis-less procedure call syntax is enabled (`sc_allowproccall`,
    /// set in `statement()` at `sc1.c:4800`).
    ///
    /// `foo` and `client_print id, print_chat, "x"` are calls here but not in
    /// [`Parser::parse_expr`]. Statement parsing should prefer this entry point;
    /// using `parse_expr` merely loses the paren-less form, it never misparses.
    pub fn parse_expr_stmt(&mut self) -> Expr {
        let start = self.cur_span();
        let first = self.hier14(Ctx { allow_proccall: true, ..Ctx::top() });

        // `expression()` (sc3.c) is `hier14()` followed by a loop on `,` - the
        // comma operator, which at statement position chains several expressions
        // into one statement: `f(1), f(2), f(3);`. Only a *declaration's*
        // initialiser stops at hier14, which is what keeps `new a = 1, b = 2`
        // parsing as two declarators; that path uses `parse_expr`, not this one.
        if !self.at(&TokenKind::Comma) {
            return first;
        }
        let mut exprs = vec![first];
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.hier14(Ctx { allow_proccall: true, ..Ctx::top() }));
        }
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::Comma { exprs, span }, span }
    }

    // ============================================================== hier14

    /// `hier14()`: assignment, `=` and the compound forms. Right-associative -
    /// the original recurses through `plnge2(oper, hier14, ..)`.
    fn hier14(&mut self, ctx: Ctx) -> Expr {
        let start = self.cur_span();
        let target = self.hier13(ctx);
        let Some(op) = assign_op(self.peek()) else { return target };
        self.bump();
        // "may no longer use procedure call syntax" (sc3.c:830)
        let value = self.hier14(ctx.no_proccall());
        let span = start.to(self.prev_span());
        Expr {
            kind: ExprKind::Assign {
                op,
                target: Box::new(target),
                value: Box::new(value),
                span,
            },
            span,
        }
    }

    // ============================================================== hier13

    /// `hier13()`: the conditional operator. Both arms recurse into `hier13`,
    /// making it right-associative.
    fn hier13(&mut self, ctx: Ctx) -> Expr {
        let start = self.cur_span();
        let cond = self.hier12(ctx);
        if !self.eat(&TokenKind::Question) {
            return cond;
        }
        // sc_allowtags=FALSE: inside the first arm a `:` ends the conditional,
        // so `Name:` cannot be a tag override there.
        let then_expr = self.hier13(ctx.no_tags().no_proccall());
        self.eat_ternary_colon();
        let else_expr = self.hier13(ctx.no_proccall());
        let span = start.to(self.prev_span());
        Expr {
            kind: ExprKind::Ternary {
                cond: Box::new(cond),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
                span,
            },
            span,
        }
    }

    /// Consume the `:` of a conditional.
    ///
    /// The scanner glues `name:` into one [`TokenKind::Label`] token, so in
    /// `a ? b: c` the colon has already been swallowed by the `b:` label and
    /// there is no `:` left to match. `primary()` unpacks such a token back into
    /// an identifier when tags are not allowed; this recognises that it did.
    fn eat_ternary_colon(&mut self) {
        if self.eat(&TokenKind::Colon) {
            return;
        }
        if matches!(self.prev_kind(), Some(TokenKind::Label(_))) {
            return; // the colon was lexed as part of the label token
        }
        self.expect(&TokenKind::Colon);
    }

    // ========================================================= hier12..hier3

    /// `hier12()`: `||`.
    fn hier12(&mut self, ctx: Ctx) -> Expr {
        self.plnge(ctx, &[(TokenKind::OrOr, BinOp::LogOr)], Self::hier11)
    }

    /// `hier11()`: `&&`.
    fn hier11(&mut self, ctx: Ctx) -> Expr {
        self.plnge(ctx, &[(TokenKind::AndAnd, BinOp::LogAnd)], Self::hier10)
    }

    /// `hier10()`: `==` and `!=`.
    fn hier10(&mut self, ctx: Ctx) -> Expr {
        self.plnge(
            ctx,
            &[(TokenKind::EqEq, BinOp::Eq), (TokenKind::NotEq, BinOp::Ne)],
            Self::hier9,
        )
    }

    /// `hier9()`: the relational operators, via `plnge_rel()` rather than
    /// `plnge()`.
    ///
    /// Pawn *chains* these: `a < b < c` means `a < b && b < c` with `b`
    /// evaluated once, not C's `(a < b) < c`. The run is recorded as left-nested
    /// [`ExprKind::Binary`] nodes and recognised later through
    /// [`BinOp::is_relational`] - see the note on that method.
    fn hier9(&mut self, ctx: Ctx) -> Expr {
        self.plnge(
            ctx,
            &[
                (TokenKind::LtEq, BinOp::Le),
                (TokenKind::GtEq, BinOp::Ge),
                (TokenKind::Lt, BinOp::Lt),
                (TokenKind::Gt, BinOp::Gt),
            ],
            Self::hier8,
        )
    }

    /// `hier8()`: `|`.
    fn hier8(&mut self, ctx: Ctx) -> Expr {
        self.plnge(ctx, &[(TokenKind::Pipe, BinOp::BitOr)], Self::hier7)
    }

    /// `hier7()`: `^`.
    fn hier7(&mut self, ctx: Ctx) -> Expr {
        self.plnge(ctx, &[(TokenKind::Caret, BinOp::BitXor)], Self::hier6)
    }

    /// `hier6()`: `&`.
    fn hier6(&mut self, ctx: Ctx) -> Expr {
        self.plnge(ctx, &[(TokenKind::Amp, BinOp::BitAnd)], Self::hier5)
    }

    /// `hier5()`: `<<`, `>>`, `>>>`.
    fn hier5(&mut self, ctx: Ctx) -> Expr {
        self.plnge(
            ctx,
            &[
                (TokenKind::Shl, BinOp::Shl),
                (TokenKind::UShr, BinOp::ShrU),
                (TokenKind::Shr, BinOp::Shr),
            ],
            Self::hier4,
        )
    }

    /// `hier4()`: `+` and `-`.
    ///
    /// This is also where adjacent string literals are folded: the scanner
    /// concatenates `"a" + "b"` itself (`scanplus()` in `sc2.c`), but our scanner
    /// hands both literals to the parser instead.
    fn hier4(&mut self, ctx: Ctx) -> Expr {
        self.plnge(
            ctx,
            &[(TokenKind::Plus, BinOp::Add), (TokenKind::Minus, BinOp::Sub)],
            Self::hier3,
        )
    }

    /// `hier3()`: `*`, `/`, `%`.
    fn hier3(&mut self, ctx: Ctx) -> Expr {
        self.plnge(
            ctx,
            &[
                (TokenKind::Star, BinOp::Mul),
                (TokenKind::Slash, BinOp::Div),
                (TokenKind::Percent, BinOp::Mod),
            ],
            Self::hier2,
        )
    }

    /// `plnge()`: one left-associative binary level.
    ///
    /// `nextop()` clears `sc_allowproccall` as soon as it matches an operator,
    /// which is why the right operand is parsed without it.
    fn plnge(&mut self, ctx: Ctx, ops: &[(TokenKind, BinOp)], next: Rung<'a>) -> Expr {
        let start = self.cur_span();
        let mut lhs = next(self, ctx);
        while let Some(op) = self.nextop(ops) {
            let rhs = next(self, ctx.no_proccall());
            let span = start.to(self.prev_span());
            lhs = match fold_string_concat(op, &lhs, &rhs, span) {
                Some(folded) => folded,
                None => Expr {
                    kind: ExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        span,
                    },
                    span,
                },
            };
        }
        lhs
    }

    /// `nextop()`: consume the next operator of this level, if it is one of
    /// `ops`.
    fn nextop(&mut self, ops: &[(TokenKind, BinOp)]) -> Option<BinOp> {
        let &(_, op) = ops.iter().find(|(tk, _)| self.at(tk))?;
        self.bump();
        Some(op)
    }

    // =============================================================== hier2

    /// `hier2()`: prefix operators, the compile-time operators, tag overrides,
    /// and (after descending to `hier1`) the postfix operators.
    ///
    /// There is deliberately no unary `+`: the original's `switch` has no `'+'`
    /// case, so `+x` is a syntax error in Pawn.
    fn hier2(&mut self, ctx: Ctx) -> Expr {
        let start = self.cur_span();

        // --- prefix ---
        if let Some(op) = incdec_op(self.peek()) {
            self.bump();
            let operand = self.hier2(ctx.no_proccall());
            let span = start.to(self.prev_span());
            return Expr {
                kind: ExprKind::IncDec {
                    op,
                    fixity: Fixity::Prefix,
                    operand: Box::new(operand),
                    span,
                },
                span,
            };
        }
        if let Some(op) = un_op(self.peek()) {
            self.bump();
            let operand = self.hier2(ctx.no_proccall());
            let span = start.to(self.prev_span());
            return Expr {
                kind: ExprKind::Unary { op, operand: Box::new(operand), span },
                span,
            };
        }
        // `Float:x`, `_:x` - a tag override. The scanner cannot tell this from a
        // label, so the decision is positional: a `Name:` token in *operand*
        // position is a cast, unless we are in the first arm of a `?:`, where
        // the colon belongs to the conditional (`sc_allowtags`).
        if ctx.allow_tags && matches!(self.peek(), TokenKind::Label(_)) {
            let tag = self.eat_tag().expect("just peeked a label");
            let expr = self.hier2(ctx.no_proccall());
            let span = start.to(self.prev_span());
            return Expr {
                kind: ExprKind::Cast { tag, expr: Box::new(expr), span },
                span,
            };
        }
        match self.peek() {
            TokenKind::Defined => return self.parse_defined(),
            TokenKind::Sizeof => return self.parse_sizeof(),
            TokenKind::Tagof => return self.parse_tagof(),
            _ => {}
        }

        // --- operand plus postfix ---
        let mut expr = self.hier1(ctx);
        loop {
            // "Found a ';' / a newline that ends a statement, do not look
            // further for postfix operators" (sc3.c:1379).
            if self.at(&TokenKind::Semi) || self.tok().line_start {
                break;
            }
            if let Some(op) = incdec_op(self.peek()) {
                self.bump();
                let span = start.to(self.prev_span());
                expr = Expr {
                    kind: ExprKind::IncDec {
                        op,
                        fixity: Fixity::Postfix,
                        operand: Box::new(expr),
                        span,
                    },
                    span,
                };
                continue;
            }
            // `n char`: the number of cells needed for `n` packed characters.
            if self.eat(&TokenKind::Char) {
                let span = start.to(self.prev_span());
                expr = Expr {
                    kind: ExprKind::CharCells { operand: Box::new(expr), span },
                    span,
                };
                continue;
            }
            break;
        }
        expr
    }

    /// `defined SYMBOL` (`hier2()`, `case tDEFINED`). Takes a *symbol*, not an
    /// expression, and tolerates any number of parentheses around it.
    fn parse_defined(&mut self) -> Expr {
        let start = self.cur_span();
        self.bump(); // `defined`
        let parens = self.eat_repeated_lparens();
        let symbol = self.expect_ident().unwrap_or_else(|| Ident::new(String::new(), start));
        self.close_repeated_parens(parens);
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::Defined { symbol, span }, span }
    }

    /// `sizeof x`, `sizeof(x)`, `sizeof x[]`, `sizeof x[Field]`
    /// (`hier2()`, `case tSIZEOF`).
    fn parse_sizeof(&mut self) -> Expr {
        let start = self.cur_span();
        self.bump(); // `sizeof`
        let parens = self.eat_repeated_lparens();
        // The scanner glues `name` + an adjacent `:` into one Label token, so in
        // `case 1 .. sizeof g:` the operand arrives as Label("g") with the case's
        // terminating colon already swallowed. Unpack it back into the identifier
        // and let the caller know the colon is gone - the compiler avoids this by
        // clearing `sc_allowtags` inside `doswitch()`, which our scanner cannot do.
        let symbol = if let TokenKind::Label(name) = self.peek().clone() {
            let span = self.cur_span();
            self.bump();
            self.pending_label_colon = true;
            Ident::new(name, span)
        } else {
            self.expect_ident().unwrap_or_else(|| Ident::new(String::new(), start))
        };
        let levels = self.parse_sizeof_levels();
        self.close_repeated_parens(parens);
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::SizeOf { symbol, levels, span }, span }
    }

    /// `tagof x` or `tagof(Tag:)` (`hier2()`, `case tTAGOF`). Unlike `sizeof`
    /// the operand may be a tag name written directly, colon included.
    fn parse_tagof(&mut self) -> Expr {
        let start = self.cur_span();
        self.bump(); // `tagof`
        let parens = self.eat_repeated_lparens();
        let target = if matches!(self.peek(), TokenKind::Label(_)) {
            TagOfTarget::Tag(self.eat_tag().expect("just peeked a label"))
        } else {
            TagOfTarget::Symbol(
                self.expect_ident().unwrap_or_else(|| Ident::new(String::new(), start)),
            )
        };
        let levels = self.parse_sizeof_levels();
        self.close_repeated_parens(parens);
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::TagOf { target, levels, span }, span }
    }

    /// The `[]` / `[Field]` suffixes shared by `sizeof` and `tagof`.
    ///
    /// Only the innermost level may name an enumeration field; the original
    /// enforces that with the symbol table (`level==sym->dim.array.level`), which
    /// the parser cannot do, so every level accepts a name and the check moves to
    /// the semantic pass.
    fn parse_sizeof_levels(&mut self) -> Vec<SizeOfLevel> {
        let mut levels = Vec::new();
        while self.at(&TokenKind::LBracket) {
            let start = self.cur_span();
            self.bump();
            let field = self.eat_ident();
            self.expect(&TokenKind::RBracket);
            levels.push(SizeOfLevel { field, span: start.to(self.prev_span()) });
        }
        levels
    }

    /// `while (matchtoken('(')) paranthese++;`
    fn eat_repeated_lparens(&mut self) -> usize {
        let mut n = 0;
        while self.eat(&TokenKind::LParen) {
            n += 1;
        }
        n
    }

    /// `while (paranthese--) needtoken(')');`
    fn close_repeated_parens(&mut self, n: usize) {
        for _ in 0..n {
            self.expect(&TokenKind::RParen);
        }
    }

    // =============================================================== hier1

    /// `hier1()`: the tightest level - array indices and function calls, applied
    /// left to right (`goto restart` in the original).
    fn hier1(&mut self, ctx: Ctx) -> Expr {
        let start = self.cur_span();
        let was_name = matches!(self.peek(), TokenKind::Ident(_));
        let mut expr = self.primary(ctx);
        let mut suffixed = false;

        loop {
            let kind = match self.peek() {
                TokenKind::LBracket => IndexKind::Cell,
                TokenKind::LBrace => IndexKind::Char,
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_call_args(ctx, true);
                    self.expect(&TokenKind::RParen);
                    let span = start.to(self.prev_span());
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                            parenthesised: true,
                            span,
                        },
                        span,
                    };
                    suffixed = true;
                    continue;
                }
                _ => break,
            };
            self.bump();
            let index = self.hier14(ctx.no_proccall());
            let close =
                if kind == IndexKind::Cell { TokenKind::RBracket } else { TokenKind::RBrace };
            self.expect(&close);
            let span = start.to(self.prev_span());
            expr = Expr {
                kind: ExprKind::Index {
                    base: Box::new(expr),
                    index: Box::new(index),
                    kind,
                    span,
                },
                span,
            };
            suffixed = true;
        }

        // The parenthesis-less procedure call: `foo` / `client_print id, 1, "x"`
        // (`sc3.c:1663`). The compiler decides with the symbol table - a bare name
        // that resolves to a function *is* a call; the parser has no symbols, so
        // it takes the syntactic evidence instead. See `starts_bare_argument`.
        if ctx.allow_proccall && was_name && !suffixed && matches!(expr.kind, ExprKind::Ident(_)) {
            let (args, is_call) = if self.at_bare_call_end() {
                (Vec::new(), true)
            } else if self.starts_bare_argument() {
                (self.parse_call_args(ctx, false), true)
            } else {
                (Vec::new(), false)
            };
            if is_call {
                let span = start.to(self.prev_span());
                return Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                        parenthesised: false,
                        span,
                    },
                    span,
                };
            }
        }
        expr
    }

    /// The end of a parenthesis-less call that passes nothing:
    /// `close=matchtoken(tTERM)` in `callfunction()`, with the `.` look-ahead
    /// that lets a named argument continue on the next line.
    fn at_bare_call_end(&self) -> bool {
        if matches!(self.peek(), TokenKind::Semi | TokenKind::Eof | TokenKind::RBrace) {
            return true;
        }
        self.tok().line_start && !self.at(&TokenKind::Dot)
    }

    /// Whether the current token can begin the first argument of a
    /// parenthesis-less call.
    ///
    /// Deliberately narrower than the compiler's, which knows from the symbol
    /// table that a call is under way and therefore lets *any* expression follow.
    /// `-` `++` `--` are excluded because without symbols `foo -1` is
    /// indistinguishable from the subtraction `foo - 1`, and reading a binary
    /// operator as a call would corrupt far more code than the paren-less form
    /// buys. `(`, `[` and `{` never reach here - they are suffixes, consumed
    /// above.
    fn starts_bare_argument(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Ident(_)
                | TokenKind::Int(_)
                | TokenKind::Rational(_)
                | TokenKind::Str(_)
                | TokenKind::PackedStr(_)
                | TokenKind::Label(_)
                | TokenKind::Not
                | TokenKind::Tilde
                | TokenKind::Dot
                | TokenKind::Sizeof
                | TokenKind::Tagof
                | TokenKind::Defined
        )
    }

    /// `callfunction()`: the argument list, with or without parentheses.
    ///
    /// Handles the named form `.name = value` (which must follow all positional
    /// arguments, error 44) and the bare `_` placeholder that asks for the
    /// parameter's default value.
    fn parse_call_args(&mut self, ctx: Ctx, parenthesised: bool) -> Vec<Arg> {
        let mut args = Vec::new();
        if parenthesised && self.at(&TokenKind::RParen) {
            return args;
        }
        let mut named_seen = false;
        loop {
            let start = self.cur_span();
            let name = if self.at(&TokenKind::Dot) {
                self.bump();
                named_seen = true;
                let id = self.expect_ident();
                self.expect(&TokenKind::Assign);
                id
            } else {
                if named_seen {
                    let span = self.cur_span();
                    self.error(44, span, &[]); // positional after named
                }
                None
            };
            let value = if self.at_default_placeholder() {
                let span = self.cur_span();
                self.bump();
                ArgValue::Default(span)
            } else {
                ArgValue::Expr(self.hier14(ctx.no_proccall()))
            };
            args.push(Arg { name, value, span: start.to(self.prev_span()) });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if !parenthesised && self.at_eof() {
                break;
            }
        }
        args
    }

    /// A bare `_` argument. The compiler's scanner turns a lone underscore into
    /// its own token (`sc2.c:2039`); ours reports it as an identifier, so the
    /// look-ahead confirms nothing follows that would make it a normal operand.
    fn at_default_placeholder(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Ident(name) if name == "_") {
            return false;
        }
        matches!(
            self.peek_at(1),
            TokenKind::Comma | TokenKind::RParen | TokenKind::Semi | TokenKind::Eof
        ) || self.tok_at(1).line_start
    }

    // ============================================================= primary

    /// `primary()`: a parenthesised (possibly comma) expression, a name, or a
    /// constant.
    fn primary(&mut self, ctx: Ctx) -> Expr {
        let start = self.cur_span();

        if self.eat(&TokenKind::LParen) {
            // "allow tagnames to be used in parenthesized expressions" and
            // no procedure-call syntax inside them (sc3.c:1701).
            let inner = Ctx { allow_tags: true, allow_proccall: false };
            let mut exprs = vec![self.hier14(inner)];
            while self.eat(&TokenKind::Comma) {
                exprs.push(self.hier14(inner));
            }
            self.expect(&TokenKind::RParen);
            if exprs.len() == 1 {
                return exprs.pop().expect("just checked the length");
            }
            let span = start.to(self.prev_span());
            return Expr { kind: ExprKind::Comma { exprs, span }, span };
        }

        if let Some(id) = self.eat_ident() {
            let span = id.span;
            return Expr { kind: ExprKind::Ident(id), span };
        }

        // A `Name:` token where a tag override is not allowed (the first arm of
        // a `?:`): the compiler's scanner would not have produced a label at all,
        // so put the name back and let `hier13` account for the eaten colon.
        if !ctx.allow_tags && let TokenKind::Label(name) = self.peek() {
            let span = self.cur_span();
            self.bump();
            return Expr { kind: ExprKind::Ident(Ident::new(name.clone(), span)), span };
        }

        self.constant()
    }

    // ============================================================ constant

    /// `constant()`: numbers, rationals, characters, strings and the literal
    /// array `{1, 2, 3}`.
    fn constant(&mut self) -> Expr {
        let span = self.cur_span();
        match self.peek() {
            TokenKind::Int(v) => {
                let v = *v;
                self.bump();
                // Character literals are folded to a number by the scanner; the
                // source text is the only thing that still distinguishes `'\n'`
                // from `10`, and tooling wants that distinction back.
                let kind = if self.text(span).starts_with('\'') {
                    ExprKind::Char { value: v, span }
                } else {
                    ExprKind::Num(v)
                };
                Expr { kind, span }
            }
            TokenKind::Rational(v) => {
                let value = *v;
                self.bump();
                let raw = self.text(span).to_owned();
                Expr { kind: ExprKind::Rational(RationalLit { value, raw, span }), span }
            }
            TokenKind::Str(_) | TokenKind::PackedStr(_) => self.string_literal(),
            TokenKind::LBrace => self.literal_array(),
            _ => {
                // `constant()` returned FALSE: "expression error, assumed 0".
                self.error(29, span, &[]);
                // Structural tokens are left for the caller to resynchronise on;
                // anything else is consumed so the parser always makes progress.
                if !matches!(
                    self.peek(),
                    TokenKind::RParen
                        | TokenKind::RBracket
                        | TokenKind::RBrace
                        | TokenKind::Comma
                        | TokenKind::Semi
                        | TokenKind::Colon
                        | TokenKind::Eof
                ) {
                    self.bump();
                }
                Expr { kind: ExprKind::Error(span), span }
            }
        }
    }

    /// A string literal, folding the concatenated form.
    ///
    /// The compiler does this in the scanner: after a literal it calls
    /// `scanplus()` and, if a `+` follows, keeps appending segments
    /// (`sc2.c:2100`). Our scanner emits one token per segment and leaves the
    /// folding here, where `+` between two literals is unambiguous. Simple
    /// adjacency (`"a" "b"`) is folded too - nothing else could be meant by it.
    fn string_literal(&mut self) -> Expr {
        let start = self.cur_span();
        let (mut value, packed) = match self.peek() {
            TokenKind::Str(s) => (s.clone(), false),
            TokenKind::PackedStr(s) => (s.clone(), true),
            _ => unreachable!("only called on a string token"),
        };
        self.bump();
        loop {
            let next = match (self.peek(), self.peek_at(1)) {
                (TokenKind::Str(s), _) | (TokenKind::PackedStr(s), _) => {
                    let s = s.clone();
                    self.bump();
                    s
                }
                (TokenKind::Plus, TokenKind::Str(s) | TokenKind::PackedStr(s)) => {
                    let s = s.clone();
                    self.bump();
                    self.bump();
                    s
                }
                _ => break,
            };
            value.push_str(&next);
        }
        let span = start.to(self.prev_span());
        // `raw` is not recoverable from the token stream: the scanner resolves
        // (or preserves) escapes itself and does not record which spelling was
        // used, so the flag is read back from the source text.
        let raw = self.text(start).contains('^');
        Expr {
            kind: ExprKind::Str(StringLit { value, packed, raw, span }),
            span,
        }
    }

    /// `constant()`, `case '{'`: a literal array in expression position.
    fn literal_array(&mut self) -> Expr {
        let start = self.cur_span();
        self.bump(); // `{`
        let mut elems = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            loop {
                elems.push(self.hier14(Ctx::top()));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.at(&TokenKind::RBrace) || self.at_eof() {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RBrace);
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::LitArray { elems, span }, span }
    }

    // ============================================================== helpers

    /// The kind of the token just consumed, if any.
    fn prev_kind(&self) -> Option<&'a TokenKind> {
        (self.pos > 0).then(|| &self.tokens[(self.pos - 1).min(self.tokens.len() - 1)].kind)
    }
}

/// `hier14()`'s assignment `switch`: `Some(None)` is a plain `=`, `Some(Some(op))`
/// a compound assignment, `None` not an assignment at all.
#[allow(clippy::option_option)]
fn assign_op(tok: &TokenKind) -> Option<Option<BinOp>> {
    Some(match tok {
        TokenKind::Assign => None,
        TokenKind::PlusAssign => Some(BinOp::Add),
        TokenKind::MinusAssign => Some(BinOp::Sub),
        TokenKind::StarAssign => Some(BinOp::Mul),
        TokenKind::SlashAssign => Some(BinOp::Div),
        TokenKind::PercentAssign => Some(BinOp::Mod),
        TokenKind::ShlAssign => Some(BinOp::Shl),
        TokenKind::ShrAssign => Some(BinOp::Shr),
        TokenKind::UShrAssign => Some(BinOp::ShrU),
        TokenKind::AndAssign => Some(BinOp::BitAnd),
        TokenKind::OrAssign => Some(BinOp::BitOr),
        TokenKind::XorAssign => Some(BinOp::BitXor),
        _ => return None,
    })
}

/// The prefix operators `hier2()` accepts. There is no `+`.
fn un_op(tok: &TokenKind) -> Option<UnOp> {
    Some(match tok {
        TokenKind::Minus => UnOp::Neg,
        TokenKind::Not => UnOp::LogNot,
        TokenKind::Tilde => UnOp::BitNot,
        _ => return None,
    })
}

fn incdec_op(tok: &TokenKind) -> Option<IncDecOp> {
    Some(match tok {
        TokenKind::PlusPlus => IncDecOp::Inc,
        TokenKind::MinusMinus => IncDecOp::Dec,
        _ => return None,
    })
}

/// `"a" + "b"` is one string literal, not an addition (`scanplus()`).
fn fold_string_concat(op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> Option<Expr> {
    if op != BinOp::Add {
        return None;
    }
    let (ExprKind::Str(a), ExprKind::Str(b)) = (&lhs.kind, &rhs.kind) else { return None };
    let lit = StringLit {
        value: format!("{}{}", a.value, b.value),
        packed: a.packed,
        raw: a.raw,
        span,
    };
    Some(Expr { kind: ExprKind::Str(lit), span })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpc_diag::Diagnostics;
    use zpc_lex::{Scanner, Token};

    fn lex(src: &str) -> Vec<Token> {
        let mut d = Diagnostics::new();
        let toks = Scanner::new(src, "test.sma").scan(&mut d);
        assert!(d.items().is_empty(), "scanner errors in {src:?}: {:?}", d.items());
        toks
    }

    /// Parse `src` as an expression, asserting it is diagnostic-free.
    fn expr(src: &str) -> Expr {
        let toks = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        let e = p.parse_expr();
        assert!(p.diags.items().is_empty(), "{src:?} produced {:?}", p.diags.items());
        assert!(p.at_eof(), "{src:?} left tokens unconsumed: {:?}", p.peek());
        e
    }

    /// Parse `src` in statement position (parenthesis-less calls enabled).
    fn stmt_expr(src: &str) -> Expr {
        let toks = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        let e = p.parse_expr_stmt();
        assert!(p.diags.items().is_empty(), "{src:?} produced {:?}", p.diags.items());
        e
    }

    fn diags_of(src: &str) -> Vec<u16> {
        let toks = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        let _ = p.parse_expr();
        p.diags.items().iter().map(|d| d.code).collect()
    }

    /// A parenthesised s-expression rendering, so tests can assert on shape.
    fn sexpr(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Num(v) => v.to_string(),
            ExprKind::Char { value, .. } => format!("char:{value}"),
            ExprKind::Rational(r) => format!("rat:{}", r.raw),
            ExprKind::Str(s) => {
                let mut out = format!("str:{}", s.value);
                if s.packed {
                    out.push_str(":packed");
                }
                out
            }
            ExprKind::LitArray { elems, .. } => {
                format!("{{{}}}", elems.iter().map(sexpr).collect::<Vec<_>>().join(" "))
            }
            ExprKind::Ident(id) => id.name.clone(),
            ExprKind::Comma { exprs, .. } => {
                format!("(, {})", exprs.iter().map(sexpr).collect::<Vec<_>>().join(" "))
            }
            ExprKind::Unary { op, operand, .. } => format!("({op:?} {})", sexpr(operand)),
            ExprKind::IncDec { op, fixity, operand, .. } => {
                format!("({op:?}/{fixity:?} {})", sexpr(operand))
            }
            ExprKind::Binary { op, lhs, rhs, .. } => {
                format!("({op:?} {} {})", sexpr(lhs), sexpr(rhs))
            }
            ExprKind::Assign { op, target, value, .. } => {
                let name = op.map_or_else(|| "=".to_owned(), |o| format!("{o:?}="));
                format!("({name} {} {})", sexpr(target), sexpr(value))
            }
            ExprKind::Ternary { cond, then_expr, else_expr, .. } => {
                format!("(?: {} {} {})", sexpr(cond), sexpr(then_expr), sexpr(else_expr))
            }
            ExprKind::Index { base, index, kind, .. } => {
                let (o, c) = if *kind == IndexKind::Cell { ("[", "]") } else { ("{", "}") };
                format!("(idx {} {o}{}{c})", sexpr(base), sexpr(index))
            }
            ExprKind::Call { callee, args, parenthesised, .. } => {
                let tag = if *parenthesised { "call" } else { "proccall" };
                let args = args
                    .iter()
                    .map(|a| {
                        let v = match &a.value {
                            ArgValue::Expr(e) => sexpr(e),
                            ArgValue::Default(_) => "_".to_owned(),
                        };
                        match &a.name {
                            Some(n) => format!(".{}={v}", n.name),
                            None => v,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({tag} {}{}{args})", sexpr(callee), if args.is_empty() { "" } else { " " })
            }
            ExprKind::Cast { tag, expr, .. } => {
                format!("(cast {}: {})", tag.name.name, sexpr(expr))
            }
            ExprKind::SizeOf { symbol, levels, .. } => {
                format!("(sizeof {}{})", symbol.name, render_levels(levels))
            }
            ExprKind::TagOf { target, levels, .. } => {
                let t = match target {
                    TagOfTarget::Symbol(id) => id.name.clone(),
                    TagOfTarget::Tag(t) => format!("{}:", t.name.name),
                };
                format!("(tagof {t}{})", render_levels(levels))
            }
            ExprKind::Defined { symbol, .. } => format!("(defined {})", symbol.name),
            ExprKind::CharCells { operand, .. } => format!("(char {})", sexpr(operand)),
            ExprKind::Error(_) => "<error>".to_owned(),
        }
    }

    fn render_levels(levels: &[SizeOfLevel]) -> String {
        levels
            .iter()
            .map(|l| match &l.field {
                Some(f) => format!("[{}]", f.name),
                None => "[]".to_owned(),
            })
            .collect()
    }

    fn assert_shape(src: &str, expected: &str) {
        assert_eq!(sexpr(&expr(src)), expected, "for {src:?}");
    }

    // ------------------------------------------------------- the whole ladder

    #[test]
    fn each_tier_binds_tighter_than_the_one_above_it() {
        // hier3 over hier4
        assert_shape("a + b * c", "(Add a (Mul b c))");
        // hier4 over hier5
        assert_shape("a << b + c", "(Shl a (Add b c))");
        // hier5 over hier6
        assert_shape("a & b << c", "(BitAnd a (Shl b c))");
        // hier6 over hier7
        assert_shape("a ^ b & c", "(BitXor a (BitAnd b c))");
        // hier7 over hier8
        assert_shape("a | b ^ c", "(BitOr a (BitXor b c))");
        // hier8 over hier9
        assert_shape("a < b | c", "(Lt a (BitOr b c))");
        // hier9 over hier10
        assert_shape("a == b < c", "(Eq a (Lt b c))");
        // hier10 over hier11
        assert_shape("a && b == c", "(LogAnd a (Eq b c))");
        // hier11 over hier12
        assert_shape("a || b && c", "(LogOr a (LogAnd b c))");
        // hier12 over hier13
        assert_shape("a ? b : c || d", "(?: a b (LogOr c d))");
        // hier13 over hier14
        assert_shape("a = b ? c : d", "(= a (?: b c d))");
    }

    #[test]
    fn the_precedence_table_agrees_with_the_ladder() {
        // The AST's table is what a precedence-climbing consumer would use; it
        // must order the operators exactly as the hier ladder does.
        let ladder = [
            BinOp::Mul,
            BinOp::Add,
            BinOp::Shl,
            BinOp::BitAnd,
            BinOp::BitXor,
            BinOp::BitOr,
            BinOp::Lt,
            BinOp::Eq,
            BinOp::LogAnd,
            BinOp::LogOr,
        ];
        for pair in ladder.windows(2) {
            assert!(
                pair[0].precedence() > pair[1].precedence(),
                "{:?} must bind tighter than {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn binary_levels_are_left_associative() {
        assert_shape("a - b - c", "(Sub (Sub a b) c)");
        assert_shape("a / b / c", "(Div (Div a b) c)");
        assert_shape("a << 2 >> 1", "(Shr (Shl a 2) 1)");
        assert_shape("a || b || c", "(LogOr (LogOr a b) c)");
    }

    #[test]
    fn parentheses_override_precedence() {
        assert_shape("(a + b) * c", "(Mul (Add a b) c)");
        assert_shape("(a | b) & (a ^ b)", "(BitAnd (BitOr a b) (BitXor a b))");
    }

    // ------------------------------------------------------ hier14 / hier13

    #[test]
    fn assignment_is_right_associative_and_chains() {
        assert_shape("a = b = c", "(= a (= b c))");
        assert_shape("a = b = c = 1", "(= a (= b (= c 1)))");
    }

    #[test]
    fn every_compound_assignment_is_recognised() {
        assert_shape("x += a", "(Add= x a)");
        assert_shape("x -= a", "(Sub= x a)");
        assert_shape("x *= a", "(Mul= x a)");
        assert_shape("x /= a", "(Div= x a)");
        assert_shape("x %= a", "(Mod= x a)");
        assert_shape("x <<= 1", "(Shl= x 1)");
        assert_shape("x >>= 1", "(Shr= x 1)");
        assert_shape("x >>>= 1", "(ShrU= x 1)");
        assert_shape("x &= a", "(BitAnd= x a)");
        assert_shape("x |= a", "(BitOr= x a)");
        assert_shape("x ^= a", "(BitXor= x a)");
    }

    #[test]
    fn ternaries_nest_to_the_right() {
        assert_shape("a ? b : c ? d : e", "(?: a b (?: c d e))");
        assert_shape("a ? b ? c : d : e", "(?: a (?: b c d) e)");
        assert_shape("a > b ? a : b", "(?: (Gt a b) a b)");
    }

    #[test]
    fn a_label_in_the_first_ternary_arm_is_a_name_not_a_tag() {
        // The scanner glues `b:` into one Label token; `sc_allowtags` is off in
        // the first arm, so it has to read back as `b` plus the conditional's `:`.
        assert_shape("a ? b: c", "(?: a b c)");
        assert_shape("a ? b : c", "(?: a b c)");
        // ...but in the second arm tags are allowed again.
        assert_shape("a ? b : Float:c", "(?: a b (cast Float: c))");
    }

    // ----------------------------------------------------- relational chains

    #[test]
    fn relational_operators_chain() {
        // Pawn reads this as `a < b && b < c`, not `(a < b) < c`. The AST keeps
        // the run as left-nested relational nodes for the semantic pass.
        let e = expr("a < b < c");
        assert_shape("a < b < c", "(Lt (Lt a b) c)");
        let ExprKind::Binary { op, lhs, .. } = &e.kind else { panic!("expected a binary node") };
        assert!(op.is_relational());
        let ExprKind::Binary { op: inner, .. } = &lhs.kind else { panic!("expected a chain") };
        assert!(inner.is_relational(), "a run of relational operators is one chain");
        assert_shape("a <= b >= c > d", "(Gt (Ge (Le a b) c) d)");
    }

    #[test]
    fn equality_does_not_chain_with_relationals() {
        // `==` is a separate tier, so it cannot join the chain.
        let e = expr("a < b == c");
        let ExprKind::Binary { op, .. } = &e.kind else { panic!("expected a binary node") };
        assert_eq!(*op, BinOp::Eq);
        assert!(!op.is_relational());
    }

    // --------------------------------------------------------------- hier2

    #[test]
    fn prefix_operators() {
        assert_shape("-a", "(Neg a)");
        assert_shape("~a", "(BitNot a)");
        assert_shape("!a", "(LogNot a)");
        assert_shape("-!~a", "(Neg (LogNot (BitNot a)))");
        assert_shape("a && b || !a", "(LogOr (LogAnd a b) (LogNot a))");
    }

    #[test]
    fn there_is_no_unary_plus() {
        // `hier2()` has no '+' case: the operand is missing, so `constant()`
        // fails with error 29.
        assert_eq!(diags_of("+a"), vec![29]);
    }

    #[test]
    fn increment_and_decrement_in_both_fixities() {
        assert_shape("++x", "(Inc/Prefix x)");
        assert_shape("--x", "(Dec/Prefix x)");
        assert_shape("x++", "(Inc/Postfix x)");
        assert_shape("x--", "(Dec/Postfix x)");
        assert_shape("-x++", "(Neg (Inc/Postfix x))");
    }

    #[test]
    fn a_line_break_stops_the_search_for_postfix_operators() {
        // `x` then `++y`, not `x++` then `y` (sc3.c:1379).
        let src = "x\n++y";
        let toks = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        assert_eq!(sexpr(&p.parse_expr()), "x");
        assert_eq!(sexpr(&p.parse_expr()), "(Inc/Prefix y)");
    }

    #[test]
    fn the_postfix_char_operator() {
        assert_shape("n char", "(char n)");
        assert_shape("32 char", "(char 32)");
        // it binds tighter than the arithmetic around it
        assert_shape("1 + n char", "(Add 1 (char n))");
    }

    #[test]
    fn tag_casts() {
        assert_shape("Float:x", "(cast Float: x)");
        assert_shape("_:f", "(cast _: f)");
        // a cast covers the whole hier2 operand, but not a binary operator
        assert_shape("Float:a + b", "(Add (cast Float: a) b)");
        assert_shape("bool:!x", "(cast bool: (LogNot x))");
    }

    #[test]
    fn sizeof_takes_a_symbol_and_tolerates_repeated_parentheses() {
        assert_shape("sizeof arr", "(sizeof arr)");
        assert_shape("sizeof(arr)", "(sizeof arr)");
        assert_shape("sizeof(((arr)))", "(sizeof arr)");
        assert_shape("sizeof arr[]", "(sizeof arr[])");
        assert_shape("sizeof arr[][]", "(sizeof arr[][])");
    }

    #[test]
    fn the_innermost_sizeof_bracket_may_name_an_enum_field() {
        assert_shape("sizeof arr[Coords]", "(sizeof arr[Coords])");
        assert_shape("sizeof(players[][Data])", "(sizeof players[][Data])");
    }

    #[test]
    fn tagof_takes_a_symbol_or_a_tag_name() {
        assert_shape("tagof x", "(tagof x)");
        assert_shape("tagof(Float:)", "(tagof Float:)");
        assert_shape("tagof(arr[Coords])", "(tagof arr[Coords])");
    }

    #[test]
    fn defined_takes_a_symbol() {
        assert_shape("defined FOO", "(defined FOO)");
        assert_shape("defined(FOO)", "(defined FOO)");
        assert_shape("!defined(FOO)", "(LogNot (defined FOO))");
    }

    #[test]
    fn sizeof_of_a_non_symbol_is_error_20() {
        assert_eq!(diags_of("sizeof 3"), vec![20]);
    }

    // --------------------------------------------------------------- hier1

    #[test]
    fn subscripts_distinguish_cell_from_char() {
        assert_shape("arr[i]", "(idx arr [i])");
        assert_shape("arr{i}", "(idx arr {i})");
        assert_shape("arr[i][j]", "(idx (idx arr [i]) [j])");
        assert_shape("arr[i]{j}", "(idx (idx arr [i]) {j})");
        // the subscript itself is a full expression
        assert_shape("arr[i + 1]", "(idx arr [(Add i 1)])");
    }

    #[test]
    fn calls_and_their_suffixes() {
        assert_shape("foo()", "(call foo)");
        assert_shape("foo(1, 2)", "(call foo 1 2)");
        assert_shape("foo(a)[2]", "(idx (call foo a) [2])");
        assert_shape("float(a)", "(call float a)");
        assert_shape("floatround(f)", "(call floatround f)");
    }

    #[test]
    fn named_arguments_and_the_default_placeholder() {
        assert_shape("foo(.b = 1)", "(call foo .b=1)");
        assert_shape("foo(1, .b = 2, .c = 3)", "(call foo 1 .b=2 .c=3)");
        assert_shape("foo(_)", "(call foo _)");
        assert_shape("foo(1, _, 3)", "(call foo 1 _ 3)");
        assert_shape("foo(.b = _)", "(call foo .b=_)");
        // `_` is only a placeholder when it stands alone
        assert_shape("foo(_ + 1)", "(call foo (Add _ 1))");
    }

    #[test]
    fn a_positional_argument_after_a_named_one_is_error_44() {
        assert_eq!(diags_of("foo(.b = 1, 2)"), vec![44]);
    }

    // ------------------------------------------- parenthesis-less procedure calls

    #[test]
    fn a_bare_name_in_statement_position_is_a_call() {
        assert_eq!(sexpr(&stmt_expr("foo;")), "(proccall foo)");
        assert_eq!(sexpr(&stmt_expr("foo")), "(proccall foo)");
    }

    #[test]
    fn a_parenthesis_less_call_takes_a_comma_separated_argument_list() {
        assert_eq!(
            sexpr(&stmt_expr(r#"client_print id, print_chat, "x""#)),
            "(proccall client_print id print_chat str:x)"
        );
        assert_eq!(sexpr(&stmt_expr("foo .a = 1")), "(proccall foo .a=1)");
    }

    #[test]
    fn the_paren_less_form_does_not_disturb_ordinary_statements() {
        // Everything that an expression statement can start with must still
        // parse as an expression, not as a call to its first name.
        assert_eq!(sexpr(&stmt_expr("x = 1")), "(= x 1)");
        assert_eq!(sexpr(&stmt_expr("x += a")), "(Add= x a)");
        assert_eq!(sexpr(&stmt_expr("x++")), "(Inc/Postfix x)");
        assert_eq!(sexpr(&stmt_expr("x - 1")), "(Sub x 1)");
        assert_eq!(sexpr(&stmt_expr("arr[0] = 1")), "(= (idx arr [0]) 1)");
        assert_eq!(sexpr(&stmt_expr("foo(1)")), "(call foo 1)");
        assert_eq!(sexpr(&stmt_expr("a && b")), "(LogAnd a b)");
    }

    #[test]
    fn the_paren_less_form_is_off_outside_statement_position() {
        // `parse_expr` never produces it, which is what keeps declaration
        // initialisers (`new x = y`) from becoming calls.
        assert_shape("y", "y");
        // `hier14` is the entry point, so a top-level comma is left for the
        // caller (declarators, argument lists) rather than swallowed.
        let src = "a, b";
        let toks = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        assert_eq!(sexpr(&p.parse_expr()), "a");
        assert!(p.at(&TokenKind::Comma));
    }

    #[test]
    fn a_sub_expression_in_parentheses_disables_the_paren_less_form() {
        assert_eq!(sexpr(&stmt_expr("(foo)")), "foo");
    }

    // ------------------------------------------------------------ constants

    #[test]
    fn literals() {
        assert_shape("42", "42");
        assert_shape("0xDEAD", "57005");
        assert_shape("1.5", "rat:1.5");
        assert_shape("'a'", "char:97");
        assert_shape("'^n'", "char:10");
        assert_shape(r#""hi""#, "str:hi");
        assert_shape(r#"!"hi""#, "str:hi:packed");
    }

    #[test]
    fn string_literals_concatenate() {
        // The compiler folds `"a" + "b"` in the scanner (`scanplus()`); we do it
        // here instead.
        assert_shape(r#""a" + "b""#, "str:ab");
        assert_shape(r#""a" "b""#, "str:ab");
        assert_shape(r#""a" + "b" + "c""#, "str:abc");
        // ...but a `+` with a non-literal operand is still an addition
        assert_shape(r#""a" + b"#, "(Add str:a b)");
    }

    #[test]
    fn literal_arrays_in_expression_position() {
        assert_shape("{1, 2, 3}", "{1 2 3}");
        assert_shape("{1}", "{1}");
        assert_shape("{}", "{}");
        assert_shape("{1 + 2, 3}", "{(Add 1 2) 3}");
        assert_shape("foo({1, 2})", "(call foo {1 2})");
    }

    #[test]
    fn comma_expressions_exist_only_inside_parentheses() {
        assert_shape("(a, b)", "(, a b)");
        assert_shape("(a, b) + c", "(Add (, a b) c)");
        assert_shape("(a)", "a");
    }

    // ------------------------------------------------------------- recovery

    #[test]
    fn an_unparsable_operand_reports_error_29_once_and_makes_progress() {
        assert_eq!(diags_of("a + ;"), vec![29]);
        assert_eq!(diags_of("*"), vec![29]);
    }

    #[test]
    fn an_unclosed_subscript_reports_error_1() {
        assert_eq!(diags_of("arr[i"), vec![1]);
    }

    // ---------------------------------------------------- the fixture's cases

    #[test]
    fn the_expression_fixture_parses() {
        // Every expression from `expressions()` in
        // `crates/zpc/tests/fixtures/stmt_expr_edge_cases.sma`.
        for src in [
            "x = a + b * 2 - 1",
            "x = (a | b) & (a ^ b)",
            "x = a << 2 >> 1",
            "x = a >>> 3",
            "x = a && b || !a",
            "x = a > b ? a : b",
            "x += a",
            "x >>>= 1",
            "x = -a",
            "x = ~a",
            "x = !a",
            "x++",
            "++x",
            "x--",
            "--x",
            "x = sizeof(arr)",
            "x = charsmax(arr)",
            "float(a)",
            "x = floatround(f)",
            "x = _:f",
            r#"register_plugin("stmt/expr edge cases", "1.0", "zpc")"#,
            "control_flow(1)",
            "expressions(2, 3)",
        ] {
            let _ = expr(src);
        }
    }

    #[test]
    fn spans_cover_the_whole_expression() {
        let src = "a + b * c";
        let e = expr(src);
        assert_eq!(e.span.start, 0);
        assert_eq!(e.span.end as usize, src.len());
    }
}
