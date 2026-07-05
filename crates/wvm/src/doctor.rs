//! `wvm doctor` — diagnose the installation and shell integration.
//!
//! Native (like `exec`/`completions`): it needs the real `PATH`, the shell rc
//! files, and to run external `wasmtime` binaries — none of which the sandboxed
//! app can see. Offline: it reports configuration, not currency (use
//! `wvm seed status` / `wvm --upgrade --check` for update checks).

use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use wvm_core::layout::Layout;

pub fn run(layout: &Layout) -> Result<()> {
    println!("wvm doctor\n");
    let mut problems = 0usize;
    let exe = std::env::current_exe().ok();
    let shims_dir = layout.shims_dir();
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    section("WVM_HOME");
    if dir_writable(&layout.root) {
        ok(&format!("{} (writable)", layout.root.display()));
    } else {
        fail(&format!("{} is not writable", layout.root.display()));
        problems += 1;
    }

    section("wvm binary");
    let where_bin = exe
        .as_ref()
        .map(|e| format!(" at {}", e.display()))
        .unwrap_or_default();
    ok(&format!("wvm {}{where_bin}", env!("CARGO_PKG_VERSION")));

    section("Seed runtime");
    match crate::seed::installed(layout) {
        Some(v) => ok(&format!(
            "Wasmtime {v}  (check for updates: `wvm seed status`)"
        )),
        None => warn("not installed yet (downloaded on the first real command)"),
    }

    section("Shim & PATH");
    let shim = layout.shim_bin("wasmtime");
    match (exe.as_deref(), std::fs::read_link(&shim).ok()) {
        (Some(e), Some(t)) if t == e => ok(&format!("shims/wasmtime → {} (current)", e.display())),
        (_, Some(t)) => warn(&format!(
            "shims/wasmtime → {} (stale; any wvm command refreshes it)",
            t.display()
        )),
        (_, None) if shim.exists() => ok("shims/wasmtime present"),
        _ => warn("shims/wasmtime missing (any wvm command creates it)"),
    }
    match path_dirs.iter().position(|d| d == &shims_dir) {
        Some(i) => ok(&format!("shims dir on PATH (position {})", i + 1)),
        None => {
            fail(&format!(
                "shims dir not on PATH — run `wvm shell-init >> {}`",
                default_rc().display()
            ));
            problems += 1;
        }
    }

    // Does something else provide `wasmtime` ahead of our shim on PATH?
    let externals = detect_external(layout, &path_dirs);
    if let Some((p, _)) = externals.iter().find(|(p, _)| {
        first_path_index(&path_dirs, p.parent())
            .zip(path_dirs.iter().position(|d| d == &shims_dir))
            .map(|(ext, shim)| ext < shim)
            .unwrap_or(false)
    }) {
        warn(&format!(
            "an external wasmtime at {} comes before the shim on PATH — `wasmtime` will bypass wvm",
            p.display()
        ));
    }

    section("Shell integration");
    match detect_hook(&shims_dir) {
        Some(f) => ok(&format!("shim/use hook found in {f}")),
        None => warn(&format!(
            "hook not found — run `wvm shell-init >> {}`, then restart your shell",
            default_rc().display()
        )),
    }

    section("Default runtime");
    match wvm_core::discovery::default_version(layout) {
        Some(spec) => match wvm_core::discovery::resolve_installed(layout, &spec) {
            Some(v) => ok(&format!("default '{spec}' → {v} (installed)")),
            None => warn(&format!(
                "default '{spec}' set but no matching version installed — `wvm install {spec}`"
            )),
        },
        None => warn("no default set (the seed serves as default until you set one)"),
    }

    section("External wasmtimes (not managed by wvm)");
    if externals.is_empty() {
        ok("none found");
    } else {
        for (p, ver) in &externals {
            println!(
                "  • {}  at {}",
                ver.as_deref().unwrap_or("unknown version"),
                p.display()
            );
        }
        println!(
            "  (wvm can fall back to these via WASMTIME_HOME / PATH; it does not manage them)"
        );
    }

    println!();
    if problems == 0 {
        println!("No problems found.");
        Ok(())
    } else {
        println!("{problems} problem(s) found.");
        std::process::exit(1);
    }
}

/// Discover `wasmtime` binaries not owned by wvm: from `PATH`, the runtime-home
/// env vars, and common install locations. Anything under `WVM_HOME` (our shim,
/// seed, and managed versions) is excluded. Returns `(canonical path, version)`.
fn detect_external(layout: &Layout, path_dirs: &[PathBuf]) -> Vec<(PathBuf, Option<String>)> {
    let mut candidates: Vec<PathBuf> = path_dirs.iter().map(|d| d.join("wasmtime")).collect();
    for var in ["WASMTIME_HOME", "WASM_RUNTIME_HOME"] {
        if let Some(v) = std::env::var_os(var) {
            let p = PathBuf::from(v);
            candidates.push(p.join("bin").join("wasmtime"));
            candidates.push(p.join("wasmtime"));
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        candidates.push(PathBuf::from(dir).join("wasmtime"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".cargo/bin/wasmtime"));
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in candidates {
        if !c.is_file() {
            continue;
        }
        let canon = std::fs::canonicalize(&c).unwrap_or(c);
        // Skip anything wvm owns (shim, seed, managed versions).
        if canon.starts_with(&layout.root) {
            continue;
        }
        if !seen.insert(canon.clone()) {
            continue;
        }
        let version = Command::new(&canon)
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        out.push((canon, version));
    }
    out
}

fn first_path_index(path_dirs: &[PathBuf], dir: Option<&Path>) -> Option<usize> {
    let dir = dir?;
    path_dirs.iter().position(|d| d == dir)
}

/// The rc file for the current shell (`$SHELL`), for hint messages.
fn default_rc() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let shell = std::env::var("SHELL").unwrap_or_default();
    let base = Path::new(&shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match base {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bashrc"),
        "fish" => home.join(".config/fish/config.fish"),
        _ => home.join(".profile"),
    }
}

/// Best-effort: which rc file contains the shim PATH wiring, if any.
fn detect_hook(shims_dir: &Path) -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let needle = shims_dir.to_string_lossy().into_owned();
    let files = [
        ".zshrc",
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".config/fish/config.fish",
    ];
    for f in files {
        let path = home.join(f);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if text.contains(&needle) {
                return Some(f.to_string());
            }
        }
    }
    None
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".wvm-doctor-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn section(title: &str) {
    println!("{title}");
}

fn ok(msg: &str) {
    println!("  \u{2713} {msg}");
}

fn warn(msg: &str) {
    println!("  \u{26a0} {msg}");
}

fn fail(msg: &str) {
    println!("  \u{2717} {msg}");
}
