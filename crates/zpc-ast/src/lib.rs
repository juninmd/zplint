//! AST node definitions for `zpc`, a Rust reimplementation of the AMX Mod X Pawn
//! compiler (`amxxpc`).
//!
//! The shapes here mirror the constructs the original compiler actually parses
//! (`libpc300/sc1.c` for declarations and statements, `sc3.c` for expressions),
//! not the constructs a "C-like language" would be expected to have. Pawn differs
//! from C in ways that are load-bearing for the parser, the semantic analyser and
//! the code generator alike; those differences are called out on the individual
//! types.
//!
//! # Conventions
//!
//! * Every node carries a [`Span`] (re-exported from `zpc_diag`) covering the whole
//!   construct, including any leading keywords and trailing punctuation that the
//!   node owns.
//! * Enums are flat: variants hold their own data instead of pointing at a separate
//!   "payload" struct, except where the payload is shared between variants.
//!   Recursive expression children are boxed; everything else is stored inline or
//!   in a `Vec`.
//! * Every node type derives `Debug` and `Clone`. `PartialEq` is derived only on
//!   nodes that cannot transitively contain a floating-point literal, because
//!   Pawn rationals ([`expr::RationalLit`]) are `f64` and structural equality on
//!   `NaN`/`-0.0` would be a trap for a semantic pass comparing trees.
//! * Each recoverable syntax error gets an explicit `Error` variant
//!   ([`decl::Item::Error`], [`stmt::Stmt::Error`], [`expr::ExprKind::Error`]) so a
//!   parser can keep going and later passes can skip poisoned subtrees instead of
//!   guessing.
//!
//! # Not modelled here
//!
//! The preprocessor (`#define`, `#include`, `#if`, macro substitution, `#assert`,
//! `#error`, `#file`, `#line`) runs *inside* the lexer in `sc2.c` and never reaches
//! the parser, so it produces no AST. The two directives that do survive into the
//! parse are [`decl::Pragma`] (settings that later phases must honour) and
//! [`stmt::Stmt::Emit`] (inline assembly, which is a statement).

#![forbid(unsafe_code)]

pub mod decl;
pub mod expr;
pub mod stmt;

pub use zpc_diag::Span;

/// A whole translation unit: everything one `.sma` file expands to after
/// preprocessing. Mirrors the loop in `parse()` (`sc1.c`), which accepts only
/// declarations at the top level - Pawn has no top-level statements.
#[derive(Clone, Debug)]
pub struct Program {
    pub items: Vec<decl::Item>,
    pub span: Span,
}

/// A source-level identifier: a variable, function, tag, label, state or
/// automaton name.
///
/// The name is stored exactly as written. Two Pawn quirks are deliberately *not*
/// normalised away here, because later phases need to see them:
///
/// * `amxxpc` truncates identifiers to `sNAMEMAX` (31) significant characters;
///   truncation is a semantic concern, not a syntactic one.
/// * A leading `@` ([`PUBLIC_CHAR`]) makes a function or global variable
///   implicitly `public` even without the keyword (`declglb()`/`newfunc()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self { name: name.into(), span }
    }

    /// True if the name starts with `@`, which makes the symbol implicitly public.
    pub fn is_implicitly_public(&self) -> bool {
        self.name.starts_with(PUBLIC_CHAR)
    }
}

/// The prefix character that marks a symbol as implicitly `public` (`PUBLIC_CHAR`
/// in `sc.h`). AMX Mod X uses it for forward handlers such as `@task_think`.
pub const PUBLIC_CHAR: char = '@';

/// The name of the "untagged" tag. Writing `_:expr` forces an expression back to
/// no tag at all, and `{Float,_}:x` declares an argument accepting either a
/// `Float:` or an untagged value.
pub const UNTAGGED: &str = "_";

/// A reference to a tag by name, as written at a use site: the `Float` in
/// `Float:x`, `new Float:f`, or `tagof(Float:)`.
///
/// Pawn tags are not types: they are a weak, nominal annotation checked by
/// `matchtag()` and coercible in many directions. A tag is never *declared*;
/// it springs into existence the first time it is mentioned (`pc_addtag`), which
/// is why there is no tag-declaration node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRef {
    pub name: Ident,
    pub span: Span,
}

impl TagRef {
    /// True for the pseudo-tag `_`, which means "explicitly untagged".
    pub fn is_untagged(&self) -> bool {
        self.name.name == UNTAGGED
    }
}

/// One or more tags accepted at a single position, written `Tag:` or
/// `{TagA,TagB,_}:`.
///
/// Only function arguments may carry more than one tag (`declargs()` in `sc1.c`
/// reads up to `MAXTAGS` = 16 of them). Variables, constants, enum members and
/// return values allow exactly one, so those nodes store an `Option<TagRef>`
/// instead of this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSpec {
    pub tags: Vec<TagRef>,
    pub span: Span,
}

/// One array dimension in a *declaration*, i.e. one `[...]` group.
///
/// `size` is `None` for an indeterminate dimension (`msg[]`): legal for function
/// arguments, for the last dimension of an initialised variable (the size is
/// then deduced from the initialiser), and never for a `native`/`forward` array
/// return value. When present, the expression must fold to a constant
/// (`needsub()` calls `constexpr()`), and its *tag* also becomes the dimension's
/// index tag - that is how `new data[Player]` gives `data[...]` an enum-typed
/// index without any extra syntax.
#[derive(Clone, Debug)]
pub struct Dim {
    pub size: Option<expr::Expr>,
    pub span: Span,
}

/// A reference to one state of one automaton, as used both in a function's state
/// list (`func() <running> {}`) and in a `state` transition statement.
///
/// `automaton` is `None` for the anonymous automaton, which is what almost all
/// real code uses: `<idle>` rather than `<engine:idle>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRef {
    pub automaton: Option<Ident>,
    pub state: Ident,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::decl::*;
    use super::expr::*;
    use super::stmt::*;
    use super::*;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn id(name: &str) -> Ident {
        Ident::new(name, sp())
    }

    fn e(kind: ExprKind) -> Expr {
        Expr { kind, span: sp() }
    }

    fn num(v: i64) -> Expr {
        e(ExprKind::Num(v))
    }

    fn var(name: &str) -> Expr {
        e(ExprKind::Ident(id(name)))
    }

    /// Builds the tree for a small but representative plugin:
    ///
    /// ```pawn
    /// stock check_player(id, Float:origin[3]) {
    ///     if (id > 0) {
    ///         switch (id) {
    ///             case 1 .. 5: log_amx("low");
    ///             case 7, 9: log_amx("odd");
    ///             default: return 0;
    ///         }
    ///     } else {
    ///         return -1;
    ///     }
    ///     return get_distance(origin, .to = origin);
    /// }
    /// ```
    fn sample_function() -> FuncDecl {
        let case_range = SwitchCase {
            labels: vec![CaseLabel::Range { lo: num(1), hi: num(5), span: sp() }],
            body: Box::new(Stmt::Expr {
                expr: e(ExprKind::Call {
                    callee: Box::new(var("log_amx")),
                    args: vec![Arg {
                        name: None,
                        value: ArgValue::Expr(e(ExprKind::Str(StringLit {
                            value: "low".into(),
                            packed: false,
                            raw: false,
                            span: sp(),
                        }))),
                        span: sp(),
                    }],
                    parenthesised: true,
                    span: sp(),
                }),
                span: sp(),
            }),
            span: sp(),
        };

        let case_multi = SwitchCase {
            labels: vec![CaseLabel::Single(num(7)), CaseLabel::Single(num(9))],
            body: Box::new(Stmt::Expr {
                expr: e(ExprKind::Call {
                    callee: Box::new(var("log_amx")),
                    args: vec![Arg {
                        name: None,
                        value: ArgValue::Expr(e(ExprKind::Str(StringLit {
                            value: "odd".into(),
                            packed: false,
                            raw: false,
                            span: sp(),
                        }))),
                        span: sp(),
                    }],
                    parenthesised: true,
                    span: sp(),
                }),
                span: sp(),
            }),
            span: sp(),
        };

        let switch = Stmt::Switch {
            scrutinee: var("id"),
            cases: vec![case_range, case_multi],
            default: Some(Box::new(Stmt::Return { value: Some(num(0)), span: sp() })),
            span: sp(),
        };

        let branch = Stmt::If {
            cond: e(ExprKind::Binary {
                op: BinOp::Gt,
                lhs: Box::new(var("id")),
                rhs: Box::new(num(0)),
                span: sp(),
            }),
            then_branch: Box::new(Stmt::Block(Block { stmts: vec![switch], span: sp() })),
            else_branch: Some(Box::new(Stmt::Block(Block {
                stmts: vec![Stmt::Return {
                    value: Some(e(ExprKind::Unary {
                        op: UnOp::Neg,
                        operand: Box::new(num(1)),
                        span: sp(),
                    })),
                    span: sp(),
                }],
                span: sp(),
            }))),
            span: sp(),
        };

        let tail = Stmt::Return {
            value: Some(e(ExprKind::Call {
                callee: Box::new(var("get_distance")),
                args: vec![
                    Arg { name: None, value: ArgValue::Expr(var("origin")), span: sp() },
                    Arg {
                        name: Some(id("to")),
                        value: ArgValue::Expr(var("origin")),
                        span: sp(),
                    },
                ],
                parenthesised: true,
                span: sp(),
            })),
            span: sp(),
        };

        FuncDecl {
            kind: FuncKind::Normal,
            modifiers: FuncModifiers { public: false, stock: true, static_: false },
            return_tag: None,
            return_dims: Vec::new(),
            name: FuncName::Ident(id("check_player")),
            params: vec![
                Param::Fixed(ParamDecl {
                    name: id("id"),
                    tags: None,
                    by_ref: false,
                    is_const: false,
                    dims: Vec::new(),
                    default: None,
                    span: sp(),
                }),
                Param::Fixed(ParamDecl {
                    name: id("origin"),
                    tags: Some(TagSpec {
                        tags: vec![TagRef { name: id("Float"), span: sp() }],
                        span: sp(),
                    }),
                    by_ref: false,
                    is_const: false,
                    dims: vec![Dim { size: Some(num(3)), span: sp() }],
                    default: None,
                    span: sp(),
                }),
            ],
            states: None,
            alias: None,
            body: Some(Block { stmts: vec![branch, tail], span: sp() }),
            span: sp(),
        }
    }

    #[test]
    fn a_representative_function_composes() {
        let func = sample_function();
        let program = Program { items: vec![Item::Func(func)], span: sp() };

        let Item::Func(f) = &program.items[0] else { panic!("expected a function") };
        assert_eq!(f.params.len(), 2);
        assert!(f.modifiers.stock);

        // The array argument keeps its tag and its single constant dimension.
        let Param::Fixed(origin) = &f.params[1] else { panic!("expected a fixed param") };
        assert_eq!(origin.dims.len(), 1);
        assert!(origin.tags.as_ref().unwrap().tags[0].name.name == "Float");

        // The body is if/else + a return; the switch lives inside the "then" arm.
        let body = f.body.as_ref().expect("function has a body");
        assert_eq!(body.stmts.len(), 2);
        let Stmt::If { then_branch, else_branch, .. } = &body.stmts[0] else {
            panic!("expected an if")
        };
        assert!(else_branch.is_some());
        let Stmt::Block(then_block) = then_branch.as_ref() else { panic!("expected a block") };
        let Stmt::Switch { cases, default, .. } = &then_block.stmts[0] else {
            panic!("expected a switch")
        };
        assert!(default.is_some());
        assert!(matches!(cases[0].labels[0], CaseLabel::Range { .. }));
        assert_eq!(cases[1].labels.len(), 2, "case 7, 9: is one case with two labels");
    }

    #[test]
    fn named_call_arguments_round_trip() {
        let func = sample_function();
        let body = func.body.unwrap();
        let Stmt::Return { value: Some(call), .. } = &body.stmts[1] else {
            panic!("expected a return")
        };
        let ExprKind::Call { args, .. } = &call.kind else { panic!("expected a call") };
        assert!(args[0].name.is_none(), "positional argument");
        assert_eq!(args[1].name.as_ref().unwrap().name, "to", ".to = origin");
    }

    #[test]
    fn tree_is_cloneable_and_borrow_free() {
        // Cloning a whole program must not require any lifetime gymnastics: the
        // parser hands trees to sema and codegen by value.
        let program = Program { items: vec![Item::Func(sample_function())], span: sp() };
        let copy = program.clone();
        assert_eq!(copy.items.len(), program.items.len());
    }

    #[test]
    fn declarations_cover_enum_and_array_initialisers() {
        // enum (<<= 1) Flags { FlagA = 1, FlagB, Coords[3] };
        let flags = Item::Enum(EnumDecl {
            tag: None,
            name: Some(id("Flags")),
            step: Some(EnumStep { op: EnumStepOp::Shl, value: num(1), span: sp() }),
            members: vec![
                EnumMember {
                    tag: None,
                    name: id("FlagA"),
                    size: None,
                    value: Some(num(1)),
                    span: sp(),
                },
                EnumMember { tag: None, name: id("FlagB"), size: None, value: None, span: sp() },
                EnumMember {
                    tag: Some(TagRef { name: id("Float"), span: sp() }),
                    name: id("Coords"),
                    size: Some(num(3)),
                    value: None,
                    span: sp(),
                },
            ],
            span: sp(),
        });

        // new g_grid[2][3] = { {1, 2, 3}, {4, ...} };
        let grid = Item::Var(VarDecl {
            modifiers: VarModifiers {
                public: false,
                static_: false,
                stock: false,
                is_const: false,
            },
            declarators: vec![Declarator {
                name: id("g_grid"),
                tag: None,
                dims: vec![
                    Dim { size: Some(num(2)), span: sp() },
                    Dim { size: Some(num(3)), span: sp() },
                ],
                init: Some(Init::List(InitList {
                    elems: vec![
                        Init::List(InitList {
                            elems: vec![Init::Expr(num(1)), Init::Expr(num(2)), Init::Expr(num(3))],
                            fill_rest: false,
                            span: sp(),
                        }),
                        Init::List(InitList {
                            elems: vec![Init::Expr(num(4))],
                            fill_rest: true,
                            span: sp(),
                        }),
                    ],
                    fill_rest: false,
                    span: sp(),
                })),
                span: sp(),
            }],
            span: sp(),
        });

        let program = Program { items: vec![flags, grid], span: sp() };
        let Item::Enum(en) = &program.items[0] else { panic!("expected an enum") };
        assert_eq!(en.members.len(), 3);
        assert!(matches!(en.step.as_ref().unwrap().op, EnumStepOp::Shl));
        assert!(en.members[2].size.is_some(), "Coords[3] is a sized enum member");

        let Item::Var(v) = &program.items[1] else { panic!("expected a variable") };
        let Some(Init::List(outer)) = &v.declarators[0].init else { panic!("expected a list") };
        let Init::List(second_row) = &outer.elems[1] else { panic!("expected a nested list") };
        assert!(second_row.fill_rest, "{{4, ...}} fills the rest of the row");
    }

    #[test]
    fn binop_precedence_matches_the_hier_ladder() {
        // sc3.c: hier3 (* / %) binds tightest, hier12 (||) loosest.
        assert!(BinOp::Mul.precedence() > BinOp::Add.precedence());
        assert!(BinOp::Add.precedence() > BinOp::Shl.precedence());
        assert!(BinOp::Shl.precedence() > BinOp::BitAnd.precedence());
        assert!(BinOp::BitAnd.precedence() > BinOp::BitXor.precedence());
        assert!(BinOp::BitXor.precedence() > BinOp::BitOr.precedence());
        assert!(BinOp::BitOr.precedence() > BinOp::Lt.precedence());
        assert!(BinOp::Lt.precedence() > BinOp::Eq.precedence());
        assert!(BinOp::Eq.precedence() > BinOp::LogAnd.precedence());
        assert!(BinOp::LogAnd.precedence() > BinOp::LogOr.precedence());
        assert!(BinOp::Lt.is_relational() && !BinOp::Eq.is_relational());
    }

    #[test]
    fn implicit_public_prefix_is_detected() {
        assert!(id("@task_think").is_implicitly_public());
        assert!(!id("task_think").is_implicitly_public());
        assert!(TagRef { name: id(UNTAGGED), span: sp() }.is_untagged());
    }
}
