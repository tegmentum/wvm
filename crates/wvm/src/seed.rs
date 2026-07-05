//! Native management of the protected seed Wasmtime runtime.
//!
//! The seed is the runtime that runs the wvm app itself. It is downloaded once
//! (native HTTP, since no runtime exists yet) and locked: it lives in its own
//! `seed/` directory that wvm's own commands never touch. It can be updated in
//! place with `wvm seed upgrade` (the one path by which the seed changes), so a
//! Wasmtime fix in the runtime that runs everything is remediable.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::cmp::Ordering;
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
    let release = latest_release().context("resolving latest Wasmtime release")?;
    let version = normalize(&release.tag_name);
    eprintln!("wvm: bootstrapping seed runtime (Wasmtime {version}) …");
    let bin = fetch_binary(layout, &release)?;
    place_seed(layout, &version, &bin)?;
    Ok(version)
}

/// `wvm seed upgrade [--check]` — update the seed to the latest Wasmtime. With
/// `check_only`, only report whether a newer release is available.
pub fn upgrade(layout: &Layout, check_only: bool) -> Result<()> {
    let current = installed(layout);
    let release = latest_release().context("resolving latest Wasmtime release")?;
    let version = normalize(&release.tag_name);

    match &current {
        Some(cur) if wvm_core::version_cmp(&version, cur) != Ordering::Greater => {
            println!("Seed runtime is up to date (Wasmtime {cur}).");
            Ok(())
        }
        Some(cur) if check_only => {
            println!(
                "Seed runtime: Wasmtime {cur} → {version} available (run `wvm seed upgrade`)."
            );
            Ok(())
        }
        Some(cur) => {
            let bin = fetch_binary(layout, &release)?;
            place_seed(layout, &version, &bin)?;
            println!("Upgraded seed runtime: Wasmtime {cur} → {version}.");
            Ok(())
        }
        None if check_only => {
            println!("No seed runtime installed yet (first run installs Wasmtime {version}).");
            Ok(())
        }
        None => {
            let bin = fetch_binary(layout, &release)?;
            place_seed(layout, &version, &bin)?;
            println!("Installed seed runtime: Wasmtime {version}.");
            Ok(())
        }
    }
}

/// `wvm seed status` — show the installed seed version and whether a newer one
/// is available (best-effort; skips the network check on failure).
pub fn status(layout: &Layout) -> Result<()> {
    match installed(layout) {
        Some(v) => {
            println!("Seed runtime: Wasmtime {v}");
            println!("  path: {}", layout.seed_bin().display());
            match latest_release() {
                Ok(rel) => {
                    let latest = normalize(&rel.tag_name);
                    if wvm_core::version_cmp(&latest, &v) == Ordering::Greater {
                        println!("  update available: {latest} (run `wvm seed upgrade`)");
                    } else {
                        println!("  up to date");
                    }
                }
                Err(_) => println!("  (could not check for updates)"),
            }
        }
        None => println!("No seed runtime installed (it is downloaded on first use)."),
    }
    Ok(())
}

/// The installed seed version, if locked.
pub fn installed(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.seed_marker()).ok()?;
    let v = text.trim();
    (!v.is_empty() && layout.seed_bin().is_file()).then(|| v.to_string())
}

fn normalize(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

/// Download, checksum-verify, and extract the `bin/wasmtime` from a release.
fn fetch_binary(layout: &Layout, release: &Release) -> Result<Vec<u8>> {
    let platform = Platform::detect()?;
    let version = normalize(&release.tag_name);
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

    let files = archive::extract_tar_xz(&archive_path)?;
    let bin = files
        .iter()
        .find(|f| f.logical_path == "bin/wasmtime")
        .ok_or_else(|| anyhow!("seed archive did not contain bin/wasmtime"))?
        .data
        .clone();
    let _ = std::fs::remove_file(&archive_path);
    Ok(bin)
}

/// Write the seed binary and lock it (replacing any existing, read-only seed).
fn place_seed(layout: &Layout, version: &str, bin: &[u8]) -> Result<()> {
    let seed_bin = layout.seed_bin();
    std::fs::create_dir_all(seed_bin.parent().unwrap())?;
    // The existing binary may be read-only; the directory is writable, so a
    // remove-then-write replaces it cleanly.
    let _ = std::fs::remove_file(&seed_bin);
    std::fs::write(&seed_bin, bin).with_context(|| format!("writing {}", seed_bin.display()))?;
    set_executable(&seed_bin)?;
    std::fs::write(layout.seed_marker(), version)?;
    make_readonly(&seed_bin);
    Ok(())
}

fn latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("requesting {url}"))?
        .into_string()?;
    serde_json::from_str(&body).context("parsing release JSON")
}

fn download(url: &str, dest: &Path) -> Result<u64> {
    let resp = agent()
        .get(url)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    std::fs::write(dest, &buf)?;
    Ok(buf.len() as u64)
}

/// A ureq agent honoring `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` (and lowercase),
/// so native downloads work behind a corporate proxy.
fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().user_agent(USER_AGENT);
    if let Some(url) = proxy_from_env() {
        if let Ok(proxy) = ureq::Proxy::new(&url) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

fn proxy_from_env() -> Option<String> {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .into_iter()
    .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
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
