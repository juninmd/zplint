use crate::config::Config;
use crate::engine::lint_file;
use std::path::Path;

/// Errors when the fixed file cannot be read back or written, so the caller does not report it as fixed.
pub fn auto_fix(
    filepath: &Path,
    config: &Config,
    unsafe_fixes: bool,
    _use_color: bool,
) -> std::io::Result<i32> {
    let issues = lint_file(filepath, &config.rules);
    let fixable: Vec<_> = issues
        .iter()
        .filter(|i| i.fix.is_safe() || (unsafe_fixes && i.fix == crate::rules::FixSafety::Unsafe))
        .collect();
    if fixable.is_empty() { return Ok(0); }

    let raw = std::fs::read_to_string(filepath)?;
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
        std::fs::write(filepath, rejoin(&raw, &lines))?;
    }
    Ok(fixes)
}

/// Joins edited lines with the file's own line ending and keeps its final newline, so a fix on a
/// CRLF plugin does not rewrite every line of it.
fn rejoin(raw: &str, lines: &[String]) -> String {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = lines.join(eol);
    if raw.ends_with('\n') {
        out.push_str(eol);
    }
    out
}

fn apply_fix(line: &str, issue: &crate::rules::LintIssue) -> Option<String> {
    let re_line = regex::Regex::new(r"^(\s*)if\s*\(\s*(\w+)\s*\)\s*(\{)?\s*$").unwrap();
    if (issue.rule_id == "zp_gamemode_if" || issue.rule_id == "zp_class_if")
        && let Some(caps) = re_line.captures(line) {
        let brace = if caps.get(3).is_some() { " {" } else { "" };
        return Some(format!(
            "{}if ({} > 0){}",
            caps.get(1).unwrap().as_str(),
            caps.get(2).unwrap().as_str(),
            brace
        ));
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
    if issue.rule_id == "sizeof_string_len" {
        for (_, buf, sizeof_text) in crate::detectors::sizeof_len_sites(line) {
            // Only rewrite when the sizeof appears once on the line: the same text elsewhere
            // (a declaration `new other[sizeof buf]`) must not be touched.
            if line.matches(sizeof_text.as_str()).count() == 1 {
                return Some(line.replace(&sizeof_text, &format!("charsmax({})", buf)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_keeps_crlf_and_the_final_newline() {
        let dir = std::env::temp_dir().join(format!("zplint_fix_eol_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.sma");
        let src = "#include <amxmodx>\r\n\r\npublic plugin_init()\r\n{\r\n\tnew name[32]\r\n\tget_user_name(1, name, 32)\r\n}\r\n";
        std::fs::write(&file, src).unwrap();

        assert_eq!(auto_fix(&file, &Config::default(), false, false).unwrap(), 1);

        let fixed = std::fs::read_to_string(&file).unwrap();
        assert_eq!(fixed, src.replace("name, 32)", "name, charsmax(name))"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fix_reports_a_write_failure_instead_of_success() {
        let dir = std::env::temp_dir().join(format!("zplint_fix_ro_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.sma");
        let src = "public plugin_init()\n{\n\tnew name[32]\n\tget_user_name(1, name, 32)\n}\n";
        std::fs::write(&file, src).unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms.clone()).unwrap();

        // Root ignores file modes; the check only means something when the OS refuses the write.
        if std::fs::OpenOptions::new().write(true).open(&file).is_err() {
            assert!(auto_fix(&file, &Config::default(), false, false).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), src);
        }
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejoin_keeps_lf_files_lf() {
        let lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(rejoin("a\nb\n", &lines), "a\nb\n");
        assert_eq!(rejoin("a\nb", &lines), "a\nb");
    }
}
