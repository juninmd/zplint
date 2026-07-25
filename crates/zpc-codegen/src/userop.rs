//! Tag propagation and user-defined operator dispatch: the port of
//! `check_userop()` (`sc3.c:114`) together with the `value.tag` flow that feeds
//! it.
//!
//! # Why a separate tag walk
//!
//! In `sc3.c` the tag of a subexpression is carried in the `value` struct that
//! every `hierN()` fills in as it emits. [`crate::expr`] instead returns a
//! [`Val`], which records only what code generation branches on. Rather than
//! widen `Val` and thread a second value through every emitter, the tag of an
//! operand is recomputed structurally by [`Generator::expr_tag`], which is a
//! pure function of the declaration environment and emits nothing. It reaches
//! exactly the constructs `check_userop()` can be reached from: literals,
//! variables, parameters, constants and enum members, tag casts, call results,
//! subscripts, and the operators themselves.
//!
//! # The dispatch decision
//!
//! `check_userop()` opens with a quick exit - "since user-defined operators on
//! untagged operands are forbidden" - and then looks up the mangled name
//! `operator_symname()` builds out of the operand tags, retrying with the tags
//! swapped when the operator is commutative. That resolution lives in
//! [`zpc_sema::tags::Overloads`] and is *not* repeated here; this module only
//! turns a hit into a call.

use zpc_asm::Opcode;
use zpc_ast::Span;
use zpc_ast::expr::{BinOp, Expr, ExprKind, IncDecOp, UnOp};
use zpc_sema::tags::{OpKind, Overloads, TagId};

use crate::emit::Generator;
use crate::expr::Val;
use crate::layout::Callee;
use crate::stream::{CELL, Reg};

/// One resolved overload: the symbol to call and whether the operands must be
/// pushed in the swapped order (`swapparams` in `check_userop()`).
struct Resolved {
    name: String,
    swapped: bool,
}

impl Generator {
    // ------------------------------------------------------- tag propagation

    /// The tag of an expression, as `sc3.c` would leave it in `lval->tag`.
    ///
    /// Emits nothing. Unknown or unsupported forms yield [`TagId::UNTAGGED`],
    /// which is the value that makes `check_userop()` take its quick exit, so an
    /// incomplete answer here can only ever *miss* an overload - it can never
    /// select the wrong one.
    pub(crate) fn expr_tag(&mut self, e: &Expr) -> TagId {
        match &e.kind {
            // "the tag of an integer, character or string literal, of sizeof,
            // of defined, and of tagof: all untagged".
            ExprKind::Num(_)
            | ExprKind::Char { .. }
            | ExprKind::Str(_)
            | ExprKind::LitArray { .. }
            | ExprKind::SizeOf { .. }
            | ExprKind::TagOf { .. }
            | ExprKind::Defined { .. }
            | ExprKind::CharCells { .. }
            | ExprKind::Error(_) => TagId::UNTAGGED,
            // `sc_rationaltag`, set by `#pragma rational Float`.
            ExprKind::Rational(_) => self.tag_tab.rational_tag(),
            ExprKind::Cast { tag, .. } => self.tag_of(Some(tag)),
            ExprKind::Ident(id) => match self.env.var(&id.name) {
                Some(v) => v.tag,
                // A constant or enum member: the folder knows its tag.
                None => match self.try_fold(e) {
                    Some(c) => self.tag_from_raw(c.tag),
                    None => TagId::UNTAGGED,
                },
            },
            ExprKind::Comma { exprs, .. } => match exprs.last() {
                Some(last) => self.expr_tag(last),
                None => TagId::UNTAGGED,
            },
            // `hier2()`: `!` forces `bool:`; unary `-` keeps the operand's tag
            // unless an overload replaces it. `~` is not overloadable.
            ExprKind::Unary { op, operand, .. } => {
                let inner = self.expr_tag(operand);
                match OpKind::from_unop(*op) {
                    Some(kind) => match self.ops.find(kind, inner, None) {
                        Some(result) => result,
                        None if *op == UnOp::LogNot => self.tag_tab.bool_tag(),
                        None => inner,
                    },
                    None => inner,
                }
            }
            ExprKind::IncDec { op, operand, .. } => {
                let inner = self.expr_tag(operand);
                let kind = incdec_kind(*op);
                self.ops.find(kind, inner, None).unwrap_or(inner)
            }
            ExprKind::Binary { op, lhs, rhs, .. } => {
                if matches!(op, BinOp::LogAnd | BinOp::LogOr) {
                    return self.tag_tab.bool_tag();
                }
                let ltag = self.expr_tag(lhs);
                let rtag = self.expr_tag(rhs);
                let overload =
                    OpKind::from_binop(*op).and_then(|k| self.ops.find(k, ltag, Some(rtag)));
                // `plnge()` overwrites the tag with `bool:` for the relational
                // and equality levels, *after* `check_userop()` has run.
                if op.is_boolean() {
                    return self.tag_tab.bool_tag();
                }
                overload.unwrap_or(ltag)
            }
            // `hier14()`: the result of an assignment is the stored value.
            ExprKind::Assign { target, .. } => self.expr_tag(target),
            ExprKind::Ternary { then_expr, .. } => self.expr_tag(then_expr),
            // An array's tag is the tag of its elements, so a subscript keeps it.
            ExprKind::Index { base, .. } => self.expr_tag(base),
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Ident(id) => {
                    self.env.func(&id.name).map(|f| f.ret_tag).unwrap_or(TagId::UNTAGGED)
                }
                _ => TagId::UNTAGGED,
            },
        }
    }

    // ------------------------------------------------------------- dispatch

    /// Resolve the overload `check_userop()` would select, or `None` when the
    /// built-in opcode stands.
    ///
    /// The tag logic - the untagged quick exit, the mangled lookup and the
    /// commutative retry - is [`Overloads::find`]'s; this only has to work out
    /// *which* of the two candidate names it matched, because that is the symbol
    /// to call.
    fn resolve(&mut self, kind: OpKind, lhs: TagId, rhs: Option<TagId>) -> Option<Resolved> {
        self.ops.find(kind, lhs, rhs)?;
        let direct = Overloads::mangle(kind, lhs, rhs, lhs);
        if self.env.func(&direct).is_some() {
            return Some(Resolved { name: direct, swapped: false });
        }
        let r = rhs?;
        let swapped = Overloads::mangle(kind, r, Some(lhs), r);
        self.env.func(&swapped).map(|_| Resolved { name: swapped, swapped: true })
    }

    /// "we don't want to use the redefined operator in the function that
    /// redefines the operator itself" (`sym == curfunc`, `sc3.c:208`).
    fn is_current_operator(&self, name: &str) -> bool {
        self.cur_op.as_deref() == Some(name)
    }

    /// Whether the resolved operator can actually be called. A `forward`-only
    /// operator (`forward operator%(Float:, Float:);` in `float.inc`) exists
    /// precisely so that the operation is *rejected* rather than compiled to the
    /// integer opcode, which is `error(4)` in `check_userop()`.
    fn operator_callable(&mut self, r: &Resolved, span: Span) -> bool {
        match self.env.func(&r.name) {
            Some(info) if info.defined => true,
            _ => {
                self.error(4, span, &[&r.name]);
                false
            }
        }
    }

    /// `ffcall()` for a resolved operator: the argument-count cell, then the
    /// call. Operands must already be pushed.
    fn call_operator(&mut self, name: &str, nargs: i32) {
        let Some(callee) = self.env.func(name).map(|f| f.callee.clone()) else { return };
        self.asm.pushval(nargs * CELL);
        match callee {
            Callee::Func(label) => self.asm.emit_call(label),
            Callee::Native(idx) => {
                self.asm.emit1(Opcode::SysreqC, idx);
                self.asm.emit1(Opcode::Stack, (nargs + 1) * CELL);
            }
        }
    }

    /// `check_userop()` at a binary operator, with ALT holding the left operand
    /// and PRI the right one.
    ///
    /// Returns `true` when the built-in opcode must **not** be emitted - either
    /// because a call replaced it, or because the operation was rejected. A
    /// rejected operation deliberately does not fall through: compiling
    /// `Float:a % Float:b` as an integer `sdiv` is exactly the silent
    /// miscompilation the `forward` declaration exists to prevent.
    pub(crate) fn binary_userop(
        &mut self,
        op: BinOp,
        ltag: TagId,
        rtag: TagId,
        span: Span,
    ) -> bool {
        let Some(kind) = OpKind::from_binop(op) else { return false };
        let Some(r) = self.resolve(kind, ltag, Some(rtag)) else { return false };
        if self.is_current_operator(&r.name) {
            return false;
        }
        if !self.operator_callable(&r, span) {
            return true;
        }
        // `binoper_savepri`: the chained comparison operators require that ALT
        // is unmodified across the call, "actually, we save PRI because the
        // normal instruction sequence (without user operator) swaps PRI and ALT".
        let savepri = matches!(kind, OpKind::Le | OpKind::Ge | OpKind::Lt | OpKind::Gt);
        if savepri {
            self.asm.pushreg(Reg::Pri);
        }
        // "a function expects that the parameters are pushed in reversed order,
        // and the left operand is in the secondary register".
        if r.swapped {
            self.asm.pushreg(Reg::Alt);
            self.asm.pushreg(Reg::Pri);
        } else {
            self.asm.pushreg(Reg::Pri);
            self.asm.pushreg(Reg::Alt);
        }
        self.call_operator(&r.name, 2);
        if savepri {
            self.asm.popreg(Reg::Alt);
        }
        true
    }

    /// `check_userop()` at a prefix `!` or `-`, with the operand in PRI.
    pub(crate) fn unary_userop(&mut self, op: UnOp, tag: TagId, span: Span) -> bool {
        let Some(kind) = OpKind::from_unop(op) else { return false };
        let Some(r) = self.resolve(kind, tag, None) else { return false };
        if self.is_current_operator(&r.name) {
            return false;
        }
        if !self.operator_callable(&r, span) {
            return true;
        }
        self.asm.pushreg(Reg::Pri);
        self.call_operator(&r.name, 1);
        true
    }

    /// `check_userop()` at `++`/`--` (`sc3.c:211-220` and `sc3.c:274-280`).
    ///
    /// Unlike the other operators this one owns the whole read-modify-write:
    /// "for increment and decrement operators, the symbol must first be loaded
    /// (and stored back afterwards)". `target` must be the lvalue, with an
    /// `iARRAYCELL`/`iARRAYCHAR` address already in PRI.
    ///
    /// The closing `moveto1()` is `move.pri`, i.e. `PRI = ALT`; it is what
    /// restores the cell address for the `rvalue()` that `hier2()` performs
    /// after a *prefix* `++`. It is emitted unconditionally upstream, so it is
    /// emitted unconditionally here.
    pub(crate) fn incdec_userop(
        &mut self,
        op: IncDecOp,
        target: &Val,
        tag: TagId,
        span: Span,
    ) -> bool {
        let kind = incdec_kind(op);
        let Some(r) = self.resolve(kind, tag, None) else { return false };
        if self.is_current_operator(&r.name) {
            return false;
        }
        if !self.operator_callable(&r, span) {
            return true;
        }
        let indirect = matches!(target, Val::ArrayCell | Val::ArrayChar);
        if indirect {
            self.asm.pushreg(Reg::Pri); // save current address in PRI
        }
        self.rvalue(target); // get the symbol's value in PRI
        self.asm.pushreg(Reg::Pri);
        self.call_operator(&r.name, 1);
        if indirect {
            self.asm.popreg(Reg::Alt); // restore address (in ALT)
        }
        self.store(target);
        self.asm.moveto1();
        true
    }
}

/// `user_inc` / `user_dec`.
fn incdec_kind(op: IncDecOp) -> OpKind {
    match op {
        IncDecOp::Inc => OpKind::Inc,
        IncDecOp::Dec => OpKind::Dec,
    }
}
