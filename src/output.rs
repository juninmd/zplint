use crate::rules::{LintIssue, Severity};
use clap::ValueEnum;
use std::io::Write;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug, Default)]
pub enum Reporter {
    #[default]
    Default,
    Json,
    Github,
    Sarif,
    Junit,
}

/// Output-side options, mirroring the Biome CLI switches that matter for CI:
/// a diagnostic cap, warnings-as-errors and a machine-readable reporter.
pub struct OutputOptions {
    pub color: bool,
    /// Maximum number of diagnostics to print. `0` means unlimited.
    pub max_diagnostics: usize,
    pub error_on_warnings: bool,
    pub reporter: Reporter,
}

pub fn print_results(
    results: &[(std::path::PathBuf, Vec<LintIssue>)],
    elapsed: Duration,
    opts: &OutputOptions,
) -> i32 {
    let mut total = 0u32;
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut fixable = 0u32;
    let mut files_bad = 0u32;

    for (_, issues) in results {
        if issues.is_empty() { continue; }
        files_bad += 1;
        total += issues.len() as u32;
        errors += issues.iter().filter(|i| i.severity == Severity::Error).count() as u32;
        warnings += issues.iter().filter(|i| i.severity == Severity::Warning).count() as u32;
        fixable += issues.iter().filter(|i| i.fix.is_some()).count() as u32;
    }

    let limit = if opts.max_diagnostics == 0 { usize::MAX } else { opts.max_diagnostics };
    match opts.reporter {
        Reporter::Default => {
            print_text(results, elapsed, opts, total, errors, warnings, fixable, files_bad)
        }
        Reporter::Json => {
            print_json(results, elapsed, total, errors, warnings, fixable, files_bad, limit)
        }
        Reporter::Github => print_github(results, limit),
        Reporter::Sarif => print_sarif(results, limit),
        Reporter::Junit => print_junit(results, limit),
    }

    let fail = errors > 0 || (opts.error_on_warnings && warnings > 0);
    if fail { 1 } else { 0 }
}

#[allow(clippy::too_many_arguments)]
fn print_text(
    results: &[(std::path::PathBuf, Vec<LintIssue>)],
    elapsed: Duration,
    opts: &OutputOptions,
    total: u32,
    errors: u32,
    warnings: u32,
    fixable: u32,
    files_bad: u32,
) {
    let color = if opts.color { ColorChoice::Auto } else { ColorChoice::Never };
    let mut stdout = StandardStream::stdout(color);
    let mut shown = 0usize;
    let limit = if opts.max_diagnostics == 0 { usize::MAX } else { opts.max_diagnostics };
    let mut hidden = 0usize;

    for (path, issues) in results {
        if issues.is_empty() { continue; }
        if shown >= limit {
            hidden += issues.len();
            continue;
        }

        writeln!(stdout).ok();
        let rel = path.to_string_lossy();
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Cyan)).set_bold(true)).ok();
        write!(stdout, "{}", rel).ok();
        stdout.reset().ok();
        writeln!(stdout).ok();

        for iss in issues {
            if shown >= limit {
                hidden += 1;
                continue;
            }
            shown += 1;
            let sev_color = if iss.severity == Severity::Error { Color::Red } else { Color::Yellow };
            stdout.set_color(ColorSpec::new().set_fg(Some(sev_color))).ok();
            write!(stdout, "  x ").ok();
            stdout.reset().ok();
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::White))).ok();
            write!(stdout, "L{}: ", iss.lineno).ok();
            stdout.reset().ok();
            write!(stdout, "{}", iss.message).ok();
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Ansi256(244)))).ok();
            write!(stdout, "  {}", iss.rule_id).ok();
            stdout.reset().ok();
            if iss.fix.is_some() {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green))).ok();
                write!(stdout, " [fix{}]", if iss.fix.is_safe() { "" } else { "!" }).ok();
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
        if hidden > 0 {
            write!(stdout, "  {} diagnostic(s) hidden by --max-diagnostics", hidden).ok();
            writeln!(stdout).ok();
        }
        if fixable > 0 {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green))).ok();
            writeln!(stdout, "  {} auto-fixable -- run `zplint fix`", fixable).ok();
            stdout.reset().ok();
        }
    }
    writeln!(stdout).ok();
}

#[allow(clippy::too_many_arguments)]
fn print_json(
    results: &[(std::path::PathBuf, Vec<LintIssue>)],
    elapsed: Duration,
    total: u32,
    errors: u32,
    warnings: u32,
    fixable: u32,
    files_bad: u32,
    limit: usize,
) {
    let mut out = String::with_capacity(4096);
    out.push_str("{\"version\":1,");
    out.push_str(&format!("\"elapsed_ms\":{},", elapsed.as_millis()));
    out.push_str("\"diagnostics\":[");
    let mut first = true;
    let mut shown = 0usize;
    let mut hidden = 0u32;
    for (path, issues) in results {
        for iss in issues {
            if shown >= limit {
                hidden += 1;
                continue;
            }
            shown += 1;
            if !first { out.push(','); }
            first = false;
            out.push_str(&format!(
                "{{\"file\":\"{}\",\"line\":{},\"rule\":\"{}\",\"severity\":\"{}\",\"message\":\"{}\",\"fixable\":{},\"fix\":\"{}\"}}",
                json_escape(&path.to_string_lossy()),
                iss.lineno,
                json_escape(iss.rule_id),
                if iss.severity == Severity::Error { "error" } else { "warning" },
                json_escape(&iss.message),
                iss.fix.is_some(),
                iss.fix.as_str()
            ));
        }
    }
    out.push_str("],\"summary\":");
    out.push_str(&format!(
        "{{\"files_with_issues\":{},\"issues\":{},\"errors\":{},\"warnings\":{},\"fixable\":{},\"hidden\":{}}}",
        files_bad, total, errors, warnings, fixable, hidden
    ));
    out.push_str("}\n");
    print!("{}", out);
    std::io::stdout().flush().ok();
}

fn print_github(results: &[(std::path::PathBuf, Vec<LintIssue>)], limit: usize) {
    let mut shown = 0usize;
    for (path, issues) in results {
        for iss in issues {
            if shown >= limit { return; }
            shown += 1;
            let level = if iss.severity == Severity::Error { "error" } else { "warning" };
            println!(
                "::{level} file={},line={},title={}::{}",
                gh_escape(&path.to_string_lossy()),
                iss.lineno,
                gh_escape(iss.rule_id),
                gh_escape(&iss.message)
            );
        }
    }
}

fn gh_escape(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(':', "%3A")
        .replace(',', "%2C")
}

fn print_sarif(results: &[(std::path::PathBuf, Vec<LintIssue>)], limit: usize) {
    let mut results_json = Vec::new();
    let mut shown = 0usize;
    for (path, issues) in results {
        for iss in issues {
            if shown >= limit { break; }
            shown += 1;
            results_json.push(serde_json::json!({
                "ruleId": iss.rule_id,
                "level": if iss.severity == Severity::Error { "error" } else { "warning" },
                "message": { "text": iss.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": path.to_string_lossy().replace('\\', "/") },
                        "region": { "startLine": iss.lineno }
                    }
                }]
            }));
        }
    }
    let sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "zplint", "version": env!("CARGO_PKG_VERSION") } },
            "results": results_json
        }]
    });
    println!("{}", serde_json::to_string_pretty(&sarif).unwrap_or_default());
}

fn print_junit(results: &[(std::path::PathBuf, Vec<LintIssue>)], limit: usize) {
    let mut cases = String::new();
    let mut shown = 0usize;
    let mut failures = 0usize;
    for (path, issues) in results {
        for iss in issues {
            if shown >= limit { break; }
            shown += 1;
            if iss.severity == Severity::Error { failures += 1; }
            cases.push_str(&format!(
                "  <testcase classname=\"{}\" name=\"{}\"><failure message=\"{}\">{}</failure></testcase>\n",
                xml_escape(&path.to_string_lossy()),
                xml_escape(iss.rule_id),
                xml_escape(&iss.message),
                xml_escape(&format!("{}:{}", path.display(), iss.lineno))
            ));
        }
    }
    println!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites><testsuite name=\"zplint\" tests=\"{shown}\" failures=\"{failures}\">\n{cases}</testsuite></testsuites>"
    );
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
