//! Build orchestration for wvm, run as `cargo xtask <task>`.
//!
//! The native `wvm` binary embeds the wasm app component, so the build is two
//! steps: build `wvm-app` for `wasm32-wasip2`, then build `wvm` with
//! `WVM_APP_WASM` pointing at that artifact (its `build.rs` `include_bytes!`s
//! it). This replaces the old Makefile — no `make` dependency, cross-platform.
//!
//! Tasks: `build` (default), `ci`, `act`.

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

fn main() {
    let task = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "build".to_string());
    let ok = match task.as_str() {
        "build" | "all" => build(),
        "ci" => ci(),
        "act" => act(),
        other => {
            eprintln!("unknown task '{other}'\ntasks: build, ci, act");
            false
        }
    };
    if !ok {
        exit(1);
    }
}

/// Workspace root (this crate lives in `<root>/xtask`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent dir")
        .to_path_buf()
}

/// Absolute path of the built wasm app component.
fn app_wasm() -> PathBuf {
    root().join("target/wasm32-wasip2/release/wvm_app.wasm")
}

fn run(program: &str, args: &[&str], env: &[(&str, String)]) -> bool {
    eprintln!("> {program} {}", args.join(" "));
    let mut cmd = Command::new(program);
    cmd.current_dir(root()).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.status() {
        Ok(status) => status.success(),
        Err(e) => {
            eprintln!("failed to run {program}: {e}");
            false
        }
    }
}

/// Build the wasm app, then the native binary embedding it.
fn build() -> bool {
    run(
        "cargo",
        &[
            "build",
            "-p",
            "wvm-app",
            "--target",
            "wasm32-wasip2",
            "--release",
        ],
        &[],
    ) && run(
        "cargo",
        &["build", "-p", "wvm", "--release"],
        &[("WVM_APP_WASM".into(), app_wasm().display().to_string())],
    )
}

/// The same checks CI runs, locally and without Docker.
fn ci() -> bool {
    run("cargo", &["fmt", "--all", "--check"], &[])
        && build()
        && run(
            "cargo",
            &[
                "clippy",
                "-p",
                "wvm-core",
                "-p",
                "wvm",
                "--release",
                "--",
                "-D",
                "warnings",
            ],
            &[],
        )
        && run(
            "cargo",
            &[
                "clippy",
                "-p",
                "wvm-app",
                "--target",
                "wasm32-wasip2",
                "--release",
                "--",
                "-D",
                "warnings",
            ],
            &[],
        )
        && run("cargo", &["test"], &[])
}

/// Run the CI workflow locally in Docker via nektos/act. Resolves the active
/// Docker context's socket so it works on Colima without exporting DOCKER_HOST.
fn act() -> bool {
    let sock = Command::new("docker")
        .args([
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unix:///var/run/docker.sock".to_string());
    run(
        "act",
        &["-W", ".github/workflows/ci.yml"],
        &[("DOCKER_HOST".into(), sock)],
    )
}
