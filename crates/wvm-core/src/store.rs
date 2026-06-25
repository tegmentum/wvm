//! The simple file-level content-addressable store.
//!
//! Objects are addressed by `sha256(file_bytes)` and stored at
//! `store/sha256/<ab>/<cd>/<digest>`. The store deliberately stays "boring":
//! it stores bytes by hash and nothing more.

use crate::layout::Layout;
use anyhow::{Context, Result};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// True if an object with `digest` already exists in the store.
pub fn has(layout: &Layout, digest: &str) -> bool {
    layout.object_path(digest).exists()
}

/// Insert `data` into the store under `digest` if not already present.
///
/// Returns the absolute object path. Writes via a temp file + rename so a
/// concurrent reader never sees a partial object. `mode` is applied to the
/// stored bytes on unix.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn put(layout: &Layout, digest: &str, data: &[u8], mode: u32) -> Result<PathBuf> {
    let dest = layout.object_path(digest);
    if dest.exists() {
        return Ok(dest);
    }

    let parent = dest.parent().expect("object path has a parent");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating store shard {}", parent.display()))?;

    // The digest already uniquely identifies the bytes, so a temp name derived
    // from it is collision-free for distinct objects. (Avoids `process::id`,
    // which is unsupported under wasm.)
    let tmp = parent.join(format!(".tmp-{digest}"));
    std::fs::write(&tmp, data).with_context(|| format!("writing store temp {}", tmp.display()))?;

    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting mode on {}", tmp.display()))?;

    // Rename is atomic on the same filesystem; if another process won the race
    // the destination already exists and our temp is redundant.
    if dest.exists() {
        let _ = std::fs::remove_file(&tmp);
    } else if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        if !dest.exists() {
            return Err(e).with_context(|| format!("publishing object {}", dest.display()));
        }
    }
    Ok(dest)
}
