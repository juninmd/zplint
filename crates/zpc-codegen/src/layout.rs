//! Storage layout: where a variable lives, and the data segment.
//!
//! `libpc300` stores this in `symbol.addr` plus `symbol.vclass` (`sc.h`), which the
//! `sc4.c` emitters branch on. The split here is the same one, made explicit.

use std::collections::HashMap;

use crate::stream::{CELL, LabelId};

/// How a variable is reached. Mirrors the `ident` field of `value` in `sc.h`, which
/// is what `rvalue()`, `store()` and `address()` switch on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VarKind {
    /// `iVARIABLE`: a plain cell.
    Scalar,
    /// `iARRAY`: an array the function owns, with its declared dimensions.
    Array(Vec<i32>),
    /// `iREFERENCE`: a `&x` argument. The slot holds the *address* of the cell.
    Reference,
    /// `iREFARRAY`: an `x[]` argument. The slot holds the array's base address.
    RefArray(Vec<i32>),
}

impl VarKind {
    pub fn is_array(&self) -> bool {
        matches!(self, VarKind::Array(_) | VarKind::RefArray(_))
    }

    /// Declared dimensions, major first; empty for a scalar.
    pub fn dims(&self) -> &[i32] {
        match self {
            VarKind::Array(d) | VarKind::RefArray(d) => d,
            _ => &[],
        }
    }

    /// Total cell count of an owned array, including the index vectors that a
    /// multi-dimensional array needs (`calc_arraysize()` in `sc1.c`:
    /// `dim[cur] + dim[cur] * calc_arraysize(cur+1)`).
    pub fn total_cells(&self) -> i32 {
        fn size(dims: &[i32]) -> i32 {
            match dims.split_first() {
                None => 0,
                Some((&d, rest)) => d + d * size(rest),
            }
        }
        size(self.dims())
    }
}

/// Where the storage lives. `vclass` in `sc.h`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// `sGLOBAL`/`sSTATIC`: `addr` is a byte offset into the data segment.
    Global,
    /// `sLOCAL`: `addr` is a byte offset from FRM - negative for locals, positive
    /// (`(n+3)*cell`) for arguments.
    Local,
}

/// One variable, as codegen needs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VarInfo {
    pub addr: i32,
    pub class: Class,
    pub kind: VarKind,
    /// `uCONST` - assignment to it is error 22.
    pub is_const: bool,
}

impl VarInfo {
    pub fn global(addr: i32, kind: VarKind) -> Self {
        Self { addr, class: Class::Global, kind, is_const: false }
    }

    pub fn local(addr: i32, kind: VarKind) -> Self {
        Self { addr, class: Class::Local, kind, is_const: false }
    }
}

/// What a name at a call site resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    /// An ordinary function: `call <label>`.
    Func(LabelId),
    /// A native: `sysreq.c <index>` followed by `stack (nargs+1)*cell`.
    Native(i32),
}

/// How one declared parameter must be passed. `arginfo.ident` in `sc.h`, which is
/// the value `callfunction()` switches on when it prepares each argument.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// `iVARIABLE` - by value.
    Value,
    /// `iREFERENCE` - `&x`, the address is pushed.
    Reference,
    /// `iREFARRAY` - `x[]`, the base address is pushed.
    Array,
    /// `iVARARGS` - `...`, always passed by reference.
    VarArgs,
}

/// A parameter of a declared function.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    /// The value `_` (and an omitted trailing argument) expands to.
    pub default: Option<i32>,
}

/// The callable signature codegen needs: enough to lay out the argument block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FuncInfo {
    pub callee: Callee,
    pub params: Vec<Param>,
    /// True when the last declared parameter is `...`.
    pub variadic: bool,
}

impl FuncInfo {
    /// The parameter a positional argument at `index` binds to. Arguments past the
    /// last fixed parameter of a variadic function all bind to the `...`.
    pub fn param_at(&self, index: usize) -> Option<&Param> {
        match self.params.get(index) {
            Some(p) => Some(p),
            None if self.variadic => self.params.last(),
            None => None,
        }
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }
}

/// The data segment under construction: globals first, then the literal pool.
///
/// `glb_declared` counts cells in `sc1.c`; addresses are `glb_declared *
/// sizeof(cell)`. The literal queue (`litq`) is flushed into the same segment, so a
/// string's address is `(litidx + glb_declared) * sizeof(cell)` - see the
/// `ldconst((cur_lit+glb_declared)*sizeof(cell),sPRI)` in `declloc()`. Here the two
/// share one growing vector, which produces the same addresses without the
/// two-stage bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct DataSeg {
    cells: Vec<i32>,
}

impl DataSeg {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cells(&self) -> &[i32] {
        &self.cells
    }

    /// Byte size of the segment.
    pub fn len_bytes(&self) -> i32 {
        self.cells.len() as i32 * CELL
    }

    /// Reserve `count` zeroed cells and return the byte address of the first.
    pub fn alloc(&mut self, count: i32) -> i32 {
        let addr = self.len_bytes();
        self.cells.extend(std::iter::repeat_n(0, count.max(0) as usize));
        addr
    }

    /// Append a literal block and return its byte address. This is `litadd()`
    /// followed by the flush at the end of `newfunc()`.
    pub fn literal(&mut self, values: &[i32]) -> i32 {
        let addr = self.len_bytes();
        self.cells.extend_from_slice(values);
        addr
    }

    /// Write `values` at an address already reserved by [`DataSeg::alloc`].
    pub fn init_at(&mut self, addr: i32, values: &[i32]) {
        let start = (addr / CELL) as usize;
        for (i, v) in values.iter().enumerate() {
            if let Some(slot) = self.cells.get_mut(start + i) {
                *slot = *v;
            }
        }
    }
}

/// The nested name environment: one global map plus a stack of block scopes.
///
/// `sc1.c` keeps two symbol tables (`glbtab`, `loctab`) and deletes locals by
/// `nestlevel` on leaving a compound block; the scope stack does the same.
#[derive(Clone, Debug, Default)]
pub struct Env {
    globals: HashMap<String, VarInfo>,
    scopes: Vec<HashMap<String, VarInfo>>,
    funcs: HashMap<String, FuncInfo>,
    /// `const` symbols and enum members, already folded to a cell.
    constants: HashMap<String, i32>,
}

impl Env {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enter(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn leave(&mut self) {
        self.scopes.pop();
    }

    pub fn declare_global(&mut self, name: impl Into<String>, info: VarInfo) {
        self.globals.insert(name.into(), info);
    }

    pub fn declare_local(&mut self, name: impl Into<String>, info: VarInfo) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.into(), info);
        } else {
            self.globals.insert(name.into(), info);
        }
    }

    pub fn declare_func(&mut self, name: impl Into<String>, info: FuncInfo) {
        self.funcs.insert(name.into(), info);
    }

    pub fn declare_const(&mut self, name: impl Into<String>, value: i32) {
        self.constants.insert(name.into(), value);
    }

    /// Innermost binding wins, then the globals - `findloc()` before `findglb()`.
    pub fn var(&self, name: &str) -> Option<&VarInfo> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .or_else(|| self.globals.get(name))
    }

    pub fn func(&self, name: &str) -> Option<&FuncInfo> {
        self.funcs.get(name)
    }

    pub fn constant(&self, name: &str) -> Option<i32> {
        self.constants.get(name).copied()
    }

    pub fn constants(&self) -> &HashMap<String, i32> {
        &self.constants
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multidimensional_arrays_reserve_their_index_vectors() {
        // calc_arraysize({2,3}) == 2 + 2*3 == 8: two index cells plus two rows.
        assert_eq!(VarKind::Array(vec![2, 3]).total_cells(), 8);
        assert_eq!(VarKind::Array(vec![5]).total_cells(), 5);
        assert_eq!(VarKind::Scalar.total_cells(), 0);
    }

    #[test]
    fn data_segment_addresses_are_byte_offsets() {
        let mut d = DataSeg::new();
        assert_eq!(d.alloc(1), 0);
        assert_eq!(d.alloc(3), 4);
        assert_eq!(d.literal(&[1, 2]), 16);
        assert_eq!(d.cells(), &[0, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn inner_scopes_shadow_outer_ones() {
        let mut env = Env::new();
        env.declare_global("x", VarInfo::global(0, VarKind::Scalar));
        env.enter();
        env.declare_local("x", VarInfo::local(-4, VarKind::Scalar));
        assert_eq!(env.var("x").unwrap().class, Class::Local);
        env.leave();
        assert_eq!(env.var("x").unwrap().class, Class::Global);
    }

    #[test]
    fn variadic_arguments_all_bind_to_the_ellipsis() {
        let info = FuncInfo {
            callee: Callee::Native(0),
            params: vec![
                Param { name: "fmt".into(), kind: ParamKind::Array, default: None },
                Param { name: "".into(), kind: ParamKind::VarArgs, default: None },
            ],
            variadic: true,
        };
        assert_eq!(info.param_at(0).unwrap().kind, ParamKind::Array);
        assert_eq!(info.param_at(9).unwrap().kind, ParamKind::VarArgs);
    }
}
