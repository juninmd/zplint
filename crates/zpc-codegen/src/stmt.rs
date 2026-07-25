//! Statement code generation: the port of `statement()` and friends in `sc1.c`.

use zpc_asm::Opcode;
use zpc_ast::decl::{Declarator, Init, VarDecl};
use zpc_ast::expr::Expr;
use zpc_ast::stmt::{Block, CaseLabel, EmitOperand, EmitStmt, ForInit, Stmt, SwitchCase};
use zpc_ast::{Ident, Span};

use crate::expr::Val;
use crate::emit::{Generator, LoopFrame};
use crate::layout::{Class, VarInfo, VarKind};
use crate::stream::{CELL, LabelId, Reg};

/// `xEXIT` from `sc.h`: exit code in PRI, tag in ALT.
const X_EXIT: i32 = 1;
/// `xASSERTION`.
const X_ASSERTION: i32 = 2;
/// `xSLEEP`.
const X_SLEEP: i32 = 12;

impl Generator {
    /// The body of a function: a block that must *not* open a scope of its own
    /// beyond the one holding the parameters, and whose stack is unwound by the
    /// caller (`newfunc()` does the final `modstk`).
    pub(crate) fn block_body(&mut self, block: &Block) {
        for s in &block.stmts {
            self.statement(s);
        }
    }

    /// `compound()`: a nested block owns the locals it declares.
    fn block(&mut self, block: &Block) {
        let save = self.declared;
        self.env.enter();
        for s in &block.stmts {
            self.statement(s);
        }
        self.asm.modstk((self.declared - save) * CELL);
        self.declared = save;
        self.env.leave();
    }

    pub(crate) fn statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Block(b) => self.block(b),
            Stmt::Empty(_) => {}
            Stmt::Expr { expr, .. } => self.expression(|g| {
                let v = g.eval(expr);
                // A bare lvalue statement still has to be materialised, because
                // `doexpr()` ends with `markexpr(sEXPR)` on a loaded value.
                if matches!(v, Val::Var(_) | Val::ArrayCell | Val::ArrayChar) {
                    g.rvalue(&v);
                }
            }),
            Stmt::Var(v) => self.declloc(v),
            Stmt::Const(c) => {
                let value = self.const_expr(&c.value);
                self.define_const(&c.name.name, value, c.name.span);
            }
            Stmt::Enum(_) => {
                // Block-local enums were already folded by the global pre-pass for
                // top-level enums only; a local one is not registered. See the
                // crate docs.
            }
            Stmt::If { cond, then_branch, else_branch, .. } => {
                self.do_if(cond, then_branch, else_branch.as_deref());
            }
            Stmt::While { cond, body, .. } => self.do_while(cond, body),
            Stmt::DoWhile { body, cond, .. } => self.do_do(body, cond),
            Stmt::For { init, cond, step, body, .. } => {
                self.do_for(init.as_ref(), cond.as_ref(), step.as_ref(), body);
            }
            Stmt::Switch { scrutinee, cases, default, .. } => {
                self.do_switch(scrutinee, cases, default.as_deref());
            }
            Stmt::Return { value, span } => self.do_return(value.as_ref(), *span),
            Stmt::Break { span } => self.do_break(*span),
            Stmt::Continue { span } => self.do_continue(*span),
            Stmt::Goto { label, .. } => {
                let l = self.goto_label(label);
                self.asm.jumplabel(l);
            }
            Stmt::Label { name, .. } => {
                let l = self.goto_label(name);
                self.asm.place(l);
                // dolabel(): a jump may arrive from a scope with a different
                // stack depth, so STK is forced back to this point's depth.
                if self.declared != 0 {
                    self.asm.setstk(-self.declared * CELL);
                }
            }
            Stmt::Assert { exprs, .. } => {
                for e in exprs {
                    let ok = self.asm.label();
                    self.expression(|g| g.load(e));
                    self.asm.jmp_ne0(ok);
                    self.asm.ffabort(X_ASSERTION);
                    self.asm.place(ok);
                }
            }
            Stmt::Exit { value, .. } => self.do_abort(value.as_ref(), X_EXIT),
            Stmt::Sleep { value, .. } => self.do_abort(value.as_ref(), X_SLEEP),
            Stmt::State { span, .. } => {
                // Automata are not implemented; see the crate docs.
                self.error(86, *span, &["<state>"]);
            }
            Stmt::Emit(e) => self.do_emit(e),
            Stmt::Error(_) => {}
        }
    }

    // ------------------------------------------------------------- locals

    /// `declloc()`: reserve stack space, then initialise.
    fn declloc(&mut self, decl: &VarDecl) {
        for d in &decl.declarators {
            let kind = self.var_kind(d, false);
            let cells = match &kind {
                VarKind::Scalar => 1,
                other => other.total_cells().max(1),
            };
            // `declared += size; sym->addr = -declared * sizeof(cell)` - the slot
            // is addressed from its *low* end, then STK moves down past it.
            self.declared += cells;
            let addr = -self.declared * CELL;
            self.asm.modstk(-cells * CELL);

            if let VarKind::Array(dims) = &kind {
                self.fold_env =
                    std::mem::take(&mut self.fold_env).with_array(&d.name.name, dims.as_slice());
            }
            let tag = self.tag_of(d.tag.as_ref());
            self.fold_env =
                std::mem::take(&mut self.fold_env).with_symbol_tag(&d.name.name, tag.raw() as i32);
            let mut info = VarInfo { addr, class: Class::Local, kind, is_const: false, tag };
            info.is_const = decl.modifiers.is_const;

            // Locals must reach the symbol table too, not just `env`: the folder
            // resolves `sizeof`/`charsmax` through the table, so a local array was
            // reported as an undefined symbol (17) by `sizeof(buf)` even though
            // reading `buf` worked. `charsmax()` expands to `sizeof(x)-1`, so this
            // hit essentially every local string buffer.
            self.table.declare(
                zpc_sema::symbols::SymbolDecl::new(
                    &d.name.name,
                    if info.kind.is_array() {
                        zpc_sema::symbols::SymKind::Array
                    } else {
                        zpc_sema::symbols::SymKind::Variable
                    },
                    d.name.span,
                )
                .with_usage(zpc_sema::symbols::Usage::DEFINED),
            );

            self.env.declare_local(d.name.name.clone(), info.clone());

            if info.kind.is_array() {
                self.init_local_array(d, &info, cells);
            } else {
                self.expression(|g| match &d.init {
                    Some(Init::Expr(e)) => {
                        g.load(e);
                    }
                    // "uninitialized variable, set to zero"
                    _ => g.asm.ldconst(0, Reg::Pri),
                });
                let target = Val::Var(info);
                self.store(&target);
            }
        }
    }

    /// The array half of `declloc()`: zero-fill, then copy the literals in.
    fn init_local_array(&mut self, d: &Declarator, info: &VarInfo, cells: i32) {
        let values = match &d.init {
            Some(init) => self.init_cells(init, info.kind.dims()),
            // An uninitialised multi-dimensional array is still not all zeroes:
            // `initials2()` (sc1.c:2311-2330) reserves and then fills the
            // indirection vectors even when there is no `=`.
            None => crate::layout::indirection_tables(info.kind.dims()),
        };
        if values.len() < cells as usize {
            // fillarray(sym, size, 0)
            self.asm.ldconst(0, Reg::Pri);
            self.address(info, Reg::Alt);
            self.asm.emit1(Opcode::Fill, cells * CELL);
        }
        if values.is_empty() {
            return;
        }
        if values.iter().all(|v| *v == values[0]) {
            // "check whether the complete array is set to a single value; if it
            // is, more compact code can be generated"
            self.asm.ldconst(values[0], Reg::Pri);
            self.address(info, Reg::Alt);
            self.asm.emit1(Opcode::Fill, values.len() as i32 * CELL);
        } else {
            let addr = self.data.literal(&values);
            self.asm.ldconst(addr, Reg::Pri); // source
            self.address(info, Reg::Alt); // destination
            self.asm.memcopy(values.len() as i32 * CELL);
        }
    }

    // --------------------------------------------------------- control flow

    /// `test()` in `sc1.c`: evaluate a condition and branch.
    ///
    /// `jump_if_true` selects `jnz` over `jzer`; `if`/`while` want the false
    /// branch, `assert` wants the true one.
    fn test(&mut self, cond: &Expr, target: LabelId, jump_if_true: bool) {
        // Upstream reports the redundant-test warning from `test()` itself.
        if let Some(c) = self.try_fold(cond) {
            let code = if c.is_zero() { 205 } else { 206 };
            self.error(code, cond.span, &[]);
        }
        self.expression(|g| g.load(cond));
        if jump_if_true {
            self.asm.jmp_ne0(target);
        } else {
            self.asm.jmp_eq0(target);
        }
    }

    fn do_if(&mut self, cond: &Expr, then_branch: &Stmt, else_branch: Option<&Stmt>) {
        let flab1 = self.asm.label();
        self.test(cond, flab1, false);
        self.statement(then_branch);
        match else_branch {
            None => self.asm.place(flab1),
            Some(alt) => {
                let flab2 = self.asm.label();
                self.asm.jumplabel(flab2);
                self.asm.place(flab1);
                self.statement(alt);
                self.asm.place(flab2);
            }
        }
    }

    fn do_while(&mut self, cond: &Expr, body: &Stmt) {
        let frame = self.push_loop();
        self.asm.place(frame.loop_top);
        self.test(cond, frame.exit, false);
        self.statement(body);
        self.asm.jumplabel(frame.loop_top);
        self.asm.place(frame.exit);
        self.loops.pop();
    }

    /// `dodo()`. Note that `continue` jumps to the *test*, not to the top.
    fn do_do(&mut self, body: &Stmt, cond: &Expr) {
        let frame = self.push_loop();
        let top = self.asm.label();
        self.asm.place(top);
        self.statement(body);
        self.asm.place(frame.loop_top);
        self.test(cond, frame.exit, false);
        self.asm.jumplabel(top);
        self.asm.place(frame.exit);
        self.loops.pop();
    }

    /// `dofor()`.
    ///
    /// Expressions 2 and 3 are *reversed* in the generated code: the staging marks
    /// around them make `stgstring()` flush expression 3 first, so the loop body's
    /// back-edge lands on the step and falls through into the test.
    fn do_for(
        &mut self,
        init: Option<&ForInit>,
        cond: Option<&Expr>,
        step: Option<&Expr>,
        body: &Stmt,
    ) {
        let save = self.declared;
        self.env.enter();
        match init {
            Some(ForInit::Decl(v)) => self.declloc(v),
            Some(ForInit::Expr(e)) => self.expression(|g| {
                let v = g.eval(e);
                g.rvalue(&v);
            }),
            None => {}
        }

        // The loop variable of expr1 is *not* unwound by break/continue: the
        // wq[wqBRK]/wq[wqCONT] fields are re-read after expr1 has run.
        let mut frame = self.push_loop();
        frame.declared_brk = self.declared;
        frame.declared_cont = self.declared;
        if let Some(top) = self.loops.last_mut() {
            *top = frame;
        }

        let skiplab = self.asm.label();
        self.asm.jumplabel(skiplab);
        self.asm.place(frame.loop_top);
        if let Some(step) = step {
            self.expression(|g| {
                let v = g.eval(step);
                g.rvalue(&v);
            });
        }
        self.asm.place(skiplab);
        if let Some(cond) = cond {
            self.test(cond, frame.exit, false);
        }
        self.statement(body);
        self.asm.jumplabel(frame.loop_top);
        self.asm.place(frame.exit);
        self.loops.pop();

        self.asm.modstk((self.declared - save) * CELL);
        self.declared = save;
        self.env.leave();
    }

    fn push_loop(&mut self) -> LoopFrame {
        let frame = LoopFrame {
            exit: self.asm.label(),
            loop_top: self.asm.label(),
            declared_brk: self.declared,
            declared_cont: self.declared,
        };
        self.loops.push(frame);
        frame
    }

    fn do_break(&mut self, span: Span) {
        let Some(frame) = self.loops.last().copied() else {
            self.error(24, span, &[]);
            return;
        };
        self.asm.modstk((self.declared - frame.declared_brk) * CELL);
        self.asm.jumplabel(frame.exit);
    }

    fn do_continue(&mut self, span: Span) {
        let Some(frame) = self.loops.last().copied() else {
            self.error(24, span, &[]);
            return;
        };
        self.asm.modstk((self.declared - frame.declared_cont) * CELL);
        self.asm.jumplabel(frame.loop_top);
    }

    /// `doreturn()`: the whole local frame is dropped, then `retn`.
    ///
    /// A function declared as returning an array copies the returned array into a
    /// caller-supplied heap block instead of loading a value into PRI
    /// (`sc1.c:5521-5532`):
    ///
    /// ```c
    /// address(sub,sALT);                /* ALT = destination */
    /// arraysize=calc_arraysize(dim,numdim,0);
    /// memcopy(arraysize*sizeof(cell));  /* source already in PRI */
    /// /* moveto1(); is not necessary, callfunction() does a popreg() */
    /// ```
    ///
    /// `sub` is an `iREFARRAY` at `(argcount+3)*sizeof(cell)` (`sc1.c:5508`), and
    /// `address()` on an `iREFARRAY` is `load.s.alt` regardless of `vclass`
    /// (`sc4.c:423-433`) - the slot holds the heap address, not the array.
    fn do_return(&mut self, value: Option<&Expr>, span: Span) {
        let ret_dims = self.cur_ret_dims.clone();
        if !ret_dims.is_empty() {
            match value {
                Some(e) => self.return_array(e, &ret_dims),
                // "function should return a value" (sc1.c:5537-5541)
                None => self.error(209, span, &["<function>"]),
            }
        } else {
            // Returning an array from a function whose shape could not be
            // determined would compile a `load.i` of the base address, which is
            // silently wrong. `doreturn()` reaches the same conclusion through
            // `error(46)`, "unknown array size" (sc1.c:5490-5491).
            if let Some(e) = value
                && !self.expr_dims(e).is_empty()
            {
                self.error(46, e.span, &[&self.expr_name(e)]);
            }
            self.expression(|g| match value {
                Some(e) => {
                    g.load(e);
                }
                None => g.asm.ldconst(0, Reg::Pri),
            });
        }
        let declared = self.declared;
        self.asm.modstk(declared * CELL);
        self.asm.ffret();
    }

    fn return_array(&mut self, e: &Expr, ret_dims: &[i32]) {
        let dims = self.expr_dims(e);
        let v = self.expression(|g| g.eval(e));
        if !matches!(v, Val::ArrayRef) {
            // "constant array must be assigned to a symbol" / mixing return forms.
            self.error(39, e.span, &[]);
            return;
        }
        if dims != ret_dims {
            // error 47: "array sizes do not match, or destination array is too small"
            self.error(47, e.span, &[]);
        }
        // The hidden parameter sits one slot past the last declared argument.
        self.asm.emit1(Opcode::LoadSAlt, (self.cur_nargs + 3) * CELL);
        self.asm.memcopy(crate::layout::calc_arraysize(ret_dims) * CELL);
    }

    /// `doexit()`/`dosleep()`: value in PRI, tag in ALT, then `halt`.
    ///
    /// The tag is always 0 here because codegen does not carry tags; `exporttag()`
    /// belongs to the tag table, which is a separate phase.
    fn do_abort(&mut self, value: Option<&Expr>, reason: i32) {
        self.expression(|g| match value {
            Some(e) => {
                g.load(e);
            }
            None => g.asm.ldconst(0, Reg::Pri),
        });
        self.asm.ldconst(0, Reg::Alt);
        self.asm.ffabort(reason);
    }

    // ------------------------------------------------------------- switch

    /// `doswitch()`.
    ///
    /// The `switch` instruction's operand is the address of a `casetbl`, which is
    /// emitted *after* every clause and before the exit label. The table is
    /// `2 + 2n` cells: `(n, default_address)` followed by `n` sorted
    /// `(value, address)` pairs, so an abstract machine can binary-search it.
    /// A `case 1 .. 5:` range is *expanded* into one record per value.
    fn do_switch(&mut self, scrutinee: &Expr, cases: &[SwitchCase], default: Option<&Stmt>) {
        self.expression(|g| g.load(scrutinee));
        let table = self.asm.label();
        let exit = self.asm.label();
        self.asm.ffswitch(table);

        let mut records: Vec<(i32, LabelId)> = Vec::new();
        for case in cases {
            let lbl = self.asm.label();
            for label in &case.labels {
                match label {
                    CaseLabel::Single(e) => {
                        let v = self.const_expr(e);
                        self.add_case(&mut records, v, lbl, e.span);
                    }
                    CaseLabel::Range { lo, hi, span } => {
                        let lo_v = self.const_expr(lo);
                        let hi_v = self.const_expr(hi);
                        if hi_v <= lo_v {
                            self.error(50, *span, &[]);
                            continue;
                        }
                        for v in lo_v..=hi_v {
                            self.add_case(&mut records, v, lbl, *span);
                        }
                    }
                }
            }
            self.asm.place(lbl);
            self.statement(&case.body);
            // Pawn cases never fall through.
            self.asm.jumplabel(exit);
        }

        let default_label = match default {
            Some(body) => {
                let lbl = self.asm.label();
                self.asm.place(lbl);
                self.statement(body);
                self.asm.jumplabel(exit);
                lbl
            }
            None => exit,
        };

        records.sort_by_key(|(v, _)| *v);
        self.asm.place(table);
        self.asm.case_table(default_label, records);
        self.asm.place(exit);
    }

    fn add_case(&mut self, records: &mut Vec<(i32, LabelId)>, value: i32, lbl: LabelId, span: Span) {
        if records.iter().any(|(v, _)| *v == value) {
            self.error(40, span, &[&value.to_string()]);
            return;
        }
        records.push((value, lbl));
    }

    // ------------------------------------------------------------- goto

    fn goto_label(&mut self, name: &Ident) -> LabelId {
        if let Some(l) = self.goto_labels.get(&name.name) {
            return *l;
        }
        let l = self.asm.label();
        self.goto_labels.insert(name.name.clone(), l);
        l
    }

    // ------------------------------------------------------------- #emit

    /// `#emit` writes the mnemonic straight to the assembly stream without any
    /// validation in `sc2.c`; here the mnemonic must at least name a real opcode,
    /// because the stream is structured. An unknown one is fatal error 104,
    /// which is what `sc6.c` raises when it fails to assemble the line.
    fn do_emit(&mut self, e: &EmitStmt) {
        let text = e.opcode.name.to_ascii_lowercase();
        let Some(opcode) = zpc_asm::opcode::ALL.iter().find(|o| o.mnemonic() == text).copied()
        else {
            self.error(104, e.opcode.span, &[&e.opcode.name]);
            return;
        };
        match &e.operand {
            None => self.asm.emit0(opcode),
            Some(EmitOperand::Int { value, .. }) => self.asm.emit1(opcode, *value as i32),
            Some(EmitOperand::Rational { value, .. }) => {
                self.asm.emit1(opcode, (*value as f32).to_bits() as i32);
            }
            Some(EmitOperand::Symbol(id)) => {
                // "A symbol operand is replaced by the symbol's address."
                match self.env.var(&id.name).map(|v| v.addr) {
                    Some(addr) => self.asm.emit1(opcode, addr),
                    None => {
                        self.error(17, id.span, &[&id.name]);
                        self.asm.emit1(opcode, 0);
                    }
                }
            }
        }
    }
}
