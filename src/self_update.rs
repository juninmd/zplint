//! `zplint self-update`: replace the running binary with the latest GitHub release.
//!
//! The release workflow publishes a raw binary plus a `.sha256` file per target, so the
//! updater needs no archive support: download, verify the checksum, swap the file.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

const REPO: &str = "juninmd/zplint";
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;

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

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
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
    let agent = agent();
    let release = fetch_release(&agent, version)?;
    let latest = release.tag_name.trim_start_matches('v');

    if version.is_none() && !is_newer(latest, current) {
        println!("zplint {current} is up to date");
        return Ok(0);
    }
    if check {
        println!("update available: {current} -> {latest} (run `zplint self-update`)");
        return Ok(1);
    }

    let Some(name) = asset_name() else {
        bail!("no prebuilt binary for this platform; use `cargo install zplint --force`");
    };
    let find = |n: &str| release.assets.iter().find(|a| a.name == n);
    let bin = find(name).with_context(|| format!("release v{latest} has no asset {name}"))?;
    let sum = find(&format!("{name}.sha256"))
        .with_context(|| format!("release v{latest} has no checksum for {name}"))?;

    let expected = parse_checksum(&download_text(&agent, &sum.browser_download_url)?)
        .context("malformed checksum file")?;
    let bytes = download(&agent, &bin.browser_download_url)?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    install(&bytes, &exe)?;
    println!("updated zplint {current} -> {latest}");
    Ok(0)
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
}
