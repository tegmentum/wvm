//! Application manifest — the `[app]` section of an app's `wvm.toml`.
//!
//! This is the **canonical** declaration of which runtime(s) an application was
//! tested against. The app owns it and reads it itself, so the app works with
//! no wvm installed and may point at its own custom runtime. `wvm register`
//! ingests it into wvm's index purely for lifecycle bookkeeping (safe removal,
//! listing) — registration is advisory and never required by the app.
//!
//! ```toml
//! [app]
//! name = "tegmentum-foo"
//! runtimes = ["44.0.0", "45.0.0"]   # wvm-managed versions tested against
//! # runtime-path = "/opt/foo/bin/wasmtime"   # optional: app brings its own
//! ```

use crate::normalize_version;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

/// File (shared with the project-pin format) that may carry an `[app]` section.
pub const MANIFEST_FILE: &str = "wvm.toml";

#[derive(Debug, Clone)]
pub struct AppManifest {
    pub name: String,
    /// wvm-managed versions the app was tested against (may be empty if the app
    /// brings its own runtime).
    pub runtimes: Vec<String>,
    /// Optional custom runtime the app supplies itself (decoupled from wvm).
    pub runtime_path: Option<String>,
}

#[derive(Deserialize)]
struct RawFile {
    app: Option<RawApp>,
}

#[derive(Deserialize)]
struct RawApp {
    name: String,
    #[serde(default)]
    runtimes: Vec<String>,
    #[serde(default, rename = "runtime-path")]
    runtime_path: Option<String>,
}

impl AppManifest {
    /// Read and parse `<dir>/wvm.toml`'s `[app]` section.
    pub fn read_dir(dir: &Path) -> Result<AppManifest> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<AppManifest> {
        let raw: RawFile = toml::from_str(text)?;
        let app = raw
            .app
            .context("no [app] section (an application manifest needs `[app]`)")?;
        if app.name.trim().is_empty() {
            bail!("[app] name must not be empty");
        }
        if app.runtimes.is_empty() && app.runtime_path.is_none() {
            bail!("[app] must list `runtimes` or set `runtime-path`");
        }
        let runtimes = app.runtimes.iter().map(|v| normalize_version(v)).collect();
        Ok(AppManifest { name: app.name, runtimes, runtime_path: app.runtime_path })
    }
}
