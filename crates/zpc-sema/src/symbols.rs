//! The `zpc` symbol table.
//!
//! # Provenance
//!
//! This is a **clean-room** implementation. The upstream Pawn compiler stores
//! symbols in `libpc300/sp_symhash.c`, a file that carries **no copyright or
//! licence header at all** - see `docs/LICENSING.md`, which classifies it as
//! "procedência indefinida = risco" and forbids porting it. Nothing here was
//! derived from that file; the container is Rust's [`HashMap`] plus a
//! declaration-ordered arena, and the *behaviour* below was designed from the
//! observable contract the rest of the compiler needs (which symbol wins a
//! lookup, when a symbol disappears, which diagnostic fires when) together with
//! the zlib-licensed `sc.h` symbol *model* (the `ident` kinds `iVARIABLE`,
//! `iREFERENCE`, `iARRAY`, `iREFARRAY`, `iCONSTEXPR`, `iFUNCTN`, `iLABEL` and
//! the `usage` flags `uDEFINE`/`uREAD`/`uWRITTEN`/`uCONST`/`uPROTOTYPED`/
//! `uPUBLIC`/`uNATIVE`/`uSTOCK`/`uMISSING`).
//!
//! # Model
//!
//! One [`SymbolTable`] covers one translation unit (one `.sma` after
//! preprocessing). It holds:
//!
//! * an **arena** `Vec<Symbol>` - symbols are never moved or freed, so a
//!   [`SymbolId`] stays valid after a scope is left and the "was it used?"
//!   bookkeeping survives into the final report;
//! * a **name index** `HashMap<String, Vec<SymbolId>>` whose per-name vector is
//!   a stack: the innermost declaration is last, so a lookup is `last()` and a
//!   local naturally shadows a global;
//! * a **scope level**: 0 is global/file scope, each `{` pushes a level. Leaving
//!   a level pops every name declared at that level or deeper, which is the
//!   observable effect of `delete_symbols()` at block exit.
//!
//! Pawn's storage classes map to [`Scope`]: `sGLOBAL`, `sSTATIC` (global
//! lifetime, file scope) and `sLOCAL`. Because a `SymbolTable` never spans more
//! than one translation unit, `static` globals live in the same map as ordinary
//! globals and differ only in the flag they carry; cross-unit visibility is a
//! linker concern that `zpc` does not have (AMX Mod X compiles one file at a
//! time).
//!
//! # Ordering semantics and the deliberate divergence
//!
//! Upstream Pawn is a **single-pass** compiler: a name must already be in the
//! table when it is used. The table below preserves that by resolving
//! references *eagerly* - callers are expected to drive it in source order, and
//! [`SymbolTable::reference`] answers using the table exactly as it stands at
//! that point. Only one thing is deferred, and it is deferred upstream too: a
//! call to an unknown name is *not* an error, because a bare `foo()` creates an
//! implicit forward declaration (`uFORWARD|uMISSING`) that a later definition is
//! allowed to satisfy. Those are collected and settled in
//! [`SymbolTable::finish`], which is the "resolve" half of the collect-then-
//! resolve split.
//!
//! Where we knowingly differ:
//!
//! 1. **No reparse.** Upstream, calling a *tagged* function before its
//!    definition forces a second parse of the file (warning 208). `zpc` reports
//!    warning 208 at the definition and keeps going; it does not re-run the
//!    parser. The accept/reject set is unchanged - 208 is a warning either way -
//!    but the *tag* of such a call expression is only correct on the second
//!    upstream pass, so tag checking of those call sites is left to the caller.
//! 2. **Unused-symbol reports are batched.** Upstream emits 203/204 as each
//!    scope is destroyed and as globals are released at the end of the compile.
//!    Here the diagnostics are buffered and sorted by source position before
//!    being handed out, so output order is a function of the source, never of
//!    `HashMap` iteration order.
//! 3. **`sc_status` two-pass.** Upstream compiles the file twice (statFIRST to
//!    size things, statWRITE to emit). The symbol table is rebuilt in between,
//!    so nothing here needs to model it; a fresh [`SymbolTable`] per pass
//!    reproduces it exactly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use zpc_diag::{Diagnostic, Span};

/// Maximum significant length of a symbol name (`sNAMEMAX` in `amx.h`, 63 for
/// AMX Mod X). Longer names are truncated and reported with warning 200.
pub const NAME_MAX: usize = 63;

/// What a symbol *is*, mirroring the `ident` field of `struct symbol` in `sc.h`.
///
/// `iEXPRESSION` has no counterpart here: it describes the result of an
/// expression, never anything that lives in a symbol table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SymKind {
    /// `iLABEL` - a `goto` target. Labels have their own flat function-wide
    /// namespace in Pawn, not block scope.
    Label,
    /// `iVARIABLE` - a single cell with an address.
    Variable,
    /// `iREFERENCE` - a by-reference argument (`&x`); an lvalue behind a pointer.
    Reference,
    /// `iARRAY` - an array the symbol owns storage for.
    Array,
    /// `iREFARRAY` - an array passed by reference, i.e. an array argument.
    RefArray,
    /// `iCONSTEXPR` - a named constant: `const`, an `enum` member, a `#define`d
    /// numeric constant promoted into the table, or `sizeof`-style literals.
    Constant,
    /// `iFUNCTN` without `uNATIVE`.
    Function,
    /// `iFUNCTN` with `uNATIVE` - implemented by the host, never by the script.
    Native,
}

impl SymKind {
    /// True for the kinds that occupy storage and can therefore be read from or
    /// assigned to - the only ones "declared but never used" applies to.
    pub fn is_variable_like(self) -> bool {
        matches!(
            self,
            SymKind::Variable | SymKind::Reference | SymKind::Array | SymKind::RefArray
        )
    }

    /// True for `iFUNCTN`, native or not.
    pub fn is_function(self) -> bool {
        matches!(self, SymKind::Function | SymKind::Native)
    }
}

/// Bit flags mirroring the `usage` field of `struct symbol` in `sc.h`.
///
/// The numeric values are *not* copied from upstream: `uWRITTEN`/`uRETVALUE` and
/// `uCONST`/`uPROTOTYPED` share a bit there because the meanings never collide
/// on one symbol kind, which is a memory optimisation we have no reason to
/// reproduce. Each flag here gets its own bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Usage(u16);

impl Usage {
    /// No flags.
    pub const EMPTY: Usage = Usage(0);
    /// `uDEFINE` - the symbol has a body/storage, not just a heading.
    pub const DEFINED: Usage = Usage(1 << 0);
    /// `uREAD` - the value was fetched at least once.
    pub const READ: Usage = Usage(1 << 1);
    /// `uWRITTEN` - the symbol was assigned at least once.
    pub const WRITTEN: Usage = Usage(1 << 2);
    /// `uCONST` - read-only storage (`const` argument or variable).
    pub const CONST: Usage = Usage(1 << 3);
    /// `uPROTOTYPED` - a heading was seen (`forward`, `native`, or a definition).
    pub const PROTOTYPED: Usage = Usage(1 << 4);
    /// `uPUBLIC` - exported to the AMX; never "unused" from the script's view.
    pub const PUBLIC: Usage = Usage(1 << 5);
    /// `uNATIVE` - implemented by the host module.
    pub const NATIVE: Usage = Usage(1 << 6);
    /// `uSTOCK` - library code; being unused is the *point*, never warn.
    pub const STOCK: Usage = Usage(1 << 7);
    /// `uMISSING` - referenced but not (yet) implemented.
    pub const MISSING: Usage = Usage(1 << 8);
    /// `uRETVALUE` - the function returns a value.
    pub const RETVALUE: Usage = Usage(1 << 9);
    /// Not in `sc.h`: the heading was synthesised by a call to an unknown name,
    /// so a later definition with a return tag must raise warning 208.
    pub const IMPLICIT_FORWARD: Usage = Usage(1 << 10);
    /// Not in `sc.h`: file-scoped (`static`) rather than exported-global.
    pub const STATIC: Usage = Usage(1 << 11);

    /// True when every bit of `other` is set here.
    pub fn contains(self, other: Usage) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when at least one bit of `other` is set here.
    pub fn intersects(self, other: Usage) -> bool {
        self.0 & other.0 != 0
    }

    /// Sets every bit of `other`.
    pub fn insert(&mut self, other: Usage) {
        self.0 |= other.0;
    }

    /// Clears every bit of `other`.
    pub fn remove(&mut self, other: Usage) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for Usage {
    type Output = Usage;
    fn bitor(self, rhs: Usage) -> Usage {
        Usage(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Usage {
    fn bitor_assign(&mut self, rhs: Usage) {
        self.0 |= rhs.0;
    }
}

/// Where a symbol lives, mirroring `sGLOBAL`/`sSTATIC`/`sLOCAL`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// `sGLOBAL` - visible to the whole translation unit.
    Global,
    /// `sSTATIC` - global lifetime, file scope. Same visibility as
    /// [`Scope::Global`] inside one `.sma`; the distinction only matters to the
    /// code generator, which must not export it.
    Static,
    /// `sLOCAL` at nesting `level` (1 = a function's outermost block).
    Local { level: u32 },
}

impl Scope {
    /// The nesting level used for shadowing and scope-exit decisions; globals
    /// and statics are level 0.
    pub fn level(self) -> u32 {
        match self {
            Scope::Global | Scope::Static => 0,
            Scope::Local { level } => level,
        }
    }

    /// True for anything that survives to the end of the translation unit.
    pub fn is_global(self) -> bool {
        matches!(self, Scope::Global | Scope::Static)
    }
}

/// One parameter of a function heading, reduced to the parts error 25
/// ("function heading differs from prototype") actually compares.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ParamSig {
    /// Tag names as written, in source order. Empty means untagged. More than
    /// one entry is the `{Float,_}:` multi-tag form, which only arguments allow.
    pub tags: Vec<String>,
    /// `&x` - passed by reference.
    pub by_ref: bool,
    /// `const x` - the callee may not write through it.
    pub is_const: bool,
    /// Number of `[]` groups; 0 for a plain cell.
    pub dims: u8,
    /// The parameter has a default value, so it may be omitted at the call site.
    pub has_default: bool,
}

/// A function heading, as compared between a `forward`/`native` prototype and
/// the later definition.
///
/// Upstream compares headings inside `funcdisplay`/`newfunc` and rejects any
/// difference with error 25. The name is deliberately *not* part of this struct:
/// two headings are only ever compared when they already share a name.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FuncSig {
    /// Return tag as written, `None` for untagged.
    pub return_tag: Option<String>,
    pub params: Vec<ParamSig>,
    /// The heading ends in `...` (`sizeof`-style variadic argument list).
    pub variadic: bool,
}

impl FuncSig {
    /// Smallest number of arguments a call may pass: everything up to the first
    /// parameter with a default value.
    pub fn min_arity(&self) -> usize {
        self.params.iter().take_while(|p| !p.has_default).count()
    }

    /// Number of declared parameters, ignoring a trailing `...`.
    pub fn arity(&self) -> usize {
        self.params.len()
    }
}

/// Index into the table's arena. Stable for the lifetime of the table, even
/// after the symbol's scope is left.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SymbolId(u32);

impl SymbolId {
    /// Declaration order, usable as a deterministic sort key.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A symbol as stored in the table.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Name after truncation to [`NAME_MAX`], matching what upstream stores.
    pub name: String,
    pub kind: SymKind,
    pub usage: Usage,
    pub scope: Scope,
    /// Span of the *declaring* occurrence of the name. For a function that was
    /// forward-declared first, this stays on the `forward`, matching upstream's
    /// "first heading wins" behaviour.
    pub span: Span,
    /// Span of the defining occurrence, once one is seen.
    pub def_span: Option<Span>,
    /// Present for [`SymKind::Function`]/[`SymKind::Native`].
    pub sig: Option<FuncSig>,
    /// True once the symbol has been removed from lookup by leaving its scope.
    pub retired: bool,
}

impl Symbol {
    /// Whether "never used" reporting applies at all.
    ///
    /// Exemptions follow upstream: `stock` exists precisely so an unused library
    /// function stays quiet; `public` and `native` symbols are reachable from
    /// outside the script; and a symbol synthesised by the compiler (an implicit
    /// forward) was never written by the user, so it has no declaration to
    /// complain about.
    fn wants_unused_report(&self) -> bool {
        if self
            .usage
            .intersects(Usage::STOCK | Usage::PUBLIC | Usage::NATIVE | Usage::IMPLICIT_FORWARD)
        {
            return false;
        }
        matches!(
            self.kind,
            SymKind::Variable
                | SymKind::Reference
                | SymKind::Array
                | SymKind::RefArray
                | SymKind::Constant
                | SymKind::Function
        )
    }
}

/// What a reference does to the symbol it names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefKind {
    /// The value is fetched (`uREAD`).
    Read,
    /// The symbol is assigned (`uWRITTEN`).
    Write,
    /// Read-modify-write: `x += 1`, `x++`, passing `x` to a `&` parameter.
    ReadWrite,
    /// A call. Marks `uREAD`, and an unknown name becomes an implicit forward
    /// declaration instead of an immediate error.
    Call,
}

/// A declaration request handed to [`SymbolTable::declare`].
#[derive(Clone, Debug)]
pub struct SymbolDecl {
    pub name: String,
    pub kind: SymKind,
    pub usage: Usage,
    pub span: Span,
    pub sig: Option<FuncSig>,
}

impl SymbolDecl {
    /// A declaration with no flags and no signature.
    pub fn new(name: impl Into<String>, kind: SymKind, span: Span) -> Self {
        Self { name: name.into(), kind, usage: Usage::EMPTY, span, sig: None }
    }

    /// Adds usage flags.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage |= usage;
        self
    }

    /// Attaches a function heading.
    #[must_use]
    pub fn with_sig(mut self, sig: FuncSig) -> Self {
        self.sig = Some(sig);
        self
    }
}

/// Scoped symbol table for one translation unit.
///
/// Drive it in source order: [`SymbolTable::declare`] at each declaration,
/// [`SymbolTable::reference`] at each use, [`SymbolTable::enter_scope`] /
/// [`SymbolTable::leave_scope`] around each block, and [`SymbolTable::finish`]
/// once at the end of the file.
pub struct SymbolTable {
    file: PathBuf,
    arena: Vec<Symbol>,
    /// name -> declaration stack, innermost last.
    index: HashMap<String, Vec<SymbolId>>,
    level: u32,
    diags: Vec<Diagnostic>,
    /// Calls to names that had no declaration yet, in encounter order.
    pending_calls: Vec<(SymbolId, Span)>,
    finished: bool,
}

impl SymbolTable {
    /// Builds an empty table for `file`; the path is only used to stamp
    /// diagnostics.
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            arena: Vec::new(),
            index: HashMap::new(),
            level: 0,
            diags: Vec::new(),
            pending_calls: Vec::new(),
            finished: false,
        }
    }

    /// The file every diagnostic is attributed to.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Current nesting level; 0 at file scope.
    pub fn level(&self) -> u32 {
        self.level
    }

    /// All symbols ever declared, in declaration order (including retired ones).
    pub fn symbols(&self) -> &[Symbol] {
        &self.arena
    }

    /// Looks a symbol up by id.
    pub fn get(&self, id: SymbolId) -> &Symbol {
        &self.arena[id.index()]
    }

    fn get_mut(&mut self, id: SymbolId) -> &mut Symbol {
        &mut self.arena[id.index()]
    }

    // ---------------------------------------------------------------- scopes

    /// Opens a block. Every symbol declared from now until the matching
    /// [`SymbolTable::leave_scope`] is a local of this level.
    pub fn enter_scope(&mut self) {
        self.level += 1;
    }

    /// Closes a block: every name declared at this level (or deeper, had a
    /// caller unbalanced the calls) leaves lookup, and each one that was never
    /// read is reported.
    ///
    /// This is the observable half of upstream's `delete_symbols()`. The symbols
    /// stay in the arena so ids remain valid and so `finish()` can still report
    /// on them; they are marked [`Symbol::retired`].
    ///
    /// Reporting is done in declaration order, which is source order, so the
    /// `HashMap` never influences it.
    pub fn leave_scope(&mut self) {
        if self.level == 0 {
            debug_assert!(false, "leave_scope() at file scope: unbalanced scope handling");
            return;
        }
        let dying = self.level;
        self.level -= 1;

        let mut retired: Vec<SymbolId> = Vec::new();
        {
            let Self { index, arena, .. } = self;
            index.retain(|_, stack| {
                stack.retain(|id| {
                    let keep = arena[id.index()].scope.level() < dying;
                    if !keep {
                        retired.push(*id);
                    }
                    keep
                });
                !stack.is_empty()
            });
        }
        retired.sort_unstable();
        for id in retired {
            self.arena[id.index()].retired = true;
            self.report_unused(id);
        }
    }

    // ----------------------------------------------------------- declaration

    /// Declares a symbol at the current level and returns its id.
    ///
    /// Returns the *existing* id, without inserting anything, when the name is
    /// already declared at this level (error 21) - a caller that keeps using the
    /// returned id therefore keeps working on a coherent symbol instead of
    /// tracking a duplicate.
    ///
    /// Diagnostics this may produce:
    ///
    /// * **200** `symbol "%s" is truncated to %d characters` - the name is
    ///   longer than [`NAME_MAX`]; the stored name is the truncated one, so two
    ///   long names with a common prefix genuinely collide, exactly as upstream.
    /// * **21** `symbol already defined: "%s"` - a second declaration at the
    ///   same level. A definition that follows a matching prototype is *not* a
    ///   redefinition; see below.
    /// * **25** `function heading differs from prototype` - a definition whose
    ///   heading does not match the `forward`/`native` heading already seen.
    /// * **208** `function with tag result used before definition, forcing
    ///   reparse` - the heading was synthesised by an earlier call and the real
    ///   definition turns out to have a return tag.
    /// * **219** `local variable "%s" shadows a variable at a preceding level` -
    ///   a local whose name already exists at an outer level.
    pub fn declare(&mut self, decl: SymbolDecl) -> SymbolId {
        let SymbolDecl { name, kind, usage, span, sig } = decl;
        let name = self.truncate_name(name, span);

        if let Some(existing) = self.lookup(&name) {
            let same_level = self.arena[existing.index()].scope.level() == self.level;
            if same_level {
                if self.arena[existing.index()].kind.is_function() && kind.is_function() {
                    return self.merge_function(existing, kind, usage, span, sig);
                }
                self.emit(21, span, &[&name]);
                return existing;
            }
            // Shadowing is legal; warn only for storage, since a local constant
            // or a nested label reuses a name without hiding an lvalue.
            if self.level > 0 && kind.is_variable_like() {
                self.emit(219, span, &[&name]);
            }
        }

        self.insert(name, kind, usage, span, sig)
    }

    /// Convenience wrapper for the overwhelmingly common case.
    pub fn declare_var(&mut self, name: impl Into<String>, span: Span, usage: Usage) -> SymbolId {
        self.declare(SymbolDecl::new(name, SymKind::Variable, span).with_usage(usage))
    }

    fn insert(
        &mut self,
        name: String,
        kind: SymKind,
        mut usage: Usage,
        span: Span,
        sig: Option<FuncSig>,
    ) -> SymbolId {
        let scope = if self.level == 0 {
            if usage.contains(Usage::STATIC) { Scope::Static } else { Scope::Global }
        } else {
            Scope::Local { level: self.level }
        };
        if kind == SymKind::Native {
            usage |= Usage::NATIVE | Usage::PROTOTYPED;
        }
        let def_span = usage.contains(Usage::DEFINED).then_some(span);
        let id = SymbolId(self.arena.len() as u32);
        self.arena.push(Symbol {
            name: name.clone(),
            kind,
            usage,
            scope,
            span,
            def_span,
            sig,
            retired: false,
        });
        self.index.entry(name).or_default().push(id);
        id
    }

    /// Reconciles a second function heading for a name already in the table.
    ///
    /// Pawn allows exactly one shape of repetition: a heading (`forward`,
    /// `native`, or an implicit forward created by an earlier call) followed by
    /// the definition. Anything else - two definitions, or a `forward` after the
    /// definition - is error 21.
    fn merge_function(
        &mut self,
        existing: SymbolId,
        kind: SymKind,
        usage: Usage,
        span: Span,
        sig: Option<FuncSig>,
    ) -> SymbolId {
        let already_defined = self.get(existing).usage.contains(Usage::DEFINED);
        let now_defining = usage.contains(Usage::DEFINED);

        if already_defined || !now_defining {
            let name = self.get(existing).name.clone();
            self.emit(21, span, &[&name]);
            return existing;
        }

        // Heading comparison. An implicit forward has no real heading to compare
        // against, so it never triggers error 25 - upstream reaches the same
        // conclusion by reparsing instead.
        let implicit = self.get(existing).usage.contains(Usage::IMPLICIT_FORWARD);
        if !implicit
            && let (Some(proto), Some(def)) = (self.get(existing).sig.as_ref(), sig.as_ref())
            && proto != def
        {
            self.emit(25, span, &[]);
        }
        if implicit && sig.as_ref().is_some_and(|s| s.return_tag.is_some()) {
            self.emit(208, span, &[]);
        }

        let sym = self.get_mut(existing);
        sym.usage |= usage | Usage::PROTOTYPED;
        sym.usage.remove(Usage::MISSING | Usage::IMPLICIT_FORWARD);
        sym.def_span = Some(span);
        if sym.sig.is_none() || implicit {
            sym.sig = sig;
        }
        if kind == SymKind::Native {
            sym.kind = SymKind::Native;
        }
        existing
    }

    // ------------------------------------------------------------ references

    /// Records a use of `name` and returns the symbol it resolved to.
    ///
    /// * A [`RefKind::Call`] on an unknown name creates an implicit forward
    ///   declaration (upstream's `uFORWARD|uMISSING` function symbol) at file
    ///   scope, so `foo()` before `foo() {}` is accepted. If no definition ever
    ///   arrives, [`SymbolTable::finish`] reports **error 4**, `function "%s" is
    ///   not implemented` - which is what upstream reports too, *not* error 17.
    /// * Any other use of an unknown name is **error 17**, `undefined symbol
    ///   "%s"`, emitted immediately. This is what makes the table single-pass:
    ///   a global variable used above its declaration is rejected because the
    ///   lookup happens while the table is still in the state that source
    ///   position implies.
    pub fn reference(&mut self, name: &str, span: Span, kind: RefKind) -> Option<SymbolId> {
        let name = self.truncate_name(name.to_owned(), span);
        match self.lookup(&name) {
            Some(id) => {
                self.mark(id, kind);
                Some(id)
            }
            None if kind == RefKind::Call => {
                let id = self.insert_implicit_forward(name, span);
                self.pending_calls.push((id, span));
                Some(id)
            }
            None => {
                self.emit(17, span, &[&name]);
                None
            }
        }
    }

    fn insert_implicit_forward(&mut self, name: String, span: Span) -> SymbolId {
        // Always file scope: Pawn has no nested functions, so the synthesised
        // heading must outlive the block the call appeared in.
        let saved = std::mem::replace(&mut self.level, 0);
        let usage = Usage::PROTOTYPED | Usage::MISSING | Usage::IMPLICIT_FORWARD | Usage::READ;
        let id = self.insert(name, SymKind::Function, usage, span, None);
        self.level = saved;
        id
    }

    /// Applies the `uREAD`/`uWRITTEN` bits a reference implies.
    pub fn mark(&mut self, id: SymbolId, kind: RefKind) {
        let sym = self.get_mut(id);
        match kind {
            RefKind::Read | RefKind::Call => sym.usage |= Usage::READ,
            RefKind::Write => sym.usage |= Usage::WRITTEN,
            RefKind::ReadWrite => sym.usage |= Usage::READ | Usage::WRITTEN,
        }
    }

    /// Innermost visible declaration of `name`, or `None`.
    ///
    /// The per-name stack is ordered by declaration, so `last()` is the
    /// innermost binding and locals shadow globals without any scan.
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        self.index.get(name).and_then(|stack| stack.last().copied())
    }

    /// Innermost declaration of `name` that is a local (level > 0).
    /// The analogue of upstream's `findloc`.
    pub fn lookup_local(&self, name: &str) -> Option<SymbolId> {
        let id = self.lookup(name)?;
        (!self.get(id).scope.is_global()).then_some(id)
    }

    /// The file-scope declaration of `name`, ignoring any local shadowing it.
    /// The analogue of upstream's `findglb`.
    pub fn lookup_global(&self, name: &str) -> Option<SymbolId> {
        self.index
            .get(name)?
            .iter()
            .find(|id| self.arena[id.index()].scope.is_global())
            .copied()
    }

    // ---------------------------------------------------------------- finish

    /// Settles everything that could not be decided in source order and returns
    /// the table's diagnostics, sorted by source position.
    ///
    /// Three things happen here:
    ///
    /// 1. every implicit forward that never got a definition becomes **error 4**
    ///    (`function "%s" is not implemented`), reported at the call site that
    ///    created it;
    /// 2. still-live globals get their **203**/**204** unused report;
    /// 3. the buffer is sorted by `(span.start, code)`. Sorting is what keeps
    ///    output independent of `HashMap` iteration order; the secondary key
    ///    breaks ties for two diagnostics on the same token, and the sort is
    ///    stable so equal keys keep declaration order.
    pub fn finish(&mut self) -> Vec<Diagnostic> {
        debug_assert!(!self.finished, "finish() called twice");
        self.finished = true;

        for (id, span) in std::mem::take(&mut self.pending_calls) {
            if self.get(id).usage.contains(Usage::MISSING) {
                let name = self.get(id).name.clone();
                self.emit(4, span, &[&name]);
            }
        }

        let live: Vec<SymbolId> = (0..self.arena.len())
            .map(|i| SymbolId(i as u32))
            .filter(|id| !self.arena[id.index()].retired)
            .collect();
        for id in live {
            self.arena[id.index()].retired = true;
            self.report_unused(id);
        }

        let mut diags = std::mem::take(&mut self.diags);
        diags.sort_by_key(|d| (d.span.start, d.code));
        diags
    }

    /// Emits 203 or 204 for one symbol, if it wants a report at all.
    ///
    /// * **203** `symbol is never used: "%s"` - never read and never written.
    /// * **204** `symbol is assigned a value that is never used: "%s"` - written
    ///   but never read. Both hinge on `uREAD`; the split is only about whether
    ///   `uWRITTEN` is set, matching upstream.
    ///
    /// A function is judged by `uREAD` alone (it is never "assigned"), and a
    /// declared-but-undefined function is left to error 4 instead of being
    /// double-reported here.
    fn report_unused(&mut self, id: SymbolId) {
        let sym = &self.arena[id.index()];
        if !sym.wants_unused_report() || sym.usage.contains(Usage::READ) {
            return;
        }
        if sym.kind.is_function() && !sym.usage.contains(Usage::DEFINED) {
            return;
        }
        let code = if sym.usage.contains(Usage::WRITTEN) { 204 } else { 203 };
        let (name, span) = (sym.name.clone(), sym.span);
        self.emit(code, span, &[&name]);
    }

    // ----------------------------------------------------------------- utils

    /// Truncates to [`NAME_MAX`] on a character boundary and reports warning 200
    /// (`symbol "%s" is truncated to %d characters`) when it bites. Pawn counts
    /// bytes; identifiers are ASCII, so the boundary walk only guards against a
    /// caller passing something the lexer would never have produced.
    fn truncate_name(&mut self, name: String, span: Span) -> String {
        if name.len() <= NAME_MAX {
            return name;
        }
        let mut cut = NAME_MAX;
        while cut > 0 && !name.is_char_boundary(cut) {
            cut -= 1;
        }
        let short = name[..cut].to_owned();
        self.emit(200, span, &[&short, &cut.to_string()]);
        short
    }

    fn emit(&mut self, code: u16, span: Span, args: &[&str]) {
        self.diags.push(Diagnostic::new(code, span, self.file.clone(), args));
    }

    /// Diagnostics buffered so far, unsorted. For tests and incremental
    /// reporting; prefer [`SymbolTable::finish`] for anything that reaches
    /// output.
    pub fn pending_diagnostics(&self) -> &[Diagnostic] {
        &self.diags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(start: u32) -> Span {
        Span::new(start, start + 1)
    }

    fn table() -> SymbolTable {
        SymbolTable::new("plugin.sma")
    }

    fn codes(diags: &[Diagnostic]) -> Vec<u16> {
        diags.iter().map(|d| d.code).collect()
    }

    fn func_decl(name: &str, span: Span, usage: Usage, sig: FuncSig) -> SymbolDecl {
        SymbolDecl::new(name, SymKind::Function, span).with_usage(usage).with_sig(sig)
    }

    fn sig_untagged(arity: usize) -> FuncSig {
        FuncSig { return_tag: None, params: vec![ParamSig::default(); arity], variadic: false }
    }

    #[test]
    fn local_shadows_global_and_is_restored_on_scope_exit() {
        let mut t = table();
        let g = t.declare_var("count", sp(0), Usage::DEFINED | Usage::READ);
        t.enter_scope();
        let l = t.declare_var("count", sp(10), Usage::DEFINED | Usage::READ);
        assert_ne!(g, l);
        assert_eq!(t.lookup("count"), Some(l), "innermost declaration wins");
        assert_eq!(t.lookup_global("count"), Some(g), "findglb still sees the global");
        assert_eq!(t.lookup_local("count"), Some(l));
        assert_eq!(codes(t.pending_diagnostics()), vec![219], "219 local shadows global");
        t.leave_scope();
        assert_eq!(t.lookup("count"), Some(g), "the local is gone again");
        assert_eq!(t.lookup_local("count"), None);
    }

    #[test]
    fn nested_scopes_pop_only_their_own_level() {
        let mut t = table();
        t.enter_scope();
        let outer = t.declare_var("a", sp(0), Usage::DEFINED | Usage::READ);
        t.enter_scope();
        let inner = t.declare_var("b", sp(5), Usage::DEFINED | Usage::READ);
        assert_eq!(t.lookup("b"), Some(inner));
        t.leave_scope();
        assert_eq!(t.lookup("b"), None);
        assert_eq!(t.lookup("a"), Some(outer));
        t.leave_scope();
        assert_eq!(t.lookup("a"), None);
    }

    #[test]
    fn redefinition_in_the_same_scope_is_error_21() {
        let mut t = table();
        t.declare_var("x", sp(0), Usage::DEFINED | Usage::READ);
        let second = t.declare_var("x", sp(8), Usage::DEFINED | Usage::READ);
        assert_eq!(codes(t.pending_diagnostics()), vec![21]);
        assert_eq!(second, t.lookup("x").unwrap(), "the first symbol is kept");
        assert_eq!(t.symbols().len(), 1, "no duplicate lands in the arena");
    }

    #[test]
    fn undefined_value_reference_is_error_17() {
        let mut t = table();
        assert_eq!(t.reference("nope", sp(3), RefKind::Read), None);
        assert_eq!(codes(t.pending_diagnostics()), vec![17]);
    }

    #[test]
    fn a_call_before_the_definition_is_accepted() {
        let mut t = table();
        // foo() is called at offset 0 and only defined at offset 40.
        let called = t.reference("foo", sp(0), RefKind::Call).unwrap();
        assert!(t.get(called).usage.contains(Usage::MISSING));
        let defined = t.declare(func_decl(
            "foo",
            sp(40),
            Usage::DEFINED | Usage::PROTOTYPED,
            sig_untagged(0),
        ));
        assert_eq!(called, defined, "the definition fills in the implicit forward");
        assert!(!t.get(called).usage.contains(Usage::MISSING));
        assert!(t.finish().is_empty());
    }

    #[test]
    fn a_call_that_is_never_defined_is_error_4() {
        let mut t = table();
        t.reference("ghost", sp(7), RefKind::Call);
        let diags = t.finish();
        assert_eq!(codes(&diags), vec![4]);
        assert!(diags[0].message.contains("ghost"));
        assert_eq!(diags[0].span, sp(7), "reported at the call site");
    }

    #[test]
    fn implicit_forward_of_a_tagged_function_warns_208() {
        let mut t = table();
        // helper() is called before it is known to return Float:.
        t.reference("helper", sp(0), RefKind::Call);
        let sig = FuncSig { return_tag: Some("Float".into()), ..sig_untagged(0) };
        t.declare(func_decl("helper", sp(30), Usage::DEFINED | Usage::PROTOTYPED, sig));
        assert_eq!(codes(t.pending_diagnostics()), vec![208]);
        // No error 4 and no 203: the definition satisfied the implicit forward
        // and the call counts as a read.
        assert_eq!(codes(&t.finish()), vec![208]);
    }

    #[test]
    fn prototype_mismatch_is_error_25_and_a_match_is_silent() {
        let proto = FuncSig {
            return_tag: None,
            params: vec![ParamSig { tags: vec!["Float".into()], ..ParamSig::default() }],
            variadic: false,
        };

        let mut ok = table();
        ok.declare(func_decl("f", sp(0), Usage::PROTOTYPED, proto.clone()));
        ok.declare(func_decl("f", sp(20), Usage::DEFINED | Usage::PROTOTYPED, proto.clone()));
        assert!(ok.pending_diagnostics().is_empty(), "matching heading is silent");

        let mut bad = table();
        bad.declare(func_decl("f", sp(0), Usage::PROTOTYPED, proto));
        let different = FuncSig {
            return_tag: None,
            params: vec![ParamSig { by_ref: true, ..ParamSig::default() }],
            variadic: false,
        };
        bad.declare(func_decl("f", sp(20), Usage::DEFINED | Usage::PROTOTYPED, different));
        assert_eq!(codes(bad.pending_diagnostics()), vec![25]);
    }

    #[test]
    fn two_definitions_of_one_function_are_error_21() {
        let mut t = table();
        t.declare(func_decl("f", sp(0), Usage::DEFINED | Usage::PROTOTYPED, sig_untagged(0)));
        t.declare(func_decl("f", sp(20), Usage::DEFINED | Usage::PROTOTYPED, sig_untagged(0)));
        assert_eq!(codes(t.pending_diagnostics()), vec![21]);
    }

    #[test]
    fn unused_local_is_203_and_write_only_local_is_204() {
        let mut t = table();
        t.enter_scope();
        t.declare_var("never", sp(0), Usage::DEFINED);
        let w = t.declare_var("stored", sp(10), Usage::DEFINED);
        t.mark(w, RefKind::Write);
        let r = t.declare_var("used", sp(20), Usage::DEFINED);
        t.mark(r, RefKind::Read);
        t.leave_scope();
        let diags = t.finish();
        assert_eq!(codes(&diags), vec![203, 204], "sorted by position, `used` is silent");
        assert!(diags[0].message.contains("never"));
        assert!(diags[1].message.contains("stored"));
    }

    #[test]
    fn stock_and_public_are_exempt_from_unused_reports() {
        let mut t = table();
        t.declare(func_decl("lib_helper", sp(0), Usage::DEFINED | Usage::STOCK, sig_untagged(0)));
        t.declare(func_decl(
            "plugin_init",
            sp(20),
            Usage::DEFINED | Usage::PUBLIC,
            sig_untagged(0),
        ));
        t.declare(SymbolDecl::new("g_stockvar", SymKind::Variable, sp(40)).with_usage(Usage::STOCK));
        t.declare(SymbolDecl::new("native_fn", SymKind::Native, sp(60)).with_sig(sig_untagged(0)));
        assert!(t.finish().is_empty(), "stock/public/native are never 'unused'");
    }

    #[test]
    fn an_unused_plain_global_function_is_reported() {
        let mut t = table();
        t.declare(func_decl("dead", sp(0), Usage::DEFINED | Usage::PROTOTYPED, sig_untagged(0)));
        assert_eq!(codes(&t.finish()), vec![203]);
    }

    #[test]
    fn overlong_names_warn_200_and_are_truncated() {
        let long = "a".repeat(NAME_MAX + 5);
        let mut t = table();
        let id = t.declare_var(long.clone(), sp(0), Usage::DEFINED | Usage::READ);
        assert_eq!(t.get(id).name.len(), NAME_MAX);
        assert_eq!(codes(t.pending_diagnostics()), vec![200]);
        // A reference written with the full name still finds the truncated symbol.
        assert_eq!(t.reference(&long, sp(80), RefKind::Read), Some(id));
    }

    #[test]
    fn static_globals_are_file_scoped_but_still_globally_visible() {
        let mut t = table();
        let id = t.declare(
            SymbolDecl::new("g_cache", SymKind::Variable, sp(0))
                .with_usage(Usage::DEFINED | Usage::STATIC | Usage::READ),
        );
        assert_eq!(t.get(id).scope, Scope::Static);
        assert!(t.get(id).scope.is_global());
        t.enter_scope();
        assert_eq!(t.lookup("g_cache"), Some(id));
        t.leave_scope();
    }

    #[test]
    fn diagnostic_order_is_deterministic_across_runs() {
        // Declaring many names exercises HashMap iteration during leave_scope();
        // the reported order must still be source order.
        let run = || {
            let mut t = table();
            t.enter_scope();
            for i in 0..64u32 {
                t.declare_var(format!("v{i:02}"), sp(i * 10), Usage::DEFINED);
            }
            t.leave_scope();
            t.finish()
                .into_iter()
                .map(|d| (d.span.start, d.code, d.message))
                .collect::<Vec<_>>()
        };
        let first = run();
        assert_eq!(first.len(), 64);
        assert!(first.windows(2).all(|w| w[0].0 < w[1].0), "ascending by position");
        for _ in 0..5 {
            assert_eq!(run(), first);
        }
    }

    #[test]
    fn usage_flags_compose_like_the_c_bitfield() {
        let mut u = Usage::DEFINED | Usage::READ;
        assert!(u.contains(Usage::DEFINED | Usage::READ));
        assert!(!u.contains(Usage::WRITTEN));
        assert!(u.intersects(Usage::READ | Usage::WRITTEN));
        u.insert(Usage::STOCK);
        assert!(u.contains(Usage::STOCK));
        u.remove(Usage::STOCK | Usage::READ);
        assert_eq!(u, Usage::DEFINED);
    }

    #[test]
    fn func_sig_arity_accounts_for_defaults() {
        let sig = FuncSig {
            return_tag: None,
            params: vec![
                ParamSig::default(),
                ParamSig { has_default: true, ..ParamSig::default() },
            ],
            variadic: false,
        };
        assert_eq!(sig.arity(), 2);
        assert_eq!(sig.min_arity(), 1);
    }
}
