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
    NativeAlias, OverloadableOp, Param as AstParam, ParamDefault, VarDecl,
};
use zpc_ast::expr::{Expr, ExprKind, StringLit};
use zpc_ast::{Program, Span, TagRef};
use zpc_diag::Diagnostics;
use zpc_sema::fold::{ArrayInfo, Cell, Const, Folder, MapEnv, TagConfig};
use zpc_sema::symbols::{SymKind, SymbolDecl, SymbolTable, Usage};
use zpc_sema::tags::{OpKind, Overload, Overloads, TagId, Tags};

use crate::layout::{ArgDefault, Callee, Class, DataSeg, Env, FuncInfo, Param, ParamKind, VarInfo, VarKind};
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
    /// `tagname_tab`: every tag mentioned in this unit, interned.
    pub(crate) tag_tab: Tags,
    /// The reverse of [`Generator::tag_tab`]'s interning, so that a raw tag id
    /// coming back out of the folder can be turned into a [`TagId`] again.
    pub(crate) tag_by_raw: HashMap<i32, TagId>,
    /// The declared `operator` overloads, keyed exactly as `findglb()` keys them.
    /// The callable side of each one lives in `env` under the same mangled name.
    pub(crate) ops: Overloads,
    /// The mangled name `collect_func()` gave each `operator` declaration, keyed
    /// by the declaration's span so that the emit pass can find it again.
    pub(crate) op_names: HashMap<(u32, u32), String>,
    /// The mangled name of the operator whose body is being emitted, if any.
    /// `check_userop()` refuses to dispatch to `sym == curfunc`, which is what
    /// stops `Float:operator+(Float:a, Float:b) return a + b` recursing.
    pub(crate) cur_op: Option<String>,

    /// Cells of locals allocated in the current function, `declared` in `sc1.c`.
    pub(crate) declared: i32,
    /// Heap cells claimed by the expression currently being emitted, `decl_heap`
    /// in `sc3.c`. `expression()` (`sc3.c:674-683`) gives them back at the end of
    /// every full expression: "scrap any arrays left on the heap".
    pub(crate) decl_heap: i32,
    /// The number of declared parameters and the return dimensions of the
    /// function being emitted - what `doreturn()` needs to find the hidden
    /// destination parameter.
    pub(crate) cur_nargs: i32,
    pub(crate) cur_ret_dims: Vec<i32>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) goto_labels: HashMap<String, LabelId>,
    /// Native names in the order their `sysreq.c` indices were handed out.
    pub(crate) natives: Vec<String>,
    pub(crate) publics: Vec<(String, LabelId)>,
}

impl Generator {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let mut g = Self {
            asm: AsmStream::new(),
            data: DataSeg::new(),
            env: Env::new(),
            diags: Diagnostics::new(),
            table: SymbolTable::new(file.clone()),
            file,
            fold_env: MapEnv::new(),
            tags: TagConfig::default(),
            tag_tab: Tags::new(),
            tag_by_raw: HashMap::new(),
            ops: Overloads::new(),
            op_names: HashMap::new(),
            cur_op: None,
            declared: 0,
            decl_heap: 0,
            cur_nargs: 0,
            cur_ret_dims: Vec::new(),
            loops: Vec::new(),
            goto_labels: HashMap::new(),
            natives: Vec::new(),
            publics: Vec::new(),
        };
        // `#pragma rational Float` in float.inc. Registering it up front is what
        // stops the folder from constant-folding `1.0 + 2.0` into an *integer*
        // add of two bit patterns: `fold_binary()` bails out on the rational tag
        // precisely because `check_userop()` would claim the expression.
        for name in ["bool", "Float", "String", "any"] {
            g.intern_tag(name);
        }
        g.tags = TagConfig {
            bool_tag: g.tag_tab.bool_tag().raw() as i32,
            rational_tag: g.tag_tab.rational_tag().raw() as i32,
            rational_digits: 0,
        };
        g
    }

    // ------------------------------------------------------------------ tags

    /// `pc_addtag()`, plus the two side tables codegen needs: the raw-id reverse
    /// map and the folder's tag environment (which resolves `Float:x` casts).
    pub(crate) fn intern_tag(&mut self, name: &str) -> TagId {
        let id = self.tag_tab.add(name);
        self.tag_by_raw.insert(id.raw() as i32, id);
        self.fold_env = std::mem::take(&mut self.fold_env).with_tag(name, id.raw() as i32);
        id
    }

    /// The tag a declaration ascribes: `None` and `_:` are both untagged.
    pub(crate) fn tag_of(&mut self, tag: Option<&TagRef>) -> TagId {
        match tag {
            None => TagId::UNTAGGED,
            Some(t) => self.intern_tag(&t.name.name),
        }
    }

    /// Turn a raw tag id (as the folder reports it) back into a [`TagId`].
    pub(crate) fn tag_from_raw(&self, raw: i32) -> TagId {
        self.tag_by_raw.get(&raw).copied().unwrap_or(TagId::UNTAGGED)
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

    /// `expression()` in `sc3.c`:
    ///
    /// ```c
    /// int locheap=decl_heap;
    /// if (hier14(&lval)) rvalue(&lval);
    /// /* scrap any arrays left on the heap */
    /// modheap((locheap-decl_heap)*sizeof(cell));
    /// decl_heap=locheap;
    /// ```
    ///
    /// Every full expression must be wrapped in this, or the heap block an
    /// array-returning call reserves is never given back. `modheap(0)` emits
    /// nothing, so wrapping an expression that claims no heap is free.
    pub(crate) fn expression<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let locheap = self.decl_heap;
        let r = f(self);
        self.asm.modheap((locheap - self.decl_heap) * CELL);
        self.decl_heap = locheap;
        r
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
        self.setconstants();
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

    /// `setconstants()` (sc1.c:1513): the constants the compiler defines before
    /// reading a single line of source. Without these, `cellmin` and friends are
    /// undefined symbols - and they are used inside `float.inc`'s own operator
    /// bodies, so every plugin that touches floats fails.
    ///
    /// Values are for the 32-bit cell build with `sCHARBITS == 8`, which is the
    /// only configuration AMX Mod X ships.
    fn setconstants(&mut self) {
        let bool_tag = self.intern_tag("bool");
        let span = Span::default();

        // `true`/`false` carry the bool tag; the rest are untagged.
        self.define_const_tagged("true", 1, bool_tag, span);
        self.define_const_tagged("false", 0, bool_tag, span);

        for (name, value) in [
            ("EOS", 0),
            ("cellbits", 32),
            ("cellmax", i32::MAX),
            ("cellmin", i32::MIN),
            ("charbits", 8),
            ("charmin", 0),
            // `~(-1UL << sCHARBITS) - 1` = 0xFF - 1
            ("charmax", 254),
            // `(1 << (sizeof(cell)-1)*8) - 1` = (1 << 24) - 1
            ("ucharmax", 16_777_215),
            // `debug` reflects the -d level; a release build compiles with 0.
            ("debug", 0),
            // `__LINE__` is seeded to 0 here and rewritten per line by the
            // preprocessor, which already substitutes it textually.
            ("__LINE__", 0),
        ] {
            self.define_const_tagged(name, value, TagId::UNTAGGED, span);
        }
    }

    fn collect_func(&mut self, f: &FuncDecl) {
        // `operatoradjust()` (sc1.c:3024) renames an operator overload to the
        // mangled name `operator_symname()` builds out of its argument tags;
        // from here on it is an ordinary global function that only
        // `check_userop()` ever names.
        let (name, span, is_op) = match &f.name {
            FuncName::Ident(id) => (id.name.clone(), id.span, false),
            FuncName::Operator { op, span } => match self.declare_operator(f, *op, *span) {
                Some(mangled) => (mangled, *span, true),
                None => return,
            },
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
                    // A scalar default is an `ldconst` of its value; an ARRAY
                    // default is an `ldconst` of the *address* of a copy parked in
                    // the data segment, which is what `declargs()` does with it.
                    // Without the array case, `stock f(a, const s[] = "")` counted
                    // `s` as required and every `f(1)` raised error 88 - the AMXX
                    // headers are full of exactly that shape.
                    let default = match d.default.as_ref() {
                        Some(ParamDefault::Expr(e)) => match &e.kind {
                            ExprKind::Str(s) => {
                                let cells = self.string_cells(s);
                                Some(ArgDefault::Const(self.intern_literal(&cells)))
                            }
                            _ => self.fold(e).map(|c| ArgDefault::Const(c.value)),
                        },
                        Some(ParamDefault::Array(init)) => {
                            let cells = self.init_cells(init, &[]);
                            Some(ArgDefault::Const(self.intern_literal(&cells)))
                        }
                        Some(ParamDefault::Symbol(id)) => {
                            // "the address of an existing global array is used
                            // directly" - no copy is made.
                            self.env.var(&id.name).map(|v| ArgDefault::Const(v.addr))
                        }
                        // `= sizeof other` names another PARAMETER and is resolved
                        // per call site; record which one.
                        Some(ParamDefault::SizeOf { arg, levels, .. }) => f
                            .params
                            .iter()
                            .position(|p| {
                                matches!(p, AstParam::Fixed(o) if o.name.name == arg.name)
                            })
                            .map(|index| ArgDefault::SizeOfArg { index, levels: *levels }),
                        // `= tagof other` is not implemented, so the parameter
                        // stays required rather than silently taking a wrong value.
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
                // A native operator has no legal exported name of its own, so
                // `native Float:operator*(...) = floatmul;` must register
                // `floatmul` in the native table (`funcstub()`, sc1.c).
                let exported = match (is_op, &f.alias) {
                    (true, Some(NativeAlias::Symbol(a))) => a.name.clone(),
                    _ => name.clone(),
                };
                self.natives.push(exported);
                Callee::Native(idx)
            }
            _ => {
                if let Some(existing) = self.env.func(&name) {
                    existing.callee.clone()
                } else {
                    Callee::Func(self.asm.label())
                }
            }
        };

        let implicitly_public = match &f.name {
            FuncName::Ident(id) => id.is_implicitly_public(),
            FuncName::Operator { .. } => false,
        };
        if f.body.is_some()
            && (f.modifiers.public || implicitly_public)
            && let Callee::Func(l) = callee
        {
            self.publics.push((name.clone(), l));
        }

        // `funcstub()` (sc1.c:3242-3258) reads the `[n]` list that sits between the
        // return tag and the name (`native Float:[3] make_vec();`). Every dimension
        // must be known: `if (size==0) error(9)`.
        let mut ret_dims = Vec::new();
        for dim in &f.return_dims {
            let n = match &dim.size {
                Some(e) => self.const_expr(e),
                None => 0,
            };
            if n <= 0 {
                self.error(9, dim.span, &[]);
                ret_dims.clear();
                break;
            }
            ret_dims.push(n);
        }
        // `newfunc()` has no return-dimension loop, so a *definition* gets its
        // shape from `doreturn()` instead: "this function does not yet have an
        // array attached; clone the returned symbol beneath the current function"
        // (sc1.c:5473-5508). Upstream can do that during the write pass because
        // it re-parses (`sc_reparse=TRUE`, sc1.c:5518) when the function was
        // already called; the single pass here has to infer it up front.
        //
        // A definition that follows a `forward`/`native` keeps the *declared*
        // shape, so that `doreturn()` can compare the returned array against it
        // and raise error 47 on a mismatch (sc1.c:5450-5472).
        if ret_dims.is_empty()
            && let Some(existing) = self.env.func(&name)
        {
            ret_dims = existing.ret_dims.clone();
        }
        if ret_dims.is_empty() {
            ret_dims = self.infer_return_dims(f);
        }

        let ret_tag = self.tag_of(f.return_tag.as_ref());
        // A `forward` followed by a definition must end up defined; a definition
        // followed by a prototype must stay defined.
        let defined = f.body.is_some()
            || f.kind == FuncKind::Native
            || self.env.func(&name).is_some_and(|e| e.defined);

        self.table.declare(SymbolDecl::new(&name, SymKind::Function, span));
        self.env.declare_func(
            name,
            FuncInfo { callee, params, variadic, ret_dims, ret_tag, defined },
        );
    }

    /// `operatoradjust()` (`sc1.c:3024`): validate the shape of an `operator`
    /// declaration, record the overload, and return the mangled symbol name the
    /// function is stored under.
    ///
    /// `None` means the form is rejected; the diagnostic has been raised.
    fn declare_operator(
        &mut self,
        f: &FuncDecl,
        op: OverloadableOp,
        span: Span,
    ) -> Option<String> {
        // `operator=` is a coercion hook consulted on every assignment,
        // initialisation and by-value argument pass, and `operator~` is the
        // array destructor. Neither is dispatched by this port, so registering
        // them would be worse than refusing them: error 7 stands for those two
        // forms only. Nothing in the AMX Mod X headers declares either.
        if matches!(op, OverloadableOp::Assign | OverloadableOp::BitNot) {
            self.error(7, span, &[]);
            return None;
        }

        // "count arguments and save (first two) tags"
        let mut tags = [TagId::UNTAGGED; 2];
        let mut count = 0usize;
        for p in &f.params {
            let AstParam::Fixed(d) = p else {
                // `...` cannot appear on an operator; `arg->ident != iVARIABLE`.
                self.error(66, span, &[]);
                return None;
            };
            if count < 2 {
                match d.tags.as_ref().map(|t| t.tags.as_slice()) {
                    Some([one]) => tags[count] = self.intern_tag(&one.name.name),
                    Some([]) | None => {}
                    // "function argument may only have a single tag"
                    Some(_) => self.error(65, d.span, &[&(count + 1).to_string()]),
                }
            }
            if d.by_ref || !d.dims.is_empty() {
                self.error(66, d.span, &[&d.name.name]);
                return None;
            }
            if d.default.is_some() {
                self.error(59, d.span, &[&d.name.name]);
            }
            count += 1;
        }

        let kind = match (op, count) {
            (OverloadableOp::Add, 2) => OpKind::Add,
            (OverloadableOp::Sub, 2) => OpKind::Sub,
            (OverloadableOp::Sub, 1) => OpKind::Neg,
            (OverloadableOp::Mul, 2) => OpKind::Mul,
            (OverloadableOp::Div, 2) => OpKind::Div,
            (OverloadableOp::Mod, 2) => OpKind::Mod,
            (OverloadableOp::Gt, 2) => OpKind::Gt,
            (OverloadableOp::Lt, 2) => OpKind::Lt,
            (OverloadableOp::Ge, 2) => OpKind::Ge,
            (OverloadableOp::Le, 2) => OpKind::Le,
            (OverloadableOp::Eq, 2) => OpKind::Eq,
            (OverloadableOp::Ne, 2) => OpKind::Ne,
            (OverloadableOp::Not, 1) => OpKind::LogNot,
            (OverloadableOp::Inc, 1) => OpKind::Inc,
            (OverloadableOp::Dec, 1) => OpKind::Dec,
            // `=` and `~` were rejected above; anything else reaching here has
            // the wrong number of operands.
            // "number or placement of the operands does not fit the operator"
            _ => {
                self.error(62, span, &[]);
                return None;
            }
        };

        let rhs = (count == 2).then_some(tags[1]);
        // "cannot change predefined operators": an overload on untagged
        // operands could never be selected anyway (`check_userop()`'s quick
        // exit), so it would silently do nothing.
        if tags[0].is_untagged() && rhs.is_none_or(TagId::is_untagged) {
            self.error(64, span, &[]);
            return None;
        }

        let result = self.tag_of(f.return_tag.as_ref());
        self.ops.declare(Overload { kind, lhs: tags[0], rhs, result });
        let mangled = Overloads::mangle(kind, tags[0], rhs, tags[0]);
        self.op_names.insert((f.span.start, f.span.end), mangled.clone());
        Some(mangled)
    }

    /// The mangled name [`Generator::declare_operator`] recorded for this
    /// declaration.
    fn operator_name(&self, f: &FuncDecl) -> Option<String> {
        self.op_names.get(&(f.span.start, f.span.end)).cloned()
    }

    /// The shape `doreturn()` would clone beneath the function symbol: the
    /// dimensions of the array named by the first `return <name>;` in the body.
    ///
    /// Only a plain identifier naming an unambiguously declared array is
    /// accepted. `doreturn()` insists on a symbol as well ("returning a literal
    /// string is not supported (it must be a variable)", `sc1.c:5427-5430`) and on
    /// every dimension being known (`error(46)`, `sc1.c:5490`). Anything less
    /// certain than that yields no shape, so the function keeps the ordinary
    /// cell-returning convention and `do_return` diagnoses the mismatch rather
    /// than emitting a copy against a parameter slot that does not exist.
    fn infer_return_dims(&mut self, f: &FuncDecl) -> Vec<i32> {
        let Some(body) = &f.body else { return Vec::new() };

        let mut arrays: HashMap<String, Vec<i32>> = HashMap::new();
        let mut ambiguous: Vec<String> = Vec::new();
        let mut returned: Option<String> = None;
        let mut decls: Vec<Declarator> = Vec::new();
        collect_return_shape(&body.stmts, &mut decls, &mut returned);

        // This is an INFERENCE pass over the whole body, run before any statement
        // has executed, so a dimension may legitimately reference something not
        // declared yet - a block-local `const SIZE = 63` used by a later
        // `new buf[SIZE]`, for instance. Folding here must therefore be silent:
        // reporting error 8 from a speculative walk produced a diagnostic for
        // perfectly valid code, and the real `declloc()` folds the same dimension
        // again later, at a point where the constant *is* in scope.
        let saved = std::mem::take(&mut self.diags);
        for d in decls {
            if d.dims.is_empty() {
                continue;
            }
            let dims = self.var_kind(&d, false).dims().to_vec();
            if arrays.insert(d.name.name.clone(), dims).is_some() {
                // Two `new` declarations of the same name in one body: which one
                // the `return` sees depends on scope, which this walk does not
                // model, so decline to guess.
                ambiguous.push(d.name.name.clone());
            }
        }
        // Drop whatever the speculative folds reported and restore the real list.
        self.diags = saved;

        let Some(name) = returned else { return Vec::new() };
        if ambiguous.contains(&name) {
            return Vec::new();
        }
        let dims = arrays
            .get(&name)
            .cloned()
            .or_else(|| {
                // A returned parameter array or global.
                self.env.var(&name).filter(|v| v.kind.is_array()).map(|v| v.kind.dims().to_vec())
            })
            .unwrap_or_default();
        // "check that all dimensions are known": `if (dim[numdim]<=0) error(46)`.
        if dims.iter().any(|&d| d <= 0) { Vec::new() } else { dims }
    }

    fn collect_const(&mut self, c: &ConstDecl) {
        let value = self.const_expr(&c.value);
        let tag = self.tag_of(c.tag.as_ref());
        self.define_const_tagged(&c.name.name, value, tag, c.name.span);
    }

    fn collect_enum(&mut self, e: &EnumDecl) {
        // doenum() in sc1.c: members take successive values, advanced by the
        // step expression (`enum (<<= 1)`), and a sized member `Field[3]`
        // advances by its size.
        let mut next: Cell = 0;
        let step = e.step.as_ref().map(|s| (s.op, self.const_expr(&s.value)));
        // `enum Colour {..}` tags every member with `Colour:`; an explicit
        // `enum Tag: {..}` overrides that and `enum _: {..}` clears it.
        let enum_tag = match (&e.tag, &e.name) {
            (Some(t), _) => self.tag_of(Some(t)),
            (None, Some(n)) => self.intern_tag(&n.name),
            (None, None) => TagId::UNTAGGED,
        };
        for m in &e.members {
            let value = match &m.value {
                Some(v) => self.const_expr(v),
                None => next,
            };
            let tag = match &m.tag {
                Some(t) => self.tag_of(Some(t)),
                None => enum_tag,
            };
            self.define_const_tagged(&m.name.name, value, tag, m.name.span);
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

        // "set the enum name to the 'next' value (typically the last value plus
        // one)": `enumsym->addr = value` in decl_enum(). This is what makes the
        // enum-as-struct idiom work - `enum PlayerData { pd_name[32], pd_score }`
        // followed by `new data[PlayerData]`, where the enum's own name is the
        // total size. Without it every such array size was error 8.
        if let Some(n) = &e.name {
            self.define_const_tagged(&n.name, next, TagId::UNTAGGED, n.span);
        }
    }

    pub(crate) fn define_const(&mut self, name: &str, value: Cell, span: Span) {
        self.define_const_tagged(name, value, TagId::UNTAGGED, span);
    }

    pub(crate) fn define_const_tagged(
        &mut self,
        name: &str,
        value: Cell,
        tag: TagId,
        span: Span,
    ) {
        // Same reason variables need it: a constant declaration IS its definition,
        // and the folder rejects a symbol that is merely announced.
        self.table.declare(
            SymbolDecl::new(name, SymKind::Constant, span).with_usage(Usage::DEFINED),
        );
        self.fold_env =
            std::mem::take(&mut self.fold_env).with_const(name, value, tag.raw() as i32);
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
            // A variable declaration IS its definition - it has storage. Without
            // Usage::DEFINED the folder's `sizeof` treats the symbol as merely
            // announced and reports error 17, so `sizeof(g)` - and therefore every
            // `charsmax(g)`, which expands to `sizeof(g)-1` - failed on a variable
            // that was declared perfectly well.
            self.table.declare(
                SymbolDecl::new(
                    &d.name.name,
                    if kind.is_array() { SymKind::Array } else { SymKind::Variable },
                    d.name.span,
                )
                .with_usage(Usage::DEFINED),
            );
            let mut info = VarInfo::global(addr, kind);
            info.is_const = v.modifiers.is_const;
            info.tag = self.tag_of(d.tag.as_ref());
            self.fold_env = std::mem::take(&mut self.fold_env)
                .with_symbol_tag(&d.name.name, info.tag.raw() as i32);
            self.env.declare_global(d.name.name.clone(), info);

            // Global initialisers are written straight into the data segment;
            // no code is generated (declglb() in sc1.c fills litq).
            let dims =
                self.env.var(&d.name.name).map(|v| v.kind.dims().to_vec()).unwrap_or_default();
            let values = match &d.init {
                Some(init) => self.init_cells(init, &dims),
                // "if (!matchtoken('=')) ... first reserve space for the
                // indirection vectors of the array, then adjust it to contain the
                // proper values" - sc1.c:2311-2330. An *uninitialised*
                // multi-dimensional array still needs its index vectors.
                None => crate::layout::indirection_tables(&dims),
            };
            if !values.is_empty() {
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
        // `initials()` deduces EVERY unsized dimension from the initialiser, not
        // just the last one: `new const t[][][Field] = {{{1,2},{3,4}}, ...}` gets
        // both leading dimensions from the nesting. Deducing only the last left
        // the others at 0, so `sizeof t[]` folded to 0 and a `case 1 .. sizeof t[]:`
        // became an invalid range.
        if let Some(init) = d.init.as_ref() {
            for level in 0..dims.len() {
                if dims[level] != 0 {
                    continue;
                }
                if let Some(n) = self.init_len_at(init, level) {
                    dims[level] = n;
                }
            }
        }
        VarKind::Array(dims)
    }

    /// Element count of the initialiser at nesting `level`: level 0 is the outermost
    /// brace, level 1 its first sub-list, and so on. `None` when the initialiser is
    /// not nested that deep, which leaves the dimension unknown rather than guessing.
    ///
    /// Only the *first* sub-list is measured at each level - a ragged initialiser is
    /// a different matter (error 47), reported elsewhere.
    fn init_len_at(&mut self, init: &Init, level: usize) -> Option<i32> {
        if level == 0 {
            return Some(self.init_len(init));
        }
        match init {
            Init::List(l) => {
                let first = l.elems.first()?;
                self.init_len_at(first, level - 1)
            }
            Init::Expr(_) => None,
        }
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

    /// The complete data image of an initialised variable: the index vectors of
    /// every dimension but the last, followed by the element data row-major.
    ///
    /// This is `initials2()` (`sc1.c:2282`): it reserves
    /// `calc_arraysize(dim,numdim-1,0)` zero cells (line 2356), lets `initarray()`
    /// append the rows, then calls `adjust_indirectiontables()` (line 2395) to fill
    /// the reserved cells in. A one-dimensional array has no vector at all
    /// (`initvector(...,dim[0],FALSE,...)`, line 2342) and is *not* zero-padded.
    pub(crate) fn init_cells(&mut self, init: &Init, dims: &[i32]) -> Vec<i32> {
        if dims.len() > 1 {
            let mut image = crate::layout::indirection_tables(dims);
            image.extend(self.init_rows(init, dims));
            return image;
        }
        self.init_leaf_cells(init, dims)
    }

    /// The element data of a multi-dimensional initialiser, row-major and padded.
    ///
    /// `initarray()` (`sc1.c:2410`) walks the major dimensions and hands each
    /// innermost row to `initvector(..., dim[numdim-1], TRUE, ...)`; the `TRUE` is
    /// `fillzero`, so a short row is padded to the declared minor size (`sc1.c:2561`
    /// `while ((litidx-curlit)<(int)size) litadd(0)`). Rows that the initialiser
    /// omits entirely stay zero, which is what makes the fixed offsets computed by
    /// `adjust_indirectiontables()` correct.
    fn init_rows(&mut self, init: &Init, dims: &[i32]) -> Vec<i32> {
        let want: usize = dims.iter().map(|&d| d.max(0) as usize).product();
        let mut out = match dims {
            [_] | [] => self.init_leaf_cells(init, dims),
            [_, rest @ ..] => match init {
                Init::List(list) => {
                    let mut out = Vec::with_capacity(want);
                    for elem in &list.elems {
                        if out.len() >= want {
                            // error 18 (initialisation data exceeds array size) is
                            // the parser's; codegen only refuses to overflow.
                            break;
                        }
                        out.extend(self.init_rows(elem, rest));
                    }
                    out
                }
                // A non-braced initialiser for a multi-dimensional array only
                // reaches here for a string, which fills the first row.
                Init::Expr(_) => self.init_leaf_cells(init, &dims[dims.len() - 1..]),
            },
        };
        out.resize(want, 0);
        out
    }

    /// One innermost row (or a whole one-dimensional array), unpadded.
    fn init_leaf_cells(&mut self, init: &Init, dims: &[i32]) -> Vec<i32> {
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
            out.extend(self.init_leaf_cells(elem, inner));
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
    /// Park a literal block in the data segment and return its byte address.
    /// Used for array parameter defaults, which are passed by address.
    pub(crate) fn intern_literal(&mut self, cells: &[i32]) -> i32 {
        let addr = self.data.alloc(cells.len().max(1) as i32);
        if !cells.is_empty() {
            self.data.init_at(addr, cells);
        }
        addr
    }

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
        let name = match &f.name {
            FuncName::Ident(id) => id.name.clone(),
            // `collect_func()` stored the overload under its mangled name; if it
            // rejected the declaration there is nothing to emit.
            FuncName::Operator { .. } => match self.operator_name(f) {
                Some(n) => n,
                None => return,
            },
        };
        let Some(info @ FuncInfo { callee: Callee::Func(label), .. }) =
            self.env.func(&name).cloned()
        else {
            return;
        };
        self.cur_op = matches!(f.name, FuncName::Operator { .. }).then(|| name.clone());

        self.asm.place(label);
        self.asm.emit0(Opcode::Proc); // startfunc(): "creates stack frame"
        self.declared = 0;
        self.decl_heap = 0;
        self.cur_nargs = info.params.len() as i32;
        self.cur_ret_dims = info.ret_dims.clone();
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
        self.cur_op = None;
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
            // A variable declaration IS its definition - it has storage. Without
            // Usage::DEFINED the folder's `sizeof` treats the symbol as merely
            // announced and reports error 17, so `sizeof(g)` - and therefore every
            // `charsmax(g)`, which expands to `sizeof(g)-1` - failed on a variable
            // that was declared perfectly well.
            self.table.declare(
                SymbolDecl::new(
                    &d.name.name,
                    if kind.is_array() { SymKind::Array } else { SymKind::Variable },
                    d.name.span,
                )
                .with_usage(Usage::DEFINED),
            );
            // A multi-tag parameter (`{Float,_}:x`) has no single tag to
            // dispatch on; `check_userop()` would use `tags[0]`, and so do we.
            let tag = match d.tags.as_ref().and_then(|t| t.tags.first()) {
                Some(t) => self.intern_tag(&t.name.name),
                None => TagId::UNTAGGED,
            };
            self.fold_env =
                std::mem::take(&mut self.fold_env).with_symbol_tag(&d.name.name, tag.raw() as i32);
            let info =
                VarInfo { addr, class: Class::Local, kind, is_const: d.is_const, tag };
            self.env.declare_local(d.name.name.clone(), info);
        }
    }
}

/// Gather every `new` declarator in a body and the name of the first
/// `return <ident>;`. Used only by [`Generator::infer_return_dims`].
fn collect_return_shape(
    stmts: &[zpc_ast::stmt::Stmt],
    decls: &mut Vec<Declarator>,
    returned: &mut Option<String>,
) {
    use zpc_ast::stmt::{ForInit, Stmt};
    for s in stmts {
        match s {
            Stmt::Var(v) => decls.extend(v.declarators.iter().cloned()),
            Stmt::Return { value: Some(e), .. } => {
                if returned.is_none()
                    && let ExprKind::Ident(id) = &e.kind
                {
                    *returned = Some(id.name.clone());
                }
            }
            Stmt::Block(b) => collect_return_shape(&b.stmts, decls, returned),
            Stmt::If { then_branch, else_branch, .. } => {
                collect_return_shape(std::slice::from_ref(then_branch.as_ref()), decls, returned);
                if let Some(alt) = else_branch {
                    collect_return_shape(std::slice::from_ref(alt.as_ref()), decls, returned);
                }
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. } => {
                collect_return_shape(std::slice::from_ref(body.as_ref()), decls, returned);
            }
            Stmt::For { init, body, .. } => {
                if let Some(ForInit::Decl(v)) = init {
                    decls.extend(v.declarators.iter().cloned());
                }
                collect_return_shape(std::slice::from_ref(body.as_ref()), decls, returned);
            }
            Stmt::Switch { cases, default, .. } => {
                for c in cases {
                    collect_return_shape(std::slice::from_ref(&c.body), decls, returned);
                }
                if let Some(d) = default {
                    collect_return_shape(std::slice::from_ref(d.as_ref()), decls, returned);
                }
            }
            _ => {}
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



