//! Transparent usage tracking.
//!
//! The `shims/wasmtime` pass-through appends one JSON line per invocation to
//! `usage.log`, then execs the real runtime. This is deliberately cheap — a
//! single append on the hot path, no database, no WASM boot. `usage.log` *is*
//! the usage store: reads parse it directly and everything else (per-version
//! rollups, recent invocations) is derived from those entries. To keep the file
//! bounded it is compacted on read to the most recent [`CAP`] entries.
//!
//! Observation complements registration: an app needs to do nothing (not even
//! know wvm exists) to be seen here — it just calls `wasmtime`.

use crate::layout::Layout;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Maximum number of entries retained in `usage.log`; older ones are dropped
/// when the log is compacted on read.
const CAP: usize = 10_000;

/// Observed-usage rollup for one runtime version.
#[derive(Debug, Clone)]
pub struct VersionUsage {
    pub version: String,
    pub count: i64,
    pub last_used: i64,
}

/// An `[app]` manifest discovered at (or above) an invocation's working
/// directory. Carried on a [`UsageEntry`] so the app can auto-register the
/// application when it ingests the log — no manual `wvm register` needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRef {
    /// Application name from the manifest's `[app]` section.
    pub name: String,
    /// Directory containing the `wvm.toml` (the app's root).
    pub dir: String,
    /// Declared wvm-managed runtimes the app depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<String>,
    /// Optional custom runtime the app supplies itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_path: Option<String>,
}

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
    /// `[app]` manifest discovered at/above the cwd, for auto-registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<AppRef>,
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

/// Read every recorded invocation from `usage.log`. Unparseable lines are
/// skipped. When the log holds more than [`CAP`] entries it is compacted in
/// place — rewritten to just the most recent `CAP` — and the retained entries
/// are returned.
pub fn read(layout: &Layout) -> Result<Vec<UsageEntry>> {
    let path = layout.usage_log();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut entries: Vec<UsageEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UsageEntry>(l).ok())
        .collect();

    if entries.len() > CAP {
        entries.drain(..entries.len() - CAP);
        let mut out = String::new();
        for e in &entries {
            if let Ok(line) = serde_json::to_string(e) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        std::fs::write(&path, out).with_context(|| format!("compacting {}", path.display()))?;
    }
    Ok(entries)
}

/// Per-version rollup of `entries`: invocation count and last-used timestamp,
/// most recently used first.
pub fn by_version(entries: &[UsageEntry]) -> Vec<VersionUsage> {
    use std::collections::HashMap;
    let mut map: HashMap<&str, (i64, i64)> = HashMap::new();
    for e in entries {
        let slot = map.entry(e.version.as_str()).or_insert((0, i64::MIN));
        slot.0 += 1;
        slot.1 = slot.1.max(e.invoked_at);
    }
    let mut out: Vec<VersionUsage> = map
        .into_iter()
        .map(|(version, (count, last_used))| VersionUsage {
            version: version.to_string(),
            count,
            last_used,
        })
        .collect();
    out.sort_by_key(|u| std::cmp::Reverse(u.last_used));
    out
}

/// The `limit` most recent invocations, newest first (stable on ties).
pub fn recent(entries: &[UsageEntry], limit: usize) -> Vec<UsageEntry> {
    let mut ordered: Vec<UsageEntry> = entries.to_vec();
    ordered.sort_by_key(|e| std::cmp::Reverse(e.invoked_at));
    ordered.truncate(limit);
    ordered
}
