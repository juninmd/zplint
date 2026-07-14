//! Detectors sourced from web research (see docs/KNOWLEDGE.md).
//! All rules here are on by default and can be turned off via `rules.disable`.

use crate::config::RulesConfig;
use crate::engine::{enclosing_function_name, extract_call_args, iss};
use crate::rules::*;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static RE_ELSE_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\belse\s*\(").unwrap());
static RE_STR_COMPARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[=!]=\s*""#).unwrap());
static RE_TASK_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bset_task\s*\(\s*(\d+)\s*,").unwrap());
static RE_PEV_FLOAT_INT: LazyLock<Regex> = LazyLock::new(|| {
    // literal 0 is exempt: its bit pattern equals 0.0
    Regex::new(r"\bset_pev\s*\(\s*[^,]+,\s*pev_(health|gravity|maxspeed|speed|dmg|takedamage|animtime|framerate|scale|renderamt|frame|fuser[1-4])\s*,\s*-?0*[1-9]\d*\s*\)").unwrap()
});
static RE_INT_NATIVE_FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(set_user_health|set_user_armor|set_user_frags|cs_set_user_money|zp_ammopacks_set)\s*\(\s*[^,]+,\s*-?\d+\.\d+").unwrap()
});
static RE_USERID_INDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+\[\s*get_user_userid\s*\(").unwrap());
static RE_FIND_ENT_CONST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"while\s*\(\s*\(?\s*\w+\s*=\s*(?:find_ent_by_(?:class|owner|target)\s*\(\s*(?:-1|0)\s*,|engfunc\s*\(\s*EngFunc_FindEntityByString\s*,\s*(?:-1|0)\s*,)").unwrap()
});
static RE_PRECACHE_MP3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(precache_sound|emit_sound)\s*\([^)]*"[^"]+\.mp3""#).unwrap()
});
static RE_SOUND_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(precache_sound|emit_sound)\s*\([^"]*"sound/"#).unwrap()
});
static RE_MP3_LOADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)client_cmd\s*\([^;]*"(?:mp3\s+play|spk)[^"]*loading"#).unwrap()
});
static RE_TE_RELIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bmessage_begin\s*\(\s*MSG_(ALL|ONE)\s*,\s*SVC_TEMPENTITY").unwrap()
});
static RE_CHANGELEVEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"server_cmd\s*\(\s*"changelevel"#).unwrap());
static RE_DEPRECATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(md5|md5_file|strbreak)\s*\(").unwrap());
static RE_DEPRECATED_DISCONNECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bpublic\s+client_disconnect\s*\(").unwrap()
});
static RE_RESERVED_DEFINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#define\s+(MAX_PLAYERS|MAX_NAME_LENGTH|MAX_STRING_LENGTH|MAX_MOTD_LENGTH|MAX_IP_LENGTH|MAX_AUTHID_LENGTH)\b").unwrap()
});
static RE_CONST_COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bif\s*\(\s*(0|1|true|false)\s*\)").unwrap());
static RE_EMPTY_STMT_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(if|while)\s*\(").unwrap());
static RE_SELF_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z_][\w\[\]]*)\s*=\s*([A-Za-z_][\w\[\]]*)\s*;?\s*$").unwrap()
});
static RE_STRING_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([A-Za-z_]\w*)\s*=\s*"([^"]*)"\s*;?\s*$"#).unwrap()
});
static RE_ARRAY_SIZE_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?(\w+)\s*\[\s*(\d+)\s*\]").unwrap()
});
static RE_CMP_STMT: LazyLock<Regex> = LazyLock::new(|| {
    // require the trailing `;` - without it the line is usually a multi-line condition
    Regex::new(r"^\s*[A-Za-z_][\w\[\]]*\s*==\s*[^;=|&<>]+;\s*$").unwrap()
});
static RE_STRLEN_LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"for\s*\([^;]*;[^;]*\bstrlen\s*\(").unwrap());
static RE_GET_CVAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bget_cvar_(num|float|string)\s*\(").unwrap());
static RE_NEW_ARRAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?\w+\s*\[\s*(\d+)\s*\]").unwrap());
static RE_RW_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(read_file|write_file)\s*\(").unwrap());
static RE_PRECACHE_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bprecache_(model|sound|generic)\s*\(").unwrap());
static RE_DIV_RUNTIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[/%]\s*(get_playersnum|get_maxplayers|get_pcvar_num|get_pcvar_float)\s*\(").unwrap()
});
static RE_PRAGMA_DYNAMIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#pragma\s+dynamic").unwrap());
static RE_GLOBAL_NEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^new\s+(.+)$").unwrap());
static RE_DECL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|,)\s*(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)").unwrap());
static RE_LOCAL_NEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)").unwrap());
static RE_PLAYER_ARR_32: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bnew\s+(?:const\s+)?(?:\w+:)?([A-Za-z_]\w*)\s*\[\s*32\s*\]").unwrap()
});
static RE_LOOP_HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(for|while)\s*\(").unwrap());
static RE_CONTAIN_COND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:if|while)\s*\(\s*!?\s*(contain|containi)\s*\(").unwrap()
});
static RE_STRCMP_COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:if|while)\s*\(\s*strcmp\s*\(").unwrap());
static RE_CMP_OP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[=!<>]=|[<>]").unwrap());
static RE_ZP_REG_STMT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*zp_(items|class_zombie|class_human|gamemodes)_register\s*\(").unwrap()
});
static RE_ZP_GET_INIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bzp_(?:items|class_zombie|class_human|gamemodes)_get_(?:id|count)\s*\(").unwrap()
});
static RE_ZP_FW_INFECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_core_(?:infect|cure)(?:_post)?\s*\(\s*\w+\s*,\s*(\w+)\s*\)").unwrap()
});
static RE_ZP_SELECT_PRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_(?:items|class_zombie|class_human)_select_pre\s*\(\s*\w+\s*,\s*(\w+)").unwrap()
});
static RE_ZP_CORE_PRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+zp_fw_core_(?:infect|cure)_pre\s*\(").unwrap()
});
static RE_ZP43_NATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bzp_(get_user_zombie|register_extra_item|register_zombie_class|get_user_ammo_packs|set_user_ammo_packs)\s*\(").unwrap()
});
static RE_DEATHMSG_REG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"register_event\s*\(\s*"DeathMsg"\s*,\s*"(\w+)""#).unwrap()
});
static RE_READ_DATA1: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"new\s+(\w+)\s*=\s*read_data\s*\(\s*1\s*\)").unwrap());
static RE_PRECACHE_MODEL_LIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"precache_model\s*\(\s*"([^"]+)""#).unwrap());
static RE_SET_MODEL_LIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:entity_set_model\s*\(\s*[^,]+,\s*|EngFunc_SetModel\s*,\s*[^,]+,\s*|set_pev\s*\(\s*[^,]+,\s*pev_model\s*,\s*)"([^"]+)""#).unwrap()
});
static RE_CREATE_ENT_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bcreate_entity\s*\(|EngFunc_CreateNamedEntity").unwrap()
});
static RE_REMOVE_ENT_ANY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"remove_entity\s*\(|REMOVE_ENTITY|EngFunc_RemoveEntity|FL_KILLME").unwrap()
});
static RE_FWD_ZERO_ARG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+(plugin_init|plugin_cfg|plugin_precache|plugin_end|plugin_natives)\s*\(\s*([^)\s][^)]*)\)").unwrap()
});
static RE_FWD_ONE_ARG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"public\s+(client_putinserver|client_command|client_infochanged)\s*\(([^)]*)\)").unwrap()
});
static RE_CASE_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*case\s+[^:]+:\s*(//.*)?$").unwrap());
static RE_PP_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#(if|ifdef|ifndef|else|elseif|emit|endif)\b").unwrap());
static RE_CALLBACK_STR: LazyLock<Regex> = LazyLock::new(|| {
    // first arg (no comma/paren inside), then the quoted callback name
    Regex::new(r#"\b(register_clcmd|register_concmd|register_srvcmd|register_logevent|register_message|menu_create)\s*\([^,()]+,\s*"([A-Za-z_]\w*)""#).unwrap()
});
static RE_PUBLIC_HANDLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"return\s+PLUGIN_HANDLED\b").unwrap());
static RE_IDENT_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_]\w*$").unwrap());

/// Strip string/char literal contents and `//` comments. `esc` is the file's
/// escape character: AMXX defaults to `^`, overridable via `#pragma ctrlchar`.
/// Returns the sanitized line and whether a double-quoted string was left open.
pub(crate) fn sanitize_line(line: &str, esc: char) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_str = false;
    let mut in_char = false;
    while let Some(c) = chars.next() {
        if in_str {
            if c == esc {
                chars.next();
                continue;
            }
            if c == '"' {
                in_str = false;
                out.push('"');
            }
            continue;
        }
        if in_char {
            if c == esc {
                chars.next();
                continue;
            }
            if c == '\'' {
                in_char = false;
                out.push('\'');
            }
            continue;
        }
        match c {
            '"' => { in_str = true; out.push('"'); }
            '\'' => { in_char = true; out.push('\''); }
            '/' if chars.peek() == Some(&'/') => break,
            _ => out.push(c),
        }
    }
    (out, in_str)
}

/// True if the byte offset `pos` in `line` is outside any string literal.
fn outside_string(line: &str, pos: usize, esc: char) -> bool {
    let mut in_str = false;
    let mut skip = false;
    for (i, c) in line.char_indices() {
        if i >= pos { break; }
        if skip { skip = false; continue; }
        if in_str && c == esc { skip = true; continue; }
        if c == '"' { in_str = !in_str; }
    }
    !in_str
}

/// True when the balanced `(...)` condition starting after the opening paren is
/// immediately followed by only `;` (an empty statement).
fn condition_ends_with_semicolon(after_paren: &str) -> bool {
    let mut depth = 1i32;
    for (i, c) in after_paren.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return after_paren[i + 1..].trim() == ";";
                }
            }
            _ => {}
        }
    }
    false
}

/// Single `=` (not ==/!=/<=/>=/compound) at paren depth 1 of an if/while condition.
/// Depth >= 2 (the `if ((x = y))` idiom and call arguments) is not flagged.
fn assignment_in_condition(line: &str) -> bool {
    let Some(m) = Regex::new(r"\b(if|while)\s*\(").unwrap().find(line) else { return false; };
    let cond = &line[m.end()..];
    let mut depth = 1i32;
    let bytes: Vec<char> = cond.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '(' => depth += 1,
            ')' => { depth -= 1; if depth == 0 { break; } }
            '=' if depth == 1 => {
                let prev = if i > 0 { bytes[i - 1] } else { ' ' };
                let next = if i + 1 < bytes.len() { bytes[i + 1] } else { ' ' };
                if next != '=' && !"=!<>+-*/%&|^".contains(prev) {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

pub fn run(raw_clean: &str, lines: &[&str], config: &RulesConfig, issues: &mut Vec<crate::rules::LintIssue>) {
    let esc = if raw_clean.contains(r"#pragma ctrlchar '\'") { '\\' } else { '^' };
    let sanitized: Vec<(String, bool)> = lines.iter().map(|l| sanitize_line(l, esc)).collect();

    // Shared context: brace depth per line, loop membership, function names.
    let mut depth_before: Vec<i32> = Vec::with_capacity(lines.len());
    let mut in_loop: Vec<bool> = Vec::with_capacity(lines.len());
    {
        let mut depth = 0i32;
        let mut loop_stack: Vec<i32> = Vec::new();
        let mut pending_loop = false;
        for (san, _) in &sanitized {
            depth_before.push(depth);
            while let Some(&top) = loop_stack.last() {
                if depth < top { loop_stack.pop(); } else { break; }
            }
            in_loop.push(!loop_stack.is_empty());
            let opens = san.matches('{').count() as i32;
            let closes = san.matches('}').count() as i32;
            let is_loop_header = RE_LOOP_HEADER.is_match(san);
            if is_loop_header && opens > 0 {
                loop_stack.push(depth + 1);
            } else if is_loop_header {
                pending_loop = true;
            } else if pending_loop {
                if opens > 0 { loop_stack.push(depth + 1); }
                pending_loop = false;
            }
            depth += opens - closes;
        }
    }

    let publics = find_publics(raw_clean);
    let mut function_names: Vec<String> = publics.iter().map(|n| n.to_string()).collect();
    function_names.extend(find_nonpublics(raw_clean, &publics));

    let has_pragma_dynamic = RE_PRAGMA_DYNAMIC.is_match(raw_clean);
    let raw_sq = squash(raw_clean);

    // Declared array sizes (any scope) for string_assign.
    let mut array_sizes: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (san, _) in &sanitized {
        for caps in RE_ARRAY_SIZE_DECL.captures_iter(san) {
            if let Ok(n) = caps.get(2).unwrap().as_str().parse::<usize>() {
                array_sizes.entry(caps.get(1).unwrap().as_str().to_string()).or_insert(n);
            }
        }
    }

    // Globals (depth 0 `new` declarations) for shadowing / player_array_32.
    let mut globals: HashSet<String> = HashSet::new();
    let mut player32: Vec<(usize, String)> = Vec::new();
    for (i, (san, _)) in sanitized.iter().enumerate() {
        if depth_before[i] != 0 { continue; }
        if let Some(caps) = RE_GLOBAL_NEW.captures(san.trim_start()) {
            let decls = caps.get(1).unwrap().as_str();
            let stop = decls.find(&['=', '['][..]).unwrap_or(decls.len());
            for c in RE_DECL_NAME.captures_iter(&decls[..stop.min(decls.len())]) {
                globals.insert(c.get(1).unwrap().as_str().to_string());
            }
            // multi declarations after `[..]` are rare; also record simple names conservatively
            for c in RE_PLAYER_ARR_32.captures_iter(san) {
                player32.push((i + 1, c.get(1).unwrap().as_str().to_string()));
            }
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let lineno = i + 1;
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('*') { continue; }
        let (san, unterminated) = &sanitized[i];
        let san_trim = san.trim();

        // --- compile-structure ---
        if config.enabled("else_paren") && RE_ELSE_PAREN.is_match(san) && !san.contains("else if") {
            issues.push(iss(lineno, "'else (cond)' derails the parser (error 029/010) - use 'else if (cond)'".into(), "else_paren", false));
        }

        if config.enabled("unterminated_string") && *unterminated
            && !line.trim_end().ends_with('\\') && !line.trim_end().ends_with('^') {
            // a continuation of a multi-line string starts mid-string on this line
            let prev_continues = lines[..i].iter().rev().find(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().ends_with('\\') || l.trim_end().ends_with('^'))
                .unwrap_or(false);
            if !prev_continues {
                issues.push(iss(lineno, "unterminated string literal (error 037: possibly non-terminated string)".into(), "unterminated_string", false));
            }
        }

        if config.enabled("line_too_long") && line.len() > 511 {
            issues.push(iss(lineno, format!("line is {} chars; amxxpc 1.8.x errors past 511 (error 075: input line too long) - split the statement", line.len()), "line_too_long", false));
        }

        if config.enabled("empty_statement") && let Some(m) = RE_EMPTY_STMT_HEAD.find(san)
            && condition_ends_with_semicolon(&san[m.end()..]) {
            // `while (...);` right after `}` is a do-while terminator
            let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim()).unwrap_or("");
            if !(san_trim.starts_with("while") && (prev == "}" || prev.ends_with('}'))) {
                issues.push(iss(lineno, "semicolon right after the condition detaches the block below (error 036: empty statement)".into(), "empty_statement", false));
            }
        }

        if config.enabled("stacked_case") && RE_CASE_LABEL.is_match(san)
            && let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty())
                && next.trim_start().starts_with("case ") {
                issues.push(iss(lineno, "Pawn switch has no fallthrough; stacked 'case A:' 'case B:' does not compile - use 'case A, B:'".into(), "stacked_case", false));
            }

        // --- correctness ---
        if config.enabled("string_literal_compare")
            && let Some(m) = RE_STR_COMPARE.find(line)
            && outside_string(line, m.start(), esc) && !san_trim.starts_with('#') {
            issues.push(iss(lineno, "strings cannot be compared with ==/!= (error 033) - use equal()/equali()".into(), "string_literal_compare", false));
        }

        if config.enabled("assignment_in_condition") && assignment_in_condition(san) {
            issues.push(iss(lineno, "assignment inside condition (warning 211) - use == or wrap in double parentheses if intended".into(), "assignment_in_condition", false));
        }

        if config.enabled("comparison_as_statement") && RE_CMP_STMT.is_match(san)
            && !san_trim.starts_with("for") && !san_trim.starts_with("if") && !san_trim.starts_with("while") {
            issues.push(iss(lineno, "comparison used as a statement does nothing (warning 215) - did you mean '='?".into(), "comparison_as_statement", false));
        }

        if config.enabled("self_assignment") && let Some(caps) = RE_SELF_ASSIGN.captures(san)
            && squash(caps.get(1).unwrap().as_str()) == squash(caps.get(2).unwrap().as_str()) {
            issues.push(iss(lineno, "variable assigned to itself (warning 226) - probably a typo in the source variable".into(), "self_assignment", false));
        }

        // assigning a literal that fits is legal Pawn; only a too-long literal is error 047
        if config.enabled("string_assign") && let Some(caps) = RE_STRING_ASSIGN.captures(line)
            && !["new ", "static ", "const ", "stock ", "#"].iter().any(|kw| san_trim.starts_with(kw)) {
            let name = caps.get(1).unwrap().as_str();
            let lit = caps.get(2).unwrap().as_str();
            // effective cells: escape sequences (^x) collapse to one cell, plus terminator
            let mut cells = 1usize;
            let mut esc_next = false;
            for c in lit.chars() {
                if esc_next { esc_next = false; continue; }
                if c == esc { esc_next = true; }
                cells += 1;
            }
            if let Some(size) = array_sizes.get(name)
                && cells > *size {
                issues.push(iss(lineno, format!("string of {} cells assigned to {}[{}] does not fit (error 047: array sizes do not match) - use copy()", cells, name, size), "string_assign", false));
            }
        }

        if config.enabled("constant_condition") && RE_CONST_COND.is_match(san) {
            issues.push(iss(lineno, "constant condition dead-codes this branch (warnings 205/206) - debugging leftover?".into(), "constant_condition", false));
        }

        if config.enabled("contain_truthy") && let Some(m) = RE_CONTAIN_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) {
                issues.push(iss(lineno, "contain()/containi() return -1 when NOT found; bare truthiness inverts the logic - compare with != -1".into(), "contain_truthy", false));
            }
        }

        if config.enabled("strcmp_truthy") && let Some(m) = RE_STRCMP_COND.find(san) {
            let rest = &san[m.end()..];
            let close = rest.find(')').map(|p| &rest[p..]).unwrap_or("");
            if !RE_CMP_OP.is_match(close) && !san.contains("!strcmp") {
                issues.push(iss(lineno, "strcmp() returns 0 on match; bare 'if (strcmp(..))' means strings DIFFER - use equal() or '== 0'".into(), "strcmp_truthy", false));
            }
        }

        if config.enabled("formatex_self") {
            for args in extract_call_args(line, "formatex") {
                if args.len() >= 3 && RE_IDENT_ONLY.is_match(args[0].trim()) {
                    let buf = args[0].trim();
                    let re_word = Regex::new(&format!(r"\b{}\b", regex::escape(buf))).unwrap();
                    if args.iter().skip(2).any(|a| re_word.is_match(a)) {
                        issues.push(iss(lineno, format!("formatex() output buffer \"{}\" is also an input; formatex skips copy-back checking - use format()", buf), "formatex_self", false));
                    }
                }
            }
        }

        // --- tag mismatch ---
        if config.enabled("set_task_int_interval") && let Some(caps) = RE_TASK_INT.captures(san)
            && caps.get(1).unwrap().as_str() != "0" {
            issues.push(iss(lineno, format!("set_task interval {} is an integer (warning 213); the bit pattern becomes ~1e-44s (runs every frame) - write {}.0", caps.get(1).unwrap().as_str(), caps.get(1).unwrap().as_str()), "set_task_int_interval", false));
        }

        if config.enabled("pev_float_int") && RE_PEV_FLOAT_INT.is_match(san) {
            issues.push(iss(lineno, "integer literal into a Float pev field (warning 213); e.g. pev_health 100 becomes ~1.4e-43 (instant death) - add .0".into(), "pev_float_int", false));
        }

        if config.enabled("int_native_float") && RE_INT_NATIVE_FLOAT.is_match(san) {
            issues.push(iss(lineno, "float literal into an integer native (warning 213); 100.0 becomes 1120403456 - drop the decimals or use floatround()".into(), "int_native_float", false));
        }

        // --- runtime crashes ---
        if config.enabled("userid_as_index") && RE_USERID_INDEX.is_match(san) {
            issues.push(iss(lineno, "get_user_userid() is a session counter (can be 500+), not a client index - indexing an array with it is run time error 4".into(), "userid_as_index", false));
        }

        if config.enabled("find_ent_no_advance") && RE_FIND_ENT_CONST.is_match(san) {
            issues.push(iss(lineno, "entity-search loop restarts from a constant every iteration - infinite loop, server freezes; pass the previous entity as start index".into(), "find_ent_no_advance", false));
        }

        if config.enabled("div_by_runtime") && let Some(m) = RE_DIV_RUNTIME.find(san)
            && outside_string(line, m.start(), esc) {
            issues.push(iss(lineno, "division/modulo by a runtime value that can be zero (empty server / cvar 0) - run time error 11; guard > 0 first".into(), "div_by_runtime", false));
        }

        if config.enabled("pragma_dynamic_stack") && !has_pragma_dynamic
            && (line.starts_with(' ') || line.starts_with('\t'))
            && let Some(caps) = RE_NEW_ARRAY.captures(san)
            && caps.get(1).unwrap().as_str().parse::<u32>().unwrap_or(0) >= 2048 {
            issues.push(iss(lineno, format!("local array of {} cells can blow the default 4096-cell AMX stack (run time error 3) - add #pragma dynamic or make it global/static", caps.get(1).unwrap().as_str()), "pragma_dynamic_stack", false));
        }

        // --- engine/HLDS ---
        if config.enabled("precache_mp3") && RE_PRECACHE_MP3.is_match(line) {
            issues.push(iss(lineno, ".mp3 cannot go through precache_sound/emit_sound - use precache_generic(\"sound/...\") + client_cmd \"mp3 play\"".into(), "precache_mp3", false));
        }

        if config.enabled("sound_prefix") && RE_SOUND_PREFIX.is_match(line) {
            issues.push(iss(lineno, "precache_sound/emit_sound paths are relative to sound/ - \"sound/x.wav\" resolves to sound/sound/x.wav and never plays".into(), "sound_prefix", false));
        }

        if config.enabled("mp3_loading_path") && RE_MP3_LOADING.is_match(line) {
            issues.push(iss(lineno, "GoldSrc clients reject stufftext containing 'loading' - this mp3/spk path is silently blocked (amxmodx issue #818)".into(), "mp3_loading_path", false));
        }

        if config.enabled("te_reliable") && RE_TE_RELIABLE.is_match(san) {
            issues.push(iss(lineno, "SVC_TEMPENTITY on the reliable channel (MSG_ALL/MSG_ONE) can overflow netchan and kick players - use MSG_BROADCAST/MSG_ONE_UNRELIABLE".into(), "te_reliable", false));
        }

        if config.enabled("changelevel_cmd") && RE_CHANGELEVEL.is_match(line) {
            issues.push(iss(lineno, "server_cmd(\"changelevel\") skips the server_changelevel forward and map validity check - use is_map_valid() + change_level()".into(), "changelevel_cmd", false));
        }

        if config.enabled("hud_channel_range") {
            for args in extract_call_args(san, "set_hudmessage") {
                if let Some(ch) = args.get(10).and_then(|a| a.trim().parse::<i32>().ok())
                    && !(-1..=4).contains(&ch) {
                    issues.push(iss(lineno, format!("set_hudmessage channel {} - clients only have channels 1-4 (or -1 auto); other values are masked by the engine and stomp channels unpredictably", ch), "hud_channel_range", false));
                }
            }
        }

        // --- deprecated / defines ---
        if config.enabled("deprecated_symbols") {
            if let Some(caps) = RE_DEPRECATED.captures(san) {
                issues.push(iss(lineno, format!("{}() is deprecated in AMXX 1.9 (warning 233) - use the hasher API / argbreak()", caps.get(1).unwrap().as_str()), "deprecated_symbols", false));
            }
            if RE_DEPRECATED_DISCONNECT.is_match(san) {
                issues.push(iss(lineno, "client_disconnect is deprecated in AMXX 1.9 - client_disconnected also fires for aborted connections (prevents state leaks)".into(), "deprecated_symbols", false));
            }
        }

        if config.enabled("define_reserved_const") && let Some(caps) = RE_RESERVED_DEFINE.captures(line) {
            issues.push(iss(lineno, format!("#define {} redefines an amxconst.inc constant (warning 201); a different value silently desynchronizes buffer sizes", caps.get(1).unwrap().as_str()), "define_reserved_const", false));
        }

        // --- perf (loop / hot path) ---
        if config.enabled("strlen_in_loop") && RE_STRLEN_LOOP.is_match(san) {
            issues.push(iss(lineno, "strlen() in the loop condition is recomputed every iteration (O(n^2)) - cache the length before the loop".into(), "strlen_in_loop", false));
        }

        if in_loop[i] {
            if config.enabled("buffer_in_loop") && let Some(caps) = RE_NEW_ARRAY.captures(san)
                && caps.get(1).unwrap().as_str().parse::<u32>().unwrap_or(0) >= 64 {
                issues.push(iss(lineno, "array declared inside a loop body is re-zeroed every iteration - hoist it out of the loop".into(), "buffer_in_loop", false));
            }
            if config.enabled("read_file_loop") && RE_RW_FILE.is_match(san) {
                issues.push(iss(lineno, "read_file/write_file reopen and rescan the file per call (O(n^2) in loops) - use fopen/fgets/fputs/fclose".into(), "read_file_loop", false));
            }
            if config.enabled("precache_in_loop") && RE_PRECACHE_CALL.is_match(san) {
                issues.push(iss(lineno, "precache_* inside a loop risks the 512-entry engine precache limit (fatal Host_Error at map start)".into(), "precache_in_loop", false));
            }
        }

        if config.enabled("get_cvar_hotpath") && let Some(m) = RE_GET_CVAR.find(san)
            && outside_string(line, m.start(), esc) {
            let f = enclosing_function_name(lines, i, &function_names);
            if !matches!(f.as_deref(), Some("plugin_init") | Some("plugin_cfg") | Some("plugin_precache") | Some("plugin_natives") | Some("plugin_end") | None) {
                issues.push(iss(lineno, "get_cvar_* does a string lookup per call - cache the pointer from register_cvar() and use get_pcvar_* (docs: 'dozens of times faster')".into(), "get_cvar_hotpath", false));
            }
        }

        // --- format injection ---
        if config.enabled("format_injection") {
            let candidates: [(&str, usize); 3] = [("client_print", 3), ("console_print", 2), ("log_amx", 1)];
            for (native, fmt_count) in candidates {
                for args in extract_call_args(san, native) {
                    if args.len() == fmt_count && RE_IDENT_ONLY.is_match(args[fmt_count - 1].trim()) {
                        let ident = args[fmt_count - 1].trim().to_string();
                        let body_sq = squash(&enclosing_body(lines, i));
                        let user_controlled = body_sq.contains(&format!("read_args({},", ident))
                            || (body_sq.contains("read_argv(") && body_sq.contains(&format!(",{},", ident)))
                            || body_sq.contains(&format!("get_user_name({},", ident).replacen(ident.as_str(), "", 1))
                            || Regex::new(&format!(r"get_user_name\([^,]+,{}\b", regex::escape(&ident))).unwrap().is_match(&body_sq);
                        if user_controlled {
                            issues.push(iss(lineno, format!("{}() format argument \"{}\" holds user text; a '%' in chat/nickname is interpreted as a format specifier - use a literal \"%s\"", native, ident), "format_injection", false));
                        }
                    }
                }
            }
        }

        // --- global shadowing ---
        if config.enabled("global_shadowing") && depth_before[i] > 0
            && let Some(caps) = RE_LOCAL_NEW.captures(san) {
            let name = caps.get(1).unwrap().as_str();
            if globals.contains(name) {
                issues.push(iss(lineno, format!("local 'new {}' shadows the global (warning 219) - writes never reach the global", name), "global_shadowing", false));
            }
        }

        // --- ZP50 ---
        if config.enabled("zp50_register_return") && RE_ZP_REG_STMT.is_match(san) {
            issues.push(iss(lineno, "registration id discarded; it is the only handle to filter select_pre/forwards for YOUR item/class - assign it to a global".into(), "zp50_register_return", false));
        }

        if config.enabled("zp50_get_in_init") && RE_ZP_GET_INIT.is_match(san)
            && enclosing_function_name(lines, i, &function_names).as_deref() == Some("plugin_init") {
            issues.push(iss(lineno, "zp50 query natives in plugin_init hit 'Invalid Array Handle' when plugin load order puts you before the core - query in plugin_cfg or forwards".into(), "zp50_get_in_init", false));
        }

        if config.enabled("zp_fw_attacker_guard") && let Some(caps) = RE_ZP_FW_INFECT.captures(san) {
            let attacker = caps.get(1).unwrap().as_str().to_string();
            let body = enclosing_body(lines, i);
            let body_sq = squash(&body);
            let uses = uses_player_native_on(&body, &attacker)
                || body_sq.contains(&format!("zp_ammopacks_set({}", attacker))
                || body_sq.contains(&format!("zp_ammopacks_get({}", attacker))
                || body_sq.contains(&format!("get_user_name({},", attacker));
            let guarded = has_guard(&body, &attacker)
                || body_sq.contains(&format!("!{}", attacker))
                || body_sq.contains(&format!("if({})", attacker))
                || body_sq.contains(&format!("if({}&&", attacker));
            if uses && !guarded {
                issues.push(iss(lineno, format!("zp_fw_core_infect/cure '{}' is 0 for gamemode/admin/console infections (documented) - guard before player natives or it errors every round start", attacker), "zp_fw_attacker_guard", false));
            }
        }

        if config.enabled("zp_select_pre_filter") && let Some(caps) = RE_ZP_SELECT_PRE.captures(san) {
            let param = caps.get(1).unwrap().as_str();
            let body = enclosing_body(lines, i);
            let body_sq = squash(&body);
            let restrictive = body_sq.contains("ZP_ITEM_NOT_AVAILABLE") || body_sq.contains("ZP_ITEM_DONT_SHOW")
                || body_sq.contains("ZP_CLASS_NOT_AVAILABLE") || body_sq.contains("ZP_CLASS_DONT_SHOW");
            // referencing the param at all (cost lookup, comparison, ...) counts as filtering;
            // the bug is ignoring it entirely (manager plugins legitimately apply to all items)
            let refs = Regex::new(&format!(r"\b{}\b", regex::escape(param)))
                .map(|re| re.find_iter(&body).count()).unwrap_or(2);
            if restrictive && refs < 2 {
                issues.push(iss(lineno, format!("select_pre returns a restrictive ZP_* without ever using '{}' - the max across plugins wins, this blocks/hides EVERY item/class server-wide", param), "zp_select_pre_filter", false));
            }
        }

        if config.enabled("zp_select_pre_return") {
            if RE_ZP_SELECT_PRE.is_match(san) {
                let body_sq = squash(&enclosing_body(lines, i));
                // PLUGIN_CONTINUE (=0) aliases ZP_*_AVAILABLE and is harmless
                if body_sq.contains("returnPLUGIN_HANDLED") {
                    issues.push(iss(lineno, "select_pre forwards use ZP_ITEM_*/ZP_CLASS_* return constants; PLUGIN_HANDLED (=1) accidentally means NOT_AVAILABLE".into(), "zp_select_pre_return", false));
                }
            }
            if RE_ZP_CORE_PRE.is_match(san) {
                let body_sq = squash(&enclosing_body(lines, i));
                if body_sq.contains("returnZP_ITEM_") || body_sq.contains("returnZP_CLASS_") {
                    issues.push(iss(lineno, "zp_fw_core_*_pre is blocked with PLUGIN_HANDLED, not ZP_ITEM_*/ZP_CLASS_* constants".into(), "zp_select_pre_return", false));
                }
            }
        }

        // --- forward contracts ---
        if config.enabled("client_command_handled") && RE_PUBLIC_HANDLED.is_match(san)
            && enclosing_function_name(lines, i, &function_names).as_deref() == Some("client_command") {
            issues.push(iss(lineno, "PLUGIN_HANDLED in client_command also starves other plugins' handlers - use PLUGIN_HANDLED_MAIN (amxconst.inc documents this exact case)".into(), "client_command_handled", false));
        }

        if config.enabled("client_connect_actions") && enclosing_function_name(lines, i, &function_names).as_deref() == Some("client_connect") {
            for nat in ["client_print(", "show_menu(", "set_user_", "cs_set_user_", "give_item("] {
                if san.contains(nat) {
                    issues.push(iss(lineno, "client_connect is 'too early to do anything that directly affects the client' (official docs) - move to client_putinserver".into(), "client_connect_actions", false));
                    break;
                }
            }
        }

        // --- unreachable code ---
        if config.enabled("unreachable_code") && !san.contains('}')
            && (san_trim == "return" || (san_trim.starts_with("return") && san_trim[6..].starts_with([' ', ';', '\t']))) {
            let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim()).unwrap_or("");
            let prev_is_branch = (prev.starts_with("if") || prev.starts_with("else") || prev.starts_with("for") || prev.starts_with("while") || prev.starts_with("case") || prev.starts_with("default")) && !prev.contains('{');
            if !prev_is_branch
                && let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty()) {
                let nt = next.trim();
                // a top-level declaration after the return means the return was a
                // braceless function body, not dead code
                let top_level_decl = ["public ", "stock ", "static ", "forward ", "native ", "new ", "enum"]
                    .iter().any(|kw| nt.starts_with(kw));
                if !nt.starts_with('}') && !nt.starts_with("case") && !nt.starts_with("default")
                    && !nt.starts_with('#') && !nt.starts_with("//") && !nt.starts_with("else")
                    && !nt.starts_with('*') && !nt.starts_with('{') && !top_level_decl {
                    issues.push(iss(lineno, "code after an unconditional return never runs (warning 225)".into(), "unreachable_code", false));
                }
            }
        }
    }

    // ---------- file-level post-passes ----------

    if config.enabled("unbalanced_preprocessor") {
        let mut stack: Vec<usize> = Vec::new();
        let mut else_seen: Vec<bool> = Vec::new();
        for (i, (san, _)) in sanitized.iter().enumerate() {
            let Some(caps) = RE_PP_DIRECTIVE.captures(san) else { continue };
            match caps.get(1).unwrap().as_str() {
                "if" | "ifdef" | "ifndef" => { stack.push(i + 1); else_seen.push(false); }
                "else" => {
                    if stack.is_empty() {
                        issues.push(iss(i + 1, "#else without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    } else if let Some(seen) = else_seen.last_mut() {
                        if *seen {
                            issues.push(iss(i + 1, "multiple #else in one #if block (error 060)".into(), "unbalanced_preprocessor", false));
                        }
                        *seen = true;
                    }
                }
                "elseif" => {
                    if stack.is_empty() {
                        issues.push(iss(i + 1, "#elseif without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    } else if else_seen.last() == Some(&true) {
                        issues.push(iss(i + 1, "#elseif after #else (error 061)".into(), "unbalanced_preprocessor", false));
                    }
                }
                "endif" => {
                    if stack.pop().is_none() {
                        issues.push(iss(i + 1, "#endif without an open #if (error 026)".into(), "unbalanced_preprocessor", false));
                    }
                    else_seen.pop();
                }
                _ => {}
            }
        }
        for open_line in stack {
            issues.push(iss(open_line, "#if opened here is never closed with #endif".into(), "unbalanced_preprocessor", false));
        }
    }

    // Brace balance: skip when the file uses #else (branches may intentionally unbalance).
    // Only report when the file ends unbalanced - a transient negative that recovers by
    // EOF means our line model missed something, not that the code is broken.
    if config.enabled("unbalanced_braces") && !sanitized.iter().any(|(s, _)| s.trim_start().starts_with("#else")) {
        let mut depth = 0i32;
        let mut last_open = 0usize;
        let mut first_negative = 0usize;
        for (i, (san, _)) in sanitized.iter().enumerate() {
            for c in san.chars() {
                match c {
                    '{' => { depth += 1; last_open = i + 1; }
                    '}' => {
                        depth -= 1;
                        if depth < 0 && first_negative == 0 { first_negative = i + 1; }
                    }
                    _ => {}
                }
            }
        }
        if depth < 0 {
            issues.push(iss(first_negative.max(1), "unmatched closing brace (error 054); every function below this line will also fail (error 010/004)".into(), "unbalanced_braces", false));
        } else if depth > 0 {
            issues.push(iss(last_open, format!("{} unclosed brace(s) at end of file (error 030: compound statement not closed)", depth), "unbalanced_braces", false));
        }
    }

    if config.enabled("forward_arity") {
        for caps in RE_FWD_ZERO_ARG.captures_iter(raw_clean) {
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            issues.push(iss(lineno, format!("{}() takes no parameters (error 025: heading differs from prototype)", caps.get(1).unwrap().as_str()), "forward_arity", false));
        }
        for caps in RE_FWD_ONE_ARG.captures_iter(raw_clean) {
            let params = caps.get(2).unwrap().as_str();
            let count = if params.trim().is_empty() { 0 } else { params.split(',').count() };
            if count != 1 {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("{}(id) takes exactly 1 parameter, found {} (error 025)", caps.get(1).unwrap().as_str(), count), "forward_arity", false));
            }
        }
    }

    if config.enabled("player_array_32") {
        for (decl_line, name) in &player32 {
            let re_use = Regex::new(&format!(r"\b{}\[\s*(?:id|player)\s*\]", regex::escape(name))).unwrap();
            if re_use.is_match(raw_clean) {
                issues.push(iss(*decl_line, format!("'{}[32]' is indexed by a player id (1..32) - slot 32 overflows on a full server (run time error 4); declare [33] / [MAX_PLAYERS + 1]", name), "player_array_32", false));
            }
        }
    }

    if config.enabled("model_not_precached") {
        let precached: HashSet<&str> = RE_PRECACHE_MODEL_LIT.captures_iter(raw_clean)
            .map(|c| c.get(1).unwrap().as_str()).collect();
        static RE_STOCK_MODEL: LazyLock<Regex> = LazyLock::new(|| {
            // standard game content (w_/v_/p_ weapon models at models/ root) is
            // precached by the engine itself
            Regex::new(r"^models/[wvp]_\w+\.mdl$").unwrap()
        });
        for caps in RE_SET_MODEL_LIT.captures_iter(raw_clean) {
            let model = caps.get(1).unwrap().as_str();
            if model.ends_with(".mdl") && !precached.contains(model) && !RE_STOCK_MODEL.is_match(model) {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("model \"{}\" is set but never precached in this file - fatal 'SV_ModelIndex: model not precached' if no other plugin precaches it", model), "model_not_precached", false));
            }
        }
    }

    if config.enabled("entity_leak") && RE_CREATE_ENT_ANY.is_match(raw_clean) && !RE_REMOVE_ENT_ANY.is_match(raw_clean)
        && let Some(m) = RE_CREATE_ENT_ANY.find(raw_clean) {
            let lineno = raw_clean[..m.start()].matches('\n').count() + 1;
            issues.push(iss(lineno, "entities are created but never removed anywhere in this file - edicts accumulate until fatal 'ED_Alloc: no free edicts'".into(), "entity_leak", false));
        }

    if config.enabled("callback_not_defined") {
        for caps in RE_CALLBACK_STR.captures_iter(raw_clean) {
            let cb = caps.get(2).unwrap().as_str();
            if function_names.iter().any(|f| f == cb) { continue; }
            // RegisterHam/register_event string args can be event/class names, not callbacks;
            // only flag identifiers that look like function names and are truly absent.
            if !raw_sq.contains(&format!("{}(", cb)) {
                let native = caps.get(1).unwrap().as_str();
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(lineno, format!("{} callback \"{}\" has no function definition in this file - plugin fails at load with 'function not found'", native, cb), "callback_not_defined", false));
            }
        }
    }

    if config.enabled("deathmsg_killer_guard") {
        for caps in RE_DEATHMSG_REG.captures_iter(raw_clean) {
            let cb = caps.get(1).unwrap().as_str();
            let body = find_function_body_in(lines, cb);
            if body.is_empty() { continue; }
            if let Some(vcaps) = RE_READ_DATA1.captures(&body) {
                let var = vcaps.get(1).unwrap().as_str().to_string();
                let body_sq = squash(&body);
                let used = body_sq.contains(&format!("[{}]", var))
                    || body_sq.contains(&format!("get_user_name({},", var))
                    || uses_player_native_on(&body, &var);
                let guarded = has_guard(&body, &var)
                    || body_sq.contains(&format!("!{}", var))
                    || body_sq.contains(&format!("if({})", var))
                    || body_sq.contains(&format!("if({}&&", var));
                if used && !guarded {
                    let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                    issues.push(iss(lineno, format!("DeathMsg killer '{}' (read_data(1)) is 0 for fall/acid/world deaths - guard before using it as index/player", var), "deathmsg_killer_guard", false));
                }
            }
        }
    }

    if config.enabled("zp43_mixing") && raw_clean.contains("#include <zombieplague>")
        && (raw_clean.contains("#include <zp50_") || RE_ZP43_NATIVE.is_match(raw_clean).eq(&false))
        && raw_clean.contains("#include <zp50_") {
            let pos = raw_clean.find("#include <zombieplague>").unwrap();
            let lineno = raw_clean[..pos].matches('\n').count() + 1;
            issues.push(iss(lineno, "mixing ZP 4.3 API (<zombieplague>) with zp50 includes fails to load without the compat addon ('missing natives')".into(), "zp43_mixing", false));
        }
    if config.enabled("zp43_mixing") && raw_clean.contains("#include <zp50_")
        && let Some(m) = RE_ZP43_NATIVE.find(raw_clean) {
        let lineno = raw_clean[..m.start()].matches('\n').count() + 1;
        issues.push(iss(lineno, "legacy ZP 4.3 native used alongside zp50 includes - only works with the 4.3 compat addon loaded".into(), "zp43_mixing", false));
    }
}

#[cfg(test)]
mod tests {
    use crate::config::RulesConfig;
    use crate::engine::lint_file;

    fn lint_str(name: &str, content: &str) -> Vec<&'static str> {
        let path = std::env::temp_dir().join(format!("zplint_det_{}_{}.sma", name, std::process::id()));
        std::fs::write(&path, content).unwrap();
        let issues = lint_file(&path, &RulesConfig::default());
        std::fs::remove_file(path).unwrap();
        issues.into_iter().map(|i| i.rule_id).collect()
    }

    #[test]
    fn else_paren_flagged() {
        let r = lint_str("elsep", "public f(item) {\n\tif (item == 0) { a(); }\n\telse (item == 1) { b(); }\n}\n");
        assert!(r.contains(&"else_paren"));
        let ok = lint_str("elseok", "public f(item) {\n\tif (item == 0) { a(); }\n\telse if (item == 1) { b(); }\n}\n");
        assert!(!ok.contains(&"else_paren"));
    }

    #[test]
    fn string_compare_flagged() {
        let r = lint_str("strcmp1", "public f() {\n\tnew name[32];\n\tif (name == \"admin\") return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"string_literal_compare"));
        let ok = lint_str("strcmp2", "public f() {\n\tnew name[32];\n\tif (equal(name, \"admin\")) return 1;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"string_literal_compare"));
        // == "..." inside a string literal must not fire
        let ok2 = lint_str("strcmp3", "public f(id) {\n\tclient_print(id, print_chat, \"x == \\\"y\\\"\");\n}\n");
        assert!(!ok2.contains(&"string_literal_compare"));
    }

    #[test]
    fn set_task_int_interval() {
        let r = lint_str("taskint", "public plugin_init() {\n\tset_task(10, \"tick\");\n}\npublic tick() {}\n");
        assert!(r.contains(&"set_task_int_interval"));
        let ok = lint_str("taskfloat", "public plugin_init() {\n\tset_task(10.0, \"tick\");\n}\npublic tick() {}\n");
        assert!(!ok.contains(&"set_task_int_interval"));
    }

    #[test]
    fn pev_float_int() {
        let r = lint_str("pevint", "public f(id) {\n\tset_pev(id, pev_health, 100)\n}\n");
        assert!(r.contains(&"pev_float_int"));
        let ok = lint_str("pevfloat", "public f(id) {\n\tset_pev(id, pev_health, 100.0)\n}\n");
        assert!(!ok.contains(&"pev_float_int"));
    }

    #[test]
    fn int_native_float() {
        let r = lint_str("intfl", "public f(id) {\n\tset_user_health(id, 100.0)\n}\n");
        assert!(r.contains(&"int_native_float"));
        let ok = lint_str("intok", "public f(id) {\n\tset_user_health(id, 100)\n}\n");
        assert!(!ok.contains(&"int_native_float"));
    }

    #[test]
    fn userid_index() {
        let r = lint_str("userid", "new g_x[33];\npublic f(id) {\n\tg_x[get_user_userid(id)]++;\n}\n");
        assert!(r.contains(&"userid_as_index"));
    }

    #[test]
    fn find_ent_no_advance() {
        let r = lint_str("fent", "public f() {\n\tnew ent;\n\twhile ((ent = find_ent_by_class(-1, \"x\"))) {\n\t\tremove_entity(ent);\n\t}\n}\n");
        assert!(r.contains(&"find_ent_no_advance"));
        let ok = lint_str("fentok", "public f() {\n\tnew ent = -1;\n\twhile ((ent = find_ent_by_class(ent, \"x\")) > 0) {\n\t\tremove_entity(ent);\n\t}\n}\n");
        assert!(!ok.contains(&"find_ent_no_advance"));
    }

    #[test]
    fn precache_mp3_and_prefix() {
        let r = lint_str("mp3", "public plugin_precache() {\n\tprecache_sound(\"music/theme.mp3\")\n}\n");
        assert!(r.contains(&"precache_mp3"));
        let r2 = lint_str("sndpre", "public plugin_precache() {\n\tprecache_sound(\"sound/zombie/pain.wav\")\n}\n");
        assert!(r2.contains(&"sound_prefix"));
        let ok = lint_str("sndok", "public plugin_precache() {\n\tprecache_sound(\"zombie/pain.wav\")\n}\n");
        assert!(!ok.contains(&"sound_prefix") && !ok.contains(&"precache_mp3"));
    }

    #[test]
    fn te_reliable() {
        let r = lint_str("terel", "public f(id) {\n\tmessage_begin(MSG_ONE, SVC_TEMPENTITY, {0,0,0}, id)\n\tmessage_end()\n}\n");
        assert!(r.contains(&"te_reliable"));
        let ok = lint_str("terelok", "public f() {\n\tmessage_begin(MSG_BROADCAST, SVC_TEMPENTITY)\n\tmessage_end()\n}\n");
        assert!(!ok.contains(&"te_reliable"));
    }

    #[test]
    fn assignment_in_condition_rule() {
        let r = lint_str("asgn", "public f(x) {\n\tif (x = 1) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"assignment_in_condition"));
        let ok = lint_str("asgnok", "public f(x) {\n\tif (x == 1) return 1;\n\tif ((x = other())) return 2;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"assignment_in_condition"));
        let ok2 = lint_str("asgnok2", "public f(x) {\n\tif (x >= 1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"assignment_in_condition"));
    }

    #[test]
    fn self_assignment_rule() {
        let r = lint_str("selfa", "public f(id) {\n\tg_class[id] = g_class[id];\n}\n");
        assert!(r.contains(&"self_assignment"));
        let ok = lint_str("selfok", "public f(id) {\n\tg_class[id] = g_next[id];\n}\n");
        assert!(!ok.contains(&"self_assignment"));
    }

    #[test]
    fn comparison_as_statement_rule() {
        let r = lint_str("cmpst", "public f() {\n\tg_mode == 5;\n}\n");
        assert!(r.contains(&"comparison_as_statement"));
        let ok = lint_str("cmpok", "public f() {\n\tif (g_mode == 5) return;\n}\n");
        assert!(!ok.contains(&"comparison_as_statement"));
    }

    #[test]
    fn contain_and_strcmp_truthy() {
        let r = lint_str("cont", "public f() {\n\tnew msg[64];\n\tif (contain(msg, \"admin\")) return 1;\n\treturn 0;\n}\n");
        assert!(r.contains(&"contain_truthy"));
        let ok = lint_str("contok", "public f() {\n\tnew msg[64];\n\tif (contain(msg, \"admin\") != -1) return 1;\n\treturn 0;\n}\n");
        assert!(!ok.contains(&"contain_truthy"));
        let r2 = lint_str("strc", "public f() {\n\tnew a[8], b[8];\n\tif (strcmp(a, b)) return 1;\n\treturn 0;\n}\n");
        assert!(r2.contains(&"strcmp_truthy"));
        let ok2 = lint_str("strcok", "public f() {\n\tnew a[8], b[8];\n\tif (strcmp(a, b) == 0) return 1;\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"strcmp_truthy"));
    }

    #[test]
    fn formatex_self_rule() {
        let r = lint_str("fmx", "public f() {\n\tnew buf[64];\n\tformatex(buf, charsmax(buf), \"prefix %s\", buf);\n}\n");
        assert!(r.contains(&"formatex_self"));
        let ok = lint_str("fmxok", "public f() {\n\tnew buf[64], src[64];\n\tformatex(buf, charsmax(buf), \"prefix %s\", src);\n}\n");
        assert!(!ok.contains(&"formatex_self"));
    }

    #[test]
    fn unbalanced_preprocessor_rule() {
        let r = lint_str("ppbad", "#if defined X\nnew g_a;\npublic plugin_init() {\n}\n");
        assert!(r.contains(&"unbalanced_preprocessor"));
        let ok = lint_str("ppok", "#if defined X\nnew g_a;\n#endif\npublic plugin_init() {\n}\n");
        assert!(!ok.contains(&"unbalanced_preprocessor"));
    }

    #[test]
    fn unbalanced_braces_rule() {
        let r = lint_str("brbad", "public plugin_init() {\n\tregister_plugin(\"x\", \"1\", \"a\");\n\npublic other() {\n}\n");
        assert!(r.contains(&"unbalanced_braces"));
        let ok = lint_str("brok", "public plugin_init() {\n\tregister_plugin(\"x\", \"1\", \"a\");\n}\n");
        assert!(!ok.contains(&"unbalanced_braces"));
        // braces inside strings/chars must not count
        let ok2 = lint_str("brstr", "public f() {\n\tnew c = '{';\n\tclient_print(0, print_chat, \"{ %d }\", c);\n}\n");
        assert!(!ok2.contains(&"unbalanced_braces"));
    }

    #[test]
    fn unterminated_string_rule() {
        let r = lint_str("unterm", "public f(id) {\n\tclient_print(id, print_chat, \"Welcome!);\n}\n");
        assert!(r.contains(&"unterminated_string"));
        let ok = lint_str("untermok", "public f(id) {\n\tclient_print(id, print_chat, \"Welcome ^\"quoted^\"!\");\n}\n");
        assert!(!ok.contains(&"unterminated_string"));
    }

    #[test]
    fn player_array_32_rule() {
        let r = lint_str("p32", "new g_hp[32];\npublic f(id) {\n\tg_hp[id] = 100;\n}\n");
        assert!(r.contains(&"player_array_32"));
        let ok = lint_str("p33", "new g_hp[33];\npublic f(id) {\n\tg_hp[id] = 100;\n}\n");
        assert!(!ok.contains(&"player_array_32"));
        let ok2 = lint_str("p32i", "new g_slots[32];\npublic f() {\n\tfor (new i = 0; i < 32; i++) g_slots[i] = 0;\n}\n");
        assert!(!ok2.contains(&"player_array_32"));
    }

    #[test]
    fn forward_arity_rule() {
        let r = lint_str("arity", "public plugin_init(id) {\n}\n");
        assert!(r.contains(&"forward_arity"));
        let r2 = lint_str("arity2", "public client_putinserver(id, extra) {\n}\n");
        assert!(r2.contains(&"forward_arity"));
        let ok = lint_str("arityok", "public plugin_init() {\n}\npublic client_putinserver(id) {\n}\n");
        assert!(!ok.contains(&"forward_arity"));
    }

    #[test]
    fn stacked_case_rule() {
        let r = lint_str("scase", "public f(w) {\n\tswitch (w) {\n\t\tcase 1:\n\t\tcase 2: g();\n\t}\n}\n");
        assert!(r.contains(&"stacked_case"));
        let ok = lint_str("scaseok", "public f(w) {\n\tswitch (w) {\n\t\tcase 1, 2: g();\n\t}\n}\n");
        assert!(!ok.contains(&"stacked_case"));
    }

    #[test]
    fn global_shadowing_rule() {
        let r = lint_str("shadow", "new g_count;\npublic f() {\n\tnew g_count = 1;\n\tg_count++;\n}\n");
        assert!(r.contains(&"global_shadowing"));
        let ok = lint_str("shadowok", "new g_count;\npublic f() {\n\tnew local = 1;\n\tg_count += local;\n}\n");
        assert!(!ok.contains(&"global_shadowing"));
    }

    #[test]
    fn loops_context_rules() {
        let r = lint_str("bufloop", "public f() {\n\tfor (new i = 0; i < 32; i++) {\n\t\tnew name[64];\n\t\tget_user_name(i, name, charsmax(name));\n\t}\n}\n");
        assert!(r.contains(&"buffer_in_loop"));
        let r2 = lint_str("rfloop", "public f() {\n\tnew buf[128], len;\n\tfor (new i = 0; i < 10; i++) {\n\t\tread_file(\"x.txt\", i, buf, charsmax(buf), len);\n\t}\n}\n");
        assert!(r2.contains(&"read_file_loop"));
        let ok = lint_str("bufok", "public f() {\n\tnew name[64];\n\tfor (new i = 0; i < 32; i++) {\n\t\tget_user_name(i, name, charsmax(name));\n\t}\n}\n");
        assert!(!ok.contains(&"buffer_in_loop"));
    }

    #[test]
    fn get_cvar_hotpath_rule() {
        let r = lint_str("cvarhot", "public plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tif (get_cvar_num(\"zp_on\")) return;\n}\n");
        assert!(r.contains(&"get_cvar_hotpath"));
        let ok = lint_str("cvarok", "public plugin_init() {\n\tnew v = get_cvar_num(\"zp_on\");\n}\n");
        assert!(!ok.contains(&"get_cvar_hotpath"));
    }

    #[test]
    fn deathmsg_killer_rule() {
        let r = lint_str("dmsg", "new g_kills[33];\npublic plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tnew killer = read_data(1);\n\tg_kills[killer]++;\n}\n");
        assert!(r.contains(&"deathmsg_killer_guard"));
        let ok = lint_str("dmsgok", "new g_kills[33];\npublic plugin_init() {\n\tregister_event(\"DeathMsg\", \"ev_death\", \"a\");\n}\npublic ev_death() {\n\tnew killer = read_data(1);\n\tif (!is_user_connected(killer)) return;\n\tg_kills[killer]++;\n}\n");
        assert!(!ok.contains(&"deathmsg_killer_guard"));
    }

    #[test]
    fn zp50_rules() {
        let r = lint_str("zpreg", "public plugin_precache() {\n\tzp_items_register(\"Trip Mine\", 20);\n}\n");
        assert!(r.contains(&"zp50_register_return"));
        let ok = lint_str("zpregok", "new g_item;\npublic plugin_precache() {\n\tg_item = zp_items_register(\"Trip Mine\", 20);\n}\n");
        assert!(!ok.contains(&"zp50_register_return"));

        let r2 = lint_str("zpatt", "public zp_fw_core_infect(id, attacker) {\n\tzp_ammopacks_set(attacker, zp_ammopacks_get(attacker) + 5);\n}\n");
        assert!(r2.contains(&"zp_fw_attacker_guard"));
        let ok2 = lint_str("zpattok", "public zp_fw_core_infect(id, attacker) {\n\tif (!attacker || !is_user_connected(attacker)) return;\n\tzp_ammopacks_set(attacker, zp_ammopacks_get(attacker) + 5);\n}\n");
        assert!(!ok2.contains(&"zp_fw_attacker_guard"));

        let r3 = lint_str("zpsel", "public zp_fw_items_select_pre(id, itemid, ignorecost) {\n\tif (zp_core_is_zombie(id))\n\t\treturn ZP_ITEM_DONT_SHOW;\n\treturn ZP_ITEM_AVAILABLE;\n}\n");
        assert!(r3.contains(&"zp_select_pre_filter"));
        let ok3 = lint_str("zpselok", "new g_item;\npublic zp_fw_items_select_pre(id, itemid, ignorecost) {\n\tif (itemid != g_item)\n\t\treturn ZP_ITEM_AVAILABLE;\n\tif (zp_core_is_zombie(id))\n\t\treturn ZP_ITEM_DONT_SHOW;\n\treturn ZP_ITEM_AVAILABLE;\n}\n");
        assert!(!ok3.contains(&"zp_select_pre_filter"));
    }

    #[test]
    fn unreachable_code_rule() {
        let r = lint_str("unreach", "public f(id) {\n\treturn PLUGIN_HANDLED;\n\tclient_print(id, print_chat, \"x\");\n}\n");
        assert!(r.contains(&"unreachable_code"));
        let ok = lint_str("unreachok", "public f(id) {\n\tif (id)\n\t\treturn PLUGIN_HANDLED;\n\tclient_print(id, print_chat, \"x\");\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(!ok.contains(&"unreachable_code"));
        let ok2 = lint_str("unreach2", "public f(id) {\n\tswitch (id) {\n\t\tcase 1: return 1;\n\t\tcase 2: return 2;\n\t}\n\treturn 0;\n}\n");
        assert!(!ok2.contains(&"unreachable_code"));
    }

    #[test]
    fn format_injection_rule() {
        let r = lint_str("fmtinj", "public f(id) {\n\tnew said[192];\n\tread_args(said, charsmax(said));\n\tclient_print(0, print_chat, said);\n}\n");
        assert!(r.contains(&"format_injection"));
        let ok = lint_str("fmtok", "public f(id) {\n\tnew said[192];\n\tread_args(said, charsmax(said));\n\tclient_print(0, print_chat, \"%s\", said);\n}\n");
        assert!(!ok.contains(&"format_injection"));
    }

    #[test]
    fn empty_statement_and_dowhile() {
        let r = lint_str("empt", "public f(id) {\n\tif (is_user_alive(id));\n\t\tuser_kill(id);\n}\n");
        assert!(r.contains(&"empty_statement"));
        let ok = lint_str("dowhile", "public f() {\n\tnew i = 0;\n\tdo {\n\t\ti++;\n\t}\n\twhile (i < 3);\n}\n");
        assert!(!ok.contains(&"empty_statement"));
    }

    #[test]
    fn client_command_handled_rule() {
        let r = lint_str("cch", "public client_command(id) {\n\tif (id) {\n\t\treturn PLUGIN_HANDLED;\n\t}\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(r.contains(&"client_command_handled"));
        let ok = lint_str("cchok", "public client_command(id) {\n\tif (id) {\n\t\treturn PLUGIN_HANDLED_MAIN;\n\t}\n\treturn PLUGIN_CONTINUE;\n}\n");
        assert!(!ok.contains(&"client_command_handled"));
    }

    #[test]
    fn model_not_precached_rule() {
        let r = lint_str("mdl", "public fw_spawn(ent) {\n\tentity_set_model(ent, \"models/custom/crate.mdl\")\n}\n");
        assert!(r.contains(&"model_not_precached"));
        let ok = lint_str("mdlok", "public plugin_precache() {\n\tprecache_model(\"models/custom/crate.mdl\")\n}\npublic fw_spawn(ent) {\n\tentity_set_model(ent, \"models/custom/crate.mdl\")\n}\n");
        assert!(!ok.contains(&"model_not_precached"));
    }

    #[test]
    fn entity_leak_rule() {
        let r = lint_str("leak", "public fw_kill(id) {\n\tnew ent = create_entity(\"info_target\");\n\tif (!ent) return;\n}\n");
        assert!(r.contains(&"entity_leak"));
        let ok = lint_str("leakok", "public fw_kill(id) {\n\tnew ent = create_entity(\"info_target\");\n\tif (!ent) return;\n\tremove_entity(ent);\n}\n");
        assert!(!ok.contains(&"entity_leak"));
    }

    #[test]
    fn callback_not_defined_rule() {
        let r = lint_str("cbnd", "public plugin_init() {\n\tregister_clcmd(\"say /vip\", \"cmd_vip\");\n}\npublic cmdVip(id) {\n\treturn PLUGIN_HANDLED;\n}\n");
        assert!(r.contains(&"callback_not_defined"));
        let ok = lint_str("cbndok", "public plugin_init() {\n\tregister_clcmd(\"say /vip\", \"cmd_vip\");\n}\npublic cmd_vip(id) {\n\treturn PLUGIN_HANDLED;\n}\n");
        assert!(!ok.contains(&"callback_not_defined"));
    }

    #[test]
    fn deprecated_and_define_rules() {
        let r = lint_str("depr", "public f() {\n\tnew hash[34];\n\tmd5(\"x\", hash);\n}\n");
        assert!(r.contains(&"deprecated_symbols"));
        let r2 = lint_str("defres", "#define MAX_PLAYERS 32\nnew g_hp[MAX_PLAYERS + 1];\n");
        assert!(r2.contains(&"define_reserved_const"));
    }

    #[test]
    fn string_assign_rule() {
        let r = lint_str("strassign", "public f() {\n\tnew msg[8];\n\tmsg = \"Hello World!\";\n}\n");
        assert!(r.contains(&"string_assign"));
        let ok = lint_str("strassignok", "public f() {\n\tnew msg[16];\n\tcopy(msg, charsmax(msg), \"Hello World!\");\n}\n");
        assert!(!ok.contains(&"string_assign"));
        // a literal that fits is legal Pawn
        let ok3 = lint_str("strassignfits", "public f() {\n\tnew msg[16];\n\tmsg = \"Hello\";\n}\n");
        assert!(!ok3.contains(&"string_assign"));
        let ok2 = lint_str("strassigndecl", "new g_prefix[] = \"[ZP]\";\npublic f() {\n}\n");
        assert!(!ok2.contains(&"string_assign"));
    }

    #[test]
    fn div_by_runtime_rule() {
        let r = lint_str("div", "public f(total) {\n\tnew share = total / get_playersnum();\n\treturn share;\n}\n");
        assert!(r.contains(&"div_by_runtime"));
        // '%' and '/' inside strings must not fire
        let ok = lint_str("divok", "public f(id) {\n\tclient_print(id, print_chat, \"hp: %d / max\", 100);\n}\n");
        assert!(!ok.contains(&"div_by_runtime"));
    }
}
