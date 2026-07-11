use std::path::{Path, PathBuf};

pub fn discover_files(root: &Path, paths: &[String], exclude: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for p in paths {
        let base = root.join(p);
        if !base.exists() { continue; }
        if base.is_file() && base.extension().map_or(false, |e| e == "sma") {
            files.push(base);
            continue;
        }
        for entry in walkdir::WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "sma") { continue; }
            let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().to_string();
            if exclude.iter().any(|e| rel.contains(e)) { continue; }
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}
