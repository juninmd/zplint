use crate::config::Config;
use crate::engine::lint_file;
use std::path::Path;

pub fn auto_fix(filepath: &Path, config: &Config, _use_color: bool) -> i32 {
    let issues = lint_file(filepath, &config.rules);
    let fixable: Vec<_> = issues.iter().filter(|i| i.auto_fixable).collect();
    if fixable.is_empty() { return 0; }

    let raw = match std::fs::read_to_string(filepath) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
    let mut fixes = 0i32;

    for iss in fixable.iter().rev() {
        let idx = iss.lineno - 1;
        if idx >= lines.len() { continue; }
        let line = &lines[idx];
        let new = apply_fix(line, iss);
        if let Some(nl) = new {
            lines[idx] = nl;
            fixes += 1;
        }
    }

    if fixes > 0 {
        let out = lines.join("\n");
        std::fs::write(filepath, out).ok();
    }
    fixes
}

fn apply_fix(line: &str, issue: &crate::rules::LintIssue) -> Option<String> {
    let re_line = regex::Regex::new(r"^(\s*)if\s*\(\s*(\w+)\s*\)\s*$").unwrap();
    if (issue.rule_id == "zp_gamemode_if" || issue.rule_id == "zp_class_if")
        && let Some(caps) = re_line.captures(line) {
        return Some(format!("{}if ({} > 0)", caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str()));
    }
    if issue.rule_id == "buffer_size" {
        let re = regex::Regex::new(r"\b(get_user_name|get_user_authid|get_user_ip|get_user_team)\s*\(([^,]+),\s*(\w+),\s*(\d+)").unwrap();
        if let Some(caps) = re.captures(line) {
            let native = caps.get(1).unwrap().as_str();
            let id_var = caps.get(2).unwrap().as_str().trim();
            let buf_var = caps.get(3).unwrap().as_str().trim();
            let old = format!("{}({}, {}, {}", native, id_var, buf_var, caps.get(4).unwrap().as_str());
            let new = format!("{}({}, {}, charsmax({})", native, id_var, buf_var, buf_var);
            if line.contains(&old) {
                return Some(line.replace(&old, &new));
            }
        }
    }
    None
}
