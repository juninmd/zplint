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
        && !cx.enabled("duplicate_global")
        && !cx.enabled("sizeof_function")
        && !cx.enabled("mixed_array_return")
        && !cx.enabled("in_file_arity")
    {
        return;
    }

    let mut table = SymbolTable::new(cx.path);
    let mut local_sigs: HashMap<String, (usize, usize)> = HashMap::new();
    let mut global_dims: HashMap<String, usize> = HashMap::new();

    // Pass 1: declare every file-scope name first, so a function can refer to a
    // global/function declared below it (Pawn allows that).
    for item in &program.items {
        match item {
            Item::Var(v) => {
                declare_var(&mut table, v, false);
                for d in &v.declarators {
                    global_dims.insert(d.name.name.clone(), d.dims.len());
                }
            }
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

                if crate::api::lookup(&name.name).is_none() {
                    let min_args = f
                        .params
                        .iter()
                        .filter(|p| matches!(p, Param::Fixed(fp) if fp.default.is_none()))
                        .count();
                    let max_args = if f.params.iter().any(|p| matches!(p, Param::Rest(_))) {
                        usize::MAX
                    } else {
                        f.params.len()
                    };
                    if f.body.is_some() || !local_sigs.contains_key(&name.name) {
                        local_sigs.insert(name.name.clone(), (min_args, max_args));
                    }
                }
            }
            _ => {}
        }
    }

    let sema_ctx = SemaCtx {
        local_sigs: &local_sigs,
        global_dims: &global_dims,
    };

    // Pass 2: bodies and global initialisers.
    for item in &program.items {
        match item {
            Item::Var(v) => {
                for d in &v.declarators {
                    let mut tr = Tracker::default();
                    tr.enter();
                    for dim in &d.dims {
                        if let Some(e) = &dim.size {
                            walk_expr(cx, &sema_ctx, &mut table, &mut tr, e);
                        }
                    }
                    if let Some(Init::Expr(e)) = &d.init {
                        walk_expr(cx, &sema_ctx, &mut table, &mut tr, e);
                    }
                    tr.leave();
                }
            }
            Item::Const(c) => {
                let mut tr = Tracker::default();
                tr.enter();
                walk_expr(cx, &sema_ctx, &mut table, &mut tr, &c.value);
                tr.leave();
            }
            Item::Func(f) => {
                if let Some(body) = &f.body {
                    let mut tr = Tracker::default();
                    let mut ret = ReturnTracker::default();
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
                            tr.declare(&p.name.name, p.name.span, true, p.dims.len());
                        }
                    }
                    walk_block(cx, &sema_ctx, &mut table, &mut tr, &mut ret, body);
                    if cx.enabled("mixed_array_return")
                        && let (Some(arr_off), Some(scalar_off)) = (ret.array_ret, ret.scalar_ret)
                    {
                        cx.report(
                            arr_off.max(scalar_off),
                            "function mixes array/string and scalar return values (error 079: inconsistent return types)",
                            "mixed_array_return",
                        );
                    }
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
            21 if cx.enabled("duplicate_global") => {
                cx.report(
                    d.span.start,
                    &format!("{} (error 021)", d.message),
                    "duplicate_global",
                );
            }
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

struct SemaCtx<'a> {
    local_sigs: &'a HashMap<String, (usize, usize)>,
    global_dims: &'a HashMap<String, usize>,
}

#[derive(Default)]
struct ReturnTracker {
    array_ret: Option<u32>,
    scalar_ret: Option<u32>,
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

fn walk_block(
    cx: &mut Ctx,
    sema: &SemaCtx,
    table: &mut SymbolTable,
    tr: &mut Tracker,
    ret: &mut ReturnTracker,
    block: &Block,
) {
    table.enter_scope();
    tr.enter();
    for s in &block.stmts {
        walk_stmt(cx, sema, table, tr, ret, s);
    }
    tr.leave();
    table.leave_scope();
}

fn walk_stmt(
    cx: &mut Ctx,
    sema: &SemaCtx,
    table: &mut SymbolTable,
    tr: &mut Tracker,
    ret: &mut ReturnTracker,
    stmt: &Stmt,
) {
    match stmt {
        Stmt::Block(b) => walk_block(cx, sema, table, tr, ret, b),
        Stmt::Var(v) => {
            declare_var(table, v, true);
            for d in &v.declarators {
                for dim in &d.dims {
                    if let Some(e) = &dim.size {
                        walk_expr(cx, sema, table, tr, e);
                    }
                }
                let zeroed = !d.dims.is_empty() || v.modifiers.static_;
                let dims = d.dims.len();
                match &d.init {
                    Some(Init::Expr(e)) => {
                        tr.declare(&d.name.name, d.name.span, false, dims);
                        walk_expr(cx, sema, table, tr, e);
                        tr.write(&d.name.name);
                    }
                    Some(Init::List(_)) => {
                        tr.declare(&d.name.name, d.name.span, true, dims);
                    }
                    None => {
                        tr.declare(&d.name.name, d.name.span, zeroed, dims);
                    }
                }
            }
        }
        Stmt::Const(_) | Stmt::Enum(_) => {}
        Stmt::Expr { expr, .. } => walk_expr(cx, sema, table, tr, expr),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_bitwise(cx, cond);
            walk_expr(cx, sema, table, tr, cond);
            walk_stmt(cx, sema, table, tr, ret, then_branch);
            if let Some(e) = else_branch {
                walk_stmt(cx, sema, table, tr, ret, e);
            }
        }
        Stmt::While { cond, body, .. } => {
            check_bitwise(cx, cond);
            walk_expr(cx, sema, table, tr, cond);
            walk_stmt(cx, sema, table, tr, ret, body);
        }
        Stmt::DoWhile { body, cond, .. } => {
            walk_stmt(cx, sema, table, tr, ret, body);
            check_bitwise(cx, cond);
            walk_expr(cx, sema, table, tr, cond);
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
                                walk_expr(cx, sema, table, tr, e);
                            }
                        }
                        let zeroed = !d.dims.is_empty() || v.modifiers.static_;
                        let dims = d.dims.len();
                        match &d.init {
                            Some(Init::Expr(e)) => {
                                tr.declare(&d.name.name, d.name.span, false, dims);
                                walk_expr(cx, sema, table, tr, e);
                                tr.write(&d.name.name);
                            }
                            Some(Init::List(_)) => {
                                tr.declare(&d.name.name, d.name.span, true, dims);
                            }
                            None => {
                                tr.declare(&d.name.name, d.name.span, zeroed, dims);
                            }
                        }
                    }
                }
                Some(ForInit::Expr(e)) => walk_expr(cx, sema, table, tr, e),
                None => {}
            }
            if let Some(c) = cond {
                check_bitwise(cx, c);
                walk_expr(cx, sema, table, tr, c);
            }
            if let Some(s) = step {
                walk_expr(cx, sema, table, tr, s);
            }
            walk_stmt(cx, sema, table, tr, ret, body);
            tr.leave();
            table.leave_scope();
        }
        Stmt::Switch {
            scrutinee,
            cases,
            default,
            ..
        } => {
            walk_expr(cx, sema, table, tr, scrutinee);
            for c in cases {
                for label in &c.labels {
                    match label {
                        zpc_ast::stmt::CaseLabel::Single(e) => {
                            walk_expr(cx, sema, table, tr, e)
                        }
                        zpc_ast::stmt::CaseLabel::Range { lo, hi, .. } => {
                            walk_expr(cx, sema, table, tr, lo);
                            walk_expr(cx, sema, table, tr, hi);
                        }
                    }
                }
                walk_stmt(cx, sema, table, tr, ret, &c.body);
            }
            if let Some(d) = default {
                walk_stmt(cx, sema, table, tr, ret, d);
            }
        }
        Stmt::Return {
            value: Some(e),
            span,
        } => {
            match expr_is_array(e, tr, sema.global_dims, table) {
                Some(true) => {
                    ret.array_ret.get_or_insert(span.start);
                }
                Some(false) => {
                    ret.scalar_ret.get_or_insert(span.start);
                }
                None => {}
            }
            walk_expr(cx, sema, table, tr, e);
        }
        Stmt::Exit { value: Some(e), .. } | Stmt::Sleep { value: Some(e), .. } => {
            walk_expr(cx, sema, table, tr, e)
        }
        Stmt::Assert { exprs, .. } => {
            for e in exprs {
                walk_expr(cx, sema, table, tr, e);
            }
        }
        Stmt::State { cond: Some(c), .. } => walk_expr(cx, sema, table, tr, c),
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

fn expr_is_array(
    expr: &Expr,
    tr: &Tracker,
    global_dims: &HashMap<String, usize>,
    table: &SymbolTable,
) -> Option<bool> {
    match &expr.kind {
        ExprKind::Str(_) | ExprKind::LitArray { .. } => Some(true),
        ExprKind::Num(_)
        | ExprKind::Rational(_)
        | ExprKind::Char { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::IncDec { .. }
        | ExprKind::SizeOf { .. }
        | ExprKind::TagOf { .. }
        | ExprKind::Defined { .. }
        | ExprKind::CharCells { .. } => Some(false),
        ExprKind::Ident(id) => {
            if id.name == "true" || id.name == "false" {
                return Some(false);
            }
            if let Some(dims) = tr.dims(&id.name).or_else(|| global_dims.get(&id.name).copied()) {
                return Some(dims > 0);
            }
            if let Some(sym) = table.lookup(&id.name)
                && table.get(sym).kind == SymKind::Constant
            {
                return Some(false);
            }
            None
        }
        ExprKind::Index { .. } => {
            let mut cur = expr;
            let mut depth = 0usize;
            while let ExprKind::Index { base, .. } = &cur.kind {
                depth += 1;
                cur = base;
            }
            if let ExprKind::Ident(id) = &cur.kind
                && let Some(dims) = tr.dims(&id.name).or_else(|| global_dims.get(&id.name).copied())
            {
                if dims > depth {
                    return Some(true);
                }
                if dims == depth {
                    return Some(false);
                }
            }
            None
        }
        ExprKind::Cast { expr, .. } => expr_is_array(expr, tr, global_dims, table),
        ExprKind::Ternary {
            then_expr,
            else_expr,
            ..
        } => {
            match (
                expr_is_array(then_expr, tr, global_dims, table),
                expr_is_array(else_expr, tr, global_dims, table),
            ) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            }
        }
        _ => None,
    }
}

fn walk_expr(
    cx: &mut Ctx,
    sema: &SemaCtx,
    table: &mut SymbolTable,
    tr: &mut Tracker,
    expr: &Expr,
) {
    match &expr.kind {
        ExprKind::Ident(id) => {
            table.reference(&id.name, id.span, RefKind::Read);
            if let Some((span, message)) = tr.read(&id.name) {
                cx.report(span.start, &message, "use_before_init");
            }
        }
        ExprKind::Comma { exprs, .. } => {
            for e in exprs {
                walk_expr(cx, sema, table, tr, e);
            }
        }
        ExprKind::Unary { operand, .. } => walk_expr(cx, sema, table, tr, operand),
        ExprKind::IncDec { operand, .. } => {
            if let ExprKind::Ident(id) = &operand.kind {
                table.reference(&id.name, id.span, RefKind::ReadWrite);
                if let Some((span, message)) = tr.read(&id.name) {
                    cx.report(span.start, &message, "use_before_init");
                }
                tr.write(&id.name);
            } else {
                walk_expr(cx, sema, table, tr, operand);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_expr(cx, sema, table, tr, lhs);
            walk_expr(cx, sema, table, tr, rhs);
        }
        ExprKind::Assign {
            op, target, value, ..
        } => {
            walk_expr(cx, sema, table, tr, value);
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
                walk_expr(cx, sema, table, tr, target);
            }
        }
        ExprKind::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            walk_expr(cx, sema, table, tr, cond);
            walk_expr(cx, sema, table, tr, then_expr);
            walk_expr(cx, sema, table, tr, else_expr);
        }
        ExprKind::Index { base, index, .. } => {
            walk_expr(cx, sema, table, tr, base);
            walk_expr(cx, sema, table, tr, index);
        }
        ExprKind::Call {
            callee, args, span, ..
        } => {
            let api = if let ExprKind::Ident(id) = &callee.kind {
                table.reference(&id.name, id.span, RefKind::Call);
                if cx.enabled("in_file_arity")
                    && tr.dims(&id.name).is_none()
                    && let Some(&(min, max)) = sema.local_sigs.get(&id.name)
                {
                    let has_named = args.iter().any(|a| a.name.is_some());
                    if args.len() > max {
                        cx.report(
                            span.start,
                            &format!(
                                "function '{}' takes at most {} argument(s), {} given (error 092)",
                                id.name,
                                max,
                                args.len()
                            ),
                            "in_file_arity",
                        );
                    } else if !has_named && args.len() < min {
                        cx.report(
                            span.start,
                            &format!(
                                "function '{}' requires at least {} argument(s), {} given (warning 202)",
                                id.name,
                                min,
                                args.len()
                            ),
                            "in_file_arity",
                        );
                    }
                }
                crate::api::lookup(&id.name)
            } else {
                walk_expr(cx, sema, table, tr, callee);
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
                    walk_expr(cx, sema, table, tr, e);
                }
            }
        }
        ExprKind::Cast { expr, .. } | ExprKind::CharCells { operand: expr, .. } => {
            walk_expr(cx, sema, table, tr, expr);
        }
        ExprKind::SizeOf { symbol, .. } => {
            table.reference(&symbol.name, symbol.span, RefKind::Read);
            if cx.enabled("sizeof_function") {
                let is_func = match table.lookup(&symbol.name) {
                    Some(id) => table.get(id).kind.is_function(),
                    None => crate::api::lookup(&symbol.name).is_some(),
                };
                if is_func {
                    cx.report(
                        symbol.span.start,
                        &format!(
                            "sizeof cannot be applied to function '{}' (error 072: \"sizeof\" operator is invalid on \"function\" symbols)",
                            symbol.name
                        ),
                        "sizeof_function",
                    );
                }
            }
        }
        ExprKind::TagOf {
            target: zpc_ast::expr::TagOfTarget::Symbol(id),
            ..
        } => {
            table.reference(&id.name, id.span, RefKind::Read);
        }
        ExprKind::LitArray { elems, .. } => {
            for e in elems {
                walk_expr(cx, sema, table, tr, e);
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

    #[test]
    fn duplicate_global_is_flagged_but_forward_pair_passes() {
        let bad = lint("dupbad", "new g_x;\nnew g_x;\npublic f() { g_x = 1; }\n");
        assert!(has(&bad, "duplicate_global"));

        let bad_fn = lint("dupfn", "helper() { return 1; }\nhelper() { return 2; }\n");
        assert!(has(&bad_fn, "duplicate_global"));

        let ok = lint("dupok", "forward my_fwd(id);\npublic my_fwd(id) { return id; }\n");
        assert!(!has(&ok, "duplicate_global"));
    }

    #[test]
    fn sizeof_function_is_flagged_but_array_passes() {
        let bad = lint("szfnbad", "helper() { return 1; }\npublic f() {\n\treturn sizeof helper;\n}\n");
        assert!(has(&bad, "sizeof_function"));

        let bad_nat = lint("szfnnat", "public f() {\n\treturn sizeof get_user_name;\n}\n");
        assert!(has(&bad_nat, "sizeof_function"));

        let ok = lint("szfnok", "public f() {\n\tnew arr[8];\n\treturn sizeof arr;\n}\n");
        assert!(!has(&ok, "sizeof_function"));
    }

    #[test]
    fn mixed_array_return_is_flagged() {
        let bad = lint(
            "mixarrbad",
            "stock f(cond) {\n\tnew arr[4];\n\tif (cond) return arr;\n\treturn 0;\n}\n",
        );
        assert!(has(&bad, "mixed_array_return"));

        let ok_elem = lint(
            "mixarrok1",
            "stock f(cond) {\n\tnew arr[4];\n\tif (cond) return arr[0];\n\treturn 0;\n}\n",
        );
        assert!(!has(&ok_elem, "mixed_array_return"));

        let ok_both = lint(
            "mixarrok2",
            "stock f(cond, a[4], b[4]) {\n\tif (cond) return a;\n\treturn b;\n}\n",
        );
        assert!(!has(&ok_both, "mixed_array_return"));
    }

    #[test]
    fn in_file_arity_checks_local_functions() {
        let bad_few = lint(
            "arityfew",
            "helper(a, b) { return a + b; }\npublic f() { return helper(1); }\n",
        );
        assert!(has(&bad_few, "in_file_arity"));

        let bad_many = lint(
            "aritymany",
            "helper(a) { return a; }\npublic f() { return helper(1, 2); }\n",
        );
        assert!(has(&bad_many, "in_file_arity"));

        let ok_def = lint(
            "aritydef",
            "helper(a, b = 10) { return a + b; }\npublic f() { return helper(1) + helper(1, 2); }\n",
        );
        assert!(!has(&ok_def, "in_file_arity"));

        let ok_var = lint(
            "arityvar",
            "helper(a, ...) { return a; }\npublic f() { return helper(1, 2, 3, 4); }\n",
        );
        assert!(!has(&ok_var, "in_file_arity"));
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
    dims: usize,
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

    fn declare(&mut self, name: &str, span: Span, initialized: bool, dims: usize) {
        self.locals
            .entry(name.to_string())
            .or_default()
            .push(LocalState {
                declared: span,
                initialized,
                reported: false,
                dims,
            });
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.to_string());
        }
    }

    fn dims(&self, name: &str) -> Option<usize> {
        self.locals.get(name)?.last().map(|s| s.dims)
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
