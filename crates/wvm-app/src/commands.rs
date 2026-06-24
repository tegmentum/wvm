//! WVM command implementations, executed inside the wasm app component.
//!
//! Filesystem/CAS logic comes from `wvm-core`; the index is the
//! `sqlite:wasm/high-level` component via [`ComponentIndex`].

use crate::index_component::ComponentIndex;
use anyhow::{anyhow, bail, Context, Result};
use wvm_core::index::{reindex, Index};
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::Manifest;
use wvm_core::{discovery, human_bytes, normalize_version, version_cmp};

/// Open the index DB through the SQLite component.
pub fn open_index(layout: &Layout) -> Result<ComponentIndex> {
    let path = layout.db_file();
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("WVM_HOME path is not valid UTF-8"))?;
    ComponentIndex::open(path)
}

pub fn list() -> Result<()> {
    let layout = Layout::discover()?;
    layout.ensure_base()?;

    // Reconcile the index from disk (also exercises the SQLite component).
    let mut index = open_index(&layout)?;
    reindex(&mut index, &layout)?;

    if let Some(seed) = seed_version(&layout) {
        println!("Seed runtime (protected)");
        println!("  {seed}  [seed]");
    }

    let versions = installed_versions(&layout)?;
    let active = discovery::active_version(&layout);
    if versions.is_empty() {
        println!("No user runtimes installed. Try `wvm install latest`.");
    } else {
        println!("Installed Runtimes");
        for v in versions {
            let marker = if active.as_deref() == Some(v.as_str()) { "*" } else { " " };
            println!("{marker} {v}");
        }
    }
    Ok(())
}

pub fn current() -> Result<()> {
    let layout = Layout::discover()?;
    match discovery::active_version(&layout) {
        Some(v) => println!("{v}"),
        None => {
            eprintln!("no active runtime (use `wvm use <version>`)");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub fn path(version: Option<&str>) -> Result<()> {
    let layout = Layout::discover()?;
    let version = match version {
        Some(v) => normalize_version(v),
        None => discovery::active_version(&layout)
            .ok_or_else(|| anyhow!("no active runtime; pass a version or run `wvm use <version>`"))?,
    };
    let dir = layout.version_dir(WASMTIME, &version);
    if !dir.exists() {
        bail!("wasmtime {version} is not installed");
    }
    println!("{}", dir.display());
    Ok(())
}

pub fn use_version(version_arg: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let version = normalize_version(version_arg);
    if !is_installed(&layout, &version) {
        bail!("wasmtime {version} is not installed; run `wvm install {version}`");
    }
    discovery::set_active_version(&layout, &version)?;
    println!("Now using wasmtime {version}");
    Ok(())
}

pub fn uninstall(version_arg: &str) -> Result<()> {
    let layout = Layout::discover()?;
    let version = normalize_version(version_arg);

    if seed_version(&layout).as_deref() == Some(version.as_str()) {
        bail!("wasmtime {version} is the protected seed runtime and cannot be removed");
    }

    let dir = layout.version_dir(WASMTIME, &version);
    if !dir.exists() {
        bail!("wasmtime {version} is not installed");
    }
    std::fs::remove_dir_all(&dir)
        .with_context(|| format!("removing {}", dir.display()))?;

    if let Ok(mut index) = open_index(&layout) {
        let _ = index.remove_version(WASMTIME, &version);
    }

    if discovery::active_version(&layout).as_deref() == Some(version.as_str()) {
        let _ = std::fs::remove_file(layout.active_file(WASMTIME));
        eprintln!("note: {version} was the active runtime; no runtime is active now");
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
                std::fs::remove_file(&p)
                    .with_context(|| format!("removing {}", p.display()))?;
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
        println!("  {}  {:>10}  {who}", &digest[..12], human_bytes(size.max(0) as u64));
    }
    Ok(())
}

// --- helpers -------------------------------------------------------------

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
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
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
