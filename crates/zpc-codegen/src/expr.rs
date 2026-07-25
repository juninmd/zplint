//! Expression code generation: the port of `sc3.c`.
//!
//! Every routine keeps the invariant the C compiler keeps: an expression's value
//! ends up in PRI, and a binary operator finds its *left* operand in ALT and its
//! *right* operand in PRI (see the comment above `os_lt()` in `sc4.c`).

use zpc_asm::Opcode;
use zpc_ast::expr::{
    Arg, ArgValue, BinOp, Expr, ExprKind, Fixity, IncDecOp, IndexKind, UnOp,
};
use zpc_ast::{Ident, Span};
use zpc_diag::Diagnostics;
use zpc_sema::fold::{Const, Folder};
use zpc_sema::tags::TagId;

use crate::emit::Generator;
use crate::layout::{Callee, Class, ParamKind, VarInfo, VarKind};
use crate::stream::{CELL, CELL_SHIFT, Reg};

/// What an evaluated expression left behind. This is the `ident` field of `value`
/// in `sc.h`, reduced to what code generation actually branches on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Val {
    /// `iCONSTEXPR`: nothing was emitted, the value is known.
    Const(i32),
    /// `iEXPRESSION`: the value is in PRI and is not an lvalue.
    Expr,
    /// `iVARIABLE`/`iREFERENCE`: nothing was emitted; the variable is addressable.
    Var(VarInfo),
    /// `iARRAYCELL`: the *address* of the cell is in PRI.
    ArrayCell,
    /// `iARRAYCHAR`: the address of a packed character is in PRI.
    ArrayChar,
    /// `iARRAY`/`iREFARRAY`: the array's base address is in PRI.
    ArrayRef,
}

impl Val {
    /// Whether `store()` may be applied - `lvalue` in `sc3.c`.
    pub(crate) fn is_lvalue(&self) -> bool {
        matches!(self, Val::Var(_) | Val::ArrayCell | Val::ArrayChar)
    }
}

/// `commutative()` in `sc3.c` line 2363. Only these let `plnge2()` drop the
/// `push.pri`/`pop.alt` pair around a constant right operand.
fn commutative(op: Option<BinOp>) -> bool {
    matches!(
        op,
        Some(
            BinOp::Add
                | BinOp::Mul
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::BitAnd
                | BinOp::BitXor
                | BinOp::BitOr
        )
    )
}

impl Generator {
    // -------------------------------------------------------------- folding

    /// Fold speculatively: diagnostics raised while probing are discarded, because
    /// the expression will be *emitted* if the fold fails and the emitter reports
    /// its own errors. `sc3.c` gets this for free - it folds while parsing.
    pub(crate) fn try_fold(&self, expr: &Expr) -> Option<Const> {
        let mut scratch = Diagnostics::new();
        Folder::new(&self.table, &self.fold_env).with_tags(self.tags).fold(expr, &mut scratch)
    }

    // ------------------------------------------------------- sc4.c accessors

    /// `rvalue()`: get the value described by `val` into PRI.
    pub(crate) fn rvalue(&mut self, val: &Val) {
        match val {
            Val::Const(v) => self.asm.ldconst(*v, Reg::Pri),
            Val::Expr | Val::ArrayRef => {}
            Val::ArrayCell => self.asm.load_i(),
            Val::ArrayChar => self.asm.emit1(Opcode::LodbI, crate::stream::CHAR_BYTES),
            Val::Var(info) => match (&info.kind, info.class) {
                (VarKind::Reference, Class::Local) => {
                    self.asm.emit1(Opcode::LrefSPri, info.addr);
                }
                (VarKind::Reference, Class::Global) => {
                    // "global references don't exist in Pawn" (sc4.c), but the
                    // emitter has the branch, so keep it.
                    self.asm.emit1(Opcode::LrefPri, info.addr);
                }
                (_, Class::Local) => self.asm.emit1(Opcode::LoadSPri, info.addr),
                (_, Class::Global) => self.asm.emit1(Opcode::LoadPri, info.addr),
            },
        }
    }

    /// `address()`: get the *address* of a symbol into `reg`.
    pub(crate) fn address(&mut self, info: &VarInfo, reg: Reg) {
        let op = match (&info.kind, info.class, reg) {
            // A reference or an array argument already holds an address, so it is
            // *loaded*, not computed.
            (VarKind::Reference | VarKind::RefArray(_), _, Reg::Pri) => Opcode::LoadSPri,
            (VarKind::Reference | VarKind::RefArray(_), _, Reg::Alt) => Opcode::LoadSAlt,
            (_, Class::Local, Reg::Pri) => Opcode::AddrPri,
            (_, Class::Local, Reg::Alt) => Opcode::AddrAlt,
            (_, Class::Global, Reg::Pri) => Opcode::ConstPri,
            (_, Class::Global, Reg::Alt) => Opcode::ConstAlt,
        };
        self.asm.emit1(op, info.addr);
    }

    /// `store()`: save PRI into the cell described by `val`.
    pub(crate) fn store(&mut self, val: &Val) {
        match val {
            Val::ArrayCell => self.asm.emit0(Opcode::StorI),
            Val::ArrayChar => self.asm.emit1(Opcode::StrbI, crate::stream::CHAR_BYTES),
            Val::Var(info) => match (&info.kind, info.class) {
                (VarKind::Reference, Class::Local) => self.asm.emit1(Opcode::SrefSPri, info.addr),
                (VarKind::Reference, Class::Global) => self.asm.emit1(Opcode::SrefPri, info.addr),
                (_, Class::Local) => self.asm.emit1(Opcode::StorSPri, info.addr),
                (_, Class::Global) => self.asm.emit1(Opcode::StorPri, info.addr),
            },
            // Not an lvalue; the caller has already reported error 22.
            Val::Const(_) | Val::Expr | Val::ArrayRef => {}
        }
    }

    // -------------------------------------------------------------- entry

    /// Evaluate an expression, leaving it in whatever form is cheapest - the
    /// caller decides whether to `rvalue()` it. `hier14()` in `sc3.c`.
    pub(crate) fn eval(&mut self, expr: &Expr) -> Val {
        if let Some(c) = self.try_fold(expr) {
            return Val::Const(c.value);
        }
        match &expr.kind {
            ExprKind::Num(v) => Val::Const(*v as i32),
            ExprKind::Char { value, .. } => Val::Const(*value as i32),
            ExprKind::Rational(_) => Val::Const(self.try_fold(expr).map(|c| c.value).unwrap_or(0)),
            ExprKind::Str(s) => {
                let cells = self.string_cells(s);
                let addr = self.data.literal(&cells);
                self.asm.ldconst(addr, Reg::Pri);
                Val::ArrayRef
            }
            ExprKind::LitArray { elems, .. } => {
                let cells: Vec<i32> = elems.iter().map(|e| self.const_expr(e)).collect();
                let addr = self.data.literal(&cells);
                self.asm.ldconst(addr, Reg::Pri);
                Val::ArrayRef
            }
            ExprKind::Ident(id) => self.eval_ident(id),
            ExprKind::Comma { exprs, .. } => {
                let mut last = Val::Const(0);
                for (i, e) in exprs.iter().enumerate() {
                    last = self.eval(e);
                    if i + 1 < exprs.len() {
                        // Intermediate operands are evaluated for their side
                        // effects only; force them out so a bare lvalue does not
                        // vanish.
                        let v = last.clone();
                        self.rvalue(&v);
                    }
                }
                last
            }
            ExprKind::Cast { expr: inner, .. } => self.eval(inner),
            ExprKind::Unary { op, operand, .. } => self.eval_unary(*op, operand, expr.span),
            ExprKind::IncDec { op, fixity, operand, .. } => {
                self.eval_incdec(*op, *fixity, operand, expr.span)
            }
            ExprKind::Binary { op, lhs, rhs, .. } => self.eval_binary(*op, lhs, rhs, expr.span),
            ExprKind::Assign { op, target, value, .. } => {
                self.eval_assign(*op, target, value, expr.span)
            }
            ExprKind::Ternary { cond, then_expr, else_expr, .. } => {
                self.eval_ternary(cond, then_expr, else_expr)
            }
            ExprKind::Index { base, index, kind, .. } => self.eval_index(base, index, *kind),
            ExprKind::Call { callee, args, .. } => self.eval_call(callee, args, expr.span),
            ExprKind::CharCells { operand, .. } => {
                // `n char`: from characters to bytes, round up, truncate to cells.
                self.load(operand);
                self.asm.char2addr();
                self.asm.addconst(CELL - 1);
                self.asm.addr2cell();
                Val::Expr
            }
            // These are constant-only constructs; reaching here means the fold
            // failed, and `const_expr` reports the reason.
            ExprKind::SizeOf { .. } | ExprKind::TagOf { .. } | ExprKind::Defined { .. } => {
                let v = self.const_expr(expr);
                Val::Const(v)
            }
            ExprKind::Error(_) => Val::Const(0),
        }
    }

    /// Evaluate and force the result into PRI.
    pub(crate) fn load(&mut self, expr: &Expr) -> Val {
        let v = self.eval(expr);
        self.rvalue(&v);
        v
    }

    fn eval_ident(&mut self, id: &Ident) -> Val {
        if let Some(v) = self.env.constant(&id.name) {
            return Val::Const(v);
        }
        match self.env.var(&id.name).cloned() {
            Some(info) if info.kind.is_array() => {
                self.address(&info, Reg::Pri);
                Val::ArrayRef
            }
            Some(info) => Val::Var(info),
            None => {
                if self.env.func(&id.name).is_some() {
                    // Taking a function's address (`iREFFUNC`) is not implemented.
                    self.error(76, id.span, &[]);
                } else {
                    self.error(17, id.span, &[&id.name]);
                }
                Val::Const(0)
            }
        }
    }

    // -------------------------------------------------------------- unary

    fn eval_unary(&mut self, op: UnOp, operand: &Expr, span: Span) -> Val {
        let tag = self.expr_tag(operand);
        self.load(operand);
        if self.unary_userop(op, tag, span) {
            return Val::Expr;
        }
        match op {
            UnOp::Neg => self.asm.emit0(Opcode::Neg),
            UnOp::LogNot => self.asm.emit0(Opcode::Not),
            UnOp::BitNot => self.asm.emit0(Opcode::Invert),
        }
        Val::Expr
    }

    /// `inc()`/`dec()` in `sc4.c`, driven by `hier2()` (prefix) and `hier1()`
    /// (postfix).
    fn eval_incdec(
        &mut self,
        op: IncDecOp,
        fixity: Fixity,
        operand: &Expr,
        span: Span,
    ) -> Val {
        let tag = self.expr_tag(operand);
        let target = self.eval(operand);
        if !target.is_lvalue() {
            self.error(22, operand.span, &[]);
            return Val::Const(0);
        }
        if let Val::Var(info) = &target
            && info.is_const
        {
            self.error(22, operand.span, &[]);
            return Val::Const(0);
        }

        // For an array cell the address sits in PRI and must survive the read.
        let indirect = matches!(target, Val::ArrayCell | Val::ArrayChar);
        match fixity {
            Fixity::Prefix => {
                // `if (!check_userop(user_inc,...)) inc(lval); rvalue(lval);`
                if !self.incdec_userop(op, &target, tag, span) {
                    self.emit_incdec(op, &target);
                }
                self.rvalue(&target);
            }
            Fixity::Postfix => {
                if indirect {
                    self.asm.pushreg(Reg::Pri);
                }
                self.rvalue(&target);
                if indirect {
                    // swap.pri: stash the value, restore the address in PRI
                    self.asm.swap1();
                }
                if !self.incdec_userop(op, &target, tag, span) {
                    self.emit_incdec(op, &target);
                }
                if indirect {
                    self.asm.popreg(Reg::Pri);
                }
            }
        }
        Val::Expr
    }

    fn emit_incdec(&mut self, op: IncDecOp, target: &Val) {
        let inc = op == IncDecOp::Inc;
        match target {
            Val::ArrayCell => {
                self.asm.emit0(if inc { Opcode::IncI } else { Opcode::DecI });
            }
            Val::ArrayChar => {
                // sc4.c inc(): read-modify-write around the address in PRI.
                self.asm.pushreg(Reg::Pri);
                self.asm.pushreg(Reg::Alt);
                self.asm.move_alt();
                self.asm.emit1(Opcode::LodbI, crate::stream::CHAR_BYTES);
                self.asm.emit0(if inc { Opcode::IncPri } else { Opcode::DecPri });
                self.asm.emit1(Opcode::StrbI, crate::stream::CHAR_BYTES);
                self.asm.popreg(Reg::Alt);
                self.asm.popreg(Reg::Pri);
            }
            Val::Var(info) => match (&info.kind, info.class) {
                (VarKind::Reference, _) => {
                    self.asm.pushreg(Reg::Pri);
                    self.asm.emit1(Opcode::LrefSPri, info.addr);
                    self.asm.emit0(if inc { Opcode::IncPri } else { Opcode::DecPri });
                    self.asm.emit1(Opcode::SrefSPri, info.addr);
                    self.asm.popreg(Reg::Pri);
                }
                (_, Class::Local) => {
                    self.asm.emit1(if inc { Opcode::IncS } else { Opcode::DecS }, info.addr);
                }
                (_, Class::Global) => {
                    self.asm.emit1(if inc { Opcode::Inc } else { Opcode::Dec }, info.addr);
                }
            },
            _ => {}
        }
    }

    // -------------------------------------------------------------- binary

    fn eval_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> Val {
        match op {
            BinOp::LogOr => self.skim(lhs, rhs, op),
            BinOp::LogAnd => self.skim(lhs, rhs, op),
            _ if op.is_relational() => self.plnge_rel(lhs, rhs, op),
            _ => {
                let ltag = self.expr_tag(lhs);
                let left = self.load_operand(lhs);
                self.plnge2(Some(op), left, rhs, span, ltag);
                Val::Expr
            }
        }
    }

    /// `plnge1()` plus the `if (lvalue) rvalue(lval)` that follows it: evaluate an
    /// operand, but leave a constant unloaded so `plnge2()` can put it in ALT.
    fn load_operand(&mut self, expr: &Expr) -> Option<i32> {
        match self.eval(expr) {
            Val::Const(c) => Some(c),
            other => {
                self.rvalue(&other);
                None
            }
        }
    }

    /// `plnge2()`: with the left operand already evaluated, evaluate the right one
    /// and apply `op`, leaving ALT = left and PRI = right.
    fn plnge2(
        &mut self,
        op: Option<BinOp>,
        left: Option<i32>,
        rhs: &Expr,
        span: Span,
        ltag: TagId,
    ) {
        let mut ltag = ltag;
        let mut rtag = self.expr_tag(rhs);
        match left {
            Some(c) => {
                // Constant on the left; it is not yet loaded.
                if let Some(rc) = self.load_operand(rhs) {
                    self.asm.ldconst(rc, Reg::Pri);
                }
                self.asm.ldconst(c, Reg::Alt);
            }
            None => {
                let mark = self.asm.mark();
                self.asm.pushreg(Reg::Pri);
                match self.load_operand(rhs) {
                    Some(rc) if commutative(op) => {
                        // stgdel(): scratch the push and re-fetch the constant into
                        // ALT. The operands swap registers, which is safe exactly
                        // because the operator is commutative.
                        self.asm.truncate(mark);
                        self.asm.ldconst(rc, Reg::Alt);
                        // "swap the lval variables so that lval1 is associated
                        // with the secondary register and lval2 with the
                        // primary" (sc3.c:569-576). `check_userop()` runs after
                        // this swap, so it must see the swapped tags too.
                        std::mem::swap(&mut ltag, &mut rtag);
                    }
                    Some(rc) => {
                        self.asm.ldconst(rc, Reg::Pri);
                        self.asm.popreg(Reg::Alt);
                    }
                    None => self.asm.popreg(Reg::Alt),
                }
            }
        }
        if let Some(op) = op
            && !self.binary_userop(op, ltag, rtag, span)
        {
            self.emit_binop(op, span);
        }
    }

    /// The `op1[]` table of `sc3.c`, resolved to the `sc4.c` emitters.
    fn emit_binop(&mut self, op: BinOp, span: Span) {
        match op {
            BinOp::Add => self.asm.emit0(Opcode::Add),
            // ob_sub is `sub.alt`: PRI = ALT - PRI.
            BinOp::Sub => self.asm.emit0(Opcode::SubAlt),
            BinOp::Mul => self.asm.emit0(Opcode::Smul),
            BinOp::Div => self.asm.emit0(Opcode::SdivAlt),
            BinOp::Mod => {
                // os_mod(): the remainder is left in ALT, so move it across.
                self.asm.emit0(Opcode::SdivAlt);
                self.asm.moveto1();
            }
            BinOp::Shl => {
                self.asm.emit0(Opcode::Xchg);
                self.asm.emit0(Opcode::Shl);
            }
            BinOp::Shr => {
                self.asm.emit0(Opcode::Xchg);
                self.asm.emit0(Opcode::Sshr);
            }
            BinOp::ShrU => {
                self.asm.emit0(Opcode::Xchg);
                self.asm.emit0(Opcode::Shr);
            }
            BinOp::BitAnd => self.asm.emit0(Opcode::And),
            BinOp::BitOr => self.asm.emit0(Opcode::Or),
            BinOp::BitXor => self.asm.emit0(Opcode::Xor),
            BinOp::Eq => self.asm.emit0(Opcode::Eq),
            BinOp::Ne => self.asm.emit0(Opcode::Neq),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.asm.emit0(Opcode::Xchg);
                self.asm.emit0(match op {
                    BinOp::Lt => Opcode::Sless,
                    BinOp::Le => Opcode::Sleq,
                    BinOp::Gt => Opcode::Sgrtr,
                    _ => Opcode::Sgeq,
                });
            }
            BinOp::LogAnd | BinOp::LogOr => {
                // Handled by skim(); reaching here is a codegen bug.
                self.error(76, span, &[]);
            }
        }
    }

    /// `plnge_rel()`: Pawn chains relational operators, so `a < b < c` means
    /// `a < b && b < c` with `b` evaluated once.
    fn plnge_rel(&mut self, lhs: &Expr, rhs: &Expr, op: BinOp) -> Val {
        let mut operands = vec![rhs];
        let mut ops = vec![op];
        let mut cur = lhs;
        while let ExprKind::Binary { op: o, lhs: l, rhs: r, .. } = &cur.kind {
            if !o.is_relational() {
                break;
            }
            operands.push(r);
            ops.push(*o);
            cur = l;
        }
        operands.push(cur);
        operands.reverse();
        ops.reverse();

        let mut ltag = self.expr_tag(operands[0]);
        let mut left = self.load_operand(operands[0]);
        for (i, o) in ops.iter().enumerate() {
            if i > 0 {
                self.asm.relop_prefix();
                // After relop_prefix the previous right operand is in PRI, so it
                // is the left operand of this link of the chain.
                ltag = self.expr_tag(operands[i]);
                left = None;
            }
            self.plnge2(Some(*o), left, operands[i + 1], operands[i + 1].span, ltag);
            if i > 0 {
                self.asm.relop_suffix();
            }
            left = None;
        }
        Val::Expr
    }

    /// `skim()`: short-circuit `&&` and `||`.
    ///
    /// `||` drops out when an operand is true (`jnz`), yielding 1; `&&` drops out
    /// when one is false (`jzer`), yielding 0. The C compiler short-circuits at
    /// code-generation time even though `plnge2()` does not fold the pair.
    fn skim(&mut self, lhs: &Expr, rhs: &Expr, op: BinOp) -> Val {
        let mut operands = vec![rhs];
        let mut cur = lhs;
        while let ExprKind::Binary { op: o, lhs: l, rhs: r, .. } = &cur.kind {
            if *o != op {
                break;
            }
            operands.push(r);
            cur = l;
        }
        operands.push(cur);
        operands.reverse();

        let (drop_val, end_val) = if op == BinOp::LogOr { (1, 0) } else { (0, 1) };
        let droplab = self.asm.label();
        let endlab = self.asm.label();
        for e in &operands {
            self.load(e);
            if op == BinOp::LogOr {
                self.asm.jmp_ne0(droplab);
            } else {
                self.asm.jmp_eq0(droplab);
            }
        }
        self.asm.ldconst(end_val, Reg::Pri);
        self.asm.jumplabel(endlab);
        self.asm.place(droplab);
        self.asm.ldconst(drop_val, Reg::Pri);
        self.asm.place(endlab);
        Val::Expr
    }

    // ------------------------------------------------------------- ternary

    /// `hier13()`. The heap equilibration the C compiler does between the two arms
    /// is not reproduced - see the crate docs.
    fn eval_ternary(&mut self, cond: &Expr, then_expr: &Expr, else_expr: &Expr) -> Val {
        let flab1 = self.asm.label();
        let flab2 = self.asm.label();
        self.load(cond);
        self.asm.jmp_eq0(flab1);
        self.load(then_expr);
        self.asm.jumplabel(flab2);
        self.asm.place(flab1);
        let other = self.load(else_expr);
        self.asm.place(flab2);
        if matches!(other, Val::ArrayRef) { Val::ArrayRef } else { Val::Expr }
    }

    // ----------------------------------------------------------- assignment

    fn eval_assign(&mut self, op: Option<BinOp>, target: &Expr, value: &Expr, span: Span) -> Val {
        // `a += b` is `a = a + b`, so the left operand of the arithmetic is the
        // destination and carries its tag.
        let ltag = self.expr_tag(target);
        let lhs = self.eval(target);
        if let Val::ArrayRef = lhs {
            return self.assign_array(target, value, op, span);
        }
        if !lhs.is_lvalue() {
            self.error(22, target.span, &[]);
            return Val::Const(0);
        }
        if let Val::Var(info) = &lhs
            && info.is_const
        {
            self.error(22, target.span, &[]);
            return Val::Const(0);
        }

        let indirect = matches!(lhs, Val::ArrayCell | Val::ArrayChar);
        if indirect {
            // The cell's address is in PRI and must be preserved across the
            // right-hand side; plnge2() pushes it and pops it back into ALT,
            // which is exactly where stor.i wants it.
            if op.is_some() {
                self.asm.pushreg(Reg::Pri);
                self.rvalue(&lhs);
            }
            self.plnge2(op, None, value, span, ltag);
            if op.is_some() {
                self.asm.popreg(Reg::Alt);
            }
        } else if op.is_some() {
            self.rvalue(&lhs);
            self.plnge2(op, None, value, span, ltag);
        } else {
            // "if direct fetch and simple assignment: no push and pop needed"
            self.load(value);
        }
        self.store(&lhs);
        // hier14() returns FALSE after an assignment: the result is the stored
        // value, in PRI, and is no longer an lvalue.
        Val::Expr
    }

    /// Whole-array assignment: `dest = src` copies `movs` bytes.
    fn assign_array(&mut self, target: &Expr, value: &Expr, op: Option<BinOp>, span: Span) -> Val {
        if op.is_some() {
            // "array assignment must be simple assignment"
            self.error(23, span, &[]);
            return Val::Const(0);
        }
        let dims = self.expr_dims(target);
        let cells: i32 = dims.first().copied().unwrap_or(0);
        // The destination address was already loaded into PRI by eval(target);
        // discard it and reload into ALT after the source, matching copyarray().
        let src = self.eval(value);
        if !matches!(src, Val::ArrayRef) {
            self.error(47, span, &[]);
            return Val::Const(0);
        }
        let Some(info) = self.symbol_of(target) else {
            self.error(22, target.span, &[]);
            return Val::Const(0);
        };
        self.address(&info, Reg::Alt);
        self.asm.memcopy(cells * CELL);
        Val::ArrayRef
    }

    // -------------------------------------------------------------- indexing

    /// The remaining dimensions of an (possibly already indexed) array expression.
    pub(crate) fn expr_dims(&self, expr: &Expr) -> Vec<i32> {
        match &expr.kind {
            ExprKind::Ident(id) => {
                self.env.var(&id.name).map(|v| v.kind.dims().to_vec()).unwrap_or_default()
            }
            ExprKind::Index { base, .. } => {
                let d = self.expr_dims(base);
                d.get(1..).map(<[i32]>::to_vec).unwrap_or_default()
            }
            // The result of an array-returning call is an `iREFARRAY` whose
            // dimensions are the ones attached to the function symbol
            // (`lval_result->sym=symret`, sc3.c:1911), so it can be subscripted.
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Ident(id) => {
                    self.env.func(&id.name).map(|f| f.ret_dims.clone()).unwrap_or_default()
                }
                _ => Vec::new(),
            },
            ExprKind::Cast { expr, .. } => self.expr_dims(expr),
            _ => Vec::new(),
        }
    }

    /// The variable an lvalue expression ultimately names, if any.
    fn symbol_of(&self, expr: &Expr) -> Option<VarInfo> {
        match &expr.kind {
            ExprKind::Ident(id) => self.env.var(&id.name).cloned(),
            ExprKind::Index { base, .. } | ExprKind::Cast { expr: base, .. } => {
                self.symbol_of(base)
            }
            _ => None,
        }
    }

    /// The subscript half of `hier1()`.
    fn eval_index(&mut self, base: &Expr, index: &Expr, kind: IndexKind) -> Val {
        let dims = self.expr_dims(base);
        let base_val = self.eval(base);
        if !matches!(base_val, Val::ArrayRef) {
            self.error(28, base.span, &["<expression>"]);
            return Val::Const(0);
        }
        let len = dims.first().copied().unwrap_or(0);
        let char_index = kind == IndexKind::Char;
        if char_index && dims.len() > 1 {
            // "invalid subscript, use [ ] operators on major dimensions"
            self.error(51, index.span, &[]);
            return Val::Const(0);
        }

        // stgget() is taken *before* the push, so a constant index can scratch it.
        let mark = self.asm.mark();
        self.asm.pushreg(Reg::Pri);
        let folded = self.try_fold(index);
        match folded {
            Some(c) => {
                self.asm.truncate(mark);
                let c = c.value;
                let limit = if char_index { len.saturating_mul(CELL) } else { len };
                if c < 0 || (limit != 0 && limit <= c) {
                    self.error(32, index.span, &[&self.name_of(base)]);
                }
                if c != 0 {
                    let off = if char_index { c } else { c << CELL_SHIFT };
                    self.asm.ldconst(off, Reg::Alt);
                    self.asm.emit0(Opcode::Add);
                }
                if char_index {
                    self.asm.charalign();
                }
            }
            None => {
                self.load(index);
                if len != 0 {
                    let limit = if char_index { len * CELL - 1 } else { len - 1 };
                    self.asm.ffbounds(limit);
                }
                if char_index {
                    self.asm.char2addr();
                } else {
                    self.asm.cell2addr();
                }
                self.asm.popreg(Reg::Alt);
                self.asm.emit0(Opcode::Add);
                if char_index {
                    self.asm.charalign();
                }
            }
        }

        if dims.len() > 1 {
            // A major dimension holds an offset to the sub-array; follow it.
            self.asm.pushreg(Reg::Pri);
            self.asm.load_i();
            self.asm.popreg(Reg::Alt);
            self.asm.emit0(Opcode::Add);
            return Val::ArrayRef;
        }
        if char_index { Val::ArrayChar } else { Val::ArrayCell }
    }

    pub(crate) fn expr_name(&self, expr: &Expr) -> String {
        self.name_of(expr)
    }

    fn name_of(&self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Ident(id) => id.name.clone(),
            ExprKind::Index { base, .. } | ExprKind::Cast { expr: base, .. } => self.name_of(base),
            _ => "-unknown-".to_owned(),
        }
    }

    // ------------------------------------------------------------- calls

    /// `callfunction()` in `sc3.c`.
    ///
    /// # Argument push order
    ///
    /// Arguments are *parsed* left to right, but each one is wrapped in a staging
    /// mark (`stgmark(sEXPRSTART + argpos)`, `sc3.c:1976`) between
    /// `stgmark(sSTARTREORDER)` (`sc3.c:1930`) and `stgmark(sENDREORDER)`
    /// (`sc3.c:2280`). `stgstring()` in `sc7.c:266-270` then flushes the collected
    /// blocks with `while (argc) { argc--; stgstring(stack[argc]...); }` - i.e. in
    /// *descending* argument index. So the emitted code pushes the **last argument
    /// first** and the first argument last, leaving argument 0 nearest the top of
    /// the stack at `FRM + 3*cell`.
    ///
    /// After the arguments, `pushval((cell)nargs*sizeof(cell))` (`sc3.c:2281`)
    /// pushes the **argument count in bytes**; that cell is what `retn` pops and
    /// what `load.s.pri 8` reads.
    fn eval_call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> Val {
        let ExprKind::Ident(name) = &callee.kind else {
            self.error(12, span, &[]);
            return Val::Const(0);
        };
        let Some(info) = self.env.func(&name.name).cloned() else {
            self.error(17, name.span, &[&name.name]);
            return Val::Const(0);
        };

        // Bind arguments to parameter positions: positional first, then named.
        let mut slots: Vec<Option<&Arg>> = vec![None; info.params.len()];
        let mut positional = 0usize;
        for arg in args {
            let pos = match &arg.name {
                None => {
                    let p = positional;
                    positional += 1;
                    p
                }
                Some(n) => match info.param_index(&n.name) {
                    Some(i) => i,
                    None => {
                        self.error(17, n.span, &[&n.name]);
                        continue;
                    }
                },
            };
            if pos >= slots.len() {
                if !info.variadic {
                    self.error(45, arg.span, &[]);
                    continue;
                }
                slots.resize(pos + 1, None);
            }
            if slots[pos].is_some() {
                self.error(58, arg.span, &[]);
            }
            slots[pos] = Some(arg);
        }

        // `nargs` counts every slot that gets a value, defaults included.
        let nargs = slots.iter().filter(|s| s.is_some()).count().max(
            slots
                .iter()
                .enumerate()
                .filter(|(i, s)| {
                    s.is_some() || info.params.get(*i).is_some_and(|p| p.default.is_some())
                })
                .count(),
        );

        // `callfunction()` (sc3.c:1896-1912): "check whether this is a function
        // that returns an array ... allocate space on the heap for the array, and
        // pass the pointer to the reserved memory block as a hidden parameter".
        //
        //     modheap(retsize*sizeof(cell));  /* address is in ALT */
        //     pushreg(sALT);                  /* the last (hidden) parameter */
        //     decl_heap+=retsize;
        //
        // It runs *before* any argument is evaluated and outside the staging marks
        // that sc7.c reorders, so the hidden pointer is pushed first and therefore
        // lands at the highest address of the argument block - FRM+(n+3)*cell, the
        // slot doreturn() reads (sc1.c:5493-5502). It is deliberately *not*
        // counted in `nargs`, so `retn` leaves it on the stack for the `pop.pri`
        // below to collect.
        let returns_array = info.returns_array();
        if returns_array {
            self.asm.modheap(info.ret_cells() * CELL);
            self.asm.pushreg(Reg::Alt);
            self.decl_heap += info.ret_cells();
        }

        let mut heapalloc = 0i32;
        // Descending order: this is the reordering sc7.c performs.
        for idx in (0..slots.len()).rev() {
            let kind = info.param_at(idx).map(|p| p.kind.clone()).unwrap_or(ParamKind::Value);
            let default = info.param_at(idx).and_then(|p| p.default);
            // An explicit `_` and an omitted argument both take the declared
            // default; the C compiler flags the first ARG_IGNORED and handles
            // both in the same fixup loop at the end of callfunction().
            let supplied = match slots[idx] {
                Some(Arg { value: ArgValue::Expr(e), .. }) => Some(e),
                _ => None,
            };
            match supplied {
                Some(e) => heapalloc += self.push_arg(e, &kind),
                None => {
                    let (sp, code) = match slots[idx] {
                        Some(a) => (a.span, 34u16),
                        None => (span, 88u16),
                    };
                    match default {
                        Some(v) => self.asm.ldconst(v, Reg::Pri),
                        None => {
                            self.error(code, sp, &[]);
                            self.asm.ldconst(0, Reg::Pri);
                        }
                    }
                    self.asm.pushreg(Reg::Pri);
                }
            }
        }

        self.asm.pushval(nargs as i32 * CELL);
        match info.callee {
            Callee::Func(label) => self.asm.emit_call(label),
            Callee::Native(idx) => {
                self.asm.emit1(Opcode::SysreqC, idx);
                // A native does not clean the stack itself: the caller drops the
                // arguments plus the count cell.
                self.asm.emit1(Opcode::Stack, (nargs as i32 + 1) * CELL);
            }
        }
        self.asm.modheap(-heapalloc * CELL);
        if returns_array {
            // sc3.c:2289-2290: `if (symret!=NULL) popreg(sPRI);` - "pop hidden
            // parameter as function result". The callee never puts the address in
            // PRI itself (sc1.c:5532: "moveto1() is not necessary, callfunction()
            // does a popreg()"). The heap block stays allocated until the end of
            // the enclosing expression; `expression()` (sc3.c:682) releases it.
            self.asm.popreg(Reg::Pri);
            return Val::ArrayRef;
        }
        Val::Expr
    }

    /// Emit one argument and push it. Returns the number of heap cells claimed.
    fn push_arg(&mut self, e: &Expr, kind: &ParamKind) -> i32 {
        let mut heap = 0;
        match kind {
            ParamKind::Value => {
                self.load(e);
            }
            ParamKind::Reference => {
                let v = self.eval(e);
                match v {
                    Val::Var(info) => self.address(&info, Reg::Pri),
                    Val::ArrayCell | Val::ArrayChar => {
                        // The address is already in PRI.
                    }
                    other => {
                        // Not an lvalue: park the value on the heap and pass that.
                        self.rvalue(&other);
                        self.asm.setheap_pri();
                        heap += 1;
                    }
                }
            }
            ParamKind::Array => {
                let v = self.eval(e);
                // `case iREFARRAY:` (sc3.c:2078) accepts iARRAY, iREFARRAY **and
                // iARRAYCELL**. The last one is how Pawn passes a sub-array:
                // `copy(dest, len, text[pos])` hands over the address of element
                // `pos` onward, and the AMXX headers rely on it heavily
                // (string_stocks.inc alone does it eleven times). `eval` has
                // already left that address in PRI, so there is nothing to emit -
                // rejecting it was the bug.
                match v {
                    Val::ArrayRef | Val::ArrayCell | Val::ArrayChar => {}
                    other => {
                        self.error(35, e.span, &[]);
                        self.rvalue(&other);
                    }
                }
            }
            ParamKind::VarArgs => {
                // "always pass by reference" (sc3.c, case iVARARGS)
                let v = self.eval(e);
                match v {
                    Val::Var(info) if !info.is_const => self.address(&info, Reg::Pri),
                    Val::ArrayRef | Val::ArrayCell => {}
                    other => {
                        self.rvalue(&other);
                        self.asm.setheap_pri();
                        heap += 1;
                    }
                }
            }
        }
        self.asm.pushreg(Reg::Pri);
        heap
    }
}

impl crate::stream::AsmStream {
    /// `ffcall()` for a non-native: `call <label>`.
    pub(crate) fn emit_call(&mut self, target: crate::stream::LabelId) {
        self.emit_jump(Opcode::Call, target);
    }
}
