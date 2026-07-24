//! AST to Pawn assembly code generation - the port of `libpc300`'s `sc3.c` and
//! `sc4.c`, driven by the statement routines of `sc1.c`.
//!
//! # What this produces
//!
//! [`Generator::program`] turns a [`zpc_ast::Program`] into a [`Unit`]: a stream of
//! [`stream::Item`] (the structured equivalent of the text `stgwrite()` emits), a
//! data segment, the native table and the public table. [`Unit::assemble`] resolves
//! labels and encodes the code segment, which [`zpc_asm::disassemble`] reads back.
//!
//! # The machine model
//!
//! See the module docs of [`emit`] for the register and frame layout. The two facts
//! that decide almost everything else:
//!
//! * Expression results live in PRI; a binary operator's *left* operand is in ALT
//!   (`ob_sub` is `sub.alt`, i.e. `PRI = ALT - PRI`).
//! * Arguments are pushed **right to left**, and a final `push.c nargs*cell` cell
//!   carries the argument count. See the doc comment on the call emitter in
//!   [`expr`] for the `sc3.c`/`sc7.c` line references that establish this.
//!
//! # Not implemented
//!
//! These constructs parse and are diagnosed rather than mis-compiled:
//!
//! * **Automata and states** (`state x;`, `func() <idle>`): `writeleader()` in
//!   `sc4.c` builds a state-selector table and per-function dispatch stubs. Not
//!   emitted; `Stmt::State` raises error 86.
//! * **User-defined operators** (`operator+(Float:,Float:)`): `check_userop()` in
//!   `sc3.c` rewrites almost every operator emission into a function call.
//!   `FuncName::Operator` raises error 7.
//! * **Functions returning arrays**: the hidden heap parameter of `doreturn()`
//!   (`sc1.c:5493`) and the `popreg(sPRI)` at the end of `callfunction()`.
//! * **The peephole optimiser** (`sc7.c`): the output here is the unoptimised
//!   stream, which is exactly what `sc4.c` alone produces.
//! * **The index vector of a multi-dimensional array**: [`layout::VarKind`]
//!   reserves the cells (`calc_arraysize()`), and indexing follows the offset with
//!   `load.i`, but the initialiser does not *write* the vector, so a
//!   multi-dimensional array reads zeroes at its major dimension.
//! * **Heap equilibration across the arms of `?:`** (the `heap1`/`heap2`
//!   bookkeeping in `hier13()`), which only matters once functions may return
//!   arrays.
//! * **Block-local `enum`**: only file-scope enums are folded into constants.
//! * **Tags**: no tag ids are propagated, so `exit`/`sleep` always pass tag 0 and
//!   no warning 213 is raised here. Tag checking is [`zpc_sema::tags`]' job.
//!
//! # Uncertainties
//!
//! * **Native indices.** `ffcall()` assigns `sym->addr = ntv_funcid++` at the first
//!   *call*, during the write pass, so amxxpc numbers natives in call order, not
//!   declaration order. Here they are numbered in declaration order because the
//!   generator makes a single pass. The numbers are internal to the `.amx` (the
//!   native table maps them to names), so this is a layout difference, not a
//!   behavioural one - but a byte-level differential test against amxxpc will see
//!   it.
//! * **`lastst` tracking.** Upstream suppresses the trailing `zero.pri; retn` when
//!   the last statement was a `return` or `goto`. That bookkeeping is left to the
//!   peephole pass, so a function ending in `return` gets one dead pair here.

#![forbid(unsafe_code)]

pub mod expr;
pub mod emit;
pub mod layout;
pub mod stmt;
pub mod stream;

pub use emit::{Generator, Unit};
pub use stream::{
    AsmError, AsmStream, Item, LabelId, Operand, Reg, assemble, assemble_with_labels, render_asm,
};

#[cfg(test)]
mod tests;
