//! Lexer and preprocessor for the zpc Pawn compiler.
//!
//! Ported from `compiler/libpc300/sc2.c` (Pawn compiler, (c) ITB CompuPhase,
//! zlib-style licence - see ATTRIBUTION.md). This is an altered version: it produces
//! a token stream carrying spans instead of feeding a single-pass compiler directly.

pub mod scanner;
pub mod token;

pub use scanner::{DEFAULT_CTRL_CHAR, Scanner};
pub use token::{Token, TokenKind, OPERATORS};
