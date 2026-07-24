//! Signature-level checks against the full AMX Mod X API (see `src/api.rs`).
//!
//! Everything here works off the generated table of all 1600+ natives, stocks and
//! forwards, so the checks cover the whole API surface rather than a hand-picked
//! list: wrong argument count, tag (`Float:` vs cell) mismatches, literals passed
//! to by-reference parameters, deprecated natives and missing `#include`s.

use crate::api::{self, ApiFunc, Kind};
use crate::config::RulesConfig;
use crate::detectors::sanitize_line;
use crate::engine::iss;
use crate::rules::LintIssue;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Words that are followed by `(` but are not calls.
static KEYWORDS: &[&str] = &[
    "if", "while", "for", "switch", "case", "return", "sizeof", "new", "else",
    "do", "static", "public", "stock", "native", "forward", "const", "enum",
    "operator", "defined", "charsmax", "tagof", "state", "assert", "goto",
];

static RE_INT_LIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+$").unwrap());
static RE_FLOAT_LIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+\.\d+$").unwrap());
static RE_INCLUDE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*#include\s*[<"]([^>"]+)[>"]"#).unwrap());
static RE_DEFINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#define\s+([A-Za-z_]\w*)").unwrap());

/// Hand-written tag rules that report the same defect as `api_tag_*` but with a
/// more specific message. When one of them already fired on a line, the generic
/// rule stays quiet so the same bug is not reported twice.
static SPECIFIC_TAG_RULES: &[&str] = &[
    "set_task_int_interval", "pev_float_int", "int_native_float",
    "cs_float_int", "fun_float_int", "engfunc_int_float", "engfunc_float_int",
    "ham_int_float", "ham_float_int", "amxmodx_int_float", "amxmodx_float_int",
];

/// A call site found in the source.
struct Call {
    name: String,
    lineno: usize,
    args: Vec<String>,
    /// True when the argument list could not be closed (unbalanced / truncated).
    truncated: bool,
}

pub fn run(raw_clean: &str, lines: &[&str], config: &RulesConfig, issues: &mut Vec<LintIssue>) {
    let esc = if raw_clean.contains(r"#pragma ctrlchar '\'") { '\\' } else { '^' };
    let sanitized: Vec<String> = lines.iter().map(|l| sanitize_line(l, esc).0).collect();

    let local = local_symbols(lines, &sanitized);
    let calls = collect_calls(&sanitized, &local);

    // `#include` set: only usable when every include is one we know about.
    let (provided, all_includes_known) = included_set(lines);

    for call in &calls {
        let Some(f) = api::lookup(&call.name) else {
            if all_includes_known
                && config.enabled("api_unknown_native")
                && let Some(sug) = near_miss(&call.name)
            {
                let msg = format!(
                    "'{}' is not an AMX Mod X function - did you mean '{}'?",
                    call.name, sug
                );
                issues.push(iss(call.lineno, msg, "api_unknown_native", false));
            }
            continue;
        };
        if f.kind == Kind::Forward {
            continue; // forwards are implemented, not called
        }

        check_arity(call, f, config, issues);
        check_args(call, f, config, issues);

        if let Some(rep) = f.deprecated
            && config.enabled("api_deprecated")
        {
            issues.push(iss(
                call.lineno,
                format!("'{}' is deprecated: {}", f.name, rep),
                "api_deprecated",
                false,
            ));
        }

        if all_includes_known
            && config.enabled("api_missing_include")
            && !provided.contains(f.include)
        {
            issues.push(iss(
                call.lineno,
                format!(
                    "'{}' needs #include <{}>, which this plugin does not include",
                    f.name, f.include
                ),
                "api_missing_include",
                false,
            ));
        }
    }
}

fn check_arity(call: &Call, f: &ApiFunc, config: &RulesConfig, issues: &mut Vec<LintIssue>) {
    if call.truncated || !config.enabled("api_arity") {
        return;
    }
    let argc = call.args.len();
    if argc < f.min_args as usize {
        issues.push(iss(
            call.lineno,
            format!(
                "{}() takes at least {} argument(s), {} given",
                f.name, f.min_args, argc
            ),
            "api_arity",
            false,
        ));
    } else if argc > f.max_args as usize {
        issues.push(iss(
            call.lineno,
            format!(
                "{}() takes at most {} argument(s), {} given",
                f.name, f.max_args, argc
            ),
            "api_arity",
            false,
        ));
    }
}

/// Per-argument tag and by-reference checks.
fn check_args(call: &Call, f: &ApiFunc, config: &RulesConfig, issues: &mut Vec<LintIssue>) {
    let already_tagged = issues
        .iter()
        .any(|i| i.lineno == call.lineno && SPECIFIC_TAG_RULES.contains(&i.rule_id));

    for (i, arg) in call.args.iter().enumerate() {
        let Some(p) = f.params.get(i) else { break }; // variadic tail
        let a = arg.trim();
        let is_int = RE_INT_LIT.is_match(a);
        let is_float = RE_FLOAT_LIT.is_match(a);
        let is_str = a.starts_with('"');

        if p.by_ref && (is_int || is_float || is_str) && config.enabled("api_byref_literal") {
            issues.push(iss(
                call.lineno,
                format!(
                    "{}() argument {} is by-reference (&) - pass a variable, not the literal {}",
                    f.name,
                    i + 1,
                    a
                ),
                "api_byref_literal",
                false,
            ));
            continue;
        }
        if p.is_array || p.tag == "any" || already_tagged {
            continue;
        }
        // A cell holding an int where a Float: is expected is bit-reinterpreted as
        // a denormal (~1e-43). Literal 0 is exempt: its bit pattern is 0.0.
        if is_float && (p.tag == "_" || p.tag == "bool") && config.enabled("api_tag_float_arg") {
            issues.push(iss(
                call.lineno,
                format!(
                    "{}() argument {} is an integer parameter - {} is read as a huge int",
                    f.name,
                    i + 1,
                    a
                ),
                "api_tag_float_arg",
                false,
            ));
        } else if is_int && p.tag == "Float" && a != "0" && config.enabled("api_tag_int_arg") {
            issues.push(iss(
                call.lineno,
                format!(
                    "{}() argument {} is Float: - write {}.0, not {}",
                    f.name,
                    i + 1,
                    a,
                    a
                ),
                "api_tag_int_arg",
                false,
            ));
        }
    }
}

/// Names defined by the plugin itself: top-level function definitions/declarations
/// and `#define`d macros. Calls to these must never be matched against the API,
/// because the plugin's own version shadows it.
fn local_symbols(lines: &[&str], sanitized: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut depth = 0i32;
    for (raw, san) in lines.iter().zip(sanitized) {
        if let Some(c) = RE_DEFINE.captures(raw) {
            out.insert(c[1].to_string());
        }
        if depth == 0
            && let Some(name) = leading_decl_name(san)
        {
            out.insert(name);
        }
        depth += san.matches('{').count() as i32 - san.matches('}').count() as i32;
    }
    out
}

/// For a top-level line, the name being declared/defined (`native foo(`, `public bar(`,
/// `Float:baz(`, `stock qux(`), if any.
fn leading_decl_name(san: &str) -> Option<String> {
    let s = san.trim_start();
    if s.is_empty() || s.starts_with('#') || s.starts_with('}') {
        return None;
    }
    let mut rest = s;
    for kw in ["public ", "stock ", "native ", "forward ", "static "] {
        while let Some(r) = rest.strip_prefix(kw) {
            rest = r.trim_start();
        }
    }
    // optional return tag
    if let Some(pos) = rest.find(':')
        && !rest[..pos].is_empty()
        && rest[..pos].chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        rest = rest[pos + 1..].trim_start();
    }
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || KEYWORDS.contains(&name.as_str()) {
        return None;
    }
    // must be followed by '(' to be a function
    let after = rest[name.len()..].trim_start();
    if after.starts_with('(') {
        Some(name)
    } else {
        None
    }
}

/// All bundled includes reachable from this plugin's `#include` lines, plus whether
/// every `#include` was a bundled one. An unknown (third-party) include means we
/// cannot reason about what is or is not provided, so include-sensitive rules are
/// skipped for that file.
fn included_set(lines: &[&str]) -> (HashSet<&'static str>, bool) {
    let mut set = HashSet::new();
    let mut all_known = true;
    let mut any = false;
    for line in lines {
        let Some(c) = RE_INCLUDE.captures(line) else { continue };
        any = true;
        let name = c[1].trim().trim_end_matches(".inc");
        match api::include_closure(name) {
            Some(closure) => set.extend(closure.iter().copied()),
            None => all_known = false,
        }
    }
    (set, any && all_known)
}

/// Closest API name within edit distance 1, used to report obvious typos only.
/// Anything further away is far more likely to be a symbol we simply do not know.
fn near_miss(name: &str) -> Option<&'static str> {
    if name.len() < 5 {
        return None;
    }
    api::API
        .iter()
        .find(|f| f.kind != Kind::Forward && (edit_distance_1(name, f.name) || transposed(name, f.name)))
        .map(|f| f.name)
}

/// True when `a` becomes `b` by swapping one pair of adjacent characters
/// (`get_user_nmae` -> `get_user_name`), the most common kind of typo.
fn transposed(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let Some(i) = (0..a.len()).find(|&i| a[i] != b[i]) else { return false };
    i + 1 < a.len() && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..]
}

/// True when `a` and `b` differ by exactly one insertion, deletion or substitution.
fn edit_distance_1(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let mut i = 0;
    let mut j = 0;
    let mut diff = 0;
    while i < long.len() && j < short.len() {
        if long[i] == short[j] {
            i += 1;
            j += 1;
            continue;
        }
        diff += 1;
        if diff > 1 {
            return false;
        }
        if long.len() == short.len() {
            i += 1;
            j += 1;
        } else {
            i += 1;
        }
    }
    diff + (long.len() - i) + (short.len() - j) == 1
}

/// Find every call site in the (string-sanitized) source, skipping declarations,
/// keywords and plugin-local symbols. Calls may span lines.
fn collect_calls(sanitized: &[String], local: &HashSet<String>) -> Vec<Call> {
    let mut calls = Vec::new();
    let mut depth = 0i32;
    for (lineno, san) in sanitized.iter().enumerate() {
        let bytes = san.as_bytes();
        let mut i = 0;
        // Preprocessor lines never contain calls we can trust.
        let is_pp = san.trim_start().starts_with('#');
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '{' {
                depth += 1;
                i += 1;
                continue;
            }
            if c == '}' {
                depth -= 1;
                i += 1;
                continue;
            }
            if !(c.is_alphabetic() || c == '_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && ((bytes[i] as char).is_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &san[start..i];
            // Qualified/member-ish contexts and tags (`Float:x`) are not calls.
            let prev = san[..start].chars().rev().find(|c| !c.is_whitespace());
            let after = san[i..].trim_start();
            if !after.starts_with('(')
                || is_pp
                || depth <= 0
                || KEYWORDS.contains(&name)
                || local.contains(name)
                || matches!(prev, Some(':') | Some('.') | Some('#'))
            {
                continue;
            }
            let open = i + (san[i..].len() - after.len());
            let (args, truncated) = read_args(sanitized, lineno, open);
            calls.push(Call {
                name: name.to_string(),
                lineno: lineno + 1,
                args,
                truncated,
            });
        }
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(src: &str) -> Vec<&'static str> {
        let lines: Vec<&str> = src.lines().collect();
        let mut issues = Vec::new();
        run(src, &lines, &RulesConfig::default(), &mut issues);
        issues.into_iter().map(|i| i.rule_id).collect()
    }

    /// Wrap statements in a plugin body; calls are only recognised inside functions.
    fn body(includes: &str, stmts: &str) -> String {
        format!("{includes}\npublic plugin_init()\n{{\n{stmts}\n}}\n")
    }

    #[test]
    fn arity_too_few_and_too_many() {
        let bad = body("#include <amxmodx>", "\tnew n[32]\n\tget_user_name(1, n)");
        assert!(lint(&bad).contains(&"api_arity"));
        let bad2 = body("#include <amxmodx>", "\tnew n[32]\n\tget_user_name(1, n, 31, 5)");
        assert!(lint(&bad2).contains(&"api_arity"));
        let ok = body("#include <amxmodx>", "\tnew n[32]\n\tget_user_name(1, n, charsmax(n))");
        assert!(!lint(&ok).contains(&"api_arity"));
    }

    #[test]
    fn variadic_natives_accept_extra_args() {
        let ok = body(
            "#include <amxmodx>",
            "\tclient_print(0, print_chat, \"%s %d %d\", \"a\", 1, 2)",
        );
        assert!(!lint(&ok).contains(&"api_arity"));
    }

    #[test]
    fn multiline_calls_are_counted_once() {
        let ok = body(
            "#include <amxmodx>",
            "\tnew n[32]\n\tget_user_name(\n\t\t1,\n\t\tn,\n\t\tcharsmax(n)\n\t)",
        );
        assert!(!lint(&ok).contains(&"api_arity"));
    }

    #[test]
    fn tag_mismatch_both_directions() {
        let int_for_float = body("#include <amxmodx>", "\tset_task(5, \"f\")");
        assert!(lint(&int_for_float).contains(&"api_tag_int_arg"));
        let ok = body("#include <amxmodx>", "\tset_task(5.0, \"f\")");
        assert!(!lint(&ok).contains(&"api_tag_int_arg"));
        // literal 0 is exempt: its bit pattern is already 0.0
        let zero = body("#include <amxmodx>", "\tset_task(0, \"f\")");
        assert!(!lint(&zero).contains(&"api_tag_int_arg"));

        let float_for_int = body("#include <fun>", "\tset_user_health(1, 100.0)");
        assert!(lint(&float_for_int).contains(&"api_tag_float_arg"));
    }

    #[test]
    fn missing_include_is_flagged_only_when_all_includes_are_known() {
        let missing = body("#include <amxmodx>", "\tcs_get_user_money(1)");
        assert!(lint(&missing).contains(&"api_missing_include"));
        let present = body("#include <amxmodx>\n#include <cstrike>", "\tcs_get_user_money(1)");
        assert!(!lint(&present).contains(&"api_missing_include"));
        // a third-party include could provide anything, so the rule backs off
        let unknown = body("#include <amxmodx>\n#include <zombieplague>", "\tcs_get_user_money(1)");
        assert!(!lint(&unknown).contains(&"api_missing_include"));
    }

    #[test]
    fn typos_are_reported_but_unknown_symbols_are_not() {
        let typo = body("#include <amxmodx>", "\tnew n[32]\n\tget_user_nmae(1, n, 31)");
        assert!(lint(&typo).contains(&"api_unknown_native"));
        let third_party = body("#include <amxmodx>", "\tzp_get_user_zombie(1)");
        assert!(!lint(&third_party).contains(&"api_unknown_native"));
    }

    #[test]
    fn plugin_local_definitions_shadow_the_api() {
        // the plugin defines its own set_task-like helper; the API signature must not apply
        let src = "#include <amxmodx>\nstock get_user_name(a, b, c, d)\n{\n\treturn a + b + c + d\n}\npublic plugin_init()\n{\n\tget_user_name(1, 2, 3, 4)\n}\n";
        assert!(!lint(src).contains(&"api_arity"));
    }

    #[test]
    fn declarations_and_forwards_are_not_calls() {
        let src = "#include <amxmodx>\nnative my_native(a, b)\nforward client_disconnected(id, bool:drop, message[], maxlen)\npublic plugin_init()\n{\n\tregister_plugin(\"a\", \"b\", \"c\")\n}\n";
        assert!(lint(src).is_empty());
    }

    #[test]
    fn byref_literal_is_flagged() {
        // get_user_ping(index, &ping, &loss) - a literal cannot receive the output
        let bad = body("#include <amxmodx>", "\tget_user_ping(1, 5, 6)");
        assert!(lint(&bad).contains(&"api_byref_literal"));
        let ok = body("#include <amxmodx>", "\tnew p, l\n\tget_user_ping(1, p, l)");
        assert!(!lint(&ok).contains(&"api_byref_literal"));
    }

    #[test]
    fn edit_distance_and_transposition() {
        assert!(edit_distance_1("get_user_nam", "get_user_name"));
        assert!(edit_distance_1("get_user_namex", "get_user_name"));
        assert!(!edit_distance_1("get_user_nmae", "get_user_name"));
        assert!(transposed("get_user_nmae", "get_user_name"));
        assert!(!transposed("get_user_name", "get_user_name"));
    }

    #[test]
    fn api_table_is_sorted_and_lookups_work() {
        assert!(api::API.windows(2).all(|w| w[0].name <= w[1].name));
        assert!(api::lookup("get_user_name").is_some());
        assert!(api::lookup("definitely_not_a_native").is_none());
    }
}

/// Read the argument list starting at the `(` at `lines[lineno][open]`, continuing
/// across lines until the matching `)`. Returns the top-level arguments.
fn read_args(lines: &[String], lineno: usize, open: usize) -> (Vec<String>, bool) {
    let mut depth = 0i32;
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    for (n, line) in lines.iter().enumerate().skip(lineno) {
        let slice = if n == lineno { &line[open..] } else { &line[..] };
        for ch in slice.chars() {
            match ch {
                '(' | '[' | '{' => {
                    depth += 1;
                    if depth == 1 && ch == '(' {
                        started = true;
                        continue;
                    }
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        if !cur.trim().is_empty() {
                            args.push(cur.trim().to_string());
                        }
                        return (args, false);
                    }
                }
                ',' if depth == 1 => {
                    args.push(cur.trim().to_string());
                    cur.clear();
                    continue;
                }
                _ => {}
            }
            if started {
                cur.push(ch);
            }
        }
        cur.push(' ');
        // Bail out on runaway scans (unbalanced source).
        if n - lineno > 40 {
            break;
        }
    }
    (args, true)
}
