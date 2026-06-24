//! Runtime discovery: project pin, active runtime, env override, system/PATH.

use crate::layout::{Layout, WASMTIME};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Project pin file name, searched upward from the working directory.
pub const PIN_FILE: &str = "wvm.toml";

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

/// Find a project pin by walking up from `start`. Returns the pinned version
/// string and the file it was read from.
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

/// Read the active version from the active-version file, if set.
pub fn active_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.active_file(WASMTIME)).ok()?;
    let v = text.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Set the active version by writing the active-version file.
pub fn set_active_version(layout: &Layout, version: &str) -> Result<()> {
    let path = layout.active_file(WASMTIME);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, version)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn binary_in_version(layout: &Layout, version: &str) -> PathBuf {
    layout.version_dir(WASMTIME, version).join("bin").join("wasmtime")
}

/// Resolve a wasmtime binary following the documented discovery order:
/// project pin → active runtime → env override → system/PATH.
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

    // 2. Active runtime
    if let Some(version) = active_version(layout) {
        let bin = binary_in_version(layout, &version);
        if bin.exists() {
            return Ok(Resolved { binary: bin, source: format!("active runtime ({version})") });
        }
    }

    // 3. Explicit environment variable
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

    // 4 & 5. System runtime / PATH lookup
    if let Some(bin) = which("wasmtime") {
        return Ok(Resolved { binary: bin, source: "PATH".to_string() });
    }

    bail!("no wasmtime runtime found (no project pin, active runtime, env override, or PATH entry); try `wvm install latest && wvm use latest`")
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
