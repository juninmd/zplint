//! The code generator: declarations, functions and the shared state that the
//! expression and statement emitters in [`crate::expr`] and [`crate::stmt`] drive.
//!
//! # The AMX machine model
//!
//! Two general registers, PRI and ALT, plus four pointers: FRM (frame base), STK
//! (stack pointer, growing *down*), HEA (heap pointer, growing up) and CIP (the
//! instruction pointer). Every expression leaves its result in PRI; ALT is the
//! scratch register and holds the *left* operand of a binary operator (`sc4.c`'s
//! `ob_sub` is `sub.alt`, i.e. `PRI = ALT - PRI`).
//!
//! One call frame, as laid out by `callfunction()` in `sc3.c` and documented above
//! `load_argcount()` in `sc4.c`:
//!
//! ```text
//!   FRM + 0             previous FRM        (pushed by `proc`)
//!   FRM + 1*cell        return address      (pushed by `call`)
//!   FRM + 2*cell        argument count in bytes
//!   FRM + 3*cell        first argument
//!   ...
//!   FRM - n*cell        locals, allocated downwards by `stack -n*cell`
//! ```
//!
//! So arguments are at *positive* offsets `(i+3)*cell` from FRM (`sc1.c`
//! `declargs()`, which passes `(argcnt+3)*sizeof(cell)` to `doarg()`) and locals at
//! *negative* offsets `-declared*cell` (`sc1.c` `declloc()`).

use std::collections::HashMap;
use std::path::PathBuf;

use zpc_asm::Opcode;
use zpc_ast::decl::{
    ConstDecl, Declarator, EnumDecl, EnumStepOp, FuncDecl, FuncKind, FuncName, Init, InitList, Item,
    Param as AstParam, ParamDefault, VarDecl,
};
use zpc_ast::expr::{Expr, ExprKind, StringLit};
use zpc_ast::{Program, Span};
use zpc_diag::Diagnostics;
use zpc_sema::fold::{ArrayInfo, Cell, Const, Folder, MapEnv, TagConfig};
use zpc_sema::symbols::{SymKind, SymbolDecl, SymbolTable};

use crate::layout::{Callee, Class, DataSeg, Env, FuncInfo, Param, ParamKind, VarInfo, VarKind};
use crate::stream::{AsmStream, CELL, Item as AsmItem, LabelId, Reg};

/// One entry of the loop stack: where `break` and `continue` jump, and how much
/// stack each must give back. `wq[wqEXIT]`/`wq[wqLOOP]`/`wq[wqBRK]`/`wq[wqCONT]` in
/// `sc1.c`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopFrame {
    pub exit: LabelId,
    pub loop_top: LabelId,
    /// `declared` at the point `break` unwinds to, in cells.
    pub declared_brk: i32,
    /// `declared` at the point `continue` unwinds to, in cells.
    pub declared_cont: i32,
}

/// A finished translation unit.
pub struct Unit {
    /// The assembly stream, labels still symbolic.
    pub code: Vec<AsmItem>,
    /// The data segment, in cells.
    pub data: Vec<i32>,
    /// Native functions in the order their `sysreq.c` indices were handed out
    /// (`ntv_funcid++` in `ffcall()`).
    pub natives: Vec<String>,
    /// Public functions and the label of their `proc`.
    pub publics: Vec<(String, LabelId)>,
    pub diags: Diagnostics,
}

impl Unit {
    /// Resolve labels and encode the code segment.
    pub fn assemble(&self) -> Result<Vec<u8>, crate::stream::AsmError> {
        crate::stream::assemble(&self.code)
    }
}

/// Shared state for one translation unit.
pub struct Generator {
    pub(crate) asm: AsmStream,
    pub(crate) data: DataSeg,
    pub(crate) env: Env,
    pub(crate) diags: Diagnostics,
    pub(crate) file: PathBuf,
    /// Backing store for [`Folder`]. Codegen never resolves names through it - the
    /// folder does, for `defined` and for rejecting non-constant symbols.
    pub(crate) table: SymbolTable,
    pub(crate) fold_env: MapEnv,
    pub(crate) tags: TagConfig,

    /// Cells of locals allocated in the current function, `declared` in `sc1.c`.
    pub(crate) declared: i32,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) goto_labels: HashMap<String, LabelId>,
    /// Native names in the order their `sysreq.c` indices were handed out.
    pub(crate) natives: Vec<String>,
    pub(crate) publics: Vec<(String, LabelId)>,
}

impl Generator {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        Self {
            asm: AsmStream::new(),
            data: DataSeg::new(),
            env: Env::new(),
            diags: Diagnostics::new(),
            table: SymbolTable::new(file.clone()),
            file,
            fold_env: MapEnv::new(),
            tags: TagConfig::default(),
            declared: 0,
            loops: Vec::new(),
            goto_labels: HashMap::new(),
            natives: Vec::new(),
            publics: Vec::new(),
        }
    }

    // ------------------------------------------------------------ diagnostics

    pub(crate) fn error(&mut self, code: u16, span: Span, args: &[&str]) {
        let file = self.file.clone();
        self.diags.emit(code, span, &file, args);
    }

    /// Run the semantic folder over an expression. Diagnostics it raises (undefined
    /// symbol, `sizeof` of a non-array, ...) are merged into ours.
    pub(crate) fn fold(&mut self, expr: &Expr) -> Option<Const> {
        let mut diags = std::mem::take(&mut self.diags);
        let result = {
            let folder = Folder::new(&self.table, &self.fold_env).with_tags(self.tags);
            folder.fold(expr, &mut diags)
        };
        self.diags = diags;
        result
    }

    /// `constexpr()`: demand a constant, error 8 otherwise.
    pub(crate) fn const_expr(&mut self, expr: &Expr) -> Cell {
        match self.fold(expr) {
            Some(c) => c.value,
            None => {
                self.error(8, expr.span, &[]);
                0
            }
        }
    }

    // ------------------------------------------------------------ entry point

    /// Emit a whole program and consume the generator.
    pub fn program(mut self, program: &Program) -> Unit {
        // writeleader(): "When a subroutine returns to address 0, the AMX must
        // halt", so address 0 always holds `halt 0` (sc4.c).
        self.asm.ffabort(0);

        self.collect(program);
        for item in &program.items {
            self.item(item);
        }

        Unit {
            code: self.asm.into_items(),
            data: self.data.cells().to_vec(),
            natives: self.natives,
            publics: self.publics,
            diags: self.diags,
        }
    }

    /// Pre-pass: give every function a label (or native index) and register every
    /// global name, so forward references resolve. The C compiler gets this for
    /// free by running the parser twice (`sc_status == statFIRST` then `statWRITE`).
    fn collect(&mut self, program: &Program) {
        for item in &program.items {
            match item {
                Item::Func(f) => self.collect_func(f),
                Item::Var(v) => self.collect_global_var(v),
                Item::Const(c) => self.collect_const(c),
                Item::Enum(e) => self.collect_enum(e),
                _ => {}
            }
        }
    }

    fn collect_func(&mut self, f: &FuncDecl) {
        let FuncName::Ident(name) = &f.name else {
            // Operator overloads (`operator+(...)`) are mangled into ordinary
            // symbols by `operator_symname()` in sc3.c and then dispatched by
            // `check_userop()`. Neither is implemented; see the crate docs.
            self.error(7, f.span, &[]);
            return;
        };

        let params: Vec<Param> = f
            .params
            .iter()
            .map(|p| match p {
                AstParam::Fixed(d) => {
                    let kind = if !d.dims.is_empty() {
                        ParamKind::Array
                    } else if d.by_ref {
                        ParamKind::Reference
                    } else {
                        ParamKind::Value
                    };
                    // Only a scalar default can be materialised with `ldconst`;
                    // array, `sizeof` and `tagof` defaults are not implemented.
                    let default = match d.default.as_ref() {
                        Some(ParamDefault::Expr(e)) => self.fold(e).map(|c| c.value),
                        _ => None,
                    };
                    Param { name: d.name.name.clone(), kind, default }
                }
                AstParam::Rest(_) => {
                    Param { name: String::new(), kind: ParamKind::VarArgs, default: None }
                }
            })
            .collect();
        let variadic = matches!(f.params.last(), Some(AstParam::Rest(_)));

        let callee = match f.kind {
            FuncKind::Native => {
                // ffcall(): "reserve a SYSREQ id if called for the first time",
                // `sym->addr = ntv_funcid++`. Indices are handed out in
                // declaration order here, which is deterministic and matches
                // amxxpc for the common case of one native table per file.
                let idx = self.natives.len() as i32;
                self.natives.push(name.name.clone());
                Callee::Native(idx)
            }
            _ => {
                if let Some(existing) = self.env.func(&name.name) {
                    existing.callee.clone()
                } else {
                    Callee::Func(self.asm.label())
                }
            }
        };

        if f.body.is_some()
            && (f.modifiers.public || name.is_implicitly_public())
            && let Callee::Func(l) = callee
        {
            self.publics.push((name.name.clone(), l));
        }

        self.table.declare(SymbolDecl::new(&name.name, SymKind::Function, name.span));
        self.env.declare_func(name.name.clone(), FuncInfo { callee, params, variadic });
    }

    fn collect_const(&mut self, c: &ConstDecl) {
        let value = self.const_expr(&c.value);
        self.define_const(&c.name.name, value, c.name.span);
    }

    fn collect_enum(&mut self, e: &EnumDecl) {
        // doenum() in sc1.c: members take successive values, advanced by the
        // step expression (`enum (<<= 1)`), and a sized member `Field[3]`
        // advances by its size.
        let mut next: Cell = 0;
        let step = e.step.as_ref().map(|s| (s.op, self.const_expr(&s.value)));
        for m in &e.members {
            let value = match &m.value {
                Some(v) => self.const_expr(v),
                None => next,
            };
            self.define_const(&m.name.name, value, m.name.span);
            let size = match &m.size {
                Some(s) => self.const_expr(s),
                None => 1,
            };
            // A sized member is also an enum *field*: `sizeof arr[Field]`.
            self.fold_env = std::mem::take(&mut self.fold_env).with_field(&m.name.name, size, 0);
            next = match step {
                None => value.wrapping_add(size),
                Some((EnumStepOp::Add, s)) => value.wrapping_add(s),
                Some((EnumStepOp::Mult, s)) => value.wrapping_mul(s),
                Some((EnumStepOp::Shl, s)) => ((value as u32) << (s as u32 & 31)) as i32,
            };
        }
    }

    pub(crate) fn define_const(&mut self, name: &str, value: Cell, span: Span) {
        self.table.declare(SymbolDecl::new(name, SymKind::Constant, span));
        self.fold_env = std::mem::take(&mut self.fold_env).with_const(name, value, 0);
        self.env.declare_const(name, value);
    }

    // ------------------------------------------------------- global variables

    fn collect_global_var(&mut self, v: &VarDecl) {
        for d in &v.declarators {
            let kind = self.var_kind(d, false);
            let cells = match &kind {
                VarKind::Scalar => 1,
                other => other.total_cells().max(1),
            };
            let addr = self.data.alloc(cells);
            if let VarKind::Array(dims) = &kind {
                self.fold_env =
                    std::mem::take(&mut self.fold_env).with_array(&d.name.name, dims.as_slice());
            }
            self.table.declare(SymbolDecl::new(
                &d.name.name,
                if kind.is_array() { SymKind::Array } else { SymKind::Variable },
                d.name.span,
            ));
            let mut info = VarInfo::global(addr, kind);
            info.is_const = v.modifiers.is_const;
            self.env.declare_global(d.name.name.clone(), info);

            // Global initialisers are written straight into the data segment;
            // no code is generated (declglb() in sc1.c fills litq).
            if let Some(init) = &d.init {
                let dims =
                    self.env.var(&d.name.name).map(|v| v.kind.dims().to_vec()).unwrap_or_default();
                let values = self.init_cells(init, &dims);
                self.data.init_at(addr, &values);
            }
        }
    }

    /// The declared shape of a variable: dimensions folded to constants.
    ///
    /// A dimension with no size (`msg[]`) is 0, which upstream also uses to mean
    /// "unknown"; `ffbounds()` and error 32 are then skipped, matching the
    /// `sym->dim.array.length != 0` guards in `hier1()`.
    pub(crate) fn var_kind(&mut self, d: &Declarator, by_ref: bool) -> VarKind {
        if d.dims.is_empty() {
            return if by_ref { VarKind::Reference } else { VarKind::Scalar };
        }
        let mut dims: Vec<i32> = d
            .dims
            .iter()
            .map(|dim| match &dim.size {
                Some(e) => self.const_expr(e),
                None => 0,
            })
            .collect();
        // `new a[] = {1,2,3}` deduces the last dimension from the initialiser.
        if let (Some(0), Some(init)) = (dims.last().copied(), d.init.as_ref()) {
            let n = self.init_len(init);
            if let Some(last) = dims.last_mut() {
                *last = n;
            }
        }
        VarKind::Array(dims)
    }

    fn init_len(&mut self, init: &Init) -> i32 {
        match init {
            Init::List(l) => l.elems.len() as i32,
            Init::Expr(e) => match &e.kind {
                // A string initialiser sizes the array including its terminator.
                ExprKind::Str(s) => self.string_cells(s).len() as i32,
                ExprKind::LitArray { elems, .. } => elems.len() as i32,
                _ => 1,
            },
        }
    }

    /// Flatten an initialiser to cells. Multi-dimensional initialisers are laid out
    /// row-major *without* the leading index vector, which is the part of
    /// `initarray()` that is not implemented - see the crate docs.
    pub(crate) fn init_cells(&mut self, init: &Init, dims: &[i32]) -> Vec<i32> {
        match init {
            Init::Expr(e) => match &e.kind {
                ExprKind::Str(s) => self.string_cells(s),
                ExprKind::LitArray { elems, .. } => {
                    elems.iter().map(|e| self.const_expr(e)).collect()
                }
                _ => vec![self.const_expr(e)],
            },
            Init::List(list) => self.init_list_cells(list, dims),
        }
    }

    fn init_list_cells(&mut self, list: &InitList, dims: &[i32]) -> Vec<i32> {
        let inner = dims.split_first().map(|(_, rest)| rest).unwrap_or(&[]);
        let mut out = Vec::new();
        for elem in &list.elems {
            out.extend(self.init_cells(elem, inner));
        }
        // `{1, 3, ...}` EXTRAPOLATES: it does not repeat the last value. `initvector()`
        // (sc1.c) keeps the previous two values and computes `step = prev1 - prev2`,
        // then fills with `prev1 += step`. So `{1, 3, ...}` continues 5, 7, 9 and
        // `{0, ...}` (one value, step 0) repeats 0 - which is why the repeat-the-last
        // reading looks right until a second value appears.
        if list.fill_rest
            && let Some(&want) = dims.first()
            && let Some(&last) = out.last()
        {
            let want = if inner.is_empty() { want } else { want * inner.iter().product::<i32>() };
            // With a single element there is no previous value, so the step is 0.
            let step = match out.len() {
                0 => 0,
                1 => 0,
                n => last - out[n - 2],
            };
            let mut value = last;
            while (out.len() as i32) < want {
                value = value.wrapping_add(step);
                out.push(value);
            }
        }
        out
    }

    /// Decode a string literal into cells.
    ///
    /// An unpacked string is one character per cell plus a `\0` terminator; a packed
    /// string (`!"..."`) is `sizeof(cell)/sCHARBITS*8` = 4 characters per cell, most
    /// significant byte first ("the first character in a pack occupies the highest
    /// bits of the cell", `charalign()` in `sc4.c`).
    pub(crate) fn string_cells(&self, s: &StringLit) -> Vec<i32> {
        if s.packed {
            let bytes: Vec<u8> = s.value.bytes().chain(std::iter::once(0)).collect();
            bytes
                .chunks(4)
                .map(|c| {
                    let mut cell = 0u32;
                    for (i, b) in c.iter().enumerate() {
                        cell |= u32::from(*b) << (24 - 8 * i);
                    }
                    cell as i32
                })
                .collect()
        } else {
            s.value.chars().map(|c| c as i32).chain(std::iter::once(0)).collect()
        }
    }

    // ------------------------------------------------------------- top level

    fn item(&mut self, item: &Item) {
        match item {
            Item::Func(f) => self.function(f),
            // Globals, constants and enums were fully handled by the pre-pass;
            // they generate no code.
            Item::Var(_) | Item::Const(_) | Item::Enum(_) => {}
            Item::Pragma(_) | Item::Error(_) => {}
        }
    }

    /// `newfunc()` in `sc1.c`: the prologue, the body, and the implicit
    /// `zero.pri; retn` that closes a function whose last statement was not a
    /// `return`.
    fn function(&mut self, f: &FuncDecl) {
        let Some(body) = &f.body else { return };
        let FuncName::Ident(name) = &f.name else { return };
        let Some(FuncInfo { callee: Callee::Func(label), .. }) = self.env.func(&name.name).cloned()
        else {
            return;
        };

        self.asm.place(label);
        self.asm.emit0(Opcode::Proc); // startfunc(): "creates stack frame"
        self.declared = 0;
        self.goto_labels.clear();
        self.env.enter();
        self.bind_params(f);

        self.block_body(body);

        // Even a single-statement body may have declared a variable (sc1.c
        // deliberately handles "this very special (and useless) case").
        let declared = self.declared;
        self.asm.modstk(declared * CELL);
        self.declared = 0;
        self.env.leave();

        // `if ((lastst!=tRETURN) && (lastst!=tGOTO)) { ldconst(0,sPRI); ffret(); }`
        // The unconditional emission here costs a dead `zero.pri; retn` after a
        // trailing `return`; the peephole pass removes it, exactly as upstream's
        // `lastst` tracking would.
        self.asm.ldconst(0, Reg::Pri);
        self.asm.ffret();
    }

    /// `define_args()`: argument `i` lives at `(i+3)*cell` from FRM.
    fn bind_params(&mut self, f: &FuncDecl) {
        for (i, p) in f.params.iter().enumerate() {
            let AstParam::Fixed(d) = p else { continue };
            let addr = (i as i32 + 3) * CELL;
            let kind = if d.dims.is_empty() {
                if d.by_ref { VarKind::Reference } else { VarKind::Scalar }
            } else {
                let decl = Declarator {
                    name: d.name.clone(),
                    tag: None,
                    dims: d.dims.clone(),
                    init: None,
                    span: d.span,
                };
                VarKind::RefArray(self.var_kind(&decl, false).dims().to_vec())
            };
            if kind.is_array() {
                self.fold_env = std::mem::take(&mut self.fold_env)
                    .with_array(&d.name.name, &ArrayInfo { dims: kind.dims().to_vec() }.dims);
            }
            self.table.declare(SymbolDecl::new(
                &d.name.name,
                if kind.is_array() { SymKind::Array } else { SymKind::Variable },
                d.name.span,
            ));
            let mut info = VarInfo { addr, class: Class::Local, kind, is_const: d.is_const };
            info.is_const = d.is_const;
            self.env.declare_local(d.name.name.clone(), info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    #[test]
    fn unpacked_strings_get_a_terminator_cell() {
        let g = Generator::new("t.sma");
        let s = StringLit { value: "hi".into(), packed: false, raw: false, span: sp() };
        assert_eq!(g.string_cells(&s), vec![b'h' as i32, b'i' as i32, 0]);
    }

    #[test]
    fn packed_strings_put_the_first_character_in_the_high_byte() {
        let g = Generator::new("t.sma");
        let s = StringLit { value: "abcd".into(), packed: true, raw: false, span: sp() };
        // "abcd" + NUL = 5 bytes -> 2 cells, the second holding only the NUL.
        assert_eq!(g.string_cells(&s), vec![0x6162_6364u32 as i32, 0]);
    }
}
