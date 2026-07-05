//! Pass-through runtime shim.
//!
//! When this binary is invoked as `wasmtime` (via the `shims/wasmtime` link on
//! `PATH`), it resolves the active runtime, records the invocation to the usage
//! log, and execs the real runtime — forwarding all arguments. An application
//! calling `wasmtime` therefore needs to know nothing about wvm: the tracking
//! and version selection are invisible side effects of a name on `PATH`.

use crate::{ensure_active_runtime, ensure_executable, exec_or_run, now_epoch};
use anyhow::Result;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use wvm_core::layout::Layout;
use wvm_core::usage::{self, UsageEntry};

pub fn run() -> Result<()> {
    let layout = Layout::discover()?;
    let _ = std::fs::create_dir_all(&layout.root);
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // A leading `--no-usage` opts this invocation out of recording; everything
    // else is forwarded to the runtime unchanged.
    let no_usage = raw.first().map(String::as_str) == Some("--no-usage");
    let args: Vec<String> = if no_usage { raw[1..].to_vec() } else { raw };
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

    if !no_usage {
        record_invocation(&layout, &resolved, cwd.as_deref(), &args);
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
    let module_sha256 = module.as_deref().and_then(|m| hash_module(Path::new(m)));

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
        manifest: discover_app(cwd),
        invoked_at: now_epoch(),
    };
    let _ = usage::record(layout, &entry);

    // Auto-register an app observed running from a directory with an `[app]`
    // manifest, so `uninstall` gating and `wvm apps` work without a manual
    // `wvm register`. Best-effort: a failed registration never fails the launch.
    if let Some(app_ref) = &entry.manifest {
        let _ = wvm_core::apps::register(
            layout,
            &app_ref.name,
            Some(&app_ref.dir),
            app_ref.runtime_path.as_deref(),
            &app_ref.runtimes,
            entry.invoked_at,
        );
    }
}

/// Discover an `[app]` manifest at or above `cwd`, so the app can auto-register
/// the application on ingest (no manual `wvm register`). Best-effort: stops at
/// the nearest `wvm.toml`; returns `None` when it has no `[app]` section.
fn discover_app(cwd: Option<&Path>) -> Option<wvm_core::usage::AppRef> {
    let mut dir = cwd?;
    loop {
        if dir.join(wvm_core::discovery::PIN_FILE).is_file() {
            let m = wvm_core::appmanifest::AppManifest::read_dir(dir).ok()?;
            return Some(wvm_core::usage::AppRef {
                name: m.name,
                dir: dir.display().to_string(),
                runtimes: m.runtimes,
                runtime_path: m.runtime_path,
            });
        }
        dir = dir.parent()?;
    }
}

/// Hash a module's bytes for the usage record, warning interactively before a
/// large read so the user knows how to opt out. The warning is gated on an
/// stderr terminal so it never pollutes an app's output when run non-interactively.
fn hash_module(path: &Path) -> Option<String> {
    if let Ok(meta) = std::fs::metadata(path) {
        let threshold = large_module_threshold();
        if threshold > 0 && meta.len() >= threshold && std::io::stderr().is_terminal() {
            eprintln!(
                "wvm: hashing large module ({}) for usage tracking; \
                 opt out with `--no-usage` or WVM_NO_USAGE=1",
                wvm_core::human_bytes(meta.len())
            );
        }
    }
    wvm_core::hash::sha256_file(path).ok()
}

/// Module-size threshold (bytes) above which hashing warns. `WVM_HASH_WARN_MB`
/// overrides the 100 MiB default; `0` disables the warning.
fn large_module_threshold() -> u64 {
    std::env::var("WVM_HASH_WARN_MB")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(100)
        .saturating_mul(1024 * 1024)
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
