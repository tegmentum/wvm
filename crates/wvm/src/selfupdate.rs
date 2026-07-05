//! Native in-place self-upgrade of the `wvm` binary.
//!
//! `wvm` embeds its wasm app, so upgrading the *tool* means replacing this
//! native executable — distinct from `wvm upgrade <spec>`, which upgrades the
//! managed Wasmtime runtimes. We reuse the same GitHub-release + checksum path
//! as the seed downloader (native HTTP via `ureq`, since this runs before any
//! runtime is bootstrapped), fetch the release asset for this host, verify it,
//! and atomically rename it over the running binary. On Linux/macOS a running
//! process keeps its original inode, so replacing the on-disk file is safe.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use wvm_core::cache;
use wvm_core::layout::Layout;

/// Default source repo; overridable with `WVM_REPO` (mirrors install.sh).
const DEFAULT_REPO: &str = "tegmentum/wvm";
const USER_AGENT: &str = concat!("wvm/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Throttle state for the background update notice: when we last asked GitHub
/// for the latest release, and what it was.
#[derive(Serialize, Deserialize)]
struct CheckState {
    checked_at: i64,
    latest: String,
}

fn repo() -> String {
    std::env::var("WVM_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

/// Release asset base name for this host, matching install.sh (`wvm-<arch>-<os>`).
fn asset_name() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture for self-upgrade: {other}"),
    };
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => bail!("unsupported OS for self-upgrade: {other}"),
    };
    Ok(format!("wvm-{arch}-{os}"))
}

/// `wvm --upgrade [--check]` — update the wvm binary to the latest release.
/// With `check_only`, report whether a newer release exists without installing.
pub fn run(check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let release = latest_release().context("checking for the latest wvm release")?;
    let latest = wvm_core::normalize_version(release.tag_name.trim());

    // Already current (or ahead, e.g. a local dev build): nothing to do.
    if wvm_core::version_cmp(current, &latest) != std::cmp::Ordering::Less {
        println!("wvm {current} is already up to date (latest release: {latest})");
        return Ok(());
    }

    if check_only {
        println!("a newer wvm is available: {current} -> {latest}");
        println!("run `wvm --upgrade` to install it");
        return Ok(());
    }

    let asset_name = asset_name()?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow!("release {latest} has no asset {asset_name} for this host"))?;

    eprintln!("wvm: upgrading {current} -> {latest} …");
    let bytes = download(&asset.browser_download_url)
        .with_context(|| format!("downloading {asset_name}"))?;

    // Verify against the published `.sha256` sidecar asset when present.
    if let Some(sum) = release
        .assets
        .iter()
        .find(|a| a.name == format!("{asset_name}.sha256"))
    {
        let text = download_text(&sum.browser_download_url)
            .with_context(|| format!("downloading {asset_name}.sha256"))?;
        let expected = text.split_whitespace().next().unwrap_or("").to_lowercase();
        if !expected.is_empty() {
            let observed = wvm_core::hash::sha256_hex(&bytes);
            if observed != expected {
                bail!("checksum mismatch for {asset_name}: expected {expected}, got {observed}");
            }
        }
    }

    replace_self(&bytes)?;
    println!("wvm upgraded to {latest}");
    Ok(())
}

/// Best-effort, throttled "a newer wvm is available" notice on stderr.
///
/// Called on ordinary `wvm` management commands (never on the `exec` hot path).
/// The network is touched at most once per `WVM_REFRESH_INTERVAL` (default 1h),
/// reusing the same throttle knob as runtime resolution; between refreshes the
/// last-known latest version is compared from a small state file. Any failure —
/// offline, parse error, unwritable cache — is swallowed so the actual command
/// is never disrupted. Set `WVM_NO_UPDATE_NOTIFIER` to disable entirely.
pub fn notify(layout: &Layout) {
    if std::env::var_os("WVM_NO_UPDATE_NOTIFIER").is_some() {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let now = now_epoch();
    let ttl = cache::refresh_interval();
    let state = read_check(layout);

    let fresh = matches!(&state, Some(s) if ttl > 0 && now.saturating_sub(s.checked_at) < ttl);

    let latest = if fresh || ttl == 0 {
        // Fresh, or offline mode (`WVM_REFRESH_INTERVAL=0`): never hit the
        // network; nag only from a previously cached value, if any.
        state.as_ref().map(|s| s.latest.clone())
    } else {
        // Stale and online: refresh in the foreground, falling back to any
        // cached value if the check fails.
        match latest_release() {
            Ok(r) => {
                let v = wvm_core::normalize_version(r.tag_name.trim());
                let _ = write_check(layout, now, &v);
                Some(v)
            }
            Err(_) => state.as_ref().map(|s| s.latest.clone()),
        }
    };

    if let Some(latest) = latest {
        if !latest.is_empty() && wvm_core::version_cmp(current, &latest) == std::cmp::Ordering::Less
        {
            eprintln!(
                "wvm: a newer version is available ({current} -> {latest}); run `wvm --upgrade`"
            );
        }
    }
}

fn check_file(layout: &Layout) -> PathBuf {
    layout.cache_dir().join("update-check.json")
}

fn read_check(layout: &Layout) -> Option<CheckState> {
    let text = std::fs::read_to_string(check_file(layout)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_check(layout: &Layout, now: i64, latest: &str) -> Result<()> {
    let path = check_file(layout);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let state = CheckState {
        checked_at: now,
        latest: latest.to_string(),
    };
    std::fs::write(&path, serde_json::to_string(&state)?)?;
    Ok(())
}

fn now_epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Fetch the latest release metadata for the wvm repo.
fn latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let body = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("requesting {url}"))?
        .into_string()?;
    serde_json::from_str(&body).context("parsing release JSON")
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf)?;
    Ok(buf)
}

fn download_text(url: &str) -> Result<String> {
    ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("downloading {url}"))?
        .into_string()
        .map_err(Into::into)
}

/// Atomically replace the currently-running binary with `bytes`: write a temp
/// file beside it, make it executable, then rename over the original. The
/// rename is on the same filesystem (same directory), so it is atomic and
/// leaves no window with a half-written binary.
fn replace_self(bytes: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("locating the running wvm binary")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("wvm binary path has no parent directory"))?;
    let tmp = dir.join(".wvm.upgrade");

    std::fs::write(&tmp, bytes).with_context(|| {
        format!(
            "writing {} (need write access to {})",
            tmp.display(),
            dir.display()
        )
    })?;
    set_executable(&tmp)?;

    std::fs::rename(&tmp, &exe).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!(
            "replacing {}; if it is system-owned, re-run the install script or use sudo",
            exe.display()
        )
    })?;
    Ok(())
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
