//! Rule metadata: category, recommended flag, severity and fix safety.
//!
//! Biome's rule groups are a large part of its UX (`--only=correctness`,
//! `--skip=style`, a `recommended` set). zplint's 100+ rules are still a flat
//! list in the engine, so the category is derived here instead of duplicating a
//! table that would drift. The id list and short descriptions come from the
//! curated README/KNOWLEDGE files at compile time, so `zplint rules` always
//! knows exactly the rules the docs advertise.

use crate::engine;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Correctness,
    Suspicious,
    Performance,
    Security,
    Style,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Correctness => "correctness",
            Category::Suspicious => "suspicious",
            Category::Performance => "performance",
            Category::Security => "security",
            Category::Style => "style",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "correctness" => Some(Category::Correctness),
            "suspicious" => Some(Category::Suspicious),
            "performance" => Some(Category::Performance),
            "security" => Some(Category::Security),
            "style" => Some(Category::Style),
            _ => None,
        }
    }
}

/// Rules that are style/perf/contract smells rather than crash paths.
pub fn category(id: &str) -> Category {
    match id {
        // Performance: hot-path work that can be hoisted or cached.
        "get_cvar_hotpath"
        | "library_exists_hotpath"
        | "allocstring_hotpath"
        | "strlen_in_loop"
        | "buffer_in_loop"
        | "read_file_loop"
        | "precache_in_loop"
        | "cs_model_hotpath"
        | "array_size_in_loop"
        | "get_players_in_loop"
        | "get_user_name_in_loop"
        | "format_in_loop"
        | "pdata_in_loop" => Category::Performance,

        // Security: user input reaching a command/format/SQL sink.
        "format_injection"
        | "cmd_injection_read_argv"
        | "sql_injection"
        | "path_traversal"
        | "log_injection"
        | "setinfo_injection" => Category::Security,

        // Style / modernization.
        "deprecated_symbols"
        | "define_reserved_const"
        | "duplicate_include"
        | "unknown_pragma"
        | "line_too_long"
        | "menu_handler_handled"
        | "hardcoded_maxplayers"
        | "client_cmd_spk"
        | "buffer_size"
        | "sizeof_scalar"
        | "redundant_else"
        | "boolean_return"
        | "if_equals_true"
        | "naming_convention"
        | "magic_number"
        | "deep_nesting"
        | "trailing_whitespace" => Category::Style,

        // Suspicious: likely a bug, but context can make it intentional.
        "assignment_in_condition"
        | "comparison_as_statement"
        | "self_assignment"
        | "constant_condition"
        | "contain_truthy"
        | "strcmp_truthy"
        | "sql_fieldname_truthy"
        | "func_id_truthy"
        | "global_shadowing"
        | "unreachable_code"
        | "mixed_return"
        | "self_recursion"
        | "use_before_init"
        | "unintended_bitwise"
        | "velocity_overwrite_no_add"
        | "zp_maxhealth_arg_order"
        | "zp_id_get_truthy"
        | "zp_menu_text_add_outside_select_pre"
        | "zp_select_pre_filter"
        | "zp_select_pre_return"
        | "zp43_mixing"
        | "zp50_register_return"
        | "zp50_get_in_init"
        | "client_command_handled"
        | "client_connect_actions"
        | "say_hook_handled"
        | "touch_spam"
        | "pev_oldbuttons"
        | "get_user_origin"
        | "te_reliable"
        | "mp3_loading_path"
        | "changelevel_cmd"
        | "hud_channel_range"
        | "entity_leak"
        | "precache_outside_precache"
        | "pragma_dynamic_stack"
        | "div_by_runtime"
        | "read_data_multi_context"
        | "callback_not_defined"
        | "get_players_flags"
        | "find_player_flags"
        | "read_data_bounds"
        | "api_deprecated" => Category::Suspicious,

        _ => Category::Correctness,
    }
}

/// Biome's `recommended` set excludes purely stylistic rules.
pub fn recommended(id: &str) -> bool {
    category(id) != Category::Style
}

/// All rule ids advertised by the README tables and the knowledge base.
pub fn all_rule_ids() -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for line in include_str!("../README.md").lines() {
        let t = line.trim();
        if !t.starts_with("| `") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("| `")
            && let Some(end) = rest.find('`')
        {
            let id = &rest[..end];
            if is_rule_id(id) {
                ids.push(id.to_string());
            }
        }
    }
    let knowledge = include_str!("../docs/KNOWLEDGE.md");
    for (i, _) in knowledge.match_indices("[rule: ") {
        let rest = &knowledge[i + 7..];
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if is_rule_id(&id) {
            ids.push(id);
        }
    }
    for (i, _) in knowledge.match_indices("[rules: ") {
        let rest = &knowledge[i + 8..];
        let end = rest.find(']').unwrap_or(0);
        for part in rest[..end].split(',') {
            let id = part.trim();
            if is_rule_id(id) {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// A README table's short description for `id`, if present.
pub fn description(id: &str) -> Option<String> {
    for line in include_str!("../README.md").lines() {
        let t = line.trim();
        if !t.starts_with(&format!("| `{id}`")) {
            continue;
        }
        let cells: Vec<&str> = t.split('|').collect();
        // Original-detector tables have an extra "Fix" column (✅/❌);
        // research-driven tables go straight from severity to description.
        let desc = if matches!(cells.get(3).map(|c| c.trim()), Some("✅" | "❌")) {
            cells.get(4)
        } else {
            cells.get(3)
        };
        if let Some(desc) = desc {
            return Some(desc.trim().to_string());
        }
    }
    None
}

pub fn severity(id: &str) -> &'static str {
    if engine::rule_is_warning(id) {
        "warning"
    } else {
        "error"
    }
}

fn is_rule_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() >= 4
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && s.contains('_')
}
