use regex::Regex;

#[derive(Clone, Copy, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone)]
pub struct LintIssue {
    pub lineno: usize,
    pub message: String,
    pub rule_id: &'static str,
    pub severity: Severity,
    pub auto_fixable: bool,
}

pub static STOCK_SOUND_DIRS: &[&str] = &[
    "common", "items", "weapons", "player", "ambience", "buttons",
    "debris", "doors", "fvox", "hgrunt", "plats", "vox", "turret",
    "ambient", "misc", "scientist", "gunpickup", "wpn", "radio",
    "ars", "barney", "gman", "zombie", "materials", "tank", "combine",
];

pub static RISKY_NATIVES: &[&str] = &[
    "set_user_rendering", "set_user_health", "set_user_armor",
    "set_user_gravity", "set_user_maxspeed", "set_user_footsteps",
    "set_user_godmode", "cs_set_user_armor", "cs_set_user_maxspeed",
    "cs_reset_user_maxspeed", "cs_set_player_maxspeed",
    "cs_reset_player_maxspeed", "zp_ammopacks_set",
];

pub static DANGEROUS_FORWARDS: &[(&str, &str)] = &[
    ("client_disconnected", "id"),
    ("client_disconnect", "id"),
    ("fw_Spawn", "id"),
    ("fw_Killed", "victim"),
    ("Ham_Killed", "victim"),
];

/// Check if `var` is guarded in `scope` using fast string matching (no regex).
pub fn has_guard(scope: &str, var: &str) -> bool {
    // Range checks: var < 1, var <= 0, var == 0, var == -1
    if scope.contains(&format!("{} < 1", var))
        || scope.contains(&format!("{} <= 0", var))
        || scope.contains(&format!("{} == 0", var))
        || scope.contains(&format!("{} == -1", var))
    {
        return true;
    }
    // is_user_connected/is_user_alive/pev_valid/is_user_valid
    if scope.contains(&format!("is_user_connected({})", var))
        || scope.contains(&format!("is_user_alive({})", var))
        || scope.contains(&format!("pev_valid({})", var))
        || scope.contains(&format!("is_user_valid({})", var))
    {
        return true;
    }
    // pev(var, ...
    if scope.contains(&format!("pev({},", var)) {
        return true;
    }
    // get_players(...)
    if scope.contains("get_players(") {
        return true;
    }
    false
}

pub fn enclosing_body(lines: &[&str], idx: usize) -> String {
    let mut start = 0;
    for j in (0..=idx).rev() {
        let raw = lines[j];
        let ln = raw.trim();
        if ln.is_empty() { continue; }
        // Skip indented lines (function calls, not definitions)
        if raw.starts_with(' ') || raw.starts_with('\t') { continue; }
        let c = ln.chars().next().unwrap();
        if "#*/}{;,".contains(c) { continue; }
        if ln.contains('(') && !ln.starts_with("new ") && !ln.starts_with("return")
            && !ln.starts_with("if ") && !ln.starts_with("while ") && !ln.starts_with("for ")
            && !ln.starts_with("switch ")
        {
            start = j;
            break;
        }
    }
    let mut depth = 0i32;
    let mut started = false;
    let mut body = Vec::new();
    for line in lines.iter().skip(start) {
        body.push(*line);
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if line.contains('{') { started = true; }
        if started && depth <= 0 { break; }
    }
    body.join("\n")
}

pub fn find_publics(raw: &str) -> Vec<&str> {
    let re = Regex::new(r"\bpublic\s+([A-Za-z_]\w*)\s*\(").unwrap();
    re.captures_iter(raw).map(|c| c.get(1).unwrap().as_str()).collect()
}

pub fn find_nonpublics(raw: &str, publics: &[&str]) -> Vec<String> {
    let re = Regex::new(r"(?m)^([A-Za-z_]\w*)\s*\([^;{]*\)\s*(?:\{)?\s*$").unwrap();
    let all: Vec<String> = re.captures_iter(raw)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .collect();
    all.into_iter().filter(|n| !publics.contains(&n.as_str())).collect()
}

pub fn find_function_body_in(lines: &[&str], name: &str) -> String {
    let re = Regex::new(&format!(r"\bpublic\s+{}\s*\(", regex::escape(name))).unwrap();
    for (i, ln) in lines.iter().enumerate() {
        if re.is_match(ln) {
            return enclosing_body(lines, i);
        }
    }
    String::new()
}
