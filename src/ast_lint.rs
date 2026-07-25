//! AST-backed lint path: the linter and the `zpc` compiler front-end share one
//! lexer, one preprocessor and one parser.
//!
//! # Why a second path exists
//!
//! The 106 rules in [`crate::detectors`] and [`crate::engine`] are line- and
//! regex-based. That is the right tool for most AMXX rules (they are about
//! *which native is called with what*, which is a local, textual question), but
//! it is the wrong tool for rules about **program structure**: whether a `;` is
//! the whole body of an `if`, whether an `else` is followed by a condition
//! instead of a statement, whether a `==` has a string literal on one side.
//! Those questions have exact answers in a parse tree and only heuristic answers
//! in a regex, so a handful of rules are migrated here. Every migrated rule keeps
//! its original `rule_id`, so `zplint.toml`'s `rules.disable` list, the severity
//! table in [`crate::engine`] and the output format are unaffected.
//!
//! # Non-negotiable: parse failure is never a lint error
//!
//! Real plugins `#include` third-party headers we cannot resolve, use macros we
//! never see expanded, and target compiler quirks the parser does not model yet.
//! [`lint`] therefore returns `Option`: `None` means "this file did not parse
//! cleanly, use the regex engine for it". A parse diagnostic is *never* turned
//! into a [`LintIssue`] — doing so would flood the corpus with false positives
//! and break the "official amxmodx plugins lint with 0 errors" baseline.
//!
//! # How the source is prepared
//!
//! The preprocessor runs with **no include directories**. That is deliberate:
//! with nothing to pull in, `Preprocessed::text` stays line-for-line aligned with
//! the original file (directives and skipped branches become blank lines), and no
//! diagnostic can originate in a header the user did not write. Issue line
//! numbers are still mapped back through [`zpc_lex::LineMap`] and dropped unless
//! they belong to the file being linted.
//!
//! The cost of skipping includes is accepted: `#if defined SOMETHING_FROM_A_HEADER`
//! evaluates false, so that branch is blanked out and not linted. That direction
//! of error produces false *negatives*, never false positives.

#![forbid(unsafe_code)]

use std::path::Path;

use zpc_ast::{
    decl::Item,
    expr::{BinOp, Expr, ExprKind},
    stmt::{Block, ForInit, Stmt},
};
use zpc_diag::{Diagnostics, LineIndex};
use zpc_lex::{Preprocessor, Scanner, preproc::LineMap};

use crate::config::RulesConfig;
use crate::engine::iss;
use crate::rules::LintIssue;

/// Rules that moved to the AST. When [`lint`] succeeds for a file, the engine
/// suppresses these ids in the regex pass so nothing is reported twice.
///
/// `else_paren` is deliberately **not** here. `else (cond) { ... }` does not
/// parse at all — the parser reports "expected `}`, but found `;`" and then an
/// unmatched brace, which is exactly the cascade the rule exists to explain — so
/// every file containing the bug falls back to the regex engine anyway. Moving
/// it would mean the rule never fires. It becomes migratable once the parser
/// grows error recovery good enough to keep building a tree past that point.
pub const MIGRATED_RULES: &[&str] =
    &["empty_statement", "string_literal_compare", "comparison_as_statement"];

/// Parse diagnostics that do **not** disqualify the tree.
///
/// 36 is "empty statement" — the very thing `empty_statement` reports. The
/// parser flags it and then carries on with a faithful [`Stmt::Empty`] node, so
/// treating it as a parse failure would make the migrated rule unreachable.
const TOLERATED_PARSE_CODES: &[u16] = &[36];

/// Lint `src` through the compiler front-end.
///
/// Returns `None` when the file cannot be lexed or parsed without errors; the
/// caller must then fall back to the regex engine for the whole file.
pub fn lint(path: &Path, src: &str, config: &RulesConfig) -> Option<Vec<LintIssue>> {
    let program_src = prepare(path, src)?;
    let (text, map, program) = program_src;

    let mut cx = Ctx {
        index: LineIndex::new(&text),
        map: &map,
        path,
        config,
        out: Vec::new(),
    };

    for item in &program.items {
        match item {
            Item::Func(f) => {
                if let Some(body) = &f.body {
                    cx.walk_block(body);
                }
            }
            // Global initialisers are walked too, so `string_literal_compare`
            // keeps the coverage the line-based rule had.
            Item::Var(v) => cx.walk_var(v),
            Item::Const(c) => cx.walk_expr(&c.value),
            _ => {}
        }
    }

    Some(cx.out)
}

/// True when `path`/`src` reach a clean parse — i.e. when the AST path is in
/// charge of [`MIGRATED_RULES`] for this file. Used by the corpus statistics in
/// `zplint ast-compare`.
pub fn parses_cleanly(path: &Path, src: &str) -> bool {
    prepare(path, src).is_some()
}

/// Preprocess → scan → parse, insisting on zero errors at every stage.
fn prepare(path: &Path, src: &str) -> Option<(String, LineMap, zpc_ast::Program)> {
    // `#include` lines are blanked before preprocessing rather than resolved.
    // An unresolvable include is diagnostic 100, whose severity is *fatal*, and a
    // fatal diagnostic stops the preprocessor mid-file - which would silently
    // leave us linting a truncated program. Blanking sidesteps that, keeps the
    // expanded text line-for-line aligned with the source, and guarantees no
    // finding can originate in a header the user did not write.
    let stripped = blank_includes(src);

    let mut pp = Preprocessor::new(Vec::new());
    let (pre, pp_diags) = pp.process(path, &stripped);
    // Any other fatal (a `#error`, an include cycle, runaway nesting) truncates
    // the output; the remaining text would parse fine and we would lint half a
    // file. Fall back instead.
    if pp_diags.aborted() {
        return None;
    }

    let mut scanner = Scanner::new(&pre.text, path);
    scanner.set_ctrl_char(pre.state.ctrlchar);
    let mut scan_diags = Diagnostics::new();
    let tokens = scanner.scan(&mut scan_diags);
    if scan_diags.error_count() > 0 || scan_diags.aborted() {
        return None;
    }

    let (program, parse_diags) = zpc_parse::parse(&pre.text, &tokens, path);
    if parse_diags.aborted()
        || parse_diags
            .items()
            .iter()
            .any(|d| d.is_error() && !TOLERATED_PARSE_CODES.contains(&d.code))
    {
        return None;
    }
    // Belt and braces: a recovered `Error` node means the tree is not a faithful
    // picture of the source even if no diagnostic was counted as an error.
    if program.items.iter().any(|i| matches!(i, Item::Error(_))) {
        return None;
    }

    Some((pre.text, pre.map, program))
}

/// Replace every `#include` / `#tryinclude` line with an empty one, keeping the
/// line count and therefore every later line number.
fn blank_includes(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for (i, line) in src.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let t = line.trim_start();
        if !(t.starts_with("#include") || t.starts_with("#tryinclude")) {
            out.push_str(line);
        }
    }
    out
}

struct Ctx<'a> {
    index: LineIndex,
    map: &'a LineMap,
    path: &'a Path,
    config: &'a RulesConfig,
    out: Vec<LintIssue>,
}

impl Ctx<'_> {
    /// 1-based line in the *original* file for a byte offset in the expanded
    /// text, or `None` when the offset came from another file.
    fn line_of(&self, offset: u32) -> Option<usize> {
        let (line, _) = self.index.line_col(offset);
        match self.map.origin(line as usize - 1) {
            Some((file, src_line)) if file == self.path => Some(src_line as usize),
            Some(_) => None,
            // No map entry (e.g. an empty file): the text is the source.
            None => Some(line as usize),
        }
    }

    fn report(&mut self, offset: u32, message: &str, rule_id: &'static str) {
        if let Some(lineno) = self.line_of(offset) {
            self.out.push(iss(lineno, message.to_string(), rule_id, false));
        }
    }

    fn enabled(&self, rule_id: &str) -> bool {
        self.config.enabled(rule_id)
    }

    fn walk_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.walk_stmt(stmt, true);
        }
    }

    /// `in_block` is true only for statements that are direct children of a
    /// compound statement, i.e. not the single-statement body of an `if`/loop.
    fn walk_stmt(&mut self, stmt: &Stmt, in_block: bool) {
        match stmt {
            Stmt::Block(b) => self.walk_block(b),

            Stmt::If { cond, then_branch, else_branch, span } => {
                self.walk_expr(cond);
                // `empty_statement`: `if (x);` — the `;` *is* the body, so the
                // block below runs unconditionally (amxxpc error 036). The regex
                // form has to scan forward past the closing paren by hand and
                // special-case `while (...)` after a `}` to avoid flagging the
                // tail of a do-while; on the AST a `do ... while (c);` is a
                // `Stmt::DoWhile`, so the ambiguity does not exist.
                if self.enabled("empty_statement") && matches!(**then_branch, Stmt::Empty(_)) {
                    self.report(
                        span.start,
                        "semicolon right after the condition detaches the block below (error 036: empty statement)",
                        "empty_statement",
                    );
                }
                self.walk_stmt(then_branch, false);

                if let Some(e) = else_branch {
                    self.walk_stmt(e, false);
                }
            }

            Stmt::While { cond, body, span } => {
                self.walk_expr(cond);
                if self.enabled("empty_statement") && matches!(**body, Stmt::Empty(_)) {
                    self.report(
                        span.start,
                        "semicolon right after the condition detaches the block below (error 036: empty statement)",
                        "empty_statement",
                    );
                }
                self.walk_stmt(body, false);
            }

            Stmt::DoWhile { body, cond, .. } => {
                self.walk_stmt(body, false);
                self.walk_expr(cond);
            }

            Stmt::For { init, cond, step, body, .. } => {
                match init {
                    Some(ForInit::Expr(e)) => self.walk_expr(e),
                    Some(ForInit::Decl(d)) => self.walk_var(d),
                    None => {}
                }
                if let Some(c) = cond {
                    self.walk_expr(c);
                }
                if let Some(s) = step {
                    self.walk_expr(s);
                }
                // A `for (...);` body is a deliberate idiom often enough that the
                // regex rule never covered it; not flagging it here keeps the two
                // paths in agreement.
                self.walk_stmt(body, false);
            }

            Stmt::Switch { scrutinee, cases, default, .. } => {
                self.walk_expr(scrutinee);
                for case in cases {
                    self.walk_stmt(&case.body, false);
                }
                if let Some(d) = default {
                    self.walk_stmt(d, false);
                }
            }

            Stmt::Expr { expr, .. } => {
                // `comparison_as_statement`: `a == b;` computes a value and drops
                // it (warning 215) — almost always a typo for `=`. Restricting it
                // to direct children of a block is what keeps it from
                // double-reporting the `else (cond)` above. The regex form has to
                // require a trailing `;` and forbid `|&<>` in the right operand to
                // avoid tripping over multi-line conditions; the AST knows the
                // statement boundary exactly.
                if in_block
                    && self.enabled("comparison_as_statement")
                    && let ExprKind::Binary { op, .. } = &expr.kind
                    && matches!(op, BinOp::Eq | BinOp::Ne)
                {
                    self.report(
                        expr.span.start,
                        "comparison used as a statement does nothing (warning 215) - did you mean '='?",
                        "comparison_as_statement",
                    );
                }
                self.walk_expr(expr);
            }

            Stmt::Var(v) => self.walk_var(v),
            Stmt::Const(c) => self.walk_expr(&c.value),
            Stmt::Return { value: Some(e), .. }
            | Stmt::Exit { value: Some(e), .. }
            | Stmt::Sleep { value: Some(e), .. } => self.walk_expr(e),
            Stmt::Assert { exprs, .. } => {
                for e in exprs {
                    self.walk_expr(e);
                }
            }
            Stmt::State { cond: Some(c), .. } => self.walk_expr(c),
            _ => {}
        }
    }

    fn walk_var(&mut self, decl: &zpc_ast::decl::VarDecl) {
        for d in &decl.declarators {
            if let Some(zpc_ast::decl::Init::Expr(e)) = &d.init {
                self.walk_expr(e);
            }
        }
    }

    fn walk_expr(&mut self, expr: &Expr) {
        // `string_literal_compare`: `name == "admin"` compares the *address* of
        // the array with the address of the literal (error 033) — it never does
        // what it looks like. The regex form matches the text `== "` anywhere on
        // the line and then has to prove the match is outside a string literal
        // and outside a `#` directive by hand; on the AST it is exactly "a
        // `Eq`/`Ne` node with a `Str` operand", with no possible confusion
        // between source text and program structure.
        if let ExprKind::Binary { op, lhs, rhs, span } = &expr.kind
            && matches!(op, BinOp::Eq | BinOp::Ne)
            && self.enabled("string_literal_compare")
            && (matches!(lhs.kind, ExprKind::Str(_)) || matches!(rhs.kind, ExprKind::Str(_)))
        {
            self.report(
                span.start,
                "strings cannot be compared with ==/!= (error 033) - use equal()/equali()",
                "string_literal_compare",
            );
        }

        for child in children(expr) {
            self.walk_expr(child);
        }
    }

}

/// Direct sub-expressions, in source order.
fn children(expr: &Expr) -> Vec<&Expr> {
    match &expr.kind {
        ExprKind::LitArray { elems, .. } | ExprKind::Comma { exprs: elems, .. } => {
            elems.iter().collect()
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::IncDec { operand, .. }
        | ExprKind::CharCells { operand, .. } => vec![operand],
        ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
        ExprKind::Assign { target, value, .. } => vec![target, value],
        ExprKind::Ternary { cond, then_expr, else_expr, .. } => vec![cond, then_expr, else_expr],
        ExprKind::Index { base, index, .. } => vec![base, index],
        ExprKind::Cast { expr, .. } => vec![expr],
        ExprKind::Call { callee, args, .. } => {
            let mut v = vec![&**callee];
            for a in args {
                if let zpc_ast::expr::ArgValue::Expr(e) = &a.value {
                    v.push(e);
                }
            }
            v
        }
        _ => Vec::new(),
    }
}

// --- comparison mode --------------------------------------------------------

/// One place where the AST path and the regex path disagree about a migrated
/// rule.
pub struct Divergence {
    pub rule_id: String,
    pub lineno: usize,
    /// `true` when only the AST path reported it, `false` when only the regex
    /// path did.
    pub ast_only: bool,
}

/// Lint `path` both ways and report every migrated-rule finding the two paths do
/// not agree on. Backs `zplint ast-compare`, which is how the migration is
/// checked against a whole corpus rather than against fixtures alone.
pub fn compare(path: &Path, config: &RulesConfig) -> Vec<Divergence> {
    let mut regex_cfg = config.clone();
    regex_cfg.ast = false;
    let mut ast_cfg = config.clone();
    ast_cfg.ast = true;

    let regex_found = migrated_findings(&crate::engine::lint_file(path, &regex_cfg));
    let ast_found = migrated_findings(&crate::engine::lint_file(path, &ast_cfg));

    let mut out = Vec::new();
    for f in &ast_found {
        if !regex_found.contains(f) {
            out.push(Divergence { rule_id: f.0.to_string(), lineno: f.1, ast_only: true });
        }
    }
    for f in &regex_found {
        if !ast_found.contains(f) {
            out.push(Divergence { rule_id: f.0.to_string(), lineno: f.1, ast_only: false });
        }
    }
    out.sort_by(|a, b| (a.lineno, &a.rule_id).cmp(&(b.lineno, &b.rule_id)));
    out
}

fn migrated_findings(issues: &[LintIssue]) -> Vec<(&'static str, usize)> {
    issues
        .iter()
        .filter(|i| MIGRATED_RULES.contains(&i.rule_id))
        .map(|i| (i.rule_id, i.lineno))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir()
            .join(format!("zplint_ast_{}_{}.sma", name, std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn rules_at(path: &std::path::Path) -> Vec<&'static str> {
        let src = std::fs::read_to_string(path).unwrap();
        lint(path, &src, &RulesConfig::default())
            .expect("fixture must parse")
            .iter()
            .map(|i| i.rule_id)
            .collect()
    }

    #[test]
    fn empty_statement_is_flagged_on_the_ast() {
        let p = write("empty", "public f(x) {\n    if (x);\n    {\n        x++;\n    }\n}\n");
        assert!(rules_at(&p).contains(&"empty_statement"));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn do_while_terminator_is_not_an_empty_statement() {
        let p = write(
            "dowhile",
            "public f(x) {\n    do\n    {\n        x++;\n    }\n    while (x < 3);\n}\n",
        );
        assert!(!rules_at(&p).contains(&"empty_statement"));
        std::fs::remove_file(p).unwrap();
    }

    /// `else (cond)` is the reason `else_paren` stays on the regex path: the
    /// construct does not parse, so the AST path bows out and the regex engine
    /// still reports it. This pins that contract.
    #[test]
    fn else_paren_falls_back_to_the_regex_engine() {
        let p = write("elsebad", "public f(x) {\n    if (x == 1)\n        x++;\n    else (x == 2)\n    {\n        x--;\n    }\n}\n");
        let src = std::fs::read_to_string(&p).unwrap();
        assert!(lint(&p, &src, &RulesConfig::default()).is_none());
        let issues = crate::engine::lint_file(&p, &RulesConfig::default());
        assert!(issues.iter().any(|i| i.rule_id == "else_paren"));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn string_literal_compare_is_flagged() {
        let p = write("strcmp", "public f(name[]) {\n    if (name == \"admin\")\n        return 1;\n    return 0;\n}\n");
        assert!(rules_at(&p).contains(&"string_literal_compare"));
        std::fs::remove_file(p).unwrap();
    }

    /// A `==` inside a *string* cannot be a comparison. The regex rule needs an
    /// explicit "is this offset inside a literal?" scan to know that; the parser
    /// gets it for free.
    #[test]
    fn equals_quote_inside_a_string_literal_is_not_a_comparison() {
        let p = write(
            "strinner",
            "public f(id) {\n    client_print(id, 3, \"score == ^\"top^\"\");\n}\n",
        );
        assert!(!rules_at(&p).contains(&"string_literal_compare"));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn comparison_as_statement_is_flagged_only_at_block_level() {
        let p = write("cmpstmt", "public f(x, y) {\n    x == y;\n}\n");
        assert!(rules_at(&p).contains(&"comparison_as_statement"));
        std::fs::remove_file(p).unwrap();

        let ok = write("cmpstmt_ok", "public f(x, y) {\n    if (x == y)\n        return 1;\n    return 0;\n}\n");
        assert!(!rules_at(&ok).contains(&"comparison_as_statement"));
        std::fs::remove_file(ok).unwrap();
    }

    #[test]
    fn unparsable_source_falls_back_instead_of_erroring() {
        let p = write("broken", "public f(\n");
        let src = std::fs::read_to_string(&p).unwrap();
        assert!(lint(&p, &src, &RulesConfig::default()).is_none());
        // and the whole-engine path still produces no parser-derived error
        let issues = crate::engine::lint_file(&p, &RulesConfig::default());
        assert!(issues.iter().all(|i| !i.message.contains("expected")));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn unresolvable_includes_do_not_block_the_ast_path() {
        let p = write(
            "inc",
            "#include <amxmodx>\n#include <zombieplague>\n\npublic plugin_init() {\n    new x = 1;\n    if (x);\n}\n",
        );
        assert!(rules_at(&p).contains(&"empty_statement"));
        std::fs::remove_file(p).unwrap();
    }

    /// Line numbers must survive preprocessing: directives are blanked, not
    /// removed, so a finding keeps the line the user sees in the editor.
    #[test]
    fn line_numbers_survive_directives() {
        let p = write(
            "lines",
            "#include <amxmodx>\n#define FOO 1\n\npublic f(x) {\n    if (x);\n}\n",
        );
        let src = std::fs::read_to_string(&p).unwrap();
        let issues = lint(&p, &src, &RulesConfig::default()).unwrap();
        assert_eq!(issues[0].lineno, 5);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn disable_list_still_silences_a_migrated_rule() {
        let p = write("disabled", "public f(x) {\n    if (x);\n}\n");
        let src = std::fs::read_to_string(&p).unwrap();
        let mut cfg = RulesConfig::default();
        cfg.disable.push("empty_statement".to_string());
        assert!(lint(&p, &src, &cfg).unwrap().is_empty());
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn both_paths_agree_on_a_representative_plugin() {
        let p = write(
            "agree",
            r#"#include <amxmodx>

public plugin_init() {
    register_plugin("t", "1.0", "a");
}

public cmd(id, name[]) {
    if (name == "admin")
        return 1;
    if (id);
    {
        client_print(id, 3, "hi");
    }
    if (id == 1)
        return 2;
    else if (id == 2)
        return 3;
    return 0;
}
"#,
        );
        let div = compare(&p, &RulesConfig::default());
        assert!(div.is_empty(), "paths disagree: {:?}", div.iter().map(|d| (&d.rule_id, d.lineno, d.ast_only)).collect::<Vec<_>>());
        std::fs::remove_file(p).unwrap();
    }
}
