mod config;
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
        None => {
            run_lint(&root, &cfg, cli.files);
        }
    }
}

fn run_lint(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>) {
    let sma_files = if files.is_empty() {
        discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        files
    };

    if sma_files.is_empty() {
        eprintln!("No .sma files found");
        std::process::exit(0);
    }

    let start = Instant::now();
    let results: Vec<_> = sma_files.iter()
        .map(|f| {
            let issues = engine::lint_file(f, &cfg.rules);
            (f.clone(), issues)
        })
        .collect();
    let elapsed = start.elapsed();

    let exit_code = output::print_results(&results, elapsed, cfg.output.color);
    std::process::exit(exit_code);
}

fn run_fix(root: &std::path::Path, cfg: &config::Config, files: Vec<PathBuf>) {
    let sma_files = if files.is_empty() {
        discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        files
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
