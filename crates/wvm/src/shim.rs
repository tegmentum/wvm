//! Pass-through runtime shim.
//!
//! When this binary is invoked as `wasmtime` (via the `shims/wasmtime` link on
//! `PATH`), it resolves the active runtime, records the invocation to the usage
//! log, and execs the real runtime — forwarding all arguments. An application
//! calling `wasmtime` therefore needs to know nothing about wvm: the tracking
//! and version selection are invisible side effects of a name on `PATH`.

use crate::{ensure_active_runtime, ensure_executable, exec_or_run, now_epoch};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use wvm_core::layout::Layout;
use wvm_core::usage::{self, UsageEntry};

pub fn run() -> Result<()> {
    let layout = Layout::discover()?;
    let _ = std::fs::create_dir_all(&layout.root);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().ok();

    // Activation-time auto-install for a floating active spec (best-effort;
    // network failures fall back to the best installed match).
    if let Some(dir) = &cwd {
        if let Err(e) = ensure_active_runtime(&layout, dir) {
            if std::env::var_os("WVM_VERBOSE").is_some() {
                eprintln!("wvm(shim): auto-install check skipped: {e:#}");
            }
        }
    }

    let resolve_dir = cwd.clone().unwrap_or_else(|| PathBuf::from("."));
    let resolved = wvm_core::discovery::resolve(&layout, &resolve_dir)?;

    // Record the invocation unless the user opted out. Never fatal.
    if std::env::var_os("WVM_NO_USAGE").is_none() {
        let entry = UsageEntry {
            version: resolved_version(&resolved.binary, &resolved.source),
            app: env_nonempty("WVM_APP"),
            caller: detect_caller(),
            cwd: cwd.as_ref().map(|c| c.display().to_string()),
            invoked_at: now_epoch(),
        };
        let _ = usage::record(&layout, &entry);
    }

    if std::env::var_os("WVM_VERBOSE").is_some() {
        eprintln!(
            "wvm(shim): {} [{}]",
            resolved.binary.display(),
            resolved.source
        );
    }

    ensure_executable(&resolved.binary);
    let mut cmd = Command::new(&resolved.binary);
    cmd.args(&args);
    exec_or_run(cmd, &resolved.binary)
}

/// `…/versions/<version>/bin/wasmtime` → `<version>`; otherwise the source
/// label (e.g. `PATH`) for a runtime resolved outside the store.
fn resolved_version(binary: &Path, source: &str) -> String {
    binary
        .parent()
        .and_then(Path::parent)
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|v| *v != "bin" && *v != "seed")
        .map(str::to_string)
        .unwrap_or_else(|| source.to_string())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Best-effort invoking process name. Linux reads `/proc`; macOS shells out to
/// `ps`; elsewhere we rely on `WVM_APP`.
#[cfg(target_os = "linux")]
fn detect_caller() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    std::fs::read_to_string(format!("/proc/{ppid}/comm")).map_or(None, |s| {
        let name = s.trim().to_string();
        (!name.is_empty()).then_some(name)
    })
}

#[cfg(target_os = "macos")]
fn detect_caller() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &ppid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let name = s.trim();
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
    (!base.is_empty()).then(|| base.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_caller() -> Option<String> {
    None
}
