//! AST-backed semantic rules that need more than one statement of context.
//!
//! The regex engine already covers the "this line looks wrong" rules. This
//! module covers the four that need a symbol table or a scope walk:
//!
//! * `unused_symbol` (amxxpc warning 203) - a local/global never read.
//! * `write_only_symbol` (warning 204) - assigned but never read.
//! * `use_before_init` (warning 210) - a scalar read before its first write.
//! * `unintended_bitwise` (warning 212) - `&`/`|` between two boolean
//!   expressions inside a condition.
//!
//! Like the rest of `ast_lint`, a file that does not parse never reaches this
//! module; the regex engine answers it instead. Parse diagnostics are never
//! promoted to lint issues.

use std::collections::HashMap;

use zpc_ast::{
    decl::{FuncKind, FuncName, Init, Item, Param, VarDecl},
    expr::{ArgValue, BinOp, Expr, ExprKind, UnOp},
    stmt::{Block, ForInit, Stmt},
};
use zpc_diag::Span;
use zpc_sema::{RefKind, SymKind, SymbolDecl, SymbolTable, Usage};

use crate::ast_lint::Ctx;

pub fn run(cx: &mut Ctx, program: &zpc_ast::Program) {
    if !cx.enabled("unused_symbol")
        && !cx.enabled("write_only_symbol")
        && !cx.enabled("use_before_init")
        && !cx.enabled("unintended_bitwise")
    {
        return;
    }

    let mut table = SymbolTable::new(cx.path);

    // Pass 1: declare every file-scope name first, so a function can refer to a
    // global/function declared below it (Pawn allows that).
    for item in &program.items {
        match item {
            Item::Var(v) => declare_var(&mut table, v, false),
            Item::Const(c) => {
                // Constants are intentionally READ-exempt: flag enums and
                // compile-time tables are routinely "unused" by design.
                table.declare(
                    SymbolDecl::new(c.name.name.clone(), SymKind::Constant, c.name.span)
                        .with_usage(Usage::DEFINED | Usage::CONST | Usage::READ),
                );
            }
            Item::Enum(e) => {
                if let Some(name) = &e.name {
                    table.declare(
                        SymbolDecl::new(name.name.clone(), SymKind::Constant, name.span)
                            .with_usage(Usage::DEFINED | Usage::CONST | Usage::READ),
                    );
                }
                for m in &e.members {
                    table.declare(
                        SymbolDecl::new(m.name.name.clone(), SymKind::Constant, m.name.span)
                            .with_usage(Usage::DEFINED | Usage::CONST | Usage::READ),
                    );
                }
            }
            Item::Func(f) => {
                let FuncName::Ident(name) = &f.name else {
                    continue;
                };
                let mut usage = Usage::DEFINED | Usage::PROTOTYPED;
                if f.body.is_none() {
                    usage.remove(Usage::DEFINED);
                }
                if f.modifiers.public || name.is_implicitly_public() || f.kind != FuncKind::Normal {
                    usage |= Usage::PUBLIC;
                }
                if f.modifiers.stock {
                    usage |= Usage::STOCK;
                }
                if f.modifiers.static_ {
                    usage |= Usage::STATIC;
                }
                if f.kind == FuncKind::Native {
                    usage |= Usage::NATIVE;
                }
                table.declare(
                    SymbolDecl::new(name.name.clone(), SymKind::Function, name.span)
                        .with_usage(usage),
                );
            }
            _ => {}
        }
    }

    // Pass 2: bodies and global initialisers.
    for item in &program.items {
        match item {
            Item::Var(v) => {
                for d in &v.declarators {
                    let mut tr = Tracker::default();
                    tr.enter();
                    for dim in &d.dims {
                        if let Some(e) = &dim.size {
                            walk_expr(cx, &mut table, &mut tr, e);
                        }
                    }
                    if let Some(Init::Expr(e)) = &d.init {
                        walk_expr(cx, &mut table, &mut tr, e);
                    }
                    tr.leave();
                }
            }
            Item::Const(c) => {
                let mut tr = Tracker::default();
                tr.enter();
                walk_expr(cx, &mut table, &mut tr, &c.value);
                tr.leave();
            }
            Item::Func(f) => {
                if let Some(body) = &f.body {
                    let mut tr = Tracker::default();
                    table.enter_scope();
                    tr.enter();
                    for p in &f.params {
                        if let Param::Fixed(p) = p {
                            let kind = if !p.dims.is_empty() {
                                SymKind::RefArray
                            } else if p.by_ref {
                                SymKind::Reference
                            } else {
                                SymKind::Variable
                            };
                            // Parameters arrive initialized; marking READ keeps
                            // callback signatures quiet (a public forward may
                            // deliberately ignore an argument).
                            table.declare(
                                SymbolDecl::new(p.name.name.clone(), kind, p.name.span)
                                    .with_usage(Usage::DEFINED | Usage::READ),
                            );
                            tr.declare(&p.name.name, p.name.span, true);
                        }
                    }
                    walk_block(cx, &mut table, &mut tr, body);
                    tr.leave();
                    table.leave_scope();
                }
            }
            _ => {}
        }
    }

    let diagnostics = table.finish();
    for d in diagnostics {
        match d.code {
            203 if cx.enabled("unused_symbol") => {
                cx.report(
                    d.span.start,
                    &format!("{} (warning 203)", d.message),
                    "unused_symbol",
                );
            }
            204 if cx.enabled("write_only_symbol") => {
                cx.report(
                    d.span.start,
                    &format!("{} (warning 204)", d.message),
                    "write_only_symbol",
                );
            }
            _ => {}
        }
    }
}

fn declare_var(table: &mut SymbolTable, v: &VarDecl, local: bool) {
    for d in &v.declarators {
        let kind = if d.dims.is_empty() {
            SymKind::Variable
        } else {
            SymKind::Array
        };
        let mut usage = Usage::DEFINED;
        if v.modifiers.public || d.name.is_implicitly_public() {
            usage |= Usage::PUBLIC;
        }
        if v.modifiers.stock {
            usage |= Usage::STOCK;
        }
        if v.modifiers.static_ {
            usage |= Usage::STATIC;
        }
        if v.modifiers.is_const {
            usage |= Usage::CONST;
        }
        if local && d.init.is_some() {
            usage |= Usage::WRITTEN;
        }
        table.declare(SymbolDecl::new(d.name.name.clone(), kind, d.name.span).with_usage(usage));
    }
}

fn walk_block(cx: &mut Ctx, table: &mut SymbolTable, tr: &mut Tracker, block: &Block) {
    table.enter_scope();
    tr.enter();
    for s in &block.stmts {
        walk_stmt(cx, table, tr, s);
    }
    tr.leave();
    table.leave_scope();
}

fn walk_stmt(cx: &mut Ctx, table: &mut SymbolTable, tr: &mut Tracker, stmt: &Stmt) {
    match stmt {
        Stmt::Block(b) => walk_block(cx, table, tr, b),
        Stmt::Var(v) => {
            declare_var(table, v, true);
            for d in &v.declarators {
                for dim in &d.dims {
                    if let Some(e) = &dim.size {
                        walk_expr(cx, table, tr, e);
                    }
                }
                let zeroed = !d.dims.is_empty() || v.modifiers.static_;
                match &d.init {
                    Some(Init::Expr(e)) => {
                        tr.declare(&d.name.name, d.name.span, false);
                        walk_expr(cx, table, tr, e);
                        tr.write(&d.name.name);
                    }
                    Some(Init::List(_)) => {
                        tr.declare(&d.name.name, d.name.span, true);
                    }
                    None => {
                        tr.declare(&d.name.name, d.name.span, zeroed);
                    }
                }
            }
        }
        Stmt::Const(_) | Stmt::Enum(_) => {}
        Stmt::Expr { expr, .. } => walk_expr(cx, table, tr, expr),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_bitwise(cx, cond);
            walk_expr(cx, table, tr, cond);
            walk_stmt(cx, table, tr, then_branch);
            if let Some(e) = else_branch {
                walk_stmt(cx, table, tr, e);
            }
        }
        Stmt::While { cond, body, .. } => {
            check_bitwise(cx, cond);
            walk_expr(cx, table, tr, cond);
            walk_stmt(cx, table, tr, body);
        }
        Stmt::DoWhile { body, cond, .. } => {
            walk_stmt(cx, table, tr, body);
            check_bitwise(cx, cond);
            walk_expr(cx, table, tr, cond);
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
            ..
        } => {
            table.enter_scope();
            tr.enter();
            match init {
                Some(ForInit::Decl(v)) => {
                    declare_var(table, v, true);
                    for d in &v.declarators {
                        for dim in &d.dims {
                            if let Some(e) = &dim.size {
                                walk_expr(cx, table, tr, e);
                            }
                        }
                        let zeroed = !d.dims.is_empty() || v.modifiers.static_;
                        match &d.init {
                            Some(Init::Expr(e)) => {
                                tr.declare(&d.name.name, d.name.span, false);
                                walk_expr(cx, table, tr, e);
                                tr.write(&d.name.name);
                            }
                            Some(Init::List(_)) => {
                                tr.declare(&d.name.name, d.name.span, true);
                            }
                            None => {
                                tr.declare(&d.name.name, d.name.span, zeroed);
                            }
                        }
                    }
                }
                Some(ForInit::Expr(e)) => walk_expr(cx, table, tr, e),
                None => {}
            }
            if let Some(c) = cond {
                check_bitwise(cx, c);
                walk_expr(cx, table, tr, c);
            }
            if let Some(s) = step {
                walk_expr(cx, table, tr, s);
            }
            walk_stmt(cx, table, tr, body);
            tr.leave();
            table.leave_scope();
        }
        Stmt::Switch {
            scrutinee,
            cases,
            default,
            ..
        } => {
            walk_expr(cx, table, tr, scrutinee);
            for c in cases {
                for label in &c.labels {
                    match label {
                        zpc_ast::stmt::CaseLabel::Single(e) => walk_expr(cx, table, tr, e),
                        zpc_ast::stmt::CaseLabel::Range { lo, hi, .. } => {
                            walk_expr(cx, table, tr, lo);
                            walk_expr(cx, table, tr, hi);
                        }
                    }
                }
                walk_stmt(cx, table, tr, &c.body);
            }
            if let Some(d) = default {
                walk_stmt(cx, table, tr, d);
            }
        }
        Stmt::Return { value: Some(e), .. }
        | Stmt::Exit { value: Some(e), .. }
        | Stmt::Sleep { value: Some(e), .. } => walk_expr(cx, table, tr, e),
        Stmt::Assert { exprs, .. } => {
            for e in exprs {
                walk_expr(cx, table, tr, e);
            }
        }
        Stmt::State { cond: Some(c), .. } => walk_expr(cx, table, tr, c),
        Stmt::Goto { label, .. } => {
            table.reference(&label.name, label.span, RefKind::Read);
        }
        Stmt::Label { name, .. } => {
            table.declare(SymbolDecl::new(
                name.name.clone(),
                SymKind::Label,
                name.span,
            ));
        }
        Stmt::Emit(e) => {
            if let Some(zpc_ast::stmt::EmitOperand::Symbol(id)) = &e.operand {
                table.reference(&id.name, id.span, RefKind::ReadWrite);
            }
        }
        _ => {}
    }
}

fn walk_expr(cx: &mut Ctx, table: &mut SymbolTable, tr: &mut Tracker, expr: &Expr) {
    match &expr.kind {
        ExprKind::Ident(id) => {
            table.reference(&id.name, id.span, RefKind::Read);
            if let Some((span, message)) = tr.read(&id.name) {
                cx.report(span.start, &message, "use_before_init");
            }
        }
        ExprKind::Comma { exprs, .. } => {
            for e in exprs {
                walk_expr(cx, table, tr, e);
            }
        }
        ExprKind::Unary { operand, .. } => walk_expr(cx, table, tr, operand),
        ExprKind::IncDec { operand, .. } => {
            if let ExprKind::Ident(id) = &operand.kind {
                table.reference(&id.name, id.span, RefKind::ReadWrite);
                if let Some((span, message)) = tr.read(&id.name) {
                    cx.report(span.start, &message, "use_before_init");
                }
                tr.write(&id.name);
            } else {
                walk_expr(cx, table, tr, operand);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_expr(cx, table, tr, lhs);
            walk_expr(cx, table, tr, rhs);
        }
        ExprKind::Assign {
            op, target, value, ..
        } => {
            walk_expr(cx, table, tr, value);
            if let ExprKind::Ident(id) = &target.kind {
                let kind = if op.is_some() {
                    RefKind::ReadWrite
                } else {
                    RefKind::Write
                };
                table.reference(&id.name, id.span, kind);
                if op.is_some()
                    && let Some((span, message)) = tr.read(&id.name)
                {
                    cx.report(span.start, &message, "use_before_init");
                }
                tr.write(&id.name);
            } else {
                walk_expr(cx, table, tr, target);
            }
        }
        ExprKind::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            walk_expr(cx, table, tr, cond);
            walk_expr(cx, table, tr, then_expr);
            walk_expr(cx, table, tr, else_expr);
        }
        ExprKind::Index { base, index, .. } => {
            walk_expr(cx, table, tr, base);
            walk_expr(cx, table, tr, index);
        }
        ExprKind::Call { callee, args, .. } => {
            let api = if let ExprKind::Ident(id) = &callee.kind {
                table.reference(&id.name, id.span, RefKind::Call);
                crate::api::lookup(&id.name)
            } else {
                walk_expr(cx, table, tr, callee);
                None
            };
            for (i, a) in args.iter().enumerate() {
                if let ArgValue::Expr(e) = &a.value {
                    // An uninitialized scalar passed to a by-reference/array
                    // parameter is an out-parameter (`get_user_ping(id, ping,
                    // loss)`), so the call counts as a write. A plain read
                    // argument (`client_print(..., x)`) does not.
                    if let ExprKind::Ident(id) = &e.kind
                        && api
                            .and_then(|f| f.params.get(i))
                            .is_some_and(|p| p.by_ref || p.is_array)
                    {
                        tr.write(&id.name);
                    }
                    walk_expr(cx, table, tr, e);
                }
            }
        }
        ExprKind::Cast { expr, .. } | ExprKind::CharCells { operand: expr, .. } => {
            walk_expr(cx, table, tr, expr);
        }
        ExprKind::SizeOf { symbol, .. } => {
            table.reference(&symbol.name, symbol.span, RefKind::Read);
        }
        ExprKind::TagOf {
            target: zpc_ast::expr::TagOfTarget::Symbol(id),
            ..
        } => {
            table.reference(&id.name, id.span, RefKind::Read);
        }
        ExprKind::LitArray { elems, .. } => {
            for e in elems {
                walk_expr(cx, table, tr, e);
            }
        }
        _ => {}
    }
}

/// `&`/`|` between two expressions that are already boolean is almost always a
/// typo for `&&`/`||` (amxxpc warning 212). `flags & FLAG` stays quiet because a
/// bare identifier is a legitimate bit mask.
fn check_bitwise(cx: &mut Ctx, expr: &Expr) {
    if !cx.enabled("unintended_bitwise") {
        return;
    }
    if let ExprKind::Binary {
        op: BinOp::BitAnd | BinOp::BitOr,
        lhs,
        rhs,
        span,
    } = &expr.kind
        && is_boolean(lhs)
        && is_boolean(rhs)
    {
        cx.report(
            span.start,
            "bitwise operator between two boolean expressions (warning 212) - did you mean '&&'/'||'?",
            "unintended_bitwise",
        );
    }
}

fn is_boolean(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Binary { op, .. } => matches!(
            op,
            BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::LogAnd
                | BinOp::LogOr
        ),
        ExprKind::Unary {
            op: UnOp::LogNot, ..
        } => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::config::RulesConfig;
    use crate::engine::lint_file;
    use crate::rules::LintIssue;

    fn lint(name: &str, src: &str) -> Vec<LintIssue> {
        let path =
            std::env::temp_dir().join(format!("zplint_sema_{}_{}.sma", name, std::process::id()));
        std::fs::write(&path, src).unwrap();
        let issues = lint_file(&path, &RulesConfig::default());
        std::fs::remove_file(path).unwrap();
        issues
    }

    fn has(issues: &[LintIssue], rule: &str) -> bool {
        issues.iter().any(|i| i.rule_id == rule)
    }

    #[test]
    fn unused_and_write_only() {
        let r = lint(
            "unused",
            "public f() {\n\tnew never\n\tnew stored\n\tstored = 1\n}\n",
        );
        assert!(has(&r, "unused_symbol"));
        assert!(has(&r, "write_only_symbol"));

        let ok = lint("used", "public f() {\n\tnew x\n\tx = 1\n\treturn x\n}\n");
        assert!(!has(&ok, "unused_symbol"));
        assert!(!has(&ok, "write_only_symbol"));
    }

    #[test]
    fn public_stock_and_parameters_are_exempt() {
        let r = lint(
            "exempt",
            "stock helper() { return 1 }\npublic f(unused_param) {\n}\n",
        );
        assert!(!has(&r, "unused_symbol"));
        assert!(!has(&r, "write_only_symbol"));
    }

    #[test]
    fn use_before_init_but_out_params_are_ok() {
        let bad = lint(
            "ubad",
            "public f(id) {\n\tnew x\n\tclient_print(id, print_chat, \"%d\", x)\n}\n",
        );
        assert!(has(&bad, "use_before_init"));

        let ok = lint(
            "uok",
            "public f(id) {\n\tnew ping, loss\n\tget_user_ping(id, ping, loss)\n}\n",
        );
        assert!(!has(&ok, "use_before_init"));

        let arrays = lint("uarr", "public f() {\n\tnew a[8]\n\treturn a[0]\n}\n");
        assert!(!has(&arrays, "use_before_init"));
    }

    #[test]
    fn bitwise_between_booleans() {
        let bad = lint(
            "bitbad",
            "public f() {\n\tif ((1 == 1) & (2 == 2)) return\n}\n",
        );
        assert!(has(&bad, "unintended_bitwise"));

        let ok = lint("bitok", "public f(flags) {\n\tif (flags & 1) return\n}\n");
        assert!(!has(&ok, "unintended_bitwise"));
    }
}

#[derive(Default)]
struct Tracker {
    /// One stack of states per name, so an inner block can shadow an outer one.
    locals: HashMap<String, Vec<LocalState>>,
    scopes: Vec<Vec<String>>,
}

struct LocalState {
    declared: Span,
    initialized: bool,
    reported: bool,
}

impl Tracker {
    fn enter(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn leave(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for name in names {
                if let Some(stack) = self.locals.get_mut(&name) {
                    stack.pop();
                    if stack.is_empty() {
                        self.locals.remove(&name);
                    }
                }
            }
        }
    }

    fn declare(&mut self, name: &str, span: Span, initialized: bool) {
        self.locals
            .entry(name.to_string())
            .or_default()
            .push(LocalState {
                declared: span,
                initialized,
                reported: false,
            });
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.to_string());
        }
    }

    fn write(&mut self, name: &str) {
        if let Some(state) = self.locals.get_mut(name).and_then(|s| s.last_mut()) {
            state.initialized = true;
        }
    }

    fn read(&mut self, name: &str) -> Option<(Span, String)> {
        let state = self.locals.get_mut(name)?.last_mut()?;
        if state.initialized || state.reported {
            return None;
        }
        state.reported = true;
        Some((
            state.declared,
            format!(
                "'{}' is read before it is assigned (warning 210: possible use before initialization) - initialize it at the declaration",
                name
            ),
        ))
    }
}
