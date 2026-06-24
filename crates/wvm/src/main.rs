//! WVM native bootstrapper.
//!
//! The only native component of WVM. It establishes the protected seed Wasmtime
//! runtime (downloading it once), materializes the embedded app component, and
//! runs the app on the seed runtime — forwarding all arguments. All real WVM
//! logic lives in the app (`wvm-app`), executed as a WebAssembly component.

mod seed;

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use wvm_core::layout::Layout;

/// The composed app component, embedded at build time (see `build.rs`).
static APP_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.wasm"));

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let layout = Layout::discover()?;
    std::fs::create_dir_all(&layout.root)
        .with_context(|| format!("creating {}", layout.root.display()))?;

    // `exec` is handled natively: a wasm guest cannot spawn a process, and
    // project-pin discovery needs the user's real working directory (which is
    // not preopened into the app sandbox).
    if args.first().map(String::as_str) == Some("exec") {
        return exec_runtime(&layout, &args[1..]);
    }

    materialize_app(&layout)?;
    let _seed_version = seed::ensure(&layout)?;

    launch_app(&layout, &args)
}

/// `wvm exec [--] <args>` — resolve the user's selected runtime and replace
/// this process with it, forwarding the arguments.
fn exec_runtime(layout: &Layout, raw: &[String]) -> Result<()> {
    let forwarded: &[String] = match raw.first() {
        Some(first) if first == "--" => &raw[1..],
        _ => raw,
    };

    let cwd = std::env::current_dir().context("getting current directory")?;
    let resolved = wvm_core::discovery::resolve(layout, &cwd)?;
    if std::env::var_os("WVM_VERBOSE").is_some() {
        eprintln!("wvm: using wasmtime from {} [{}]", resolved.binary.display(), resolved.source);
    }

    // Materialized runtime files are copies (symlink-free under wasm) and may
    // lack the executable bit; restore it before exec.
    ensure_executable(&resolved.binary);

    let mut cmd = Command::new(&resolved.binary);
    cmd.args(forwarded);
    exec_or_run(cmd, &resolved.binary)
}

#[cfg(unix)]
fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode();
        if mode & 0o111 == 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o755));
        }
    }
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) {}

/// Write the embedded app component to `WVM_HOME` if missing or changed.
fn materialize_app(layout: &Layout) -> Result<()> {
    if APP_WASM.is_empty() {
        bail!("no app component embedded; build with `make` (which builds and composes wvm-app)");
    }
    let dest = layout.app_wasm();
    let needs_write = match std::fs::metadata(&dest) {
        Ok(m) => m.len() != APP_WASM.len() as u64,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&dest, APP_WASM)
            .with_context(|| format!("writing {}", dest.display()))?;
    }
    Ok(())
}

/// Run the app component on the seed Wasmtime, forwarding `args`.
fn launch_app(layout: &Layout, args: &[String]) -> Result<()> {
    let seed_bin = layout.seed_bin();
    let app_wasm = layout.app_wasm();
    let home = layout.root.to_str().context("WVM_HOME is not valid UTF-8")?;

    let mut cmd = Command::new(&seed_bin);
    cmd.arg("run")
        .arg("-S")
        .arg("http") // enable wasi:http host support (used from M2)
        .arg("--dir")
        .arg(format!("{home}::{home}"))
        .arg("--env")
        .arg(format!("WVM_HOME={home}"))
        .arg("--env")
        .arg(format!("WVM_HOST_ARCH={}", std::env::consts::ARCH))
        .arg("--env")
        .arg(format!("WVM_HOST_OS={}", std::env::consts::OS));
    if let Ok(v) = std::env::var("WVM_VERBOSE") {
        cmd.arg("--env").arg(format!("WVM_VERBOSE={v}"));
    }
    // Forward the per-session override so the app reflects it in
    // list/current and resolution.
    if let Ok(v) = std::env::var(wvm_core::discovery::SESSION_VAR) {
        cmd.arg("--env").arg(format!("{}={v}", wvm_core::discovery::SESSION_VAR));
    }
    // Everything after the module path is passed to the guest as argv[1..].
    cmd.arg(&app_wasm);
    cmd.args(args);

    exec_or_run(cmd, &seed_bin)
}

#[cfg(unix)]
fn exec_or_run(mut cmd: Command, bin: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = cmd.exec(); // replaces this process on success
    Err(anyhow::anyhow!("failed to exec {}: {err}", bin.display()))
}

#[cfg(not(unix))]
fn exec_or_run(mut cmd: Command, bin: &Path) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("running {}", bin.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}
