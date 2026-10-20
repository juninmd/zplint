mod api;
mod api_check;
mod ast_lint;
mod config;
mod detectors;
mod compile_cmd;
mod disasm_cmd;
mod discover;
mod engine;
mod fix;
mod output;
mod rules;
mod watch;

use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "zplint",
    version,
    about = "Lightning-fast linter for ZP5.0 AMXX plugins"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Specific .sma files to lint
    files: Vec<PathBuf>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Lint .sma files (default)
    Lint {
        files: Vec<PathBuf>,
        /// Run only these rule ids (repeatable, comma-separated, or `all`)
        #[arg(long, value_name = "RULE", value_delimiter = ',')]
        only: Vec<String>,
        /// Skip these rule ids (repeatable, comma-separated, or `all`)
        #[arg(long, value_name = "RULE", value_delimiter = ',')]
        skip: Vec<String>,
        /// Exit with status 1 when only warnings are reported
        #[arg(long)]
        error_on_warnings: bool,
        /// Print at most N diagnostics (0 = unlimited)
        #[arg(long, default_value_t = 0, value_name = "N")]
        max_diagnostics: usize,
        /// Machine-readable JSON report
        #[arg(long)]
        json: bool,
    },
    /// Print the documentation entry for a rule id
    Explain {
        /// Rule id, e.g. `handle_leak`
        rule: String,
    },
    /// Watch mode: re-lint on file changes
    Watch,
    /// Apply auto-fixes
    Fix {
        files: Vec<PathBuf>,
    },
    /// Compile a .sma plugin to .amxx
    Compile {
        file: PathBuf,
        /// Output path (defaults to the input with a .amxx extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Directory to search for #include files (repeatable)
        #[arg(short = 'i', long = "include")]
        includes: Vec<PathBuf>,
        /// Print the generated assembly before writing the output
        #[arg(long)]
        emit_asm: bool,
    },
    /// Lint the given files twice - once through the regex engine, once through
    /// the zpc parser - and report the parse rate plus every migrated-rule
    /// finding the two paths disagree on
    AstCompare {
        files: Vec<PathBuf>,
    },
    /// Disassemble a compiled .amxx (or raw .amx) plugin
    Disasm {
        file: PathBuf,
        /// Drop addresses and label jump targets, so output is comparable
        /// between builds that differ only in code layout
        #[arg(long)]
        normalised: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = config::Config::load(&root).unwrap_or_default();

    match cli.command {
        Some(Command::Watch) => {
            watch::start_watch(&root, &cfg);
        }
        Some(Command::Fix { files }) => {
            run_fix(&root, &cfg, files);
        }
        Some(Command::Lint { files, only, skip, error_on_warnings, max_diagnostics, json }) => {
            run_lint(&root, &cfg, files, LintOptions { only, skip, error_on_warnings, max_diagnostics, json });
        }
        Some(Command::Explain { rule }) => {
            std::process::exit(explain(&rule));
        }
        Some(Command::Compile { file, output, includes, emit_asm }) => {
            std::process::exit(compile_cmd::run(&file, output, includes, emit_asm));
        }
        Some(Command::AstCompare { files }) => {
            std::process::exit(run_ast_compare(&root, &cfg, files));
        }
        Some(Command::Disasm { file, normalised }) => {
            std::process::exit(disasm_cmd::run(&file, normalised));
        }
        None => {
            run_lint(&root, &cfg, cli.files, LintOptions::default());
        }
    }
}

#[derive(Default)]
struct LintOptions {
    only: Vec<String>,
    skip: Vec<String>,
    error_on_warnings: bool,
    max_diagnostics: usize,
    json: bool,
}

fn run_lint(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>, opts: LintOptions) {
    let mut cfg = cfg.clone();
    cfg.rules.only = opts.only;
    cfg.rules.skip = opts.skip;
    let cfg = &cfg;

    let sma_files = if files.is_empty() {
        discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        discover::resolve_input_files(root, &files, &cfg.exclude)
    };

    if sma_files.is_empty() {
        eprintln!("No .sma files found");
        std::process::exit(0);
    }

    let start = Instant::now();
    use rayon::prelude::*;
    let results: Vec<_> = sma_files.par_iter()
        .map(|f| {
            let issues = engine::lint_file(f, &cfg.rules);
            (f.clone(), issues)
        })
        .collect();
    let elapsed = start.elapsed();

    let output_opts = output::OutputOptions {
        color: cfg.output.color,
        max_diagnostics: opts.max_diagnostics,
        error_on_warnings: opts.error_on_warnings,
        json: opts.json,
    };
    let exit_code = output::print_results(&results, elapsed, &output_opts);
    std::process::exit(exit_code);
}

/// `zplint explain <rule>`: the documentation is already curated in
/// `docs/KNOWLEDGE.md` (problem / consequence / sources) and the README rule
/// tables. Reading it at compile time keeps the two in lock-step without a
/// second hand-maintained copy.
fn explain(rule: &str) -> i32 {
    let knowledge = include_str!("../docs/KNOWLEDGE.md");
    let lines: Vec<&str> = knowledge.lines().collect();
    let needle = format!("rule: {rule}");
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains(&needle) || l.contains(&format!("rules: {rule}")) || l.contains(&format!(", {rule}")))
        .map(|(i, _)| i)
        .collect();

    for &hit in &hits {
        let start = (0..=hit)
            .rev()
            .find(|&i| lines[i].trim_start().starts_with("###"))
            .unwrap_or(hit);
        let end = (hit + 1..lines.len())
            .find(|&i| {
                let t = lines[i].trim_start();
                t.starts_with("###") || t == "---" || t.starts_with("## ")
            })
            .unwrap_or(lines.len());
        let section = lines[start..end].join("\n").trim().to_string();
        if !section.is_empty() {
            println!("{section}\n");
            let sev = if engine::rule_is_warning(rule) { "warning" } else { "error" };
            println!("Rule: {rule}\nSeverity: {sev}");
            return 0;
        }
    }

    // Fallback: the README tables cover the original 37 detectors.
    let readme = include_str!("../README.md");
    if let Some(line) = readme.lines().find(|l| l.contains(&format!("`{rule}`"))) {
        println!("{}\n", line.trim());
        let sev = if engine::rule_is_warning(rule) { "warning" } else { "error" };
        println!("Rule: {rule}\nSeverity: {sev}");
        return 0;
    }

    eprintln!("Unknown rule '{rule}'. Try `zplint explain <rule-id>` with an id printed by a lint run.");
    1
}

/// Corpus-level proof for the AST migration: how many files the parser can
/// actually handle, and whether it says anything different from the regex rules
/// on the ones it can.
fn run_ast_compare(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>) -> i32 {
    let sma_files = if files.is_empty() {
        discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        discover::resolve_input_files(root, &files, &cfg.exclude)
    };

    let mut parsed = 0usize;
    let mut divergences = 0usize;
    for f in &sma_files {
        let src = std::fs::read_to_string(f).unwrap_or_default();
        if ast_lint::parses_cleanly(f, &src) {
            parsed += 1;
        }
        for d in ast_lint::compare(f, &cfg.rules) {
            divergences += 1;
            let side = if d.ast_only { "ast-only" } else { "regex-only" };
            println!("{}:{} {} [{}]", f.display(), d.lineno, d.rule_id, side);
        }
    }

    println!(
        "\n{}/{} file(s) parse cleanly ({:.1}%), {} divergence(s)",
        parsed,
        sma_files.len(),
        if sma_files.is_empty() { 0.0 } else { parsed as f64 * 100.0 / sma_files.len() as f64 },
        divergences
    );
    0
}

fn run_fix(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>) {
    let sma_files = if files.is_empty() {
        discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        discover::resolve_input_files(root, &files, &cfg.exclude)
    };

    if sma_files.is_empty() {
        eprintln!("No .sma files found");
        return;
    }

    let mut total = 0i32;
    for f in &sma_files {
        let fixes = fix::auto_fix(f, cfg, cfg.output.color);
        if fixes > 0 {
            total += fixes;
            let rel = f.strip_prefix(root).unwrap_or(f);
            eprintln!("  v {} fix(es) on {}", fixes, rel.display());
        }
    }
    if total > 0 {
        eprintln!("\n{} total fix(es) applied", total);
    } else {
        eprintln!("No fixes needed");
    }
}

