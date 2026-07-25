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
#[command(name = "zplint", about = "Lightning-fast linter for ZP5.0 AMXX plugins")]
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
        Some(Command::Lint { files }) => {
            run_lint(&root, &cfg, files);
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
            run_lint(&root, &cfg, cli.files);
        }
    }
}

fn run_lint(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>) {
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

    let exit_code = output::print_results(&results, elapsed, cfg.output.color);
    std::process::exit(exit_code);
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

