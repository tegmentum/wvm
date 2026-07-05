//! Integration tests for the plain-file storage layers: application
//! registrations (`apps.json`) and observed usage (`usage.log`).

use tempfile::TempDir;
use wvm_core::apps;
use wvm_core::layout::Layout;
use wvm_core::usage::{self, UsageEntry};

/// A `Layout` rooted at a fresh temp dir, returned alongside the dir guard so
/// it lives for the test's duration.
fn temp_layout() -> (TempDir, Layout) {
    let dir = TempDir::new().expect("create temp dir");
    let layout = Layout {
        root: dir.path().to_path_buf(),
    };
    (dir, layout)
}

/// Minimal usage entry with only the required fields populated.
fn entry(version: &str, invoked_at: i64) -> UsageEntry {
    UsageEntry {
        version: version.to_string(),
        runtime_path: None,
        app: None,
        caller: None,
        cwd: None,
        args: Vec::new(),
        module: None,
        module_path: None,
        module_sha256: None,
        manifest: None,
        invoked_at,
    }
}

// --- apps.rs ---------------------------------------------------------------

#[test]
fn apps_read_empty_on_fresh_layout() {
    let (_dir, layout) = temp_layout();
    let apps = apps::read(&layout).expect("read");
    assert!(apps.is_empty(), "fresh layout has no apps");
}

#[test]
fn apps_register_roundtrips_and_upserts() {
    let (_dir, layout) = temp_layout();

    apps::register(
        &layout,
        "alpha",
        Some("/apps/alpha"),
        None,
        &["24.0.0".to_string()],
        100,
    )
    .expect("register alpha");

    let apps = apps::read(&layout).expect("read");
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].name, "alpha");
    assert_eq!(apps[0].path.as_deref(), Some("/apps/alpha"));
    assert_eq!(apps[0].runtimes, vec!["24.0.0".to_string()]);
    assert_eq!(apps[0].registered_at, 100);

    // Registering the same name again REPLACES, not duplicates.
    apps::register(
        &layout,
        "alpha",
        Some("/apps/alpha-v2"),
        None,
        &["25.0.3".to_string()],
        200,
    )
    .expect("re-register alpha");

    let apps = apps::read(&layout).expect("read");
    assert_eq!(apps.len(), 1, "upsert must not duplicate");
    assert_eq!(apps[0].path.as_deref(), Some("/apps/alpha-v2"));
    assert_eq!(apps[0].runtimes, vec!["25.0.3".to_string()]);
    assert_eq!(apps[0].registered_at, 200);
}

#[test]
fn apps_unregister_reports_and_removes() {
    let (_dir, layout) = temp_layout();
    apps::register(&layout, "alpha", None, None, &[], 1).expect("register");
    apps::register(&layout, "beta", None, None, &[], 2).expect("register");

    // Missing app: false, nothing changed.
    assert!(!apps::unregister(&layout, "missing").expect("unregister missing"));
    assert_eq!(apps::read(&layout).expect("read").len(), 2);

    // Existing app: true, and it is gone.
    assert!(apps::unregister(&layout, "alpha").expect("unregister alpha"));
    let names: Vec<String> = apps::read(&layout)
        .expect("read")
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["beta".to_string()]);
}

#[test]
fn apps_using_filters_and_sorts() {
    let (_dir, layout) = temp_layout();
    apps::register(
        &layout,
        "zeta",
        None,
        None,
        &["24.0.0".to_string(), "25.0.3".to_string()],
        1,
    )
    .expect("register zeta");
    apps::register(&layout, "alpha", None, None, &["24.0.0".to_string()], 2)
        .expect("register alpha");
    apps::register(&layout, "gamma", None, None, &["25.0.3".to_string()], 3)
        .expect("register gamma");

    // Only apps whose runtimes contain the version, sorted by name.
    assert_eq!(
        apps::apps_using(&layout, "24.0.0").expect("apps_using"),
        vec!["alpha".to_string(), "zeta".to_string()]
    );
    assert_eq!(
        apps::apps_using(&layout, "25.0.3").expect("apps_using"),
        vec!["gamma".to_string(), "zeta".to_string()]
    );
    assert!(apps::apps_using(&layout, "99.0.0")
        .expect("apps_using")
        .is_empty());
}

// --- usage.rs --------------------------------------------------------------

#[test]
fn usage_record_appends_and_reads_in_order() {
    let (_dir, layout) = temp_layout();

    // Fresh layout: no log yet.
    assert!(usage::read(&layout).expect("read").is_empty());

    usage::record(&layout, &entry("24.0.0", 10)).expect("record");
    usage::record(&layout, &entry("25.0.3", 20)).expect("record");
    usage::record(&layout, &entry("24.0.0", 30)).expect("record");

    let entries = usage::read(&layout).expect("read");
    let seen: Vec<(String, i64)> = entries
        .into_iter()
        .map(|e| (e.version, e.invoked_at))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("24.0.0".to_string(), 10),
            ("25.0.3".to_string(), 20),
            ("24.0.0".to_string(), 30),
        ]
    );
}

#[test]
fn usage_by_version_aggregates_and_sorts() {
    let entries = vec![
        entry("24.0.0", 10),
        entry("25.0.3", 40),
        entry("24.0.0", 30),
        entry("25.0.3", 20),
    ];
    let rollup = usage::by_version(&entries);

    // Most-recent-first: 25.0.3 last used at 40, 24.0.0 at 30.
    assert_eq!(rollup.len(), 2);
    assert_eq!(rollup[0].version, "25.0.3");
    assert_eq!(rollup[0].count, 2);
    assert_eq!(rollup[0].last_used, 40);
    assert_eq!(rollup[1].version, "24.0.0");
    assert_eq!(rollup[1].count, 2);
    assert_eq!(rollup[1].last_used, 30);
}

#[test]
fn usage_recent_returns_newest_limited() {
    let entries = vec![
        entry("a", 10),
        entry("b", 50),
        entry("c", 30),
        entry("d", 40),
        entry("e", 20),
    ];
    let recent = usage::recent(&entries, 3);
    let order: Vec<i64> = recent.iter().map(|e| e.invoked_at).collect();
    assert_eq!(order, vec![50, 40, 30]);
}

#[test]
fn usage_read_compacts_to_cap() {
    // Mirror the private `CAP` in usage.rs.
    const CAP: usize = 10_000;
    let (_dir, layout) = temp_layout();

    // Write CAP + 50 entries with increasing timestamps directly to the log,
    // one JSON line each, to avoid CAP+ separate append syscalls.
    let path = layout.usage_log();
    let mut text = String::new();
    let total = CAP + 50;
    for i in 0..total {
        let line = serde_json::to_string(&entry("24.0.0", i as i64)).expect("serialize");
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::write(&path, &text).expect("write log");

    // Read compacts in place to the most recent CAP entries.
    let entries = usage::read(&layout).expect("read");
    assert_eq!(entries.len(), CAP, "read returns exactly CAP entries");
    assert_eq!(
        entries.first().map(|e| e.invoked_at),
        Some(50),
        "oldest 50 entries were dropped"
    );
    assert_eq!(
        entries.last().map(|e| e.invoked_at),
        Some((total - 1) as i64),
        "newest entry retained"
    );

    // The on-disk log was rewritten to CAP lines.
    let on_disk = std::fs::read_to_string(&path).expect("re-read log");
    assert_eq!(
        on_disk.lines().filter(|l| !l.trim().is_empty()).count(),
        CAP,
        "usage.log rewritten to CAP entries"
    );
}
