//! Transparent usage tracking.
//!
//! The `shims/wasmtime` pass-through appends one JSON line per invocation to
//! `usage.log`, then execs the real runtime. This is deliberately cheap — a
//! single append on the hot path, no database, no WASM boot. The app later
//! [`drain`]s the log into the SQLite `usage` table (see `index::ingest_usage_log`).
//!
//! Observation complements registration: an app needs to do nothing (not even
//! know wvm exists) to be seen here — it just calls `wasmtime`.

use crate::layout::Layout;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// One recorded runtime invocation, capturing the full context of the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEntry {
    /// Resolved runtime version (or a source label like `PATH` for external
    /// runtimes).
    pub version: String,
    /// Absolute path of the runtime binary that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_path: Option<String>,
    /// Self-identified application (`WVM_APP`), if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Best-effort invoking process name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// Working directory at invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Full argument vector forwarded to the runtime (flags, options, and the
    /// module together — the ground truth of what ran).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// The module argument as given on the command line (best-effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Canonical absolute path of the module, if it resolved to a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_path: Option<String>,
    /// `sha256` of the module bytes, if a module file was identified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_sha256: Option<String>,
    /// Unix timestamp (seconds).
    pub invoked_at: i64,
}

/// Append one entry to the usage log. Best-effort: callers on the hot path
/// should ignore the error rather than fail the runtime launch.
pub fn record(layout: &Layout, entry: &UsageEntry) -> Result<()> {
    let path = layout.usage_log();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("appending to {}", path.display()))?;
    Ok(())
}

/// Atomically take everything currently in the log and return it, leaving a
/// fresh (empty) log for concurrent appenders. Renames the log aside first so a
/// shim appending mid-ingest writes to the new file rather than losing a line.
/// Unparseable lines are skipped.
pub fn drain(layout: &Layout) -> Result<Vec<UsageEntry>> {
    let path = layout.usage_log();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let taken = path.with_extension("log.ingesting");
    // If a previous ingest was interrupted, fold its leftovers in too.
    if taken.exists() {
        let _ = std::fs::remove_file(&taken);
    }
    std::fs::rename(&path, &taken).with_context(|| format!("rotating {}", path.display()))?;

    let text = std::fs::read_to_string(&taken).unwrap_or_default();
    let entries = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UsageEntry>(l).ok())
        .collect();
    let _ = std::fs::remove_file(&taken);
    Ok(entries)
}
