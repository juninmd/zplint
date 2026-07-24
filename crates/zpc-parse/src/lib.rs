//! Parser producing the zpc AST.
//!
//! Ported from `parse()` and friends in `compiler/libpc300/sc1.c` (Pawn compiler,
//! (c) ITB CompuPhase, zlib-style licence - see ATTRIBUTION.md). Unlike the
//! original, which is a single-pass compiler, this builds a tree first: the token
//! stream comes from [`zpc_lex`] and the result is a [`zpc_ast::Program`] plus the
//! diagnostics gathered on the way.
//!
//! # Structure
//!
//! * `lib.rs` (this file) owns the [`Parser`] struct, the token cursor, the
//!   diagnostic helpers and error recovery. Nothing here parses grammar.
//! * `decl.rs` parses declarations (`parse()`, `declglb()`, `declloc()`,
//!   `decl_enum()`, `decl_const()`, `funcstub()`, `newfunc()`, `declargs()`).
//! * `stmt.rs` and `expr.rs` are written separately; they add their own
//!   `impl<'a> Parser<'a>` blocks and their own `mod` line below.
//!
//! # Contract between the three parser halves
//!
//! Declaration parsing needs expressions (array sizes, initialisers, default
//! values, enum values) and statements (function bodies). It reaches them through
//! exactly three methods, whose signatures are fixed:
//!
//! ```ignore
//! pub(crate) fn parse_expr(&mut self) -> zpc_ast::expr::Expr;
//! pub(crate) fn parse_block(&mut self) -> zpc_ast::stmt::Block;
//! pub(crate) fn parse_stmt(&mut self) -> zpc_ast::stmt::Stmt;
//! ```
//!
//! Placeholder bodies for all three live at the bottom of this file, in the
//! `placeholders` section. The expression and statement authors must **move**
//! their method out of that section into their own file (deleting the
//! placeholder), not add a second definition - two inherent methods with the same
//! name on `Parser` will not compile.
//!
//! The placeholders are deliberately non-panicking and diagnostic-free: they
//! consume a balanced, well-delimited region of tokens and return the
//! corresponding `Error` node, so declaration parsing and its tests work before
//! the other two halves land.

#![forbid(unsafe_code)]

mod decl;
// Added by the statement author:  mod stmt;
// Added by the expression author: mod expr;

use std::path::PathBuf;

use zpc_ast::{
    Ident, Program, Span, TagRef,
    expr::{Expr, ExprKind},
    stmt::{Block, Stmt},
};
use zpc_diag::Diagnostics;
use zpc_lex::{Token, TokenKind};

/// Maximum number of array dimensions (`sDIMEN_MAX` in `sc.h`).
pub(crate) const MAX_DIMENSIONS: usize = 4;

/// Parse a token stream into a [`Program`].
///
/// `src` is the (already preprocessed) source text the tokens were scanned from;
/// it is needed because a couple of constructs - `#pragma` argument tails - are
/// kept verbatim rather than tokenised.
pub fn parse(src: &str, tokens: &[Token], file: impl Into<PathBuf>) -> (Program, Diagnostics) {
    let mut p = Parser::new(src, tokens, file);
    let program = p.parse_program();
    (program, p.diags)
}

/// Recursive-descent parser over a scanned token stream.
///
/// The cursor never runs off the end: [`Parser::tok`] clamps to the trailing
/// [`TokenKind::Eof`] token, so every helper can be called unconditionally.
pub struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Token],
    pos: usize,
    file: PathBuf,
    pub(crate) diags: Diagnostics,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, tokens: &'a [Token], file: impl Into<PathBuf>) -> Self {
        assert!(
            tokens.last().is_some_and(|t| t.kind == TokenKind::Eof),
            "the token stream must be terminated by Eof"
        );
        Self { src, tokens, pos: 0, file: file.into(), diags: Diagnostics::new() }
    }

    /// The whole translation unit. Mirrors `parse()`: only declarations are legal
    /// at file scope, and a broken one is reported and skipped rather than
    /// aborting the file.
    pub fn parse_program(&mut self) -> Program {
        let start = self.cur_span();
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            if let Some(item) = self.parse_item() {
                items.push(item);
            }
            // Every branch of `parse_item` consumes at least one token; this is a
            // belt-and-braces guard so a future edit cannot turn the loop infinite.
            if self.pos == before {
                self.bump();
            }
        }
        let end = self.cur_span();
        Program { items, span: start.to(end) }
    }

    // ------------------------------------------------------------------ cursor

    /// The token under the cursor, clamped to the final `Eof`.
    pub(crate) fn tok(&self) -> &'a Token {
        let last = self.tokens.len() - 1;
        &self.tokens[self.pos.min(last)]
    }

    /// The token `n` positions ahead, clamped to the final `Eof`.
    pub(crate) fn tok_at(&self, n: usize) -> &'a Token {
        let last = self.tokens.len() - 1;
        &self.tokens[(self.pos + n).min(last)]
    }

    pub(crate) fn peek(&self) -> &'a TokenKind {
        &self.tok().kind
    }

    pub(crate) fn peek_at(&self, n: usize) -> &'a TokenKind {
        &self.tok_at(n).kind
    }

    /// Span of the token under the cursor.
    pub(crate) fn cur_span(&self) -> Span {
        self.tok().span
    }

    /// Span of the token just consumed; a zero-width span at the start of input
    /// if nothing has been consumed yet.
    pub(crate) fn prev_span(&self) -> Span {
        if self.pos == 0 {
            Span::at(self.tokens[0].span.start)
        } else {
            self.tokens[(self.pos - 1).min(self.tokens.len() - 1)].span
        }
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Consume and return the current token.
    pub(crate) fn bump(&mut self) -> &'a Token {
        let t = self.tok();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    pub(crate) fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    /// `matchtoken()`: consume the token if it is the expected one.
    pub(crate) fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// `needtoken()`: consume the expected token, or report error 1 and leave the
    /// cursor where it is so the caller can resync.
    pub(crate) fn expect(&mut self, kind: &TokenKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        let (span, found) = (self.cur_span(), self.peek().describe());
        self.error(1, span, &[kind.describe(), found]);
        false
    }

    /// An identifier, if one is under the cursor. `@name` is returned as written -
    /// the `@` prefix is what makes a symbol implicitly `public`, and stripping it
    /// here would lose that.
    pub(crate) fn eat_ident(&mut self) -> Option<Ident> {
        match self.peek() {
            TokenKind::Ident(name) => {
                let span = self.cur_span();
                self.bump();
                Some(Ident::new(name.clone(), span))
            }
            _ => None,
        }
    }

    /// An identifier, reporting error 20 ("invalid symbol name") if there is none.
    pub(crate) fn expect_ident(&mut self) -> Option<Ident> {
        if let Some(id) = self.eat_ident() {
            return Some(id);
        }
        let (span, found) = (self.cur_span(), self.peek().describe());
        self.error(20, span, &[found]);
        None
    }

    /// A `Name:` tag prefix.
    ///
    /// The scanner cannot tell a tag override from a label: `Float:x` and the
    /// statement label `Float:` are the same production ([`TokenKind::Label`]).
    /// Disambiguation is positional, so this helper is only called where a tag is
    /// the only legal reading (before a declarator, a return type, a parameter, an
    /// enum name or an enum member).
    pub(crate) fn eat_tag(&mut self) -> Option<TagRef> {
        match self.peek() {
            TokenKind::Label(name) => {
                let span = self.cur_span();
                self.bump();
                Some(TagRef { name: Ident::new(name.clone(), span), span })
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------- diagnostics

    pub(crate) fn error(&mut self, code: u16, span: Span, args: &[&str]) {
        self.diags.emit(code, span, &self.file, args);
    }

    /// The source text a span covers. Used for the verbatim tail of a `#pragma`.
    pub(crate) fn text(&self, span: Span) -> &'a str {
        let start = (span.start as usize).min(self.src.len());
        let end = (span.end as usize).min(self.src.len());
        // Spans come from the byte-oriented scanner and always land on token
        // boundaries, but clamp defensively rather than risk a panic on a
        // multi-byte character.
        if self.src.is_char_boundary(start) && self.src.is_char_boundary(end) {
            &self.src[start..end]
        } else {
            ""
        }
    }

    // ---------------------------------------------------------- terminators

    /// True where `tTERM` would be accepted: an explicit `;`, end of input, a
    /// closing brace, or a line break (Pawn's semicolons are optional unless
    /// `#pragma semicolon 1` is in force, and the scanner records the line break
    /// as [`Token::line_start`]).
    pub(crate) fn at_terminator(&self) -> bool {
        matches!(self.peek(), TokenKind::Semi | TokenKind::Eof | TokenKind::RBrace)
            || self.tok().line_start
    }

    /// `needtoken(tTERM)`: end the current declaration, reporting error 1 and
    /// dropping the rest of the line if it does not end here.
    pub(crate) fn eat_terminator(&mut self) {
        if self.eat(&TokenKind::Semi) || self.at_terminator() {
            return;
        }
        let (span, found) = (self.cur_span(), self.peek().describe());
        self.error(1, span, &[";", found]);
        self.skip_line();
    }

    // -------------------------------------------------------------- recovery

    /// `lexclr(TRUE)`: drop the rest of the physical line.
    pub(crate) fn skip_line(&mut self) {
        while !self.at_eof() && !self.eat(&TokenKind::Semi) {
            self.bump();
            if self.tok().line_start {
                break;
            }
        }
    }

    /// Resync after a declaration the parser could not make sense of.
    ///
    /// Stops before the next token that can plausibly start a top-level
    /// declaration, so one bad declaration costs one diagnostic rather than a
    /// cascade. Braced regions are skipped whole: a malformed function header
    /// must not leave the body's statements to be parsed as declarations.
    pub(crate) fn resync_decl(&mut self) {
        loop {
            if self.at_eof() {
                return;
            }
            if self.at(&TokenKind::LBrace) {
                self.skip_braced();
                continue;
            }
            if self.eat(&TokenKind::Semi) {
                return;
            }
            self.bump();
            if self.tok().line_start && self.starts_declaration() {
                return;
            }
        }
    }

    /// True if the current token can begin a top-level declaration.
    fn starts_declaration(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::New
                | TokenKind::Static
                | TokenKind::Stock
                | TokenKind::Public
                | TokenKind::Const
                | TokenKind::Enum
                | TokenKind::Native
                | TokenKind::Forward
                | TokenKind::Operator
                | TokenKind::Eof
        )
    }

    /// Consume a `{ ... }` region, brace-balanced. The cursor must be on the `{`.
    pub(crate) fn skip_braced(&mut self) -> Span {
        let start = self.cur_span();
        if !self.at(&TokenKind::LBrace) {
            return start;
        }
        let mut depth = 0usize;
        loop {
            match self.peek() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                TokenKind::Eof => {
                    self.error(30, start, &[]); // compound statement not closed at EOF
                    return start.to(self.cur_span());
                }
                _ => {}
            }
            self.bump();
            if depth == 0 {
                return start.to(self.prev_span());
            }
        }
    }

    // ====================================================================
    // placeholders - see the module docs. Move these into `expr.rs`/`stmt.rs`
    // rather than redefining them there.
    // ====================================================================

    /// Parse one expression.
    ///
    /// PLACEHOLDER (expression author: replace). It consumes a balanced token
    /// region up to the first top-level `,`, `;`, closing bracket, or line break,
    /// and yields [`ExprKind::Error`] without emitting a diagnostic - so callers
    /// stay in sync with the token stream and no spurious errors are reported.
    pub(crate) fn parse_expr(&mut self) -> Expr {
        let start = self.cur_span();
        let mut depth = 0usize;
        let mut consumed = false;
        loop {
            // Pawn's optional semicolons make a line break an expression
            // terminator, but only outside brackets.
            if consumed && depth == 0 && self.tok().line_start {
                break;
            }
            match self.peek() {
                TokenKind::Eof => break,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Comma | TokenKind::Semi | TokenKind::Ellipsis if depth == 0 => break,
                _ => {}
            }
            self.bump();
            consumed = true;
        }
        let span =
            if consumed { start.to(self.prev_span()) } else { Span::at(start.start) };
        Expr { kind: ExprKind::Error(span), span }
    }

    /// Parse a `{ ... }` compound statement.
    ///
    /// PLACEHOLDER (statement author: replace). It skips the block
    /// brace-balanced and returns an empty [`Block`], which is enough for
    /// declaration parsing to walk past a function body.
    pub(crate) fn parse_block(&mut self) -> Block {
        let span = self.skip_braced();
        Block { stmts: Vec::new(), span }
    }

    /// Parse one statement.
    ///
    /// PLACEHOLDER (statement author: replace). It drops tokens up to the end of
    /// the statement and returns [`Stmt::Error`].
    pub(crate) fn parse_stmt(&mut self) -> Stmt {
        if self.at(&TokenKind::LBrace) {
            return Stmt::Block(self.parse_block());
        }
        let start = self.cur_span();
        self.skip_line();
        Stmt::Error(start.to(self.prev_span()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpc_lex::Scanner;

    fn lex(src: &str) -> (Vec<Token>, Diagnostics) {
        let mut d = Diagnostics::new();
        let toks = Scanner::new(src, "test.sma").scan(&mut d);
        (toks, d)
    }

    #[test]
    fn cursor_clamps_at_end_of_input() {
        let (toks, _) = lex("new a");
        let mut p = Parser::new("new a", &toks, "test.sma");
        for _ in 0..10 {
            p.bump();
        }
        assert!(p.at_eof());
        assert!(p.at_eof(), "bumping past Eof must stay at Eof");
    }

    #[test]
    fn expect_reports_error_1_with_both_spellings() {
        let (toks, _) = lex("new");
        let mut p = Parser::new("new", &toks, "test.sma");
        assert!(!p.expect(&TokenKind::Semi));
        let d = &p.diags.items()[0];
        assert_eq!(d.code, 1);
        assert_eq!(d.message, "expected token: \";\", but found \"new\"");
    }

    #[test]
    fn a_line_break_terminates_a_declaration() {
        let src = "new a\nnew b";
        let (toks, _) = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        p.bump(); // new
        p.bump(); // a
        p.eat_terminator();
        assert_eq!(p.diags.items().len(), 0, "an optional semicolon is not an error");
        assert!(matches!(p.peek(), TokenKind::New));
    }

    #[test]
    fn tags_and_labels_share_one_token_production() {
        let src = "Float:x";
        let (toks, _) = lex(src);
        let mut p = Parser::new(src, &toks, "test.sma");
        let tag = p.eat_tag().expect("Float: lexes as a label token");
        assert_eq!(tag.name.name, "Float");
        assert!(!tag.is_untagged());
        assert_eq!(p.eat_ident().unwrap().name, "x");
    }
}
