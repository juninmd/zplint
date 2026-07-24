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
static RE_CALLBACK_ARG1: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(menu_makecallback|set_error_filter)\s*\(\s*"([A-Za-z_]\w*)""#).unwrap()
});
static RE_CALLBACK_ARG2: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(menu_create|register_clcmd|register_concmd|register_srvcmd|register_menucmd|register_message|register_logevent|query_client_cvar|register_native)\s*\([^,]+,\s*"([A-Za-z_]\w*)""#).unwrap()
});
static RE_PERCENT_N_FORMAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(client_print|console_print|engclient_print|show_hudmessage|ShowSyncHudMsg|format|formatex)\s*\([^;]*"[^"]*%n"#).unwrap()
});
static RE_REG_EVENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"register_event\s*\([^,]+,\s*"(\w+)""#).unwrap());
static RE_REG_OTHER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(set_task|RegisterHam|register_forward)\s*\([^;]*?"(\w+)"#).unwrap());
static RE_ITEMS_REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(g_\w+)\s*=\s*zp_items_register\s*\(").unwrap());
static RE_IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_]\w*$").unwrap());
// New rules
static RE_TAKE_DAMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(Ham_TakeDamage|fw_TakeDamage|fw_Takedamage)\s*\(").unwrap());
static RE_GET_USER_ORIGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bget_user_origin\s*\(").unwrap());
static RE_TASK_ZERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bset_task\s*\(\s*(0(?:\.0*)?)\s*,"#).unwrap());
static RE_ABORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\babort\s*\(").unwrap());
static RE_MSG_BEGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:e?message_begin|e?message_begin_f)\s*\(|EngFunc_MessageBegin\b").unwrap());
static RE_MSG_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\be?message_end\s*\(|EngFunc_MessageEnd\b").unwrap());
static RE_MSG_WRITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\be?write_(?:byte|char|short|long|entity|angle|angle_f|coord|coord_f|string)\s*\(").unwrap());
static RE_MSG_HOOK_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:get_msg_(?:args|argtype|arg_int|arg_float|arg_string|origin)|set_msg_arg_(?:int|float|string))\s*\(").unwrap());
static RE_FUNCTION_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:public\s+)?([A-Za-z_]\w*)\s*\(").unwrap());
static RE_ARRAY_RANDOM_SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bArray(?:GetString|GetCell|GetArray)\s*\([^,]+,\s*random_num\s*\(\s*0\s*,\s*ArraySize\s*\(\s*([A-Za-z_]\w*)\s*\)\s*-\s*1\s*\)").unwrap());
static RE_FOPEN_ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:new\s+)?([A-Za-z_]\w*)\s*=\s*fopen\s*\(").unwrap());
static _RE_MAXPLAYERS_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#define\s+MAXPLAYERS\s+32").unwrap());
static RE_MAXPLAYERS_LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"for\s*\([^;]+;[^;]+<=?\s*MAXPLAYERS\b").unwrap());
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
    let raw = match std::fs::read(filepath) {
        // Old .sma files are often Windows-1252; byte -> codepoint keeps ASCII
        // and line structure intact, which is all the detectors need.
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
        },
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
    let has_hardcoded_maxplayers = _RE_MAXPLAYERS_DEF.is_match(&raw_clean);
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
            if let Some(nat) = RISKY_NATIVES.iter().find(|n| body.contains(**n))
                && !body.contains("is_user_connected") {
                issues.push(iss(lineno, format!("client_disconnected uses {} without is_user_connected guard (always crashes)", nat), "client_disconnect_guard", false));
            }
        }

        if config.dangerous_forward_guard {
            for (idx, (fwd, param)) in DANGEROUS_FORWARDS.iter().enumerate() {
                if !FWD_RE[idx].is_match(stripped) { continue; }
                let body = enclosing_body(&lines_clean, i);
                if has_guard(&body, param) { continue; }
                let body_sq = squash(&body);
                if let Some(nat) = RISKY_NATIVES.iter().find(|n| body_sq.contains(&format!("{}({}", n, param))) {
                    issues.push(iss(lineno, format!("{} calls {} on '{}' without is_user_connected/valid guard", fwd, nat, param), "dangerous_forward_guard", false));
                }
            }
        }

        if config.message_begin_guard && stripped.contains("MSG_ONE") && stripped.contains("message_begin")
            && let Some(var) = RE_MSG_ONE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string())
            && var != "0" && var != "1" && !var.chars().all(|c| c.is_ascii_digit())
            && !has_guard(&enclosing_body(&lines_clean, i), &var) {
            issues.push(iss(lineno, format!("message_begin(MSG_ONE,..,{}) without 1-32/is_user_* guard (may be non-player entity -> svc_bad)", var), "message_begin_guard", false));
        }

        if config.touch_spam && (stripped.contains("Ham_Touch") || stripped.contains("fw_Touch")) {
            let body = enclosing_body(&lines_clean, i);
            if body.matches("client_print(").count() > 1 && !body.contains("set_pev") && !body.to_lowercase().contains("task") {
                issues.push(iss(lineno, "Touch handler prints multiple times without a cooldown (spam)".into(), "touch_spam", false));
            }
        }

        if config.precache_sound && stripped.contains("emit_sound")
            && let Some(caps) = RE_EMIT_SOUND.captures(stripped) {
            let snd = caps.get(1).unwrap().as_str();
            let top = snd.split('/').next().unwrap_or("").to_lowercase();
            if !STOCK_SOUND_DIRS.contains(&top.as_str()) && !raw_clean.contains("precache_sound") {
                issues.push(iss(lineno, format!("emit_sound(\"{}\") custom sound with no precache_sound in file", snd), "precache_sound", false));
            }
        }

        if config.find_entity_in_sphere && stripped.contains("FindEntityInSphere")
            && let Some(var) = RE_FIND_SPHERE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
            let end = (i + 8).min(lines_clean.len());
            let after = lines_clean[i..end].join("\n");
            if uses_player_native_on(&after, &var) && !has_guard(&after, &var) {
                issues.push(iss(lineno, format!("FindEntityInSphere result '{}' used as player without 1-32 guard", var), "find_entity_in_sphere", false));
            }
        }

        if config.loop_player_guard
            && let Some(var) = RE_LOOP.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
            let end = (i + 40).min(lines_clean.len());
            let mut depth = 0i32; let mut started = false; let mut body = Vec::new();
            for line in lines_clean.iter().take(end).skip(i) {
                body.push(*line);
                depth += (line.matches('{').count() as i32) - (line.matches('}').count() as i32);
                if line.contains('{') { started = true; }
                if started && depth <= 0 { break; }
            }
            let body = body.join("\n");
            if uses_player_native_on(&body, &var) && !has_guard(&body, &var) {
                issues.push(iss(lineno, format!("loop 1-32 uses player natives on '{}' without is_user_connected/alive guard", var), "loop_player_guard", false));
            }
        }

        if config.zp_infect_cure_guard && (stripped.contains("zp_core_infect") || stripped.contains("zp_core_cure"))
            && let Some(caps) = RE_INFECT.captures(stripped) {
            let var = caps.get(2).unwrap().as_str().to_string();
            let body = enclosing_body(&lines_clean, i);
            if !squash(&body).contains(&format!("zp_core_is_zombie({}", var)) {
                issues.push(iss(lineno, format!("zp_core_{}('{}') without checking if already infected/cured first (run time error 10)", caps.get(1).unwrap().as_str(), var), "zp_infect_cure_guard", false));
            }
        }

        if config.zp_gamemode_if && stripped.contains("zp_gamemodes_get_current")
            && let Some(var) = RE_GAMEMODE.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
            let body = squash(&enclosing_body(&lines_clean, i));
            if body.contains(&format!("if({})", var)) && !body.contains(&format!("if({}>0)", var)) {
                issues.push(iss(lineno, format!("if ({}) should be if ({} > 0) - gamemode can return -2 (ZP_NO_GAME_MODE)", var, var), "zp_gamemode_if", true));
            }
        }

        if config.zp_class_if {
            for re_fn in [&*RE_CLASS_Z, &*RE_CLASS_H] {
                if let Some(var) = re_fn.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
                    let body = squash(&enclosing_body(&lines_clean, i));
                    if body.contains(&format!("if({})", var)) && !body.contains(&format!("if({}>0)", var)) {
                        issues.push(iss(lineno, format!("if ({}) should be if ({} > 0) - class ID can return -1 (ZP_NO_CLASS)", var, var), "zp_class_if", true));
                    }
                }
            }
        }

        if config.pev_oldbuttons && stripped.contains("pev_oldbuttons") {
            issues.push(iss(lineno, "pev_oldbuttons used (unreliable in PreThink, use manual pev_button tracking instead)".into(), "pev_oldbuttons", false));
        }

        if config.precache_sound_sprite && stripped.contains("precache_sound(")
            && let Some(varname) = RE_PRECACHE_SPR.captures(stripped).map(|c| c.get(1).unwrap().as_str())
            && RE_SPRITE.is_match(varname) {
            issues.push(iss(lineno, format!("precache_sound assigned to '{}' (variable will be 0/1 (bool) not a sprite handle; use precache_model instead)", varname), "precache_sound_sprite", false));
        }

        if config.create_entity_guard && stripped.contains("create_entity(")
            && let Some(var) = RE_CREATE_ENT.captures(stripped).map(|c| c.get(1).unwrap().as_str().to_string()) {
            let body = squash(&enclosing_body(&lines_clean, i));
            if !body.contains(&format!("is_valid_ent({}", var)) && !body.contains(&format!("!{}", var)) {
                issues.push(iss(lineno, format!("create_entity result '{}' used without is_valid_ent check", var), "create_entity_guard", false));
            }
        }

        if config.buffer_size && let Some(caps) = RE_BUFFER.captures(stripped) {
            let bufsize: usize = caps.get(3).unwrap().as_str().parse().unwrap_or(64);
            if bufsize < 64 {
                issues.push(iss(lineno, format!("{} uses hardcoded buffer size {} (prefer charsmax({}) over hardcoded {})", caps.get(1).unwrap().as_str(), bufsize, caps.get(2).unwrap().as_str(), bufsize), "buffer_size", true));
            }
        }

        if config.client_cmd_spk && stripped.contains("client_cmd(0,") && stripped.contains("\"spk") {
            issues.push(iss(lineno, "use emit_sound() instead of client_cmd(0, 'spk...')".into(), "client_cmd_spk", false));
        }

        if config.percent_n_player_name && stripped.contains("%n") && RE_PERCENT_N_FORMAT.is_match(stripped) {
            issues.push(iss(lineno, "%n player-name formatter can throw on invalid/disconnected index; guard the player and use get_user_name() + %s".into(), "percent_n_player_name", false));
        }

        // --- NEW RULES ---

        // 18. attacker_not_validated - fw_TakeDamage/Ham_TakeDamage handlers using attacker without guard
        if config.attacker_not_validated && RE_TAKE_DAMAGE.is_match(stripped) {
            let body = enclosing_body(&lines_clean, i);
            let body_sq = squash(&body);
            let has_user_alive_check = body_sq.contains("is_user_alive(attacker)")
                || body_sq.contains("is_user_connected(attacker)")
                || body_sq.contains("!attacker");
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
        if config.task_interval_zero && let Some(caps) = RE_TASK_ZERO.captures(stripped) {
            issues.push(iss(lineno, format!("set_task with interval {} is invalid (minimum 0.1)", caps.get(1).unwrap().as_str()), "task_interval_zero", false));
        }

        if config.set_task_flags && stripped.contains("set_task") {
            for args in extract_call_args(stripped, "set_task") {
                if args.len() < 6 {
                    continue;
                }
                let flags = trim_string_literal(&args[5]);
                if !flags.chars().all(|flag| matches!(flag, 'a' | 'b' | 'c' | 'd')) {
                    issues.push(iss(lineno, format!("set_task flags \"{}\" contains unsupported flag; valid flags are a/b/c/d", flags), "set_task_flags", false));
                    continue;
                }
                let repeat = args.get(6).map(|arg| arg.trim()).unwrap_or("0");
                let repeat_is_zero = repeat.is_empty() || repeat == "0";
                if flags.contains('a') && repeat_is_zero {
                    issues.push(iss(lineno, "set_task flag \"a\" requires repeat > 0".into(), "set_task_flags", false));
                } else if !flags.contains('a') && !repeat_is_zero {
                    issues.push(iss(lineno, "set_task repeat argument is ignored unless flag \"a\" is present".into(), "set_task_flags", false));
                }
            }
        }

        // 21. abort_call - abort( usage (abort(AMX_ERR_NATIVE, ...) inside a
        // registered native is the documented way to raise a native error)
        if config.abort_call && RE_ABORT.is_match(stripped) && !stripped.contains("AMX_ERR_") {
            issues.push(iss(lineno, "abort() causes run time error 1 - use log_error() for graceful degradation".into(), "abort_call", false));
        }

        if config.message_write_outside {
            let has_write = RE_MSG_WRITE.find(stripped);
            if let Some(write_match) = has_write {
                let begin_before_write = RE_MSG_BEGIN.find(stripped).is_some_and(|m| m.start() < write_match.start());
                if msg_nesting == 0 && !begin_before_write {
                    issues.push(iss(lineno, "write_*() outside message_begin()/message_end() will crash the server immediately".into(), "message_write_outside", false));
                }
            }
        }

        if config.message_end_without_begin && let Some(end_match) = RE_MSG_END.find(stripped) {
            let begin_before_end = RE_MSG_BEGIN.find(stripped).is_some_and(|m| m.start() < end_match.start());
            if msg_nesting == 0 && !begin_before_end {
                issues.push(iss(lineno, "message_end() without message_begin() will crash the server immediately".into(), "message_end_without_begin", false));
            }
        }

        if config.hardcoded_message_id {
            for function in ["message_begin", "emessage_begin"] {
                for args in extract_call_args(stripped, function) {
                    if args.get(1).is_some_and(|arg| arg.trim().chars().all(|ch| ch.is_ascii_digit())) {
                        issues.push(iss(lineno, format!("{} uses hardcoded numeric message id {}; use get_user_msgid() or an AMXX message constant", function, args[1].trim()), "hardcoded_message_id", false));
                    }
                }
            }
        }

        if config.array_random_empty && let Some(caps) = RE_ARRAY_RANDOM_SIZE.captures(stripped) {
            let array_name = caps.get(1).unwrap().as_str();
            if !has_array_size_guard(&enclosing_body(&lines_clean, i), array_name) {
                issues.push(iss(lineno, format!("random_num(0, ArraySize({}) - 1) can produce an invalid index when the array is empty; guard ArraySize({}) > 0 first", array_name, array_name), "array_random_empty", false));
            }
        }

        if config.fopen_close && let Some(caps) = RE_FOPEN_ASSIGN.captures(stripped) {
            let file_handle = caps.get(1).unwrap().as_str();
            if !squash(&enclosing_body(&lines_clean, i)).contains(&format!("fclose({})", file_handle)) {
                issues.push(iss(lineno, format!("fopen handle \"{}\" is not closed with fclose({}) in this function", file_handle, file_handle), "fopen_close", false));
            }
        }

        // 22. nested_message - message_begin before message_end
        if config.nested_message {
            if RE_MSG_BEGIN.is_match(stripped) {
                // `if (..) message_begin(A) else message_begin(B)` is one message, not nesting
                let prev = lines_clean[..i].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim()).unwrap_or("");
                let else_branch = prev == "else" || prev.starts_with("else ") || prev.starts_with("else{");
                if msg_nesting > 0 && !else_branch {
                    issues.push(iss(lineno, "nested message_begin() without closing previous message_end() (will crash server)".into(), "nested_message", false));
                }
                if !else_branch { msg_nesting += 1; }
                _msg_begin_lineno = lineno;
            }
            if RE_MSG_END.is_match(stripped) {
                msg_nesting = 0i32.max(msg_nesting - 1);
            }
        }

        // 23. hardcoded_maxplayers - runtime loop using #define MAXPLAYERS 32
        if config.hardcoded_maxplayers && has_hardcoded_maxplayers && RE_MAXPLAYERS_LOOP.is_match(stripped) {
            issues.push(iss(lineno, "loop uses hardcoded MAXPLAYERS 32 as runtime player count; use get_maxplayers() or cached MaxClients".into(), "hardcoded_maxplayers", false));
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
            if !squash(&body_enclosing).contains(&format!("zp_core_is_zombie({}", var)) {
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

    // Post-pass: 28. Infection-by-damage must validate last-human AND survivor.
    // ZP invariant: a zombie infecting a human on damage must (a) never infect a
    // survivor (immune) and (b) NOT infect the last human - the last human must be
    // KILLED so CS ends the round (CT eliminated). Skipping either bugs the game:
    // the round never ends when everyone turns zombie, or a survivor gets infected.
    if config.zp_infect_lasthuman_survivor {
        for caps in RE_TAKE_DAMAGE.captures_iter(&raw_clean) {
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            let body = enclosing_body_from_pos(&lines_clean, lineno - 1);
            // Only handlers that actually infect on damage are subject to the rule.
            if !body.contains("zp_core_infect") && !body.contains("zp_core_force_infect") {
                continue;
            }
            let body_sq = squash(&body);
            let has_lasthuman = body_sq.contains("get_human_count()==1")
                || body_sq.contains("zp_core_is_last_human")
                || body_sq.contains("is_last_human");
            let has_survivor = body_sq.contains("survivor_get")
                || body_sq.contains("zp_class_survivor");
            if has_lasthuman && has_survivor {
                continue;
            }
            let mut missing: Vec<&str> = Vec::new();
            if !has_lasthuman { missing.push("last-human (zp_core_get_human_count()==1)"); }
            if !has_survivor { missing.push("survivor (zp_class_survivor_get)"); }
            issues.push(iss(
                lineno,
                format!("infection-by-damage handler must validate {} before infecting, or the round can stall / a survivor gets infected", missing.join(" and ")),
                "zp_infect_lasthuman_survivor",
                false,
            ));
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

    if config.registered_callback_public {
        let publics = find_publics(&raw_clean);
        let nonpublics = find_nonpublics(&raw_clean, &publics);
        for re in [&*RE_CALLBACK_ARG1, &*RE_CALLBACK_ARG2] {
            for caps in re.captures_iter(&raw_clean) {
                let cb = caps.get(2).unwrap().as_str().to_string();
                if !nonpublics.contains(&cb) {
                    continue;
                }

                let native = caps.get(1).unwrap().as_str();
                let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
                issues.push(iss(
                    lineno,
                    format!("{} callback \"{}\" is a non-public function (AMXX requires registered callbacks to be public)", native, cb),
                    "registered_callback_public",
                    false,
                ));
            }
        }
    }

    if config.menu_handler_destroy {
        let publics = find_publics(&raw_clean);
        let mut function_names: Vec<String> = publics.iter().map(|name| name.to_string()).collect();
        function_names.extend(find_nonpublics(&raw_clean, &publics));
        for caps in RE_CALLBACK_ARG2.captures_iter(&raw_clean) {
            if caps.get(1).unwrap().as_str() != "menu_create" {
                continue;
            }

            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            if enclosing_function_name(&lines_clean, lineno - 1, &function_names).as_deref() == Some("plugin_init") {
                continue;
            }

            let handler = caps.get(2).unwrap().as_str();
            let handler_body = find_function_body_in(&lines_clean, handler);
            if handler_body.is_empty() || handler_body.contains("menu_destroy(") {
                continue;
            }

            issues.push(iss(lineno, format!("dynamic menu_create handler \"{}\" does not call menu_destroy(); AMXX docs require destroying dynamic menu resources", handler), "menu_handler_destroy", false));
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
            if let Some((_, lineno, reg)) = other_regs.iter().find(|(c, _, _)| c == cb)
                && find_function_body_in(&lines_clean, cb).contains("read_data(") {
                issues.push(iss(*lineno, format!("read_data() in '{}' but also registered as non-event callback via {} (may read stale data)", cb, reg), "read_data_multi_context", false));
            }
        }
    }

    if config.message_hook_scope {
        let publics = find_publics(&raw_clean);
        let mut function_names: Vec<String> = publics.iter().map(|name| name.to_string()).collect();
        function_names.extend(find_nonpublics(&raw_clean, &publics));
        let message_cbs: Vec<String> = RE_CALLBACK_ARG2
            .captures_iter(&raw_clean)
            .filter(|caps| caps.get(1).unwrap().as_str() == "register_message")
            .map(|caps| caps.get(2).unwrap().as_str().to_string())
            .collect();
        let mut flagged_lines: Vec<usize> = Vec::new();
        for caps in RE_MSG_HOOK_ONLY.captures_iter(&raw_clean) {
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            if flagged_lines.contains(&lineno) {
                continue;
            }
            let fn_name = enclosing_function_name(&lines_clean, lineno - 1, &function_names);
            if fn_name.as_ref().is_some_and(|name| message_cbs.iter().any(|cb| cb == name)) {
                continue;
            }
            issues.push(iss(lineno, "get_msg_arg*/set_msg_arg* used outside a register_message callback (AMXX will throw outside hooked message scope)".into(), "message_hook_scope", false));
            flagged_lines.push(lineno);
        }
    }

    crate::detectors::run(&raw_clean, &lines_clean, config, &mut issues);
    crate::api_check::run(&raw_clean, &lines_clean, config, &mut issues);

    if config.zp_items_register_check {
        for caps in RE_ITEMS_REG.captures_iter(&raw_clean) {
            let gvar = caps.get(1).unwrap().as_str().to_string();
            let lineno = raw_clean[..caps.get(0).unwrap().start()].matches('\n').count() + 1;
            let raw_sq = squash(&raw_clean);
            if !raw_sq.contains(&format!("{}==-1", gvar))
                && !raw_sq.contains(&format!("{}<=-1", gvar))
                && !raw_sq.contains(&format!("{}<0", gvar))
                && !raw_sq.contains(&format!("{}!=-1", gvar))
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

pub(crate) fn enclosing_function_name(lines: &[&str], lineno: usize, function_names: &[String]) -> Option<String> {
    let mut idx = lineno.min(lines.len().saturating_sub(1));
    loop {
        if let Some(caps) = RE_FUNCTION_DEF.captures(lines[idx]) {
            let name = caps.get(1).unwrap().as_str();
            if function_names.iter().any(|function_name| function_name == name) {
                return Some(name.to_string());
            }
        }
        if idx == 0 {
            break;
        }
        idx -= 1;
    }
    None
}

/// Rules that flag style/perf smells rather than crash risks (do not fail CI).
static WARNING_RULES: &[&str] = &[
    // api_check.rs: still compiles, unlike the other api_* rules
    "api_deprecated",
    "touch_spam", "pev_oldbuttons", "get_user_origin", "library_exists_hotpath",
    "hardcoded_maxplayers", "client_cmd_spk", "buffer_size",
    // detectors.rs warnings
    "mp3_loading_path", "te_reliable", "changelevel_cmd", "deprecated_symbols",
    "define_reserved_const", "constant_condition", "self_assignment",
    "comparison_as_statement", "assignment_in_condition", "unreachable_code",
    "strlen_in_loop", "get_cvar_hotpath", "buffer_in_loop", "read_file_loop",
    "precache_in_loop", "pragma_dynamic_stack", "div_by_runtime", "global_shadowing",
    "callback_not_defined", "zp50_register_return", "zp50_get_in_init",
    "zp_select_pre_filter", "zp_select_pre_return", "zp43_mixing", "entity_leak",
    "client_command_handled", "client_connect_actions", "contain_truthy",
    "strcmp_truthy", "sql_fieldname_truthy", "func_id_truthy", "format_injection", "string_assign", "hud_channel_range",
    "line_too_long",
];

pub(crate) fn iss(lineno: usize, message: String, rule_id: &'static str, fixable: bool) -> LintIssue {
    let severity = if WARNING_RULES.contains(&rule_id) { Severity::Warning } else { Severity::Error };
    LintIssue { lineno, message, rule_id, severity, auto_fixable: fixable }
}

/// Remove /* */ comments, preserving line structure. Unlike a plain regex this
/// ignores `/*` inside line comments (`//***`) and string literals.
fn strip_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_block = false;
    let mut in_line = false;
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            } else if c == '\n' {
                out.push('\n');
            }
            continue;
        }
        if c == '\n' {
            in_line = false;
            in_str = false; // Pawn strings cannot span a raw newline
            out.push('\n');
            continue;
        }
        if in_line {
            out.push(c);
            continue;
        }
        if in_str {
            out.push(c);
            if c == '^' {
                if let Some(n) = chars.next() { out.push(n); }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => { in_str = true; out.push(c); }
            '/' if chars.peek() == Some(&'/') => { in_line = true; out.push(c); }
            '/' if chars.peek() == Some(&'*') => { chars.next(); in_block = true; }
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn extract_call_args(line: &str, function: &str) -> Vec<Vec<String>> {
    let mut calls = Vec::new();
    let needle = format!("{}(", function);
    let mut offset = 0usize;
    while let Some(found) = line[offset..].find(&needle) {
        let args_start = offset + found + needle.len();
        if let Some((args, end)) = parse_args(&line[args_start..]) {
            calls.push(args);
            offset = args_start + end + 1;
        } else {
            break;
        }
    }
    calls
}

fn parse_args(input: &str) -> Option<(Vec<String>, usize)> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut prev_escape = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            current.push(ch);
            if ch == '"' && !prev_escape {
                in_string = false;
            }
            prev_escape = ch == '\\' && !prev_escape;
            if ch != '\\' {
                prev_escape = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                current.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' if depth == 0 => {
                args.push(current.trim().to_string());
                return Some((args, idx));
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    None
}

fn trim_string_literal(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn has_array_size_guard(body: &str, array_name: &str) -> bool {
    let body_sq = squash(body);
    let size_call = format!("ArraySize({})", array_name);
    body_sq.contains(&format!("if({})", size_call))
        || body_sq.contains(&format!("{}>0", size_call))
        || body_sq.contains(&format!("0<{}", size_call))
        || body_sq.contains(&format!("{}!=0", size_call))
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

    #[test]
    fn test_registered_callback_private_function() {
        let path = write_temp_sma("private_menu_callback", r#"public plugin_init() {
    menu_create("Menu", "menu_handler")
}

menu_handler(id, menu, item) {
    return PLUGIN_HANDLED
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(
            issues.iter().any(|issue| issue.rule_id == "registered_callback_public"),
            "issues: {:?}",
            issues.iter().map(|issue| issue.rule_id).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_callback_arg2_regex() {
        let caps = RE_CALLBACK_ARG2
            .captures(r#"menu_create("Menu", "menu_handler")"#)
            .unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "menu_create");
        assert_eq!(caps.get(2).unwrap().as_str(), "menu_handler");
    }

    #[test]
    fn test_find_nonpublics_same_line_brace() {
        let raw = "public plugin_init() {\n}\n\nmenu_handler(id, menu, item) {\n    return PLUGIN_HANDLED\n}\n";
        let publics = find_publics(raw);
        let nonpublics = find_nonpublics(raw, &publics);
        assert!(nonpublics.contains(&"menu_handler".to_string()));
    }

    #[test]
    fn test_registered_callback_public_function() {
        let path = write_temp_sma("public_menu_callback", r#"public plugin_init() {
    menu_create("Menu", "menu_handler")
}

public menu_handler(id, menu, item) {
    return PLUGIN_HANDLED
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "registered_callback_public"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_task_interval_zero_rejects_zero_only() {
        assert!(RE_TASK_ZERO.is_match(r#"set_task(0.0, "cb")"#));
        assert!(!RE_TASK_ZERO.is_match(r#"set_task(0.1, "cb")"#));
        assert!(!RE_TASK_ZERO.is_match(r#"set_task(1.0, "cb")"#));
    }

    #[test]
    fn test_percent_n_player_name_is_flagged() {
        let path = write_temp_sma("percent_n", r#"public show_boss(g_Boss) {
    ShowSyncHudMsg(0, g_HudSync, "Boss: %n", g_Boss)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "percent_n_player_name"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_percent_s_player_name_is_ok() {
        let path = write_temp_sma("percent_s", r#"public show_boss() {
    new name[32]
    get_user_name(1, name, charsmax(name))
    ShowSyncHudMsg(0, g_HudSync, "Boss: %s", name)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "percent_n_player_name"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_hardcoded_maxplayers_flags_runtime_loop_only() {
        let path = write_temp_sma("hardcoded_maxplayers", r#"#define MAXPLAYERS 32
new g_Seen[MAXPLAYERS + 1]

public scan_players() {
    for (new id = 1; id <= MAXPLAYERS; id++) {
        client_print(id, print_chat, "x")
    }
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "hardcoded_maxplayers"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_hardcoded_maxplayers_allows_array_bound_only() {
        let path = write_temp_sma("maxplayers_array", r#"#define MAXPLAYERS 32
new g_Seen[MAXPLAYERS + 1]
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "hardcoded_maxplayers"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_write_outside_is_flagged() {
        let path = write_temp_sma("message_write_outside", r#"public bad_message() {
    write_byte(TE_EXPLOSION)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "message_write_outside"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_write_inside_is_ok() {
        let path = write_temp_sma("message_write_inside", r#"public good_message() {
    message_begin(MSG_BROADCAST, SVC_TEMPENTITY)
    write_byte(TE_EXPLOSION)
    message_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "message_write_outside"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_write_inline_after_begin_is_ok() {
        let path = write_temp_sma("message_write_inline", r#"public inline_message() {
    message_begin(MSG_BROADCAST, SVC_TEMPENTITY); write_byte(TE_EXPLOSION); message_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "message_write_outside"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_end_without_begin_is_flagged() {
        let path = write_temp_sma("message_end_without_begin", r#"public bad_message() {
    message_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "message_end_without_begin"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_end_inline_after_begin_is_ok() {
        let path = write_temp_sma("message_end_inline", r#"public inline_message() {
    message_begin(MSG_BROADCAST, SVC_TEMPENTITY); message_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "message_end_without_begin"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_set_task_invalid_flag_is_flagged() {
        let path = write_temp_sma("set_task_invalid_flag", r#"public plugin_init() {
    set_task(1.0, "tick", 0, _, _, "x")
}
public tick() {}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "set_task_flags"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_set_task_repeat_without_flag_a_is_flagged() {
        let path = write_temp_sma("set_task_repeat_without_a", r#"public plugin_init() {
    set_task(1.0, "tick", 0, _, _, "b", 3)
}
public tick() {}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "set_task_flags"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_set_task_flag_a_with_repeat_is_ok() {
        let path = write_temp_sma("set_task_flag_a", r#"public plugin_init() {
    set_task(1.0, "tick", 0, _, _, "a", 3)
}
public tick() {}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "set_task_flags"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_hook_scope_outside_callback_is_flagged() {
        let path = write_temp_sma("message_hook_scope_bad", r#"public plugin_init() {
}

public not_a_message_hook() {
    return get_msg_arg_int(1)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "message_hook_scope"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_hook_scope_registered_callback_is_ok() {
        let path = write_temp_sma("message_hook_scope_ok", r#"public plugin_init() {
    register_message(get_user_msgid("Health"), "message_health")
}

public message_health() {
    new value = get_msg_arg_int(1)
    new text[16]
    get_msg_arg_string(2, text, charsmax(text))
    return value
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "message_hook_scope"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_block_is_not_hook_scope_only() {
        let path = write_temp_sma("message_block_ok", r#"public plugin_init() {
    new msg = get_user_msgid("DeathMsg")
    set_msg_block(msg, BLOCK_ONCE)
    get_msg_block(msg)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "message_hook_scope"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_hardcoded_message_id_is_flagged() {
        let path = write_temp_sma("hardcoded_message_id_bad", r#"public bad_message(id) {
    message_begin(MSG_ONE, 108, {0,0,0}, id)
    message_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "hardcoded_message_id"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_message_constants_and_variables_are_ok() {
        let path = write_temp_sma("hardcoded_message_id_ok", r#"public good_message(id) {
    new msg = get_user_msgid("StatusIcon")
    message_begin(MSG_ONE, msg, _, id)
    message_end()
    message_begin(MSG_BROADCAST, SVC_TEMPENTITY)
    message_end()
    emessage_begin(MSG_ONE, get_user_msgid("Health"), _, id)
    emessage_end()
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "hardcoded_message_id"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_array_random_empty_is_flagged() {
        let path = write_temp_sma("array_random_empty_bad", r#"public play_sound() {
    new sound[64]
    ArrayGetString(g_sound_win, random_num(0, ArraySize(g_sound_win) - 1), sound, charsmax(sound))
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "array_random_empty"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_array_random_empty_guard_is_ok() {
        let path = write_temp_sma("array_random_empty_ok", r#"public play_sound() {
    if (ArraySize(g_sound_win) > 0) {
        new sound[64]
        ArrayGetString(g_sound_win, random_num(0, ArraySize(g_sound_win) - 1), sound, charsmax(sound))
    }
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "array_random_empty"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_menu_handler_destroy_is_flagged() {
        let path = write_temp_sma("menu_handler_destroy_bad", r#"public show_menu(id) {
    new menu = menu_create("Main", "menu_main")
    menu_display(id, menu)
}

public menu_main(id, menu, item) {
    return PLUGIN_HANDLED
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "menu_handler_destroy"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_menu_handler_destroy_is_ok() {
        let path = write_temp_sma("menu_handler_destroy_ok", r#"public show_menu(id) {
    new menu = menu_create("Main", "menu_main")
    menu_display(id, menu)
}

public menu_main(id, menu, item) {
    menu_destroy(menu)
    return PLUGIN_HANDLED
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "menu_handler_destroy"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_menu_create_in_plugin_init_is_not_dynamic_menu_destroy() {
        let path = write_temp_sma("menu_handler_destroy_plugin_init", r#"public plugin_init() {
    g_Menu = menu_create("Main", "menu_main")
}

public menu_main(id, menu, item) {
    return PLUGIN_HANDLED
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "menu_handler_destroy"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_fopen_close_is_flagged() {
        let path = write_temp_sma("fopen_close_bad", r#"public load_file() {
    new file = fopen("x.ini", "rt")
    if (!file) return
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(issues.iter().any(|issue| issue.rule_id == "fopen_close"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_fopen_close_is_ok() {
        let path = write_temp_sma("fopen_close_ok", r#"public load_file() {
    new file = fopen("x.ini", "rt")
    if (!file) return
    fclose(file)
}
"#);

        let issues = lint_file(&path, &RulesConfig::default());
        assert!(!issues.iter().any(|issue| issue.rule_id == "fopen_close"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_extract_call_args_keeps_nested_commas() {
        let calls = extract_call_args(r#"set_task(random_float(1.0, 2.0), "tick", 0, _, _, "a", 3)"#, "set_task");
        assert_eq!(calls[0][0], "random_float(1.0, 2.0)");
        assert_eq!(calls[0][5], "\"a\"");
        assert_eq!(calls[0][6], "3");
    }

    fn write_temp_sma(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("zplint_{}_{}.sma", name, std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }
}
