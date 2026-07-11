use crate::config::RulesConfig;
use crate::rules::*;
use regex::Regex;
use std::sync::LazyLock;

static RE_MSG_ONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"message_begin\(\s*MSG_ONE[^,]*,\s*[^,]+,\s*[^,]+,\s*(\w+)\s*\)").unwrap());
static RE_EMIT_SOUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"emit_sound\([^,]+,\s*[^,]+,\s*"([^"]+)"#).unwrap());
static RE_FIND_SPHERE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"FindEntityInSphere[^,]+,\s*(\w+)").unwrap());
static RE_LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"for\s*\(\s*new\s+(\w+)\s*=\s*1\s*;\s*\w+\s*<=?\s*(32|33|get_maxplayers)").unwrap());
static RE_INFECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"zp_core_(infect|cure)\s*\(\s*(\w+)").unwrap());
static RE_GAMEMODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"new\s+(\w+)\s*=\s*zp_gamemodes_get_current\s*\(").unwrap());
static RE_CLASS_Z: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"new\s+(\w+)\s*=\s*zp_class_zombie_get_current\s*\(").unwrap());
static RE_CLASS_H: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"new\s+(\w+)\s*=\s*zp_class_human_get_current\s*\(").unwrap());
static RE_PRECACHE_SPR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(g_\w+)\s*=\s*precache_sound\(").unwrap());
static RE_SPRITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(Spr|Beam|Glow|Smoke|Shock|Explosion|Model)").unwrap());
static RE_CREATE_ENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:new\s+)?(\w+)\s*=\s*create_entity\s*\(").unwrap());
static RE_BUFFER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(get_user_name|get_user_authid|get_user_ip|get_user_team)\s*\([^,]+,\s*(\w+),\s*(\d+)").unwrap());
static RE_SET_TASK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(set_task|register_impulse)\s*\([^;]*?"([^"]+)"#).unwrap());
static RE_REG_EVENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"register_event\s*\([^,]+,\s*"(\w+)""#).unwrap());
static RE_REG_OTHER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(set_task|RegisterHam|register_forward)\s*\([^;]*?"(\w+)"#).unwrap());
static RE_ITEMS_REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(g_\w+)\s*=\s*zp_items_register\s*\(").unwrap());
static RE_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)/\*.*?\*/").unwrap());
static RE_IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_]\w*$").unwrap());
// New rules
static RE_TAKE_DAMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(Ham_TakeDamage|fw_TakeDamage|fw_Takedamage)\s*\(").unwrap());
static RE_GET_USER_ORIGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bget_user_origin\s*\(").unwrap());
static RE_TASK_ZERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bset_task\s*\(\s*(0[\.\d]*|0)\s*,?\s*"#).unwrap());
static RE_ABORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\babort\s*\(").unwrap());
static RE_MSG_BEGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bmessage_begin\s*\(").unwrap());
static RE_MSG_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bmessage_end\s*\(").unwrap());
static _RE_MAXPLAYERS_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#define\s+MAXPLAYERS\s+32").unwrap());
static RE_PRECACHE_ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(precache_model|precache_sound|precache_generic)\s*\(").unwrap());
static RE_PLUGIN_INIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"public\s+plugin_init\s*\(").unwrap());
static RE_ZP_FORCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"zp_core_force_(infect|cure)\s*\(\s*(\w+)").unwrap());
static RE_CLASS_REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bzp_class_(zombie|human)_register\s*\(").unwrap());
static RE_LIBRARY_EXISTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bLibraryExists\s*\(").unwrap());
static RE_PLUGIN_PRECACHE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"public\s+plugin_precache\s*\(").unwrap());

static FWD_RE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    DANGEROUS_FORWARDS.iter().map(|(fwd, _)| Regex::new(&format!(r"\b{}\s*\(", fwd)).unwrap()).collect()
});

pub fn lint_file(filepath: &std::path::Path, config: &RulesConfig) -> Vec<LintIssue> {
    let raw = match std::fs::read_to_string(filepath) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let raw_clean = strip_block_comments(&raw);
    let lines_clean: Vec<&str> = raw_clean.split('\n').collect();
    let mut issues = Vec::new();
    // Track message_begin nesting for rule: nested_message
    let mut _msg_begin_lineno: usize = 0;
    let mut msg_nesting = 0i32;

    // Find functions for scope-aware new rules
    let has_precache_func = RE_PLUGIN_PRECACHE.is_match(&raw_clean);
    let has_init_func = RE_PLUGIN_INIT.is_match(&raw_clean);
    // Find the line range of plugin_init body
    let mut _init_start: usize = 0;
    let mut _init_end: usize = 0;
    if has_init_func {
        for (i, ln) in lines_clean.iter().enumerate() {
            if RE_PLUGIN_INIT.is_match(ln) {
                _init_start = i;
                let body = enclosing_body(&lines_clean, i);
                _init_end = _init_start + body.matches('\n').count();
                break;
            }
        }
    }

    for (i, line) in lines_clean.iter().enumerate() {
        let lineno = i + 1;
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('*') { continue; }

        // --- Existing 17 rules ---
        if config.client_disconnect_guard && stripped.contains("client_disconnect") {
            let body = enclosing_body(&lines_clean, i);
            if let Some(nat) = RISKY_NATIVES.iter().find(|n| body.contains(**n)) {
                if !body.contains("is_user_connected") {
                    issues.push(iss(lineno, format!("client_disconnected uses {} without is_user_connected guard (always crashes)", nat), "client_disconnect_guard", false));
                }
            }
        }

        if config.dangerous_forward_guard {
            for (idx, (fwd, param)) in DANGEROUS_FORWARDS.iter().enumerate() {
                if !FWD_RE[idx].is_match(stripped) { continue; }
                let body = enclosing_body(&lines_clean, i);
                if has_guard(&body, param) { continue; }
                if let Some(nat) = RISKY_NATIVES.iter().find(|n| body.contains(&format!("{}({}", n, param))) {
                    issues.push(iss(lineno, format!("{} calls {} on '{}' without is_user_connected/valid guard", fwd, nat, param), "dangerous_forward_guard", false));
                }
            }
        }

        if config.message_begin_guard && stripped.contains("MSG_ONE") && stripped.contains("message_begin") {
            if let Some(var) = RE_MSG_ONE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                if var != "0" && var != "1" && !var.chars().all(|c| c.is_ascii_digit()) {
                    if !has_guard(&enclosing_body(&lines_clean, i), &var) {
                        issues.push(iss(lineno, format!("message_begin(MSG_ONE,..,{}) without 1-32/is_user_* guard (may be non-player entity -> svc_bad)", var), "message_begin_guard", false));
                    }
                }
            }
        }

        if config.touch_spam && (stripped.contains("Ham_Touch") || stripped.contains("fw_Touch")) {
            let body = enclosing_body(&lines_clean, i);
            if body.matches("client_print(").count() > 1 && !body.contains("set_pev") && !body.to_lowercase().contains("task") {
                issues.push(iss(lineno, "Touch handler prints multiple times without a cooldown (spam)".into(), "touch_spam", false));
            }
        }

        if config.precache_sound && stripped.contains("emit_sound") {
            if let Some(caps) = RE_EMIT_SOUND.captures(stripped) {
                let snd = caps.get(1).unwrap().as_str();
                let top = snd.split('/').next().unwrap_or("").to_lowercase();
                if !STOCK_SOUND_DIRS.contains(&top.as_str()) && !raw_clean.contains("precache_sound") {
                    issues.push(iss(lineno, format!("emit_sound(\"{}\") custom sound with no precache_sound in file", snd), "precache_sound", false));
                }
            }
        }

        if config.find_entity_in_sphere && stripped.contains("FindEntityInSphere") {
            if let Some(var) = RE_FIND_SPHERE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                let end = (i + 8).min(lines_clean.len());
                let after = lines_clean[i..end].join("\n");
                if after.contains(&format!("set_user_({}", var)) && !has_guard(&after, &var) {
                    issues.push(iss(lineno, format!("FindEntityInSphere result '{}' used as player without 1-32 guard", var), "find_entity_in_sphere", false));
                }
            }
        }

        if config.loop_player_guard {
            if let Some(var) = RE_LOOP.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                let end = (i + 40).min(lines_clean.len());
                let mut depth = 0i32; let mut started = false; let mut body = Vec::new();
                for j in i..end {
                    body.push(lines_clean[j]);
                    depth += (lines_clean[j].matches('{').count() as i32) - (lines_clean[j].matches('}').count() as i32);
                    if lines_clean[j].contains('{') { started = true; }
                    if started && depth <= 0 { break; }
                }
                let body = body.join("\n");
                if (body.contains(&format!("set_user_({}", var)) || body.contains(&format!("cs_set_({}", var)))
                    && !has_guard(&body, &var) {
                    issues.push(iss(lineno, format!("loop 1-32 uses player natives on '{}' without is_user_connected/alive guard", var), "loop_player_guard", false));
                }
            }
        }

        if config.zp_infect_cure_guard && (stripped.contains("zp_core_infect") || stripped.contains("zp_core_cure")) {
            if let Some(caps) = RE_INFECT.captures(stripped) {
                let var = caps.get(2).unwrap().as_str().to_string();
                let body = enclosing_body(&lines_clean, i);
                if !body.contains(&format!("zp_core_is_zombie({}", var)) {
                    issues.push(iss(lineno, format!("zp_core_{}('{}') without checking if already infected/cured first (run time error 10)", caps.get(1).unwrap().as_str(), var), "zp_infect_cure_guard", false));
                }
            }
        }

        if config.zp_gamemode_if && stripped.contains("zp_gamemodes_get_current") {
            if let Some(var) = RE_GAMEMODE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                let body = enclosing_body(&lines_clean, i);
                if body.contains(&format!("if ({})", var)) && !body.contains(&format!("if ({} > 0)", var)) {
                    issues.push(iss(lineno, format!("if ({}) should be if ({} > 0) - gamemode can return -2 (ZP_NO_GAME_MODE)", var, var), "zp_gamemode_if", true));
                }
            }
        }

        if config.zp_class_if {
            for re_fn in [&*RE_CLASS_Z, &*RE_CLASS_H] {
                if let Some(var) = re_fn.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                    let body = enclosing_body(&lines_clean, i);
                    if body.contains(&format!("if ({})", var)) && !body.contains(&format!("if ({} > 0)", var)) {
                        issues.push(iss(lineno, format!("if ({}) should be if ({} > 0) - class ID can return -1 (ZP_NO_CLASS)", var, var), "zp_class_if", true));
                    }
                }
            }
        }

        if config.pev_oldbuttons && stripped.contains("pev_oldbuttons") {
            issues.push(iss(lineno, "pev_oldbuttons used (unreliable in PreThink, use manual pev_button tracking instead)".into(), "pev_oldbuttons", false));
        }

        if config.precache_sound_sprite && stripped.contains("precache_sound(") {
            if let Some(varname) = RE_PRECACHE_SPR.captures(stripped).map(|c| c.get(1).unwrap().as_str()) {
                if RE_SPRITE.is_match(varname) {
                    issues.push(iss(lineno, format!("precache_sound assigned to '{}' (variable will be 0/1 (bool) not a sprite handle; use precache_model instead)", varname), "precache_sound_sprite", false));
                }
            }
        }

        if config.create_entity_guard && stripped.contains("create_entity(") {
            if let Some(var) = RE_CREATE_ENT.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                let body = enclosing_body(&lines_clean, i);
                if !body.contains(&format!("is_valid_ent({}", var)) && !body.contains(&format!("!{}", var)) {
                    issues.push(iss(lineno, format!("create_entity result '{}' used without is_valid_ent check", var), "create_entity_guard", false));
                }
            }
        }

        if config.buffer_size {
            if let Some(caps) = RE_BUFFER.captures(stripped) {
                let bufsize: usize = caps.get(3).unwrap().as_str().parse().unwrap_or(64);
                if bufsize < 64 {
                    issues.push(iss(lineno, format!("{} uses hardcoded buffer size {} (prefer charsmax({}) over hardcoded {})", caps.get(1).unwrap().as_str(), bufsize, caps.get(2).unwrap().as_str(), bufsize), "buffer_size", true));
                }
            }
        }

        if config.client_cmd_spk && stripped.contains("client_cmd(0,") && stripped.contains("\"spk") {
            issues.push(iss(lineno, "use emit_sound() instead of client_cmd(0, 'spk...')".into(), "client_cmd_spk", false));
        }

        // --- NEW RULES ---

        // 18. attacker_not_validated - fw_TakeDamage/Ham_TakeDamage handlers using attacker without guard
        if config.attacker_not_validated && RE_TAKE_DAMAGE.is_match(stripped) {
            let body = enclosing_body(&lines_clean, i);
            let has_user_alive_check = body.contains("is_user_alive(attacker)")
                || body.contains("is_user_connected(attacker)")
                || body.contains("is_user_alive ( attacker )")
                || body.contains("!attacker");
            if !has_user_alive_check && (body.contains("attacker") || body.contains("g_attacker")) {
                // Only flag if attacker is actually used in a risky context
                if body.contains("set_user_") || body.contains("cs_set_") || body.contains("zp_") {
                    issues.push(iss(lineno, "TakeDamage handler uses 'attacker' without is_user_alive guard (attacker can be 0/world)".into(), "attacker_not_validated", false));
                }
            }
        }

        // 19. get_user_origin - loses float precision
        if config.get_user_origin && RE_GET_USER_ORIGIN.is_match(stripped) {
            issues.push(iss(lineno, "get_user_origin() loses float precision - use pev(id, pev_origin) instead".into(), "get_user_origin", false));
        }

        // 20. task_interval_zero - set_task(0.0 or 0,
        if config.task_interval_zero {
            if let Some(caps) = RE_TASK_ZERO.captures(stripped) {
                let interval = caps.get(1).unwrap().as_str();
                if interval.starts_with('0') {
                    issues.push(iss(lineno, "set_task with interval 0/0.0 is invalid (minimum 0.1)".into(), "task_interval_zero", false));
                }
            }
        }

        // 21. abort_call - abort( usage
        if config.abort_call && RE_ABORT.is_match(stripped) {
            issues.push(iss(lineno, "abort() causes run time error 1 - use log_error() for graceful degradation".into(), "abort_call", false));
        }

        // 22. nested_message - message_begin before message_end
        if config.nested_message {
            if RE_MSG_BEGIN.is_match(stripped) {
                if msg_nesting > 0 {
                    issues.push(iss(lineno, "nested message_begin() without closing previous message_end() (will crash server)".into(), "nested_message", false));
                }
                msg_nesting += 1;
                _msg_begin_lineno = lineno;
            }
            if RE_MSG_END.is_match(stripped) {
                msg_nesting = 0i32.max(msg_nesting - 1);
            }
        }

        // 23. hardcoded_maxplayers - #define MAXPLAYERS 32
        if config.hardcoded_maxplayers && stripped.contains("#define") && stripped.contains("MAXPLAYERS") && stripped.contains("32") {
            issues.push(iss(lineno, "hardcoded #define MAXPLAYERS 32 - use get_maxplayers() at runtime for flexibility".into(), "hardcoded_maxplayers", false));
        }

        // 24. precache_outside_precache - precache_* outside plugin_precache()
        if config.precache_outside_precache && RE_PRECACHE_ANY.is_match(stripped) {
            // Check if we're inside plugin_precache() or plugin_init()
            let body = enclosing_body(&lines_clean, i);
            let is_in_precache = body.contains("plugin_precache");
            let is_in_init = body.contains("plugin_init");
            if !is_in_precache && is_in_init {
                issues.push(iss(lineno, "precache_*() called in plugin_init() instead of plugin_precache() (will crash server)".into(), "precache_outside_precache", false));
            } else if !is_in_precache && !is_in_init && has_init_func {
                // Outside both - check if plugin_precache() exists at all
                if !has_precache_func {
                    issues.push(iss(lineno, "precache_*() called but no plugin_precache() function found (will crash server)".into(), "precache_outside_precache", false));
                }
            }
        }

        // 25. zp_class_register outside plugin_precache()
        if config.zp_class_if && RE_CLASS_REG.is_match(stripped) && has_precache_func && has_init_func {
            let body = enclosing_body(&lines_clean, i);
            if body.contains("plugin_init") && !body.contains("plugin_precache") {
                issues.push(iss(lineno, "zp_class_*_register() called in plugin_init() instead of plugin_precache() (will crash server)".into(), "zp_class_in_init", false));
            }
        }
    }

    // Post-pass: 26. zp_core_force_infect/cure without guard (can't detect per-line due to scope)
    if config.zp_force_no_guard {
        for caps in RE_ZP_FORCE.captures_iter(&raw_clean) {
            let var = caps.get(2).unwrap().as_str().to_string();
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            let body_enclosing = find_enclosing_fn(&lines_clean, &raw_clean, caps.get(0).unwrap().start());
            if !body_enclosing.contains(&format!("zp_core_is_zombie({}", var)) {
                issues.push(iss(lineno, format!("zp_core_force_{}('{}') without zp_core_is_zombie check first (bypasses validation)", caps.get(1).unwrap().as_str(), var), "zp_force_no_guard", false));
            }
        }
    }

    // Post-pass: 27. LibraryExists in damage handlers
    if config.library_exists_hotpath {
        for caps in RE_TAKE_DAMAGE.captures_iter(&raw_clean) {
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            let body = enclosing_body_from_pos(&lines_clean, lineno - 1);
            if RE_LIBRARY_EXISTS.is_match(&body) {
                issues.push(iss(lineno, "LibraryExists() called in TakeDamage (per-hit) - cache as global boolean instead".into(), "library_exists_hotpath", false));
            }
        }
    }

    // --- Existing post-pass rules ---
    if config.set_task_public {
        let publics = find_publics(&raw_clean);
        let nonpublics = find_nonpublics(&raw_clean, &publics);
        for caps in RE_SET_TASK.captures_iter(&raw_clean) {
            let cb = caps.get(2).unwrap().as_str().to_string();
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            if cb.contains(' ') || cb.contains("//") || cb.contains("/*") {
                issues.push(iss(lineno, format!("{} callback \"{}\" is malformed (embedded space/comment breaks the name)", caps.get(1).unwrap().as_str(), cb), "set_task_public", false));
            } else if nonpublics.contains(&cb) {
                issues.push(iss(lineno, format!("{} callback \"{}\" is a non-public function (must be public or it fails at runtime)", caps.get(1).unwrap().as_str(), cb), "set_task_public", false));
            }
        }
    }

    if config.read_data_multi_context {
        let event_cbs: Vec<String> = RE_REG_EVENT.captures_iter(&raw_clean).map(|c| c.get(1).unwrap().as_str().to_string()).collect();
        let mut other_regs: Vec<(String, usize, String)> = Vec::new();
        for caps in RE_REG_OTHER.captures_iter(&raw_clean) {
            let cb = caps.get(2).unwrap().as_str().to_string();
            if RE_IDENT.is_match(&cb) {
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                other_regs.push((cb, lineno, caps.get(1).unwrap().as_str().to_string()));
            }
        }
        for cb in &event_cbs {
            if let Some((_, lineno, reg)) = other_regs.iter().find(|(c, _, _)| c == cb) {
                if find_function_body_in(&lines_clean, cb).contains("read_data(") {
                    issues.push(iss(*lineno, format!("read_data() in '{}' but also registered as non-event callback via {} (may read stale data)", cb, reg), "read_data_multi_context", false));
                }
            }
        }
    }

    if config.zp_items_register_check {
        for caps in RE_ITEMS_REG.captures_iter(&raw_clean) {
            let gvar = caps.get(1).unwrap().as_str().to_string();
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            if !raw_clean.contains(&format!("{} == -1", gvar))
                && !raw_clean.contains(&format!("{} <= -1", gvar))
                && !raw_clean.contains(&format!("{} < 0", gvar))
                && !raw_clean.contains(&format!("{} != -1", gvar))
            {
                issues.push(iss(lineno, format!("'{}' = zp_items_register() return value not checked against -1", gvar), "zp_items_register_check", false));
            }
        }
    }

    issues
}

fn find_enclosing_fn(lines: &[&str], raw: &str, char_pos: usize) -> String {
    let lineno = raw[..char_pos].matches('\n').count();
    if lineno < lines.len() {
        enclosing_body(lines, lineno)
    } else {
        String::new()
    }
}

fn enclosing_body_from_pos(lines: &[&str], lineno: usize) -> String {
    if lineno < lines.len() {
        enclosing_body(lines, lineno)
    } else {
        String::new()
    }
}

fn iss(lineno: usize, message: String, rule_id: &'static str, fixable: bool) -> LintIssue {
    LintIssue { lineno, message, rule_id, severity: Severity::Error, auto_fixable: fixable }
}

fn strip_block_comments(text: &str) -> String {
    RE_BLOCK.replace_all(text, |caps: &regex::Captures| {
        "\n".repeat(caps.get(0).unwrap().as_str().matches('\n').count())
    }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip() {
        let r = strip_block_comments("hello /* w */ foo");
        assert_eq!(r, "hello  foo");
    }

    #[test]
    fn test_strip_multi() {
        let r = strip_block_comments("a /* b
c */ d");
        assert_eq!(r, "a \n d");
    }

    #[test]
    fn test_guard_range() {
        let s = "public f(id) {
	if (id < 1) return
	set_user_health(id, 100)
}";
        assert!(crate::rules::has_guard(s, "id"));
    }

    #[test]
    fn test_guard_is_user() {
        let s = "public f(id) {
	if (!is_user_connected(id)) return
	set_user_health(id, 100)
}";
        assert!(crate::rules::has_guard(s, "id"));
    }

    #[test]
    fn test_guard_none() {
        let s = "public f(id) {
	set_user_health(id, 100)
}";
        assert!(!crate::rules::has_guard(s, "id"));
    }

    #[test]
    fn test_body() {
        let text = "public a(id) {
    x()
}
public b() {
    return
}";
        let l: Vec<&str> = text.split('\n').collect();
        // Test from index 0 (the function def itself)
        let b0 = crate::rules::enclosing_body(&l, 0);
        assert!(b0.contains("a(id)"));
        // Test from index 1 (inside the function)
        let b1 = crate::rules::enclosing_body(&l, 1);
        assert!(b1.contains("a(id)"));
        assert!(!b1.contains("b()"));
    }
}
