//! `wvm install` — executed inside the wasm app, downloading via `wasi:http`
//! and storing through the CAS. Uses `copy` materialization (symlinks are
//! unavailable under wasm; the store still deduplicates).

use crate::commands::{open_index, seed_version};
use crate::http_wasi::WasiHttp;
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
}

fn now_epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn resolve_release(http: &WasiHttp, version_arg: &str) -> Result<Release> {
    let url = if version_arg == "latest" {
        format!("https://api.github.com/repos/{REPO}/releases/latest")
    } else {
        format!(
            "https://api.github.com/repos/{REPO}/releases/tags/v{}",
            normalize_version(version_arg)
        )
    };
    let body = http
        .get_string(&url)
        .with_context(|| format!("fetching release metadata for {version_arg}"))?;
    serde_json::from_str(&body).context("parsing release JSON")
}

pub fn install(version_arg: &str, set_use: bool) -> Result<()> {
    let layout = Layout::discover()?;
    layout.ensure_base()?;
    let platform = Platform::detect()?;
    let http = WasiHttp;

    let release = resolve_release(&http, version_arg)?;
    let version = normalize_version(&release.tag_name);

    if seed_version(&layout).as_deref() == Some(version.as_str()) {
        println!("wasmtime {version} is already present as the protected seed runtime");
    }

    if is_installed(&layout, &version) {
        println!("wasmtime {version} is already installed");
        if set_use {
            discovery::set_active_version(&layout, &version)?;
        }
        return Ok(());
    }

    let asset_name = platform.asset_name(&version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("no asset {asset_name} in release v{version}"))?;

    println!("Downloading {asset_name} …");
    let download_path = layout.downloads_dir().join(&asset_name);
    http.download(&asset.browser_download_url, &download_path)?;

    let archive_sha256 = hash::sha256_file(&download_path)?;
    match asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        Some(expected) if expected.to_lowercase() != archive_sha256 => {
            let _ = std::fs::remove_file(&download_path);
            bail!("checksum mismatch for {asset_name}: expected {expected}, got {archive_sha256}");
        }
        Some(_) => println!("Verified checksum ({}…)", &archive_sha256[..12]),
        None => eprintln!("warning: no published checksum for {asset_name}"),
    }

    println!("Extracting and storing files …");
    let files = archive::extract_tar_xz(&download_path)?;

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
    for f in &files {
        let digest = hash::sha256_hex(&f.data);
        if store::has(&layout, &digest) {
            deduped += 1;
        }
        let object = store::put(&layout, &digest, &f.data, f.mode)?;
        // `copy` materialization: symlinks are unavailable under wasm.
        materialize::materialize(Materialization::Copy, &staging, &f.logical_path, &object, f.mode)?;
        entries.push(FileEntry {
            path: f.logical_path.clone(),
            sha256: digest,
            mode: mode_string(f.mode),
            size: f.data.len() as u64,
        });
    }

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

    if set_use || discovery::active_version(&layout).is_none() {
        discovery::set_active_version(&layout, &version)?;
    }
    Ok(())
}

fn is_installed(layout: &Layout, version: &str) -> bool {
    layout.manifest_file(WASMTIME, version).is_file()
}
