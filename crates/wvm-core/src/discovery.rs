//! Runtime discovery: project pin, session override, default, env, PATH.
//!
//! Two layers of selection:
//! - **default** — persistent (`runtimes/wasmtime/default`), used by new shells.
//! - **session** — the `WVM_VERSION` environment variable, set per shell, which
//!   overrides the default for the current session only.
//!
//! Each of pin/session/default stores a [`VersionSpec`] (e.g. `latest`, `24`,
//! `24.0.1`) rather than a frozen version, so a floating pin like `24` tracks
//! the newest installed `24.*` automatically. Resolution here is **offline**:
//! specs resolve against the *installed* set. Pulling a newer matching version
//! from the network (auto-install) is layered on top at the activation boundary.

use crate::layout::{Layout, WASMTIME};
use crate::spec::VersionSpec;
use anyhow::{anyhow, bail, Context, Result};
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

/// Find a project pin by walking up from `start`. Returns the raw spec string
/// and the file it came from.
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

/// Installed versions (those with a `manifest.json`), sorted ascending.
pub fn installed_versions(layout: &Layout) -> Result<Vec<String>> {
    let dir = layout.versions_dir(WASMTIME);
    let mut versions = Vec::new();
    if dir.exists() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().join("manifest.json").is_file() {
                versions.push(name);
            }
        }
    }
    versions.sort_by(|a, b| crate::version_cmp(a, b));
    Ok(versions)
}

/// The protected seed runtime as a `(version, binary)` pair, if it is locked.
/// The seed is the Wasmtime that runs wvm itself; it is also a perfectly good
/// runtime, so discovery falls back to it (see [`resolve`]) when a spec resolves
/// to its version and no managed copy is installed. Read-only: nothing here
/// mutates the seed.
pub fn seed_runtime(layout: &Layout) -> Option<(String, PathBuf)> {
    let text = std::fs::read_to_string(layout.seed_marker()).ok()?;
    let version = text.trim().to_string();
    let bin = layout.seed_bin();
    (!version.is_empty() && bin.is_file()).then_some((version, bin))
}

/// True when `spec_str` parses and resolves exactly to `version`.
fn spec_matches(spec_str: &str, version: &str) -> bool {
    let Ok(spec) = VersionSpec::parse(spec_str) else {
        return false;
    };
    let set = [version.to_string()];
    spec.resolve(&set) == Some(version)
}

/// Resolve a spec string against the installed set (offline). `None` when the
/// spec is unparseable or nothing installed matches.
pub fn resolve_installed(layout: &Layout, spec_str: &str) -> Option<String> {
    let spec = VersionSpec::parse(spec_str).ok()?;
    let installed = installed_versions(layout).ok()?;
    spec.resolve(&installed).map(str::to_string)
}

/// The persistent default spec, if set.
pub fn default_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.default_file(WASMTIME)).ok()?;
    let v = text.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Write the persistent default spec.
pub fn set_default_version(layout: &Layout, spec: &str) -> Result<()> {
    let path = layout.default_file(WASMTIME);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, spec).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The per-session override spec (`WVM_VERSION`), if set.
pub fn session_version() -> Option<String> {
    match std::env::var(SESSION_VAR) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

/// The effective **spec** and where it came from: session overrides default.
/// This is the raw request (`24`), not what it resolves to.
pub fn effective_spec(layout: &Layout) -> Option<(String, &'static str)> {
    if let Some(v) = session_version() {
        return Some((v, "session"));
    }
    default_version(layout).map(|v| (v, "default"))
}

/// The effective **resolved** version (spec resolved against the installed
/// set) and its source. `None` when no spec is set or nothing installed
/// matches it.
pub fn effective_version(layout: &Layout) -> Option<(String, &'static str)> {
    let (spec_str, src) = effective_spec(layout)?;
    if let Some(resolved) = resolve_installed(layout, &spec_str) {
        return Some((resolved, src));
    }
    // Fall back to the seed runtime when its version satisfies the spec, so a
    // fresh install (whose default points at the seed) reports a live runtime.
    let (seed_ver, _) = seed_runtime(layout)?;
    spec_matches(&spec_str, &seed_ver).then_some((seed_ver, src))
}

/// The effective spec including the **project pin**, which needs the real
/// working directory (not available inside the app sandbox). Order: pin →
/// session → default. Used by the native bootstrapper for activation-time
/// auto-install.
pub fn effective_spec_at(layout: &Layout, cwd: &Path) -> Result<Option<(String, String)>> {
    if let Some((spec, file)) = find_pin(cwd)? {
        return Ok(Some((spec, format!("project pin ({})", file.display()))));
    }
    if let Some(v) = session_version() {
        return Ok(Some((v, "session".to_string())));
    }
    Ok(default_version(layout).map(|v| (v, "default".to_string())))
}

fn binary_in_version(layout: &Layout, version: &str) -> PathBuf {
    layout
        .version_dir(WASMTIME, version)
        .join("bin")
        .join("wasmtime")
}

/// Describe a resolution for the `source` field, showing `spec -> version` only
/// when the spec floats.
fn describe(spec: &VersionSpec, resolved: &str, src: &str) -> String {
    if spec.is_floating() {
        format!("{src} ({spec} -> {resolved})")
    } else {
        format!("{src} ({resolved})")
    }
}

/// Resolve a wasmtime binary following the discovery order:
/// project pin → session (`WVM_VERSION`) → default → `WASMTIME_HOME` → PATH.
///
/// Floating specs resolve against the installed set; this call never touches the
/// network.
pub fn resolve(layout: &Layout, cwd: &Path) -> Result<Resolved> {
    let installed = installed_versions(layout)?;

    // 1. Project pin — a pin that names an unsatisfiable spec is a hard error
    //    (the user asked for it explicitly here).
    if let Some((spec_str, file)) = find_pin(cwd)? {
        let spec =
            VersionSpec::parse(&spec_str).map_err(|e| anyhow!("{e} (in {})", file.display()))?;
        match spec.resolve(&installed) {
            Some(version) if binary_in_version(layout, version).exists() => {
                return Ok(Resolved {
                    binary: binary_in_version(layout, version),
                    source: describe(&spec, version, &format!("project pin ({})", file.display())),
                });
            }
            _ => bail!(
                "project pins wasmtime '{spec}' (from {}) but no matching version is installed; run `wvm install {spec}`",
                file.display()
            ),
        }
    }

    // 2. Session override, then 3. default. Unsatisfiable specs fall through.
    for (spec_str, src) in [
        session_version().map(|v| (v, "session")),
        default_version(layout).map(|v| (v, "default")),
    ]
    .into_iter()
    .flatten()
    {
        let Ok(spec) = VersionSpec::parse(&spec_str) else {
            continue;
        };
        if let Some(version) = spec.resolve(&installed) {
            let bin = binary_in_version(layout, version);
            if bin.exists() {
                return Ok(Resolved {
                    binary: bin,
                    source: describe(&spec, version, src),
                });
            }
        }
    }

    // 3b. Seed runtime — the protected Wasmtime that runs wvm itself is also a
    //     valid runtime. When the session/default spec resolves to its version
    //     (the initial default adopted at first bootstrap) and no managed copy
    //     is installed, run the seed directly so `wvm exec` works out of the box.
    //     Managed installs above take precedence; this is a fallback only.
    if let Some((seed_ver, seed_bin)) = seed_runtime(layout) {
        for (spec_str, src) in [
            session_version().map(|v| (v, "session")),
            default_version(layout).map(|v| (v, "default")),
        ]
        .into_iter()
        .flatten()
        {
            if spec_matches(&spec_str, &seed_ver) {
                return Ok(Resolved {
                    binary: seed_bin,
                    source: format!("{src} (seed {seed_ver})"),
                });
            }
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
                    return Ok(Resolved {
                        binary: candidate,
                        source: format!("${var}"),
                    });
                }
            }
        }
    }

    // 5. System runtime / PATH lookup
    if let Some(bin) = which("wasmtime") {
        return Ok(Resolved {
            binary: bin,
            source: "PATH".to_string(),
        });
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
