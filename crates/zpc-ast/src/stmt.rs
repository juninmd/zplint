//! Statements.
//!
//! Reference: `statement()`, `compound()`, `doif()`, `dowhile()`, `dodo()`,
//! `dofor()`, `doswitch()`, `doreturn()`, `dobreak()`, `docont()`, `dogoto()`,
//! `dolabel()`, `doassert()`, `doexit()`, `dosleep()` and `dostate()` in
//! `libpc300/sc1.c`; `#emit` is handled in `sc2.c`.

use crate::{
    Ident, Span, StateRef,
    decl::{ConstDecl, EnumDecl, VarDecl},
    expr::Expr,
};

/// A compound statement: `{ ... }`.
///
/// Each block opens a new scope level (`nestlevel`), so a local may shadow a
/// local from an enclosing block but not one from the same block. Labels are the
/// exception: they are function-wide, not block-scoped.
#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// A statement.
///
/// Declarations appear here as well: Pawn allows `new`, `static`, `const` and
/// `enum` inside a function body, and `statement()` dispatches them to the same
/// routines used at file scope.
#[derive(Clone, Debug)]
pub enum Stmt {
    /// `{ ... }`
    Block(Block),
    /// A bare `;`. `amxxpc` warns about it (error 36) but parses it.
    Empty(Span),
    /// An expression evaluated for its side effects.
    ///
    /// Pawn's "procedure call" syntax lives here: a statement that is just a
    /// function name, with no parentheses and no arguments, is a call
    /// (`sc_allowproccall` in `statement()`). `foo;` calls `foo()`, and
    /// `client_print id, print_chat, "hi"` is a call with arguments. The parser
    /// records this by setting [`crate::expr::ExprKind::Call::parenthesised`] to
    /// `false`.
    Expr { expr: Expr, span: Span },
    /// `new`/`static` local variable declaration.
    Var(VarDecl),
    /// A block-local `const`.
    Const(ConstDecl),
    /// A block-local `enum`.
    Enum(EnumDecl),
    /// `if (cond) stmt [else stmt]`
    If {
        cond: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
        span: Span,
    },
    /// `while (cond) stmt`
    While { cond: Expr, body: Box<Stmt>, span: Span },
    /// `do stmt while (cond);`
    ///
    /// Note that `continue` inside a `do` jumps to the *test*, not to the top of
    /// the loop body.
    DoWhile { body: Box<Stmt>, cond: Expr, span: Span },
    /// `for (init; cond; step) stmt`
    ///
    /// All three clauses are optional. A `new` declaration in `init` gets a scope
    /// of its own, one level deeper than the enclosing block, so the loop variable
    /// dies with the loop.
    For {
        init: Option<ForInit>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
        span: Span,
    },
    /// `switch (expr) { case ...: stmt ... default: stmt }`
    ///
    /// Pawn's switch is *not* C's: cases never fall through, so there is no
    /// `break`, and each case takes exactly one statement (use a block for more).
    Switch {
        scrutinee: Expr,
        cases: Vec<SwitchCase>,
        /// `default:`. It must be the last clause (error 15) and may appear once
        /// (error 16), so it is stored apart from `cases` rather than as a label.
        default: Option<Box<Stmt>>,
        span: Span,
    },
    /// `return [expr];`
    ///
    /// A function may return an array, but only a named one - `return "text"` is
    /// error 39. Mixing `return;` and `return value;` in one function is error 78.
    Return { value: Option<Expr>, span: Span },
    /// `break;` - leaves the innermost loop. Not used by `switch`.
    Break { span: Span },
    /// `continue;`
    Continue { span: Span },
    /// `goto label;` - jumps to a [`Stmt::Label`] anywhere in the same function.
    Goto { label: Ident, span: Span },
    /// `label:` - a jump target. Labels are function-scoped, and the compiler
    /// adjusts the stack manually when a jump crosses a declaration.
    Label { name: Ident, span: Span },
    /// `assert expr;`
    ///
    /// Compiled only when bounds checking is on; otherwise the expression is
    /// parsed and discarded. A comma-separated list is accepted, hence `Vec`.
    Assert { exprs: Vec<Expr>, span: Span },
    /// `exit [expr];` - aborts the AMX with the given code (`xEXIT`). The *tag* of
    /// the expression is passed alongside the value, so the host can tell what it
    /// means.
    Exit { value: Option<Expr>, span: Span },
    /// `sleep [expr];` - suspends the AMX (`xSLEEP`), to be resumed by the host.
    /// Like `exit`, it passes the expression's tag as well as its value.
    Sleep { value: Option<Expr>, span: Span },
    /// `state [(cond)] [automaton:]newstate;`
    ///
    /// Switches the automaton to another state, optionally only when `cond` holds.
    State { cond: Option<Expr>, target: StateRef, span: Span },
    /// `#emit OPCODE [operand]` - one instruction of inline assembly.
    Emit(EmitStmt),
    /// A statement the parser could not recover from; kept so the rest of the
    /// function still parses.
    Error(Span),
}

/// The first clause of a `for`.
#[derive(Clone, Debug)]
pub enum ForInit {
    /// `for (new i = 0; ...)`
    Decl(VarDecl),
    /// `for (i = 0; ...)`
    Expr(Expr),
}

/// One `case` clause of a `switch`.
///
/// A clause may list several labels: `case 1, 2:`. The body is a single statement
/// per Pawn's switch semantics.
#[derive(Clone, Debug)]
pub struct SwitchCase {
    pub labels: Vec<CaseLabel>,
    pub body: Box<Stmt>,
    pub span: Span,
}

/// One value or range in a `case`.
#[derive(Clone, Debug)]
pub enum CaseLabel {
    /// `case 3:` - a constant expression.
    Single(Expr),
    /// `case 1 .. 5:` - an inclusive range.
    ///
    /// Ranges exist because the AMX `SWITCH` opcode takes a flat table of
    /// (value, address) pairs: `doswitch()` *expands* the range into one table
    /// entry per value, and rejects an empty or reversed range (error 50). A very
    /// wide range therefore costs real output size - worth keeping distinct in the
    /// AST so a linter can say so.
    Range { lo: Expr, hi: Expr, span: Span },
}

/// One `#emit` line: a raw AMX opcode with an optional single operand.
///
/// The compiler does not validate the mnemonic - it is written straight to the
/// assembly stream - so `opcode` is kept as text (it may contain `.`, as in
/// `const.pri`). Case is insignificant; the original lowercases it.
#[derive(Clone, Debug)]
pub struct EmitStmt {
    pub opcode: Ident,
    pub operand: Option<EmitOperand>,
    pub span: Span,
}

/// The operand of an `#emit`.
///
/// A symbol operand is replaced by the symbol's *address*, and the symbol is
/// marked both read and written, since the compiler cannot tell what the
/// instruction will do with it.
#[derive(Clone, Debug)]
pub enum EmitOperand {
    /// An integer literal (or character constant).
    Int { value: i64, span: Span },
    /// A rational literal, emitted as its raw cell bit pattern.
    Rational { value: f64, span: Span },
    /// A variable or function name, replaced by its address.
    Symbol(Ident),
}
