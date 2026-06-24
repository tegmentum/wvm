//! Filesystem layout for the WVM root.
//!
//! ```text
//! ~/.tegmentum/wvm/
//!   store/sha256/<ab>/<cd>/<digest>
//!   runtimes/<runtime>/versions/<version>/{bin/wasmtime -> store, manifest.json}
//!   runtimes/<runtime>/current -> versions/<version>
//!   downloads/
//!   config.toml
//! ```

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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
                return Ok(Layout { root: PathBuf::from(v) });
            }
        }
        Ok(Layout { root: default_root()? })
    }

    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store").join("sha256")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// SQLite index of objects, versions, and backlinks (a derived cache).
    pub fn db_file(&self) -> PathBuf {
        self.root.join("index.db")
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

    /// Plain-text file naming the active version for a runtime (wasm-friendly,
    /// replaces the older `current` symlink).
    pub fn active_file(&self, runtime: &str) -> PathBuf {
        self.runtime_dir(runtime).join("active")
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

    /// Absolute path of a CAS object given its hex digest.
    pub fn object_path(&self, digest: &str) -> PathBuf {
        self.store_dir()
            .join(&digest[0..2])
            .join(&digest[2..4])
            .join(digest)
    }

    /// Ensure the base directory skeleton exists.
    pub fn ensure_base(&self) -> Result<()> {
        for dir in [self.store_dir(), self.downloads_dir(), self.versions_dir(WASMTIME)] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(())
    }
}

/// Default WVM root when `WVM_HOME` is unset.
#[cfg(not(target_arch = "wasm32"))]
fn default_root() -> Result<PathBuf> {
    let home =
        dirs::home_dir().context("could not determine home directory; set WVM_HOME")?;
    Ok(home.join(".tegmentum").join("wvm"))
}

/// On wasm there is no home directory; the bootstrapper must provide WVM_HOME.
#[cfg(target_arch = "wasm32")]
fn default_root() -> Result<PathBuf> {
    anyhow::bail!("WVM_HOME is not set")
}

/// Compute a relative path from `from_dir` to `target` (both absolute).
pub fn relative_to(from_dir: &Path, target: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = target.components().collect();

    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    for _ in 0..(from.len() - common) {
        result.push("..");
    }
    for comp in &to[common..] {
        result.push(comp.as_os_str());
    }
    result
}
