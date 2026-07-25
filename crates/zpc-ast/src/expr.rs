//! Expressions and the operator precedence ladder.
//!
//! Reference: `hier14()` down to `primary()`/`constant()` in `libpc300/sc3.c`.
//! The original compiler encodes precedence as a chain of recursive-descent
//! functions (`hier14` is the loosest, `hier1` the tightest); [`BinOp::precedence`]
//! reproduces that ordering for a table-driven parser.

use crate::{Ident, Span, TagRef};

/// An expression node. Every expression carries a span; recursive children are
/// boxed.
#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// The kinds of expression Pawn has.
#[derive(Clone, Debug)]
pub enum ExprKind {
    /// An integer literal: decimal, `0x`/`$` hex, `0b` binary or `0o` octal.
    /// Cells are signed and platform-sized (32-bit in AMX Mod X).
    Num(i64),
    /// A rational literal such as `1.5`. Its tag is whatever `#pragma rational`
    /// designated (`Float:` in AMX Mod X), and its representation is either an
    /// IEEE float or a fixed-point value depending on that pragma.
    Rational(RationalLit),
    /// A character literal, `'a'`.
    ///
    /// The lexer folds this to a plain number, so semantically it *is* a
    /// [`ExprKind::Num`]; it is kept apart here so tooling can tell `'\n'` from
    /// `10` when reporting or rewriting source.
    Char { value: i64, span: Span },
    /// A string literal.
    Str(StringLit),
    /// A literal array in expression position: `{1, 2, 3}`.
    ///
    /// `constant()` accepts this and materialises it in the literal pool, so it
    /// behaves like an anonymous global array. All elements must be constant and
    /// share one tag.
    LitArray { elems: Vec<Expr>, span: Span },
    /// A reference to a variable, constant, or function.
    Ident(Ident),
    /// A comma expression: `(a, b)` and the `expr1, expr2` clauses of a `for`.
    /// The value is that of the last operand.
    Comma { exprs: Vec<Expr>, span: Span },
    /// A prefix operator: `-x`, `!x`, `~x`.
    ///
    /// There is no unary `+` in Pawn - `hier2()` simply does not accept it.
    Unary { op: UnOp, operand: Box<Expr>, span: Span },
    /// `++x`, `x--`, and so on.
    IncDec { op: IncDecOp, fixity: Fixity, operand: Box<Expr>, span: Span },
    /// A binary operator.
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// An assignment: `=` when `op` is `None`, otherwise a compound assignment
    /// such as `+=` or `>>>=`.
    ///
    /// Assigning a whole array to a whole array is legal in Pawn and copies the
    /// contents, so `target` is not restricted to scalars.
    Assign { op: Option<BinOp>, target: Box<Expr>, value: Box<Expr>, span: Span },
    /// `cond ? a : b`. Right-associative, and the two arms must agree on whether
    /// they yield an array.
    Ternary { cond: Box<Expr>, then_expr: Box<Expr>, else_expr: Box<Expr>, span: Span },
    /// An array subscript.
    Index { base: Box<Expr>, index: Box<Expr>, kind: IndexKind, span: Span },
    /// A function call.
    Call { callee: Box<Expr>, args: Vec<Arg>, parenthesised: bool, span: Span },
    /// A tag override (a "cast"): `Float:x`, `_:x`.
    ///
    /// This changes only the tag the compiler ascribes to the value - no
    /// conversion code is generated, which is why `Float:1` is a bit pattern and
    /// not `1.0`. `_:x` strips the tag.
    Cast { tag: TagRef, expr: Box<Expr>, span: Span },
    /// `sizeof x`, `sizeof(x)`, `sizeof x[]`, `sizeof x[Field]`.
    ///
    /// Not a general operator: `hier2()` requires a bare *symbol*, not an
    /// expression, and folds the result at compile time. The parentheses are
    /// optional and may even be repeated.
    SizeOf { symbol: Ident, levels: Vec<SizeOfLevel>, span: Span },
    /// `tagof x` or `tagof(Tag:)` - yields the numeric tag id, marked public.
    TagOf { target: TagOfTarget, levels: Vec<SizeOfLevel>, span: Span },
    /// `defined SYMBOL` - 1 if the name is a defined symbol *or* a `#define`
    /// macro, else 0. Folded at compile time.
    Defined { symbol: Ident, span: Span },
    /// The postfix `char` operator: `n char` is the number of cells needed to
    /// hold `n` packed characters. Used for sizing packed strings.
    CharCells { operand: Box<Expr>, span: Span },
    /// An expression the parser could not build.
    Error(Span),
}

/// A rational (floating-point) literal.
///
/// `PartialEq` is deliberately not derived anywhere in the expression tree
/// because of this `f64`: two structurally identical trees containing `NaN` would
/// compare unequal, and `0.0`/`-0.0` would compare equal despite differing bit
/// patterns and differing generated code.
#[derive(Clone, Debug)]
pub struct RationalLit {
    pub value: f64,
    /// The literal exactly as written, so diagnostics and formatters can
    /// reproduce it and a fixed-point backend can re-scale from the digits.
    pub raw: String,
    pub span: Span,
}

/// A string literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringLit {
    /// The decoded contents, escapes already resolved (unless `raw`).
    pub value: String,
    /// `!"text"` - packed: several characters per cell, rather than one cell per
    /// character. The default is set by `#pragma pack`.
    pub packed: bool,
    /// `\"text"` (using the current `#pragma ctrlchar`) - raw: escape sequences
    /// are not interpreted. `\!"text"` combines both.
    pub raw: bool,
    pub span: Span,
}

/// Which bracket was used for a subscript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexKind {
    /// `arr[i]` - selects a cell, or a sub-array in a multi-dimensional array.
    Cell,
    /// `arr{i}` - selects a single *character* out of a packed array. Only valid
    /// on the last dimension (error 51 otherwise).
    Char,
}

/// One argument at a call site.
#[derive(Clone, Debug)]
pub struct Arg {
    /// `Some` for a named argument, `.name = value` (`callfunction()` in `sc3.c`).
    ///
    /// Named arguments may be given in any order but must all follow the
    /// positional ones (error 44). The leading `.` is also what lets the compiler
    /// recognise a continuation line in the parenthesis-less call syntax.
    pub name: Option<Ident>,
    pub value: ArgValue,
    pub span: Span,
}

/// The value passed for one argument.
#[derive(Clone, Debug)]
pub enum ArgValue {
    Expr(Expr),
    /// A bare `_` placeholder, meaning "use this parameter's default value".
    /// It is an error if the parameter has none (error 34).
    Default(Span),
}

/// One `[...]` after the symbol in `sizeof`/`tagof`.
///
/// `sizeof arr[]` asks for the size of a sub-dimension. At the *innermost* level
/// the brackets may instead name an enumeration field - `sizeof arr[Coords]` -
/// which yields that field's declared size, the idiom for enum-as-struct arrays.
#[derive(Clone, Debug)]
pub struct SizeOfLevel {
    pub field: Option<Ident>,
    pub span: Span,
}

/// What `tagof` is applied to.
#[derive(Clone, Debug)]
pub enum TagOfTarget {
    /// `tagof value` - the tag of a variable or constant.
    Symbol(Ident),
    /// `tagof(Float:)` - a tag name written directly, with its trailing colon.
    Tag(TagRef),
}

/// Prefix operators (`hier2()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-` - arithmetic negation. Overloadable, and special-cased for rational
    /// constants so the sign is flipped in the literal.
    Neg,
    /// `!` - logical negation; the result is tagged `bool:`.
    LogNot,
    /// `~` - bitwise complement.
    BitNot,
}

/// `++` or `--`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncDecOp {
    Inc,
    Dec,
}

/// Whether an increment/decrement is written before or after its operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixity {
    /// `++x` - the result is the new value.
    Prefix,
    /// `x++` - the result is the old value.
    Postfix,
}

/// Binary operators, in the precedence order of `sc3.c`'s `hier` ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    // hier3
    Mul,
    Div,
    Mod,
    // hier4
    Add,
    Sub,
    // hier5
    /// `<<`
    Shl,
    /// `>>` - arithmetic (sign-propagating) shift right.
    Shr,
    /// `>>>` - logical (zero-filling) shift right.
    ShrU,
    // hier6..8
    BitAnd,
    BitXor,
    BitOr,
    // hier9 (relational)
    Lt,
    Le,
    Gt,
    Ge,
    // hier10
    Eq,
    Ne,
    // hier11..12
    LogAnd,
    LogOr,
}

impl BinOp {
    /// Binding strength: a higher number binds tighter. The values correspond to
    /// the `hier` level in `sc3.c` (`hier3` = 12 down to `hier12` = 3), so a
    /// precedence-climbing parser can consume this table directly.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Mul | BinOp::Div | BinOp::Mod => 12,
            BinOp::Add | BinOp::Sub => 11,
            BinOp::Shl | BinOp::Shr | BinOp::ShrU => 10,
            BinOp::BitAnd => 9,
            BinOp::BitXor => 8,
            BinOp::BitOr => 7,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 6,
            BinOp::Eq | BinOp::Ne => 5,
            BinOp::LogAnd => 4,
            BinOp::LogOr => 3,
        }
    }

    /// True for `<`, `<=`, `>`, `>=`.
    ///
    /// Pawn *chains* these: `plnge_rel()` compiles `a < b < c` as `a < b && b < c`,
    /// evaluating `b` once, rather than as C's `(a < b) < c`. A parser must
    /// therefore treat a run of relational operators as one n-ary comparison; the
    /// AST represents the run as left-nested [`ExprKind::Binary`] nodes, and the
    /// semantic pass recognises the chain by this predicate.
    pub fn is_relational(self) -> bool {
        matches!(self, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }

    /// True for operators whose result is tagged `bool:`.
    pub fn is_boolean(self) -> bool {
        self.is_relational() || matches!(self, BinOp::Eq | BinOp::Ne | BinOp::LogAnd | BinOp::LogOr)
    }
}
