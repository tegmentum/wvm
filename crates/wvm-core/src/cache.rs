//! On-disk cache of the remote release list.
//!
//! Floating specs auto-install the newest matching version at activation time,
//! which would otherwise mean a GitHub API call on every `wvm exec`. This cache
//! bounds that: within the refresh interval, resolution reads the cached list
//! instead of the network. Only the app (which has `wasi:http`) writes it; the
//! native bootstrapper reads it to decide whether a newer version might exist.

use crate::layout::Layout;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Default refresh interval: one hour.
pub const DEFAULT_REFRESH_SECS: i64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCache {
    pub fetched_at: i64,
    pub all: bool,
    pub versions: Vec<String>,
}

impl ReleaseCache {
    /// Whether the cache is still within `ttl_secs` of `now`. A `ttl_secs <= 0`
    /// is never fresh (forces a refresh) — but see [`refresh_interval`], where
    /// `0` instead means "stay offline".
    pub fn is_fresh(&self, now: i64, ttl_secs: i64) -> bool {
        ttl_secs > 0 && now.saturating_sub(self.fetched_at) < ttl_secs
    }
}

/// Read the cached release list, if present and parseable.
pub fn read(layout: &Layout, all: bool) -> Option<ReleaseCache> {
    let text = std::fs::read_to_string(layout.release_cache_file(all)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist a freshly fetched release list.
pub fn write(layout: &Layout, all: bool, versions: &[String], now: i64) -> Result<()> {
    let path = layout.release_cache_file(all);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cache = ReleaseCache {
        fetched_at: now,
        all,
        versions: versions.to_vec(),
    };
    std::fs::write(&path, serde_json::to_string(&cache)?)?;
    Ok(())
}

/// Drop the cached release lists so the next fetch hits the network (used by
/// `wvm upgrade` to force a fresh check).
pub fn clear(layout: &Layout) {
    for all in [false, true] {
        let _ = std::fs::remove_file(layout.release_cache_file(all));
    }
}

/// Refresh interval in seconds. `WVM_REFRESH_INTERVAL` overrides the default;
/// `0` disables network refresh entirely (activation resolves offline).
pub fn refresh_interval() -> i64 {
    std::env::var("WVM_REFRESH_INTERVAL")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_REFRESH_SECS)
}
