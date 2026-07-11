use crate::config::Config;
use crate::engine::lint_file;
use crate::output::print_realtime;
use notify::{Config as NotifyConfig, Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn start_watch(root: &Path, cfg: &Config) {
    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
    let mut watcher = notify::RecommendedWatcher::new(tx, NotifyConfig::default()).unwrap();

    let paths: Vec<_> = cfg.paths.iter()
        .filter_map(|p| {
            let p = root.join(p);
            if p.exists() { Some(p) } else { None }
        })
        .collect();

    for p in &paths {
        watcher.watch(p, RecursiveMode::Recursive).unwrap();
        eprintln!("  Watching: {}", p.display());
    }
    eprintln!("Watching... Ctrl+C to stop");

    let mut debounce: HashMap<std::path::PathBuf, Instant> = HashMap::new();
    let use_color = cfg.output.color;

    loop {
        match rx.recv() {
            Ok(Ok(event)) => {
                if let EventKind::Modify(_) = event.kind {
                    for path in event.paths {
                        if path.extension().is_none_or(|e| e != "sma") { continue; }
                        let now = Instant::now();
                        if let Some(last) = debounce.get(&path)
                            && now.duration_since(*last) < Duration::from_millis(500) {
                            continue;
                        }
                        debounce.insert(path.clone(), now);
                        let issues = lint_file(&path, &cfg.rules);
                        print_realtime(&path, &issues, use_color);
                    }
                }
            }
            Ok(Err(e)) => eprintln!("Watch error: {}", e),
            Err(mpsc::RecvError) => break,
        }
    }
}
