//! The Pawn preprocessor, ported from `preprocess()`, `command()`, `substpattern()`
//! and `substallpatterns()` in `compiler/libpc300/sc2.c` (Pawn compiler,
//! (c) ITB CompuPhase, zlib-style licence - see ATTRIBUTION.md).
//!
//! Like the original this is a *text* pass: it reads the input a logical line at a
//! time, strips comments, acts on `#` directives and rewrites the line buffer with
//! macro substitutions before anything is lexed. Keeping that shape matters because
//! Pawn's macros are pattern based rather than token based - `#define FOO(%1) bar(%1)`
//! matches raw characters, so a token-level expander would not reproduce it.
//!
//! # Pawn macro semantics (differences from C)
//!
//! * A macro is keyed by the *alphanumeric prefix* of its pattern, and there is at
//!   most one macro per prefix. `#define max(%1,%2) ...` is stored under `max`.
//! * Parameters are `%0`..`%9`, written directly in the pattern; there is no
//!   parameter list. `#define FOO(%1)` and `#define FOO[%1]` are both legal, as is
//!   `#define add %1 plus %2`.
//! * A parameter is matched by scanning forward to the *literal character that
//!   follows it in the pattern*, skipping over strings and balanced `()`/`[]`/`{}`
//!   groups on the way. A trailing `%N` (one with nothing after it) is stripped at
//!   definition time because it would match anything.
//! * In the pattern, whitespace between two different non-alphanumeric characters is
//!   optional in the source; pattern characters go through escape processing.
//! * In the substitution, `#%1` stringizes, and string literals are copied verbatim.
//! * Substitution is re-scanned from the same position, so a macro may expand into
//!   another macro. The C compiler has no recursion guard at all (a self-referential
//!   macro hangs it); this port stops after [`MAX_SUBST_STEPS`] rewrites on one line
//!   and reports error 75.
//! * Macros are never expanded inside string or character literals.
//!
//! # Span mapping (limitation)
//!
//! Substitution changes line contents, so byte offsets in the expanded text do not
//! correspond to byte offsets in any original file. This pass therefore records a
//! [`LineMap`]: one entry per *output line*, naming the original file and 1-based
//! line it came from. Mapping is **line granular** - a position inside an expanded
//! line cannot be traced back to a column in the source. Diagnostics raised by the
//! preprocessor itself do carry exact byte spans, because they are emitted while the
//! original line is still in hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zpc_diag::{Diagnostics, Span};

use crate::token::TokenKind;

/// `sLINEMAX` from `sc.h`: the longest line the compiler will hold after substitution.
pub const LINE_MAX: usize = 4095;
/// `sCOMP_STACK` from `sc.h`: maximum nesting of `#if ... #endif`.
pub const COMP_STACK: usize = 32;
/// Maximum `#include` nesting. The C compiler has no explicit limit and dies on a
/// cycle; this bound is what turns an include cycle into a diagnostic.
pub const MAX_INCLUDE_DEPTH: usize = 32;
/// Maximum successful macro rewrites on a single line before giving up.
pub const MAX_SUBST_STEPS: usize = 512;

/// The escape character before any `#pragma ctrlchar`.
///
/// Pawn's default is `^`, NOT the backslash C programmers expect (`sc2.c` sets
/// `sc_ctrlchar = CTRL_CHAR` with `#define CTRL_CHAR '^'`). This must agree with
/// [`crate::scanner::DEFAULT_CTRL_CHAR`]: the driver hands this value to the
/// scanner, so a disagreement silently breaks every `^` escape in every include.
pub const DEFAULT_CTRLCHAR: u8 = crate::scanner::DEFAULT_CTRL_CHAR;

// --- character classes (sc2.c: alpha/alphanum/ishex) ------------------------

/// `alpha()`: a letter, `_`, or `@` (the "public" character).
fn alpha(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'@'
}

/// `alphanum()`: [`alpha`] or a digit.
fn alphanum(c: u8) -> bool {
    alpha(c) || c.is_ascii_digit()
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

// --- pragmas ----------------------------------------------------------------

/// Everything the later phases need to know about `#pragma`, plus the escape
/// character, which the scanner must have to lex string literals correctly.
#[derive(Clone, Debug)]
pub struct PreprocState {
    /// `#pragma ctrlchar` - the escape character inside literals. This is the
    /// value in effect at the END of the unit; for scanning, use
    /// [`PreprocState::ctrlchar_changes`], because the setting is positional.
    pub ctrlchar: u8,
    /// Every `#pragma ctrlchar`, as `(0-based output line, new character)` in
    /// source order. The scanner applies each one as it reaches that line, which
    /// is what makes a change affect only the text after it.
    pub ctrlchar_changes: Vec<(u32, u8)>,
    /// `#pragma semicolon` - when true, statements must end with `;`.
    pub semicolon: bool,
    /// `#pragma tabsize`.
    pub tabsize: u32,
    /// `#pragma amxlimit`.
    pub amxlimit: i64,
    /// `#pragma dynamic` - stack/heap size in cells.
    pub dynamic: i64,
    /// `#pragma compress`.
    pub compress: bool,
    /// `#pragma pack` - default packed/unpacked strings.
    pub pack: bool,
    /// `#pragma codepage`.
    pub codepage: Option<String>,
    /// `#pragma rational <tag>(<digits>)`.
    pub rational: Option<(String, u32)>,
    /// `#pragma library` / `reqlib` / `reqclass` / ... in source order, prefix applied.
    pub libraries: Vec<String>,
    /// Names listed by `#pragma unused`, in source order. Resolving them needs the
    /// symbol table, so that is left to a later phase.
    pub unused: Vec<String>,
    /// True once `#pragma align` was seen (it applies to the next declaration).
    pub align_next: bool,
    /// `#pragma showstackusageinfo`.
    pub stack_usage_info: bool,
}

impl Default for PreprocState {
    fn default() -> Self {
        Self {
            ctrlchar: DEFAULT_CTRLCHAR,
            semicolon: false,
            tabsize: 8,
            amxlimit: 0,
            dynamic: 0,
            compress: true,
            pack: false,
            codepage: None,
            rational: None,
            libraries: Vec::new(),
            unused: Vec::new(),
            align_next: false,
            stack_usage_info: false,
            ctrlchar_changes: Vec::new(),
        }
    }
}

// --- line map ---------------------------------------------------------------

/// Maps each line of the expanded text back to the file and line it came from.
/// Interned file names keep this compact; order of `files` is insertion order, so
/// it is deterministic.
#[derive(Clone, Debug, Default)]
pub struct LineMap {
    files: Vec<PathBuf>,
    /// `(file index, 1-based source line)` per output line.
    lines: Vec<(u32, u32)>,
}

impl LineMap {
    fn intern(&mut self, path: &Path) -> u32 {
        if let Some(i) = self.files.iter().position(|p| p == path) {
            return i as u32;
        }
        self.files.push(path.to_path_buf());
        (self.files.len() - 1) as u32
    }

    fn push(&mut self, file: u32, line: u32) {
        self.lines.push((file, line));
    }

    /// Origin of a 0-based output line.
    pub fn origin(&self, out_line: usize) -> Option<(&Path, u32)> {
        self.lines
            .get(out_line)
            .map(|&(f, l)| (self.files[f as usize].as_path(), l))
    }

    /// First expanded-text line produced by `source_line` in `file`.
    pub fn output_line(&self, file: &Path, source_line: u32) -> Option<usize> {
        let file_id = self.files.iter().position(|candidate| candidate == file)? as u32;
        self.lines
            .iter()
            .position(|&(candidate, line)| candidate == file_id && line == source_line)
    }

    /// Number of output lines recorded.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Every file that contributed a line, in the order first reached.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

/// Result of a preprocessing run.
#[derive(Clone, Debug)]
pub struct Preprocessed {
    /// The expanded source. One line per [`LineMap`] entry; directive and skipped
    /// lines become blank lines so that line numbers inside a single file stay 1:1.
    pub text: String,
    pub map: LineMap,
    pub state: PreprocState,
}

// --- source access ----------------------------------------------------------

/// How the preprocessor reads `#include` targets. Injecting this keeps include
/// resolution testable without touching the filesystem.
pub trait SourceProvider {
    /// Returns the contents of `path`, or `None` if it cannot be read.
    fn read(&self, path: &Path) -> Option<String>;
}

/// Reads from the real filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsProvider;

impl SourceProvider for FsProvider {
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read(path)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// An in-memory file set, keyed by exact path. Used by tests and by callers that
/// already hold the sources.
#[derive(Clone, Debug, Default)]
pub struct MemProvider {
    files: BTreeMap<PathBuf, String>,
}

impl MemProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> &mut Self {
        self.files.insert(path.into(), text.into());
        self
    }
}

impl SourceProvider for MemProvider {
    fn read(&self, path: &Path) -> Option<String> {
        self.files.get(path).cloned()
    }
}

// --- macro table ------------------------------------------------------------

#[derive(Clone, Debug)]
struct MacroDef {
    /// Full pattern, escapes already resolved, trailing `%N` stripped.
    pattern: Vec<u8>,
    /// Replacement text, taken verbatim.
    subst: Vec<u8>,
    /// Message from a preceding `#pragma deprecated`, if any.
    deprecated: Option<String>,
}

// --- conditional-compilation stack ------------------------------------------

const SKIP_MODE: u8 = 1;
const PARSE_MODE: u8 = 2;
const HANDLED_ELSE: u8 = 4;

// --- the preprocessor -------------------------------------------------------

/// A Pawn preprocessor. Reusable across runs; macros defined with [`Preprocessor::define`]
/// survive between calls to [`Preprocessor::process`], directive state does not.
pub struct Preprocessor {
    include_dirs: Vec<PathBuf>,
    provider: Box<dyn SourceProvider>,
    /// Keyed by the alphanumeric prefix of the pattern, matching `find_subst()`.
    macros: BTreeMap<String, MacroDef>,
    state: PreprocState,
    diags: Diagnostics,

    // per-run state
    out: String,
    map: LineMap,
    ifstack: Vec<u8>,
    skiplevel: usize,
    icomment: u8,
    depth: usize,
    /// File the current line physically came from (used for diagnostics and includes).
    cur_path: PathBuf,
    cur_file_id: u32,
    /// Name reported to diagnostics; `#file` can override it.
    report_name: PathBuf,
    /// Set by `#endinput` / `#endscript`.
    stop_file: bool,
    /// Message from `#pragma deprecated`, attached to the next `#define`.
    pending_deprecate: Option<String>,
}

impl Preprocessor {
    /// A preprocessor reading includes from the real filesystem.
    pub fn new(include_dirs: Vec<PathBuf>) -> Self {
        Self::with_provider(include_dirs, Box::new(FsProvider))
    }

    /// A preprocessor reading includes through `provider`.
    pub fn with_provider(include_dirs: Vec<PathBuf>, provider: Box<dyn SourceProvider>) -> Self {
        let mut pp = Self {
            include_dirs,
            provider,
            macros: BTreeMap::new(),
            state: PreprocState::default(),
            diags: Diagnostics::new(),
            out: String::new(),
            map: LineMap::default(),
            ifstack: Vec::new(),
            skiplevel: 0,
            icomment: 0,
            depth: 0,
            cur_path: PathBuf::new(),
            cur_file_id: 0,
            report_name: PathBuf::new(),
            stop_file: false,
            pending_deprecate: None,
        };
        let (date, time) = local_date_time();
        pp.define("__DATE__", &format!("\"{date}\""));
        pp.define("__TIME__", &format!("\"{time}\""));
        pp
    }

    /// Predefine a macro, as `-D` would. `pattern` may carry `%N` parameters.
    pub fn define(&mut self, pattern: &str, subst: &str) {
        let pat = pattern.as_bytes().to_vec();
        let prefix: String = pattern.bytes().take_while(|c| alphanum(*c)).map(char::from).collect();
        if prefix.is_empty() {
            return;
        }
        self.macros.insert(
            prefix,
            MacroDef { pattern: pat, subst: subst.as_bytes().to_vec(), deprecated: None },
        );
    }

    /// True if a macro with this prefix name is defined.
    pub fn is_defined(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    /// Preprocess `src`, which is the contents of `path`. Consumes the run state and
    /// hands back the expanded text with its line map, the pragma state and every
    /// diagnostic raised.
    pub fn process(&mut self, path: &Path, src: &str) -> (Preprocessed, Diagnostics) {
        self.out.clear();
        self.map = LineMap::default();
        self.ifstack.clear();
        self.skiplevel = 0;
        self.icomment = 0;
        self.depth = 0;
        self.stop_file = false;

        // `__FILE__` tracks the file currently being read (sc1.c: inst_file_name).
        self.process_file(path, src);

        let done = Preprocessed {
            text: std::mem::take(&mut self.out),
            map: std::mem::take(&mut self.map),
            state: self.state.clone(),
        };
        (done, std::mem::take(&mut self.diags))
    }

    fn emit(&mut self, code: u16, span: Span, args: &[&str]) {
        let file = self.report_name.clone();
        self.diags.emit(code, span, &file, args);
    }

    fn skipping(&self) -> bool {
        self.skiplevel > 0 && self.ifstack[self.skiplevel - 1] & SKIP_MODE == SKIP_MODE
    }

    // --- driving one file ---------------------------------------------------

    fn process_file(&mut self, path: &Path, src: &str) {
        if self.depth >= MAX_INCLUDE_DEPTH {
            // Real cycles land here; the C compiler simply exhausts its stack.
            let name = path.display().to_string();
            self.emit(102, Span::at(0), &[&format!("include nesting ({name})")]);
            return;
        }
        self.depth += 1;

        let file_id = self.map.intern(path);
        let saved_path = std::mem::replace(&mut self.cur_path, path.to_path_buf());
        let saved_report = std::mem::replace(&mut self.report_name, path.to_path_buf());
        let saved_file_id = std::mem::replace(&mut self.cur_file_id, file_id);
        let saved_comment = std::mem::replace(&mut self.icomment, 0);
        let saved_stop = std::mem::replace(&mut self.stop_file, false);
        let entry_iflevel = self.ifstack.len();
        let saved_skiplevel = self.skiplevel;
        self.define("__FILE__", &format!("\"{}\"", file_name_of(path)));

        self.run_lines(src, entry_iflevel);

        // sc2.c readline(): at end of input the #if stack must be empty and we must
        // not be inside a comment. `#endinput` bypasses both checks, as it does in C
        // (the file is simply popped, nothing is verified).
        if !self.stop_file {
            if self.ifstack.len() > entry_iflevel {
                self.emit(1, Span::at(src.len() as u32), &["#endif", "-end of file-"]);
            }
            if self.icomment != 0 {
                self.emit(1, Span::at(src.len() as u32), &["*/", "-end of file-"]);
            }
        }
        self.ifstack.truncate(entry_iflevel);
        self.skiplevel = saved_skiplevel;
        self.icomment = saved_comment;
        self.stop_file = saved_stop;
        self.cur_file_id = saved_file_id;
        self.report_name = saved_report;
        self.cur_path = saved_path;
        self.define("__FILE__", &format!("\"{}\"", file_name_of(&self.cur_path.clone())));
        self.depth -= 1;
    }

    fn run_lines(&mut self, src: &str, entry_iflevel: usize) {
        let bytes = src.as_bytes();
        let starts = line_starts(bytes);
        let mut idx = 0usize;

        while idx < starts.len() {
            if self.stop_file || self.diags.aborted() {
                break;
            }
            let first_line = idx;
            // readline(): join lines whose last non-blank character is a backslash.
            let mut logical: Vec<u8> = Vec::new();
            loop {
                let (s, e) = starts[idx];
                let mut seg: &[u8] = &bytes[s..e];
                if !logical.is_empty() {
                    // continuation lines lose their leading whitespace
                    let lead = seg.iter().take_while(|c| **c <= b' ').count();
                    seg = &seg[lead..];
                }
                let mut trimmed = seg;
                while let Some((last, rest)) = trimmed.split_last() {
                    if *last <= b' ' {
                        trimmed = rest;
                    } else {
                        break;
                    }
                }
                let continued = trimmed.last() == Some(&b'\\');
                if continued {
                    logical.extend_from_slice(&trimmed[..trimmed.len() - 1]);
                    // '\a' marks the join so a `//` comment can still report error 49.
                    logical.push(7);
                    idx += 1;
                    if idx >= starts.len() {
                        self.emit(49, Span::new(s as u32, e as u32), &[]);
                        break;
                    }
                } else {
                    logical.extend_from_slice(seg);
                    idx += 1;
                    break;
                }
            }
            let span = Span::new(starts[first_line].0 as u32, starts[idx - 1].1 as u32);
            let src_line = (first_line + 1) as u32;
            if logical.len() > LINE_MAX {
                self.emit(75, span, &[]);
                logical.truncate(LINE_MAX);
            }

            self.strip_comments(&mut logical, span);
            self.handle_line(logical, span, src_line, entry_iflevel);

            // keep within-file line numbering 1:1 for continued lines
            for extra in (first_line + 1)..idx {
                self.push_line("", (extra + 1) as u32);
            }
        }
    }

    fn push_line(&mut self, text: &str, src_line: u32) {
        self.out.push_str(text);
        self.out.push('\n');
        let f = self.cur_file_id;
        self.map.push(f, src_line);
    }

    /// `preprocess()`: run `command()`, then substitute macros on ordinary lines.
    fn handle_line(&mut self, mut line: Vec<u8>, span: Span, src_line: u32, entry_iflevel: usize) {
        self.define("__LINE__", &src_line.to_string());
        match self.command(&line, span, entry_iflevel) {
            Cmd::None => {
                // `#pragma deprecated` targets the next declaration or `#define`.
                // Declarations are ordinary source lines handled after this
                // preprocessor, so the macro-only pending state must stop here;
                // otherwise it leaks across the declaration and marks an
                // unrelated later macro as deprecated.
                if line.iter().any(|byte| *byte > b' ') {
                    self.pending_deprecate = None;
                }
                self.subst_all(&mut line, span);
                let text = String::from_utf8_lossy(&line).replace('\u{7}', "");
                self.push_line(text.trim_end(), src_line);
            }
            Cmd::Passthrough => {
                let text = String::from_utf8_lossy(&line).replace('\u{7}', "");
                self.push_line(text.trim_end(), src_line);
            }
            _ => self.push_line("", src_line),
        }
    }

    // --- comments (stripcom) ------------------------------------------------

    fn strip_comments(&mut self, line: &mut Vec<u8>, span: Span) {
        let ctrl = self.state.ctrlchar;
        let mut i = 0usize;
        while i < line.len() {
            if self.icomment != 0 {
                if line[i] == b'*' && at(line, i + 1) == b'/' {
                    self.icomment = 0;
                    line[i] = b' ';
                    line[i + 1] = b' ';
                    i += 2;
                } else {
                    if line[i] == b'/' && at(line, i + 1) == b'*' {
                        self.emit(216, span, &[]);
                    }
                    line[i] = b' ';
                    i += 1;
                }
            } else if line[i] == b'/' && at(line, i + 1) == b'*' {
                self.icomment = 1;
                line[i] = b' ';
                line[i + 1] = b' ';
                i += 2;
            } else if line[i] == b'/' && at(line, i + 1) == b'/' {
                if line[i..].contains(&7u8) {
                    self.emit(49, span, &[]);
                }
                line.truncate(i);
                return;
            } else if line[i] == b'"' || line[i] == b'\'' {
                let quote = line[i];
                i += 1;
                while i < line.len() && (line[i] != quote || at(line, i.wrapping_sub(1)) == ctrl) {
                    i += 1;
                }
                i += 1;
            } else {
                i += 1;
            }
        }
    }

    // --- directives (command) ----------------------------------------------

    fn command(&mut self, line: &[u8], span: Span, entry_iflevel: usize) -> Cmd {
        let mut p = skip_ws(line, 0);
        if p >= line.len() {
            return Cmd::EmptyLine;
        }
        if line[p] != b'#' {
            return if self.skipping() { Cmd::CondFalse } else { Cmd::None };
        }
        p = skip_ws(line, p + 1);
        let start = p;
        while p < line.len() && alphanum(line[p]) {
            p += 1;
        }
        let word = String::from_utf8_lossy(&line[start..p]).into_owned();
        let rest: Vec<u8> = line[p..].to_vec();

        let Some(kind) = TokenKind::directive(&word) else {
            self.emit(31, span, &[]);
            return if self.skipping() { Cmd::CondFalse } else { Cmd::None };
        };

        use TokenKind as T;
        match kind {
            T::PpIf => self.do_if(&rest, span),
            T::PpElse | T::PpElseIf => self.do_else(matches!(kind, T::PpElseIf), &rest, span),
            T::PpEndIf => self.do_endif(&rest, span, entry_iflevel),
            T::PpInclude | T::PpTryInclude => {
                if !self.skipping() {
                    self.do_include(&rest, span, matches!(kind, T::PpTryInclude));
                }
                Cmd::Include
            }
            T::PpDefine => {
                if !self.skipping() {
                    self.do_define(&rest, span);
                }
                Cmd::Define
            }
            T::PpUndef => {
                if !self.skipping() {
                    self.do_undef(&rest, span);
                }
                Cmd::Directive
            }
            T::PpPragma => {
                if !self.skipping() {
                    self.do_pragma(&rest, span);
                }
                Cmd::Directive
            }
            T::PpError => {
                if !self.skipping() {
                    let msg = String::from_utf8_lossy(&rest).trim().to_string();
                    self.emit(111, span, &[&msg]);
                }
                Cmd::Directive
            }
            T::PpAssert => {
                if !self.skipping() {
                    let text = String::from_utf8_lossy(&rest).trim().to_string();
                    let (val, tail) = self.eval_cond(&rest, span);
                    if val == 0 {
                        self.emit(110, span, &[&text]);
                    }
                    self.check_empty(&tail, span);
                }
                Cmd::Directive
            }
            T::PpLine => {
                // The line map already tracks real positions, so `#line` is recorded
                // but does not renumber the output.
                if !self.skipping() {
                    let mut i = skip_ws(&rest, 0);
                    if parse_number(&rest, &mut i).is_none() {
                        self.emit(8, span, &[]);
                    }
                }
                Cmd::Directive
            }
            T::PpFile => {
                if !self.skipping() {
                    let mut i = 0usize;
                    match get_string(&rest, &mut i) {
                        Some(name) if !name.is_empty() => {
                            self.report_name = PathBuf::from(&name);
                            self.define("__FILE__", &format!("\"{name}\""));
                        }
                        _ => self.emit(37, span, &[]),
                    }
                }
                Cmd::Directive
            }
            T::PpEndInput | T::PpEndScript => {
                if !self.skipping() {
                    self.stop_file = true;
                }
                Cmd::Directive
            }
            // `#emit` is codegen's business; hand the line through untouched.
            T::PpEmit => {
                if self.skipping() {
                    Cmd::CondFalse
                } else {
                    Cmd::Passthrough
                }
            }
            _ => {
                self.emit(31, span, &[]);
                Cmd::Directive
            }
        }
    }

    fn do_if(&mut self, rest: &[u8], span: Span) -> Cmd {
        if self.ifstack.len() >= COMP_STACK {
            self.emit(102, span, &["Conditional compilation stack"]);
            return Cmd::If;
        }
        let skipping = self.skipping();
        self.ifstack.push(0);
        if skipping {
            // Nested inside a dead branch: the level is tracked but not evaluated.
            return Cmd::If;
        }
        self.skiplevel = self.ifstack.len();
        let (val, tail) = self.eval_cond(rest, span);
        let top = self.ifstack.len() - 1;
        self.ifstack[top] = if val != 0 { PARSE_MODE } else { SKIP_MODE };
        self.check_empty(&tail, span);
        Cmd::If
    }

    fn do_else(&mut self, is_elseif: bool, rest: &[u8], span: Span) -> Cmd {
        if self.ifstack.is_empty() {
            self.emit(26, span, &[]);
            return Cmd::If;
        }
        let top = self.ifstack.len() - 1;
        let level = self.ifstack.len();
        if self.ifstack[top] & HANDLED_ELSE == HANDLED_ELSE {
            // Upstream amxmodx checks this flag but never sets it, so errors 60/61
            // are dead there; this port sets it so the checks actually fire.
            self.emit(if is_elseif { 61 } else { 60 }, span, &[]);
            return Cmd::If;
        }
        let mut tail = rest.to_vec();
        if self.ifstack[top] & PARSE_MODE == PARSE_MODE {
            self.ifstack[top] |= SKIP_MODE;
            if is_elseif && self.skiplevel == level {
                let (_, t) = self.eval_cond(rest, span);
                tail = t;
            }
        } else if is_elseif {
            let val = if self.skiplevel == level {
                let (v, t) = self.eval_cond(rest, span);
                tail = t;
                v
            } else {
                0
            };
            self.ifstack[top] = if val != 0 { PARSE_MODE } else { SKIP_MODE };
        } else {
            self.ifstack[top] &= !SKIP_MODE;
        }
        if !is_elseif {
            self.ifstack[top] |= HANDLED_ELSE;
        }
        self.check_empty(&tail, span);
        Cmd::If
    }

    fn do_endif(&mut self, rest: &[u8], span: Span, entry_iflevel: usize) -> Cmd {
        if self.ifstack.len() <= entry_iflevel {
            self.emit(26, span, &[]);
        } else {
            self.ifstack.pop();
            if self.ifstack.len() < self.skiplevel {
                self.skiplevel = self.ifstack.len();
            }
        }
        let tail = rest.to_vec();
        self.check_empty(&tail, span);
        Cmd::If
    }

    // --- #include -----------------------------------------------------------

    fn do_include(&mut self, rest: &[u8], span: Span, silent: bool) {
        let mut i = skip_ws(rest, 0);
        let terminator = match rest.get(i) {
            Some(b'<') => {
                i += 1;
                b'>'
            }
            Some(b'"') => {
                i += 1;
                b'"'
            }
            _ => 0,
        };
        if terminator != 0 {
            i = skip_ws(rest, i);
        }
        let start = i;
        while i < rest.len() && rest[i] != terminator {
            i += 1;
        }
        let mut end = i;
        while end > start && rest[end - 1] <= b' ' {
            end -= 1;
        }
        let name = String::from_utf8_lossy(&rest[start..end]).into_owned();
        if terminator != 0 {
            if i >= rest.len() {
                self.emit(37, span, &[]);
                return;
            }
            let tail = rest[i + 1..].to_vec();
            self.check_empty(&tail, span);
        }
        if name.is_empty() {
            self.emit(37, span, &[]);
            return;
        }

        // "..." and bare names may come from the current directory; <...> may not.
        match self.resolve_include(&name, terminator != b'>') {
            Some((path, text)) => self.process_file(&path, &text),
            None => {
                if !silent {
                    self.emit(100, span, &[&name]);
                }
            }
        }
    }

    fn resolve_include(&self, name: &str, try_current: bool) -> Option<(PathBuf, String)> {
        const EXTENSIONS: [&str; 4] = ["", ".inc", ".p", ".pawn"];
        let mut bases: Vec<PathBuf> = Vec::new();
        if try_current {
            bases.push(PathBuf::from(name));
            if let Some(dir) = self.cur_path.parent() {
                bases.push(dir.join(name));
            }
        }
        if !Path::new(name).is_absolute() {
            for dir in &self.include_dirs {
                bases.push(dir.join(name));
            }
        }
        for base in bases {
            for ext in EXTENSIONS {
                let cand = if ext.is_empty() {
                    base.clone()
                } else {
                    let mut s = base.clone().into_os_string();
                    s.push(ext);
                    PathBuf::from(s)
                };
                if let Some(text) = self.provider.read(&cand) {
                    return Some((cand, text));
                }
            }
        }
        None
    }

    // --- #define / #undef ---------------------------------------------------

    fn do_define(&mut self, rest: &[u8], span: Span) {
        let ctrl = self.state.ctrlchar;
        let mut i = skip_ws(rest, 0);
        let start = i;
        // Scan the pattern; whitespace inside `( ... )` does not end it.
        let mut in_parens = false;
        while i < rest.len() {
            if rest[i] == b'(' {
                in_parens = true;
            }
            if in_parens && rest[i] == b')' {
                in_parens = false;
            }
            if !in_parens && rest[i] <= b' ' {
                break;
            }
            litchar(rest, &mut i, false, ctrl);
        }
        let end = i;
        if start >= rest.len() || !alpha(rest[start]) {
            self.emit(74, span, &[]);
            return;
        }
        let mut pattern: Vec<u8> = Vec::with_capacity(end - start);
        let mut j = start;
        while j < end {
            pattern.push(litchar(rest, &mut j, false, ctrl) as u8);
        }
        // A trailing `%N` would match anything at all, so it is dropped.
        if pattern.len() >= 2
            && pattern[pattern.len() - 1].is_ascii_digit()
            && pattern[pattern.len() - 2] == b'%'
        {
            pattern.truncate(pattern.len() - 2);
        }

        let mut k = skip_ws(rest, end);
        let sub_start = k;
        let mut sub_end = rest.len();
        while sub_end > sub_start && rest[sub_end - 1] <= b' ' {
            sub_end -= 1;
        }
        k = sub_end.max(sub_start);
        let subst = rest[sub_start..k].to_vec();

        let prefix: String =
            pattern.iter().take_while(|c| alphanum(**c)).map(|c| *c as char).collect();
        if prefix.is_empty() {
            self.emit(74, span, &[]);
            return;
        }
        if let Some(old) = self.macros.get(&prefix)
            && (old.pattern != pattern || old.subst != subst)
        {
            self.emit(201, span, &[&prefix]);
        }
        let deprecated = self.pending_deprecate.take();
        self.macros.insert(prefix, MacroDef { pattern, subst, deprecated });
    }

    fn do_undef(&mut self, rest: &[u8], span: Span) {
        let mut i = skip_ws(rest, 0);
        let start = i;
        while i < rest.len() && alphanum(rest[i]) {
            i += 1;
        }
        if i == start || !alpha(rest[start]) {
            let bad = String::from_utf8_lossy(&rest[start..]).trim().to_string();
            self.emit(20, span, &[&bad]);
            return;
        }
        let name = String::from_utf8_lossy(&rest[start..i]).into_owned();
        if self.macros.remove(&name).is_none() {
            self.emit(17, span, &[&name]);
        }
        let tail = rest[i..].to_vec();
        self.check_empty(&tail, span);
    }

    // --- #pragma ------------------------------------------------------------

    fn do_pragma(&mut self, rest: &[u8], span: Span) {
        let mut i = skip_ws(rest, 0);
        let start = i;
        while i < rest.len() && alphanum(rest[i]) {
            i += 1;
        }
        if i == start {
            self.emit(207, span, &[]);
            return;
        }
        let name = String::from_utf8_lossy(&rest[start..i]).into_owned();
        let arg = &rest[i..];
        let mut tail = arg.to_vec();

        match name.as_str() {
            "amxlimit" => {
                let (v, t) = self.eval_cond(arg, span);
                self.state.amxlimit = v;
                tail = t;
            }
            "dynamic" => {
                let (v, t) = self.eval_cond(arg, span);
                self.state.dynamic = v;
                tail = t;
            }
            "compress" => {
                let (v, t) = self.eval_cond(arg, span);
                self.state.compress = v != 0;
                tail = t;
            }
            "pack" => {
                let (v, t) = self.eval_cond(arg, span);
                self.state.pack = v != 0;
                tail = t;
            }
            "semicolon" => {
                let (v, t) = self.eval_cond(arg, span);
                self.state.semicolon = v != 0;
                tail = t;
            }
            "tabsize" => {
                let (v, t) = self.eval_cond(arg, span);
                if v > 0 {
                    self.state.tabsize = v as u32;
                }
                tail = t;
            }
            "ctrlchar" => {
                let mut j = skip_ws(arg, 0);
                if j >= arg.len() {
                    self.state.ctrlchar = DEFAULT_CTRLCHAR;
                } else {
                    match parse_char_or_number(arg, &mut j, self.state.ctrlchar) {
                        Some(v) => self.state.ctrlchar = v as u8,
                        None => self.emit(27, span, &[]),
                    }
                    tail = arg[j..].to_vec();
                }
                // The control character is POSITIONAL: amxxpc interleaves
                // preprocessing and lexing, so a change applies from here on and
                // leaves everything already read alone. Record where it happened
                // (0-based output line) so the scanner can switch at the same
                // point instead of being handed one value for the whole file -
                // which made a plugin's `#pragma ctrlchar '\'` retroactively
                // re-read every header that was written for '^'.
                let line = self.map.len() as u32;
                self.state.ctrlchar_changes.push((line, self.state.ctrlchar));
            }
            "codepage" => {
                let mut j = skip_ws(arg, 0);
                let page = if arg.get(j) == Some(&b'"') {
                    get_string(arg, &mut j).unwrap_or_default()
                } else {
                    let s = j;
                    while j < arg.len() && alphanum(arg[j]) {
                        j += 1;
                    }
                    String::from_utf8_lossy(&arg[s..j]).into_owned()
                };
                self.state.codepage = Some(page);
                tail = arg[j..].to_vec();
            }
            "deprecated" => {
                let j = skip_ws(arg, 0);
                self.pending_deprecate =
                    Some(String::from_utf8_lossy(&arg[j..]).trim_end().to_string());
                tail = Vec::new();
            }
            "library" | "reqlib" | "reqclass" | "loadlib" | "explib" | "expclass"
            | "defclasslib" => {
                let prefix = match name.as_str() {
                    "reqlib" => "?rl_",
                    "reqclass" => "?rc_",
                    "loadlib" => "?f_",
                    "explib" => "?el_",
                    "expclass" => "?ec_",
                    "defclasslib" => "?d_",
                    _ => "",
                };
                let mut j = skip_ws(arg, 0);
                let lib = if arg.get(j) == Some(&b'"') {
                    get_string(arg, &mut j).unwrap_or_default()
                } else {
                    let s = j;
                    while j < arg.len() && (alphanum(arg[j]) || arg[j] == b'-') {
                        j += 1;
                    }
                    String::from_utf8_lossy(&arg[s..j]).into_owned()
                };
                if !lib.is_empty() && lib != "-" {
                    self.state.libraries.push(format!("{prefix}{lib}"));
                }
                // These pragmas take an OPTIONAL second name - sc2.c reads `name`
                // and then `sname` before skipping to end of line. Reporting the
                // second one as "extra characters" (38) broke every header using
                // `#pragma defclasslib xstats csx`, which is the AMXX autoload
                // idiom and appears in csx.inc, cstrike.inc, fun.inc and others.
                let mut k = skip_ws(arg, j);
                let s2 = k;
                while k < arg.len() && (alphanum(arg[k]) || arg[k] == b'-') {
                    k += 1;
                }
                if k > s2 {
                    let sname = String::from_utf8_lossy(&arg[s2..k]).into_owned();
                    self.state.libraries.push(format!("{prefix}{sname}"));
                    j = k;
                }
                tail = arg[j..].to_vec();
            }
            "rational" => {
                let mut j = skip_ws(arg, 0);
                let s = j;
                while j < arg.len() && alphanum(arg[j]) {
                    j += 1;
                }
                let tag = String::from_utf8_lossy(&arg[s..j]).into_owned();
                let mut digits = 0u32;
                let k = skip_ws(arg, j);
                if arg.get(k) == Some(&b'(') {
                    let (v, t) = self.eval_cond(&arg[k..], span);
                    if !(1..=9).contains(&v) {
                        self.emit(68, span, &[]);
                    } else {
                        digits = v as u32;
                    }
                    tail = t;
                } else {
                    tail = arg[j..].to_vec();
                }
                if self.state.rational.is_some() && self.state.rational != Some((tag.clone(), digits))
                {
                    self.emit(69, span, &[]);
                } else {
                    self.state.rational = Some((tag, digits));
                }
            }
            "unused" => {
                let mut j = 0usize;
                loop {
                    j = skip_ws(arg, j);
                    let s = j;
                    while j < arg.len() && alphanum(arg[j]) {
                        j += 1;
                    }
                    if j > s {
                        self.state.unused.push(String::from_utf8_lossy(&arg[s..j]).into_owned());
                    }
                    j = skip_ws(arg, j);
                    if arg.get(j) == Some(&b',') {
                        j += 1;
                    } else {
                        break;
                    }
                }
                tail = arg[j..].to_vec();
            }
            "align" => self.state.align_next = true,
            "showstackusageinfo" => self.state.stack_usage_info = true,
            _ => {
                self.emit(207, span, &[]);
                return;
            }
        }
        self.check_empty(&tail, span);
    }

    fn check_empty(&mut self, rest: &[u8], span: Span) {
        let i = skip_ws(rest, 0);
        if i < rest.len() && rest[i] != 7 {
            self.emit(38, span, &[]);
        }
    }

    // --- constant expressions ----------------------------------------------

    /// Substitutes macros in `rest` and evaluates it as an integer constant
    /// expression. Returns the value and whatever text followed the expression.
    fn eval_cond(&mut self, rest: &[u8], span: Span) -> (i64, Vec<u8>) {
        let mut buf = rest.to_vec();
        self.subst_all(&mut buf, span);
        let mut ev = Eval { b: &buf, i: 0, macros: &self.macros, ctrl: self.state.ctrlchar, err: None };
        let val = ev.expr();
        let pos = ev.i;
        let err = ev.err;
        if let Some(code) = err {
            self.emit(code, span, &[]);
        }
        (val, buf[pos.min(buf.len())..].to_vec())
    }

    // --- macro substitution (substallpatterns / substpattern) ---------------

    fn subst_all(&mut self, line: &mut Vec<u8>, span: Span) {
        let ctrl = self.state.ctrlchar;
        let mut start = 0usize;
        let mut steps = 0usize;
        while start < line.len() {
            // Find the next identifier, stepping over string and char literals.
            while start < line.len() && !alpha(line[start]) {
                if is_startstring(line, start, ctrl) {
                    start = skipstring(line, start, ctrl);
                    if start >= line.len() {
                        break;
                    }
                }
                start += 1;
            }
            if start >= line.len() {
                break;
            }
            // `defined X` / `defined(X)`: the operand must not be expanded.
            if line[start..].starts_with(b"defined")
                && !at(line, start + 7).is_ascii_alphabetic()
            {
                start += 7;
                while start < line.len() && (line[start] <= b' ' || line[start] == b'(') {
                    start += 1;
                }
                while start < line.len() && alphanum(line[start]) {
                    start += 1;
                }
                continue;
            }
            let mut end = start;
            while end < line.len() && alphanum(line[end]) {
                end += 1;
            }
            let name = String::from_utf8_lossy(&line[start..end]).into_owned();
            let Some(def) = self.macros.get(&name) else {
                start = end;
                continue;
            };
            let (pattern, subst) = (def.pattern.clone(), def.subst.clone());
            if let Some(msg) = def.deprecated.clone() {
                self.emit(233, span, &[&name, &msg]);
            }
            steps += 1;
            if steps > MAX_SUBST_STEPS {
                // The C compiler loops forever here; refuse instead.
                self.emit(75, span, &[]);
                break;
            }
            if !self.subst_pattern(line, start, &pattern, &subst, span) {
                start = end;
            }
            // On success `start` stays put: the replacement may itself match a macro.
        }
    }

    fn subst_pattern(
        &mut self,
        line: &mut Vec<u8>,
        at_idx: usize,
        pat: &[u8],
        subst: &[u8],
        span: Span,
    ) -> bool {
        let ctrl = self.state.ctrlchar;
        let semi = self.state.semicolon;
        let mut args: [Option<Vec<u8>>; 10] = Default::default();
        let prefixlen = pat.iter().take_while(|c| alphanum(**c)).count();

        let mut s = at_idx + prefixlen;
        let mut p = prefixlen;
        let mut matched = true;

        while matched && s < line.len() && p < pat.len() {
            if pat[p] == b'%' {
                p += 1;
                if p < pat.len() && pat[p].is_ascii_digit() {
                    let arg = (pat[p] - b'0') as usize;
                    p += 1;
                    // The character after `%N` in the pattern delimits the argument.
                    let stop = at(pat, p);
                    let mut e = s;
                    while e < line.len() && line[e] != stop && line[e] != b'\n' {
                        if is_startstring(line, e, ctrl) {
                            e = skipstring(line, e, ctrl);
                        } else if matches!(line.get(e), Some(b'(' | b'{' | b'[')) {
                            e = skippgroup(line, e, ctrl);
                        }
                        if e < line.len() {
                            e += 1;
                        }
                    }
                    args[arg] = Some(line[s..e.min(line.len())].to_vec());
                    let seen = at(line, e);
                    if e < line.len() && seen == stop {
                        s = e + 1;
                    } else if seen == b'\n' && stop == b';' && p + 1 >= pat.len() && !semi {
                        s = e;
                    } else {
                        matched = false;
                        s = e;
                    }
                    p += 1;
                } else {
                    matched = false;
                }
            } else if pat[p] == b';' && p + 1 == pat.len() && !semi {
                // With optional semicolons, a trailing `;` in the pattern may also
                // match the end of the line.
                s = skip_ws(line, s);
                if s < line.len() && line[s] != b';' {
                    matched = false;
                }
                p += 1;
            } else {
                // Whitespace between two *different* non-alphanumeric characters is
                // optional in the source.
                if !alphanum(pat[p]) && (p == 0 || pat[p - 1] != pat[p]) {
                    s = skip_ws(line, s);
                }
                let ch = litchar(pat, &mut p, false, ctrl);
                if s >= line.len() || line[s] as i64 != ch {
                    matched = false;
                } else {
                    s += 1;
                }
            }
        }

        if matched && p >= pat.len() {
            // An identifier-final pattern must not be followed by more identifier text.
            if p > 0 && alphanum(pat[p - 1]) && s < line.len() && alphanum(line[s]) {
                matched = false;
            }
        } else {
            matched = false;
        }
        if !matched {
            return false;
        }

        let mut built: Vec<u8> = Vec::with_capacity(subst.len());
        let mut e = 0usize;
        while e < subst.len() {
            let stringize = subst[e] == b'#'
                && at(subst, e + 1) == b'%'
                && at(subst, e + 2).is_ascii_digit();
            if stringize {
                e += 1;
            }
            if subst[e] == b'%' && at(subst, e + 1).is_ascii_digit() {
                let arg = (at(subst, e + 1) - b'0') as usize;
                match &args[arg] {
                    Some(a) => {
                        if stringize {
                            built.push(b'"');
                        }
                        built.extend_from_slice(a);
                        if stringize {
                            built.push(b'"');
                        }
                        e += 2;
                    }
                    None => {
                        built.push(subst[e]);
                        e += 1;
                    }
                }
            } else if subst[e] == b'"' && is_startstring(subst, e, ctrl) {
                let close = skipstring(subst, e, ctrl);
                let stop = (close + 1).min(subst.len());
                built.extend_from_slice(&subst[e..stop]);
                e = stop;
            } else {
                built.push(subst[e]);
                e += 1;
            }
        }

        // A disabled assertion-style macro is commonly defined with an empty
        // replacement and invoked as `MACRO(...);`. amxxpc consumes that
        // statement terminator with the empty expansion; leaving it behind
        // creates a synthetic bare `;` and a false error 036.
        if built.is_empty() && line[..at_idx].iter().all(|byte| *byte <= b' ') {
            let semicolon = skip_ws(line, s);
            if line.get(semicolon) == Some(&b';')
                && line[semicolon + 1..].iter().all(|byte| *byte <= b' ')
            {
                s = line.len();
            }
        }

        if line.len() - (s - at_idx) + built.len() > LINE_MAX {
            self.emit(75, span, &[]);
            return true;
        }
        line.splice(at_idx..s, built);
        true
    }
}

/// What `command()` decided about a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    /// Ordinary source: substitute macros and keep it.
    None,
    EmptyLine,
    CondFalse,
    Include,
    Define,
    If,
    Directive,
    /// Keep the line verbatim, without substitution (`#emit`).
    Passthrough,
}

// --- shared byte helpers ----------------------------------------------------

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] <= b' ' && b[i] != 7 {
        i += 1;
    }
    i
}

/// Byte ranges of each physical line, terminators excluded.
fn line_starts(b: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            let mut end = i;
            if end > start && b[end - 1] == b'\r' {
                end -= 1;
            }
            out.push((start, end));
            start = i + 1;
        }
    }
    if start < b.len() {
        out.push((start, b.len()));
    }
    out
}

fn file_name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `is_startstring()`: does a string or character literal begin here? Accepts the
/// `!` (packed) and escape-character prefixes in either order.
fn is_startstring(b: &[u8], mut i: usize, ctrl: u8) -> bool {
    let c = at(b, i);
    if c == b'"' || c == b'\'' {
        return true;
    }
    if c == b'!' {
        i += 1;
        if matches!(at(b, i), b'"' | b'\'') {
            return true;
        }
        if at(b, i) == ctrl {
            i += 1;
            return matches!(at(b, i), b'"' | b'\'');
        }
    } else if c == ctrl {
        i += 1;
        if matches!(at(b, i), b'"' | b'\'') {
            return true;
        }
        if at(b, i) == b'!' {
            i += 1;
            return matches!(at(b, i), b'"' | b'\'');
        }
    }
    false
}

/// `skipstring()`: index of the closing quote, or `b.len()` if unterminated.
fn skipstring(b: &[u8], mut i: usize, ctrl: u8) -> usize {
    let mut raw = false;
    while matches!(at(b, i), c if c == b'!' || c == ctrl) && i < b.len() {
        if b[i] == ctrl {
            raw = true;
        }
        i += 1;
    }
    let endquote = at(b, i);
    if endquote != b'"' && endquote != b'\'' {
        return i;
    }
    i += 1;
    while i < b.len() && b[i] != endquote {
        litchar(b, &mut i, raw, ctrl);
    }
    i.min(b.len())
}

/// `skippgroup()`: index of the matching closing bracket, or `b.len()`.
fn skippgroup(b: &[u8], mut i: usize, ctrl: u8) -> usize {
    let open = at(b, i);
    let close = match open {
        b'(' => b')',
        b'{' => b'}',
        b'[' => b']',
        b'<' => b'>',
        _ => return i,
    };
    let mut nest = 0i32;
    i += 1;
    while i < b.len() && (b[i] != close || nest > 0) {
        if b[i] == open {
            nest += 1;
        } else if b[i] == close {
            nest -= 1;
        } else if is_startstring(b, i, ctrl) {
            i = skipstring(b, i, ctrl);
        }
        if i >= b.len() {
            break;
        }
        i += 1;
    }
    i.min(b.len())
}

/// `litchar()`: read one (possibly escaped) character, advancing `i`.
fn litchar(b: &[u8], i: &mut usize, raw: bool, ctrl: u8) -> i64 {
    if *i >= b.len() {
        return 0;
    }
    if raw || b[*i] != ctrl {
        let c = b[*i] as i64;
        *i += 1;
        return c;
    }
    *i += 1;
    let c = at(b, *i);
    if c == ctrl {
        *i += 1;
        return ctrl as i64;
    }
    let simple = match c {
        b'a' => Some(7),
        b'b' => Some(8),
        b'e' => Some(27),
        b'f' => Some(12),
        b'n' => Some(10),
        b'r' => Some(13),
        b't' => Some(9),
        b'v' => Some(11),
        b'\'' | b'"' | b'%' => Some(c as i64),
        _ => None,
    };
    if let Some(v) = simple {
        *i += 1;
        return v;
    }
    if c == b'x' {
        *i += 1;
        let mut v: i64 = 0;
        while at(b, *i).is_ascii_hexdigit() {
            v = (v << 4) + (at(b, *i) as char).to_digit(16).unwrap_or(0) as i64;
            *i += 1;
        }
        if at(b, *i) == b';' {
            *i += 1;
        }
        return v;
    }
    if c.is_ascii_digit() {
        let mut v: i64 = 0;
        while at(b, *i).is_ascii_digit() {
            v = v * 10 + (at(b, *i) - b'0') as i64;
            *i += 1;
        }
        if at(b, *i) == b';' {
            *i += 1;
        }
        return v;
    }
    // Invalid escape: consume the character so scanning cannot stall.
    *i += 1;
    c as i64
}

/// `getstring()`: a `"..."` literal at `i` (whitespace skipped first).
fn get_string(b: &[u8], i: &mut usize) -> Option<String> {
    *i = skip_ws(b, *i);
    if at(b, *i) != b'"' {
        return None;
    }
    *i += 1;
    let start = *i;
    while *i < b.len() && b[*i] != b'"' {
        *i += 1;
    }
    let s = String::from_utf8_lossy(&b[start..*i]).into_owned();
    if *i < b.len() {
        *i += 1;
        Some(s)
    } else {
        None
    }
}

/// A Pawn integer literal: `0x`/`0b` prefixed or decimal, `_` allowed as separator.
fn parse_number(b: &[u8], i: &mut usize) -> Option<i64> {
    if !at(b, *i).is_ascii_digit() {
        return None;
    }
    let (radix, mut j) = if at(b, *i) == b'0' && matches!(at(b, *i + 1) | 0x20, b'x') {
        (16u32, *i + 2)
    } else if at(b, *i) == b'0' && matches!(at(b, *i + 1) | 0x20, b'b') {
        (2u32, *i + 2)
    } else {
        (10u32, *i)
    };
    let mut v: i64 = 0;
    let mut any = false;
    while j < b.len() {
        let c = b[j];
        if c == b'_' {
            j += 1;
            continue;
        }
        match (c as char).to_digit(radix) {
            Some(d) => {
                v = v.wrapping_mul(radix as i64).wrapping_add(d as i64);
                any = true;
                j += 1;
            }
            None => break,
        }
    }
    if !any {
        return None;
    }
    *i = j;
    Some(v)
}

/// A number or a `'c'` character constant, as `#pragma ctrlchar` accepts.
fn parse_char_or_number(b: &[u8], i: &mut usize, ctrl: u8) -> Option<i64> {
    *i = skip_ws(b, *i);
    if at(b, *i) == b'\'' {
        *i += 1;
        let v = litchar(b, i, false, ctrl);
        if at(b, *i) == b'\'' {
            *i += 1;
        }
        return Some(v);
    }
    parse_number(b, i)
}

// --- constant-expression evaluator ------------------------------------------

/// Evaluates `#if` / `#assert` / `#pragma` expressions.
///
/// Deviation from the C compiler: an identifier that is not a macro evaluates to 0
/// silently. amxxpc reports error 17 there because its `#if` runs against the full
/// symbol table (`const` and `enum` values included); this pass runs before any of
/// that exists, so reporting would produce false "undefined symbol" errors.
struct Eval<'a> {
    b: &'a [u8],
    i: usize,
    macros: &'a BTreeMap<String, MacroDef>,
    ctrl: u8,
    err: Option<u16>,
}

impl Eval<'_> {
    fn ws(&mut self) {
        self.i = skip_ws(self.b, self.i);
    }

    fn eat(&mut self, tok: &[u8]) -> bool {
        self.ws();
        if self.b[self.i..].starts_with(tok) {
            // `&` must not swallow the first half of `&&`, etc.
            self.i += tok.len();
            true
        } else {
            false
        }
    }

    fn peek_is(&mut self, tok: &[u8]) -> bool {
        self.ws();
        self.b[self.i..].starts_with(tok)
    }

    fn expr(&mut self) -> i64 {
        let cond = self.or();
        if self.eat(b"?") {
            let a = self.expr();
            if !self.eat(b":") {
                self.err.get_or_insert(29);
            }
            let c = self.expr();
            return if cond != 0 { a } else { c };
        }
        cond
    }

    fn or(&mut self) -> i64 {
        let mut v = self.and();
        while self.eat(b"||") {
            let r = self.and();
            v = i64::from(v != 0 || r != 0);
        }
        v
    }

    fn and(&mut self) -> i64 {
        let mut v = self.bit_or();
        while self.eat(b"&&") {
            let r = self.bit_or();
            v = i64::from(v != 0 && r != 0);
        }
        v
    }

    fn bit_or(&mut self) -> i64 {
        let mut v = self.bit_xor();
        while !self.peek_is(b"||") && self.eat(b"|") {
            v |= self.bit_xor();
        }
        v
    }

    fn bit_xor(&mut self) -> i64 {
        let mut v = self.bit_and();
        while self.eat(b"^") {
            v ^= self.bit_and();
        }
        v
    }

    fn bit_and(&mut self) -> i64 {
        let mut v = self.equality();
        while !self.peek_is(b"&&") && self.eat(b"&") {
            v &= self.equality();
        }
        v
    }

    fn equality(&mut self) -> i64 {
        let mut v = self.relational();
        loop {
            if self.eat(b"==") {
                v = i64::from(v == self.relational());
            } else if self.eat(b"!=") {
                v = i64::from(v != self.relational());
            } else {
                return v;
            }
        }
    }

    fn relational(&mut self) -> i64 {
        let mut v = self.shift();
        loop {
            if self.eat(b"<=") {
                v = i64::from(v <= self.shift());
            } else if self.eat(b">=") {
                v = i64::from(v >= self.shift());
            } else if !self.peek_is(b"<<") && self.eat(b"<") {
                v = i64::from(v < self.shift());
            } else if !self.peek_is(b">>") && self.eat(b">") {
                v = i64::from(v > self.shift());
            } else {
                return v;
            }
        }
    }

    fn shift(&mut self) -> i64 {
        let mut v = self.additive();
        loop {
            if self.eat(b">>>") {
                let r = self.additive();
                v = ((v as u64) >> (r as u64 & 63)) as i64;
            } else if self.eat(b"<<") {
                let r = self.additive();
                v = v.wrapping_shl(r as u32 & 63);
            } else if self.eat(b">>") {
                let r = self.additive();
                v = v.wrapping_shr(r as u32 & 63);
            } else {
                return v;
            }
        }
    }

    fn additive(&mut self) -> i64 {
        let mut v = self.multiplicative();
        loop {
            if self.eat(b"+") {
                v = v.wrapping_add(self.multiplicative());
            } else if self.eat(b"-") {
                v = v.wrapping_sub(self.multiplicative());
            } else {
                return v;
            }
        }
    }

    fn multiplicative(&mut self) -> i64 {
        let mut v = self.unary();
        loop {
            if self.eat(b"*") {
                v = v.wrapping_mul(self.unary());
            } else if self.eat(b"/") {
                let r = self.unary();
                v = if r == 0 {
                    self.err.get_or_insert(29);
                    0
                } else {
                    v.wrapping_div(r)
                };
            } else if self.eat(b"%") {
                let r = self.unary();
                v = if r == 0 {
                    self.err.get_or_insert(29);
                    0
                } else {
                    v.wrapping_rem(r)
                };
            } else {
                return v;
            }
        }
    }

    fn unary(&mut self) -> i64 {
        self.ws();
        if self.peek_is(b"!=") {
            return self.primary();
        }
        if self.eat(b"!") {
            return i64::from(self.unary() == 0);
        }
        if self.eat(b"-") {
            return self.unary().wrapping_neg();
        }
        if self.eat(b"~") {
            return !self.unary();
        }
        if self.eat(b"+") {
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> i64 {
        self.ws();
        if self.i >= self.b.len() {
            self.err.get_or_insert(29);
            return 0;
        }
        if self.b[self.i] == b'(' {
            self.i += 1;
            let v = self.expr();
            if !self.eat(b")") {
                self.err.get_or_insert(29);
            }
            return v;
        }
        if self.b[self.i] == b'\'' {
            self.i += 1;
            let v = litchar(self.b, &mut self.i, false, self.ctrl);
            if at(self.b, self.i) == b'\'' {
                self.i += 1;
            }
            return v;
        }
        if let Some(v) = parse_number(self.b, &mut self.i) {
            return v;
        }
        if alpha(self.b[self.i]) {
            let start = self.i;
            while self.i < self.b.len() && alphanum(self.b[self.i]) {
                self.i += 1;
            }
            let word = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
            if word == "defined" {
                self.ws();
                let paren = at(self.b, self.i) == b'(';
                if paren {
                    self.i += 1;
                    self.ws();
                }
                let s = self.i;
                while self.i < self.b.len() && alphanum(self.b[self.i]) {
                    self.i += 1;
                }
                let name = String::from_utf8_lossy(&self.b[s..self.i]).into_owned();
                if paren && !self.eat(b")") {
                    self.err.get_or_insert(29);
                }
                return i64::from(self.macros.contains_key(&name));
            }
            // Unexpanded identifier: not a macro, so it is zero here.
            return 0;
        }
        self.err.get_or_insert(29);
        self.i += 1;
        0
    }
}

// --- __DATE__ / __TIME__ ----------------------------------------------------

/// `MM/DD/YYYY` and `HH:MM:SS`, matching `inst_datetime_defines()` in sc1.c.
///
/// Deviation: the C compiler uses local time; without a timezone database this uses
/// UTC. Only `__DATE__` / `__TIME__` are affected.
fn local_date_time() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        format!("{m:02}/{d:02}/{y:04}"),
        format!("{:02}:{:02}:{:02}", tod / 3600, (tod / 60) % 60, tod % 60),
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Preprocessed, Diagnostics) {
        let mut pp = Preprocessor::with_provider(Vec::new(), Box::new(MemProvider::new()));
        pp.process(Path::new("main.sma"), src)
    }

    fn run_with(files: &[(&str, &str)], dirs: Vec<PathBuf>, src: &str) -> (Preprocessed, Diagnostics) {
        let mut mem = MemProvider::new();
        for (p, t) in files {
            mem.insert(*p, *t);
        }
        let mut pp = Preprocessor::with_provider(dirs, Box::new(mem));
        pp.process(Path::new("main.sma"), src)
    }

    /// Non-blank output lines, trimmed - the parts a later phase actually sees.
    fn body(p: &Preprocessed) -> Vec<String> {
        p.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
    }

    fn codes(d: &Diagnostics) -> Vec<u16> {
        d.items().iter().map(|i| i.code).collect()
    }

    #[test]
    fn object_like_macro_expands() {
        let (out, d) = run("#define MAX 32\nnew a[MAX];\n");
        assert_eq!(body(&out), ["new a[32];"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn object_like_macro_expands_recursively() {
        let (out, _) = run("#define A B\n#define B 7\nx = A;\n");
        assert_eq!(body(&out), ["x = 7;"]);
    }

    #[test]
    fn self_referential_macro_terminates() {
        // The C compiler loops forever on this; we stop and report error 75.
        let (_, d) = run("#define A A+1\nx = A;\n");
        assert!(codes(&d).contains(&75));
    }

    #[test]
    fn function_like_macro_uses_percent_parameters() {
        let (out, _) = run("#define SQR(%1) ((%1)*(%1))\nx = SQR(3+1);\n");
        assert_eq!(body(&out), ["x = ((3+1)*(3+1));"]);
    }

    #[test]
    fn function_like_macro_takes_several_parameters() {
        let (out, _) = run("#define MAX(%1,%2) ((%1)>(%2)?(%1):(%2))\ny = MAX(a,b);\n");
        assert_eq!(body(&out), ["y = ((a)>(b)?(a):(b));"]);
    }

    #[test]
    fn argument_scanning_skips_nested_groups_and_strings() {
        let (out, _) = run("#define F(%1) [%1]\nz = F(g(1,2));\n");
        assert_eq!(body(&out), ["z = [g(1,2)];"]);
        let (out, _) = run("#define F(%1) [%1]\nz = F(\"a)b\");\n");
        assert_eq!(body(&out), ["z = [\"a)b\"];"]);
    }

    #[test]
    fn stringize_wraps_the_argument_in_quotes() {
        let (out, _) = run("#define NAME(%1) #%1\nn = NAME(abc);\n");
        assert_eq!(body(&out), ["n = \"abc\";"]);
    }

    #[test]
    fn pattern_may_use_brackets_instead_of_parentheses() {
        // Pawn matches raw characters, so this is a legal macro shape.
        let (out, _) = run("#define AT[%1] arr[%1]\nv = AT[3];\n");
        assert_eq!(body(&out), ["v = arr[3];"]);
    }

    #[test]
    fn trailing_percent_parameter_is_dropped_from_the_pattern() {
        // `#define P%1 ok` would otherwise match anything at all after `P`, so the
        // pattern is stored as plain `P`.
        let (out, _) = run("#define P%1 ok\nP;\n");
        assert_eq!(body(&out), ["ok;"]);
        // whitespace ends the pattern, so here `%1` is part of the substitution
        let (out, _) = run("#define LOG %1\nLOG hello\n");
        assert_eq!(body(&out), ["%1 hello"]);
    }

    #[test]
    fn macro_is_not_expanded_inside_a_string() {
        let (out, _) = run("#define MAX 32\nprint(\"MAX\");\n");
        assert_eq!(body(&out), ["print(\"MAX\");"]);
    }

    #[test]
    fn macro_is_not_expanded_inside_a_character_literal() {
        let (out, _) = run("#define A 1\nc = 'A';\n");
        assert_eq!(body(&out), ["c = 'A';"]);
    }

    #[test]
    fn identifier_must_match_whole_word() {
        let (out, _) = run("#define MAX 32\nMAXIMUM = MAX;\n");
        assert_eq!(body(&out), ["MAXIMUM = 32;"]);
    }

    #[test]
    fn undef_removes_the_macro() {
        let (out, d) = run("#define MAX 32\n#undef MAX\nx = MAX;\n");
        assert_eq!(body(&out), ["x = MAX;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn undef_of_unknown_symbol_reports_17() {
        let (_, d) = run("#undef NOPE\n");
        assert_eq!(codes(&d), [17]);
    }

    #[test]
    fn redefinition_with_different_body_warns_201() {
        let (_, d) = run("#define A 1\n#define A 2\n");
        assert_eq!(codes(&d), [201]);
        // an identical redefinition is silent
        let (_, d) = run("#define A 1\n#define A 1\n");
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn define_pattern_must_start_with_a_letter() {
        let (_, d) = run("#define 1BAD x\n");
        assert_eq!(codes(&d), [74]);
    }

    #[test]
    fn comments_are_stripped() {
        // a block comment becomes one space per character, so columns are preserved
        let (out, _) = run("a; // gone\nb; /* also gone */ c;\n");
        assert_eq!(body(&out), ["a;", "b;                 c;"]);
    }

    #[test]
    fn multiline_comment_spans_lines() {
        let (out, _) = run("a;\n/* one\ntwo */ b;\n");
        assert_eq!(body(&out), ["a;", "b;"]);
    }

    #[test]
    fn unterminated_comment_at_eof_reports_1() {
        let (_, d) = run("a;\n/* never closed\n");
        assert!(codes(&d).contains(&1));
    }

    #[test]
    fn simple_if_keeps_the_true_branch() {
        let (out, _) = run("#if 1\nyes;\n#else\nno;\n#endif\n");
        assert_eq!(body(&out), ["yes;"]);
        let (out, _) = run("#if 0\nyes;\n#else\nno;\n#endif\n");
        assert_eq!(body(&out), ["no;"]);
    }

    #[test]
    fn elseif_chain_selects_one_branch() {
        let src = "#define V 2\n#if V == 1\na;\n#elseif V == 2\nb;\n#elseif V == 3\nc;\n#else\nd;\n#endif\n";
        let (out, d) = run(src);
        assert_eq!(body(&out), ["b;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn elseif_chain_falls_through_to_else() {
        let src = "#define V 9\n#if V == 1\na;\n#elseif V == 2\nb;\n#else\nd;\n#endif\n";
        let (out, _) = run(src);
        assert_eq!(body(&out), ["d;"]);
    }

    #[test]
    fn nested_if_inside_a_dead_branch_is_skipped_entirely() {
        let src = "#if 0\n#if 1\ninner;\n#endif\ndead;\n#endif\nlive;\n";
        let (out, d) = run(src);
        assert_eq!(body(&out), ["live;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn nested_if_inside_a_live_branch_is_evaluated() {
        let src = "#if 1\n#if 0\nno;\n#else\nyes;\n#endif\n#endif\n";
        let (out, _) = run(src);
        assert_eq!(body(&out), ["yes;"]);
    }

    #[test]
    fn defined_reports_macro_presence() {
        let src = "#define HAVE 1\n#if defined HAVE\na;\n#endif\n#if defined(MISSING)\nb;\n#endif\n#if !defined MISSING\nc;\n#endif\n";
        let (out, d) = run(src);
        assert_eq!(body(&out), ["a;", "c;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn defined_operand_is_not_macro_expanded() {
        // `HAVE` expands to 0, but `defined HAVE` must still see the name.
        let (out, _) = run("#define HAVE 0\n#if defined HAVE\nyes;\n#endif\n");
        assert_eq!(body(&out), ["yes;"]);
    }

    #[test]
    fn constant_expressions_cover_arithmetic_and_logic() {
        let cases = [
            ("1 + 2 * 3 == 7", true),
            ("(1 + 2) * 3 == 9", true),
            ("8 >> 2 == 2", true),
            ("1 << 4 == 16", true),
            ("0x10 == 16", true),
            ("0b101 == 5", true),
            ("'A' == 65", true),
            ("5 % 3 == 2", true),
            ("~0 == -1", true),
            ("1 && 0", false),
            ("1 || 0", true),
            ("3 & 1", true),
            ("2 & 1", false),
            ("1 ^ 1", false),
            ("2 | 1", true),
            ("1 ? 0 : 1", false),
            ("!0", true),
            ("-2 + 2", false),
        ];
        for (expr, want) in cases {
            let (out, d) = run(&format!("#if {expr}\nT;\n#endif\n"));
            assert!(codes(&d).is_empty(), "{expr} produced {:?}", codes(&d));
            assert_eq!(!body(&out).is_empty(), want, "expression: {expr}");
        }
    }

    #[test]
    fn unterminated_if_at_eof_reports_1() {
        let (_, d) = run("#if 1\nbody;\n");
        assert!(codes(&d).contains(&1));
    }

    #[test]
    fn endif_without_if_reports_26() {
        let (_, d) = run("#endif\n");
        assert_eq!(codes(&d), [26]);
    }

    #[test]
    fn elseif_after_else_reports_61() {
        let (_, d) = run("#if 0\na;\n#else\nb;\n#elseif 1\nc;\n#endif\n");
        assert!(codes(&d).contains(&61));
    }

    #[test]
    fn multiple_else_reports_60() {
        let (_, d) = run("#if 0\na;\n#else\nb;\n#else\nc;\n#endif\n");
        assert!(codes(&d).contains(&60));
    }

    #[test]
    fn include_pulls_in_the_file() {
        let (out, d) =
            run_with(&[("inc/lib.inc", "lib_symbol;\n")], vec![PathBuf::from("inc")], "#include <lib>\nmain;\n");
        assert_eq!(body(&out), ["lib_symbol;", "main;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn include_resolves_the_bare_extension_too() {
        let (out, _) = run_with(&[("inc/lib.inc", "ok;\n")], vec![PathBuf::from("inc")], "#include \"lib.inc\"\n");
        assert_eq!(body(&out), ["ok;"]);
    }

    #[test]
    fn quoted_include_searches_the_current_file_directory() {
        let (out, _) = run_with(&[("sibling.inc", "sib;\n")], Vec::new(), "#include \"sibling\"\n");
        assert_eq!(body(&out), ["sib;"]);
    }

    #[test]
    fn angle_include_ignores_the_current_directory() {
        let (_, d) = run_with(&[("sibling.inc", "sib;\n")], Vec::new(), "#include <sibling>\n");
        assert_eq!(codes(&d), [100]);
    }

    #[test]
    fn missing_include_reports_100_but_tryinclude_is_silent() {
        let (_, d) = run("#include <nope>\n");
        assert_eq!(codes(&d), [100]);
        let (_, d) = run("#tryinclude <nope>\n");
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn include_cycle_is_stopped() {
        let files = [("a.inc", "#include \"b.inc\"\n"), ("b.inc", "#include \"a.inc\"\n")];
        let (_, d) = run_with(&files, Vec::new(), "#include \"a.inc\"\n");
        assert!(codes(&d).contains(&102), "expected the nesting limit, got {:?}", codes(&d));
    }

    #[test]
    fn guarded_reinclusion_is_not_treated_as_a_cycle() {
        let files = [
            ("a.inc", "#if defined _a_included\n#endinput\n#endif\n#define _a_included 1\n#include \"b.inc\"\na_body;\n"),
            ("b.inc", "#include \"a.inc\"\nb_body;\n"),
        ];
        let (out, d) = run_with(&files, Vec::new(), "#include \"a.inc\"\n");
        assert!(codes(&d).is_empty(), "unexpected diagnostics: {:?}", codes(&d));
        assert_eq!(body(&out), ["b_body;", "a_body;"]);
    }

    #[test]
    fn macros_defined_in_an_include_survive_it() {
        let (out, _) =
            run_with(&[("inc/lib.inc", "#define LIBVAL 5\n")], vec![PathBuf::from("inc")], "#include <lib>\nx = LIBVAL;\n");
        assert_eq!(body(&out), ["x = 5;"]);
    }

    #[test]
    fn endinput_stops_only_the_current_file() {
        let files = [("inc/lib.inc", "one;\n#endinput\ntwo;\n")];
        let (out, _) = run_with(&files, vec![PathBuf::from("inc")], "#include <lib>\nafter;\n");
        assert_eq!(body(&out), ["one;", "after;"]);
    }

    #[test]
    fn pragma_ctrlchar_changes_the_escape_character() {
        // The default is '^', so a backslash is an ordinary character and
        // `#pragma ctrlchar '\'` switches the escape over to it. That is the form
        // the bundled AMXX headers actually use.
        let (out, d) = run("#pragma ctrlchar '\\'\n");
        assert_eq!(out.state.ctrlchar, b'\\');
        assert!(codes(&d).is_empty());

        // An empty argument restores the original (sc2.c: sc_ctrlchar_org).
        let (out, _) = run("#pragma ctrlchar '\\'\n#pragma ctrlchar\n");
        assert_eq!(out.state.ctrlchar, DEFAULT_CTRLCHAR);

        // Quirk reproduced faithfully: sc2.c reads the argument with lex(), which
        // uses the CURRENT control character. Writing `#pragma ctrlchar '^'` while
        // '^' is already active therefore reads `^'` as an escaped quote and yields
        // '\'' (39), not '^'. amxxpc does the same.
        let (out, _) = run("#pragma ctrlchar '^'\n");
        assert_eq!(out.state.ctrlchar, b'\'');
    }

    #[test]
    fn pragma_semicolon_is_recorded() {
        let (out, d) = run("#pragma semicolon 1\n");
        assert!(out.state.semicolon);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn recorded_pragmas_reach_the_state() {
        let src = concat!(
            "#pragma tabsize 4\n",
            "#pragma amxlimit 16384\n",
            "#pragma dynamic 8192\n",
            "#pragma compress 0\n",
            "#pragma pack 1\n",
            "#pragma codepage \"1252\"\n",
            "#pragma library amxmodx\n",
            "#pragma reqclass sound\n",
            "#pragma rational Float(3)\n",
            "#pragma unused a, b\n",
        );
        let (out, d) = run(src);
        assert!(codes(&d).is_empty(), "unexpected: {:?}", codes(&d));
        assert_eq!(out.state.tabsize, 4);
        assert_eq!(out.state.amxlimit, 16384);
        assert_eq!(out.state.dynamic, 8192);
        assert!(!out.state.compress);
        assert!(out.state.pack);
        assert_eq!(out.state.codepage.as_deref(), Some("1252"));
        assert_eq!(out.state.libraries, ["amxmodx", "?rc_sound"]);
        assert_eq!(out.state.rational, Some(("Float".to_string(), 3)));
        assert_eq!(out.state.unused, ["a", "b"]);
    }

    #[test]
    fn unknown_pragma_warns_207() {
        let (_, d) = run("#pragma frobnicate\n");
        assert_eq!(codes(&d), [207]);
    }

    #[test]
    fn pragma_deprecated_attaches_to_the_next_macro() {
        let (_, d) = run("#pragma deprecated Use B instead\n#define A 1\nx = A;\n");
        assert_eq!(codes(&d), [233]);
        assert!(d.items()[0].message.contains("Use B instead"));
    }

    #[test]
    fn pragma_deprecated_for_a_declaration_does_not_leak_to_a_later_macro() {
        let (_, d) = run(
            "#pragma deprecated Use replacement instead\n\
             native old_api();\n\
             #define UNRELATED 1\n\
             x = UNRELATED;\n",
        );
        assert!(
            !codes(&d).contains(&233),
            "declaration deprecation leaked to an unrelated macro"
        );
    }

    #[test]
    fn empty_statement_macro_consumes_its_trailing_semicolon() {
        let (out, d) = run(
            "#define ASSERT_DBG(%1,%2)\n\
             public f() {\n\
             ASSERT_DBG(1, \"ok\");\n\
             }\n",
        );

        assert!(codes(&d).is_empty());
        assert_eq!(body(&out), ["public f() {", "}"]);
    }

    #[test]
    fn error_directive_emits_111() {
        let (_, d) = run("#error broken build\n");
        assert_eq!(codes(&d), [111]);
        assert!(d.items()[0].message.contains("broken build"));
        // and it stays silent inside a dead branch
        let (_, d) = run("#if 0\n#error broken\n#endif\n");
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn assert_directive_emits_110_only_when_false() {
        let (_, d) = run("#assert 1 == 1\n");
        assert!(codes(&d).is_empty());
        let (_, d) = run("#assert 1 == 2\n");
        assert_eq!(codes(&d), [110]);
    }

    #[test]
    fn unknown_directive_reports_31() {
        let (_, d) = run("#frobnicate\n");
        assert_eq!(codes(&d), [31]);
    }

    #[test]
    fn emit_is_passed_through_unexpanded() {
        let (out, d) = run("#define A 1\n#emit push.c A\n");
        assert_eq!(body(&out), ["#emit push.c A"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn file_directive_renames_the_reported_file() {
        let (_, d) = run("#file \"other.sma\"\n#error here\n");
        assert_eq!(d.items()[0].file, PathBuf::from("other.sma"));
    }

    #[test]
    fn predefined_symbols_expand() {
        let (out, _) = run("l = __LINE__;\nf = __FILE__;\n");
        assert_eq!(body(&out), ["l = 1;", "f = \"main.sma\";"]);
        let (out, _) = run("d = __DATE__; t = __TIME__;\n");
        // MM/DD/YYYY and HH:MM:SS, quoted, exactly as inst_datetime_defines() writes them
        let line = &body(&out)[0];
        assert_eq!(line.matches('"').count(), 4, "unexpected: {line}");
        assert_eq!(line.matches('/').count(), 2);
        assert_eq!(line.matches(':').count(), 2);
    }

    #[test]
    fn file_macro_follows_includes_and_is_restored() {
        let files = [("inc/lib.inc", "f = __FILE__;\n")];
        let (out, _) = run_with(&files, vec![PathBuf::from("inc")], "#include <lib>\ng = __FILE__;\n");
        assert_eq!(body(&out), ["f = \"lib.inc\";", "g = \"main.sma\";"]);
    }

    #[test]
    fn line_continuation_joins_lines() {
        let (out, d) = run("new a = 1 + \\\n    2;\n");
        assert_eq!(body(&out), ["new a = 1 + 2;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn line_map_points_back_at_the_source() {
        let files = [("inc/lib.inc", "one;\ntwo;\n")];
        let (out, _) = run_with(&files, vec![PathBuf::from("inc")], "#include <lib>\nmain;\n");
        // line 0 is the include's first line, line 1 its second, line 2 is main.sma:2
        assert_eq!(out.map.origin(0), Some((Path::new("inc/lib.inc"), 1)));
        assert_eq!(out.map.origin(1), Some((Path::new("inc/lib.inc"), 2)));
        assert_eq!(out.map.origin(2), Some((Path::new("main.sma"), 1)));
        assert_eq!(out.map.origin(3), Some((Path::new("main.sma"), 2)));
        assert_eq!(out.map.len(), out.text.lines().count());
    }

    #[test]
    fn extra_characters_after_a_directive_report_38() {
        let (_, d) = run("#if 1\n#endif junk\n");
        assert!(codes(&d).contains(&38));
    }

    #[test]
    fn skipped_lines_are_not_expanded_or_diagnosed() {
        // Nothing inside a dead branch may raise a diagnostic or expand.
        let (out, d) = run("#if 0\n#define A 1\n#pragma frobnicate\nx = A;\n#endif\ny = A;\n");
        assert_eq!(body(&out), ["y = A;"]);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        let src = "#define B 2\n#define A 1\n#define C 3\nx = A + B + C;\n";
        let first = run(src).0.text;
        for _ in 0..5 {
            assert_eq!(run(src).0.text, first);
        }
    }
}

