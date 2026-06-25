//! Native management of the protected seed Wasmtime runtime.
//!
//! The seed is the runtime that runs the wvm app itself. It is downloaded once
//! (native HTTP, since no runtime exists yet) and locked: it lives in its own
//! `seed/` directory that wvm's own commands never touch.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use wvm_core::archive;
use wvm_core::layout::Layout;
use wvm_core::platform::Platform;

const REPO: &str = "bytecodealliance/wasmtime";
const USER_AGENT: &str = concat!("wvm/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

/// Ensure the seed runtime exists and is locked. Returns its version.
pub fn ensure(layout: &Layout) -> Result<String> {
    if let Some(version) = installed(layout) {
        return Ok(version);
    }

    let platform = Platform::detect()?;
    let release = latest_release().context("resolving latest Wasmtime release")?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let asset_name = platform.asset_name(&version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            anyhow!(
                "no Wasmtime asset {asset_name} for host {}",
                platform.label()
            )
        })?;

    eprintln!("wvm: bootstrapping seed runtime (Wasmtime {version}) …");
    let downloads = layout.downloads_dir();
    std::fs::create_dir_all(&downloads)?;
    let archive_path = downloads.join(&asset_name);
    download(&asset.browser_download_url, &archive_path)?;

    // Verify against GitHub's published digest when present.
    let observed = wvm_core::hash::sha256_file(&archive_path)?;
    if let Some(expected) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
    {
        if expected.to_lowercase() != observed {
            let _ = std::fs::remove_file(&archive_path);
            bail!("seed checksum mismatch: expected {expected}, got {observed}");
        }
    }

    // Extract and place just the wasmtime binary into the seed dir.
    let files = archive::extract_tar_xz(&archive_path)?;
    let bin = files
        .iter()
        .find(|f| f.logical_path == "bin/wasmtime")
        .ok_or_else(|| anyhow!("seed archive did not contain bin/wasmtime"))?;

    let seed_bin = layout.seed_bin();
    std::fs::create_dir_all(seed_bin.parent().unwrap())?;
    std::fs::write(&seed_bin, &bin.data)
        .with_context(|| format!("writing {}", seed_bin.display()))?;
    set_executable(&seed_bin)?;

    std::fs::write(layout.seed_marker(), &version)?;
    let _ = std::fs::remove_file(&archive_path);

    // Best-effort: make the seed binary read-only to discourage tampering.
    make_readonly(&seed_bin);

    Ok(version)
}

/// The installed seed version, if locked.
pub fn installed(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.seed_marker()).ok()?;
    let v = text.trim();
    (!v.is_empty() && layout.seed_bin().is_file()).then(|| v.to_string())
}

fn latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("requesting {url}"))?
        .into_string()?;
    serde_json::from_str(&body).context("parsing release JSON")
}

fn download(url: &str, dest: &Path) -> Result<u64> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    std::fs::write(dest, &buf)?;
    Ok(buf.len() as u64)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_readonly(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555));
}

#[cfg(not(unix))]
fn make_readonly(_path: &Path) {}
