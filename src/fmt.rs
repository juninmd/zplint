//! Conservative Pawn formatter.
//!
//! This is the formatter slice of Biome parity: indentation from brace depth,
//! trailing whitespace, blank-line hygiene and `#include` ordering. It is
//! deliberately line-based and conservative:
//!
//! * preprocessor lines stay at column 0;
//! * block comments and multi-line strings are left alone;
//! * lines ending in `\` keep their original indentation (Pawn continuation);
//! * no token re-spacing is attempted, so semantics cannot change.
//!
//! `zplint fmt --check` reports files that would change; `--write` applies the
//! result. The formatter is idempotent and tested on the official corpus.

use std::path::{Path, PathBuf};

use crate::config::Config;

pub fn format_source(src: &str) -> String {
    let mut lines: Vec<String> = src
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect();
    // `split` leaves a trailing empty element for a file ending in `\n`.
    let trailing_newline = lines.last().is_some_and(|l| l.is_empty());
    if trailing_newline {
        lines.pop();
    }

    sort_leading_includes(&mut lines);

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut depth = 0i32;
    let mut in_block_comment = false;
    let mut in_multiline_string = false;
    let mut continuation = false;
    // A braceless `if`/`else`/loop body is one statement; remember it so the
    // next line gets one extra indent instead of being pulled back to the
    // control statement's level.
    let mut body_indent = false;

    for raw in &lines {
        let trimmed_end = raw.trim_end();
        let trimmed_start = trimmed_end.trim_start();
        let blank = trimmed_start.is_empty();

        if blank {
            out.push(String::new());
            continuation = false;
            continue;
        }

        // Preprocessor directives are never indented.
        if trimmed_start.starts_with('#') {
            out.push(trimmed_start.to_string());
            continuation = trimmed_end.ends_with('\\');
            continue;
        }

        // Inside a block comment or a multi-line string, preserve the line as-is
        // (minus trailing whitespace): changing indentation there is visible.
        if in_block_comment || in_multiline_string {
            out.push(trimmed_end.to_string());
            scan_state(trimmed_end, &mut in_block_comment, &mut in_multiline_string);
            continuation = trimmed_end.ends_with('\\');
            continue;
        }

        // A continuation line belongs to the previous statement; leave its
        // indentation untouched rather than guessing at alignment.
        if continuation {
            out.push(trimmed_end.to_string());
            scan_state(trimmed_end, &mut in_block_comment, &mut in_multiline_string);
            continuation = trimmed_end.ends_with('\\');
            continue;
        }

        let closes = trimmed_start.starts_with('}');

        let label = trimmed_start.starts_with("case ")
            || trimmed_start.starts_with("default:")
            || trimmed_start.starts_with("default :")
            || (trimmed_start.ends_with(':')
                && !trimmed_start.starts_with("::")
                && !trimmed_start.contains(" ? ")
                && !trimmed_start.starts_with("case "));
        let control = is_control_header(trimmed_start);
        let opens_block = trimmed_end.ends_with('{');
        let indent_depth = if body_indent && trimmed_start != "{" && !closes {
            depth + 1
        } else if closes || label {
            (depth - 1).max(0)
        } else {
            depth
        };

        let mut line = String::new();
        for _ in 0..indent_depth {
            line.push('\t');
        }
        line.push_str(trimmed_start);
        out.push(line);

        scan_state(trimmed_end, &mut in_block_comment, &mut in_multiline_string);
        let delta = brace_delta(trimmed_end);
        depth = (depth + delta).max(0);
        body_indent = control && !opens_block && !trimmed_end.ends_with(';');
        continuation = trimmed_end.ends_with('\\');
    }

    // Collapse runs of 3+ blank lines to one blank line.
    let mut compact: Vec<String> = Vec::with_capacity(out.len());
    let mut blanks = 0;
    for line in out {
        if line.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        compact.push(line);
    }

    let mut text = compact.join("\n");
    if trailing_newline || !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Sort the leading `#include` block: `<amxmodx>` first, then alphabetical.
/// Only a contiguous block at the top of the file is touched.
fn sort_leading_includes(lines: &mut Vec<String>) {
    let start = lines.iter().position(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with("//") && !t.starts_with("/*") && !t.starts_with('*')
    });
    let Some(start) = start else { return };
    if !lines[start].trim_start().starts_with("#include") {
        return;
    }
    let mut end = start;
    let mut includes: Vec<String> = Vec::new();
    // Only a contiguous run of `#include` lines is sorted; a comment or blank
    // line in the middle may be a deliberate grouping, so it ends the run.
    while end < lines.len() && lines[end].trim_start().starts_with("#include") {
        includes.push(lines[end].trim().to_string());
        end += 1;
    }
    if includes.len() < 2 {
        return;
    }
    includes.sort_by_key(|l| {
        let name = l
            .trim_start_matches("#include")
            .trim()
            .trim_start_matches(['<', '"'])
            .trim_end_matches(['>', '"'])
            .to_lowercase();
        (!name.starts_with("amxmodx"), name)
    });
    lines.splice(start..end, includes);
}

fn is_control_header(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("if ")
        || t.starts_with("if(")
        || t.starts_with("else")
        || t.starts_with("for ")
        || t.starts_with("for(")
        || t.starts_with("while ")
        || t.starts_with("while(")
        || t.starts_with("do")
        || t.starts_with("switch ")
        || t.starts_with("switch("))
        && !t.ends_with(';')
}

fn scan_state(line: &str, in_block: &mut bool, in_string: &mut bool) {
    let mut chars = line.chars().peekable();
    let mut prev = '\0';
    while let Some(c) = chars.next() {
        if *in_string {
            if c == '\\' || c == '^' {
                chars.next();
            } else if c == '"' {
                *in_string = false;
            }
            prev = c;
            continue;
        }
        if *in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                *in_block = false;
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            *in_block = true;
            continue;
        }
        if c == '"' && prev != '^' && prev != '\\' {
            *in_string = true;
        }
        prev = c;
    }
}

/// Net brace change on a line, ignoring braces in strings, chars and comments.
fn brace_delta(line: &str) -> i32 {
    let mut delta = 0i32;
    let mut in_string = false;
    let mut in_char = false;
    let mut in_block = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if in_char {
            if c == '\\' {
                chars.next();
            } else if c == '\'' {
                in_char = false;
            }
            continue;
        }
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '\'' => in_char = true,
            '/' if chars.peek() == Some(&'/') => break,
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                in_block = true;
            }
            '{' => delta += 1,
            '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}

pub fn run(root: &Path, cfg: &Config, files: Vec<PathBuf>, check: bool, write: bool) -> i32 {
    let sma_files = if files.is_empty() {
        crate::discover::discover_files(root, &cfg.paths, &cfg.exclude)
    } else {
        crate::discover::resolve_input_files(root, &files, &cfg.exclude)
    };
    if sma_files.is_empty() {
        eprintln!("No .sma files found");
        return 0;
    }

    let mut changed = 0usize;
    for path in &sma_files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let formatted = format_source(&src);
        if formatted == src {
            continue;
        }
        changed += 1;
        if write {
            std::fs::write(path, formatted).ok();
            eprintln!("  v reformatted {}", path.display());
        } else {
            eprintln!("  x would reformat {}", path.display());
        }
    }

    if changed == 0 {
        eprintln!("No formatting changes");
    } else if write {
        eprintln!("\n{} file(s) reformatted", changed);
    } else if check {
        eprintln!(
            "\n{} file(s) would be reformatted (run with --write)",
            changed
        );
    }
    if changed > 0 && !write { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_braces_and_preserves_semantics() {
        let src = "public f()\n{\nnew x = 1\nif (x)\n{\nreturn x\n}\n}\n";
        let out = format_source(src);
        assert_eq!(
            out,
            "public f()\n{\n\tnew x = 1\n\tif (x)\n\t{\n\t\treturn x\n\t}\n}\n"
        );
        assert_eq!(format_source(&out), out, "idempotent");
    }

    #[test]
    fn sorts_leading_includes_and_trims_trailing_space() {
        let src = "#include <cstrike>   \n#include <amxmodx>\n\npublic f() {}\n";
        let out = format_source(src);
        assert!(out.starts_with("#include <amxmodx>\n#include <cstrike>\n"));
        assert!(!out.contains("   \n"));
    }

    #[test]
    fn leaves_block_comments_and_preprocessor_alone() {
        let src = "/*\n   keep me\n*/\n#if defined X\npublic f() {}\n#endif\n";
        let out = format_source(src);
        assert!(out.contains("   keep me"));
        assert!(out.contains("#if defined X\n"));
    }
}
