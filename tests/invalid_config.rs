use std::process::Command;

fn run_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zplint"))
        .args(["--no-auto-update"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run zplint")
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("zplint-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn malformed_config_fails_instead_of_silently_using_defaults() {
    let dir = temp_dir("bad-config");
    std::fs::write(dir.join("zplint.toml"), "this is = = not toml").unwrap();

    let out = run_in(&dir, &["rules"]);

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid zplint.toml"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_config_still_uses_defaults() {
    let dir = temp_dir("no-config");

    let out = run_in(&dir, &["rules"]);

    assert!(out.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}
