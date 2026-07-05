//! Index abstraction over the backlink/metadata database.
//!
//! Implemented natively with `rusqlite` and in the app via the
//! `sqlite:wasm/high-level` component. The index is a derived cache: the store
//! and manifests on disk are authoritative, and [`reindex`] rebuilds it from
//! them.

use crate::layout::{Layout, WASMTIME};
use crate::manifest::Manifest;
use crate::usage::UsageEntry;
use anyhow::Result;

/// Summary statistics for `wvm gc` / `wvm objects` reporting.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub objects: i64,
    pub referenced: i64,
    pub total_size: i64,
}

/// Observed-usage rollup for one runtime version.
#[derive(Debug, Clone)]
pub struct VersionUsage {
    pub version: String,
    pub count: i64,
    pub last_used: i64,
}

/// A registered application and the runtimes it depends on (a cache of the
/// app's own manifest).
#[derive(Debug, Clone)]
pub struct AppRecord {
    pub name: String,
    /// App directory the manifest was read from.
    pub path: Option<String>,
    /// Custom runtime the app supplies itself (decoupled from wvm).
    pub runtime_path: Option<String>,
    /// wvm-managed versions the app was tested against.
    pub runtimes: Vec<String>,
}

/// Primitive operations the index backends must provide. Higher-level routines
/// such as [`reindex`] are built on top of these.
pub trait Index {
    /// Drop all rows (before a full rebuild).
    fn clear(&mut self) -> Result<()>;

    /// Insert or update a store object's recorded size.
    fn upsert_object(&mut self, digest: &str, size: i64) -> Result<()>;

    /// Remove an object row (after its file has been pruned).
    fn delete_object(&mut self, digest: &str) -> Result<()>;

    /// Record (or refresh) an installed version and its object backlinks.
    fn record_install(&mut self, manifest: &Manifest, installed_at: i64) -> Result<()>;

    /// Forget a version and its backlinks.
    fn remove_version(&mut self, runtime: &str, version: &str) -> Result<()>;

    /// Objects with no backlinks, as `(digest, size)` — GC candidates.
    fn unreferenced_objects(&self) -> Result<Vec<(String, i64)>>;

    /// All stored objects as `(digest, size)`, largest first.
    fn all_objects(&self) -> Result<Vec<(String, i64)>>;

    /// Versions referencing an object, as `(runtime, version)` pairs.
    fn backlinks(&self, digest: &str) -> Result<Vec<(String, String)>>;

    /// Aggregate counts/sizes.
    fn stats(&self) -> Result<Stats>;

    // --- application registration (lifecycle bookkeeping) ----------------

    /// Register (or refresh) an application and the runtimes it depends on.
    fn register_app(
        &mut self,
        name: &str,
        path: Option<&str>,
        runtime_path: Option<&str>,
        runtimes: &[String],
        registered_at: i64,
    ) -> Result<()>;

    /// Remove a registration. Returns true if an app was removed.
    fn unregister_app(&mut self, name: &str) -> Result<bool>;

    /// All registered applications.
    fn list_apps(&self) -> Result<Vec<AppRecord>>;

    /// Names of applications that depend on a given runtime version.
    fn apps_using(&self, version: &str) -> Result<Vec<String>>;

    // --- observed usage (transparent tracking via the shim) --------------

    /// Insert recorded invocations drained from the usage log.
    fn record_usage(&mut self, entries: &[UsageEntry]) -> Result<()>;

    /// Most recent invocations, newest first.
    fn recent_usage(&self, limit: i64) -> Result<Vec<UsageEntry>>;

    /// Per-version rollup: invocation count and last-used timestamp, most
    /// recently used first.
    fn usage_by_version(&self) -> Result<Vec<VersionUsage>>;
}

/// Drain the usage log into the `usage` table. Returns how many invocations
/// were ingested. Safe to call before any command that reads usage.
pub fn ingest_usage_log<I: Index>(index: &mut I, layout: &Layout) -> Result<usize> {
    let entries = crate::usage::drain(layout)?;
    if entries.is_empty() {
        return Ok(0);
    }
    index.record_usage(&entries)?;
    auto_register(index, &entries);
    Ok(entries.len())
}

/// Auto-register apps observed running from a directory with an `[app]`
/// manifest, so `uninstall` gating and `wvm apps` work without a manual
/// `wvm register`. Latest observation per app wins; entirely best-effort —
/// a failed registration never fails the ingest.
fn auto_register<I: Index>(index: &mut I, entries: &[UsageEntry]) {
    use std::collections::HashMap;
    let mut latest: HashMap<&str, (&crate::usage::AppRef, i64)> = HashMap::new();
    for e in entries {
        if let Some(m) = &e.manifest {
            let slot = latest.entry(m.name.as_str()).or_insert((m, e.invoked_at));
            if e.invoked_at >= slot.1 {
                *slot = (m, e.invoked_at);
            }
        }
    }
    for (name, (m, ts)) in latest {
        let _ = index.register_app(
            name,
            Some(m.dir.as_str()),
            m.runtime_path.as_deref(),
            &m.runtimes,
            ts,
        );
    }
}

/// Rebuild the index from the authoritative on-disk state: every object file in
/// the store and every installed version's manifest. Correct even if the index
/// drifted or was deleted; also records orphaned objects from interrupted
/// installs so GC can reclaim them.
pub fn reindex<I: Index>(index: &mut I, layout: &Layout) -> Result<()> {
    index.clear()?;

    // 1. Every real object file (catches orphans).
    let store_dir = layout.store_dir();
    if store_dir.exists() {
        for file in walk_files(&store_dir)? {
            let name = match file.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with(".tmp-") {
                continue;
            }
            let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            index.upsert_object(&name, size as i64)?;
        }
    }

    // 2. Every installed version + backlinks from its manifest.
    let versions_dir = layout.versions_dir(WASMTIME);
    if versions_dir.exists() {
        for entry in std::fs::read_dir(&versions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.is_file() {
                continue;
            }
            let manifest = Manifest::read(&manifest_path)?;
            let installed_at = mtime_epoch(&manifest_path);
            index.record_install(&manifest, installed_at)?;
        }
    }
    Ok(())
}

/// File modification time as unix seconds (0 if unavailable).
fn mtime_epoch(path: &std::path::Path) -> i64 {
    use std::time::UNIX_EPOCH;
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Recursively collect regular files under `dir`.
pub fn walk_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    use anyhow::Context;
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                out.push(entry.path());
            }
        }
    }
    Ok(out)
}
