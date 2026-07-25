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
//! * **`operator=` and `operator~`**: the coercion hook and the array
//!   destructor. `operator=` is consulted at assignment, initialisation and
//!   by-value argument passing, none of which [`userop`] dispatches, and
//!   `operator~` has no call site at all. Registering either without dispatch
//!   would silently do nothing, so `FuncName::Operator` still raises error 7 for
//!   exactly those two forms. Neither appears in the AMX Mod X headers. Every
//!   other operator overload is registered and dispatched - see [`userop`].
//! * **Some `sc7.c` peephole sequences.** [`peephole::optimise`] ports most of
//!   `sequences[]`; the rows it deliberately leaves out (the chained-relational
//!   `xchg` family, the `;$lcl` declaration rows and the user-defined-operator
//!   push pairs) are listed with their reasons at the end of [`peephole`].
//! * **Heap equilibration across the arms of `?:`** (the `heap1`/`heap2`
//!   bookkeeping in `hier13()`, `sc3.c:1036-1066`). Now that a call may leave an
//!   array on the heap, a `?:` whose two arms claim different amounts leaves the
//!   heap unbalanced on one path. The `decl_heap` counter is threaded through
//!   [`emit::Generator::expression`] but not yet through `hier13()`.
//! * **Block-local `enum`**: only file-scope enums are folded into constants.
//! * **An array-returning *definition* whose shape cannot be inferred.**
//!   `newfunc()` has no return-dimension syntax; `doreturn()` derives the shape
//!   from the first `return <array>;` and re-parses the unit when the function
//!   was already called (`sc_reparse=TRUE`, `sc1.c:5518`). The single pass here
//!   infers it in the pre-pass from a `return` naming an unambiguously declared
//!   array; when it cannot, `return` of an array raises error 46 rather than
//!   compiling a `load.i` of the base address.
//! * **Tag *checking***: [`Generator::expr_tag`] propagates tag ids far enough to
//!   dispatch user-defined operators (literals, variables, parameters, constants
//!   and enum members, casts, call results, subscripts and the operators
//!   themselves), but codegen still raises no warning 213 and `exit`/`sleep`
//!   still pass tag 0 in ALT. Tag *checking* remains [`zpc_sema::tags`]' job.
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
//!   the last statement was a `return` or `goto` (`sc1.c:3517`). Here the pair is
//!   always emitted and [`peephole::optimise`] deletes it as unreachable code,
//!   reaching the same accept set without threading `lastst` through codegen.

#![forbid(unsafe_code)]

pub mod expr;
pub mod emit;
pub mod layout;
pub mod peephole;
pub mod stmt;
pub mod stream;
pub mod userop;

pub use emit::{Generator, Unit};
pub use peephole::optimise;
pub use stream::{
    AsmError, AsmStream, Item, LabelId, Operand, Reg, assemble, assemble_with_labels, render_asm,
};

#[cfg(test)]
mod tests;
