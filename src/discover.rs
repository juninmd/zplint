use std::path::{Path, PathBuf};

pub fn discover_files(root: &Path, paths: &[String], exclude: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for p in paths {
        let base = root.join(p);
        push_sma_files(root, &base, exclude, &mut files);
    }
    files.sort();
    files
}

pub fn resolve_input_files(root: &Path, inputs: &[PathBuf], exclude: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for input in inputs {
        let base = if input.is_absolute() { input.clone() } else { root.join(input) };
        push_sma_files(root, &base, exclude, &mut files);
    }
    files.sort();
    files
}

fn push_sma_files(root: &Path, base: &Path, exclude: &[String], files: &mut Vec<PathBuf>) {
    if !base.exists() { return; }
    if base.is_file() {
        if base.extension().is_some_and(|e| e == "sma") && !is_excluded(root, base, exclude) {
            files.push(base.to_path_buf());
        }
        return;
    }
    for entry in walkdir::WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sma") { continue; }
        if is_excluded(root, path, exclude) { continue; }
        files.push(path.to_path_buf());
    }
}

/// Files changed in the working tree (and staged) relative to `since`
/// (default `HEAD`), plus untracked files. Used by `--changed`.
pub fn changed_files(root: &Path, since: Option<&str>, exclude: &[String]) -> Vec<PathBuf> {
    let base = since.unwrap_or("HEAD");
    let mut out = git_names(root, &["diff", "--name-only", "--diff-filter=ACMR", base]);
    out.extend(git_names(root, &["ls-files", "--others", "--exclude-standard"]));
    filter_sma(root, out, exclude)
}

/// Files staged for the next commit. Used by `--staged`.
pub fn staged_files(root: &Path, exclude: &[String]) -> Vec<PathBuf> {
    let out = git_names(root, &["diff", "--cached", "--name-only", "--diff-filter=ACMR"]);
    filter_sma(root, out, exclude)
}

fn git_names(root: &Path, args: &[&str]) -> Vec<PathBuf> {
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| root.join(l))
        .collect()
}

fn filter_sma(root: &Path, paths: Vec<PathBuf>, exclude: &[String]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "sma"))
        .filter(|p| !is_excluded(root, p, exclude))
        .collect();
    files.sort();
    files.dedup();
    files
}

fn is_excluded(root: &Path, path: &Path, exclude: &[String]) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().to_string();
    exclude.iter().any(|e| rel.contains(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_input_files_expands_directories() {
        let root = std::env::temp_dir().join(format!("zplint_discover_{}", std::process::id()));
        let dir = root.join("plugins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.sma"), "public plugin_init() {}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "skip\n").unwrap();

        let files = resolve_input_files(&root, &[PathBuf::from("plugins")], &[]);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "a.sma");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_input_files_respects_exclude() {
        let root = std::env::temp_dir().join(format!("zplint_discover_exclude_{}", std::process::id()));
        let dir = root.join("plugins").join("00-Old_Archive");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.sma"), "public plugin_init() {}\n").unwrap();

        let files = resolve_input_files(&root, &[PathBuf::from("plugins")], &["00-Old_Archive".to_string()]);

        assert!(files.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
