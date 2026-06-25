//! `wvm install` — executed inside the wasm app, downloading via `wasi:http`
//! and storing through the CAS. Uses `copy` materialization (symlinks are
//! unavailable under wasm; the store still deduplicates).

use crate::commands::{open_index, seed_version};
use crate::http_wasi::WasiHttp;
use crate::progress::Spinner;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use wvm_core::config::Materialization;
use wvm_core::http::Http;
use wvm_core::index::Index;
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::{mode_string, FileEntry, Manifest};
use wvm_core::platform::Platform;
use wvm_core::{archive, discovery, hash, materialize, normalize_version, store};

const REPO: &str = "bytecodealliance/wasmtime";

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
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

fn now_epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn resolve_release(http: &WasiHttp, version_arg: &str) -> Result<Release> {
    let url = match version_arg {
        "latest" => format!("https://api.github.com/repos/{REPO}/releases/latest"),
        "lts" => {
            // Newest LTS available for this host.
            let lts = fetch_release_versions(false)?
                .into_iter()
                .find(|v| wvm_core::is_lts(v))
                .context("no LTS release found for this platform")?;
            format!("https://api.github.com/repos/{REPO}/releases/tags/v{lts}")
        }
        v => format!(
            "https://api.github.com/repos/{REPO}/releases/tags/v{}",
            normalize_version(v)
        ),
    };
    let body = http
        .get_string(&url)
        .with_context(|| format!("fetching release metadata for {version_arg}"))?;
    serde_json::from_str(&body).context("parsing release JSON")
}

/// `wvm ls-remote [--all]` — list versions available from the Wasmtime GitHub
/// Fetch versions available from the Wasmtime GitHub releases (first page, most
/// recent first), as version strings. By default only stable releases with a
/// build for this host are returned; `all` includes prereleases and versions
/// without a host asset.
pub fn fetch_release_versions(all: bool) -> Result<Vec<String>> {
    let platform = Platform::detect()?;
    let http = WasiHttp;

    let sp = Spinner::new("Fetching available versions");
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=100");
    let body = http.get_string(&url).context("fetching release list")?;
    let releases: Vec<Release> = serde_json::from_str(&body).context("parsing release list")?;
    sp.finish(&format!("Fetched {} releases", releases.len()));

    let mut out = Vec::new();
    for r in &releases {
        if r.draft {
            continue;
        }
        if r.prerelease && !all {
            continue;
        }
        let version = normalize_version(&r.tag_name);
        let has_build = r
            .assets
            .iter()
            .any(|a| a.name == platform.asset_name(&version));
        if !has_build && !all {
            continue;
        }
        out.push(version);
    }
    Ok(out)
}

pub fn install(version_arg: &str, make_default: bool) -> Result<()> {
    let layout = Layout::discover()?;
    layout.ensure_base()?;
    let platform = Platform::detect()?;
    let http = WasiHttp;

    let sp = Spinner::new("Resolving release");
    let release = resolve_release(&http, version_arg)?;
    let version = normalize_version(&release.tag_name);
    sp.finish(&format!("Resolved wasmtime {version}"));

    if seed_version(&layout).as_deref() == Some(version.as_str()) {
        println!("wasmtime {version} is already present as the protected seed runtime");
    }

    if is_installed(&layout, &version) {
        println!("wasmtime {version} is already installed");
        if make_default {
            discovery::set_default_version(&layout, &version)?;
            println!("Default is now wasmtime {version}");
        }
        return Ok(());
    }

    let asset_name = platform.asset_name(&version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("no asset {asset_name} in release v{version}"))?;

    let download_path = layout.downloads_dir().join(&asset_name);
    http.download_with_progress(
        &asset.browser_download_url,
        &download_path,
        &format!("Downloading {asset_name}"),
    )?;

    let archive_sha256 = hash::sha256_file(&download_path)?;
    match asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
    {
        Some(expected) if expected.to_lowercase() != archive_sha256 => {
            let _ = std::fs::remove_file(&download_path);
            bail!("checksum mismatch for {asset_name}: expected {expected}, got {archive_sha256}");
        }
        Some(_) => eprintln!("✓ Verified checksum ({}…)", &archive_sha256[..12]),
        None => eprintln!("warning: no published checksum for {asset_name}"),
    }

    let extract = Spinner::new("Extracting archive");
    let files = archive::extract_tar_xz(&download_path)?;
    extract.finish(&format!("Extracted {} files", files.len()));

    let staging = layout
        .versions_dir(WASMTIME)
        .join(format!(".staging-{version}"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;

    let mut entries = Vec::new();
    let mut deduped = 0usize;
    let mut store_sp = Spinner::new("Storing files");
    for (i, f) in files.iter().enumerate() {
        let digest = hash::sha256_hex(&f.data);
        if store::has(&layout, &digest) {
            deduped += 1;
        }
        let object = store::put(&layout, &digest, &f.data, f.mode)?;
        // `copy` materialization: symlinks are unavailable under wasm.
        materialize::materialize(
            Materialization::Copy,
            &staging,
            &f.logical_path,
            &object,
            f.mode,
        )?;
        entries.push(FileEntry {
            path: f.logical_path.clone(),
            sha256: digest,
            mode: mode_string(f.mode),
            size: f.data.len() as u64,
        });
        store_sp.tick(&format!("{}/{}", i + 1, files.len()));
    }
    store_sp.finish(&format!(
        "Stored {} files ({deduped} reused from store)",
        files.len()
    ));

    let manifest = Manifest {
        runtime: WASMTIME.to_string(),
        version: version.clone(),
        platform: platform.label(),
        archive_sha256,
        materialization: Materialization::Copy.as_str().to_string(),
        files: entries,
    };
    manifest.write(&staging.join("manifest.json"))?;

    let final_dir = layout.version_dir(WASMTIME, &version);
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }
    std::fs::rename(&staging, &final_dir)
        .with_context(|| format!("publishing {}", final_dir.display()))?;
    let _ = std::fs::remove_file(&download_path);

    println!(
        "Installed wasmtime {version} ({} files, {deduped} reused from store)",
        files.len()
    );

    if let Err(e) = (|| open_index(&layout)?.record_install(&manifest, now_epoch()))() {
        eprintln!("warning: could not update index: {e:#}");
    }

    // The first runtime installed becomes the default; otherwise honor
    // --default/--use.
    if make_default || discovery::default_version(&layout).is_none() {
        discovery::set_default_version(&layout, &version)?;
        println!("Default is now wasmtime {version}");
    }
    Ok(())
}

fn is_installed(layout: &Layout, version: &str) -> bool {
    layout.manifest_file(WASMTIME, version).is_file()
}
