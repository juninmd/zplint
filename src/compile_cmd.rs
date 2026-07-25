//! `zplint compile` - drive the zpc phases end to end: `.sma` -> `.amxx`.
//!
//! This is the seam that turns six independently-tested crates into a compiler.
//! Each phase is stopped at if it produced errors, matching amxxpc: a plugin with
//! a hard error must not yield an output file, because a stale or half-built
//! `.amxx` on disk is worse than none.

use std::path::{Path, PathBuf};
use zpc_diag::{AmxxpcStyle, Diagnostics, LineIndex, Severity};

/// Where compilation stopped, for reporting.
enum Outcome {
    /// Compiled and written.
    Wrote(PathBuf),
    /// Diagnostics prevented an output file.
    Failed,
}

/// Compile `path`, writing `.amxx` next to it (or to `out` when given).
/// Returns the process exit code: 0 on success, 1 on compile errors, 2 on I/O.
pub fn run(path: &Path, out: Option<PathBuf>, include_dirs: Vec<PathBuf>, emit_asm: bool) -> i32 {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return 2;
        }
    };

    let mut all = Diagnostics::new();

    // --- phase A: preprocess, then scan ---
    let mut pp = zpc_lex::Preprocessor::new(include_dirs);
    let (pre, pp_diags) = pp.process(path, &src);
    merge(&mut all, pp_diags.into_items());
    if report_and_stop(&all, &pre.text, path, Some(&pre.map)) {
        return 1;
    }

    // `#pragma ctrlchar` is positional, so the scanner switches at the lines where
    // the changes occurred rather than being handed one value for the whole unit.
    let scanner = zpc_lex::Scanner::new(&pre.text, path);
    let mut scan_diags = Diagnostics::new();
    let tokens =
        scanner.scan_with_ctrl_changes(&pre.state.ctrlchar_changes, &mut scan_diags);
    merge(&mut all, scan_diags.into_items());
    if report_and_stop(&all, &pre.text, path, Some(&pre.map)) {
        return 1;
    }

    // --- phase B: parse ---
    let (program, parse_diags) = zpc_parse::parse(&pre.text, &tokens, path);
    merge(&mut all, parse_diags.into_items());
    if report_and_stop(&all, &pre.text, path, Some(&pre.map)) {
        return 1;
    }

    // --- phase D: generate code ---
    let unit = zpc_codegen::Generator::new(path).program(&program);
    merge(&mut all, take_diags(&unit));
    if report_and_stop(&all, &pre.text, path, Some(&pre.map)) {
        return 1;
    }

    if emit_asm {
        print!("{}", zpc_codegen::render_asm(&unit.code));
    }

    // --- phase E: assemble ---
    // The label map comes back too: a public's entry address is its label's
    // address, and the AMX publics table stores that. Without it the table would
    // be all zeros, which loads fine and then calls nothing.
    let (code, labels) = match zpc_codegen::assemble_with_labels(&unit.code) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: assembly failed: {e:?}", path.display());
            return 1;
        }
    };

    // --- phase E/F: build the AMX image, then the container ---
    let amx = match build_image(&unit, code, &labels) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{}: {msg}", path.display());
            return 1;
        }
    };

    let amxx = match zpc_amxx::write_amx32(&amx) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: cannot build .amxx container: {e:?}", path.display());
            return 1;
        }
    };

    let dest = out.unwrap_or_else(|| path.with_extension("amxx"));
    match std::fs::write(&dest, &amxx) {
        Ok(()) => {
            report(&all, &pre.text, path, Some(&pre.map));
            print_summary(&all, Outcome::Wrote(dest), amxx.len());
            0
        }
        Err(e) => {
            eprintln!("cannot write {}: {e}", dest.display());
            2
        }
    }
}

/// Assemble the unit's data segment and metadata tables into an AMX image.
fn build_image(
    unit: &zpc_codegen::Unit,
    code: Vec<u8>,
    labels: &std::collections::HashMap<zpc_codegen::LabelId, i32>,
) -> Result<Vec<u8>, String> {
    let mut image = zpc_asm::Image::new(code);

    // The data segment is cells; the image wants bytes, little-endian.
    image.data = unit.data.iter().flat_map(|c| c.to_le_bytes()).collect();

    // Natives keep the generator's index order: the `sysreq.c` operand indexes
    // this table, so re-sorting here would redirect every native call.
    image.natives = unit
        .natives
        .iter()
        .map(|n| zpc_asm::Symbol::new(n.clone(), 0))
        .collect();

    // Publics must be sorted by name - the AMX runtime binary-searches them.
    let mut publics: Vec<_> = unit
        .publics
        .iter()
        .map(|(name, label)| {
            let addr = labels
                .get(label)
                .copied()
                .ok_or_else(|| format!("public '{name}' has no resolved address"))?;
            Ok(zpc_asm::Symbol::new(name.clone(), addr))
        })
        .collect::<Result<_, String>>()?;
    publics.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    image.publics = publics;

    image.build().map_err(|e| format!("cannot build AMX image: {e:?}"))
}

/// Codegen keeps its diagnostics on the unit; pull them out without cloning.
fn take_diags(unit: &zpc_codegen::Unit) -> Vec<zpc_diag::Diagnostic> {
    unit.diags.items().to_vec()
}

fn merge(all: &mut Diagnostics, items: impl IntoIterator<Item = zpc_diag::Diagnostic>) {
    for d in items {
        all.push(d);
    }
}

/// Print diagnostics in amxxpc's format so output can be diffed against it.
///
/// Spans point into the PREPROCESSED text, so a raw `LineIndex` over it reports
/// positions like `plugin.sma(12930)` for a 700-line file - useless to a human and
/// to a differential diff alike. `LineMap` maps each expanded line back to the file
/// and line it came from, which is the whole reason the preprocessor builds it.
fn report(all: &Diagnostics, text: &str, path: &Path, map: Option<&zpc_lex::preproc::LineMap>) {
    let index = LineIndex::new(text);
    for d in all.items() {
        let (expanded_line, _) = index.line_col(d.span.start);
        match map.and_then(|m| m.origin(expanded_line as usize - 1)) {
            Some((origin, line)) => {
                eprintln!(
                    "{}({}) : {} {:03}: {}",
                    origin.display(),
                    line,
                    d.severity.as_str(),
                    d.code,
                    d.message
                );
            }
            // No map (or a synthetic span): fall back to the raw position rather
            // than dropping the diagnostic.
            None => {
                let _ = path;
                eprintln!("{}", AmxxpcStyle { diag: d, index: &index });
            }
        }
    }
}

/// Report, then say whether compilation must stop. Errors stop the pipeline;
/// warnings do not.
fn report_and_stop(all: &Diagnostics, text: &str, path: &Path, map: Option<&zpc_lex::preproc::LineMap>) -> bool {
    if all.error_count() == 0 && !all.aborted() {
        return false;
    }
    report(all, text, path, map);
    print_summary(all, Outcome::Failed, 0);
    true
}

fn print_summary(all: &Diagnostics, outcome: Outcome, bytes: usize) {
    let errors = all.error_count();
    let warnings = all.warning_count();
    match outcome {
        Outcome::Wrote(dest) => {
            eprintln!(
                "\n{} - {} error(s), {} warning(s), {} bytes",
                dest.display(),
                errors,
                warnings,
                bytes
            );
        }
        Outcome::Failed => {
            eprintln!("\ncompilation failed - {errors} error(s), {warnings} warning(s)");
        }
    }
    let _ = Severity::Error;
}



