//! Integration tests for offline runtime discovery: installed-set scanning,
//! spec resolution, and project-pin-aware effective spec.
//!
//! These deliberately avoid the environment-reading paths (`WVM_VERSION`,
//! `WASMTIME_HOME`, `PATH`), which mutate process-global state and are flaky
//! under parallel test threads.

use tempfile::TempDir;
use wvm_core::discovery;
use wvm_core::layout::{Layout, WASMTIME};
use wvm_core::manifest::{FileEntry, Manifest};

/// A `Layout` rooted at a fresh temp dir, returned with the dir guard.
fn temp_layout() -> (TempDir, Layout) {
    let dir = TempDir::new().expect("create temp dir");
    let layout = Layout {
        root: dir.path().to_path_buf(),
    };
    (dir, layout)
}

/// Create a fake install for `version`: a `bin/wasmtime` file plus a valid
/// `manifest.json`.
fn install(layout: &Layout, version: &str) {
    let vdir = layout.version_dir(WASMTIME, version);
    let bin = vdir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin dir");
    std::fs::write(bin.join("wasmtime"), b"#!/bin/sh\n").expect("write bin");

    let manifest = Manifest {
        runtime: WASMTIME.to_string(),
        version: version.to_string(),
        platform: "test".to_string(),
        archive_sha256: "0".repeat(64),
        files: vec![FileEntry {
            path: "bin/wasmtime".to_string(),
            sha256: "0".repeat(64),
            mode: "0755".to_string(),
            size: 10,
        }],
    };
    manifest
        .write(&layout.manifest_file(WASMTIME, version))
        .expect("write manifest");
}

#[test]
fn installed_versions_lists_sorted_ignoring_junk() {
    let (_dir, layout) = temp_layout();
    install(&layout, "25.0.3");
    install(&layout, "24.0.0");
    install(&layout, "24.0.11");

    // A directory without a manifest is not an install.
    let no_manifest = layout.version_dir(WASMTIME, "26.0.0");
    std::fs::create_dir_all(no_manifest.join("bin")).expect("create dir");

    // A dotfile directory is ignored.
    let dotdir = layout.versions_dir(WASMTIME).join(".tmp");
    std::fs::create_dir_all(&dotdir).expect("create dotdir");
    std::fs::write(dotdir.join("manifest.json"), "{}").expect("write");

    let versions = discovery::installed_versions(&layout).expect("installed_versions");
    assert_eq!(
        versions,
        vec![
            "24.0.0".to_string(),
            "24.0.11".to_string(),
            "25.0.3".to_string(),
        ]
    );
}

#[test]
fn installed_versions_empty_on_fresh_layout() {
    let (_dir, layout) = temp_layout();
    assert!(discovery::installed_versions(&layout)
        .expect("installed_versions")
        .is_empty());
}

#[test]
fn resolve_installed_handles_floating_and_exact() {
    let (_dir, layout) = temp_layout();
    install(&layout, "24.0.0");
    install(&layout, "24.0.11");
    install(&layout, "25.0.3");

    // Floating major line: newest 24.x.
    assert_eq!(
        discovery::resolve_installed(&layout, "24"),
        Some("24.0.11".to_string())
    );
    // Floating major.minor line: newest 24.0.x.
    assert_eq!(
        discovery::resolve_installed(&layout, "24.0"),
        Some("24.0.11".to_string())
    );
    // latest: newest overall.
    assert_eq!(
        discovery::resolve_installed(&layout, "latest"),
        Some("25.0.3".to_string())
    );
    // Exact match.
    assert_eq!(
        discovery::resolve_installed(&layout, "24.0.0"),
        Some("24.0.0".to_string())
    );
    // Nothing installed matches.
    assert_eq!(discovery::resolve_installed(&layout, "99"), None);
}

#[test]
fn effective_spec_at_reads_project_pin() {
    let (_dir, layout) = temp_layout();

    // A separate temp dir as the working directory, holding a project pin.
    let cwd = TempDir::new().expect("create cwd");
    std::fs::write(cwd.path().join("wvm.toml"), "[wvm]\nruntime = \"24\"\n").expect("write pin");

    let (spec, source) = discovery::effective_spec_at(&layout, cwd.path())
        .expect("effective_spec_at")
        .expect("some pin");
    assert_eq!(spec, "24");
    assert!(
        source.starts_with("project pin"),
        "source should name the project pin, got {source:?}"
    );
}
