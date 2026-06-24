//! Per-version `manifest.json` describing materialized files and their digests.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Logical path within the version directory, e.g. `bin/wasmtime`.
    pub path: String,
    pub sha256: String,
    /// Octal mode string, e.g. `0755`.
    pub mode: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub runtime: String,
    pub version: String,
    pub platform: String,
    pub archive_sha256: String,
    pub materialization: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("writing manifest {}", path.display()))?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Manifest> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        let manifest = serde_json::from_str(&text)
            .with_context(|| format!("parsing manifest {}", path.display()))?;
        Ok(manifest)
    }
}

/// Format a unix mode as a 4-digit octal string (e.g. `0755`).
pub fn mode_string(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

/// Parse a `0755`-style octal mode string.
pub fn parse_mode(s: &str) -> Result<u32> {
    let trimmed = s.trim_start_matches("0o");
    u32::from_str_radix(trimmed, 8)
        .with_context(|| format!("invalid mode string: {s}"))
}
