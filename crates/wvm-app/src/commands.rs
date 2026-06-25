//! WVM command implementations, executed inside the wasm app component.
//!
//! Filesystem/CAS logic comes from `wvm-core`; the index is the
//! `sqlite:wasm/high-level` component via [`ComponentIndex`].

use crate::index_component::ComponentIndex;
use crate::install;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::path::Path;
use wvm_core::appmanifest::AppManifest;
use wvm_core::index::{reindex, Index};
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::Manifest;
use wvm_core::{discovery, human_bytes, normalize_version, version_cmp};

fn now_epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Open the index DB through the SQLite component.
pub fn open_index(layout: &Layout) -> Result<ComponentIndex> {
    let path = layout.db_file();
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("WVM_HOME path is not valid UTF-8"))?;
    ComponentIndex::open(path)
}

/// `wvm list [--all]` — one list of all available versions (most recent first),
/// with installed ones marked. Falls back to installed-only when offline.
pub fn list(all: bool) -> Result<()> {
    let layout = Layout::discover()?;
    layout.ensure_base()?;

    let seed = seed_version(&layout);
    let default = discovery::default_version(&layout);
    let effective = discovery::effective_version(&layout);

    let installed = installed_versions(&layout)?;
    let installed_set: HashSet<&str> = installed.iter().map(String::as_str).collect();

    // Try the remote list; fall back to installed-only when offline.
    let (mut versions, offline) = match install::fetch_release_versions(all) {
        Ok(mut v) => {
            for i in &installed {
                if !v.contains(i) {
                    v.push(i.clone());
                }
            }
            (v, false)
        }
        Err(e) => {
            eprintln!("warning: could not fetch available versions ({e}); showing installed only");
            (installed.clone(), true)
        }
    };
    versions.sort_by(|a, b| version_cmp(b, a)); // most recent first
    versions.dedup();

    if versions.is_empty() {
        println!("No runtimes available. Try again with a network connection.");
        return Ok(());
    }

    println!("Wasmtime runtimes  (* current; tags: lts, installed, default, seed)");
    for v in &versions {
        let is_current = effective.as_ref().map(|(e, _)| e == v).unwrap_or(false);
        let marker = if is_current { "*" } else { " " };
        let mut tags: Vec<&str> = Vec::new();
        if wvm_core::is_lts(v) {
            tags.push("lts");
        }
        if seed.as_deref() == Some(v.as_str()) {
            tags.push("seed");
        }
        if installed_set.contains(v.as_str()) {
            tags.push("installed");
        }
        if default.as_deref() == Some(v.as_str()) {
            tags.push("default");
        }
        let suffix = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(", "))
        };
        println!("{marker} {v}{suffix}");
    }
    if offline {
        eprintln!("(offline: only installed versions shown)");
    }
    Ok(())
}

pub fn current() -> Result<()> {
    let layout = Layout::discover()?;
    match discovery::effective_version(&layout) {
        Some((v, source)) => {
            println!("{v}");
            if std::env::var_os("WVM_VERBOSE").is_some() {
                eprintln!("(via {source})");
            }
        }
        None => {
            eprintln!("no default runtime set (use `wvm default <version>`)");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub fn path(version: Option<&str>) -> Result<()> {
    let layout = Layout::discover()?;
    let version = match version {
        Some(v) => normalize_version(v),
        None => discovery::effective_version(&layout)
            .map(|(v, _)| v)
            .ok_or_else(|| {
                anyhow!("no default runtime; pass a version or run `wvm default <version>`")
            })?,
    };
    let dir = layout.version_dir(WASMTIME, &version);
    if !dir.exists() {
        bail!("wasmtime {version} is not installed");
    }
    println!("{}", dir.display());
    Ok(())
}

/// `wvm default <version>` — set the persistent default (used by new shells).
pub fn set_default(version_arg: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let version = normalize_version(version_arg);
    if !is_installed(&layout, &version) {
        bail!("wasmtime {version} is not installed; run `wvm install {version}`");
    }
    discovery::set_default_version(&layout, &version)?;
    println!("Default is now wasmtime {version} (used by new shells)");
    Ok(())
}

/// `wvm use <version>` — switch the runtime for the **current shell only**.
///
/// A binary cannot change its parent shell's environment, so when run under the
/// shell hook (stdout captured) this prints `export WVM_VERSION=<v>` for the
/// hook to `eval`; when run directly in a terminal it explains how to enable the
/// hook.
pub fn use_version(version_arg: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let version = normalize_version(version_arg);
    if !is_installed(&layout, &version) {
        bail!("wasmtime {version} is not installed; run `wvm install {version}`");
    }

    if crate::progress::stdout_is_terminal() {
        eprintln!("wasmtime {version} is installed.");
        eprintln!(
            "`wvm use` switches the runtime for the current shell, which needs the shell hook:"
        );
        eprintln!("    wvm shell-init >> ~/.zshrc   # once, then restart your shell");
        eprintln!("Then `wvm use {version}` applies to this shell. For the persistent default: `wvm default {version}`.");
    } else {
        println!("export {}={version}", discovery::SESSION_VAR);
        eprintln!("Now using wasmtime {version} (this shell)");
    }
    Ok(())
}

/// `wvm deactivate` — clear the per-shell override (revert to the default).
pub fn deactivate() -> Result<()> {
    let layout = Layout::discover()?;
    if crate::progress::stdout_is_terminal() {
        eprintln!("`wvm deactivate` clears the per-shell override and needs the shell hook (`wvm shell-init`).");
    } else {
        println!("unset {}", discovery::SESSION_VAR);
        match discovery::default_version(&layout) {
            Some(d) => eprintln!("Reverted to default (wasmtime {d}) for this shell"),
            None => eprintln!("Cleared session override (no default set)"),
        }
    }
    Ok(())
}

/// `wvm shell-init` — print the shell function enabling per-shell `wvm use`.
pub fn shell_init() -> Result<()> {
    print!("{SHELL_HOOK}");
    Ok(())
}

const SHELL_HOOK: &str = r#"# wvm shell integration — add to ~/.zshrc or ~/.bashrc
wvm() {
  case "$1" in
    use|deactivate)
      local __wvm_out
      __wvm_out="$(command wvm "$@")" || return $?
      [ -n "$__wvm_out" ] && eval "$__wvm_out"
      ;;
    *)
      command wvm "$@" ;;
  esac
}
"#;

pub fn uninstall(version_arg: &str, force: bool) -> Result<()> {
    let layout = Layout::discover()?;
    let version = normalize_version(version_arg);

    if seed_version(&layout).as_deref() == Some(version.as_str()) {
        bail!("wasmtime {version} is the protected seed runtime and cannot be removed");
    }

    let dir = layout.version_dir(WASMTIME, &version);
    if !dir.exists() {
        bail!("wasmtime {version} is not installed");
    }

    // Gate on registered application dependencies.
    let dependents = open_index(&layout)
        .and_then(|idx| idx.apps_using(&version))
        .unwrap_or_default();
    if !dependents.is_empty() {
        if !force {
            bail!(
                "wasmtime {version} is required by registered app(s): {}.\n\
                 Migrate them or re-run with --force to remove anyway.",
                dependents.join(", ")
            );
        }
        eprintln!(
            "warning: removing wasmtime {version} still required by: {}",
            dependents.join(", ")
        );
    }

    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;

    if let Ok(mut index) = open_index(&layout) {
        let _ = index.remove_version(WASMTIME, &version);
    }

    if discovery::default_version(&layout).as_deref() == Some(version.as_str()) {
        let _ = std::fs::remove_file(layout.default_file(WASMTIME));
        eprintln!("note: {version} was the default; no default is set now");
    }
    println!("Uninstalled wasmtime {version}");
    println!("Run `wvm gc --prune` to reclaim unreferenced store objects.");
    Ok(())
}

pub fn verify(version_arg: Option<&str>) -> Result<()> {
    let layout = Layout::discover()?;
    let versions = match version_arg {
        Some(v) => vec![normalize_version(v)],
        None => installed_versions(&layout)?,
    };
    if versions.is_empty() {
        println!("No runtimes installed.");
        return Ok(());
    }

    let mut problems = 0usize;
    for version in &versions {
        let manifest_path = layout.manifest_file(WASMTIME, version);
        if !manifest_path.is_file() {
            println!("✗ {version}: missing manifest");
            problems += 1;
            continue;
        }
        let manifest = Manifest::read(&manifest_path)?;
        let version_dir = layout.version_dir(WASMTIME, version);
        let mut ok = true;
        for entry in &manifest.files {
            let p = version_dir.join(&entry.path);
            if !p.exists() {
                println!("✗ {version}: {} is missing", entry.path);
                ok = false;
                continue;
            }
            let actual = wvm_core::hash::sha256_file(&p)?;
            if actual != entry.sha256 {
                println!("✗ {version}: {} digest mismatch", entry.path);
                ok = false;
            }
        }
        if ok {
            println!("✓ {version}: {} files verified", manifest.files.len());
        } else {
            problems += 1;
        }
    }
    if problems > 0 {
        bail!("{problems} runtime(s) failed verification");
    }
    Ok(())
}

pub fn gc(prune: bool) -> Result<()> {
    let layout = Layout::discover()?;
    let mut index = open_index(&layout)?;
    // Reconcile from authoritative on-disk state before deciding.
    reindex(&mut index, &layout)?;

    let stats = index.stats()?;
    println!(
        "Store: {} object(s), {} referenced, {} total.",
        stats.objects,
        stats.referenced,
        human_bytes(stats.total_size.max(0) as u64)
    );

    let unreferenced = index.unreferenced_objects()?;
    let reclaimable: i64 = unreferenced.iter().map(|(_, s)| *s).sum();
    if unreferenced.is_empty() {
        println!("Nothing to reclaim.");
        return Ok(());
    }

    if prune {
        for (digest, _) in &unreferenced {
            let p = layout.object_path(digest);
            if p.exists() {
                std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            }
            index.delete_object(digest)?;
        }
        println!(
            "Pruned {} unreferenced object(s), reclaimed {}.",
            unreferenced.len(),
            human_bytes(reclaimable.max(0) as u64)
        );
    } else {
        println!(
            "{} unreferenced object(s), {} reclaimable. Run `wvm gc --prune` to delete.",
            unreferenced.len(),
            human_bytes(reclaimable.max(0) as u64)
        );
    }
    Ok(())
}

pub fn objects() -> Result<()> {
    let layout = Layout::discover()?;
    let mut index = open_index(&layout)?;
    reindex(&mut index, &layout)?;

    let stats = index.stats()?;
    let all = index.all_objects()?;
    if all.is_empty() {
        println!("Store is empty.");
        return Ok(());
    }
    println!(
        "Objects ({} total, {} referenced, {})",
        stats.objects,
        stats.referenced,
        human_bytes(stats.total_size.max(0) as u64)
    );
    for (digest, size) in all {
        let refs = index.backlinks(&digest)?;
        let who = if refs.is_empty() {
            "(unreferenced)".to_string()
        } else {
            refs.iter()
                .map(|(rt, ver)| format!("{rt}@{ver}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {}  {:>10}  {who}",
            &digest[..12],
            human_bytes(size.max(0) as u64)
        );
    }
    Ok(())
}

// --- helpers -------------------------------------------------------------

/// `wvm register <app-dir>` — read the app's `wvm.toml` `[app]` manifest and
/// cache the dependency in wvm's index (advisory; the app never needs wvm).
pub fn register(app_dir: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let dir = Path::new(app_dir);
    let manifest = AppManifest::read_dir(dir)?;

    let mut index = open_index(&layout)?;
    index.register_app(
        &manifest.name,
        Some(app_dir),
        manifest.runtime_path.as_deref(),
        &manifest.runtimes,
        now_epoch(),
    )?;

    println!("Registered application '{}'", manifest.name);
    if let Some(p) = &manifest.runtime_path {
        println!("  custom runtime: {p}");
    }
    if !manifest.runtimes.is_empty() {
        for v in &manifest.runtimes {
            let note = if is_installed(&layout, v) {
                ""
            } else {
                "  (not installed)"
            };
            println!("  runtime: {v}{note}");
        }
    }
    Ok(())
}

/// `wvm unregister <name>` — drop an application's registration.
pub fn unregister(name: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let mut index = open_index(&layout)?;
    if index.unregister_app(name)? {
        println!("Unregistered application '{name}'");
    } else {
        bail!("no application named '{name}' is registered");
    }
    Ok(())
}

/// `wvm apps` — list registered applications and the runtimes they depend on.
pub fn apps() -> Result<()> {
    let layout = Layout::discover()?;
    let index = open_index(&layout)?;
    let apps = index.list_apps()?;
    if apps.is_empty() {
        println!("No applications registered. Register one with `wvm register <app-dir>`.");
        return Ok(());
    }

    println!("Registered applications:");
    for app in apps {
        let mut parts: Vec<String> = Vec::new();
        if !app.runtimes.is_empty() {
            let versions = app
                .runtimes
                .iter()
                .map(|v| {
                    if is_installed(&layout, v) {
                        v.clone()
                    } else {
                        format!("{v} (not installed)")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("runtimes: {versions}"));
        }
        if let Some(p) = &app.runtime_path {
            parts.push(format!("custom runtime: {p}"));
        }
        let detail = if parts.is_empty() {
            "(no runtimes)".to_string()
        } else {
            parts.join("; ")
        };
        println!("  {}  {detail}", app.name);
        if let Some(p) = &app.path {
            println!("      at {p}");
        }
    }
    Ok(())
}

pub fn seed_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.seed_marker()).ok()?;
    let v = text.trim();
    (!v.is_empty()).then(|| v.to_string())
}

fn is_installed(layout: &Layout, version: &str) -> bool {
    layout.manifest_file(WASMTIME, version).is_file()
}

fn installed_versions(layout: &Layout) -> Result<Vec<String>> {
    let dir = layout.versions_dir(WASMTIME);
    let mut versions = Vec::new();
    if dir.exists() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().join("manifest.json").is_file() {
                versions.push(name);
            }
        }
    }
    versions.sort_by(|a, b| version_cmp(a, b));
    Ok(versions)
}
