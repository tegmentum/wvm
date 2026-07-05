//! Filesystem layout for the WVM root.
//!
//! ```text
//! ~/.tegmentum/wvm/
//!   runtimes/<runtime>/versions/<version>/{bin/wasmtime, manifest.json}
//!   runtimes/<runtime>/default
//!   downloads/
//!   cache/
//!   apps.json
//!   usage.log
//! ```
//!
//! Runtime versions hold their real extracted files directly — there is no
//! content-addressable store and no database; everything else is derived by
//! scanning the version directories and their manifests.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// The runtime managed by WVM v1.
pub const WASMTIME: &str = "wasmtime";

/// Resolved locations under the WVM root.
#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    /// Resolve the WVM root, honoring `WVM_HOME` then falling back to
    /// `~/.tegmentum/wvm`. Inside the wasm app `WVM_HOME` is always set by the
    /// bootstrapper.
    pub fn discover() -> Result<Layout> {
        if let Some(v) = std::env::var_os("WVM_HOME") {
            if !v.is_empty() {
                return Ok(Layout {
                    root: PathBuf::from(v),
                });
            }
        }
        Ok(Layout {
            root: default_root()?,
        })
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    /// Registered applications and the runtimes they depend on (JSON).
    pub fn apps_file(&self) -> PathBuf {
        self.root.join("apps.json")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Cached remote release list (so activation-time auto-install need not hit
    /// the network every invocation). Separate files for the stable vs `--all`
    /// listings.
    pub fn release_cache_file(&self, all: bool) -> PathBuf {
        let name = if all {
            "releases-all.json"
        } else {
            "releases.json"
        };
        self.cache_dir().join(name)
    }

    pub fn runtime_dir(&self, runtime: &str) -> PathBuf {
        self.root.join("runtimes").join(runtime)
    }

    pub fn versions_dir(&self, runtime: &str) -> PathBuf {
        self.runtime_dir(runtime).join("versions")
    }

    pub fn version_dir(&self, runtime: &str, version: &str) -> PathBuf {
        self.versions_dir(runtime).join(version)
    }

    pub fn manifest_file(&self, runtime: &str, version: &str) -> PathBuf {
        self.version_dir(runtime, version).join("manifest.json")
    }

    /// Plain-text file naming the persistent **default** version for a runtime
    /// (what new shells use). A session can override it via `WVM_VERSION`.
    pub fn default_file(&self, runtime: &str) -> PathBuf {
        self.runtime_dir(runtime).join("default")
    }

    // --- Protected seed runtime ------------------------------------------
    // The seed is the Wasmtime that runs the wvm app itself. It lives in its
    // own directory, separate from user-managed versions, so wvm's own
    // commands never list or delete it.

    pub fn seed_dir(&self) -> PathBuf {
        self.root.join("seed")
    }

    /// The seed Wasmtime executable.
    pub fn seed_bin(&self) -> PathBuf {
        self.seed_dir().join("bin").join("wasmtime")
    }

    /// Marker file recording the locked seed version (its presence = locked).
    pub fn seed_marker(&self) -> PathBuf {
        self.seed_dir().join("SEED")
    }

    /// Where the bootstrapper writes the embedded app component.
    pub fn app_wasm(&self) -> PathBuf {
        self.root.join("wvm-app.wasm")
    }

    // --- pass-through shims + usage log ----------------------------------
    // `shims/wasmtime` is the `wvm` binary under another name; when it is on
    // `PATH`, apps that call `wasmtime` transparently route through it, which
    // records the invocation and execs the resolved runtime.

    pub fn shims_dir(&self) -> PathBuf {
        self.root.join("shims")
    }

    /// Path of a named shim (e.g. `wasmtime`).
    pub fn shim_bin(&self, name: &str) -> PathBuf {
        self.shims_dir().join(name)
    }

    /// Append-only log of runtime invocations recorded by the shim; the usage
    /// store itself (JSON Lines, compacted on read).
    pub fn usage_log(&self) -> PathBuf {
        self.root.join("usage.log")
    }

    /// Ensure the base directory skeleton exists.
    pub fn ensure_base(&self) -> Result<()> {
        for dir in [self.downloads_dir(), self.versions_dir(WASMTIME)] {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(())
    }
}

/// Default WVM root when `WVM_HOME` is unset.
#[cfg(not(target_arch = "wasm32"))]
fn default_root() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory; set WVM_HOME")?;
    Ok(home.join(".tegmentum").join("wvm"))
}

/// On wasm there is no home directory; the bootstrapper must provide WVM_HOME.
#[cfg(target_arch = "wasm32")]
fn default_root() -> Result<PathBuf> {
    anyhow::bail!("WVM_HOME is not set")
}
