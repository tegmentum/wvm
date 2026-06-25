//! Archive extraction. V1 supports `.tar.xz` (Linux/macOS Wasmtime releases).

use anyhow::{bail, Context, Result};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

/// A regular file extracted from a release archive.
pub struct ExtractedFile {
    /// Logical path within the version directory, e.g. `bin/wasmtime`.
    pub logical_path: String,
    pub mode: u32,
    pub data: Vec<u8>,
}

/// Extract a `.tar.xz` archive into in-memory files.
///
/// Wasmtime archives contain a single top-level directory
/// (`wasmtime-vX-arch-os/`) holding the `wasmtime` executable plus license and
/// readme files. The top-level directory is stripped, and the executable is
/// remapped to `bin/wasmtime` so the materialized layout matches the design.
pub fn extract_tar_xz(archive: &Path) -> Result<Vec<ExtractedFile>> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("opening archive {}", archive.display()))?;
    let mut reader = BufReader::new(file);

    let mut tar_bytes = Vec::new();
    lzma_rs::xz_decompress(&mut reader, &mut tar_bytes)
        .with_context(|| format!("xz-decompressing {}", archive.display()))?;

    let mut tar = tar::Archive::new(Cursor::new(tar_bytes));
    let mut out = Vec::new();

    for entry in tar.entries().context("reading tar entries")? {
        let mut entry = entry.context("reading tar entry")?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }

        let path = entry.path().context("reading tar entry path")?.into_owned();
        let Some(logical) = logical_path(&path) else {
            continue;
        };

        let mode = entry.header().mode().unwrap_or(0o644);
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .context("reading tar entry data")?;

        out.push(ExtractedFile {
            logical_path: logical,
            mode,
            data,
        });
    }

    if out.is_empty() {
        bail!("archive {} contained no files", archive.display());
    }
    Ok(out)
}

/// Map an archive path to a logical version-directory path, stripping the
/// single top-level directory and routing the executable into `bin/`.
fn logical_path(path: &Path) -> Option<String> {
    let mut comps = path.components();
    comps.next()?; // drop the top-level `wasmtime-vX-arch-os/` directory
    let rest: std::path::PathBuf = comps.as_path().into();
    if rest.as_os_str().is_empty() {
        return None;
    }

    let rest_str = rest.to_string_lossy().replace('\\', "/");
    if rest_str == "wasmtime" || rest_str == "wasmtime.exe" {
        return Some(format!("bin/{rest_str}"));
    }
    Some(rest_str)
}
