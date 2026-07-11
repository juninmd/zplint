use crate::rules::{LintIssue, Severity};
use std::io::Write;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use std::time::Duration;

pub fn print_results(results: &[(std::path::PathBuf, Vec<LintIssue>)], elapsed: Duration, use_color: bool) -> i32 {
    let color = if use_color { ColorChoice::Auto } else { ColorChoice::Never };
    let mut stdout = StandardStream::stdout(color);
    let mut total = 0u32;
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut fixable = 0u32;
    let mut files_bad = 0u32;

    for (path, issues) in results {
        if issues.is_empty() { continue; }
        files_bad += 1;
        total += issues.len() as u32;
        errors += issues.iter().filter(|i| i.severity == Severity::Error).count() as u32;
        warnings += issues.iter().filter(|i| i.severity == Severity::Warning).count() as u32;
        fixable += issues.iter().filter(|i| i.auto_fixable).count() as u32;

        writeln!(stdout).ok();
        let rel = path.to_string_lossy();
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_bold(true)).ok();
        write!(stdout, "{}", rel).ok();
        stdout.reset().ok();
        writeln!(stdout).ok();

        for iss in issues {
            let sev_color = if iss.severity == Severity::Error { Color::Red } else { Color::Yellow };
            stdout.set_color(ColorSpec::new().set_fg(Some(sev_color))).ok();
            write!(stdout, "  x ").ok();
            stdout.reset().ok();
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::White))).ok();
            write!(stdout, "L{}: ", iss.lineno).ok();
            stdout.reset().ok();
            write!(stdout, "{}", iss.message).ok();
            if iss.auto_fixable {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green))).ok();
                write!(stdout, " [fix]").ok();
                stdout.reset().ok();
            }
            writeln!(stdout).ok();
        }
    }

    if total == 0 {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)).set_bold(true)).ok();
        writeln!(stdout, "\nv No issues").ok();
    } else {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)).set_bold(true)).ok();
        write!(stdout, "\nx {} issue(s)", total).ok();
        stdout.reset().ok();
        write!(stdout, " ({} error(s), {} warning(s))", errors, warnings).ok();
        writeln!(stdout).ok();
        write!(stdout, "  {} file(s) -- {:.2?}", files_bad, elapsed).ok();
        writeln!(stdout).ok();
        if fixable > 0 {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green))).ok();
            writeln!(stdout, "  {} auto-fixable -- run `zplint fix`", fixable).ok();
            stdout.reset().ok();
        }
    }
    writeln!(stdout).ok();
    if errors > 0 { 1 } else { 0 }
}

pub fn print_realtime(path: &std::path::Path, issues: &[LintIssue], use_color: bool) {
    let color = if use_color { ColorChoice::Auto } else { ColorChoice::Never };
    let mut stdout = StandardStream::stdout(color);
    let rel = path.to_string_lossy();

    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_bold(true)).ok();
    writeln!(stdout, "\n[watch] {}", rel).ok();
    stdout.reset().ok();

    if issues.is_empty() {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green))).ok();
        writeln!(stdout, "  v No issues").ok();
    } else {
        for iss in issues {
            let sc = if iss.severity == Severity::Error { Color::Red } else { Color::Yellow };
            stdout.set_color(ColorSpec::new().set_fg(Some(sc))).ok();
            writeln!(stdout, "  x L{}: {}", iss.lineno, iss.message).ok();
        }
    }
    stdout.reset().ok();
}
