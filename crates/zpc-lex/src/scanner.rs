//! Scanner: source text -> [`Token`] stream.
//!
//! Ported from `lex()`, `litchar()`, `number()` (`btoi`/`htoi`/`dtoi`/`ftoi`),
//! `alpha()`, `ishex()` and `stripcom()` in `compiler/libpc300/sc2.c`.
//!
//! The original compiler is line-buffered: `readline()` fills a NUL-terminated line,
//! `stripcom()` blanks comments out of it, then `lex()` walks it. zpc scans the whole
//! buffer in one pass instead, but every accept/reject decision below is taken from
//! the C code so the token stream matches amxxpc's. Where the two must differ (the C
//! code relies on a NUL sentinel that a Rust slice does not have) the divergence is
//! called out in a comment.

use std::path::{Path, PathBuf};

use zpc_diag::{Diagnostics, Span};

use crate::token::{OPERATORS, Token, TokenKind};

/// `sc_ctrlchar`'s initial value. Pawn escapes with `^`, not `\` - `"^n"` is a
/// newline and `"\n"` is a literal backslash followed by `n`. `#pragma ctrlchar`
/// changes it at any point in the file, hence [`Scanner::set_ctrl_char`].
pub const DEFAULT_CTRL_CHAR: u8 = b'^';

/// `sNAMEMAX` (amx.h): identifiers longer than this are truncated with a warning.
const NAME_MAX: usize = 63;

/// `PUBLIC_CHAR` (sc.h): `@` is an identifier character in Pawn.
const PUBLIC_CHAR: u8 = b'@';

/// Turns Pawn source into tokens.
///
/// Byte-oriented on purpose: the reference lexer works on bytes, source files are
/// effectively Latin-1/UTF-8 blobs, and [`Span`] holds byte offsets. Non-ASCII bytes
/// can therefore appear only inside literals and comments, which are copied through.
pub struct Scanner<'a> {
    src: &'a [u8],
    pos: usize,
    file: PathBuf,
    ctrl_char: u8,
    /// Whether the next token produced is the first one on its physical line.
    /// Mirrors `newline = (lptr==pline)` in `lex()`.
    at_line_start: bool,
}

impl<'a> Scanner<'a> {
    pub fn new(src: &'a str, file: impl Into<PathBuf>) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            file: file.into(),
            ctrl_char: DEFAULT_CTRL_CHAR,
            at_line_start: true,
        }
    }

    /// Change the escape character, as `#pragma ctrlchar` does. The preprocessor
    /// calls this mid-scan, which is why it is not a constructor argument.
    pub fn set_ctrl_char(&mut self, c: u8) {
        self.ctrl_char = c;
    }

    pub fn ctrl_char(&self) -> u8 {
        self.ctrl_char
    }

    /// Scan the whole buffer. The returned vector always ends with [`TokenKind::Eof`].
    pub fn scan(mut self, diags: &mut Diagnostics) -> Vec<Token> {
        let mut out = Vec::new();
        loop {
            let tok = self.next_token(diags);
            let done = tok.kind == TokenKind::Eof;
            out.push(tok);
            if done {
                return out;
            }
        }
    }

    // ---------------------------------------------------------------- helpers

    /// Byte at `p`, or `0` past the end. The C lexer reads NUL-terminated buffers
    /// and its conditions lean on that sentinel, so reproducing it keeps the ported
    /// predicates (`*ptr=='\0'`, `alphanum(*ptr)`, ...) literally correct.
    fn at(&self, p: usize) -> u8 {
        self.src.get(p).copied().unwrap_or(0)
    }

    fn file(&self) -> &Path {
        &self.file
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(start as u32, end as u32)
    }

    fn text(&self, start: usize, end: usize) -> String {
        String::from_utf8_lossy(&self.src[start..end.min(self.src.len())]).into_owned()
    }

    /// `alpha()`: a letter, `_` or `@`.
    fn is_alpha(c: u8) -> bool {
        c.is_ascii_alphabetic() || c == b'_' || c == PUBLIC_CHAR
    }

    /// `alphanum()`.
    fn is_alnum(c: u8) -> bool {
        Self::is_alpha(c) || c.is_ascii_digit()
    }

    /// `ishex()`: note it accepts both cases, unlike the `0x` prefix test.
    fn is_hex(c: u8) -> bool {
        c.is_ascii_hexdigit()
    }

    /// If a line continuation starts at `p`, the offset just past it.
    ///
    /// `readline()` joins a line whose last non-blank character is `\` with the next
    /// one, and strips the continued line's leading whitespace. The continuation
    /// marker is a backslash even though the escape character is `^`: the two are
    /// unrelated mechanisms and `readline()` hardcodes `'\\'`.
    fn line_continuation(&self, p: usize) -> Option<usize> {
        if self.at(p) != b'\\' {
            return None;
        }
        let mut q = p + 1;
        // readline() walks back over trailing whitespace before testing for the `\`,
        // so blanks between the backslash and the line break are allowed.
        while matches!(self.at(q), b' ' | b'\t' | b'\r') {
            q += 1;
        }
        if self.at(q) != b'\n' {
            return None;
        }
        q += 1;
        while matches!(self.at(q), b' ' | b'\t' | b'\r') {
            q += 1;
        }
        Some(q.min(self.src.len()))
    }

    // ---------------------------------------------------------------- trivia

    /// Consume whitespace, line continuations and comments, recording whether a
    /// physical line break was crossed.
    fn skip_trivia(&mut self, diags: &mut Diagnostics) {
        loop {
            if let Some(q) = self.line_continuation(self.pos) {
                self.pos = q;
                continue;
            }
            let c = self.at(self.pos);
            if self.pos >= self.src.len() {
                return;
            }
            if c == b'\n' {
                self.at_line_start = true;
                self.pos += 1;
                continue;
            }
            if c <= b' ' {
                self.pos += 1;
                continue;
            }
            // `stripcom()` tests `/*` before `//`, so `//*` can never open a block
            // comment - it is a line comment whose first body character is `*`.
            if c == b'/' && self.at(self.pos + 1) == b'*' {
                self.block_comment(diags);
                continue;
            }
            if c == b'/' && self.at(self.pos + 1) == b'/' {
                self.line_comment(diags);
                continue;
            }
            return;
        }
    }

    /// `/* ... */`. Pawn block comments do **not** nest: `stripcom()` reports error
    /// 216 on an inner `/*` and keeps looking for the first `*/`.
    fn block_comment(&mut self, diags: &mut Diagnostics) {
        let start = self.pos;
        self.pos += 2;
        loop {
            if self.pos >= self.src.len() {
                // readline() reports this as `error(1,"*/","-end of file-")`.
                diags.emit(
                    1,
                    self.span(start, self.src.len()),
                    self.file(),
                    &["*/", "-end of file-"],
                );
                return;
            }
            let c = self.at(self.pos);
            if c == b'/' && self.at(self.pos + 1) == b'*' {
                diags.emit(216, self.span(self.pos, self.pos + 2), self.file(), &[]);
                self.pos += 2;
                continue;
            }
            if c == b'*' && self.at(self.pos + 1) == b'/' {
                self.pos += 2;
                return;
            }
            if c == b'\n' {
                self.at_line_start = true;
            }
            self.pos += 1;
        }
    }

    /// `// ...` to end of line. The line break itself is left for [`Self::skip_trivia`].
    fn line_comment(&mut self, diags: &mut Diagnostics) {
        let start = self.pos;
        let mut reported = false;
        loop {
            // A continued line comment silently swallows the next line, which is
            // almost never intended - `stripcom()` rejects it with error 49.
            if let Some(q) = self.line_continuation(self.pos) {
                if !reported {
                    diags.emit(49, self.span(start, self.pos + 1), self.file(), &[]);
                    reported = true;
                }
                self.pos = q;
                continue;
            }
            if self.pos >= self.src.len() || self.at(self.pos) == b'\n' {
                return;
            }
            self.pos += 1;
        }
    }

    // ---------------------------------------------------------------- numbers

    /// `btoi()`: `0b` followed by binary digits and `_` separators.
    /// Only a lowercase `b` is accepted - the C code tests `*(ptr+1)=='b'`.
    fn scan_binary(&self, p: usize) -> Option<(i64, usize)> {
        if self.at(p) != b'0' || self.at(p + 1) != b'b' {
            return None;
        }
        let mut q = p + 2;
        let mut v: i64 = 0;
        while matches!(self.at(q), b'0' | b'1' | b'_') {
            if self.at(q) != b'_' {
                v = v.wrapping_shl(1) | i64::from(self.at(q) - b'0');
            }
            q += 1;
        }
        // "number must be delimited by non-alphanumeric char"
        if Self::is_alnum(self.at(q)) { None } else { Some((v, q)) }
    }

    /// `htoi()`: `0x` followed by hex digits and `_` separators.
    ///
    /// Two faithfully-reproduced quirks: only a lowercase `x` is recognised (`0X10`
    /// is not a hex literal), and `0x` with no digits at all is accepted as `0`
    /// because the digit loop may run zero times.
    fn scan_hex(&self, p: usize) -> Option<(i64, usize)> {
        if !self.at(p).is_ascii_digit() {
            return None;
        }
        if self.at(p) != b'0' || self.at(p + 1) != b'x' {
            return None;
        }
        let mut q = p + 2;
        let mut v: i64 = 0;
        while Self::is_hex(self.at(q)) || self.at(q) == b'_' {
            let c = self.at(q);
            if c != b'_' {
                let d = if c.is_ascii_digit() {
                    c - b'0'
                } else {
                    c.to_ascii_lowercase() - b'a' + 10
                };
                v = v.wrapping_shl(4).wrapping_add(i64::from(d));
            }
            q += 1;
        }
        if Self::is_alnum(self.at(q)) { None } else { Some((v, q)) }
    }

    /// `dtoi()`: decimal digits with `_` separators, rejected when a fractional part
    /// follows so that `1.5` falls through to `ftoi()`.
    fn scan_decimal(&self, p: usize) -> Option<(i64, usize)> {
        if !self.at(p).is_ascii_digit() {
            return None;
        }
        let mut q = p;
        let mut v: i64 = 0;
        while self.at(q).is_ascii_digit() || self.at(q) == b'_' {
            if self.at(q) != b'_' {
                v = v.wrapping_mul(10).wrapping_add(i64::from(self.at(q) - b'0'));
            }
            q += 1;
        }
        if Self::is_alnum(self.at(q)) {
            return None;
        }
        if self.at(q) == b'.' && self.at(q + 1).is_ascii_digit() {
            return None;
        }
        Some((v, q))
    }

    /// `number()`: binary, then hexadecimal, then decimal.
    fn scan_number(&self, p: usize) -> Option<(i64, usize)> {
        self.scan_binary(p)
            .or_else(|| self.scan_hex(p))
            .or_else(|| self.scan_decimal(p))
    }

    /// `ftoi()`. Pawn's rational syntax is stricter than C's: a digit is required on
    /// both sides of the period (`.5` and `6.` are not rationals), the period is
    /// mandatory even with an exponent (`2e3` is not a rational), and only a
    /// lowercase `e` introduces the exponent.
    ///
    /// The value is kept as `f64`. amxxpc stores it as an IEEE-754 `f32` bit pattern
    /// in a 32-bit cell, but that narrowing belongs to code generation - doing it
    /// here would lose information the parser may want for diagnostics.
    fn scan_rational(&self, p: usize) -> Option<(f64, usize)> {
        if !self.at(p).is_ascii_digit() {
            return None;
        }
        let mut q = p;
        let mut num = 0.0f64;
        while self.at(q).is_ascii_digit() || self.at(q) == b'_' {
            if self.at(q) != b'_' {
                num = num * 10.0 + f64::from(self.at(q) - b'0');
            }
            q += 1;
        }
        if self.at(q) != b'.' {
            return None;
        }
        q += 1;
        if !self.at(q).is_ascii_digit() {
            return None;
        }
        let mut frac = 0.0f64;
        let mut mult = 1.0f64;
        while self.at(q).is_ascii_digit() || self.at(q) == b'_' {
            if self.at(q) != b'_' {
                frac = frac * 10.0 + f64::from(self.at(q) - b'0');
                mult /= 10.0;
            }
            q += 1;
        }
        num += frac * mult;
        if self.at(q) == b'e' {
            let mut r = q + 1;
            let sign = if self.at(r) == b'-' {
                r += 1;
                -1.0
            } else {
                1.0
            };
            // "'e' should be followed by a digit" - otherwise the whole literal is
            // rejected, not just the exponent.
            if !self.at(r).is_ascii_digit() {
                return None;
            }
            let mut exp = 0i32;
            while self.at(r).is_ascii_digit() {
                exp = exp.saturating_mul(10).saturating_add(i32::from(self.at(r) - b'0'));
                r += 1;
            }
            num *= 10f64.powf(sign * f64::from(exp));
            q = r;
        }
        Some((num, q))
    }

    // ---------------------------------------------------------------- literals

    /// `litchar()`: read one (possibly escaped) character, advancing `self.pos`.
    ///
    /// In `RAWMODE` (a `^"..."` string) no escape is interpreted at all.
    fn litchar(&mut self, raw: bool, diags: &mut Diagnostics) -> i64 {
        let c = self.at(self.pos);
        if raw || c != self.ctrl_char {
            self.pos += 1;
            return i64::from(c);
        }
        let start = self.pos;
        self.pos += 1;
        let e = self.at(self.pos);
        if e == self.ctrl_char {
            self.pos += 1;
            return i64::from(e);
        }
        match e {
            b'a' => {
                self.pos += 1;
                7
            }
            b'b' => {
                self.pos += 1;
                8
            }
            b'e' => {
                self.pos += 1;
                27
            }
            b'f' => {
                self.pos += 1;
                12
            }
            b'n' => {
                self.pos += 1;
                10
            }
            b'r' => {
                self.pos += 1;
                13
            }
            b't' => {
                self.pos += 1;
                9
            }
            b'v' => {
                self.pos += 1;
                11
            }
            b'x' => {
                self.pos += 1;
                let mut v: i64 = 0;
                while Self::is_hex(self.at(self.pos)) {
                    let d = self.at(self.pos);
                    let d = if d.is_ascii_digit() {
                        d - b'0'
                    } else {
                        d.to_ascii_lowercase() - b'a' + 10
                    };
                    v = v.wrapping_shl(4).wrapping_add(i64::from(d));
                    self.pos += 1;
                }
                // The `;` terminator is optional; it exists so `^x41;7` is unambiguous.
                if self.at(self.pos) == b';' {
                    self.pos += 1;
                }
                v
            }
            b'\'' | b'"' | b'%' => {
                self.pos += 1;
                i64::from(e)
            }
            d if d.is_ascii_digit() => {
                // `^ddd` is DECIMAL in Pawn, not octal as in C.
                let mut v: i64 = 0;
                while self.at(self.pos).is_ascii_digit() {
                    v = v
                        .wrapping_mul(10)
                        .wrapping_add(i64::from(self.at(self.pos) - b'0'));
                    self.pos += 1;
                }
                if self.at(self.pos) == b';' {
                    self.pos += 1;
                }
                v
            }
            _ => {
                diags.emit(27, self.span(start, self.pos + 1), self.file(), &[]);
                // `litchar()` leaves the pointer on the offending character; it can
                // afford to because escapes are resolved in a second pass over a
                // NUL-terminated copy. Here the caller loops on `self.pos`, so we
                // must consume the byte or spin forever.
                if self.pos < self.src.len() {
                    self.pos += 1;
                }
                0
            }
        }
    }

    /// Cells are numbers, but [`TokenKind::Str`] holds text. Values outside the
    /// Unicode scalar range (reachable via `^xFFFFFFFF;`) become U+FFFD; the exact
    /// cell values are recovered by code generation from the source span.
    fn push_cell(out: &mut String, c: i64) {
        out.push(u32::try_from(c).ok().and_then(char::from_u32).unwrap_or('\u{fffd}'));
    }

    /// Recognise a string opener and report `(content_start, packed, raw)`.
    /// The five accepted spellings come straight from the `lex()` condition:
    /// `"`, `^"`, `!"`, `!^"` and `^!"`.
    fn string_opener(&self, p: usize) -> Option<(usize, bool, bool)> {
        let ctrl = self.ctrl_char;
        let (c0, c1, c2) = (self.at(p), self.at(p + 1), self.at(p + 2));
        if c0 == b'"' {
            Some((p + 1, false, false))
        } else if c0 == ctrl && c1 == b'"' {
            Some((p + 2, false, true))
        } else if c0 == b'!' && c1 == b'"' {
            Some((p + 2, true, false))
        } else if (c0 == b'!' && c1 == ctrl && c2 == b'"')
            || (c0 == ctrl && c1 == b'!' && c2 == b'"')
        {
            // `!^"..."` and `^!"..."` are the same thing: packed and raw.
            Some((p + 3, true, true))
        } else {
            None
        }
    }

    fn scan_string(&mut self, diags: &mut Diagnostics) -> TokenKind {
        let start = self.pos;
        let (content, packed, raw) = self
            .string_opener(self.pos)
            .expect("caller checked string_opener");
        self.pos = content;
        let mut out = String::new();
        loop {
            // A `\` at end of line splices the literal across lines; `readline()`
            // has already removed the marker, the newline and the next line's indent.
            if let Some(q) = self.line_continuation(self.pos) {
                self.pos = q;
                continue;
            }
            let c = self.at(self.pos);
            if self.pos >= self.src.len() || c == b'\n' {
                // amxxpc's line buffer ends at the newline, so an unterminated
                // string is caught at end of line with error 37.
                diags.emit(37, self.span(start, self.pos), self.file(), &[]);
                break;
            }
            if c == b'"' {
                self.pos += 1;
                break;
            }
            if raw && c == self.ctrl_char && self.at(self.pos + 1) != 0 {
                // Even in raw mode `lex()` copies the escape character together with
                // the character behind it, so `^"` inside a raw string is not a
                // terminator - but neither is it translated.
                out.push(char::from(c));
                out.push(char::from(self.at(self.pos + 1)));
                self.pos += 2;
                continue;
            }
            let cell = self.litchar(raw, diags);
            Self::push_cell(&mut out, cell);
        }
        if packed {
            TokenKind::PackedStr(out)
        } else {
            TokenKind::Str(out)
        }
    }

    /// `'c'`. A character literal is an integer token: `_lextok=tNUMBER`.
    fn scan_char(&mut self, diags: &mut Diagnostics) -> TokenKind {
        let start = self.pos;
        self.pos += 1; // opening quote
        let v = self.litchar(false, diags);
        if self.at(self.pos) == b'\'' {
            self.pos += 1;
        } else {
            // "invalid character constant (must be one character)"
            diags.emit(27, self.span(start, self.pos), self.file(), &[]);
        }
        TokenKind::Int(v)
    }

    // ---------------------------------------------------------------- driver

    fn next_token(&mut self, diags: &mut Diagnostics) -> Token {
        self.skip_trivia(diags);
        let line_start = self.at_line_start;
        self.at_line_start = false;
        let start = self.pos;

        if self.pos >= self.src.len() {
            return Token {
                kind: TokenKind::Eof,
                span: self.span(start, start),
                line_start,
            };
        }

        let kind = self.lex_one(diags);
        Token {
            kind,
            span: self.span(start, self.pos),
            line_start,
        }
    }

    fn lex_one(&mut self, diags: &mut Diagnostics) -> TokenKind {
        let start = self.pos;
        let c = self.at(start);

        // `lex()` matches the multi-character operator table before anything else.
        // The table is ordered longest-first, so the first hit is the greedy one and
        // `>>>=` cannot be mis-lexed as `>>` `>=`.
        let rest = &self.src[start..];
        for (text, kind) in OPERATORS {
            if rest.starts_with(text.as_bytes()) {
                self.pos += text.len();
                return kind.clone();
            }
        }

        // Numbers before identifiers, and integers before rationals: `dtoi()`
        // deliberately rejects `1.5` so that `ftoi()` gets a chance at it.
        if let Some((v, end)) = self.scan_number(start) {
            self.pos = end;
            return TokenKind::Int(v);
        }
        if let Some((v, end)) = self.scan_rational(start) {
            self.pos = end;
            return TokenKind::Rational(v);
        }

        if Self::is_alpha(c) {
            return self.lex_word(diags);
        }

        if self.string_opener(start).is_some() {
            return self.scan_string(diags);
        }

        if c == b'\'' {
            return self.scan_char(diags);
        }

        if c == b'#' {
            return self.lex_directive(diags);
        }

        if let Some(kind) = single_char(c) {
            self.pos += 1;
            return kind;
        }

        // `lex()` returns the raw character as the token here and lets the parser
        // complain. zpc has no token kind for an arbitrary byte, so it reports
        // "invalid expression, assumed zero" and resynchronises on the next byte.
        self.pos += 1;
        diags.emit(29, self.span(start, self.pos), self.file(), &[]);
        TokenKind::EndExpr
    }

    fn lex_word(&mut self, diags: &mut Diagnostics) -> TokenKind {
        let start = self.pos;
        while Self::is_alnum(self.at(self.pos)) {
            self.pos += 1;
        }
        let mut word = self.text(start, self.pos);
        if word.len() > NAME_MAX {
            word.truncate(NAME_MAX);
            diags.emit(
                200,
                self.span(start, self.pos),
                self.file(),
                &[&word, "63"],
            );
        }

        // Reserved words are matched before the symbol branch in `lex()`, so a
        // keyword is never turned into a label: `case:` is `case` then `:`.
        if let Some(kw) = TokenKind::keyword(&word) {
            return kw;
        }

        // `name:` is a label - or a tag override, which is spelled identically.
        // `lex()` cannot tell them apart either; it emits tLABEL and lets the parser
        // decide from context. `::` is excluded so `a::b` stays a scope operator,
        // and a bare `@` is excluded because it is an operator, not a symbol.
        if word != "@" && self.at(self.pos) == b':' && self.at(self.pos + 1) != b':' {
            self.pos += 1;
            return TokenKind::Label(word);
        }

        TokenKind::Ident(word)
    }

    /// `#word`. A lone `@` or `_` is special-cased in `lex()` as an operator and a
    /// placeholder respectively; zpc has no token kind for either, so they stay
    /// [`TokenKind::Ident`] and the parser handles them.
    fn lex_directive(&mut self, diags: &mut Diagnostics) -> TokenKind {
        let start = self.pos;
        self.pos += 1;
        let word_start = self.pos;
        while Self::is_alnum(self.at(self.pos)) {
            self.pos += 1;
        }
        let word = self.text(word_start, self.pos);
        match TokenKind::directive(&word) {
            Some(kind) => kind,
            None => {
                diags.emit(31, self.span(start, self.pos), self.file(), &[]);
                TokenKind::EndExpr
            }
        }
    }
}

/// The single-character tokens `lex()` falls through to.
fn single_char(c: u8) -> Option<TokenKind> {
    use TokenKind::*;
    Some(match c {
        b'+' => Plus,
        b'-' => Minus,
        b'*' => Star,
        b'/' => Slash,
        b'%' => Percent,
        b'=' => Assign,
        b'<' => Lt,
        b'>' => Gt,
        b'!' => Not,
        b'~' => Tilde,
        b'&' => Amp,
        b'|' => Pipe,
        b'^' => Caret,
        b'?' => Question,
        b':' => Colon,
        b';' => Semi,
        b',' => Comma,
        b'.' => Dot,
        b'(' => LParen,
        b')' => RParen,
        b'{' => LBrace,
        b'}' => RBrace,
        b'[' => LBracket,
        b']' => RBracket,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> (Vec<TokenKind>, Vec<u16>) {
        let mut diags = Diagnostics::new();
        let toks = Scanner::new(src, "t.sma").scan(&mut diags);
        let codes = diags.items().iter().map(|d| d.code).collect();
        (toks.into_iter().map(|t| t.kind).collect(), codes)
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (k, codes) = lex(src);
        assert!(codes.is_empty(), "unexpected diagnostics {codes:?} for {src:?}");
        k
    }

    fn codes(src: &str) -> Vec<u16> {
        lex(src).1
    }

    fn tokens(src: &str) -> Vec<Token> {
        let mut diags = Diagnostics::new();
        Scanner::new(src, "t.sma").scan(&mut diags)
    }

    // ---------------------------------------------------------- identifiers

    #[test]
    fn identifiers_allow_underscore_and_at() {
        use TokenKind::*;
        assert_eq!(
            kinds("_foo @bar baz_1 @"),
            vec![
                Ident("_foo".into()),
                Ident("@bar".into()),
                Ident("baz_1".into()),
                Ident("@".into()),
                Eof
            ]
        );
    }

    #[test]
    fn keywords_beat_identifiers_but_only_exact_spellings() {
        use TokenKind::*;
        assert_eq!(kinds("public"), vec![Public, Eof]);
        assert_eq!(kinds("publicx"), vec![Ident("publicx".into()), Eof]);
        // Pawn is case sensitive and `Float` is a tag from float.inc, not a keyword.
        assert_eq!(kinds("Float"), vec![Ident("Float".into()), Eof]);
    }

    #[test]
    fn overlong_identifiers_are_truncated_with_warning_200() {
        let long = "a".repeat(70);
        let (k, codes) = lex(&long);
        assert_eq!(codes, vec![200]);
        assert_eq!(k[0], TokenKind::Ident("a".repeat(63)));
    }

    // ---------------------------------------------------------------- labels

    #[test]
    fn label_requires_the_colon_to_be_adjacent_and_not_a_scope_operator() {
        use TokenKind::*;
        assert_eq!(kinds("done:"), vec![Label("done".into()), Eof]);
        // a space breaks it: this is the ternary/tag-separator shape
        assert_eq!(kinds("a : b"), vec![Ident("a".into()), Colon, Ident("b".into()), Eof]);
        assert_eq!(kinds("a::b"), vec![Ident("a".into()), ColonColon, Ident("b".into()), Eof]);
        // keywords never become labels
        assert_eq!(kinds("case:"), vec![Case, Colon, Eof]);
    }

    // --------------------------------------------------------------- numbers

    #[test]
    fn decimal_hex_and_binary_integers() {
        use TokenKind::*;
        assert_eq!(kinds("0 42 0x1f 0xFF 0b1011"), vec![
            Int(0),
            Int(42),
            Int(31),
            Int(255),
            Int(11),
            Eof
        ]);
    }

    #[test]
    fn underscores_separate_digits_in_every_base() {
        use TokenKind::*;
        assert_eq!(kinds("1_000 0xDE_AD 0b1010_1010"), vec![
            Int(1000),
            Int(0xDEAD),
            Int(0b1010_1010),
            Eof
        ]);
    }

    #[test]
    fn hex_prefix_must_be_lowercase_x() {
        // htoi() tests `*(ptr+1)=='x'`, so `0X10` is not a number at all; it falls
        // through to the "unrecognised character" path.
        assert_eq!(codes("0X10"), vec![29]);
    }

    #[test]
    fn hex_with_no_digits_is_accepted_as_zero() {
        // The digit loop in htoi() may run zero times and the result is still
        // returned, so `0x` followed by a delimiter lexes as 0.
        assert_eq!(kinds("0x;"), vec![TokenKind::Int(0), TokenKind::Semi, TokenKind::Eof]);
        // ...but `0xZ` is alphanumeric-delimited, so htoi(), dtoi() and ftoi() all
        // reject it and the leading digit is reported as an invalid expression.
        assert_eq!(codes("0xZ"), vec![29]);
    }

    #[test]
    fn rationals_need_a_digit_on_both_sides_of_the_period() {
        use TokenKind::*;
        assert_eq!(kinds("1.5"), vec![Rational(1.5), Eof]);
        // `1.` is an integer followed by a member-access dot, not a rational
        assert_eq!(kinds("1."), vec![Int(1), Dot, Eof]);
        // `.5` has no leading digit
        assert_eq!(kinds(".5"), vec![Dot, Int(5), Eof]);
    }

    #[test]
    fn rational_exponent_requires_the_period() {
        use TokenKind::*;
        assert_eq!(kinds("2.0e3"), vec![Rational(2000.0), Eof]);
        assert_eq!(kinds("2.5e-2"), vec![Rational(0.025), Eof]);
        // `2e3` has no period: dtoi() stops at `e`, which is alphanumeric, so the
        // whole thing is rejected.
        assert_eq!(codes("2e3"), vec![29]);
    }

    #[test]
    fn range_operator_does_not_swallow_the_integers() {
        use TokenKind::*;
        assert_eq!(kinds("1..5"), vec![Int(1), DotDot, Int(5), Eof]);
    }

    // --------------------------------------------------------------- strings

    #[test]
    fn escape_character_is_caret_not_backslash() {
        use TokenKind::*;
        assert_eq!(kinds("\"a^nb\""), vec![Str("a\nb".into()), Eof]);
        assert_eq!(kinds("\"a^tb\""), vec![Str("a\tb".into()), Eof]);
        assert_eq!(kinds("\"^^\""), vec![Str("^".into()), Eof]);
        assert_eq!(kinds("\"^\"\""), vec![Str("\"".into()), Eof]);
        // a backslash is an ordinary character in a Pawn string
        assert_eq!(kinds("\"a\\nb\""), vec![Str("a\\nb".into()), Eof]);
    }

    #[test]
    fn hex_and_decimal_escapes_with_optional_semicolon() {
        use TokenKind::*;
        assert_eq!(kinds("\"^x41;B\""), vec![Str("AB".into()), Eof]);
        assert_eq!(kinds("\"^xFF;\""), vec![Str("\u{ff}".into()), Eof]);
        // `^ddd` is decimal, not octal: 65 is 'A'.
        assert_eq!(kinds("\"^65;X\""), vec![Str("AX".into()), Eof]);
        // without the `;` the digits keep being consumed
        assert_eq!(kinds("\"^65\""), vec![Str("A".into()), Eof]);
    }

    #[test]
    fn ctrl_char_is_configurable_like_pragma_ctrlchar() {
        let mut diags = Diagnostics::new();
        let mut sc = Scanner::new("\"a\\nb^n\"", "t.sma");
        sc.set_ctrl_char(b'\\');
        let toks = sc.scan(&mut diags);
        assert!(diags.items().is_empty());
        assert_eq!(toks[0].kind, TokenKind::Str("a\nb^n".into()));
    }

    #[test]
    fn packed_and_raw_string_prefixes() {
        use TokenKind::*;
        assert_eq!(kinds("!\"hi^n\""), vec![PackedStr("hi\n".into()), Eof]);
        // raw mode: escapes are not translated
        assert_eq!(kinds("^\"hi^n\""), vec![Str("hi^n".into()), Eof]);
        assert_eq!(kinds("!^\"a^n\""), vec![PackedStr("a^n".into()), Eof]);
        assert_eq!(kinds("^!\"a^n\""), vec![PackedStr("a^n".into()), Eof]);
    }

    #[test]
    fn unterminated_string_reports_error_37() {
        assert_eq!(codes("\"abc\nx"), vec![37]);
        assert_eq!(codes("\"abc"), vec![37]);
    }

    #[test]
    fn strings_continue_across_a_backslash_line_break() {
        use TokenKind::*;
        // readline() strips the marker, the newline and the next line's indentation.
        assert_eq!(kinds("\"abc\\\n    def\""), vec![Str("abcdef".into()), Eof]);
    }

    #[test]
    fn line_continuation_works_outside_strings_too() {
        use TokenKind::*;
        let k = kinds("new a =\\\n 1;");
        assert_eq!(k, vec![New, Ident("a".into()), Assign, Int(1), Semi, Eof]);
    }

    #[test]
    fn continued_line_comment_is_error_49() {
        assert_eq!(codes("// note \\\nstill comment\n"), vec![49]);
    }

    // ----------------------------------------------------------- char literals

    #[test]
    fn char_literals_are_integer_tokens() {
        use TokenKind::*;
        assert_eq!(kinds("'a'"), vec![Int(97), Eof]);
        assert_eq!(kinds("'^n'"), vec![Int(10), Eof]);
        assert_eq!(kinds("'^^'"), vec![Int(b'^' as i64), Eof]);
        assert_eq!(kinds("'^x41;'"), vec![Int(65), Eof]);
        assert_eq!(kinds("'^''"), vec![Int(39), Eof]);
    }

    #[test]
    fn bad_char_literal_reports_error_27() {
        // Two characters: the closing quote is not where it should be. Resync then
        // reads `b` as a symbol and the final `'` as another (unterminated) literal,
        // so a second 27 follows - exactly what amxxpc does with this input.
        assert_eq!(codes("'ab'"), vec![27, 27]);
        // unknown escape
        assert!(codes("'^q'").contains(&27));
    }

    // -------------------------------------------------------------- comments

    #[test]
    fn block_and_line_comments_are_skipped() {
        use TokenKind::*;
        assert_eq!(kinds("a /* x\ny */ b"), vec![Ident("a".into()), Ident("b".into()), Eof]);
        assert_eq!(kinds("a // x\nb"), vec![Ident("a".into()), Ident("b".into()), Eof]);
    }

    #[test]
    fn slash_slash_star_is_a_line_comment() {
        use TokenKind::*;
        // stripcom() tests `/*` first, and `//*` fails that test because the second
        // character is `/`. So this is a line comment and `b` is live code.
        assert_eq!(kinds("a //* still a line comment\nb"), vec![
            Ident("a".into()),
            Ident("b".into()),
            Eof
        ]);
    }

    #[test]
    fn nested_block_comment_open_is_error_216_and_does_not_nest() {
        use TokenKind::*;
        let (k, codes) = lex("/* a /* b */ c");
        assert_eq!(codes, vec![216]);
        // the first `*/` closed the comment, so `c` is code
        assert_eq!(k, vec![Ident("c".into()), Eof]);
    }

    #[test]
    fn unterminated_block_comment_reports_error_1() {
        assert_eq!(codes("/* forever"), vec![1]);
    }

    // ------------------------------------------------------------- operators

    #[test]
    fn operator_matching_is_greedy_longest_first() {
        use TokenKind::*;
        assert_eq!(kinds("a>>>=b"), vec![Ident("a".into()), UShrAssign, Ident("b".into()), Eof]);
        assert_eq!(kinds("a>>>b"), vec![Ident("a".into()), UShr, Ident("b".into()), Eof]);
        assert_eq!(kinds("a>>b"), vec![Ident("a".into()), Shr, Ident("b".into()), Eof]);
        assert_eq!(kinds("a>=b"), vec![Ident("a".into()), GtEq, Ident("b".into()), Eof]);
        assert_eq!(kinds("a>b"), vec![Ident("a".into()), Gt, Ident("b".into()), Eof]);
        assert_eq!(kinds("a<<=b"), vec![Ident("a".into()), ShlAssign, Ident("b".into()), Eof]);
        assert_eq!(kinds("..."), vec![Ellipsis, Eof]);
    }

    #[test]
    fn single_character_tokens() {
        use TokenKind::*;
        assert_eq!(kinds("{ } [ ] ( ) ; , ? ~"), vec![
            LBrace, RBrace, LBracket, RBracket, LParen, RParen, Semi, Comma, Question, Tilde, Eof
        ]);
    }

    #[test]
    fn caret_alone_is_the_xor_operator() {
        use TokenKind::*;
        assert_eq!(kinds("a ^ b"), vec![Ident("a".into()), Caret, Ident("b".into()), Eof]);
        assert_eq!(kinds("a ^= b"), vec![Ident("a".into()), XorAssign, Ident("b".into()), Eof]);
    }

    // ------------------------------------------------------------ directives

    #[test]
    fn directives_are_recognised() {
        use TokenKind::*;
        let k = kinds("#include <amxmodx>");
        assert_eq!(k[0], PpInclude);
        assert_eq!(kinds("#pragma semicolon 1")[0], PpPragma);
    }

    #[test]
    fn unknown_directive_reports_error_31() {
        assert_eq!(codes("#nonsense"), vec![31]);
    }

    // ----------------------------------------------------------------- spans

    #[test]
    fn spans_are_byte_accurate() {
        let toks = tokens("new x = 12;");
        assert_eq!(toks[0].span, Span::new(0, 3)); // new
        assert_eq!(toks[1].span, Span::new(4, 5)); // x
        assert_eq!(toks[2].span, Span::new(6, 7)); // =
        assert_eq!(toks[3].span, Span::new(8, 10)); // 12
        assert_eq!(toks[4].span, Span::new(10, 11)); // ;
        assert_eq!(toks[5].kind, TokenKind::Eof);
    }

    #[test]
    fn string_span_covers_the_quotes_and_prefix() {
        let toks = tokens("!\"ab\"");
        assert_eq!(toks[0].span, Span::new(0, 5));
    }

    // ------------------------------------------------------------ line_start

    #[test]
    fn line_start_marks_the_first_token_of_each_line() {
        let toks = tokens("a b\nc\n\n  d");
        let flags: Vec<bool> = toks.iter().map(|t| t.line_start).collect();
        // a, b, c, d, eof - eof sits on `d`'s line because the input has no
        // trailing newline.
        assert_eq!(flags, vec![true, false, true, true, false]);
        // ...and with a trailing newline it does start a line.
        assert!(tokens("a\n").last().unwrap().line_start);
    }

    #[test]
    fn a_spliced_line_is_still_one_line() {
        let toks = tokens("a \\\n b");
        assert!(toks[0].line_start);
        assert!(!toks[1].line_start);
    }

    #[test]
    fn a_block_comment_spanning_lines_starts_a_new_line() {
        let toks = tokens("a /* \n */ b");
        assert!(toks[1].line_start);
    }

    // ------------------------------------------------------------------ misc

    #[test]
    fn empty_input_yields_only_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
        assert_eq!(kinds("   \n\t "), vec![TokenKind::Eof]);
    }

    #[test]
    fn a_realistic_plugin_fragment_lexes_without_diagnostics() {
        let src = "\
#include <amxmodx>

public plugin_init() {
    register_plugin(\"Test^n\", \"1.0\", \"me\");
    new Float:f = 1.5, mask = 0b1010 | 0xFF;
    for (new i = 0; i < 10; i++) {
        if (i >= 5 && (mask >>> i) != 0) continue;
    }
}
";
        let (k, codes) = lex(src);
        assert!(codes.is_empty(), "unexpected diagnostics: {codes:?}");
        assert!(k.contains(&TokenKind::UShr));
        assert!(k.contains(&TokenKind::Rational(1.5)));
        assert!(k.contains(&TokenKind::Label("Float".into())));
        assert_eq!(k.last(), Some(&TokenKind::Eof));
    }
}
