//! Diagnostics for the zpc Pawn compiler: source positions, severities and the
//! compiler's own error/warning table.
//!
//! Codes and message texts mirror amxxpc exactly (see [`table`]) so that zpc output
//! can be compared against it directly. Where zpc improves on amxxpc it does so by
//! adding span/caret information around the same message, never by rewording it.

pub mod table;

use std::fmt;
use std::path::{Path, PathBuf};

/// A byte range in a source file. `start <= end`, both are byte offsets.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "span start must not exceed end");
        Self { start, end }
    }

    /// A zero-width span at `pos`, used when a diagnostic points between tokens
    /// (for example "expected `;` here").
    pub fn at(pos: u32) -> Self {
        Self { start: pos, end: pos }
    }

    /// The smallest span covering both inputs.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn len(self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// How severe a diagnostic is. Pawn splits errors into recoverable and fatal:
/// a fatal error aborts the compile immediately, a plain error lets the compiler
/// continue so it can report more problems in one run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Warning,
    Error,
    Fatal,
}

impl Severity {
    /// The severity implied by a Pawn diagnostic code.
    pub fn of_code(code: u16) -> Severity {
        match code {
            0..=99 => Severity::Error,
            100..=199 => Severity::Fatal,
            _ => Severity::Warning,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal error",
        }
    }
}

/// One compiler diagnostic, tied to a source position.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// Pawn diagnostic number, e.g. 17 ("undefined symbol") or 213 ("tag mismatch").
    pub code: u16,
    pub severity: Severity,
    /// Message with any `%s`/`%d` placeholders already substituted.
    pub message: String,
    pub span: Span,
    /// File the span belongs to; diagnostics can come from an included file.
    pub file: PathBuf,
}

impl Diagnostic {
    /// Build a diagnostic from a Pawn code, filling in severity and the message
    /// template. `args` replace the template's `%s`/`%d` placeholders in order.
    pub fn new(code: u16, span: Span, file: impl Into<PathBuf>, args: &[&str]) -> Self {
        let template = table::message(code).unwrap_or("unknown diagnostic");
        Self {
            code,
            severity: Severity::of_code(code),
            message: fill(template, args),
            span,
            file: file.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error | Severity::Fatal)
    }
}

/// Substitute `%s`/`%d`/`%ld` placeholders left to right. Any placeholder without a
/// matching argument is left as-is rather than dropped, so a wrong call site is
/// visible in the output instead of silently producing a truncated message.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    let mut next = 0usize;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let width = if after.starts_with("%ld") {
            3
        } else if after.starts_with("%s") || after.starts_with("%d") {
            2
        } else if after.starts_with("%%") {
            // Escaped percent: emit one, consume both.
            out.push('%');
            rest = &after[2..];
            continue;
        } else {
            // Unknown directive: copy the `%` through unchanged.
            out.push('%');
            rest = &after[1..];
            continue;
        };
        match args.get(next) {
            Some(a) => {
                out.push_str(a);
                next += 1;
            }
            None => out.push_str(&after[..width]),
        }
        rest = &after[width..];
    }
    out.push_str(rest);
    out
}

/// Converts byte offsets into 1-based line/column pairs for display.
/// Built once per file; lookup is a binary search over line starts.
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        Self { line_starts }
    }

    /// 1-based (line, column) of a byte offset. Column counts bytes, matching how
    /// the original compiler reports positions.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (idx as u32 + 1, offset - self.line_starts[idx] + 1)
    }
}

/// Renders a diagnostic the way amxxpc does, so output can be diffed against it:
/// `path(line) : error 017: undefined symbol "foo"`.
pub struct AmxxpcStyle<'a> {
    pub diag: &'a Diagnostic,
    pub index: &'a LineIndex,
}

impl fmt::Display for AmxxpcStyle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (line, _) = self.index.line_col(self.diag.span.start);
        write!(
            f,
            "{}({}) : {} {:03}: {}",
            self.diag.file.display(),
            line,
            self.diag.severity.as_str(),
            self.diag.code,
            self.diag.message
        )
    }
}

/// Collects diagnostics during a compile and tracks whether compilation may continue.
#[derive(Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    fatal: bool,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        if d.severity == Severity::Fatal {
            self.fatal = true;
        }
        self.items.push(d);
    }

    /// Convenience for the common `emit(code, span, file, args)` shape.
    pub fn emit(&mut self, code: u16, span: Span, file: &Path, args: &[&str]) {
        self.push(Diagnostic::new(code, span, file, args));
    }

    /// True once a fatal error has been reported; the driver must stop.
    pub fn aborted(&self) -> bool {
        self.fatal
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.items.iter().filter(|d| d.severity == Severity::Warning).count()
    }

    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    pub fn into_items(self) -> Vec<Diagnostic> {
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_the_documented_ranges() {
        // amxxpc defines warnings 200-234 only; anything above is a SA-MP fork code.
        assert_eq!(table::message(213), Some("tag mismatch"));
        assert!(table::message(234).is_some());
        assert!(table::message(235).is_none());
        assert_eq!(table::message(17), Some("undefined symbol \"%s\""));
    }

    #[test]
    fn severity_follows_code_ranges() {
        assert_eq!(Severity::of_code(17), Severity::Error);
        assert_eq!(Severity::of_code(100), Severity::Fatal);
        assert_eq!(Severity::of_code(213), Severity::Warning);
    }

    #[test]
    fn placeholders_are_substituted_in_order() {
        assert_eq!(fill("undefined symbol \"%s\"", &["foo"]), "undefined symbol \"foo\"");
        assert_eq!(fill("%s and %s", &["a", "b"]), "a and b");
        // a missing argument leaves the placeholder visible rather than vanishing
        assert_eq!(fill("%s and %s", &["a"]), "a and %s");
        assert_eq!(fill("100%% done", &[]), "100% done");
    }

    #[test]
    fn diagnostic_formats_like_amxxpc() {
        let src = "line one\nline two\nline three\n";
        let index = LineIndex::new(src);
        // offset 9 is the start of line 2
        let d = Diagnostic::new(17, Span::new(9, 12), "plugin.sma", &["foo"]);
        let shown = AmxxpcStyle { diag: &d, index: &index }.to_string();
        assert_eq!(shown, "plugin.sma(2) : error 017: undefined symbol \"foo\"");
    }

    #[test]
    fn line_index_maps_offsets() {
        let index = LineIndex::new("ab\ncd\n");
        assert_eq!(index.line_col(0), (1, 1));
        assert_eq!(index.line_col(1), (1, 2));
        assert_eq!(index.line_col(3), (2, 1));
    }

    #[test]
    fn fatal_error_marks_the_run_aborted() {
        let mut d = Diagnostics::new();
        d.emit(213, Span::at(0), Path::new("a.sma"), &[]);
        assert!(!d.aborted());
        assert_eq!(d.warning_count(), 1);
        d.emit(100, Span::at(0), Path::new("a.sma"), &["x"]);
        assert!(d.aborted());
        assert_eq!(d.error_count(), 1);
    }
}
