//! Declarations: everything legal at the top level of a `.sma` file, plus the
//! declaration forms that may also appear inside a function body.
//!
//! Reference: `parse()`, `declfuncvar()`, `declglb()`, `declloc()`, `decl_const()`,
//! `decl_enum()`, `funcstub()`, `newfunc()`, `declargs()`, `doarg()` and
//! `initials()` in `libpc300/sc1.c`.

use crate::{Dim, Ident, Span, StateRef, TagRef, TagSpec, expr::Expr, stmt::Block};

/// A top-level declaration.
///
/// `parse()` accepts nothing else at file scope: Pawn has no top-level statements
/// and no namespaces or modules. Note that the compiler cannot tell a function
/// from a variable until it reaches the `(` (see `declfuncvar()`), so a parser
/// will need the same lookahead; the AST records the decision, not the ambiguity.
#[derive(Clone, Debug)]
pub enum Item {
    /// `new`/`static`/`stock`/`public`/`const` global variable(s).
    Var(VarDecl),
    /// A function: an ordinary definition, a prototype, `native`, `forward`, or an
    /// operator overload.
    Func(FuncDecl),
    /// `const NAME = expr;`
    Const(ConstDecl),
    /// `enum [Tag:] [Name] [(step)] { ... }`
    Enum(EnumDecl),
    /// A `#pragma` that survives preprocessing and changes how later phases behave.
    Pragma(Pragma),
    /// A declaration the parser could not make sense of. Kept so that one bad
    /// declaration does not truncate the rest of the file; semantic passes skip it.
    Error(Span),
}

/// Class specifiers on a variable declaration, parsed by `getclassspec()`.
///
/// The combinations are constrained rather than free: `static` with `public` is
/// error 42, as is repeating any one specifier. `public` and `stock` together are
/// legal on a variable but not on a function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VarModifiers {
    /// `public` - exported in the AMX public-variable table. Also implied by a
    /// name beginning with `@`.
    pub public: bool,
    /// `static` - file-local at global scope, and function-lifetime at local scope.
    pub static_: bool,
    /// `stock` - may be discarded silently if never referenced.
    pub stock: bool,
    /// `const` - assignment is rejected (`uCONST`).
    pub is_const: bool,
}

/// One `new a, b[3] = {1, 2, 3};` declaration statement.
///
/// Pawn allows several declarators after one set of class specifiers, and each
/// declarator carries its *own* tag: in `new Float:a, b`, `b` is untagged, because
/// `declglb()`/`declloc()` re-read a tag at the top of every loop iteration.
#[derive(Clone, Debug)]
pub struct VarDecl {
    pub modifiers: VarModifiers,
    pub declarators: Vec<Declarator>,
    pub span: Span,
}

/// A single name in a [`VarDecl`], with its own tag, dimensions and initialiser.
#[derive(Clone, Debug)]
pub struct Declarator {
    pub name: Ident,
    /// `Float:` in `new Float:origin[3]`. At most one tag: multi-tag syntax
    /// (`{A,B}:`) is accepted for function arguments only.
    pub tag: Option<TagRef>,
    /// One entry per `[...]`, outermost first. Empty for a scalar.
    pub dims: Vec<Dim>,
    pub init: Option<Init>,
    pub span: Span,
}

/// The right-hand side of `=` in a variable declaration or an array argument
/// default.
///
/// Mirrors `initials()`/`initvector()`: a scalar takes a single constant
/// expression, an array takes a brace-enclosed list which may nest one level per
/// dimension (and one extra level for enumeration fields).
#[derive(Clone, Debug)]
pub enum Init {
    /// A single constant expression, or a string literal for a character array.
    Expr(Expr),
    /// `{ ... }`
    List(InitList),
}

/// A braced initialiser list.
///
/// `fill_rest` records a trailing `...`. Pawn's ellipsis is not "the rest is
/// zero": `initvector()` extrapolates, repeatedly adding the difference between
/// the last two supplied values, so `{1, 3, ...}` yields `1, 3, 5, 7, ...` and
/// `{7, ...}` (a single value) repeats `7`. Zero-fill happens only when there is
/// no ellipsis at all.
#[derive(Clone, Debug)]
pub struct InitList {
    pub elems: Vec<Init>,
    pub fill_rest: bool,
    pub span: Span,
}

/// `const NAME = <constant expression>;`
///
/// Legal at file scope and inside a function body (`statement()` dispatches
/// `tCONST` to `decl_const()` with `sLOCAL`). Unlike a `const` *variable*, this
/// creates a compile-time symbol with no storage.
#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub tag: Option<TagRef>,
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// `enum [Tag:] [Name] [(step)] { members }`
///
/// Four shapes exist and all four appear in real AMX Mod X code:
/// anonymous (`enum { A, B }`), named (`enum Colour { ... }`, where the name
/// doubles as the tag of every member), explicitly tagged (`enum Bit: { ... }`),
/// and both. The enum name itself also becomes a constant whose value is the
/// value *after* the last member - that is what makes `new data[Player]` size an
/// array by an enum.
#[derive(Clone, Debug)]
pub struct EnumDecl {
    /// An explicit `Tag:` label before the name. Distinct from `name`, because
    /// `enum _: { .. }` explicitly forces members to be untagged.
    pub tag: Option<TagRef>,
    pub name: Option<Ident>,
    pub step: Option<EnumStep>,
    pub members: Vec<EnumMember>,
    pub span: Span,
}

/// The parenthesised progression of an enum: `(+= 100)`, `(*= 2)`, `(<<= 1)`.
///
/// Only these three operators are accepted (`decl_enum()`); the token really is
/// the compound-assignment token, which is why the syntax looks like an
/// assignment but is not one.
#[derive(Clone, Debug)]
pub struct EnumStep {
    pub op: EnumStepOp,
    pub value: Expr,
    pub span: Span,
}

/// Which progression an [`EnumStep`] applies between successive members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumStepOp {
    /// `(+= n)` - add `n` (times the member size) after each member.
    Add,
    /// `(*= n)` - multiply by `n` after each member.
    Mult,
    /// `(<<= n)` - multiply by `2^n`; the classic bit-flag enum.
    Shl,
}

/// One member of an [`EnumDecl`].
///
/// A member may carry its own tag (`Float:origin`), an explicit value
/// (`A = 5`), and a *size* (`Coords[3]`). The size is not an array declaration:
/// it makes the member occupy several cells so that `arr[Coords]` addresses a
/// sub-range, which is how Pawn 3 fakes a struct. The size also advances the next
/// member's value by that amount.
#[derive(Clone, Debug)]
pub struct EnumMember {
    pub tag: Option<TagRef>,
    pub name: Ident,
    /// The `[3]` in `Coords[3]`.
    pub size: Option<Expr>,
    /// The `= 5` in `A = 5`; otherwise the value follows from the progression.
    pub value: Option<Expr>,
    pub span: Span,
}

/// Which flavour of function declaration this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuncKind {
    /// An ordinary function: a definition when [`FuncDecl::body`] is `Some`, an
    /// old-style prototype (`func(a, b);`) when it is `None`.
    Normal,
    /// `native` - implemented by the host; never has a body, may have an alias.
    Native,
    /// `forward` - a declaration of a callback the host will invoke; never has a
    /// body. This is the AMX Mod X plugin-entry mechanism.
    Forward,
}

/// Class specifiers on a function (`getclassspec()`), a subset of
/// [`VarModifiers`]: a function may not be `const`, and `public` + `stock`
/// together is error 42.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FuncModifiers {
    /// `public` - callable from the host; also implied by an `@`-prefixed name.
    pub public: bool,
    /// `stock` - dropped from the output if never called.
    pub stock: bool,
    /// `static` - visible only within the file that declares it.
    pub static_: bool,
}

/// The name of a function: an ordinary identifier, or an operator being
/// overloaded.
#[derive(Clone, Debug)]
pub enum FuncName {
    Ident(Ident),
    /// `operator+(Float:a, Float:b)`. Pawn dispatches overloads on the *tags* of
    /// the arguments, so the mangled symbol name embeds them
    /// (`operatoradjust()` builds e.g. `operator+(Float,Float)`).
    Operator { op: OverloadableOp, span: Span },
}

/// The operators `operatorname()` accepts after the `operator` keyword.
///
/// This is the complete list; anything else is error 7. Note that `[]`, `()`,
/// `&&`, `||`, `+=` and friends are *not* overloadable, and that `~` is special:
/// it is the destructor-like operator and its single argument must be an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverloadableOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// `>` - the compiler derives `>=` and the reversed forms from the primitives.
    Gt,
    Lt,
    /// `!` - unary logical negation.
    Not,
    /// `~` - takes an array argument; used for user-defined destruction.
    BitNot,
    /// `=` - a one-argument conversion operator; its *return tag* selects it.
    Assign,
    Inc,
    Dec,
    Eq,
    Ne,
    Le,
    Ge,
}

/// A function declaration, definition, prototype, `native` or `forward`.
#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub kind: FuncKind,
    pub modifiers: FuncModifiers,
    /// The tag of the return value: the `Float` in `Float:get_speed()`.
    pub return_tag: Option<TagRef>,
    /// A function may return an *array*: `Float:make_vec()[3]`. The dimensions sit
    /// between the return tag and the name (`funcstub()`/`doreturn()`), and unlike
    /// argument dimensions they may not be indeterminate.
    pub return_dims: Vec<Dim>,
    pub name: FuncName,
    pub params: Vec<Param>,
    /// `<idle, running>` or `<>` after the parameter list. Illegal on `native`s
    /// and operators (error 82) and ignored on `forward`s (warning 231).
    pub states: Option<StateSpec>,
    /// The `= something` after a `native`'s parameter list.
    pub alias: Option<NativeAlias>,
    /// `None` for `native`, `forward`, and bare prototypes (`func(a);`).
    ///
    /// The body is normally a compound block, but Pawn permits a *single*
    /// statement as a whole function body (`newfunc()` calls `statement()`
    /// directly). Such a body is represented here as a [`Block`] holding one
    /// statement.
    pub body: Option<Block>,
    pub span: Span,
}

/// The `= name` / `= -17` suffix on a `native` declaration (`funcstub()`).
///
/// It renames the symbol the host looks up, letting several Pawn names bind to
/// one host native. The numeric form assigns a fixed (conventionally negative)
/// native index. It is optional for a plain `native` but *mandatory* for a native
/// operator overload, which has no legal exported name of its own.
#[derive(Clone, Debug)]
pub enum NativeAlias {
    /// `native foo(...) = bar;`
    Symbol(Ident),
    /// `native foo(...) = -3;`
    Index(Expr),
}

/// A function's state list: `func() <idle, running>` or the fall-back `func() <>`.
///
/// Pawn lets the same function name be defined several times, once per state
/// combination; the runtime dispatches on the automaton's current state. Only
/// ordinary functions may do this.
#[derive(Clone, Debug)]
pub struct StateSpec {
    /// The listed states. Empty exactly when `fallback` is set.
    pub states: Vec<StateRef>,
    /// `<>` - the implementation used when no other state list matches.
    pub fallback: bool,
    pub span: Span,
}

/// One entry in a parameter list.
#[derive(Clone, Debug)]
pub enum Param {
    Fixed(ParamDecl),
    /// `...` - the variable-argument marker (`iVARARGS`).
    ///
    /// It may carry tags (`{Float,_}:...`) constraining what may be passed, and it
    /// must be last. Arguments passed through `...` are always by reference in
    /// AMX, which is why `getarg()`/`setarg()` exist.
    Rest(RestParam),
}

/// A named function parameter.
#[derive(Clone, Debug)]
pub struct ParamDecl {
    pub name: Ident,
    /// `Float:` or `{Float,_}:`. `None` means untagged. A parameter is the only
    /// place Pawn accepts several alternative tags.
    pub tags: Option<TagSpec>,
    /// `&` - passed by reference. Mutually exclusive with `dims`: `&name[]` is
    /// error 67, because an array argument is already a reference.
    pub by_ref: bool,
    /// `const` - the callee promises not to write through the reference. This is
    /// load-bearing for arrays: a `const` array default can be passed straight
    /// from the data segment instead of being copied to the heap.
    pub is_const: bool,
    /// One entry per `[...]`. An array parameter is passed by reference
    /// (`iREFARRAY`) and its dimensions may be indeterminate (`msg[]`).
    pub dims: Vec<Dim>,
    pub default: Option<ParamDefault>,
    pub span: Span,
}

/// The `...` parameter.
#[derive(Clone, Debug)]
pub struct RestParam {
    pub tags: Option<TagSpec>,
    pub span: Span,
}

/// A parameter's default value (`doarg()`).
///
/// Public functions may not have defaults at all (error 59), nor may operator
/// overloads.
#[derive(Clone, Debug)]
pub enum ParamDefault {
    /// `= 5` on a scalar or reference parameter: any constant expression.
    Expr(Expr),
    /// `= {1, 2, 3}` on an array parameter. The data is copied to the heap per
    /// call unless the parameter is `const`.
    Array(Init),
    /// `= g_defaults` on an array parameter: the address of an existing *global*
    /// array is used directly. Locals are not accepted.
    Symbol(Ident),
    /// `= sizeof other`, `= sizeof other[]`, or the parenthesised
    /// `= sizeof(other)`.
    ///
    /// This is not an expression: it is resolved late, after the whole parameter
    /// list is known, against another *parameter* of the same function (see the
    /// `uSIZEOF` fixup loop at the end of `declargs()`). It is how
    /// `stock copy(dest[], len = sizeof dest)` works. `levels` counts the trailing
    /// `[]` pairs, selecting a sub-dimension.
    SizeOf { arg: Ident, levels: u8, span: Span },
    /// `= tagof other` - the same late-resolved mechanism, yielding the tag id of
    /// another parameter. Illegal on an indexed array (error 81), hence no levels.
    TagOf { arg: Ident, span: Span },
}

/// A `#pragma` directive that reaches the parser.
///
/// The argument text is kept verbatim in `args` rather than pre-parsed: the
/// original compiler consumes each pragma's tail with ad-hoc scanning inside
/// `sc2.c` (some take an expression, some a quoted string, some a bare word, some
/// a comma-separated symbol list), and reproducing that in the AST would bake one
/// interpretation into the shared contract. Later phases interpret `args` per
/// [`PragmaKind`].
#[derive(Clone, Debug)]
pub struct Pragma {
    pub kind: PragmaKind,
    /// The pragma's own name token, e.g. `library`.
    pub name: Ident,
    /// Everything after the name, trimmed, exactly as written.
    pub args: String,
    pub span: Span,
}

/// The complete set of pragmas `sc2.c` recognises. Anything else is warning 207.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PragmaKind {
    /// `#pragma amxlimit <n>` - cap on the generated AMX size.
    AmxLimit,
    /// `#pragma codepage <name>` - source character-set mapping.
    CodePage,
    /// `#pragma compress <0|1>` - opcode packing in the output.
    Compress,
    /// `#pragma ctrlchar <char>` - change the escape character from `\`.
    CtrlChar,
    /// `#pragma deprecated <text>` - attaches to the *next* declared symbol and
    /// makes every use of it emit warning 233 with this text.
    Deprecated,
    /// `#pragma dynamic <n>` - stack/heap size.
    Dynamic,
    /// `#pragma library <name>` - required module, recorded in the library table.
    Library,
    /// `#pragma reqlib <name>` - AMX Mod X: required library (`?rl_` prefix).
    ReqLib,
    /// `#pragma reqclass <name>` - AMX Mod X: required class (`?rc_` prefix).
    ReqClass,
    /// `#pragma loadlib <name>` - AMX Mod X: load-time library (`?f_` prefix).
    LoadLib,
    /// `#pragma explib <name> <class>` - AMX Mod X: exported library (`?el_`).
    ExpLib,
    /// `#pragma expclass <name> <class>` - AMX Mod X: exported class (`?ec_`).
    ExpClass,
    /// `#pragma defclasslib <name> <class>` - AMX Mod X: default class library.
    DefClassLib,
    /// `#pragma pack <0|1>` - whether bare string literals are packed.
    Pack,
    /// `#pragma rational <Tag>[(digits)]` - designates the rational tag, and
    /// with `digits` switches from IEEE floats to fixed point. May be set once.
    Rational,
    /// `#pragma semicolon <0|1>` - whether statement terminators are mandatory.
    Semicolon,
    /// `#pragma tabsize <n>` - tab width used by the loose-indentation warning.
    TabSize,
    /// `#pragma align` - align the *next* declared variable; consumed immediately
    /// by the following declaration.
    Align,
    /// `#pragma unused a, b` - suppress "symbol never used" for these symbols.
    Unused,
    /// `#pragma showstackusageinfo` - report stack usage after compiling.
    ShowStackUsageInfo,
    /// An unrecognised pragma; kept so a linter can report it rather than the
    /// parser silently dropping the line.
    Unknown,
}
