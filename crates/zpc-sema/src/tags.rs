//! The Pawn **tag system**: interning, `matchtag()`, and every site that emits
//! warning 213 ("tag mismatch") or 229 ("index tag mismatch").
//!
//! # Provenance
//!
//! Ported from the zlib-licensed AMX Mod X compiler (`libpc300`):
//!
//! * `pc_addtag()` in `sc1.c` - interning and the `FIXEDTAG` bit;
//! * `tag2str()` / `operator_symname()` in `sc1.c` - operator mangling;
//! * `matchtag()`, `checktag()`, `check_userop()`, `plnge2()`, `hier13()`,
//!   `hier14()`, `hier1()` and `callfunction()` in `sc3.c` - the check sites.
//!
//! # The model
//!
//! A tag is **not** a type. It is a nominal annotation carried alongside a cell,
//! erased at run time, and checked only by `matchtag()`. Upstream stores a tag
//! as a small integer with two flag bits stolen from the top:
//!
//! ```text
//! PUBLICTAG 0x80000000  set only on the *value* produced by `tagof`
//! FIXEDTAG  0x40000000  set when the tag name starts with an uppercase letter
//! TAGMASK   ~PUBLICTAG
//! ```
//!
//! `FIXEDTAG` is the crux of warning 213. From `pc_addtag()`:
//!
//! ```c
//! tag = last + 1;
//! if (isupper(*name))
//!   tag |= (int)FIXEDTAG;
//! ```
//!
//! and from `matchtag()`:
//!
//! ```c
//! if (formaltag != actualtag && formaltag != pc_anytag && actualtag != pc_anytag) {
//!   if (!allowcoerce || formaltag != 0 || (actualtag & FIXEDTAG) != 0)
//!     return FALSE;
//! }
//! return TRUE;
//! ```
//!
//! So the **weak/strong rule**, verified in the C source, is:
//!
//! * a tag whose name begins with a **lowercase** letter is *weak*: it may be
//!   silently coerced **to untagged** (and only to untagged);
//! * a tag whose name begins with an **uppercase** letter is *strong* (`FIXEDTAG`)
//!   and coerces to nothing at all.
//!
//! It is deliberately **asymmetric and one-directional**. Coercion applies only
//! when the *formal* (destination) side is untagged. Untagged never coerces
//! *into* a tagged destination, which is exactly why `set_task(5, ...)` warns
//! (formal `Float:`, actual untagged) while `new x = bool:1` does not (formal
//! untagged, actual weak `bool`).
//!
//! Coercion is furthermore only consulted where the caller passes
//! [`Coercion::Allow`]; binary operators, the two arms of `?:`, `const`
//! declarations and literal-array elements pass `FALSE` upstream and therefore
//! reject even weak coercion.
//!
//! `any:` is checked by identity against `pc_anytag` *before* the coercion rule,
//! so it matches in both directions regardless of `allowcoerce`.
//!
//! # Deliberate non-exemption for `0`
//!
//! There is **no** special case for the literal `0` anywhere in `matchtag()`,
//! `checktag()` or `callfunction()`. Passing `0` to a `Float:` parameter is an
//! untagged actual against a strong formal and amxxpc reports 213 for it. This
//! port reproduces that; see `zero_to_float_param_warns_like_amxxpc`.

use std::collections::HashMap;
use std::path::Path;

use zpc_ast::expr::{BinOp, UnOp};
use zpc_diag::{Diagnostic, Diagnostics, Span};

/// Set on the *value* yielded by `tagof`, never on a tag stored in a symbol.
pub const PUBLIC_TAG: u32 = 0x8000_0000;
/// Marks a "strong" tag - one whose name starts with an uppercase letter.
pub const FIXED_TAG: u32 = 0x4000_0000;
/// Everything except [`PUBLIC_TAG`], matching `TAGMASK` in `sc.h`.
pub const TAG_MASK: u32 = !PUBLIC_TAG;

/// The name of the untagged pseudo-tag, as written in `_:x` and `{Float,_}:`.
pub const UNTAGGED_NAME: &str = "_";

/// An interned tag, stored exactly as upstream stores it (sequence number, with
/// [`FIXED_TAG`] folded in for strong tags).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TagId(u32);

impl TagId {
    /// Tag 0: "no tag". Never appears in the tag-name table upstream either.
    pub const UNTAGGED: TagId = TagId(0);

    /// The raw value, including [`FIXED_TAG`]. This is what upstream writes into
    /// the symbol table and what `tagof` exports (with [`PUBLIC_TAG`] added).
    pub fn raw(self) -> u32 {
        self.0
    }

    /// The sequence number with the flag bits stripped - the identity used by
    /// `tag2str()` when mangling an operator name.
    pub fn sequence(self) -> u32 {
        self.0 & TAG_MASK & !FIXED_TAG
    }

    pub fn is_untagged(self) -> bool {
        self.0 == 0
    }

    /// "Strong": declared with an uppercase initial, so it never coerces.
    pub fn is_strong(self) -> bool {
        self.0 & FIXED_TAG != 0
    }

    /// "Weak": coerces silently to untagged. Untagged itself counts as weak.
    pub fn is_weak(self) -> bool {
        !self.is_strong()
    }

    /// The constant value `tagof` produces for this tag (`tag | PUBLICTAG`).
    /// The *expression* carrying that value is untagged - see [`Tags::tagof_tag`].
    pub fn public_value(self) -> u32 {
        self.0 | PUBLIC_TAG
    }
}

/// Whether the check site permits weak coercion, i.e. the `allowcoerce`
/// parameter of `matchtag()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coercion {
    /// `TRUE` upstream: assignments, arguments, returns, subscripts, index tags.
    Allow,
    /// `FALSE` upstream: binary operators, `?:` arms, `const` declarations,
    /// literal-array elements.
    Deny,
}

/// The tag-name table: `tagname_tab` plus `pc_addtag()`.
///
/// Tag numbering is assignment-order dependent upstream too, so the concrete
/// numbers here are only guaranteed to be *consistent*, never to equal the ones
/// a given amxxpc run would pick. Nothing in the checking logic depends on the
/// numbers - only on identity, on [`FIXED_TAG`], and on which tag is `any`.
#[derive(Clone, Debug)]
pub struct Tags {
    /// Sequence number (index) -> name. Slot 0 is the untagged pseudo-tag.
    names: Vec<String>,
    by_name: HashMap<String, TagId>,
    any: TagId,
    bool_tag: TagId,
    float: TagId,
    string: TagId,
}

impl Default for Tags {
    fn default() -> Self {
        Self::new()
    }
}

impl Tags {
    /// A table pre-loaded with the tags AMX Mod X always has: `bool`, `Float`,
    /// `String` and `any`.
    ///
    /// Upstream only guarantees `any` (registered by `setconstants()`); the
    /// other three are created on first mention, from `core.inc`/`float.inc` and
    /// from `#pragma rational Float`. Registering them eagerly changes nothing
    /// observable because tag identity is by name.
    pub fn new() -> Self {
        let mut t = Tags {
            names: vec![UNTAGGED_NAME.to_string()],
            by_name: HashMap::new(),
            any: TagId::UNTAGGED,
            bool_tag: TagId::UNTAGGED,
            float: TagId::UNTAGGED,
            string: TagId::UNTAGGED,
        };
        t.by_name.insert(UNTAGGED_NAME.to_string(), TagId::UNTAGGED);
        t.bool_tag = t.add("bool");
        t.float = t.add("Float");
        t.string = t.add("String");
        t.any = t.add("any");
        t
    }

    /// `pc_addtag()`: return the existing tag with this name, or intern a new
    /// one, setting [`FIXED_TAG`] when the name begins with an uppercase letter.
    ///
    /// An empty name and `"_"` both mean untagged, which is how `_:x` and the
    /// absent tag of `new x` reach here.
    pub fn add(&mut self, name: &str) -> TagId {
        if name.is_empty() || name == UNTAGGED_NAME {
            return TagId::UNTAGGED;
        }
        if let Some(&id) = self.by_name.get(name) {
            return id;
        }
        let seq = self.names.len() as u32;
        debug_assert!(seq & (FIXED_TAG | PUBLIC_TAG) == 0, "tag table overflow");
        let strong = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        let id = TagId(if strong { seq | FIXED_TAG } else { seq });
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), id);
        id
    }

    /// Look a tag up without interning it.
    pub fn find(&self, name: &str) -> Option<TagId> {
        if name.is_empty() || name == UNTAGGED_NAME {
            return Some(TagId::UNTAGGED);
        }
        self.by_name.get(name).copied()
    }

    /// The name a tag was interned under. `_` for untagged.
    pub fn name(&self, tag: TagId) -> &str {
        &self.names[tag.sequence() as usize]
    }

    /// Intern a whole multi-tag list, as written `{Float,_}:`. An empty list -
    /// the way [`zpc_sema::ParamSig`](crate::ParamSig) spells "untagged" - yields
    /// a single untagged entry, matching `numtags == 1, tags[0] == 0` upstream.
    pub fn add_all(&mut self, names: &[String]) -> Vec<TagId> {
        if names.is_empty() {
            return vec![TagId::UNTAGGED];
        }
        names.iter().map(|n| self.add(n)).collect()
    }

    pub fn any(&self) -> TagId {
        self.any
    }

    /// The tag of `a == b`, `!x`, `a && b` and every other boolean-valued form
    /// (`plnge(..., "bool", ...)` and `skim()`).
    pub fn bool_tag(&self) -> TagId {
        self.bool_tag
    }

    /// `sc_rationaltag`: the tag of a rational literal such as `1.5`, set by
    /// `#pragma rational Float` in AMX Mod X's `float.inc`.
    pub fn rational_tag(&self) -> TagId {
        self.float
    }

    pub fn string_tag(&self) -> TagId {
        self.string
    }

    /// The tag of an integer, character or string literal, of `sizeof`, of
    /// `defined`, and of `tagof`: all untagged.
    ///
    /// `tagof` is worth spelling out: it yields a *constant* equal to
    /// [`TagId::public_value`], but the expression itself carries no tag, so
    /// `new t = tagof(Float:)` does not warn.
    pub fn tagof_tag(&self) -> TagId {
        TagId::UNTAGGED
    }

    /// `matchtag()`, ported verbatim.
    pub fn matches(&self, formal: TagId, actual: TagId, coerce: Coercion) -> bool {
        // Upstream nests these; collapsed here only because clippy insists. The
        // outer condition is "the tags are not trivially compatible", the inner
        // one is "and coercion cannot rescue them".
        if formal != actual
            && formal != self.any
            && actual != self.any
            && (coerce == Coercion::Deny || !formal.is_untagged() || actual.is_strong())
        {
            return false;
        }
        true
    }

    /// `checktag()`: true when *any* tag of a multi-tag formal matches. Always
    /// called with `allowcoerce == TRUE` upstream.
    pub fn matches_any(&self, formals: &[TagId], actual: TagId) -> bool {
        debug_assert!(!formals.is_empty(), "a formal always has at least one tag");
        formals.iter().any(|&f| self.matches(f, actual, Coercion::Allow))
    }
}

/// Which operator an overload redefines.
///
/// The set is exactly the one `check_userop()` can name: `binoperstr` has empty
/// strings for the shifts and the bitwise operators, so those are **not**
/// overloadable, and `&&`/`||` never reach `check_userop()` at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum OpKind {
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Le,
    Ge,
    Lt,
    Gt,
    Eq,
    Ne,
    /// `operator=` - the coercion hook, consulted on assignment, on variable
    /// initialisation and on by-value argument passing.
    Assign,
    /// Unary `!`.
    LogNot,
    /// Unary `-`.
    Neg,
    /// `++`.
    Inc,
    /// `--`.
    Dec,
}

impl OpKind {
    /// The spelling used to build the mangled symbol name.
    pub fn spelling(self) -> &'static str {
        match self {
            OpKind::Mul => "*",
            OpKind::Div => "/",
            OpKind::Mod => "%",
            OpKind::Add => "+",
            OpKind::Sub => "-",
            OpKind::Le => "<=",
            OpKind::Ge => ">=",
            OpKind::Lt => "<",
            OpKind::Gt => ">",
            OpKind::Eq => "==",
            OpKind::Ne => "!=",
            OpKind::Assign => "=",
            OpKind::LogNot => "!",
            OpKind::Neg => "-",
            OpKind::Inc => "++",
            OpKind::Dec => "--",
        }
    }

    fn is_unary(self) -> bool {
        matches!(self, OpKind::LogNot | OpKind::Neg | OpKind::Inc | OpKind::Dec)
    }

    /// `commutative()` in `sc3.c`, restricted to the overloadable operators.
    /// When an exact overload is missing, a commutative operator retries with
    /// the operand tags swapped.
    fn is_commutative(self) -> bool {
        matches!(self, OpKind::Add | OpKind::Mul | OpKind::Eq | OpKind::Ne)
    }

    /// The overload that a binary AST operator would look for, if any.
    pub fn from_binop(op: BinOp) -> Option<OpKind> {
        Some(match op {
            BinOp::Mul => OpKind::Mul,
            BinOp::Div => OpKind::Div,
            BinOp::Mod => OpKind::Mod,
            BinOp::Add => OpKind::Add,
            BinOp::Sub => OpKind::Sub,
            BinOp::Le => OpKind::Le,
            BinOp::Ge => OpKind::Ge,
            BinOp::Lt => OpKind::Lt,
            BinOp::Gt => OpKind::Gt,
            BinOp::Eq => OpKind::Eq,
            BinOp::Ne => OpKind::Ne,
            // Shifts and bitwise operators have an empty entry in `binoperstr`;
            // `&&` and `||` are handled by `skim()` and never consult an
            // overload.
            BinOp::Shl
            | BinOp::Shr
            | BinOp::ShrU
            | BinOp::BitAnd
            | BinOp::BitXor
            | BinOp::BitOr
            | BinOp::LogAnd
            | BinOp::LogOr => return None,
        })
    }

    /// The overload a prefix operator would look for. `~` is listed in
    /// `operator_symname()` but has no entry in `unoperstr`, so `check_userop()`
    /// can never name it and it is not overloadable in practice.
    pub fn from_unop(op: UnOp) -> Option<OpKind> {
        match op {
            UnOp::Neg => Some(OpKind::Neg),
            UnOp::LogNot => Some(OpKind::LogNot),
            UnOp::BitNot => None,
        }
    }
}

/// One declared `operator` function.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Overload {
    pub kind: OpKind,
    /// First operand tag; for [`OpKind::Assign`] this is the **destination**.
    pub lhs: TagId,
    /// Second operand tag; `None` for a unary operator. For
    /// [`OpKind::Assign`] this is the **source**.
    pub rhs: Option<TagId>,
    /// Tag of the operator's return value, which becomes the tag of the whole
    /// expression.
    pub result: TagId,
}

/// The set of declared operator overloads, keyed the way `findglb()` keys them:
/// by the mangled name `operator_symname()` builds.
#[derive(Clone, Debug, Default)]
pub struct Overloads {
    by_symbol: HashMap<String, Overload>,
}

impl Overloads {
    pub fn new() -> Self {
        Self::default()
    }

    /// `operator_symname()`: `<tag1><op><tag2>` for a binary operator,
    /// `<op><tag1>` for a unary one, and `<result>=<source>` for `operator=`.
    /// Tags are rendered by `tag2str()`, i.e. hex with a leading `0` that is
    /// dropped again when the first hex digit is itself a digit.
    pub fn mangle(kind: OpKind, lhs: TagId, rhs: Option<TagId>, result: TagId) -> String {
        let op = kind.spelling();
        match kind {
            OpKind::Assign => format!("{}={}", tag2str(result), tag2str(lhs)),
            _ if kind.is_unary() || rhs.is_none() => format!("{}{}", op, tag2str(lhs)),
            _ => format!("{}{}{}", tag2str(lhs), op, tag2str(rhs.unwrap())),
        }
    }

    fn key(o: &Overload) -> String {
        // `operator_symname()` is called with `resulttag = tag2`, so for `=` the
        // name is built from the destination tag on both the "result" and the
        // "tag2" slot; passing `o.lhs` (the destination) as the result keeps the
        // lookup below symmetric with declaration.
        let result = if o.kind == OpKind::Assign { o.lhs } else { o.result };
        Self::mangle(o.kind, o.lhs, o.rhs, result)
    }

    pub fn declare(&mut self, o: Overload) {
        self.by_symbol.insert(Self::key(&o), o);
    }

    /// `check_userop()`'s resolution, tag logic only (no code generation).
    ///
    /// 1. Untagged operands never select an overload: `if (tag1 == 0 &&
    ///    (numparam == 1 || tag2 == 0)) return FALSE;`.
    /// 2. Look up the exact mangled name.
    /// 3. On a miss, if the operator is binary, commutative, and the two tags
    ///    differ, retry with them swapped.
    ///
    /// Returns the overload's result tag, which becomes the expression's tag and
    /// **suppresses** the 213 that the site would otherwise emit. `None` means
    /// no overload applies and the normal `matchtag()` check runs.
    ///
    /// Not modelled here (they belong to the symbol pass, not the tag pass):
    /// error 4 / 71 when the named operator exists but is undefined or
    /// unprototyped, and the `sym == curfunc` guard that stops an operator body
    /// from recursing into itself.
    pub fn find(&self, kind: OpKind, lhs: TagId, rhs: Option<TagId>) -> Option<TagId> {
        let unary = kind.is_unary() || rhs.is_none();
        if lhs.is_untagged() && (unary || rhs.is_some_and(TagId::is_untagged)) {
            return None;
        }
        let direct = if kind == OpKind::Assign {
            Self::mangle(kind, lhs, rhs, lhs)
        } else {
            Self::mangle(kind, lhs, rhs, TagId::UNTAGGED)
        };
        if let Some(o) = self.by_symbol.get(&direct) {
            return Some(o.result);
        }
        // Commutative retry. `oper == NULL` (i.e. `=`) blocks the swap upstream.
        if let Some(r) = rhs
            && !unary
            && kind != OpKind::Assign
            && kind.is_commutative()
            && lhs != r
            && let Some(o) = self.by_symbol.get(&Self::mangle(kind, r, Some(lhs), TagId::UNTAGGED))
        {
            return Some(o.result);
        }
        None
    }
}

/// `tag2str()`: `sprintf(dest, "0%x", tag & TAGMASK)`, then drop the leading
/// zero when the next character is a digit.
fn tag2str(tag: TagId) -> String {
    let s = format!("0{:x}", tag.raw() & TAG_MASK);
    match s.as_bytes()[1].is_ascii_digit() {
        true => s[1..].to_string(),
        false => s,
    }
}

/// Every place the compiler compares two tags and may warn.
///
/// Each method mirrors one call site in the C source and is named after it; the
/// doc comment gives the file and the surrounding function so the port can be
/// re-checked. Methods that can emit take `&mut Diagnostics` rather than owning
/// one, so tag checking composes with the rest of the pass.
pub struct TagCheck<'a> {
    pub tags: &'a Tags,
    pub ops: &'a Overloads,
    pub file: &'a Path,
}

impl<'a> TagCheck<'a> {
    pub fn new(tags: &'a Tags, ops: &'a Overloads, file: &'a Path) -> Self {
        Self { tags, ops, file }
    }

    fn warn_213(&self, diags: &mut Diagnostics, span: Span) {
        diags.push(Diagnostic::new(213, span, self.file, &[]));
    }

    fn warn_229(&self, diags: &mut Diagnostics, span: Span, symbol: &str) {
        diags.push(Diagnostic::new(229, span, self.file, &[symbol]));
    }

    /// `plnge2()` in `sc3.c`: a binary operator.
    ///
    /// An overload wins outright; otherwise the operands must match with
    /// `allowcoerce == FALSE`, so even a weak tag against untagged warns here.
    /// `&&` and `||` are handled by `skim()`, which compares nothing and forces
    /// the result to `bool:`.
    ///
    /// Returns the tag of the resulting expression: the overload's return tag,
    /// `bool:` for a comparison (`plnge()` is called with `forcetag = "bool"`),
    /// and otherwise the left operand's tag.
    pub fn binary(
        &self,
        op: BinOp,
        lhs: TagId,
        rhs: TagId,
        span: Span,
        diags: &mut Diagnostics,
    ) -> TagId {
        if matches!(op, BinOp::LogAnd | BinOp::LogOr) {
            return self.tags.bool_tag();
        }
        if let Some(kind) = OpKind::from_binop(op)
            && let Some(result) = self.ops.find(kind, lhs, Some(rhs))
        {
            // `plnge()` still overwrites the tag with `bool:` for the relational
            // and equality levels, after `check_userop()` has run.
            return if op.is_boolean() { self.tags.bool_tag() } else { result };
        }
        if !self.tags.matches(lhs, rhs, Coercion::Deny) {
            self.warn_213(diags, span);
        }
        if op.is_boolean() { self.tags.bool_tag() } else { lhs }
    }

    /// `hier2()` in `sc3.c`: a prefix operator. `!` always yields `bool:`;
    /// `-` and `~` keep the operand's tag unless an overload replaces it.
    /// No tag check, so this never warns.
    pub fn unary(&self, op: UnOp, operand: TagId) -> TagId {
        if let Some(kind) = OpKind::from_unop(op)
            && let Some(result) = self.ops.find(kind, operand, None)
        {
            return if op == UnOp::LogNot { self.tags.bool_tag() } else { result };
        }
        match op {
            UnOp::LogNot => self.tags.bool_tag(),
            UnOp::Neg | UnOp::BitNot => operand,
        }
    }

    /// `hier14()` in `sc3.c`: a scalar assignment `dest = value`.
    ///
    /// Faithful to the AMX Mod X 1.60 shape, which is *not* the stock Pawn one:
    ///
    /// * a compound assignment (`+=`) checks nothing here - `plnge2()` already
    ///   warned about the operator;
    /// * `operator=` is consulted first and, if found, replaces the value's tag
    ///   so the check below passes;
    /// * when the destination is a *symbol* with a non-zero tag, that symbol tag
    ///   is compared rather than the expression's, so that
    ///   `enum X { A, B }; new Float:arr[X]; arr[A] = 1.0;` checks `Float`;
    /// * the `forceuntag` escape hatch: if the destination expression is tagged,
    ///   the value is untagged, *and* the value was untagged explicitly with
    ///   `_:`, the check is skipped entirely.
    ///
    /// Returns the value tag after any `operator=` coercion.
    #[allow(clippy::too_many_arguments)]
    pub fn assign(
        &self,
        dest: Assignee,
        value: TagId,
        value_forced_untagged: bool,
        compound: bool,
        span: Span,
        diags: &mut Diagnostics,
    ) -> TagId {
        // `check_userop(NULL, lval2.tag, lval3.tag, 2, &lval3, &lval2.tag)`
        let value = self.ops.find(OpKind::Assign, dest.expr, Some(value)).unwrap_or(value);
        if compound {
            return value;
        }
        match dest.symbol {
            Some(sym) if !sym.is_untagged() => {
                if !self.tags.matches(sym, value, Coercion::Allow) {
                    self.warn_213(diags, span);
                }
            }
            _ if !dest.expr.is_untagged() && value.is_untagged() && value_forced_untagged => {}
            _ => {
                if !self.tags.matches(dest.expr, value, Coercion::Allow) {
                    self.warn_213(diags, span);
                }
            }
        }
        value
    }

    /// `initials()`/`declloc()` in `sc1.c`: `new Tag:x = init`.
    ///
    /// Same shape as [`TagCheck::assign`] minus the enum-field special cases:
    /// `operator=` first, then `matchtag(tag, ctag, TRUE)`.
    pub fn initialiser(
        &self,
        declared: TagId,
        value: TagId,
        span: Span,
        diags: &mut Diagnostics,
    ) -> TagId {
        let value = self.ops.find(OpKind::Assign, declared, Some(value)).unwrap_or(value);
        if !self.tags.matches(declared, value, Coercion::Allow) {
            self.warn_213(diags, span);
        }
        value
    }

    /// `decl_const()` in `sc1.c`: `const Tag:NAME = expr`.
    ///
    /// Note the `FALSE`: a `const` declaration does **not** allow weak coercion,
    /// unlike `new`. `const x = bool:1` warns; `new x = bool:1` does not.
    pub fn const_decl(&self, declared: TagId, value: TagId, span: Span, diags: &mut Diagnostics) {
        if !self.tags.matches(declared, value, Coercion::Deny) {
            self.warn_213(diags, span);
        }
    }

    /// `callfunction()` in `sc3.c`: one argument at a call site.
    ///
    /// `by_value` selects the `iVARIABLE` branch, the only one that first runs
    /// `check_userop(NULL, lval.tag, arg->tags[0], 2, ...)` and may rewrite the
    /// argument's tag. The `iREFERENCE`, `iREFARRAY` and `iVARARGS` branches go
    /// straight to `checktag()`.
    ///
    /// `checktag()` accepts if *any* of a multi-tag formal matches, which is how
    /// `{Float,_}:` parameters accept both.
    pub fn argument(
        &self,
        formals: &[TagId],
        actual: TagId,
        by_value: bool,
        span: Span,
        diags: &mut Diagnostics,
    ) -> TagId {
        let actual = if by_value {
            let first = formals.first().copied().unwrap_or(TagId::UNTAGGED);
            self.ops.find(OpKind::Assign, first, Some(actual)).unwrap_or(actual)
        } else {
            actual
        };
        if !self.tags.matches_any(formals, actual) {
            self.warn_213(diags, span);
        }
        actual
    }

    /// `declargs()` in `sc1.c`: a parameter's own default value, checked against
    /// the parameter's first tag with coercion allowed.
    pub fn default_value(
        &self,
        formals: &[TagId],
        default: TagId,
        span: Span,
        diags: &mut Diagnostics,
    ) {
        let first = formals.first().copied().unwrap_or(TagId::UNTAGGED);
        if !self.tags.matches(first, default, Coercion::Allow) {
            self.warn_213(diags, span);
        }
    }

    /// `doreturn()` in `sc1.c`: `return expr` against the function's return tag,
    /// with coercion allowed.
    pub fn return_value(&self, func: TagId, value: TagId, span: Span, diags: &mut Diagnostics) {
        if !self.tags.matches(func, value, Coercion::Allow) {
            self.warn_213(diags, span);
        }
    }

    /// `hier13()` in `sc3.c`: the two arms of `cond ? a : b`, with coercion
    /// **denied**. The result carries the "true" arm's tag.
    pub fn ternary(&self, then_tag: TagId, else_tag: TagId, span: Span, diags: &mut Diagnostics) {
        if !self.tags.matches(then_tag, else_tag, Coercion::Deny) {
            self.warn_213(diags, span);
        }
    }

    /// `constant()` in `sc3.c`: the elements of a literal array `{1, 2, 3}` must
    /// all match the first one, with coercion denied. Returns the array's
    /// element tag (that of the first element).
    pub fn literal_array(&self, elems: &[(TagId, Span)], diags: &mut Diagnostics) -> TagId {
        let Some(&(first, _)) = elems.first() else {
            return TagId::UNTAGGED;
        };
        for &(tag, span) in &elems[1..] {
            if !self.tags.matches(first, tag, Coercion::Deny) {
                self.warn_213(diags, span);
            }
        }
        first
    }

    /// `hier1()` in `sc3.c`: an array subscript, `arr[i]`.
    ///
    /// The array's *index tag* - the tag of the expression that sized the
    /// dimension, e.g. `Player` in `new data[Player]` - is the formal, and the
    /// subscript expression is the actual. Coercion is allowed.
    ///
    /// This site emits **213**, not 229, even though it is about an index tag.
    pub fn subscript(&self, index_tag: TagId, actual: TagId, span: Span, diags: &mut Diagnostics) {
        if !self.tags.matches(index_tag, actual, Coercion::Allow) {
            self.warn_213(diags, span);
        }
    }

    /// The two sites that emit **229**, both comparing one array's declared
    /// index tag against another's rather than against an expression:
    ///
    /// * `hier14()` in `sc3.c` - whole-array assignment, per dimension;
    /// * `callfunction()` in `sc3.c` - passing an array to an array parameter,
    ///   per dimension.
    ///
    /// `symbol` is the name interpolated into the message; upstream uses the
    /// *source* array's name (the destination's, if the source has no symbol).
    pub fn index_tag(
        &self,
        formal: TagId,
        actual: TagId,
        symbol: &str,
        span: Span,
        diags: &mut Diagnostics,
    ) {
        if !self.tags.matches(formal, actual, Coercion::Allow) {
            self.warn_229(diags, span, symbol);
        }
    }
}

/// The destination of an assignment, which upstream inspects on two levels.
///
/// `expr` is the tag of the destination *expression* (`lval3.tag`); `symbol` is
/// the tag of the underlying symbol (`lval3.sym->tag`), present whenever the
/// destination resolves to one. They differ for an enum-indexed pseudo-array,
/// where the expression takes the tag of the enum *field* and the symbol keeps
/// the array's own tag.
#[derive(Clone, Copy, Debug)]
pub struct Assignee {
    pub expr: TagId,
    pub symbol: Option<TagId>,
}

impl Assignee {
    /// A destination with no distinct symbol tag - a plain variable, or any
    /// destination whose symbol is untagged.
    pub fn expr(tag: TagId) -> Self {
        Self { expr: tag, symbol: None }
    }

    /// A destination that resolves to a symbol carrying its own tag.
    pub fn symbol(expr: TagId, symbol: TagId) -> Self {
        Self { expr, symbol: Some(symbol) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sp() -> Span {
        Span::new(0, 1)
    }

    struct Fx {
        tags: Tags,
        ops: Overloads,
        file: PathBuf,
    }

    impl Fx {
        fn new() -> Self {
            Fx { tags: Tags::new(), ops: Overloads::new(), file: PathBuf::from("plugin.sma") }
        }

        fn check(&self) -> TagCheck<'_> {
            TagCheck::new(&self.tags, &self.ops, &self.file)
        }

        /// Runs `f` and returns the diagnostic codes it produced.
        fn codes(&self, f: impl FnOnce(&TagCheck<'_>, &mut Diagnostics)) -> Vec<u16> {
            let mut d = Diagnostics::new();
            f(&self.check(), &mut d);
            d.items().iter().map(|x| x.code).collect()
        }
    }

    // ---- the registry -----------------------------------------------------

    #[test]
    fn untagged_is_tag_zero_under_every_spelling() {
        let mut t = Tags::new();
        assert!(t.add("").is_untagged());
        assert!(t.add("_").is_untagged());
        assert_eq!(t.find("_"), Some(TagId::UNTAGGED));
        assert_eq!(t.name(TagId::UNTAGGED), "_");
    }

    #[test]
    fn interning_is_stable_and_by_name() {
        let mut t = Tags::new();
        let a = t.add("Weapon");
        let b = t.add("Weapon");
        assert_eq!(a, b);
        assert_ne!(a, t.add("Player"));
        assert_eq!(t.name(a), "Weapon");
        assert_eq!(t.find("Nope"), None);
    }

    #[test]
    fn uppercase_initial_makes_a_strong_tag() {
        let mut t = Tags::new();
        let strong = t.add("Player");
        assert!(strong.is_strong() && !strong.is_weak());
        assert_ne!(strong.raw() & FIXED_TAG, 0);
    }

    #[test]
    fn lowercase_initial_makes_a_weak_tag() {
        let mut t = Tags::new();
        let weak = t.add("bits");
        assert!(weak.is_weak() && !weak.is_strong());
        assert_eq!(weak.raw() & FIXED_TAG, 0);
    }

    #[test]
    fn predefined_tags_have_the_expected_strength() {
        let t = Tags::new();
        // `Float` and `String` are uppercase, so strong: this is why passing a
        // float where an untagged cell is expected warns.
        assert!(t.rational_tag().is_strong());
        assert!(t.string_tag().is_strong());
        // `bool` and `any` are lowercase, so weak.
        assert!(t.bool_tag().is_weak());
        assert!(t.any().is_weak());
    }

    #[test]
    fn a_leading_non_letter_is_not_uppercase_so_the_tag_is_weak() {
        // `isupper()` is false for `_` and for digits, so such a tag is weak.
        let mut t = Tags::new();
        assert!(t.add("_leading").is_weak());
    }

    #[test]
    fn tagof_value_carries_the_public_bit_but_the_expression_does_not() {
        let t = Tags::new();
        assert_ne!(t.rational_tag().public_value() & PUBLIC_TAG, 0);
        assert!(t.tagof_tag().is_untagged());
    }

    #[test]
    fn multi_tag_lists_intern_and_empty_means_untagged() {
        let mut t = Tags::new();
        let list = t.add_all(&["Float".into(), "_".into()]);
        assert_eq!(list, vec![t.rational_tag(), TagId::UNTAGGED]);
        assert_eq!(t.add_all(&[]), vec![TagId::UNTAGGED]);
    }

    // ---- matchtag ---------------------------------------------------------

    #[test]
    fn identical_tags_always_match() {
        let t = Tags::new();
        for c in [Coercion::Allow, Coercion::Deny] {
            assert!(t.matches(t.rational_tag(), t.rational_tag(), c));
            assert!(t.matches(TagId::UNTAGGED, TagId::UNTAGGED, c));
        }
    }

    #[test]
    fn different_strong_tags_never_match() {
        let mut t = Tags::new();
        let player = t.add("Player");
        for c in [Coercion::Allow, Coercion::Deny] {
            assert!(!t.matches(player, t.rational_tag(), c));
            assert!(!t.matches(t.rational_tag(), player, c));
        }
    }

    #[test]
    fn a_weak_tag_coerces_to_untagged_when_coercion_is_allowed() {
        let t = Tags::new();
        assert!(t.matches(TagId::UNTAGGED, t.bool_tag(), Coercion::Allow));
        // ...and not when it is denied, which is why `a + bool:b` warns.
        assert!(!t.matches(TagId::UNTAGGED, t.bool_tag(), Coercion::Deny));
    }

    #[test]
    fn a_strong_tag_never_coerces_to_untagged() {
        let t = Tags::new();
        assert!(!t.matches(TagId::UNTAGGED, t.rational_tag(), Coercion::Allow));
        assert!(!t.matches(TagId::UNTAGGED, t.rational_tag(), Coercion::Deny));
    }

    #[test]
    fn coercion_is_one_directional_untagged_never_fits_a_tagged_formal() {
        let t = Tags::new();
        // The asymmetry: weak `bool:` -> untagged is fine, untagged -> `bool:`
        // is not, because the coercion arm requires `formaltag == 0`.
        assert!(t.matches(TagId::UNTAGGED, t.bool_tag(), Coercion::Allow));
        assert!(!t.matches(t.bool_tag(), TagId::UNTAGGED, Coercion::Allow));
    }

    #[test]
    fn any_matches_everything_in_both_directions_even_without_coercion() {
        let mut t = Tags::new();
        let player = t.add("Player");
        for c in [Coercion::Allow, Coercion::Deny] {
            assert!(t.matches(t.any(), player, c));
            assert!(t.matches(player, t.any(), c));
            assert!(t.matches(t.any(), TagId::UNTAGGED, c));
            assert!(t.matches(TagId::UNTAGGED, t.any(), c));
        }
    }

    #[test]
    fn multi_tag_formal_accepts_any_listed_tag_and_rejects_others() {
        let mut t = Tags::new();
        let player = t.add("Player");
        let formals = vec![t.rational_tag(), TagId::UNTAGGED];
        assert!(t.matches_any(&formals, t.rational_tag()));
        assert!(t.matches_any(&formals, TagId::UNTAGGED));
        // a weak tag reaches the untagged alternative by coercion
        assert!(t.matches_any(&formals, t.bool_tag()));
        // an unrelated strong tag matches neither alternative
        assert!(!t.matches_any(&formals, player));
    }

    // ---- warning 213 sites ------------------------------------------------

    #[test]
    fn binary_operator_warns_on_mixed_tags_and_is_quiet_on_equal_ones() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        assert_eq!(
            f.codes(|c, d| {
                c.binary(BinOp::Add, float, TagId::UNTAGGED, sp(), d);
            }),
            vec![213]
        );
        assert!(f.codes(|c, d| {
            c.binary(BinOp::Add, float, float, sp(), d);
        })
        .is_empty());
    }

    #[test]
    fn binary_operator_denies_weak_coercion() {
        // `plnge2()` passes FALSE, so even `bool:` against untagged warns.
        let f = Fx::new();
        assert_eq!(
            f.codes(|c, d| {
                c.binary(BinOp::Add, TagId::UNTAGGED, f.tags.bool_tag(), sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn logical_operators_never_compare_tags_and_yield_bool() {
        let f = Fx::new();
        let mut d = Diagnostics::new();
        let r = f.check().binary(BinOp::LogAnd, f.tags.rational_tag(), TagId::UNTAGGED, sp(), &mut d);
        assert_eq!(r, f.tags.bool_tag());
        assert_eq!(d.items().len(), 0);
    }

    #[test]
    fn comparison_yields_bool_while_arithmetic_yields_the_left_tag() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        let mut d = Diagnostics::new();
        assert_eq!(f.check().binary(BinOp::Lt, float, float, sp(), &mut d), f.tags.bool_tag());
        assert_eq!(f.check().binary(BinOp::Add, float, float, sp(), &mut d), float);
        assert_eq!(d.items().len(), 0);
    }

    #[test]
    fn assignment_allows_weak_coercion_but_not_strong() {
        let f = Fx::new();
        // untagged = bool:1  -> silent
        assert!(f.codes(|c, d| {
            c.assign(Assignee::expr(TagId::UNTAGGED), f.tags.bool_tag(), false, false, sp(), d);
        })
        .is_empty());
        // untagged = 1.0  -> warns, `Float` is strong
        assert_eq!(
            f.codes(|c, d| {
                c.assign(
                    Assignee::expr(TagId::UNTAGGED),
                    f.tags.rational_tag(),
                    false,
                    false,
                    sp(),
                    d,
                );
            }),
            vec![213]
        );
    }

    #[test]
    fn compound_assignment_does_not_re_check_the_tags() {
        // `if (!oper)` upstream: `plnge2()` already reported it.
        let f = Fx::new();
        assert!(f.codes(|c, d| {
            c.assign(Assignee::expr(TagId::UNTAGGED), f.tags.rational_tag(), false, true, sp(), d);
        })
        .is_empty());
    }

    #[test]
    fn assignment_prefers_the_destination_symbol_tag_when_it_has_one() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        // `new Float:arr[X]; arr[A] = 1.0;` - the expression tag is untagged
        // (enum field `A`) but the symbol is `Float:`, so this must be silent.
        assert!(f.codes(|c, d| {
            c.assign(Assignee::symbol(TagId::UNTAGGED, float), float, false, false, sp(), d);
        })
        .is_empty());
        // and the symbol tag is what warns when it does not match
        assert_eq!(
            f.codes(|c, d| {
                c.assign(
                    Assignee::symbol(float, float),
                    TagId::UNTAGGED,
                    false,
                    false,
                    sp(),
                    d,
                );
            }),
            vec![213]
        );
    }

    #[test]
    fn explicitly_untagging_the_value_suppresses_the_assignment_check() {
        // `enum X { Float:A }; new arr[X]; arr[A] = _:1.0;`
        let f = Fx::new();
        let float = f.tags.rational_tag();
        assert!(f.codes(|c, d| {
            c.assign(Assignee::expr(float), TagId::UNTAGGED, true, false, sp(), d);
        })
        .is_empty());
        // without the explicit `_:` the same shape warns
        assert_eq!(
            f.codes(|c, d| {
                c.assign(Assignee::expr(float), TagId::UNTAGGED, false, false, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn const_declaration_denies_the_coercion_that_new_allows() {
        let f = Fx::new();
        let b = f.tags.bool_tag();
        // `new x = bool:1` -> silent
        assert!(f.codes(|c, d| {
            c.initialiser(TagId::UNTAGGED, b, sp(), d);
        })
        .is_empty());
        // `const x = bool:1` -> warns
        assert_eq!(
            f.codes(|c, d| c.const_decl(TagId::UNTAGGED, b, sp(), d)),
            vec![213]
        );
    }

    #[test]
    fn return_value_is_checked_against_the_function_tag() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        assert_eq!(
            f.codes(|c, d| c.return_value(float, TagId::UNTAGGED, sp(), d)),
            vec![213]
        );
        assert!(f.codes(|c, d| c.return_value(float, float, sp(), d)).is_empty());
        // an untagged function accepts a weak tag back
        assert!(
            f.codes(|c, d| c.return_value(TagId::UNTAGGED, f.tags.bool_tag(), sp(), d)).is_empty()
        );
    }

    #[test]
    fn ternary_arms_must_match_without_coercion() {
        let f = Fx::new();
        assert_eq!(
            f.codes(|c, d| c.ternary(TagId::UNTAGGED, f.tags.bool_tag(), sp(), d)),
            vec![213]
        );
        assert!(
            f.codes(|c, d| c.ternary(f.tags.bool_tag(), f.tags.bool_tag(), sp(), d)).is_empty()
        );
    }

    #[test]
    fn literal_array_elements_must_share_the_first_tag() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        let mut d = Diagnostics::new();
        let tag = f.check().literal_array(&[(float, sp()), (float, sp()), (TagId::UNTAGGED, sp())], &mut d);
        assert_eq!(tag, float);
        assert_eq!(d.items().iter().map(|x| x.code).collect::<Vec<_>>(), vec![213]);
        // all-equal is silent, and an empty literal is untagged
        assert!(f.codes(|c, d| {
            c.literal_array(&[(float, sp()), (float, sp())], d);
        })
        .is_empty());
        assert!(f.check().literal_array(&[], &mut d).is_untagged());
    }

    #[test]
    fn subscript_index_tag_mismatch_is_213_not_229() {
        let mut t = Tags::new();
        let player = t.add("Player");
        let f = Fx { tags: t, ops: Overloads::new(), file: PathBuf::from("plugin.sma") };
        // `new data[Player]; data[3]` - untagged index against `Player:`
        assert_eq!(f.codes(|c, d| c.subscript(player, TagId::UNTAGGED, sp(), d)), vec![213]);
        assert!(f.codes(|c, d| c.subscript(player, player, sp(), d)).is_empty());
        // an untagged dimension accepts anything weak
        assert!(
            f.codes(|c, d| c.subscript(TagId::UNTAGGED, f.tags.bool_tag(), sp(), d)).is_empty()
        );
    }

    // ---- warning 229 ------------------------------------------------------

    #[test]
    fn array_index_tag_mismatch_is_229_and_names_the_symbol() {
        let mut t = Tags::new();
        let player = t.add("Player");
        let f = Fx { tags: t, ops: Overloads::new(), file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        f.check().index_tag(player, TagId::UNTAGGED, "scores", sp(), &mut d);
        assert_eq!(d.items().len(), 1);
        assert_eq!(d.items()[0].code, 229);
        assert_eq!(d.items()[0].message, "index tag mismatch (symbol \"scores\")");
    }

    #[test]
    fn matching_index_tags_are_silent() {
        let mut t = Tags::new();
        let player = t.add("Player");
        let f = Fx { tags: t, ops: Overloads::new(), file: PathBuf::from("plugin.sma") };
        assert!(f.codes(|c, d| c.index_tag(player, player, "scores", sp(), d)).is_empty());
        // coercion is allowed here, so a weak index tag fits an untagged formal
        assert!(
            f.codes(|c, d| c.index_tag(TagId::UNTAGGED, f.tags.bool_tag(), "s", sp(), d))
                .is_empty()
        );
    }

    // ---- arguments, with the real-world AMX Mod X cases -------------------

    #[test]
    fn set_task_with_an_integer_delay_warns_213() {
        // native set_task(Float:time, const function[], ...)
        let f = Fx::new();
        let formals = vec![f.tags.rational_tag()];
        assert_eq!(
            f.codes(|c, d| {
                c.argument(&formals, TagId::UNTAGGED, true, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn set_task_with_a_float_delay_is_silent() {
        let f = Fx::new();
        let formals = vec![f.tags.rational_tag()];
        assert!(f.codes(|c, d| {
            c.argument(&formals, f.tags.rational_tag(), true, sp(), d);
        })
        .is_empty());
    }

    #[test]
    fn set_user_health_with_a_float_warns_213() {
        // native set_user_health(index, health) - both parameters untagged, so
        // the strong `Float:` of `100.0` cannot coerce down to untagged.
        let f = Fx::new();
        let formals = vec![TagId::UNTAGGED];
        assert_eq!(
            f.codes(|c, d| {
                c.argument(&formals, f.tags.rational_tag(), true, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn zero_to_float_param_warns_like_amxxpc() {
        // There is NO exemption for the literal `0` in `matchtag()`,
        // `checktag()` or `callfunction()`: the bit pattern happening to be
        // `0.0` is irrelevant, only the tag is compared. amxxpc reports 213 for
        // `set_task(0, ...)` and so does this port.
        let f = Fx::new();
        let formals = vec![f.tags.rational_tag()];
        assert_eq!(
            f.codes(|c, d| {
                c.argument(&formals, TagId::UNTAGGED, true, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn a_bool_argument_reaches_an_untagged_parameter() {
        let f = Fx::new();
        let formals = vec![TagId::UNTAGGED];
        assert!(f.codes(|c, d| {
            c.argument(&formals, f.tags.bool_tag(), true, sp(), d);
        })
        .is_empty());
    }

    #[test]
    fn multi_tag_parameter_accepts_either_alternative() {
        // `stock f({Float,_}:value)`
        let mut t = Tags::new();
        let formals = t.add_all(&["Float".into(), "_".into()]);
        let player = t.add("Player");
        let f = Fx { tags: t, ops: Overloads::new(), file: PathBuf::from("plugin.sma") };
        for actual in [f.tags.rational_tag(), TagId::UNTAGGED, f.tags.bool_tag()] {
            assert!(
                f.codes(|c, d| {
                    c.argument(&formals, actual, true, sp(), d);
                })
                .is_empty(),
                "{} should be accepted",
                f.tags.name(actual)
            );
        }
        assert_eq!(
            f.codes(|c, d| {
                c.argument(&formals, player, true, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn an_any_parameter_swallows_every_tag() {
        let mut t = Tags::new();
        let player = t.add("Player");
        let f = Fx { tags: t, ops: Overloads::new(), file: PathBuf::from("plugin.sma") };
        let formals = vec![f.tags.any()];
        for actual in [player, f.tags.rational_tag(), TagId::UNTAGGED] {
            assert!(f.codes(|c, d| {
                c.argument(&formals, actual, true, sp(), d);
            })
            .is_empty());
        }
    }

    #[test]
    fn by_reference_arguments_skip_the_assign_overload_but_still_check() {
        let mut t = Tags::new();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        // `Float:operator=(Cash:)` would rescue a by-value pass...
        ops.declare(Overload {
            kind: OpKind::Assign,
            lhs: t.rational_tag(),
            rhs: Some(cash),
            result: t.rational_tag(),
        });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        let formals = vec![f.tags.rational_tag()];
        assert!(f.codes(|c, d| {
            c.argument(&formals, cash, true, sp(), d);
        })
        .is_empty());
        // ...but `&Float:out` takes the `iREFERENCE` branch, which never calls
        // `check_userop()`.
        assert_eq!(
            f.codes(|c, d| {
                c.argument(&formals, cash, false, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn default_argument_value_is_checked_against_the_first_tag() {
        let f = Fx::new();
        let formals = vec![f.tags.rational_tag()];
        assert_eq!(
            f.codes(|c, d| c.default_value(&formals, TagId::UNTAGGED, sp(), d)),
            vec![213]
        );
        assert!(
            f.codes(|c, d| c.default_value(&formals, f.tags.rational_tag(), sp(), d)).is_empty()
        );
    }

    // ---- operator overloads -----------------------------------------------

    #[test]
    fn mangling_matches_operator_symname() {
        // tag2str: "0%x", leading zero dropped when the next char is a digit.
        // Untagged (0) therefore renders as "0"; a strong tag keeps FIXEDTAG in
        // its hex form, so it starts with '4' and also loses the leading zero.
        assert_eq!(tag2str(TagId::UNTAGGED), "0");
        assert_eq!(tag2str(TagId(1)), "1");
        assert_eq!(tag2str(TagId(0xa)), "0a");
        assert_eq!(tag2str(TagId(FIXED_TAG | 2)), "40000002");

        let mut t = Tags::new();
        let float = t.rational_tag();
        assert_eq!(
            Overloads::mangle(OpKind::Add, float, Some(float), TagId::UNTAGGED),
            format!("{}+{}", tag2str(float), tag2str(float))
        );
        assert_eq!(
            Overloads::mangle(OpKind::Neg, float, None, TagId::UNTAGGED),
            format!("-{}", tag2str(float))
        );
        // `=` mangles as <result><op><source>
        let cash = t.add("Cash");
        assert_eq!(
            Overloads::mangle(OpKind::Assign, float, Some(cash), float),
            format!("{}={}", tag2str(float), tag2str(float))
        );
    }

    #[test]
    fn an_exact_overload_is_selected_and_suppresses_213() {
        let f0 = Fx::new();
        let float = f0.tags.rational_tag();
        let mut ops = Overloads::new();
        ops.declare(Overload {
            kind: OpKind::Add,
            lhs: float,
            rhs: Some(float),
            result: float,
        });
        let f = Fx { tags: f0.tags, ops, file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        // `Float:operator+(Float:, Float:)` - matching tags anyway, but the
        // overload's return tag is what the expression carries.
        assert_eq!(f.check().binary(BinOp::Add, float, float, sp(), &mut d), float);
        assert!(d.items().is_empty());
    }

    #[test]
    fn a_mixed_tag_overload_suppresses_the_warning_that_would_otherwise_fire() {
        let mut t = Tags::new();
        let float = t.rational_tag();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        ops.declare(Overload {
            kind: OpKind::Add,
            lhs: cash,
            rhs: Some(float),
            result: cash,
        });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        assert_eq!(f.check().binary(BinOp::Add, cash, float, sp(), &mut d), cash);
        assert!(d.items().is_empty());
    }

    #[test]
    fn a_commutative_operator_retries_with_swapped_tags() {
        let mut t = Tags::new();
        let float = t.rational_tag();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        ops.declare(Overload { kind: OpKind::Add, lhs: cash, rhs: Some(float), result: cash });
        // `-` is not commutative, so the mirrored form must not be found.
        ops.declare(Overload { kind: OpKind::Sub, lhs: cash, rhs: Some(float), result: cash });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        assert_eq!(f.ops.find(OpKind::Add, float, Some(cash)), Some(cash));
        assert_eq!(f.ops.find(OpKind::Sub, float, Some(cash)), None);
        assert_eq!(
            f.codes(|c, d| {
                c.binary(BinOp::Sub, float, cash, sp(), d);
            }),
            vec![213]
        );
    }

    #[test]
    fn no_matching_overload_falls_back_to_the_plain_tag_check() {
        let mut t = Tags::new();
        let float = t.rational_tag();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        // only `Cash + Cash` exists
        ops.declare(Overload { kind: OpKind::Add, lhs: cash, rhs: Some(cash), result: cash });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        let r = f.check().binary(BinOp::Add, cash, float, sp(), &mut d);
        assert_eq!(d.items().iter().map(|x| x.code).collect::<Vec<_>>(), vec![213]);
        // the expression keeps the left operand's tag, as `plnge2()` leaves it
        assert_eq!(r, cash);
    }

    #[test]
    fn untagged_operands_never_select_an_overload() {
        // `if (tag1 == 0 && (numparam == 1 || tag2 == 0)) return FALSE;`
        let mut t = Tags::new();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        ops.declare(Overload {
            kind: OpKind::Add,
            lhs: TagId::UNTAGGED,
            rhs: Some(TagId::UNTAGGED),
            result: cash,
        });
        ops.declare(Overload {
            kind: OpKind::Neg,
            lhs: TagId::UNTAGGED,
            rhs: None,
            result: cash,
        });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        assert_eq!(f.ops.find(OpKind::Add, TagId::UNTAGGED, Some(TagId::UNTAGGED)), None);
        assert_eq!(f.ops.find(OpKind::Neg, TagId::UNTAGGED, None), None);
        // but one tagged operand is enough to look
        assert_eq!(f.ops.find(OpKind::Add, cash, Some(TagId::UNTAGGED)), None);
    }

    #[test]
    fn shifts_and_bitwise_operators_are_not_overloadable() {
        for op in [BinOp::Shl, BinOp::Shr, BinOp::ShrU, BinOp::BitAnd, BinOp::BitXor, BinOp::BitOr]
        {
            assert_eq!(OpKind::from_binop(op), None, "{op:?} must not be overloadable");
        }
        assert_eq!(OpKind::from_unop(UnOp::BitNot), None);
    }

    #[test]
    fn a_comparison_overload_still_yields_bool() {
        let mut t = Tags::new();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        // `bool:operator<(Cash:, Cash:)` - the ladder forces `bool:` regardless
        // of what the operator declares.
        ops.declare(Overload { kind: OpKind::Lt, lhs: cash, rhs: Some(cash), result: cash });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        assert_eq!(f.check().binary(BinOp::Lt, cash, cash, sp(), &mut d), f.tags.bool_tag());
        assert!(d.items().is_empty());
    }

    #[test]
    fn an_assignment_overload_coerces_the_value_and_suppresses_213() {
        let mut t = Tags::new();
        let float = t.rational_tag();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        ops.declare(Overload { kind: OpKind::Assign, lhs: float, rhs: Some(cash), result: float });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        let mut d = Diagnostics::new();
        let got =
            f.check().assign(Assignee::expr(float), cash, false, false, sp(), &mut d);
        assert_eq!(got, float);
        assert!(d.items().is_empty());
        // the reverse direction has no overload declared
        let mut d2 = Diagnostics::new();
        f.check().assign(Assignee::expr(cash), float, false, false, sp(), &mut d2);
        assert_eq!(d2.items().len(), 1);
    }

    #[test]
    fn an_assignment_overload_is_not_commutative() {
        // `oper == NULL` short-circuits the commutative retry in check_userop().
        let mut t = Tags::new();
        let float = t.rational_tag();
        let cash = t.add("Cash");
        let mut ops = Overloads::new();
        ops.declare(Overload { kind: OpKind::Assign, lhs: float, rhs: Some(cash), result: float });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        assert_eq!(f.ops.find(OpKind::Assign, float, Some(cash)), Some(float));
        assert_eq!(f.ops.find(OpKind::Assign, cash, Some(float)), None);
    }

    #[test]
    fn unary_operators_report_the_expected_tags() {
        let f = Fx::new();
        let float = f.tags.rational_tag();
        let c = f.check();
        assert_eq!(c.unary(UnOp::LogNot, float), f.tags.bool_tag());
        assert_eq!(c.unary(UnOp::Neg, float), float);
        assert_eq!(c.unary(UnOp::BitNot, float), float);
    }

    #[test]
    fn a_negation_overload_replaces_the_operand_tag() {
        let mut t = Tags::new();
        let cash = t.add("Cash");
        let float = t.rational_tag();
        let mut ops = Overloads::new();
        ops.declare(Overload { kind: OpKind::Neg, lhs: cash, rhs: None, result: float });
        let f = Fx { tags: t, ops, file: PathBuf::from("plugin.sma") };
        assert_eq!(f.check().unary(UnOp::Neg, cash), float);
    }

    // ---- a small end-to-end shape ----------------------------------------

    #[test]
    fn a_realistic_snippet_produces_exactly_the_expected_warnings() {
        // new Float:f
        // new i
        // f = 1.0          -> ok
        // i = f            -> 213 (Float is strong)
        // f = i            -> 213 (untagged into a tagged formal)
        // set_task(5, ...) -> 213
        // set_task(5.0,...)-> ok
        let f = Fx::new();
        let float = f.tags.rational_tag();
        let codes = f.codes(|c, d| {
            c.assign(Assignee::expr(float), float, false, false, sp(), d);
            c.assign(Assignee::expr(TagId::UNTAGGED), float, false, false, sp(), d);
            c.assign(Assignee::expr(float), TagId::UNTAGGED, false, false, sp(), d);
            c.argument(&[float], TagId::UNTAGGED, true, sp(), d);
            c.argument(&[float], float, true, sp(), d);
        });
        assert_eq!(codes, vec![213, 213, 213]);
    }
}
