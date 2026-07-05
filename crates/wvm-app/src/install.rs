//! `wvm install` — executed inside the wasm app, downloading via `wasi:http`
//! and extracting the runtime files directly into its version directory.

use crate::commands::seed_version;
use crate::http_wasi::WasiHttp;
use crate::progress::Spinner;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::cmp::Ordering;
use std::path::Path;
use wvm_core::http::Http;
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::{mode_string, FileEntry, Manifest};
use wvm_core::platform::Platform;
use wvm_core::{archive, cache, discovery, hash, normalize_version, version_cmp, VersionSpec};

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
    let spec = VersionSpec::parse(version_arg).map_err(|e| anyhow!(e))?;
    let url = match &spec {
        // The `latest` endpoint reflects the true newest release directly.
        VersionSpec::Latest => format!("https://api.github.com/repos/{REPO}/releases/latest"),
        // Every other spec resolves against the available list, then we fetch
        // that concrete tag.
        _ => {
            let available = fetch_release_versions(false)?;
            let version = spec
                .resolve(&available)
                .ok_or_else(|| anyhow!("no available wasmtime version matches '{spec}'"))?;
            format!("https://api.github.com/repos/{REPO}/releases/tags/v{version}")
        }
    };
    let body = http
        .get_string(&url)
        .with_context(|| format!("fetching release metadata for {version_arg}"))?;
    serde_json::from_str(&body).context("parsing release JSON")
}

/// Ensure the newest version matching `spec_str` is installed, auto-installing
/// it if absent, and return the concrete version. Floating specs may consult
/// the network (bounded by the release cache); an exact spec installs that
/// version if missing. Offline, it falls back to the best installed match.
pub fn ensure(spec_str: &str) -> Result<String> {
    let layout = Layout::discover()?;
    let spec = VersionSpec::parse(spec_str).map_err(|e| anyhow!(e))?;

    let installed = discovery::installed_versions(&layout)?;
    let installed_best = spec.resolve(&installed).map(str::to_string);

    // Best match available remotely (cached); ignore network failures.
    let remote_best = match fetch_release_versions(false) {
        Ok(list) => spec.resolve(&list).map(str::to_string),
        Err(_) => None,
    };

    let target = match (remote_best, installed_best) {
        (Some(r), Some(i)) => {
            if version_cmp(&r, &i) == Ordering::Greater {
                r
            } else {
                i
            }
        }
        (Some(r), None) => r,
        (None, Some(i)) => i,
        (None, None) => bail!("no wasmtime version matches '{spec}' (and none is installed)"),
    };

    if !is_installed(&layout, &target) {
        // The caller (default/use/exec) manages default+session itself, so
        // suppress install's first-install-becomes-default side effect.
        install_inner(&target, false, false)?;
    }
    Ok(target)
}

/// `wvm ls-remote [--all]` — list versions available from the Wasmtime GitHub
/// Fetch versions available from the Wasmtime GitHub releases (first page, most
/// recent first), as version strings. By default only stable releases with a
/// build for this host are returned; `all` includes prereleases and versions
/// without a host asset.
pub fn fetch_release_versions(all: bool) -> Result<Vec<String>> {
    let layout = Layout::discover()?;
    let now = now_epoch();
    let ttl = cache::refresh_interval();

    // Serve from cache while fresh. `ttl == 0` means "stay offline": prefer any
    // cached list regardless of age, fetching only when there is none at all.
    if let Some(c) = cache::read(&layout, all) {
        if ttl == 0 || c.is_fresh(now, ttl) {
            return Ok(c.versions);
        }
    }

    let platform = Platform::detect()?;
    let http = WasiHttp;

    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=100");
    let body = http
        .get_string_with_progress(&url, "Fetching available versions")
        .context("fetching release list")?;
    let releases: Vec<Release> = serde_json::from_str(&body).context("parsing release list")?;

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

    // Best-effort: persist for the refresh interval so activation-time
    // auto-install need not re-fetch on every invocation.
    let _ = cache::write(&layout, all, &out, now);
    Ok(out)
}

/// `wvm install <spec>` — resolve and install a runtime. When `auto_default` is
/// set and no default exists yet, the first install becomes the default (the
/// convenience for a bare `wvm install`); callers that manage the default
/// themselves pass `false`.
pub fn install(version_arg: &str, make_default: bool) -> Result<()> {
    install_inner(version_arg, make_default, true)
}

/// Store the default as the *spec* the user asked for — so `install 24 --default`
/// floats exactly like `wvm default 24` — while reporting the concrete version
/// it resolves to now.
fn store_default_spec(layout: &Layout, version_arg: &str, resolved: &str) -> Result<()> {
    let spec = VersionSpec::parse(version_arg)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| resolved.to_string());
    discovery::set_default_version(layout, &spec)?;
    if spec != resolved {
        println!("Default is now '{spec}' (currently wasmtime {resolved}, used by new shells)");
    } else {
        println!("Default is now wasmtime {resolved} (used by new shells)");
    }
    Ok(())
}

fn install_inner(version_arg: &str, make_default: bool, auto_default: bool) -> Result<()> {
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
            store_default_spec(&layout, version_arg, &version)?;
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

    let count = materialize_install(&layout, &version, &platform, &download_path, archive_sha256)?;
    let _ = std::fs::remove_file(&download_path);
    println!("Installed wasmtime {version} ({count} files)");

    // The first runtime installed becomes the default; otherwise honor
    // --default/--use.
    if make_default || (auto_default && discovery::default_version(&layout).is_none()) {
        store_default_spec(&layout, version_arg, &version)?;
    }
    Ok(())
}

/// `wvm install <version> --from <archive>` — install from a local wasmtime
/// `.tar.xz` without any network. The version must be exact (there is nothing to
/// resolve a spec against offline); the archive must match this host.
pub fn install_from(version_arg: &str, archive: &str, make_default: bool) -> Result<()> {
    let layout = Layout::discover()?;
    layout.ensure_base()?;
    let platform = Platform::detect()?;

    let version = match VersionSpec::parse(version_arg).map_err(|e| anyhow!(e))? {
        VersionSpec::Exact(v) => v,
        _ => bail!(
            "install --from requires an exact version, e.g. `wvm install 25.0.0 --from <archive>`"
        ),
    };
    if is_installed(&layout, &version) {
        println!("wasmtime {version} is already installed");
        if make_default {
            store_default_spec(&layout, version_arg, &version)?;
        }
        return Ok(());
    }

    let archive_path = Path::new(archive);
    if !archive_path.is_file() {
        bail!("archive not found: {archive}");
    }
    let archive_sha256 = hash::sha256_file(archive_path)?;
    let count = materialize_install(&layout, &version, &platform, archive_path, archive_sha256)?;
    println!("Installed wasmtime {version} from {archive} ({count} files)");

    if make_default || discovery::default_version(&layout).is_none() {
        store_default_spec(&layout, version_arg, &version)?;
    }
    Ok(())
}

/// Extract `archive_path` into the version directory, record each file's digest
/// in `manifest.json`, and publish it atomically. Returns the file count. The
/// exec bit is not set here — the app is wasm/non-unix; the native bootstrapper
/// restores it at run time. Does not delete the archive.
fn materialize_install(
    layout: &Layout,
    version: &str,
    platform: &Platform,
    archive_path: &Path,
    archive_sha256: String,
) -> Result<usize> {
    let extract = Spinner::new("Extracting archive");
    let files = archive::extract_tar_xz(archive_path)?;
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
    let mut store_sp = Spinner::new("Writing files");
    for (i, f) in files.iter().enumerate() {
        let dest = staging.join(&f.logical_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&dest, &f.data).with_context(|| format!("writing {}", dest.display()))?;
        entries.push(FileEntry {
            path: f.logical_path.clone(),
            sha256: hash::sha256_hex(&f.data),
            mode: mode_string(f.mode),
            size: f.data.len() as u64,
        });
        store_sp.tick(&format!("{}/{}", i + 1, files.len()));
    }
    store_sp.finish(&format!("Wrote {} files", files.len()));

    let manifest = Manifest {
        runtime: WASMTIME.to_string(),
        version: version.to_string(),
        platform: platform.label(),
        archive_sha256,
        files: entries,
    };
    manifest.write(&staging.join("manifest.json"))?;

    let final_dir = layout.version_dir(WASMTIME, version);
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }
    std::fs::rename(&staging, &final_dir)
        .with_context(|| format!("publishing {}", final_dir.display()))?;
    Ok(files.len())
}

fn is_installed(layout: &Layout, version: &str) -> bool {
    layout.manifest_file(WASMTIME, version).is_file()
}
