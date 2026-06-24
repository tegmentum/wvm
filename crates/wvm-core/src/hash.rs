//! SHA-256 helpers.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Hex-encoded SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Hex-encoded SHA-256 of a file, streamed (handles large files / symlinks).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("reading {} for hashing", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}
