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

    record_invocation(&layout, &resolved, cwd.as_deref(), &args);

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

/// Record one runtime invocation to the usage log, unless `WVM_NO_USAGE` is
/// set. Shared by the shim and `wvm exec` — both are real runtime uses. Captures
/// the full argv plus the identified module's path and sha256. Never fatal.
pub(crate) fn record_invocation(
    layout: &Layout,
    resolved: &wvm_core::discovery::Resolved,
    cwd: Option<&Path>,
    args: &[String],
) {
    if std::env::var_os("WVM_NO_USAGE").is_some() {
        return;
    }
    let module = identify_module(args);
    let module_path = module
        .as_deref()
        .and_then(|m| std::fs::canonicalize(m).ok())
        .map(|p| p.display().to_string());
    let module_sha256 = module
        .as_deref()
        .and_then(|m| wvm_core::hash::sha256_file(Path::new(m)).ok());

    let entry = UsageEntry {
        version: resolved_version(&resolved.binary, &resolved.source),
        runtime_path: Some(resolved.binary.display().to_string()),
        app: env_nonempty("WVM_APP"),
        caller: detect_caller(),
        cwd: cwd.map(|c| c.display().to_string()),
        args: args.to_vec(),
        module,
        module_path,
        module_sha256,
        invoked_at: now_epoch(),
    };
    let _ = usage::record(layout, &entry);
}

/// Best-effort: the module argument in a wasmtime command line — the first
/// positional that is an existing file or has a wasm-ish extension, skipping
/// flags and known subcommands. The full `args` are recorded regardless, so
/// this only drives the module-path/sha256 capture.
fn identify_module(args: &[String]) -> Option<String> {
    const SUBCOMMANDS: &[&str] = &[
        "run", "serve", "compile", "explore", "settings", "wast", "config", "help",
    ];
    for a in args {
        if a.starts_with('-') || SUBCOMMANDS.contains(&a.as_str()) {
            continue;
        }
        if Path::new(a).is_file()
            || a.ends_with(".wasm")
            || a.ends_with(".wat")
            || a.ends_with(".cwasm")
        {
            return Some(a.clone());
        }
    }
    None
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
