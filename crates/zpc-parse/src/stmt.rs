//! Statement parsing.
//!
//! Ported from `statement()`, `compound()`, `doif()`, `dowhile()`, `dodo()`,
//! `dofor()`, `doswitch()`, `doreturn()`, `dobreak()`, `docont()`, `dogoto()`,
//! `dolabel()`, `doassert()`, `doexit()`, `dosleep()` and `dostate()` in
//! `compiler/libpc300/sc1.c`; `#emit` is `case tpEMIT` in `command()` (`sc2.c`).
//!
//! # Pawn quirks this file exists to get right
//!
//! * **Pawn's `switch` is not C's.** Cases never fall through, so `break` has no
//!   meaning in one; each clause takes *exactly one* statement (error 2 for a
//!   second); a clause may list several values (`case 1, 2:`) or an inclusive
//!   range (`case 1 .. 5:`); `default` must be the last clause (error 15) and may
//!   appear only once (error 16).
//! * **Semicolons are optional.** With the default `#pragma semicolon 0` a line
//!   break ends a statement, which is what [`Parser::at_terminator`] encodes.
//! * **Any body may be braceless.** `if (x) foo();` and `for (;;) bar();` are the
//!   norm in legacy AMXX plugins, so every body goes through [`Parser::parse_stmt`]
//!   rather than through [`Parser::parse_block`].
//! * **`name:` at statement position is a label, full stop.** The scanner cannot
//!   tell a label from a tag override - both are [`TokenKind::Label`] - and
//!   neither can `lex()`. `statement()` resolves it positionally: `case tLABEL:`
//!   in its dispatch calls `dolabel()` unconditionally, *before* the expression
//!   fallthrough is ever reached. So a statement can never begin with a tag cast,
//!   and this parser reproduces that exactly.
//! * **Declarations are statements.** `new`, `static`, `const` and `enum` inside a
//!   block route to the same routines file scope uses; outside a compound block
//!   `new`/`static` is error 3.

use zpc_ast::{
    Ident, Span, StateRef,
    decl::{Item, VarModifiers},
    expr::{Expr, ExprKind},
    stmt::{Block, CaseLabel, EmitOperand, EmitStmt, ForInit, Stmt, SwitchCase},
};
use zpc_lex::TokenKind;

use crate::Parser;

impl Parser<'_> {
    // ------------------------------------------------------------- compound

    /// `compound()`: a `{ ... }` block.
    ///
    /// `{}` is the empty statement. An unterminated block is error 30, reported
    /// against the opening brace so the message points at the culprit.
    pub(crate) fn parse_block(&mut self) -> Block {
        let start = self.cur_span();
        self.expect(&TokenKind::LBrace);
        let mut stmts = Vec::new();
        loop {
            if self.eat(&TokenKind::RBrace) {
                break;
            }
            if self.at_eof() {
                self.error(30, start, &[]); // compound statement not closed at EOF
                break;
            }
            let before = self.pos;
            stmts.push(self.parse_stmt());
            // Every branch below consumes at least one token; this guards against a
            // future edit turning the loop infinite.
            if self.pos == before {
                self.bump();
            }
        }
        Block { stmts, span: start.to(self.prev_span()) }
    }

    // ------------------------------------------------------------ statement

    /// `statement(NULL, TRUE)`: one statement, declarations allowed.
    pub(crate) fn parse_stmt(&mut self) -> Stmt {
        self.parse_statement(true)
    }

    /// `statement(NULL, FALSE)`: the body of an `if`/`while`/`for`/`case`, where a
    /// bare `new` is error 3 - a local needs a compound block of its own.
    fn parse_stmt_nodecl(&mut self) -> Stmt {
        self.parse_statement(false)
    }

    fn parse_statement(&mut self, allow_decl: bool) -> Stmt {
        let start = self.cur_span();
        match self.peek() {
            TokenKind::LBrace => Stmt::Block(self.parse_block()),
            TokenKind::Semi => {
                self.bump();
                self.error(36, start, &[]); // empty statement
                Stmt::Empty(start)
            }
            // A statement was required and the block (or the file) ended instead.
            TokenKind::RBrace | TokenKind::Eof => {
                self.error(36, start, &[]);
                Stmt::Empty(Span::at(start.start))
            }
            TokenKind::New | TokenKind::Static => self.parse_local_var(allow_decl),
            TokenKind::Const => Stmt::Const(self.parse_const_decl()),
            TokenKind::Enum => Stmt::Enum(self.parse_enum_decl()),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::Do => self.parse_do(),
            TokenKind::For => self.parse_for(),
            TokenKind::Switch => self.parse_switch(),
            TokenKind::Case | TokenKind::Default => {
                self.error(14, start, &[]); // invalid statement; not in switch
                self.resync_stmt();
                Stmt::Error(start.to(self.prev_span()))
            }
            TokenKind::Goto => self.parse_goto(),
            // `dolabel()`. See the module header: at statement position a
            // `name:` token is always a label, never a tag override.
            TokenKind::Label(_) => {
                let TokenKind::Label(name) = self.peek() else { unreachable!() };
                let name = Ident::new(name.clone(), start);
                self.bump();
                Stmt::Label { name, span: start }
            }
            TokenKind::Return => self.parse_return(),
            TokenKind::Break => {
                self.bump();
                self.eat_terminator();
                Stmt::Break { span: start.to(self.prev_span()) }
            }
            TokenKind::Continue => {
                self.bump();
                self.eat_terminator();
                Stmt::Continue { span: start.to(self.prev_span()) }
            }
            TokenKind::Exit => {
                self.bump();
                let value = self.parse_optional_value();
                Stmt::Exit { value, span: start.to(self.prev_span()) }
            }
            TokenKind::Sleep => {
                self.bump();
                let value = self.parse_optional_value();
                Stmt::Sleep { value, span: start.to(self.prev_span()) }
            }
            TokenKind::Assert => self.parse_assert(),
            TokenKind::State => self.parse_state(),
            TokenKind::PpEmit => self.parse_emit(),
            // Any other directive has already been consumed by the preprocessor;
            // the scanner still emits it, so drop its whole line.
            k if is_stmt_directive(k) => {
                self.bump();
                while !self.at_eof() && !self.tok().line_start {
                    self.bump();
                }
                Stmt::Empty(start.to(self.prev_span()))
            }
            // "non-empty expression". `sc_allowproccall` is on here, which is what
            // lets `foo;` and `client_print id, 0, "hi"` be calls; recording that is
            // the expression parser's job.
            _ => {
                // Statement position: this is the entry point that enables the
                // paren-less call form AND the comma operator, both of which
                // `parse_expr` deliberately excludes.
                let expr = self.parse_expr_stmt();
                self.eat_terminator();
                Stmt::Expr { expr, span: start.to(self.prev_span()) }
            }
        }
    }

    // ---------------------------------------------------- local declarations

    /// `declloc()`: `new`/`static` (with optional `const`) inside a function.
    fn parse_local_var(&mut self, allow_decl: bool) -> Stmt {
        let start = self.cur_span();
        if !allow_decl {
            self.error(3, start, &[]); // local declaration needs a compound block
        }
        // `getclassspec()` restricted to what is legal on a local: `new`/`static`,
        // either of which may be followed by `const`. Repeating one is error 42.
        let mut mods = VarModifiers::default();
        let mut dup: Option<Span> = None;
        let mut seen_new = false;
        loop {
            let span = self.cur_span();
            let already = match self.peek() {
                TokenKind::New => std::mem::replace(&mut seen_new, true),
                TokenKind::Static => std::mem::replace(&mut mods.static_, true),
                TokenKind::Const => std::mem::replace(&mut mods.is_const, true),
                _ => break,
            };
            self.bump();
            if already {
                dup = Some(span);
                break;
            }
        }
        if let Some(span) = dup {
            self.error(42, span, &[]); // invalid combination of class specifiers
        }
        let Item::Var(decl) = self.parse_var_decl(mods, start, None, None) else {
            return Stmt::Error(start.to(self.prev_span()));
        };
        Stmt::Var(decl)
    }

    // -------------------------------------------------------------- control

    /// `doif()`. Both branches are plain statements, so `if (x) foo();` needs no
    /// braces and `else if` is just an `if` in the else branch.
    fn parse_if(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let cond = self.parse_paren_cond();
        let then_branch = Box::new(self.parse_stmt_nodecl());
        let else_branch =
            self.eat(&TokenKind::Else).then(|| Box::new(self.parse_stmt_nodecl()));
        Stmt::If { cond, then_branch, else_branch, span: start.to(self.prev_span()) }
    }

    /// `dowhile()`.
    fn parse_while(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let cond = self.parse_paren_cond();
        let body = Box::new(self.parse_stmt_nodecl());
        Stmt::While { cond, body, span: start.to(self.prev_span()) }
    }

    /// `dodo()`: `do stmt while (cond);` - note the trailing terminator.
    fn parse_do(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let body = Box::new(self.parse_stmt_nodecl());
        self.expect(&TokenKind::While);
        let cond = self.parse_paren_cond();
        self.eat_terminator();
        Stmt::DoWhile { body, cond, span: start.to(self.prev_span()) }
    }

    /// `dofor()`. All three clauses are optional and the first may declare the
    /// loop variable, which gets a scope one level deeper than the enclosing block.
    fn parse_for(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        self.expect(&TokenKind::LParen);

        let init = if self.eat(&TokenKind::Semi) {
            None
        } else if matches!(self.peek(), TokenKind::New | TokenKind::Static) {
            // `parse_var_decl` consumes the `;` itself (`eat_terminator`).
            match self.parse_local_var(true) {
                Stmt::Var(v) => Some(ForInit::Decl(v)),
                _ => None,
            }
        } else {
            let e = self.parse_comma_expr();
            self.expect(&TokenKind::Semi);
            Some(ForInit::Expr(e))
        };

        let cond = if self.eat(&TokenKind::Semi) {
            None
        } else {
            let e = self.parse_comma_expr();
            self.expect(&TokenKind::Semi);
            Some(e)
        };

        let step = if self.at(&TokenKind::RParen) { None } else { Some(self.parse_comma_expr()) };
        self.expect(&TokenKind::RParen);

        let body = Box::new(self.parse_stmt_nodecl());
        Stmt::For { init, cond, step, body, span: start.to(self.prev_span()) }
    }

    // --------------------------------------------------------------- switch

    /// `doswitch()`.
    ///
    /// The rules enforced here, with the codes `doswitch()` uses:
    ///
    /// * a `case` after `default` - error 15 (`"default" case must be the last`),
    /// * a second `default` - error 16 (`multiple defaults in "switch"`),
    /// * anything other than `case`/`default`/`}` in the body - error 2 (`only a
    ///   single statement (or expression) can follow each "case"`), which is also
    ///   what a second statement under a clause produces, since the first one has
    ///   already been taken.
    ///
    /// A reversed or empty range is error 50, but that needs the values folded, so
    /// it is left to the semantic pass; the AST keeps [`CaseLabel::Range`] distinct
    /// precisely so it can be checked (and costed) there.
    fn parse_switch(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let scrutinee = self.parse_paren_cond();
        let brace = self.cur_span();
        self.expect(&TokenKind::LBrace);

        let mut cases: Vec<SwitchCase> = Vec::new();
        let mut default: Option<Box<Stmt>> = None;
        loop {
            if self.eat(&TokenKind::RBrace) {
                break;
            }
            if self.at_eof() {
                self.error(30, brace, &[]); // compound statement not closed at EOF
                break;
            }
            match self.peek() {
                TokenKind::Case => {
                    let cstart = self.cur_span();
                    self.bump();
                    if default.is_some() {
                        self.error(15, cstart, &[]); // "default" must be last
                    }
                    let labels = self.parse_case_labels();
                    let body = Box::new(self.parse_stmt_nodecl());
                    cases.push(SwitchCase { labels, body, span: cstart.to(self.prev_span()) });
                }
                TokenKind::Default => {
                    let dstart = self.cur_span();
                    self.bump();
                    self.expect(&TokenKind::Colon);
                    let body = self.parse_stmt_nodecl();
                    if default.is_some() {
                        self.error(16, dstart, &[]); // multiple defaults
                    } else {
                        default = Some(Box::new(body));
                    }
                }
                _ => {
                    // A second statement under a clause lands here, which is
                    // exactly the situation error 2 describes.
                    let span = self.cur_span();
                    self.error(2, span, &[]);
                    self.resync_case();
                }
            }
        }
        Stmt::Switch { scrutinee, cases, default, span: start.to(self.prev_span()) }
    }

    /// The value list of one `case`, up to and including the `:`.
    fn parse_case_labels(&mut self) -> Vec<CaseLabel> {
        let mut labels = Vec::new();
        loop {
            // `case done:` - the scanner glued the name to the colon and produced a
            // `Label`. `doswitch()` clears `sc_allowtags` here for the same reason:
            // in this position the colon ends the case, it is not a tag override.
            if let TokenKind::Label(name) = self.peek() {
                let span = self.cur_span();
                let id = Ident::new(name.clone(), span);
                self.bump();
                labels.push(CaseLabel::Single(Expr { kind: ExprKind::Ident(id), span }));
                return labels;
            }
            let lo = self.parse_expr();
            if self.eat(&TokenKind::DotDot) {
                let lospan = lo.span;
                let hi = self.parse_expr();
                let span = lospan.to(hi.span);
                labels.push(CaseLabel::Range { lo, hi, span });
            } else {
                labels.push(CaseLabel::Single(lo));
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        // `case 1 .. sizeof g:` - the scanner glued `g:` into one Label token, so
        // the colon is already consumed and demanding another would report a
        // spurious error 1.
        if std::mem::take(&mut self.pending_label_colon) {
            return labels;
        }
        self.expect(&TokenKind::Colon);
        labels
    }

    /// Recovery inside a switch body: drop tokens until the next clause or the
    /// closing brace, skipping nested blocks whole so an inner `}` cannot be
    /// mistaken for the switch's own.
    fn resync_case(&mut self) {
        loop {
            match self.peek() {
                TokenKind::Case | TokenKind::Default | TokenKind::RBrace | TokenKind::Eof => {
                    return;
                }
                TokenKind::LBrace => {
                    self.skip_braced();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    // ------------------------------------------------------------ jumps etc.

    /// `dogoto()`.
    fn parse_goto(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let label = self.expect_ident().unwrap_or_else(|| Ident::new("", self.cur_span()));
        self.eat_terminator();
        Stmt::Goto { label, span: start.to(self.prev_span()) }
    }

    /// `doreturn()`. `return;` and `return expr;` differ only here; the error for
    /// mixing them in one function (78) is a semantic check.
    fn parse_return(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let value = self.parse_optional_value();
        Stmt::Return { value, span: start.to(self.prev_span()) }
    }

    /// The shared tail of `return`, `exit` and `sleep`: an optional expression,
    /// then a terminator. `exit` and `sleep` also pass the expression's *tag* to
    /// the host, which the AST captures implicitly by keeping the whole `Expr`.
    fn parse_optional_value(&mut self) -> Option<Expr> {
        if self.at_terminator() {
            self.eat(&TokenKind::Semi);
            return None;
        }
        let e = self.parse_comma_expr();
        self.eat_terminator();
        Some(e)
    }

    /// `doassert()`: a comma-separated list, each element tested independently.
    fn parse_assert(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let mut exprs = Vec::new();
        loop {
            exprs.push(self.parse_expr());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.eat_terminator();
        Stmt::Assert { exprs, span: start.to(self.prev_span()) }
    }

    /// `dostate()`: `state [(cond)] [automaton:]newstate;`.
    fn parse_state(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump();
        let cond = if self.eat(&TokenKind::LParen) {
            let e = self.parse_comma_expr();
            self.expect(&TokenKind::RParen);
            Some(e)
        } else {
            None
        };

        // `automaton:state` reaches the parser as a `Label` followed by an `Ident`,
        // because the scanner glues a name to an adjacent colon.
        let (automaton, state) = match self.peek() {
            TokenKind::Label(name) => {
                let span = self.cur_span();
                let fsa = Ident::new(name.clone(), span);
                self.bump();
                (Some(fsa), self.expect_ident())
            }
            _ => {
                let first = self.expect_ident();
                if self.eat(&TokenKind::Colon) {
                    (first, self.expect_ident())
                } else {
                    (None, first)
                }
            }
        };
        let state = state.unwrap_or_else(|| Ident::new("", self.cur_span()));
        self.eat_terminator();
        let span = start.to(self.prev_span());
        Stmt::State { cond, target: StateRef { automaton, state, span }, span }
    }

    // ----------------------------------------------------------------- emit

    /// `case tpEMIT` in `command()` (`sc2.c`): `#emit opcode [operand]`.
    ///
    /// The original reads the mnemonic straight off the line rather than through
    /// `lex()`, accepting letters and dots, so `const.pri` is one name. Here the
    /// tokens are already scanned, and a dotted mnemonic arrives as several of
    /// them (`const` is even a keyword), so the pieces are glued back together
    /// from the source text they span. Case is insignificant; the original
    /// lowercases, and so does this.
    fn parse_emit(&mut self) -> Stmt {
        let start = self.cur_span();
        self.bump(); // `#emit`

        if self.at_eof() || self.tok().line_start {
            let (span, found) = (self.cur_span(), self.peek().describe());
            self.error(1, span, &["-identifier-", found]);
            return Stmt::Error(start.to(self.prev_span()));
        }

        let opstart = self.cur_span();
        self.bump();
        while self.at(&TokenKind::Dot) && !self.tok().line_start {
            self.bump();
            if self.at_eof() || self.tok().line_start {
                break;
            }
            self.bump();
        }
        let opspan = opstart.to(self.prev_span());
        let name: String = self
            .text(opspan)
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        let opcode = Ident::new(name, opspan);

        let operand = self.parse_emit_operand();
        Stmt::Emit(EmitStmt { opcode, operand, span: start.to(self.prev_span()) })
    }

    fn parse_emit_operand(&mut self) -> Option<EmitOperand> {
        if self.at_eof() || self.tok().line_start || self.at(&TokenKind::Semi) {
            self.eat(&TokenKind::Semi);
            return None;
        }
        let span = self.cur_span();
        // A negative operand: `lex()` returns the sign as part of the number for
        // `#emit`'s purposes, so fold it here.
        let negate = self.at(&TokenKind::Minus) && !self.tok_at(1).line_start;
        if negate {
            self.bump();
        }
        let operand = match self.peek().clone() {
            TokenKind::Int(v) => {
                self.bump();
                Some(EmitOperand::Int {
                    value: if negate { -v } else { v },
                    span: span.to(self.prev_span()),
                })
            }
            TokenKind::Rational(v) => {
                self.bump();
                Some(EmitOperand::Rational {
                    value: if negate { -v } else { v },
                    span: span.to(self.prev_span()),
                })
            }
            TokenKind::Ident(name) if !negate => {
                self.bump();
                Some(EmitOperand::Symbol(Ident::new(name, span)))
            }
            other => {
                self.error(1, span, &["-identifier-", other.describe()]);
                None
            }
        };
        // `check_empty(lptr)`: nothing else may follow on the line.
        while !self.at_eof() && !self.tok().line_start && !self.at(&TokenKind::Semi) {
            self.bump();
        }
        self.eat(&TokenKind::Semi);
        operand
    }

    // -------------------------------------------------------------- helpers

    /// `test(label, TRUE, FALSE)`: a parenthesised condition. A comma list is
    /// accepted (each element is evaluated, the last one decides).
    fn parse_paren_cond(&mut self) -> Expr {
        self.expect(&TokenKind::LParen);
        let e = self.parse_comma_expr();
        self.expect(&TokenKind::RParen);
        e
    }

    /// `doexpr(TRUE, ...)`: one or more expressions separated by commas.
    fn parse_comma_expr(&mut self) -> Expr {
        let first = self.parse_expr();
        if !self.at(&TokenKind::Comma) {
            return first;
        }
        let start = first.span;
        let mut exprs = vec![first];
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr());
        }
        let span = start.to(self.prev_span());
        Expr { kind: ExprKind::Comma { exprs, span }, span }
    }

    /// Recovery after a statement the parser could not make sense of: drop tokens
    /// up to the next `;`, line break or `}`, so one bad statement costs one
    /// diagnostic instead of a cascade.
    fn resync_stmt(&mut self) {
        while !self.at_eof() {
            if self.at(&TokenKind::RBrace) {
                return;
            }
            if self.eat(&TokenKind::Semi) {
                return;
            }
            self.bump();
            if self.tok().line_start {
                return;
            }
        }
    }
}

/// The preprocessor directives the scanner surfaces that may show up where a
/// statement is expected. `#emit` is excluded: it *is* a statement.
fn is_stmt_directive(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::PpAssert
            | TokenKind::PpDefine
            | TokenKind::PpElse
            | TokenKind::PpElseIf
            | TokenKind::PpEndIf
            | TokenKind::PpEndInput
            | TokenKind::PpEndScript
            | TokenKind::PpError
            | TokenKind::PpFile
            | TokenKind::PpIf
            | TokenKind::PpInclude
            | TokenKind::PpLine
            | TokenKind::PpPragma
            | TokenKind::PpTryInclude
            | TokenKind::PpUndef
    )
}

#[cfg(test)]
mod tests {
    use zpc_ast::{Program, decl::Item};
    use zpc_diag::Diagnostics;
    use zpc_lex::Scanner;

    use super::*;

    fn parse_src(src: &str) -> (Program, Diagnostics) {
        let mut lexdiags = Diagnostics::new();
        let toks = Scanner::new(src, "test.sma").scan(&mut lexdiags);
        assert_eq!(lexdiags.error_count(), 0, "the fixture must lex cleanly");
        crate::parse(src, &toks, "test.sma")
    }

    /// Parse `body` as the contents of a function and return its statements.
    fn body_of(src: &str) -> (Vec<Stmt>, Vec<u16>) {
        let full = format!("f()\n{{\n{src}\n}}\n");
        let (p, d) = parse_src(&full);
        let Item::Func(f) = &p.items[0] else { panic!("expected a function: {:?}", p.items) };
        let stmts = f.body.as_ref().expect("the function has a body").stmts.clone();
        (stmts, d.items().iter().map(|i| i.code).collect())
    }

    fn stmts(src: &str) -> Vec<Stmt> {
        let (s, codes) = body_of(src);
        assert!(codes.is_empty(), "unexpected diagnostics: {codes:?}");
        s
    }

    fn codes(src: &str) -> Vec<u16> {
        body_of(src).1
    }

    // ------------------------------------------------------------ dispatch

    #[test]
    fn a_block_nests_and_an_empty_block_is_legal() {
        let s = stmts("{ { } }");
        assert_eq!(s.len(), 1);
        let Stmt::Block(outer) = &s[0] else { panic!("expected a block") };
        assert_eq!(outer.stmts.len(), 1);
        assert!(matches!(outer.stmts[0], Stmt::Block(_)));
    }

    #[test]
    fn a_bare_semicolon_is_the_empty_statement() {
        assert_eq!(codes(";"), vec![36]);
    }

    #[test]
    fn local_declarations_are_statements() {
        let s = stmts("new a = 1;\nstatic b;\nconst C = 3;\nenum E { X, Y }\n");
        assert!(matches!(s[0], Stmt::Var(_)));
        let Stmt::Var(b) = &s[1] else { panic!("expected a variable") };
        assert!(b.modifiers.static_, "`static` survives into the local declaration");
        assert!(matches!(s[2], Stmt::Const(_)));
        assert!(matches!(s[3], Stmt::Enum(_)));
    }

    #[test]
    fn a_declaration_outside_a_compound_block_is_error_3() {
        // The body of an `if` is a statement, not a block, so `new` is error 3.
        assert_eq!(codes("if (a) new x = 1;"), vec![3]);
    }

    // ------------------------------------------------------ braceless bodies

    #[test]
    fn bodies_may_be_braceless() {
        let s = stmts("if (a)\n\tfoo();\nelse\n\tbar();\nwhile (a)\n\tfoo();\nfor (;;)\n\tfoo();");
        let Stmt::If { then_branch, else_branch, .. } = &s[0] else { panic!("expected an if") };
        assert!(matches!(**then_branch, Stmt::Expr { .. }));
        assert!(matches!(**else_branch.as_ref().unwrap(), Stmt::Expr { .. }));
        let Stmt::While { body, .. } = &s[1] else { panic!("expected a while") };
        assert!(matches!(**body, Stmt::Expr { .. }));
        let Stmt::For { body, init, cond, step, .. } = &s[2] else { panic!("expected a for") };
        assert!(init.is_none() && cond.is_none() && step.is_none(), "`for (;;)` is all-empty");
        assert!(matches!(**body, Stmt::Expr { .. }));
    }

    #[test]
    fn else_if_chains_nest_in_the_else_branch() {
        let s = stmts("if (a)\n\tx = 1\nelse if (b)\n\tx = 2\nelse\n\tx = 3");
        let Stmt::If { else_branch, .. } = &s[0] else { panic!("expected an if") };
        let Stmt::If { else_branch: inner, .. } = &**else_branch.as_ref().unwrap() else {
            panic!("expected a nested if")
        };
        assert!(inner.is_some(), "the final else belongs to the inner if");
    }

    // -------------------------------------------------- optional semicolons

    #[test]
    fn a_line_break_terminates_a_statement() {
        // `#pragma semicolon 0` is the default: no semicolon anywhere below.
        let s = stmts("new a = 1\na++\nfoo()\nreturn a");
        assert_eq!(s.len(), 4);
        assert!(matches!(s[3], Stmt::Return { value: Some(_), .. }));
    }

    #[test]
    fn return_without_a_value_stops_at_the_line_break() {
        let s = stmts("return\n");
        assert!(matches!(s[0], Stmt::Return { value: None, .. }));
    }

    // ------------------------------------------------------------- loops

    #[test]
    fn do_while_takes_a_trailing_terminator() {
        let s = stmts("do\n{\n\ti++\n}\nwhile (i < 10)");
        let Stmt::DoWhile { body, .. } = &s[0] else { panic!("expected a do-while") };
        assert!(matches!(**body, Stmt::Block(_)));
    }

    #[test]
    fn for_may_declare_its_loop_variable() {
        let s = stmts("for (new i = 0; i < 32; i++)\n\ttotal += i");
        let Stmt::For { init, cond, step, .. } = &s[0] else { panic!("expected a for") };
        let Some(ForInit::Decl(v)) = init else { panic!("expected a declaration in init") };
        assert_eq!(v.declarators[0].name.name, "i");
        assert!(cond.is_some() && step.is_some());
    }

    #[test]
    fn for_accepts_an_expression_init_and_empty_clauses() {
        let s = stmts("for (i = 0; ; i++)\n\tfoo()");
        let Stmt::For { init, cond, .. } = &s[0] else { panic!("expected a for") };
        assert!(matches!(init, Some(ForInit::Expr(_))));
        assert!(cond.is_none());
    }

    #[test]
    fn break_and_continue_parse_bare() {
        let s = stmts("while (a)\n{\n\tbreak\n}\nwhile (b)\n{\n\tcontinue\n}");
        let Stmt::While { body, .. } = &s[0] else { panic!("expected a while") };
        let Stmt::Block(b) = &**body else { panic!("expected a block") };
        assert!(matches!(b.stmts[0], Stmt::Break { .. }));
    }

    // ------------------------------------------------------------- switch

    // The four `#[ignore]`d tests below need a real expression parser: they are the
    // only ones whose input contains a `case` *value*, and the placeholder
    // `parse_expr` in `lib.rs` runs to the end of the line, swallowing the `:` that
    // ends the case list. Drop the `ignore` once `expr.rs` lands - no change to
    // `stmt.rs` is needed, and working around it here would mean guessing where an
    // expression ends, which is exactly what the expression parser is for.

    #[test]
    #[ignore = "needs expr.rs: the placeholder parse_expr swallows the case-label colon"]
    fn switch_supports_multi_labels_and_ranges() {
        let s = stmts(
            "switch (x)\n{\n\tcase 0: return 0\n\tcase 1, 2, 3: return 1\n\tcase 10 .. 20: return 2\n\tdefault: return -1\n}",
        );
        let Stmt::Switch { cases, default, .. } = &s[0] else { panic!("expected a switch") };
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].labels.len(), 1);
        assert_eq!(cases[1].labels.len(), 3, "`case 1, 2, 3:` is one clause with three labels");
        assert!(matches!(cases[2].labels[0], CaseLabel::Range { .. }), "`10 .. 20` is a range");
        assert!(default.is_some());
    }

    #[test]
    #[ignore = "needs expr.rs: the placeholder parse_expr swallows the case-label colon"]
    fn a_case_after_default_is_error_15() {
        assert_eq!(
            codes("switch (x)\n{\n\tdefault: foo()\n\tcase 1: bar()\n}"),
            vec![15],
            "\"default\" must be the last clause"
        );
    }

    #[test]
    fn a_second_default_is_error_16() {
        assert_eq!(codes("switch (x)\n{\n\tdefault: foo()\n\tdefault: bar()\n}"), vec![16]);
    }

    #[test]
    #[ignore = "needs expr.rs: the placeholder parse_expr swallows the case-label colon"]
    fn a_second_statement_under_a_case_is_error_2() {
        // Pawn's switch takes exactly one statement per clause; the second one is
        // not part of the case, so it is seen where a clause was expected.
        assert_eq!(codes("switch (x)\n{\n\tcase 1: foo()\n\tbar()\n}"), vec![2]);
    }

    #[test]
    #[ignore = "needs expr.rs: the placeholder parse_expr swallows the case-label colon"]
    fn switches_nest() {
        let s = stmts(
            "switch (x)\n{\n\tcase 1:\n\t\tswitch (y)\n\t\t{\n\t\t\tcase 2: foo()\n\t\t\tdefault: bar()\n\t\t}\n\tdefault: baz()\n}",
        );
        let Stmt::Switch { cases, .. } = &s[0] else { panic!("expected a switch") };
        let Stmt::Switch { cases: inner, default: idef, .. } = &*cases[0].body else {
            panic!("expected a nested switch")
        };
        assert_eq!(inner.len(), 1);
        assert!(idef.is_some());
    }

    #[test]
    fn a_case_outside_a_switch_is_error_14() {
        assert_eq!(codes("case 1: foo()"), vec![14]);
    }

    // ------------------------------------------------------ labels and goto

    #[test]
    fn labels_and_goto_round_trip() {
        let s = stmts("new i = 0\nloop:\ni++\nif (i < 10)\n\tgoto loop\nreturn i");
        let Stmt::Label { name, .. } = &s[1] else { panic!("expected a label, got {:?}", s[1]) };
        assert_eq!(name.name, "loop");
        let Stmt::If { then_branch, .. } = &s[3] else { panic!("expected an if") };
        let Stmt::Goto { label, .. } = &**then_branch else { panic!("expected a goto") };
        assert_eq!(label.name, "loop");
    }

    #[test]
    fn a_name_colon_at_statement_position_is_always_a_label() {
        // `Float:` is a tag everywhere else, but `statement()` dispatches tLABEL to
        // `dolabel()` before it ever reaches the expression fallthrough.
        let s = stmts("Float:\nreturn 0");
        let Stmt::Label { name, .. } = &s[0] else { panic!("expected a label") };
        assert_eq!(name.name, "Float");
    }

    // ------------------------------------------- assert / exit / sleep / state

    #[test]
    fn assert_takes_a_comma_list() {
        let s = stmts("assert a, b, c");
        let Stmt::Assert { exprs, .. } = &s[0] else { panic!("expected an assert") };
        assert_eq!(exprs.len(), 3);
    }

    #[test]
    fn exit_and_sleep_take_an_optional_value() {
        let s = stmts("exit\nsleep 5\nexit 1");
        assert!(matches!(s[0], Stmt::Exit { value: None, .. }));
        assert!(matches!(s[1], Stmt::Sleep { value: Some(_), .. }));
        assert!(matches!(s[2], Stmt::Exit { value: Some(_), .. }));
    }

    #[test]
    fn state_transitions_parse_with_and_without_a_condition() {
        let s = stmts("state idle\nstate (x > 1) fsa:next");
        let Stmt::State { cond, target, .. } = &s[0] else { panic!("expected a state") };
        assert!(cond.is_none());
        assert!(target.automaton.is_none());
        assert_eq!(target.state.name, "idle");

        let Stmt::State { cond, target, .. } = &s[1] else { panic!("expected a state") };
        assert!(cond.is_some(), "the conditional form keeps its test");
        assert_eq!(target.automaton.as_ref().unwrap().name, "fsa");
        assert_eq!(target.state.name, "next");
    }

    // --------------------------------------------------------------- emit

    #[test]
    fn emit_parses_opcodes_operands_and_dotted_mnemonics() {
        let s = stmts("#emit const.pri 5\n#emit push.c 0\n#emit stack\n#emit load.pri counter\n");
        let Stmt::Emit(e) = &s[0] else { panic!("expected an emit, got {:?}", s[0]) };
        assert_eq!(e.opcode.name, "const.pri", "a dotted mnemonic is one name");
        assert!(matches!(e.operand, Some(EmitOperand::Int { value: 5, .. })));

        let Stmt::Emit(e) = &s[2] else { panic!("expected an emit") };
        assert_eq!(e.opcode.name, "stack");
        assert!(e.operand.is_none(), "the operand is optional");

        let Stmt::Emit(e) = &s[3] else { panic!("expected an emit") };
        assert!(matches!(&e.operand, Some(EmitOperand::Symbol(id)) if id.name == "counter"));
    }

    // ------------------------------------------------------------ recovery

    #[test]
    fn a_bad_statement_does_not_cascade() {
        let (s, codes) = body_of("case 1: foo()\nreturn 1");
        assert_eq!(codes, vec![14], "exactly one diagnostic for one bad statement");
        assert!(matches!(s[1], Stmt::Return { .. }), "the next statement still parses");
    }

    // ------------------------------------------------------------- fixture

    #[test]
    #[ignore = "needs expr.rs: the placeholder parse_expr swallows the case-label colon"]
    fn the_statement_fixture_parses_without_spurious_diagnostics() {
        let src = include_str!("../../zpc/tests/fixtures/stmt_expr_edge_cases.sma");
        let (program, diags) = parse_src(src);
        let shown: Vec<_> = diags.items().iter().map(|d| (d.code, d.message.clone())).collect();
        assert!(shown.is_empty(), "unexpected diagnostics: {shown:?}");

        let bodies: Vec<_> = program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Func(f) => f.body.as_ref(),
                _ => None,
            })
            .collect();
        assert_eq!(bodies.len(), 4, "control_flow, expressions, labels_and_goto, plugin_init");
        // labels_and_goto: `loop:` is a label and `goto loop` finds it.
        assert!(bodies[2].stmts.iter().any(|s| matches!(s, Stmt::Label { .. })));
    }

    #[test]
    fn an_unclosed_block_is_reported_once() {
        let mut lexdiags = Diagnostics::new();
        let src = "f()\n{\n\tif (a)\n\t{\n\t\tfoo()\n";
        let toks = Scanner::new(src, "test.sma").scan(&mut lexdiags);
        let (_, d) = crate::parse(src, &toks, "test.sma");
        let codes: Vec<u16> = d.items().iter().map(|i| i.code).collect();
        assert!(codes.contains(&30), "compound statement not closed at EOF: {codes:?}");
    }
}
