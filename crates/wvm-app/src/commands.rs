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
use wvm_core::index::{ingest_usage_log, reindex, Index};
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::Manifest;
use wvm_core::{cache, discovery, human_bytes, normalize_version, version_cmp, VersionSpec};

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
    let default_spec = discovery::default_version(&layout);
    // The default is stored as a spec (e.g. `24`); tag the version it resolves
    // to, not the raw spec string.
    let default = default_spec
        .as_deref()
        .and_then(|s| discovery::resolve_installed(&layout, s));
    let effective = discovery::effective_version(&layout);

    let installed = installed_versions(&layout)?;
    let installed_set: HashSet<&str> = installed.iter().map(String::as_str).collect();

    // Best-effort last-used annotations from the usage table (ingesting any
    // pending shim invocations first). Never fatal to listing.
    let now = now_epoch();
    let usage_map: std::collections::HashMap<String, i64> = {
        let mut idx = open_index(&layout).ok();
        if let Some(i) = idx.as_mut() {
            let _ = ingest_usage_log(i, &layout);
        }
        idx.and_then(|i| i.usage_by_version().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|u| (u.version, u.last_used))
            .collect()
    };

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

    if let Some(spec) = &default_spec {
        if VersionSpec::parse(spec)
            .map(|s| s.is_floating())
            .unwrap_or(false)
        {
            match &default {
                Some(v) => println!("Default: {spec} → {v}"),
                None => println!("Default: {spec} (no matching version installed)"),
            }
        }
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
        let usage_note = match usage_map.get(v.as_str()) {
            Some(&t) if installed_set.contains(v.as_str()) => {
                format!("  · used {}", humanize_ago(now, t))
            }
            _ => String::new(),
        };
        println!("{marker} {v}{suffix}{usage_note}");
    }
    if offline {
        eprintln!("(offline: only installed versions shown)");
    }
    Ok(())
}

pub fn current() -> Result<()> {
    let layout = Layout::discover()?;
    match discovery::effective_spec(&layout) {
        Some((spec_str, source)) => match discovery::resolve_installed(&layout, &spec_str) {
            Some(v) => {
                // stdout stays just the concrete version (script-friendly).
                println!("{v}");
                let floating = VersionSpec::parse(&spec_str)
                    .map(|s| s.is_floating())
                    .unwrap_or(false);
                if floating {
                    eprintln!("(resolved from '{spec_str}')");
                }
                if std::env::var_os("WVM_VERBOSE").is_some() {
                    eprintln!("(via {source})");
                }
            }
            None => {
                eprintln!(
                    "selected '{spec_str}' but no matching version is installed; run `wvm install {spec_str}`"
                );
                std::process::exit(1);
            }
        },
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
        // A spec argument resolves against the installed set.
        Some(v) => discovery::resolve_installed(&layout, v)
            .ok_or_else(|| anyhow!("no installed wasmtime matches '{v}'"))?,
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

/// `wvm default <spec>` — set the persistent default (used by new shells). The
/// spec may float (`latest`, `24`, `24.0`) or be exact (`24.0.1`); the newest
/// matching version is installed now so the default is immediately usable, and
/// the **spec** is stored so it keeps tracking its line.
pub fn set_default(spec_arg: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let spec = VersionSpec::parse(spec_arg).map_err(|e| anyhow!(e))?;
    let resolved = install::ensure(spec_arg)?;
    discovery::set_default_version(&layout, &spec.to_string())?;
    if spec.is_floating() {
        println!("Default is now '{spec}' (currently wasmtime {resolved}, used by new shells)");
    } else {
        println!("Default is now wasmtime {resolved} (used by new shells)");
    }
    Ok(())
}

/// `wvm upgrade [spec] [--all]` — pull the newest matching release for a
/// floating line now (rather than waiting for activation-time auto-install).
/// No arg upgrades the default (when it floats); `--all` bumps every installed
/// major line to its newest available patch.
pub fn upgrade(spec_arg: Option<&str>, all: bool) -> Result<()> {
    let layout = Layout::discover()?;
    // Force a fresh remote check regardless of the cache TTL.
    cache::clear(&layout);

    if all {
        let installed = discovery::installed_versions(&layout)?;
        let mut majors: Vec<u64> = installed
            .iter()
            .filter_map(|v| v.split('.').next().and_then(|m| m.parse().ok()))
            .collect();
        majors.sort_unstable();
        majors.dedup();
        if majors.is_empty() {
            println!("Nothing installed to upgrade.");
            return Ok(());
        }
        for m in majors {
            upgrade_one(&layout, &m.to_string())?;
        }
        return Ok(());
    }

    match spec_arg {
        Some(s) => upgrade_one(&layout, s),
        None => match discovery::default_version(&layout) {
            Some(spec_str) => {
                let spec = VersionSpec::parse(&spec_str).map_err(|e| anyhow!(e))?;
                if !spec.is_floating() {
                    println!("Default is pinned to exact {spec_str}; nothing to upgrade.");
                    return Ok(());
                }
                upgrade_one(&layout, &spec_str)
            }
            None => {
                println!("No default set; pass a spec (e.g. `wvm upgrade 24`) or `--all`.");
                Ok(())
            }
        },
    }
}

fn upgrade_one(layout: &Layout, spec_str: &str) -> Result<()> {
    let before = discovery::resolve_installed(layout, spec_str);
    let after = install::ensure(spec_str)?;
    match before {
        Some(b) if b == after => println!("{spec_str}: already up to date ({after})"),
        Some(b) => println!("{spec_str}: {b} → {after}"),
        None => println!("{spec_str}: installed {after}"),
    }
    Ok(())
}

/// `wvm use <version>` — switch the runtime for the **current shell only**.
///
/// A binary cannot change its parent shell's environment, so when run under the
/// shell hook (stdout captured) this prints `export WVM_VERSION=<v>` for the
/// hook to `eval`; when run directly in a terminal it explains how to enable the
/// hook.
pub fn use_version(spec_arg: &str) -> Result<()> {
    let spec = VersionSpec::parse(spec_arg).map_err(|e| anyhow!(e))?;
    // Auto-install the newest match so the session var is immediately usable.
    let resolved = install::ensure(spec_arg)?;

    if crate::progress::stdout_is_terminal() {
        eprintln!("wasmtime {resolved} is installed.");
        eprintln!(
            "`wvm use` switches the runtime for the current shell, which needs the shell hook:"
        );
        eprintln!("    wvm shell-init >> ~/.zshrc   # once, then restart your shell");
        eprintln!("Then `wvm use {spec}` applies to this shell. For the persistent default: `wvm default {spec}`.");
    } else {
        // Store the spec (not the resolved version) so the session floats too.
        println!("export {}={spec}", discovery::SESSION_VAR);
        if spec.is_floating() {
            eprintln!("Now using '{spec}' (wasmtime {resolved}) for this shell");
        } else {
            eprintln!("Now using wasmtime {resolved} (this shell)");
        }
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

/// `wvm shell-init` — print the shell integration: put the shim on `PATH` (so
/// apps that call `wasmtime` route through wvm) and define the `use` hook.
pub fn shell_init() -> Result<()> {
    let layout = Layout::discover()?;
    let shims = layout.shims_dir();
    println!("# wvm shell integration — add to ~/.zshrc or ~/.bashrc");
    println!("# Put the wvm shim ahead of PATH so `wasmtime` routes through wvm");
    println!("# (resolves the active version and records usage transparently).");
    println!("export PATH=\"{}:$PATH\"", shims.display());
    print!("{SHELL_HOOK}");
    Ok(())
}

const SHELL_HOOK: &str = r#"wvm() {
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
    // Accept a spec: `uninstall 24` resolves to the newest installed 24.x.
    let version = discovery::resolve_installed(&layout, version_arg)
        .unwrap_or_else(|| normalize_version(version_arg));
    if version != normalize_version(version_arg) {
        eprintln!("Resolved '{version_arg}' to installed wasmtime {version}");
    }

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
    // Flush any pending shim invocations, then reconcile object state.
    let _ = ingest_usage_log(&mut index, &layout);
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
    } else if prune {
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

    report_stale_runtimes(&layout, &index)?;
    Ok(())
}

/// Advisory: installed runtimes not used in a while and safe to consider for
/// removal (not the seed, default, or an app dependency). Judged only when
/// there is observed usage to compare against — otherwise there is no basis and
/// nothing is printed. `WVM_STALE_DAYS` overrides the 90-day threshold.
fn report_stale_runtimes(layout: &Layout, index: &ComponentIndex) -> Result<()> {
    let usage: std::collections::HashMap<String, i64> = index
        .usage_by_version()?
        .into_iter()
        .map(|u| (u.version, u.last_used))
        .collect();
    if usage.is_empty() {
        return Ok(()); // no observations yet — can't judge staleness
    }

    let threshold_days: i64 = std::env::var("WVM_STALE_DAYS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(90);
    let now = now_epoch();
    let seed = seed_version(layout);
    let default_resolved =
        discovery::default_version(layout).and_then(|s| discovery::resolve_installed(layout, &s));

    let mut stale: Vec<(String, String)> = Vec::new();
    for v in installed_versions(layout)? {
        if seed.as_deref() == Some(v.as_str()) || default_resolved.as_deref() == Some(v.as_str()) {
            continue;
        }
        if !index.apps_using(&v).unwrap_or_default().is_empty() {
            continue;
        }
        match usage.get(v.as_str()) {
            Some(&t) => {
                let age = (now - t) / 86400;
                if age >= threshold_days {
                    stale.push((v, format!("last used {age}d ago")));
                }
            }
            None => stale.push((v, "never used".to_string())),
        }
    }

    if !stale.is_empty() {
        println!("\nStale runtimes (unused ≥ {threshold_days}d; not seed/default/app-required):");
        for (v, note) in stale {
            println!("  {v}   {note}   → wvm uninstall {v}");
        }
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

/// `wvm usage [--limit N]` — show observed runtime invocations recorded by the
/// pass-through shim (transparent tracking; no app registration required).
pub fn usage(limit: i64) -> Result<()> {
    let layout = Layout::discover()?;
    let mut index = open_index(&layout)?;
    ingest_usage_log(&mut index, &layout)?;

    let by_version = index.usage_by_version()?;
    if by_version.is_empty() {
        println!("No runtime usage recorded yet.");
        println!(
            "Put the shim on PATH (`wvm shell-init`) so apps that call `wasmtime` are tracked."
        );
        return Ok(());
    }

    let now = now_epoch();
    println!("Runtime usage (observed via the shim):");
    for u in &by_version {
        println!(
            "  {:<10} {:>6} run(s), last {}",
            u.version,
            u.count,
            humanize_ago(now, u.last_used)
        );
    }

    let recent = index.recent_usage(limit)?;
    if !recent.is_empty() {
        println!("\nRecent invocations:");
        for e in &recent {
            let who = e.app.as_deref().or(e.caller.as_deref()).unwrap_or("?");
            let cwd = e.cwd.as_deref().unwrap_or("");
            println!(
                "  {:<8} {:<18} {}  {}",
                e.version,
                who,
                humanize_ago(now, e.invoked_at),
                cwd
            );
        }
    }
    Ok(())
}

/// Render `then` as a coarse "… ago" relative to `now`.
fn humanize_ago(now: i64, then: i64) -> String {
    let d = (now - then).max(0);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}

/// `wvm apps` — list registered applications and the runtimes they depend on.
pub fn apps() -> Result<()> {
    let layout = Layout::discover()?;
    let mut index = open_index(&layout)?;
    ingest_usage_log(&mut index, &layout)?;
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
