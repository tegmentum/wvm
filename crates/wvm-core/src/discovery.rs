//! Runtime discovery: project pin, session override, default, env, PATH.
//!
//! Two layers of selection:
//! - **default** — persistent (`runtimes/wasmtime/default`), used by new shells.
//! - **session** — the `WVM_VERSION` environment variable, set per shell, which
//!   overrides the default for the current session only.

use crate::layout::{Layout, WASMTIME};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Project pin file name, searched upward from the working directory.
pub const PIN_FILE: &str = "wvm.toml";

/// Environment variable carrying the per-session version override.
pub const SESSION_VAR: &str = "WVM_VERSION";

#[derive(Debug, Default, Deserialize)]
struct PinFile {
    wvm: Option<PinSection>,
}

#[derive(Debug, Default, Deserialize)]
struct PinSection {
    runtime: Option<String>,
}

/// A resolved wasmtime binary plus where it came from.
#[derive(Debug)]
pub struct Resolved {
    pub binary: PathBuf,
    pub source: String,
}

/// Find a project pin by walking up from `start`.
pub fn find_pin(start: &Path) -> Result<Option<(String, PathBuf)>> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(PIN_FILE);
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let parsed: PinFile = toml::from_str(&text)
                .with_context(|| format!("parsing {}", candidate.display()))?;
            if let Some(runtime) = parsed.wvm.and_then(|w| w.runtime) {
                return Ok(Some((runtime, candidate)));
            }
        }
        dir = d.parent();
    }
    Ok(None)
}

/// The persistent default version, if set.
pub fn default_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.default_file(WASMTIME)).ok()?;
    let v = text.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Write the persistent default version.
pub fn set_default_version(layout: &Layout, version: &str) -> Result<()> {
    let path = layout.default_file(WASMTIME);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, version)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The per-session override (`WVM_VERSION`), if set.
pub fn session_version() -> Option<String> {
    match std::env::var(SESSION_VAR) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

/// The effective selected version and where it came from: session overrides
/// default.
pub fn effective_version(layout: &Layout) -> Option<(String, &'static str)> {
    if let Some(v) = session_version() {
        return Some((v, "session"));
    }
    default_version(layout).map(|v| (v, "default"))
}

fn binary_in_version(layout: &Layout, version: &str) -> PathBuf {
    layout.version_dir(WASMTIME, version).join("bin").join("wasmtime")
}

/// Resolve a wasmtime binary following the discovery order:
/// project pin → session (`WVM_VERSION`) → default → `WASMTIME_HOME` → PATH.
pub fn resolve(layout: &Layout, cwd: &Path) -> Result<Resolved> {
    // 1. Project pin
    if let Some((version, file)) = find_pin(cwd)? {
        let bin = binary_in_version(layout, &version);
        if bin.exists() {
            return Ok(Resolved {
                binary: bin,
                source: format!("project pin ({}) -> {version}", file.display()),
            });
        }
        bail!(
            "project pins wasmtime {version} (from {}) but it is not installed; run `wvm install {version}`",
            file.display()
        );
    }

    // 2. Session override, then 3. default.
    for (version, src) in [
        session_version().map(|v| (v, "session")),
        default_version(layout).map(|v| (v, "default")),
    ]
    .into_iter()
    .flatten()
    {
        let bin = binary_in_version(layout, &version);
        if bin.exists() {
            return Ok(Resolved { binary: bin, source: format!("{src} ({version})") });
        }
    }

    // 4. Explicit environment variable (path-based)
    for var in ["WASM_RUNTIME_HOME", "WASMTIME_HOME"] {
        if let Some(val) = std::env::var_os(var) {
            if val.is_empty() {
                continue;
            }
            let p = PathBuf::from(val);
            for candidate in [p.join("bin").join("wasmtime"), p.clone()] {
                if candidate.is_file() {
                    return Ok(Resolved { binary: candidate, source: format!("${var}") });
                }
            }
        }
    }

    // 5. System runtime / PATH lookup
    if let Some(bin) = which("wasmtime") {
        return Ok(Resolved { binary: bin, source: "PATH".to_string() });
    }

    bail!("no wasmtime runtime found; try `wvm install latest` then `wvm default latest`")
}

/// Minimal PATH lookup for an executable.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
