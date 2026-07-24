//! Semantic analysis: symbols, tags, constant folding.
//! Phase under construction - see docs/COMPILER_MIGRATION.md.

#![forbid(unsafe_code)]

pub mod fold;
pub mod symbols;
pub mod tags;

pub use fold::{ArrayInfo, Cell, Const, ConstEnv, EnumField, Folder, MapEnv, TagConfig, TagId};
// `tags::TagId` is deliberately not re-exported here: `fold` currently exports a
// distinct type of the same name. Use `zpc_sema::tags::TagId` for the real one.
pub use tags::{Assignee, Coercion, OpKind, Overload, Overloads, TagCheck, Tags};
pub use symbols::{
    FuncSig, ParamSig, RefKind, Scope, SymKind, Symbol, SymbolDecl, SymbolId, SymbolTable, Usage,
};
