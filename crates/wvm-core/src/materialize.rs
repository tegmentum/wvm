//! Materialize a version directory from CAS objects.

use crate::config::Materialization;
use crate::layout::relative_to;
use anyhow::{Context, Result};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Materialize one logical file inside `version_dir` from a store object.
///
/// For `Symlink`, a relative symlink to the object is created so the layout
/// stays inspectable. For `Copy`, the object bytes are copied and `mode` is
/// applied.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn materialize(
    strategy: Materialization,
    version_dir: &Path,
    logical_path: &str,
    object_abs: &Path,
    mode: u32,
) -> Result<()> {
    let link_path = version_dir.join(logical_path);
    let parent = link_path.parent().context("logical path has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    match strategy {
        Materialization::Symlink => {
            let rel = relative_to(parent, object_abs);
            symlink(&rel, &link_path).with_context(|| {
                format!("symlinking {} -> {}", link_path.display(), rel.display())
            })?;
        }
        Materialization::Copy => {
            std::fs::copy(object_abs, &link_path).with_context(|| {
                format!(
                    "copying {} -> {}",
                    object_abs.display(),
                    link_path.display()
                )
            })?;
            #[cfg(unix)]
            std::fs::set_permissions(&link_path, std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("setting mode on {}", link_path.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

// The wasm app cannot create symlinks via std; it uses `Copy` materialization
// instead. This stub keeps the crate compiling and errors if reached.
#[cfg(target_arch = "wasm32")]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink materialization is unavailable under wasm; use the `copy` strategy",
    ))
}
