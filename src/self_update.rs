//! `zplint self-update`: replace the running binary with the latest GitHub release.
//!
//! The release workflow publishes a raw binary plus a `.sha256` file per target, so the
//! updater needs no archive support: download, verify the checksum, swap the file.

use crate::config::{UpdateConfig, UpdateMode};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "juninmd/zplint";
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const AUTO_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Release asset name for the platform this binary was built for.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("zplint-x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("zplint-x86_64-pc-windows-msvc.exe")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("zplint-aarch64-apple-darwin")
    } else {
        None
    }
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let v = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    it.next().is_none().then_some(v)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// First field of a `sha256sum`-style line.
fn parse_checksum(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(concat!("zplint/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn request(
    agent: &ureq::Agent,
    url: &str,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let mut req = agent.get(url);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    req.call()
}

fn get(agent: &ureq::Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    request(agent, url).with_context(|| format!("GET {url}"))
}

fn fetch_release(agent: &ureq::Agent, version: Option<&str>) -> Result<Release> {
    let url = match version {
        Some(v) => format!(
            "https://api.github.com/repos/{REPO}/releases/tags/v{}",
            v.trim_start_matches('v')
        ),
        None => format!("https://api.github.com/repos/{REPO}/releases/latest"),
    };
    let mut resp = match request(agent, &url) {
        Err(ureq::Error::StatusCode(404)) => bail!("no matching release published on {REPO} yet"),
        other => other.with_context(|| format!("GET {url}"))?,
    };
    let body = resp.body_mut().read_to_string()?;
    serde_json::from_str(&body).context("unexpected release payload")
}

fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    Ok(get(agent, url)?
        .body_mut()
        .with_config()
        .limit(MAX_BINARY_BYTES)
        .read_to_vec()?)
}

fn download_verified(agent: &ureq::Agent, release: &Release) -> Result<Vec<u8>> {
    let latest = release.tag_name.trim_start_matches('v');
    let Some(name) = asset_name() else {
        bail!("no prebuilt binary for this platform; use `cargo install --git https://github.com/{REPO}`");
    };
    let find = |n: &str| release.assets.iter().find(|a| a.name == n);
    let bin = find(name).with_context(|| format!("release v{latest} has no asset {name}"))?;
    let sum = find(&format!("{name}.sha256"))
        .with_context(|| format!("release v{latest} has no checksum for {name}"))?;

    let expected = parse_checksum(&download_text(agent, &sum.browser_download_url)?)
        .context("malformed checksum file")?;
    let bytes = download(agent, &bin.browser_download_url)?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }
    Ok(bytes)
}

fn install(new_bytes: &[u8], exe: &Path) -> Result<()> {
    let staged = exe.with_extension("new");
    std::fs::write(&staged, new_bytes).with_context(|| format!("write {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        // A running .exe cannot be overwritten but can be renamed away.
        let old = exe.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).context("move running binary aside")?;
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e).context("install new binary");
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(&staged, exe).context("install new binary")?;
    Ok(())
}

/// Returns the process exit code: 0 up to date / updated, 1 update available with `--check`.
pub fn run(check: bool, version: Option<&str>) -> Result<i32> {
    let current = env!("CARGO_PKG_VERSION");
    let http = agent(DOWNLOAD_TIMEOUT);
    let release = fetch_release(&http, version)?;
    let latest = release.tag_name.trim_start_matches('v');
    record_check(&cache_file_path(), unix_now_secs());

    if version.is_none() && !is_newer(latest, current) {
        println!("zplint {current} is up to date");
        return Ok(0);
    }
    if check {
        println!("update available: {current} -> {latest} (run `zplint self-update`)");
        return Ok(1);
    }

    let bytes = download_verified(&http, &release)?;
    let exe = std::env::current_exe()?.canonicalize()?;
    install(&bytes, &exe)?;
    println!("updated zplint {current} -> {latest}");
    Ok(0)
}

/// Automatic background-safe self-update check triggered for interactive CLI runs.
///
/// Respects `[update]` config in `zplint.toml`, `ZPLINT_AUTO_UPDATE=0|false` /
/// `ZPLINT_NO_AUTO_UPDATE=1`, `CI` / `GITHUB_ACTIONS`, rate-limits checks via a
/// timestamp cache (`check_interval_hours`), never writes to stdout, and never
/// fails the caller's command on network or permission issues.
pub fn maybe_auto_update(cfg: &UpdateConfig) {
    if !auto_update_allowed(cfg) {
        return;
    }
    let cache = cache_file_path();
    let now = unix_now_secs();
    if !check_is_due(&cache, cfg.check_interval_hours, now) {
        return;
    }
    record_check(&cache, now);

    let fast = agent(AUTO_CHECK_TIMEOUT);
    let Ok(release) = fetch_release(&fast, None) else {
        return;
    };
    let current = env!("CARGO_PKG_VERSION");
    let latest = release.tag_name.trim_start_matches('v');
    if !is_newer(latest, current) {
        return;
    }

    if cfg.mode == UpdateMode::Notify {
        eprintln!("[zplint] update available: {current} -> {latest} (run `zplint self-update`)");
        return;
    }

    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return;
    };
    if is_cargo_target_exe(&exe) && !env_is_truthy("ZPLINT_AUTO_UPDATE") {
        eprintln!(
            "[zplint] update available: {current} -> {latest} (Cargo target build; run `zplint self-update` to replace)"
        );
        return;
    }

    let dl = agent(DOWNLOAD_TIMEOUT);
    match download_verified(&dl, &release).and_then(|bytes| install(&bytes, &exe)) {
        Ok(()) => {
            eprintln!("[zplint] auto-updated {current} -> {latest} (effective next run)");
        }
        Err(e) => {
            eprintln!(
                "[zplint] update available: {current} -> {latest} (auto-update skipped: {e}; run `zplint self-update`)"
            );
        }
    }
}

fn auto_update_allowed(cfg: &UpdateConfig) -> bool {
    should_auto_update_with(
        cfg.auto,
        std::env::var("ZPLINT_AUTO_UPDATE").ok().as_deref(),
        std::env::var("ZPLINT_NO_AUTO_UPDATE").ok().as_deref(),
        std::env::var("CI").ok().as_deref(),
        std::env::var("GITHUB_ACTIONS").ok().as_deref(),
    )
}

fn should_auto_update_with(
    cfg_auto: bool,
    zplint_auto: Option<&str>,
    zplint_no_auto: Option<&str>,
    ci: Option<&str>,
    github_actions: Option<&str>,
) -> bool {
    if !cfg_auto {
        return false;
    }
    if zplint_no_auto.is_some_and(str_is_truthy) {
        return false;
    }
    if zplint_auto.is_some_and(str_is_falsy) {
        return false;
    }
    if ci.is_some_and(str_is_truthy) || github_actions.is_some_and(str_is_truthy) {
        return false;
    }
    true
}

fn env_is_truthy(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|v| str_is_truthy(&v))
}

fn str_is_truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn str_is_falsy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
}

fn is_cargo_target_exe(exe: &Path) -> bool {
    let raw = exe.to_string_lossy().to_ascii_lowercase();
    let parts: Vec<&str> = raw.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    parts
        .windows(2)
        .any(|w| w[0] == "target" && matches!(w[1], "debug" | "release" | "deps"))
}

fn cache_file_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir).join("zplint").join("last_update_check");
    }
    if let Ok(dir) = std::env::var("LOCALAPPDATA")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir).join("zplint").join("last_update_check");
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".cache")
            .join("zplint")
            .join("last_update_check");
    }
    std::env::temp_dir().join("zplint-last-update-check")
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn check_is_due(cache_path: &Path, interval_hours: u64, now_secs: u64) -> bool {
    if interval_hours == 0 {
        return true;
    }
    let Ok(raw) = std::fs::read_to_string(cache_path) else {
        return true;
    };
    let Ok(last) = raw.trim().parse::<u64>() else {
        return true;
    };
    now_secs.saturating_sub(last) >= interval_hours.saturating_mul(3600)
}

fn record_check(cache_path: &Path, now_secs: u64) {
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(cache_path, now_secs.to_string());
}

fn download_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    Ok(get(agent, url)?.body_mut().read_to_string()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("v0.4.0", "0.3.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn prerelease_suffix_is_ignored() {
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn checksum_parsing() {
        let h = "a".repeat(64);
        assert_eq!(parse_checksum(&format!("{h}  zplint\n")), Some(h.clone()));
        assert_eq!(parse_checksum(&h.to_uppercase()), Some(h));
        assert_eq!(parse_checksum("deadbeef zplint"), None);
    }

    #[test]
    fn auto_update_gates_respect_config_and_env() {
        assert!(should_auto_update_with(true, None, None, None, None));
        assert!(!should_auto_update_with(false, Some("1"), None, None, None));
        assert!(!should_auto_update_with(true, Some("0"), None, None, None));
        assert!(!should_auto_update_with(true, Some("false"), None, None, None));
        assert!(!should_auto_update_with(true, None, Some("1"), None, None));
        assert!(!should_auto_update_with(true, None, None, Some("true"), None));
        assert!(!should_auto_update_with(true, None, None, None, Some("true")));
    }

    #[test]
    fn interval_cache_respects_cooldown() {
        let dir = std::env::temp_dir().join(format!("zplint_upd_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("last_update_check");

        assert!(check_is_due(&cache, 24, 100_000));
        record_check(&cache, 100_000);
        assert!(!check_is_due(&cache, 24, 100_000 + 3600));
        assert!(check_is_due(&cache, 24, 100_000 + 24 * 3600));
        assert!(check_is_due(&cache, 0, 100_001));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_cargo_target_paths() {
        assert!(is_cargo_target_exe(Path::new("/repo/target/release/zplint")));
        assert!(is_cargo_target_exe(Path::new("C:\\repo\\target\\debug\\zplint.exe")));
        assert!(!is_cargo_target_exe(Path::new("/usr/local/bin/zplint")));
        assert!(!is_cargo_target_exe(Path::new("/home/user/.cargo/bin/zplint")));
    }
}
