//! Update notice and `--update`.
//!
//! Each release carries `update-manifest.txt` (version, size and SHA-256 of the exe) and
//! `update-manifest.txt.sig`, an Ed25519 signature made with a private key that never leaves
//! the maintainer's machine. Nothing is believed or installed unless that signature verifies
//! against the public key below. Updates are only ever installed when the user runs `--update`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::state::Shared;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Public half of the release signing key.
const PUBLIC_KEY: [u8; 32] = [
    0x4c, 0x7a, 0x95, 0x14, 0x0b, 0x94, 0x12, 0x38, 0xb5, 0x35, 0xf0, 0x26, 0xed, 0x1d, 0x4f, 0x05,
    0xa0, 0x63, 0xbe, 0x4c, 0x32, 0xf5, 0x34, 0x0e, 0xe5, 0xeb, 0xe2, 0xed, 0x1c, 0x1b, 0xda, 0xba,
];

const RELEASES: &str = "https://github.com/JeremyWhale/solar2d-mcp/releases";
const MANIFEST_HEADER: &str = "solar2d-mcp update manifest 1";
const CHECK_INTERVAL: u64 = 24 * 60 * 60;
const RETRY_INTERVAL: u64 = 60 * 60;
const MAX_EXE_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let out = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(out)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MANIFEST_HEADER) {
        return Err("unrecognised update manifest".into());
    }
    let (mut version, mut sha256, mut size) = (None, None, None);
    for line in lines {
        match line.split_once(": ") {
            Some(("version", v)) if parse_version(v).is_some() => version = Some(v.to_string()),
            Some(("sha256", v)) if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) => {
                sha256 = Some(v.to_ascii_lowercase())
            }
            Some(("size", v)) => size = v.parse::<u64>().ok().filter(|s| *s > 0 && *s <= MAX_EXE_SIZE),
            _ => {}
        }
    }
    match (version, sha256, size) {
        (Some(version), Some(sha256), Some(size)) => Ok(Manifest { version, sha256, size }),
        _ => Err("incomplete update manifest".into()),
    }
}

/// Checks the signature first; the manifest's contents are only read if it is genuine.
fn verify_manifest(manifest: &[u8], signature: &[u8]) -> Result<Manifest, String> {
    let bad = || "the update's signature is not valid, so it was ignored".to_string();
    let key = VerifyingKey::from_bytes(&PUBLIC_KEY).map_err(|_| bad())?;
    let sig = Signature::from_slice(signature).map_err(|_| bad())?;
    key.verify_strict(manifest, &sig).map_err(|_| bad())?;
    parse_manifest(std::str::from_utf8(manifest).map_err(|_| bad())?)
}

/// Test hook. Safe to leave in: whatever it serves must still carry a valid signature.
fn url_override() -> Option<String> {
    std::env::var("SOLAR2D_MCP_UPDATE_URL").ok().filter(|s| !s.is_empty()).map(|s| s.trim_end_matches('/').to_string())
}

fn manifest_url(file: &str) -> String {
    match url_override() {
        Some(base) => format!("{base}/{file}"),
        None => format!("{RELEASES}/latest/download/{file}"),
    }
}

fn exe_url(version: &str) -> String {
    match url_override() {
        Some(base) => format!("{base}/solar2d-mcp.exe"),
        None => format!("{RELEASES}/download/v{version}/solar2d-mcp.exe"),
    }
}

/// The proxy from the environment, else the one set in Windows' Internet settings.
#[cfg(windows)]
fn system_proxy() -> Option<ureq::Proxy> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    if url_override().is_some() {
        return None;
    }
    if let Some(p) = ureq::Proxy::try_from_env() {
        return Some(p);
    }
    let k = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    if k.get_value::<u32, _>("ProxyEnable").ok()? == 0 {
        return None;
    }
    let server: String = k.get_value("ProxyServer").ok()?;
    // either "host:port" or "http=host:port;https=host:port"
    let chosen = if server.contains('=') {
        let find = |scheme: &str| server.split(';').find_map(|p| p.trim().strip_prefix(scheme).map(str::to_string));
        find("https=").or_else(|| find("http="))?
    } else {
        server.clone()
    };
    let url = if chosen.contains("://") { chosen } else { format!("http://{chosen}") };
    ureq::Proxy::new(&url).ok()
}

#[cfg(windows)]
fn http_get(url: &str, limit: u64, timeout: Duration) -> Result<Vec<u8>, String> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    // Windows' own TLS and certificate store, so company proxies with their own root CA work.
    let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!("solar2d-mcp/{VERSION}"))
        .tls_config(tls)
        .proxy(system_proxy())
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| format!("could not download {url}: {e}"))?;
    response.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("could not download {url}: {e}"))
}

#[cfg(not(windows))]
fn http_get(_url: &str, _limit: u64, _timeout: Duration) -> Result<Vec<u8>, String> {
    Err("updating is only available in the Windows build".into())
}

fn fetch_manifest(timeout: Duration) -> Result<Manifest, String> {
    let manifest = http_get(&manifest_url("update-manifest.txt"), 4096, timeout)?;
    let signature = http_get(&manifest_url("update-manifest.txt.sig"), 256, timeout)?;
    verify_manifest(&manifest, &signature)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("solar2d-mcp").join("update-check.txt")
}

/// (when to check next, latest version seen)
fn read_cache() -> (u64, Option<String>) {
    let text = std::fs::read_to_string(cache_file()).unwrap_or_default();
    let mut parts = text.split_whitespace();
    let next = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (next, parts.next().filter(|v| parse_version(v).is_some()).map(str::to_string))
}

fn write_cache(next_check: u64, latest: Option<&str>) {
    let file = cache_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, format!("{next_check} {}\n", latest.unwrap_or("")));
}

pub fn checks_disabled() -> bool {
    std::env::var("SOLAR2D_MCP_NO_UPDATE_CHECK").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

/// At most once a day, on its own thread: never delays or fails the server.
pub fn start_background_check(state: Shared) {
    if checks_disabled() {
        return;
    }
    let (next_check, cached) = read_cache();
    let set = |st: &Shared, v: Option<&str>| {
        *st.update_available.lock().unwrap() = v.filter(|v| is_newer(v, VERSION)).map(str::to_string);
    };
    set(&state, cached.as_deref());
    if now() < next_check {
        return;
    }
    std::thread::spawn(move || match fetch_manifest(Duration::from_secs(15)) {
        Ok(m) => {
            write_cache(now() + CHECK_INTERVAL, Some(&m.version));
            set(&state, Some(&m.version));
        }
        Err(_) => write_cache(now() + RETRY_INTERVAL, cached.as_deref()),
    });
}

/// Text for the agent to pass on, if a newer version is known.
pub fn notice(state: &Shared) -> Option<String> {
    let latest = state.update_available.lock().unwrap().clone()?;
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "solar2d-mcp.exe".into());
    Some(format!(
        "Update available: solar2d-mcp v{latest} (installed: v{VERSION}). Please tell the user. To install it they run: \"{exe}\" --update"
    ))
}

/// Old copies left by earlier updates (they can't be deleted while still running).
pub fn remove_old_copies() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("solar2d-mcp.old") && name.ends_with(".exe") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub fn check_now() -> Result<String, String> {
    let m = fetch_manifest(Duration::from_secs(30))?;
    Ok(if is_newer(&m.version, VERSION) {
        format!("v{} is available (installed: v{VERSION}). Run this program with --update to install it.", m.version)
    } else {
        format!("Up to date (v{VERSION}).")
    })
}

/// Download, verify and swap in the latest release.
pub fn run_update() -> Result<String, String> {
    remove_old_copies();
    let m = fetch_manifest(Duration::from_secs(30))?;
    if !is_newer(&m.version, VERSION) {
        return Ok(format!("Already up to date (v{VERSION})."));
    }
    eprintln!("Downloading solar2d-mcp v{} ({:.1} MB)...", m.version, m.size as f64 / 1_048_576.0);
    let bytes = http_get(&exe_url(&m.version), m.size + 1, Duration::from_secs(300))?;
    if bytes.len() as u64 != m.size {
        return Err("the download is the wrong size, so nothing was changed".into());
    }
    let hash: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hash != m.sha256 {
        return Err("the download doesn't match the signed checksum, so nothing was changed".into());
    }

    let exe = std::env::current_exe().map_err(|e| format!("could not find this program's path: {e}"))?;
    let dir = exe.parent().ok_or("could not find this program's folder")?;
    let denied = |e: std::io::Error| {
        format!("could not write to {} ({e}). If it's a protected folder, run this from an administrator terminal.", dir.display())
    };
    let new = dir.join("solar2d-mcp.new.exe");
    std::fs::write(&new, &bytes).map_err(denied)?;

    // make sure the new program really runs and is the version it claims before swapping
    let ran = std::process::Command::new(&new).arg("--version").output();
    let expected = format!("solar2d-mcp {}", m.version);
    if !ran.as_ref().map(|o| String::from_utf8_lossy(&o.stderr).contains(&expected)).unwrap_or(false) {
        let _ = std::fs::remove_file(&new);
        return Err("the downloaded program didn't start correctly, so nothing was changed".into());
    }

    // a running exe can't be overwritten on Windows, but it can be renamed
    let old = dir.join(format!("solar2d-mcp.old-{}.exe", now()));
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    let _ = std::fs::remove_file(&old); // fails while this copy runs; cleaned up on a later start
    write_cache(now() + CHECK_INTERVAL, Some(&m.version));
    Ok(format!("Updated to v{} (was v{VERSION}). Restart Codex (or your MCP client) to start using it.", m.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("garbage", "0.2.0"));
        assert!(!is_newer("1.0", "0.2.0"));
    }

    #[test]
    fn manifest_parses_and_rejects_bad_input() {
        let hash = "a".repeat(64);
        let good = format!("{MANIFEST_HEADER}\nversion: 1.2.3\nsha256: {hash}\nsize: 42\n");
        assert_eq!(parse_manifest(&good).unwrap(), Manifest { version: "1.2.3".into(), sha256: hash.clone(), size: 42 });
        assert!(parse_manifest("nonsense").is_err());
        assert!(parse_manifest(&format!("{MANIFEST_HEADER}\nversion: 1.2.3\n")).is_err());
        assert!(parse_manifest(&format!("{MANIFEST_HEADER}\nversion: 1.2.3\nsha256: xyz\nsize: 42\n")).is_err());
        assert!(parse_manifest(&format!("{MANIFEST_HEADER}\nversion: 1.2.3\nsha256: {hash}\nsize: 999999999999\n")).is_err());
    }

    #[test]
    fn unsigned_manifests_are_refused() {
        let good = format!("{MANIFEST_HEADER}\nversion: 9.9.9\nsha256: {}\nsize: 42\n", "a".repeat(64));
        assert!(verify_manifest(good.as_bytes(), &[0u8; 64]).is_err());
        assert!(verify_manifest(good.as_bytes(), b"short").is_err());
    }
}
