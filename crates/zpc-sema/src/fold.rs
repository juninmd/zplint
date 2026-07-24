//! Constant folding and compile-time expression evaluation.
//!
//! Port of the constant paths through `hier14()`..`primary()` in
//! `libpc300/sc3.c` - chiefly `calc()`, `plnge2()`, `plnge_rel()`, `skim()` and
//! the `sizeof`/`tagof`/`defined` cases of `hier2()` - plus `constexpr()` and
//! `test()` from `sc1.c`.
//!
//! The guiding rule is *fidelity*, not "what a constant folder should do":
//! anything amxxpc refuses to fold is refused here too, because array sizes,
//! `case` labels and default arguments are accepted or rejected on exactly that
//! basis (error 8, "must be a constant expression"). The notable consequences,
//! each documented at its implementation site, are that a ternary never folds,
//! that `0 && f()` is not a constant, and that rational arithmetic is not a
//! constant because it goes through user-defined operators.

use std::collections::HashMap;

use zpc_ast::expr::{BinOp, Expr, ExprKind, SizeOfLevel, TagOfTarget, UnOp};
use zpc_diag::{Diagnostics, Span};

use crate::symbols::{SymKind, SymbolTable, Usage};

/// A Pawn cell. AMX Mod X is built with `PAWN_CELL_SIZE==32`, so every fold in
/// this module is 32-bit and wraps; see [`Cell`] uses of `wrapping_*`.
pub type Cell = i32;

/// A numeric tag id, as handed out by `pc_addtag()`. Tag *naming* and tag
/// compatibility (`matchtag()`, warning 213) belong to the tag table, not here;
/// this module only propagates ids so the caller can check them.
pub type TagId = i32;

/// The "untagged" tag, id 0.
pub const TAG_NONE: TagId = 0;

/// `PUBLICTAG` from `sc.h`: `tagof` ORs this into its result to mark the tag as
/// exported.
pub const PUBLICTAG: TagId = 0x8000_0000u32 as i32;

/// `sCHARBITS` from `sc.h` - the width of a packed character.
pub const CHARBITS: Cell = 8;

/// Bytes per cell for a 32-bit AMX.
pub const CELL_BYTES: Cell = 4;

/// A folded compile-time value: `iCONSTEXPR` plus the tag that came with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Const {
    pub value: Cell,
    pub tag: TagId,
}

impl Const {
    pub fn new(value: Cell, tag: TagId) -> Self {
        Self { value, tag }
    }

    /// An untagged constant.
    pub fn untagged(value: Cell) -> Self {
        Self { value, tag: TAG_NONE }
    }

    pub fn is_zero(self) -> bool {
        self.value == 0
    }
}

/// Tag ids the folder needs to know by role.
///
/// They are supplied by the caller rather than looked up, so this module stays
/// independent of the tag table. Leaving a field at 0 disables the behaviour
/// that depends on it: with `rational == 0` the rational special cases in
/// `hier2()`'s unary minus and in `check_userop()` are inactive, which is
/// exactly what upstream does (it guards on `sc_rationaltag != 0`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TagConfig {
    /// `pc_addtag("bool")`.
    pub bool_tag: TagId,
    /// `sc_rationaltag`, i.e. `Float:` once `#pragma rational` has been seen.
    pub rational_tag: TagId,
    /// `rational_digits`; 0 means IEEE floating point, non-zero means fixed
    /// point (AMX Mod X only ever ships the former).
    pub rational_digits: u16,
}

/// One dimension chain of an array symbol, mirroring the `dim.array` list that
/// `finddepend()` walks in `sc3.c`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ArrayInfo {
    /// Declared length per level, major dimension first. A `0` means "size not
    /// known" (`var[]`) and drives warning 224.
    pub dims: Vec<Cell>,
}

impl ArrayInfo {
    /// `sym->dim.array.level` - the 0-based index of the last dimension.
    fn level(&self) -> usize {
        self.dims.len().saturating_sub(1)
    }

    /// `array_levelsize()`.
    fn level_size(&self, level: usize) -> Cell {
        self.dims.get(level).copied().unwrap_or(0)
    }
}

/// An enumeration member used as a "struct field", i.e. the `Field` in
/// `sizeof arr[Field]` / `tagof arr[Field]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EnumField {
    /// `dim.array.length` of the constant - the field's declared size. `0` for a
    /// plain scalar field, which `hier2()` reports as size 1.
    pub len: Cell,
    /// `x.idxtag` - the tag `tagof arr[Field]` yields.
    pub idx_tag: TagId,
}

/// Everything the folder needs about a name that the symbol table does not
/// carry: constant values, array shapes and tag ids.
///
/// The symbol table records that a name exists and what kind it is; this trait
/// supplies the payload. Keeping them apart means the folder never has to
/// mutate the table, and it can be driven from a fixture in tests.
pub trait ConstEnv {
    /// Value and tag of a named constant (`SymKind::Constant`).
    fn constant(&self, name: &str) -> Option<Const>;

    /// Dimensions of an array symbol.
    fn array(&self, name: &str) -> Option<&ArrayInfo>;

    /// The tag of any symbol, for `tagof value`.
    fn symbol_tag(&self, name: &str) -> Option<TagId> {
        self.constant(name).map(|c| c.tag)
    }

    /// A constant used as an enumeration field.
    fn enum_field(&self, name: &str) -> Option<EnumField>;

    /// Numeric id of a tag *name*, for `tagof(Float:)`.
    fn tag_id(&self, name: &str) -> Option<TagId>;

    /// True if `defined NAME` should yield 1 because the name is a `#define`
    /// macro. Symbol-table hits are handled by the folder itself.
    fn is_macro(&self, name: &str) -> bool;
}

/// A ready-made [`ConstEnv`] backed by hash maps.
#[derive(Clone, Debug, Default)]
pub struct MapEnv {
    pub constants: HashMap<String, Const>,
    pub arrays: HashMap<String, ArrayInfo>,
    pub symbol_tags: HashMap<String, TagId>,
    pub enum_fields: HashMap<String, EnumField>,
    pub tags: HashMap<String, TagId>,
    pub macros: Vec<String>,
}

impl MapEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_const(mut self, name: &str, value: Cell, tag: TagId) -> Self {
        self.constants.insert(name.to_owned(), Const::new(value, tag));
        self
    }

    pub fn with_array(mut self, name: &str, dims: &[Cell]) -> Self {
        self.arrays.insert(name.to_owned(), ArrayInfo { dims: dims.to_vec() });
        self
    }

    pub fn with_field(mut self, name: &str, len: Cell, idx_tag: TagId) -> Self {
        self.enum_fields.insert(name.to_owned(), EnumField { len, idx_tag });
        self
    }

    pub fn with_symbol_tag(mut self, name: &str, tag: TagId) -> Self {
        self.symbol_tags.insert(name.to_owned(), tag);
        self
    }

    pub fn with_tag(mut self, name: &str, tag: TagId) -> Self {
        self.tags.insert(name.to_owned(), tag);
        self
    }

    pub fn with_macro(mut self, name: &str) -> Self {
        self.macros.push(name.to_owned());
        self
    }
}

impl ConstEnv for MapEnv {
    fn constant(&self, name: &str) -> Option<Const> {
        self.constants.get(name).copied()
    }

    fn array(&self, name: &str) -> Option<&ArrayInfo> {
        self.arrays.get(name)
    }

    fn symbol_tag(&self, name: &str) -> Option<TagId> {
        self.symbol_tags
            .get(name)
            .copied()
            .or_else(|| self.constants.get(name).map(|c| c.tag))
    }

    fn enum_field(&self, name: &str) -> Option<EnumField> {
        self.enum_fields.get(name).copied()
    }

    fn tag_id(&self, name: &str) -> Option<TagId> {
        self.tags.get(name).copied()
    }

    fn is_macro(&self, name: &str) -> bool {
        self.macros.iter().any(|m| m == name)
    }
}

/// The context one folding pass runs in.
pub struct Folder<'a> {
    table: &'a SymbolTable,
    env: &'a dyn ConstEnv,
    tags: TagConfig,
}

impl<'a> Folder<'a> {
    pub fn new(table: &'a SymbolTable, env: &'a dyn ConstEnv) -> Self {
        Self { table, env, tags: TagConfig::default() }
    }

    pub fn with_tags(mut self, tags: TagConfig) -> Self {
        self.tags = tags;
        self
    }

    /// `expression()` restricted to its constant answer: `Some` iff the result
    /// would be `iCONSTEXPR`.
    ///
    /// Diagnostics that upstream raises *while* evaluating (undefined symbol,
    /// `sizeof` on a constant, ...) are pushed even when the fold fails, because
    /// upstream raises them from `hier2()` regardless of the caller.
    pub fn fold(&self, expr: &Expr, diags: &mut Diagnostics) -> Option<Const> {
        self.fold_inner(expr, diags)
    }

    /// `constexpr()` from `sc1.c`: demand a constant, and report error 8 with an
    /// assumed value of zero if the expression is not one.
    pub fn const_expr(&self, expr: &Expr, diags: &mut Diagnostics) -> Const {
        match self.fold(expr, diags) {
            Some(c) => c,
            None => {
                self.emit(diags, 8, expr.span, &[]);
                Const::untagged(0)
            }
        }
    }

    /// `needsub()`/`initials()` in `sc1.c`: an array dimension must be a
    /// constant (error 8) and must be positive (error 9).
    pub fn array_size(&self, expr: &Expr, diags: &mut Diagnostics) -> Cell {
        let val = self.const_expr(expr, diags).value;
        if val <= 0 {
            self.emit(diags, 9, expr.span, &[]);
            return 0;
        }
        val
    }

    /// `test()` in `sc1.c`: the condition of `if`/`while`/`do`/`for`.
    ///
    /// A constant condition is redundant - warning 206 when it is non-zero (the
    /// body always runs), warning 205 when it is zero (the body never runs).
    /// Returns the folded condition so a caller can also prune the branch.
    pub fn check_test(&self, expr: &Expr, diags: &mut Diagnostics) -> Option<Const> {
        let folded = self.fold(expr, diags)?;
        self.emit_redundant(diags, folded, expr.span);
        Some(folded)
    }

    fn emit_redundant(&self, diags: &mut Diagnostics, folded: Const, span: Span) {
        // 206 "redundant test: constant expression is non-zero"
        // 205 "redundant code: constant expression is zero"
        self.emit(diags, if folded.is_zero() { 205 } else { 206 }, span, &[]);
    }

    fn emit(&self, diags: &mut Diagnostics, code: u16, span: Span, args: &[&str]) {
        diags.emit(code, span, self.table.file(), args);
    }

    // ---------------------------------------------------------------- folding

    fn fold_inner(&self, expr: &Expr, diags: &mut Diagnostics) -> Option<Const> {
        match &expr.kind {
            // The lexer has already reduced every numeric form to a cell; the
            // value is stored widened, so truncate the way the compiler's
            // `cell` assignment does.
            ExprKind::Num(v) => Some(Const::untagged(*v as Cell)),
            ExprKind::Char { value, .. } => Some(Const::untagged(*value as Cell)),
            ExprKind::Rational(lit) => Some(Const::new(
                self.rational_bits(lit.value),
                self.tags.rational_tag,
            )),
            ExprKind::Ident(id) => self.fold_ident(&id.name),
            ExprKind::Cast { tag, expr: inner, .. } => {
                let inner = self.fold_inner(inner, diags)?;
                // A tag override generates no code: only the ascribed tag
                // changes, so a constant stays constant. `_:` strips the tag.
                let tag = if tag.is_untagged() {
                    TAG_NONE
                } else {
                    self.env.tag_id(&tag.name.name).unwrap_or(TAG_NONE)
                };
                Some(Const::new(inner.value, tag))
            }
            ExprKind::Unary { op, operand, .. } => self.fold_unary(*op, operand, diags),
            ExprKind::Binary { op, .. } => self.fold_binary(expr, *op, diags),
            ExprKind::CharCells { operand, .. } => {
                let v = self.fold_inner(operand, diags)?;
                // hier2(), case tCHAR: characters -> bytes -> cells, rounded up.
                // Upstream uses C division, which truncates towards zero; that
                // only differs from flooring for a negative operand, which the
                // operator is never meaningfully applied to.
                let bytes = v.value.wrapping_mul(CHARBITS / 8);
                Some(Const::new(
                    bytes.wrapping_add(CELL_BYTES - 1) / CELL_BYTES,
                    v.tag,
                ))
            }
            ExprKind::Comma { exprs, .. } => {
                // The value of a comma expression is that of the last operand;
                // upstream simply keeps the `lval` of the last `expression()`.
                let (last, rest) = exprs.split_last()?;
                for e in rest {
                    let _ = self.fold_inner(e, diags);
                }
                self.fold_inner(last, diags)
            }
            ExprKind::SizeOf { symbol, levels, .. } => {
                self.fold_sizeof(&symbol.name, symbol.span, levels, diags)
            }
            ExprKind::TagOf { target, levels, .. } => self.fold_tagof(target, levels, diags),
            ExprKind::Defined { symbol, .. } => {
                let defined = self.defined(&symbol.name) || self.env.is_macro(&symbol.name);
                Some(Const::new(Cell::from(defined), self.tags.bool_tag))
            }

            ExprKind::Ternary { cond, then_expr, else_expr, .. } => {
                // hier13(): a constant condition is still reported as a
                // redundant test, but the conditional itself is *never* folded -
                // it always ends up as iEXPRESSION. `1 ? 2 : 3` is therefore not
                // a constant expression in Pawn.
                if let Some(c) = self.fold_inner(cond, diags) {
                    self.emit_redundant(diags, c, cond.span);
                }
                let _ = self.fold_inner(then_expr, diags);
                let _ = self.fold_inner(else_expr, diags);
                None
            }

            // Never constant. A string or literal array is materialised in the
            // literal pool and evaluates to its *address* (iARRAY), assignments
            // and ++/-- have side effects, calls and subscripts are runtime.
            ExprKind::Str(_)
            | ExprKind::LitArray { .. }
            | ExprKind::Assign { .. }
            | ExprKind::IncDec { .. }
            | ExprKind::Index { .. }
            | ExprKind::Call { .. }
            | ExprKind::Error(_) => None,
        }
    }

    fn fold_ident(&self, name: &str) -> Option<Const> {
        let id = self.table.lookup(name)?;
        if self.table.get(id).kind != SymKind::Constant {
            return None;
        }
        self.env.constant(name)
    }

    fn defined(&self, name: &str) -> bool {
        // hier2(), case tDEFINED: a non-function symbol that is in the table but
        // not yet defined does not count.
        match self.table.lookup(name) {
            Some(id) => {
                let sym = self.table.get(id);
                sym.kind.is_function() || sym.usage.contains(Usage::DEFINED)
            }
            None => false,
        }
    }

    /// The cell representation of a rational literal.
    ///
    /// With `rational_digits == 0` (the AMX Mod X configuration) the cell holds
    /// the IEEE-754 bit pattern of an `f32`, which is why `Float:1` is a tiny
    /// denormal and not `1.0`. With a non-zero precision the value is fixed
    /// point: the literal scaled by `10^digits`.
    fn rational_bits(&self, value: f64) -> Cell {
        if self.tags.rational_digits == 0 {
            (value as f32).to_bits() as Cell
        } else {
            let scale = 10f64.powi(i32::from(self.tags.rational_digits));
            (value * scale).round() as Cell
        }
    }

    /// True when an operand's tag routes the operator to a user-defined
    /// `operator` function. `plnge2()` gives up on folding in that case
    /// (`check_userop()` sets `iEXPRESSION`), so `1.5 + 2.5` is *not* a
    /// constant expression - float.inc defines `operator+(Float:,Float:)`.
    fn is_userop_tag(&self, tag: TagId) -> bool {
        self.tags.rational_tag != TAG_NONE && tag == self.tags.rational_tag
    }

    fn fold_unary(&self, op: UnOp, operand: &Expr, diags: &mut Diagnostics) -> Option<Const> {
        let v = self.fold_inner(operand, diags)?;
        Some(match op {
            UnOp::BitNot => Const::new(!v.value, v.tag),
            UnOp::LogNot => {
                // A user-defined `operator!` would block the fold; none is
                // defined for Float: in the AMX Mod X headers, and `!` on a
                // float bit pattern is what upstream computes too.
                Const::new(Cell::from(v.value == 0), self.tags.bool_tag)
            }
            UnOp::Neg => {
                if self.is_userop_tag(v.tag) {
                    // hier2(), case '-': the rational constant is special-cased
                    // *before* check_userop, so the sign is flipped in place.
                    if self.tags.rational_digits == 0 {
                        Const::new((-f32::from_bits(v.value as u32)).to_bits() as Cell, v.tag)
                    } else {
                        Const::new(v.value.wrapping_neg(), v.tag)
                    }
                } else {
                    Const::new(v.value.wrapping_neg(), v.tag)
                }
            }
        })
    }

    fn fold_binary(&self, expr: &Expr, op: BinOp, diags: &mut Diagnostics) -> Option<Const> {
        match op {
            BinOp::LogAnd | BinOp::LogOr => self.fold_logical(expr, op, diags),
            _ if op.is_relational() => self.fold_relational(expr, diags),
            _ => {
                let ExprKind::Binary { lhs, rhs, .. } = &expr.kind else { unreachable!() };
                let l = self.fold_inner(lhs, diags);
                let r = self.fold_inner(rhs, diags);
                let (l, r) = (l?, r?);
                if self.is_userop_tag(l.tag) || self.is_userop_tag(r.tag) {
                    return None;
                }
                let value = self.calc(op, l.value, r.value, expr.span, diags)?;
                let tag = if op.is_boolean() { self.tags.bool_tag } else { l.tag };
                Some(Const::new(value, tag))
            }
        }
    }

    /// `calc()` from `sc3.c`, minus the relational cases (see
    /// [`Folder::fold_relational`]).
    ///
    /// Every arithmetic operation wraps. Upstream performs these in C on a
    /// signed `cell`, so overflow is formally undefined but in practice wraps,
    /// and amxxpc emits *no* diagnostic for it - error 105 ("numeric overflow")
    /// is raised only for an array dimension exceeding `INT_MAX`, which cannot
    /// happen in a 32-bit build. We reproduce the wrap and stay silent.
    fn calc(
        &self,
        op: BinOp,
        left: Cell,
        right: Cell,
        span: Span,
        diags: &mut Diagnostics,
    ) -> Option<Cell> {
        Some(match op {
            BinOp::BitOr => left | right,
            BinOp::BitXor => left ^ right,
            BinOp::BitAnd => left & right,
            BinOp::Eq => Cell::from(left == right),
            BinOp::Ne => Cell::from(left != right),
            BinOp::Add => left.wrapping_add(right),
            BinOp::Sub => left.wrapping_sub(right),
            BinOp::Mul => left.wrapping_mul(right),

            // `os_sar` - arithmetic shift right; `ou_sar` - logical shift right;
            // `ob_sal` - shift left, done on `ucell` upstream.
            //
            // A shift count outside 0..32 is undefined in C. amxxpc compiles to
            // x86 shifts, which mask the count to 5 bits, and that is what the
            // AMX interpreter does at run time as well, so we mask too. This is
            // the one place where upstream's behaviour is formally unspecified;
            // see the module notes.
            BinOp::Shr => left >> (right as u32 & 31),
            BinOp::ShrU => ((left as u32) >> (right as u32 & 31)) as Cell,
            BinOp::Shl => ((left as u32) << (right as u32 & 31)) as Cell,

            // Pawn divides towards negative infinity and its modulus takes the
            // sign of the divisor, unlike C:
            //   calc():        (left - truemodulus(left,right)) / right
            //   truemodulus(): (a % b + b) % b
            // So -7/2 == -4 and -7%2 == 1, where C would give -3 and -1.
            BinOp::Div | BinOp::Mod => {
                if right == 0 {
                    // amxxpc has no guard here at all: `calc()` divides by zero
                    // and the process traps. We report error 29, the code
                    // `calc()` itself uses for "this should never occur", and
                    // assume zero so the rest of the file can still be checked.
                    self.emit(diags, 29, span, &[]);
                    return None;
                }
                let m = truemodulus(left, right);
                if op == BinOp::Mod { m } else { left.wrapping_sub(m).wrapping_div(right) }
            }

            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::LogAnd | BinOp::LogOr => {
                unreachable!("handled by fold_relational/fold_logical")
            }
        })
    }

    /// `plnge_rel()`: Pawn *chains* relational operators.
    ///
    /// `a < b < c` means `a < b && b < c` with `b` evaluated once, not C's
    /// `(a < b) < c`. `calc()` implements this by returning the *right* operand
    /// as the running value while AND-ing each comparison into `boolresult`,
    /// which `plnge_rel()` finally installs as `constval`.
    ///
    /// The AST stores such a run as left-nested `Binary` nodes, so the chain is
    /// recovered by flattening. Ambiguity: an explicitly parenthesised
    /// `(a < b) < c` produces the same shape and is therefore folded as a chain
    /// too. That matches the parser it is paired with - `hier9()` consumes the
    /// whole run itself and there is no way to write the C meaning in Pawn - but
    /// it does mean this function cannot distinguish the two spellings.
    fn fold_relational(&self, expr: &Expr, diags: &mut Diagnostics) -> Option<Const> {
        let mut ops = Vec::new();
        let mut operands = Vec::new();
        flatten_relational(expr, &mut ops, &mut operands);

        let mut values = Vec::with_capacity(operands.len());
        let mut all_const = true;
        for e in &operands {
            match self.fold_inner(e, diags) {
                Some(c) => values.push(c),
                None => all_const = false,
            }
        }
        if !all_const {
            return None;
        }
        if values.iter().any(|c| self.is_userop_tag(c.tag)) {
            return None;
        }

        let mut result = true;
        for (i, op) in ops.iter().enumerate() {
            let (l, r) = (values[i].value, values[i + 1].value);
            result &= match op {
                BinOp::Lt => l < r,
                BinOp::Le => l <= r,
                BinOp::Gt => l > r,
                BinOp::Ge => l >= r,
                _ => unreachable!(),
            };
        }
        Some(Const::new(Cell::from(result), self.tags.bool_tag))
    }

    /// `skim()`: `&&` and `||`.
    ///
    /// Fidelity trap: upstream folds only when *every* operand is constant
    /// (`allconst = allconst && lval->ident == iCONSTEXPR`). It does not
    /// short-circuit at compile time, so `0 && f()` and `1 || f()` are runtime
    /// expressions, not constants. The generated code short-circuits; the
    /// *fold* does not.
    fn fold_logical(&self, expr: &Expr, op: BinOp, diags: &mut Diagnostics) -> Option<Const> {
        let mut operands = Vec::new();
        flatten_logical(expr, op, &mut operands);

        let mut all_const = true;
        let mut acc: Option<bool> = None;
        for e in &operands {
            match self.fold_inner(e, diags) {
                Some(c) => {
                    let b = c.value != 0;
                    acc = Some(match acc {
                        None => b,
                        Some(prev) if op == BinOp::LogOr => prev || b,
                        Some(prev) => prev && b,
                    });
                }
                None => all_const = false,
            }
        }
        if !all_const {
            return None;
        }
        Some(Const::new(Cell::from(acc?), self.tags.bool_tag))
    }

    // ------------------------------------------------------- sizeof and tagof

    /// `hier2()`, case `tSIZEOF`.
    ///
    /// `sizeof` takes a *symbol*, never an expression. `charsmax(x)` and
    /// `cellsof(x)` are not compiler tokens at all - they are `#define` macros
    /// in the AMX Mod X headers that expand to `(sizeof(x)-1)` and a `sizeof`
    /// division respectively, so they arrive here already expanded and need no
    /// special handling.
    fn fold_sizeof(
        &self,
        name: &str,
        span: Span,
        levels: &[SizeOfLevel],
        diags: &mut Diagnostics,
    ) -> Option<Const> {
        let Some(id) = self.table.lookup(name) else {
            self.emit(diags, 17, span, &[name]);
            return None;
        };
        let sym = self.table.get(id);
        let kind = sym.kind;
        match kind {
            SymKind::Constant => self.emit(diags, 39, span, &[]),
            SymKind::Function | SymKind::Native => self.emit(diags, 72, span, &[]),
            _ if !sym.usage.contains(Usage::DEFINED) => {
                self.emit(diags, 17, span, &[name]);
                return None;
            }
            _ => {}
        }

        // Anything that is not an array has size 1, whatever the brackets say.
        if !matches!(kind, SymKind::Array | SymKind::RefArray) {
            return Some(Const::untagged(1));
        }
        let info = self.env.array(name)?;

        let level = levels.len();
        let value = if level > info.level() + 1 {
            self.emit(diags, 28, span, &[name]);
            1
        } else if level == info.level() + 1 {
            // Innermost bracket naming an enumeration field:
            // `sizeof arr[Coords]` is the field's declared size.
            let field = levels.last().and_then(|l| l.field.as_ref());
            match field {
                Some(f) => match self.env.enum_field(&f.name) {
                    Some(ef) if ef.len > 0 => ef.len,
                    Some(_) => 1,
                    None => {
                        self.emit(diags, 80, f.span, &[&f.name]);
                        1
                    }
                },
                None => 1,
            }
        } else {
            info.level_size(level)
        };

        if value == 0 {
            self.emit(diags, 224, span, &[name]);
        }
        Some(Const::untagged(value))
    }

    /// `hier2()`, case `tTAGOF`. The result carries `PUBLICTAG`, which is what
    /// makes `tagof x == tagof y` comparable against a `tagof(Tag:)` literal.
    fn fold_tagof(
        &self,
        target: &TagOfTarget,
        levels: &[SizeOfLevel],
        diags: &mut Diagnostics,
    ) -> Option<Const> {
        let mut tag = match target {
            TagOfTarget::Tag(t) => {
                // An unknown tag name yields 0 upstream (find_constval fails),
                // without a diagnostic.
                if t.is_untagged() { TAG_NONE } else { self.env.tag_id(&t.name.name).unwrap_or(TAG_NONE) }
            }
            TagOfTarget::Symbol(id) => {
                let Some(sid) = self.table.lookup(&id.name) else {
                    self.emit(diags, 17, id.span, &[&id.name]);
                    return None;
                };
                let sym = self.table.get(sid);
                if !sym.kind.is_function() && !sym.usage.contains(Usage::DEFINED) {
                    self.emit(diags, 17, id.span, &[&id.name]);
                    return None;
                }
                let is_array = matches!(sym.kind, SymKind::Array | SymKind::RefArray);
                let base = self.env.symbol_tag(&id.name).unwrap_or(TAG_NONE);
                if is_array {
                    let info = self.env.array(&id.name)?;
                    if levels.len() > info.level() + 1 {
                        self.emit(diags, 28, id.span, &[&id.name]);
                    } else if levels.len() == info.level() + 1
                        && let Some(f) = levels.last().and_then(|l| l.field.as_ref())
                    {
                        match self.env.enum_field(&f.name) {
                            // Only a named field overrides the tag.
                            Some(ef) => return Some(public_tag(ef.idx_tag)),
                            None => self.emit(diags, 80, f.span, &[&f.name]),
                        }
                    }
                }
                base
            }
        };
        tag &= !PUBLICTAG;
        Some(public_tag(tag))
    }
}

fn public_tag(tag: TagId) -> Const {
    Const::untagged(tag | PUBLICTAG)
}

/// `truemodulus()` from `sc3.c`: `(a % b + b) % b`, the modulus that follows the
/// sign of the divisor. Written with wrapping arithmetic because `i32::MIN % -1`
/// panics in Rust while wrapping in C.
fn truemodulus(a: Cell, b: Cell) -> Cell {
    let first = a.wrapping_rem(b);
    first.wrapping_add(b).wrapping_rem(b)
}

/// Collect a left-nested run of relational operators into operands and ops.
fn flatten_relational<'e>(expr: &'e Expr, ops: &mut Vec<BinOp>, operands: &mut Vec<&'e Expr>) {
    if let ExprKind::Binary { op, lhs, rhs, .. } = &expr.kind
        && op.is_relational()
    {
        flatten_relational(lhs, ops, operands);
        ops.push(*op);
        operands.push(rhs);
        return;
    }
    operands.push(expr);
}

/// Collect a run of the same logical operator, matching `skim()`'s loop over one
/// operator list.
fn flatten_logical<'e>(expr: &'e Expr, want: BinOp, operands: &mut Vec<&'e Expr>) {
    if let ExprKind::Binary { op, lhs, rhs, .. } = &expr.kind
        && *op == want
    {
        flatten_logical(lhs, want, operands);
        flatten_logical(rhs, want, operands);
        return;
    }
    operands.push(expr);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpc_ast::TagRef;
    use zpc_ast::Ident;
    use zpc_ast::expr::{RationalLit, StringLit};

    use crate::symbols::{SymbolDecl, SymbolTable};

    const S: Span = Span { start: 0, end: 1 };

    fn num(v: i64) -> Expr {
        Expr { kind: ExprKind::Num(v), span: S }
    }

    fn ident(name: &str) -> Expr {
        Expr { kind: ExprKind::Ident(Ident::new(name, S)), span: S }
    }

    fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr {
            kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span: S },
            span: S,
        }
    }

    fn un(op: UnOp, e: Expr) -> Expr {
        Expr { kind: ExprKind::Unary { op, operand: Box::new(e), span: S }, span: S }
    }

    struct Fx {
        table: SymbolTable,
        env: MapEnv,
    }

    impl Fx {
        fn new() -> Self {
            Self { table: SymbolTable::new("t.sma"), env: MapEnv::new() }
        }

        fn declare(&mut self, name: &str, kind: SymKind) {
            self.table.declare(
                SymbolDecl::new(name, kind, S).with_usage(Usage::DEFINED),
            );
        }

        fn constant(mut self, name: &str, value: Cell, tag: TagId) -> Self {
            self.declare(name, SymKind::Constant);
            self.env = self.env.with_const(name, value, tag);
            self
        }

        fn array(mut self, name: &str, dims: &[Cell]) -> Self {
            self.declare(name, SymKind::Array);
            self.env = self.env.with_array(name, dims);
            self
        }

        fn folder(&self) -> Folder<'_> {
            Folder::new(&self.table, &self.env)
        }
    }

    /// Fold with a bare context and assert no diagnostics were produced.
    fn f(e: &Expr) -> Option<Cell> {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        let out = fx.folder().fold(e, &mut d).map(|c| c.value);
        assert_eq!(d.items().len(), 0, "unexpected diagnostics: {:?}", d.items());
        out
    }

    // ------------------------------------------------------------- arithmetic

    #[test]
    fn basic_arithmetic_folds() {
        assert_eq!(f(&bin(BinOp::Add, num(2), num(3))), Some(5));
        assert_eq!(f(&bin(BinOp::Sub, num(2), num(3))), Some(-1));
        assert_eq!(f(&bin(BinOp::Mul, num(6), num(7))), Some(42));
        assert_eq!(f(&bin(BinOp::BitXor, num(0b1100), num(0b1010))), Some(0b0110));
        assert_eq!(f(&bin(BinOp::BitAnd, num(0b1100), num(0b1010))), Some(0b1000));
        assert_eq!(f(&bin(BinOp::BitOr, num(0b1100), num(0b1010))), Some(0b1110));
    }

    #[test]
    fn arithmetic_wraps_instead_of_panicking() {
        // amxxpc emits no diagnostic for constant overflow; error 105 is only
        // for array sizes beyond INT_MAX, unreachable in a 32-bit build.
        let max = i64::from(i32::MAX);
        assert_eq!(f(&bin(BinOp::Add, num(max), num(1))), Some(i32::MIN));
        assert_eq!(f(&bin(BinOp::Mul, num(max), num(2))), Some(-2));
        assert_eq!(f(&bin(BinOp::Sub, num(i64::from(i32::MIN)), num(1))), Some(i32::MAX));
        assert_eq!(f(&un(UnOp::Neg, num(i64::from(i32::MIN)))), Some(i32::MIN));
    }

    #[test]
    fn division_floors_towards_negative_infinity() {
        // The Pawn trap: calc() computes (a - truemodulus(a,b)) / b, so the
        // quotient floors and the remainder follows the sign of the divisor.
        // C would give -3 and -1 for the first pair.
        assert_eq!(f(&bin(BinOp::Div, num(-7), num(2))), Some(-4));
        assert_eq!(f(&bin(BinOp::Mod, num(-7), num(2))), Some(1));

        assert_eq!(f(&bin(BinOp::Div, num(7), num(-2))), Some(-4));
        assert_eq!(f(&bin(BinOp::Mod, num(7), num(-2))), Some(-1));

        assert_eq!(f(&bin(BinOp::Div, num(-7), num(-2))), Some(3));
        assert_eq!(f(&bin(BinOp::Mod, num(-7), num(-2))), Some(-1));

        assert_eq!(f(&bin(BinOp::Div, num(7), num(2))), Some(3));
        assert_eq!(f(&bin(BinOp::Mod, num(7), num(2))), Some(1));

        // exact division is unaffected by the rounding rule
        assert_eq!(f(&bin(BinOp::Div, num(-8), num(2))), Some(-4));
        assert_eq!(f(&bin(BinOp::Mod, num(-8), num(2))), Some(0));
    }

    #[test]
    fn division_of_int_min_by_minus_one_wraps() {
        // i32::MIN / -1 overflows; Rust panics, C wraps. Must not panic.
        let e = bin(BinOp::Div, num(i64::from(i32::MIN)), num(-1));
        assert_eq!(f(&e), Some(i32::MIN));
        let e = bin(BinOp::Mod, num(i64::from(i32::MIN)), num(-1));
        assert_eq!(f(&e), Some(0));
    }

    #[test]
    fn division_by_zero_reports_instead_of_panicking() {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        let out = fx.folder().fold(&bin(BinOp::Div, num(1), num(0)), &mut d);
        assert!(out.is_none());
        assert_eq!(d.items().len(), 1);
        assert_eq!(d.items()[0].code, 29);

        let mut d = Diagnostics::new();
        assert!(fx.folder().fold(&bin(BinOp::Mod, num(1), num(0)), &mut d).is_none());
        assert_eq!(d.items()[0].code, 29);
    }

    #[test]
    fn shift_right_is_arithmetic_and_shift_right_unsigned_is_logical() {
        assert_eq!(f(&bin(BinOp::Shr, num(-8), num(1))), Some(-4));
        assert_eq!(f(&bin(BinOp::Shr, num(-1), num(31))), Some(-1));
        assert_eq!(f(&bin(BinOp::ShrU, num(-8), num(1))), Some(0x7fff_fffc));
        assert_eq!(f(&bin(BinOp::ShrU, num(-1), num(31))), Some(1));
        assert_eq!(f(&bin(BinOp::Shl, num(1), num(31))), Some(i32::MIN));
        // shift counts are masked to 5 bits, as x86 and the AMX interpreter do
        assert_eq!(f(&bin(BinOp::Shl, num(1), num(32))), Some(1));
        assert_eq!(f(&bin(BinOp::Shr, num(-1), num(32))), Some(-1));
    }

    #[test]
    fn unary_operators_fold() {
        assert_eq!(f(&un(UnOp::BitNot, num(0))), Some(-1));
        assert_eq!(f(&un(UnOp::LogNot, num(0))), Some(1));
        assert_eq!(f(&un(UnOp::LogNot, num(5))), Some(0));
        assert_eq!(f(&un(UnOp::Neg, num(5))), Some(-5));
    }

    #[test]
    fn char_cells_rounds_up() {
        let cc = |n: i64| {
            f(&Expr { kind: ExprKind::CharCells { operand: Box::new(num(n)), span: S }, span: S })
        };
        assert_eq!(cc(0), Some(0));
        assert_eq!(cc(1), Some(1));
        assert_eq!(cc(4), Some(1));
        assert_eq!(cc(5), Some(2));
        assert_eq!(cc(33), Some(9));
    }

    // ------------------------------------------------------------- comparison

    #[test]
    fn comparisons_produce_zero_or_one() {
        assert_eq!(f(&bin(BinOp::Eq, num(3), num(3))), Some(1));
        assert_eq!(f(&bin(BinOp::Ne, num(3), num(3))), Some(0));
        assert_eq!(f(&bin(BinOp::Lt, num(1), num(2))), Some(1));
        assert_eq!(f(&bin(BinOp::Ge, num(1), num(2))), Some(0));
    }

    #[test]
    fn relational_operators_chain_like_pawn_not_like_c() {
        // 1 < 2 < 3 is (1<2) && (2<3) = 1. C's (1<2)<3 would also be 1, so use
        // a case where they differ: 3 > 2 > 1 is (3>2)&&(2>1) = 1, while C's
        // (3>2)>1 is 1>1 = 0.
        let e = bin(BinOp::Gt, bin(BinOp::Gt, num(3), num(2)), num(1));
        assert_eq!(f(&e), Some(1));

        // 1 < 2 < 0 chains to (1<2) && (2<0) = 0; C's (1<2)<0 is 1<0 = 0 too,
        // so pick 0 < 1 < 1: chained (0<1)&&(1<1) = 0, C's (0<1)<1 = 1<1 = 0.
        // The clean discriminator is 5 > 4 > 3 > 2: chained 1, C's ((1)>3)>2 = 0.
        let e = bin(
            BinOp::Gt,
            bin(BinOp::Gt, bin(BinOp::Gt, num(5), num(4)), num(3)),
            num(2),
        );
        assert_eq!(f(&e), Some(1));

        // a broken link anywhere fails the whole chain
        let e = bin(BinOp::Lt, bin(BinOp::Lt, num(1), num(9)), num(3));
        assert_eq!(f(&e), Some(0));
    }

    #[test]
    fn relational_chain_is_not_constant_if_any_link_is_not() {
        let mut fx = Fx::new();
        fx.declare("g", SymKind::Variable);
        let mut d = Diagnostics::new();
        let e = bin(BinOp::Lt, bin(BinOp::Lt, num(1), ident("g")), num(3));
        assert!(fx.folder().fold(&e, &mut d).is_none());
    }

    #[test]
    fn relational_result_carries_the_bool_tag() {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        let folder = fx.folder().with_tags(TagConfig { bool_tag: 7, ..Default::default() });
        let c = folder.fold(&bin(BinOp::Lt, num(1), num(2)), &mut d).unwrap();
        assert_eq!(c.tag, 7);
    }

    // ---------------------------------------------------------------- logical

    #[test]
    fn logical_operators_fold_when_every_operand_is_constant() {
        assert_eq!(f(&bin(BinOp::LogAnd, num(1), num(2))), Some(1));
        assert_eq!(f(&bin(BinOp::LogAnd, num(0), num(2))), Some(0));
        assert_eq!(f(&bin(BinOp::LogOr, num(0), num(0))), Some(0));
        assert_eq!(f(&bin(BinOp::LogOr, num(0), num(4))), Some(1));
        // a chain of three
        let e = bin(BinOp::LogOr, bin(BinOp::LogOr, num(0), num(0)), num(9));
        assert_eq!(f(&e), Some(1));
    }

    #[test]
    fn logical_does_not_short_circuit_at_compile_time() {
        // skim() requires *every* operand to be iCONSTEXPR, so `0 && g` stays a
        // runtime expression even though its value is obviously 0.
        let mut fx = Fx::new();
        fx.declare("g", SymKind::Variable);
        let mut d = Diagnostics::new();
        assert!(
            fx.folder()
                .fold(&bin(BinOp::LogAnd, num(0), ident("g")), &mut d)
                .is_none()
        );
        assert!(
            fx.folder()
                .fold(&bin(BinOp::LogOr, num(1), ident("g")), &mut d)
                .is_none()
        );
    }

    // ------------------------------------------------------------- identifiers

    #[test]
    fn named_constants_fold_but_variables_do_not() {
        let mut fx = Fx::new().constant("MAX", 32, 0);
        fx.declare("counter", SymKind::Variable);
        let mut d = Diagnostics::new();
        let folder = fx.folder();
        assert_eq!(folder.fold(&ident("MAX"), &mut d).map(|c| c.value), Some(32));
        assert!(folder.fold(&ident("counter"), &mut d).is_none());
        assert!(folder.fold(&ident("nope"), &mut d).is_none());
        let e = bin(BinOp::Mul, ident("MAX"), num(2));
        assert_eq!(folder.fold(&e, &mut d).map(|c| c.value), Some(64));
    }

    // ------------------------------------------------------------------ sizeof

    fn sizeof(name: &str, levels: Vec<SizeOfLevel>) -> Expr {
        Expr {
            kind: ExprKind::SizeOf { symbol: Ident::new(name, S), levels, span: S },
            span: S,
        }
    }

    fn level(field: Option<&str>) -> SizeOfLevel {
        SizeOfLevel { field: field.map(|f| Ident::new(f, S)), span: S }
    }

    #[test]
    fn sizeof_on_multidimensional_arrays_walks_the_levels() {
        let fx = Fx::new().array("grid", &[4, 8, 16]);
        let mut d = Diagnostics::new();
        let folder = fx.folder();
        // sizeof grid       -> major dimension
        assert_eq!(folder.fold(&sizeof("grid", vec![]), &mut d).map(|c| c.value), Some(4));
        // sizeof grid[]     -> second dimension
        assert_eq!(
            folder.fold(&sizeof("grid", vec![level(None)]), &mut d).map(|c| c.value),
            Some(8)
        );
        // sizeof grid[][]   -> last dimension
        assert_eq!(
            folder
                .fold(&sizeof("grid", vec![level(None), level(None)]), &mut d)
                .map(|c| c.value),
            Some(16)
        );
        assert_eq!(d.items().len(), 0);
    }

    #[test]
    fn sizeof_past_the_last_dimension_is_one_and_too_far_is_error_28() {
        let fx = Fx::new().array("list", &[10]);
        let mut d = Diagnostics::new();
        let folder = fx.folder();
        // one bracket past the last dimension, with no enum field: size 1
        assert_eq!(
            folder.fold(&sizeof("list", vec![level(None)]), &mut d).map(|c| c.value),
            Some(1)
        );
        assert_eq!(d.items().len(), 0);
        // two past: invalid subscript
        let out = folder.fold(&sizeof("list", vec![level(None), level(None)]), &mut d);
        assert_eq!(out.map(|c| c.value), Some(1));
        assert_eq!(d.items()[0].code, 28);
    }

    #[test]
    fn sizeof_with_an_enum_field_uses_the_field_size() {
        let mut fx = Fx::new().array("players", &[32]);
        fx.env = fx.env.with_field("PlayerName", 33, 0).with_field("PlayerId", 0, 0);
        let mut d = Diagnostics::new();
        let folder = fx.folder();
        // sizeof players[PlayerName] -> the field's declared size
        assert_eq!(
            folder
                .fold(&sizeof("players", vec![level(Some("PlayerName"))]), &mut d)
                .map(|c| c.value),
            Some(33)
        );
        // a scalar field has length 0 in the table and reports as 1
        assert_eq!(
            folder
                .fold(&sizeof("players", vec![level(Some("PlayerId"))]), &mut d)
                .map(|c| c.value),
            Some(1)
        );
        assert_eq!(d.items().len(), 0);
        // an unknown field is error 80
        let _ = folder.fold(&sizeof("players", vec![level(Some("Bogus"))]), &mut d);
        assert_eq!(d.items()[0].code, 80);
    }

    #[test]
    fn sizeof_diagnoses_bad_symbols() {
        let mut fx = Fx::new().constant("MAX", 5, 0);
        fx.declare("go", SymKind::Function);
        let folder = fx.folder();

        let mut d = Diagnostics::new();
        let _ = folder.fold(&sizeof("missing", vec![]), &mut d);
        assert_eq!(d.items()[0].code, 17);

        let mut d = Diagnostics::new();
        let _ = folder.fold(&sizeof("MAX", vec![]), &mut d);
        assert_eq!(d.items()[0].code, 39); // constant symbol has no size

        let mut d = Diagnostics::new();
        let _ = folder.fold(&sizeof("go", vec![]), &mut d);
        assert_eq!(d.items()[0].code, 72); // "function" symbol has no size
    }

    #[test]
    fn sizeof_on_an_unsized_array_warns_224() {
        let fx = Fx::new().array("open", &[0]);
        let mut d = Diagnostics::new();
        let out = fx.folder().fold(&sizeof("open", vec![]), &mut d);
        assert_eq!(out.map(|c| c.value), Some(0));
        assert_eq!(d.items()[0].code, 224);
    }

    #[test]
    fn sizeof_on_a_scalar_is_one() {
        let mut fx = Fx::new();
        fx.declare("n", SymKind::Variable);
        let mut d = Diagnostics::new();
        assert_eq!(fx.folder().fold(&sizeof("n", vec![]), &mut d).map(|c| c.value), Some(1));
    }

    // ------------------------------------------------------------------- tagof

    #[test]
    fn tagof_yields_the_tag_with_the_public_bit() {
        let mut fx = Fx::new();
        fx.declare("f", SymKind::Variable);
        fx.env = fx.env.with_symbol_tag("f", 3).with_tag("Float", 3);
        let mut d = Diagnostics::new();
        let folder = fx.folder();

        let e = Expr {
            kind: ExprKind::TagOf {
                target: TagOfTarget::Symbol(Ident::new("f", S)),
                levels: vec![],
                span: S,
            },
            span: S,
        };
        let sym = folder.fold(&e, &mut d).unwrap();
        assert_eq!(sym.value, 3 | PUBLICTAG);

        let e = Expr {
            kind: ExprKind::TagOf {
                target: TagOfTarget::Tag(TagRef { name: Ident::new("Float", S), span: S }),
                levels: vec![],
                span: S,
            },
            span: S,
        };
        assert_eq!(folder.fold(&e, &mut d).unwrap().value, sym.value);
        assert_eq!(d.items().len(), 0);
    }

    #[test]
    fn tagof_on_an_enum_field_takes_the_field_index_tag() {
        let mut fx = Fx::new().array("players", &[32]);
        fx.env = fx.env.with_symbol_tag("players", 1).with_field("Origin", 3, 9);
        let mut d = Diagnostics::new();
        let e = Expr {
            kind: ExprKind::TagOf {
                target: TagOfTarget::Symbol(Ident::new("players", S)),
                levels: vec![level(Some("Origin"))],
                span: S,
            },
            span: S,
        };
        assert_eq!(fx.folder().fold(&e, &mut d).unwrap().value, 9 | PUBLICTAG);
    }

    // ----------------------------------------------------------------- defined

    #[test]
    fn defined_reports_symbols_and_macros() {
        let mut fx = Fx::new();
        fx.declare("known", SymKind::Variable);
        fx.env = fx.env.with_macro("MACRO");
        let folder = fx.folder();
        let mut d = Diagnostics::new();
        let def = |n: &str| Expr {
            kind: ExprKind::Defined { symbol: Ident::new(n, S), span: S },
            span: S,
        };
        assert_eq!(folder.fold(&def("known"), &mut d).map(|c| c.value), Some(1));
        assert_eq!(folder.fold(&def("MACRO"), &mut d).map(|c| c.value), Some(1));
        assert_eq!(folder.fold(&def("nope"), &mut d).map(|c| c.value), Some(0));
        assert_eq!(d.items().len(), 0);
    }

    // ---------------------------------------------------------------- rational

    #[test]
    fn a_rational_literal_folds_to_its_f32_bit_pattern() {
        let fx = Fx::new();
        let folder = fx
            .folder()
            .with_tags(TagConfig { rational_tag: 5, ..Default::default() });
        let mut d = Diagnostics::new();
        let lit = Expr {
            kind: ExprKind::Rational(RationalLit { value: 1.5, raw: "1.5".into(), span: S }),
            span: S,
        };
        let c = folder.fold(&lit, &mut d).unwrap();
        assert_eq!(c.value, 1.5f32.to_bits() as i32);
        assert_eq!(c.tag, 5);

        // unary minus flips the sign bit in place (hier2's rational special case)
        let neg = folder.fold(&un(UnOp::Neg, lit), &mut d).unwrap();
        assert_eq!(f32::from_bits(neg.value as u32), -1.5);
    }

    #[test]
    fn rational_arithmetic_is_not_a_constant_expression() {
        // float.inc defines operator+(Float:,Float:), so check_userop() fires in
        // plnge2() and the result is iEXPRESSION, not iCONSTEXPR.
        let fx = Fx::new();
        let folder = fx
            .folder()
            .with_tags(TagConfig { rational_tag: 5, ..Default::default() });
        let mut d = Diagnostics::new();
        let lit = || Expr {
            kind: ExprKind::Rational(RationalLit { value: 1.5, raw: "1.5".into(), span: S }),
            span: S,
        };
        assert!(folder.fold(&bin(BinOp::Add, lit(), lit()), &mut d).is_none());
        assert!(folder.fold(&bin(BinOp::Lt, lit(), lit()), &mut d).is_none());
    }

    // ------------------------------------------------------------------- casts

    #[test]
    fn a_tag_override_keeps_the_value_and_changes_the_tag() {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        let env_folder = fx.folder();
        let cast = |tag: &str, e: Expr| Expr {
            kind: ExprKind::Cast {
                tag: TagRef { name: Ident::new(tag, S), span: S },
                expr: Box::new(e),
                span: S,
            },
            span: S,
        };
        // unknown tag name -> 0; `Float:1` is a bit pattern, not 1.0
        let c = env_folder.fold(&cast("Float", num(1)), &mut d).unwrap();
        assert_eq!(c.value, 1);
        // `_:` strips the tag
        let c = env_folder.fold(&cast("_", num(1)), &mut d).unwrap();
        assert_eq!(c.tag, TAG_NONE);
    }

    // ----------------------------------------------------------- not constants

    #[test]
    fn non_constant_forms_do_not_fold() {
        let str_e = Expr {
            kind: ExprKind::Str(StringLit {
                value: "hi".into(),
                packed: false,
                raw: false,
                span: S,
            }),
            span: S,
        };
        assert_eq!(f(&str_e), None);

        let lit_arr = Expr {
            kind: ExprKind::LitArray { elems: vec![num(1), num(2)], span: S },
            span: S,
        };
        assert_eq!(f(&lit_arr), None);

        let call = Expr {
            kind: ExprKind::Call {
                callee: Box::new(ident("f")),
                args: vec![],
                parenthesised: true,
                span: S,
            },
            span: S,
        };
        assert_eq!(f(&call), None);
    }

    #[test]
    fn a_ternary_never_folds_and_reports_a_redundant_test() {
        // hier13() marks the conditional as iEXPRESSION unconditionally, so
        // `1 ? 2 : 3` is not usable as an array size, and the constant condition
        // still raises 206.
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        let e = Expr {
            kind: ExprKind::Ternary {
                cond: Box::new(num(1)),
                then_expr: Box::new(num(2)),
                else_expr: Box::new(num(3)),
                span: S,
            },
            span: S,
        };
        assert!(fx.folder().fold(&e, &mut d).is_none());
        assert_eq!(d.items()[0].code, 206);
    }

    #[test]
    fn a_comma_expression_takes_the_last_operand() {
        let e = Expr {
            kind: ExprKind::Comma { exprs: vec![num(1), num(2), num(3)], span: S },
            span: S,
        };
        assert_eq!(f(&e), Some(3));
    }

    // ------------------------------------------------------- constexpr / tests

    #[test]
    fn const_expr_reports_error_8_and_assumes_zero() {
        let mut fx = Fx::new();
        fx.declare("g", SymKind::Variable);
        let mut d = Diagnostics::new();
        let c = fx.folder().const_expr(&ident("g"), &mut d);
        assert_eq!(c.value, 0);
        assert_eq!(d.items()[0].code, 8);
        assert!(d.items()[0].message.starts_with("must be a constant expression"));
    }

    #[test]
    fn array_size_rejects_zero_and_negative() {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        assert_eq!(fx.folder().array_size(&num(8), &mut d), 8);
        assert_eq!(d.items().len(), 0);

        assert_eq!(fx.folder().array_size(&num(0), &mut d), 0);
        assert_eq!(d.items()[0].code, 9);

        let mut d = Diagnostics::new();
        assert_eq!(fx.folder().array_size(&num(-1), &mut d), 0);
        assert_eq!(d.items()[0].code, 9);
    }

    #[test]
    fn warning_206_fires_on_a_constant_true_condition() {
        let fx = Fx::new();
        let mut d = Diagnostics::new();
        // if (1)
        assert!(fx.folder().check_test(&num(1), &mut d).is_some());
        assert_eq!(d.items()[0].code, 206);
        assert_eq!(d.items()[0].message, "redundant test: constant expression is non-zero");

        // if (0) -> 205 instead
        let mut d = Diagnostics::new();
        fx.folder().check_test(&num(0), &mut d);
        assert_eq!(d.items()[0].code, 205);

        // a folded, non-obvious constant condition also fires
        let mut d = Diagnostics::new();
        fx.folder().check_test(&bin(BinOp::Gt, num(3), num(2)), &mut d);
        assert_eq!(d.items()[0].code, 206);
    }

    #[test]
    fn a_runtime_condition_is_silent() {
        let mut fx = Fx::new();
        fx.declare("g", SymKind::Variable);
        let mut d = Diagnostics::new();
        assert!(fx.folder().check_test(&ident("g"), &mut d).is_none());
        assert_eq!(d.items().len(), 0);
    }
}
