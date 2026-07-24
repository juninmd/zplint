//! Semantic analysis: symbols, tags, constant folding.
//! Phase under construction - see docs/COMPILER_MIGRATION.md.

#![forbid(unsafe_code)]

pub mod symbols;

pub use symbols::{
    FuncSig, ParamSig, RefKind, Scope, SymKind, Symbol, SymbolDecl, SymbolId, SymbolTable, Usage,
};
