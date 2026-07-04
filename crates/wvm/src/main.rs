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
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // Print the wvm version without bootstrapping a runtime.
    if matches!(
        args.first().map(String::as_str),
        Some("--version") | Some("-V")
    ) {
        println!("wvm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let layout = Layout::discover()?;
    std::fs::create_dir_all(&layout.root)
        .with_context(|| format!("creating {}", layout.root.display()))?;

    // `exec` is handled natively: a wasm guest cannot spawn a process, and
    // project-pin discovery needs the user's real working directory (which is
    // not preopened into the app sandbox).
    if args.first().map(String::as_str) == Some("exec") {
        return exec_runtime(&layout, &args[1..]);
    }

    // `register <app-dir>` reads a manifest outside WVM_HOME; preopen that
    // directory for the app and pass it the canonical absolute path.
    let extra_dir = register_dir(&mut args);

    materialize_app(&layout)?;
    let _seed_version = seed::ensure(&layout)?;

    launch_app(&layout, &args, extra_dir.as_deref())
}

/// For `register <dir>`, canonicalize the directory argument (rewriting it in
/// place) and return it so the bootstrapper can preopen it for the app.
fn register_dir(args: &mut [String]) -> Option<std::path::PathBuf> {
    if args.first().map(String::as_str) != Some("register") {
        return None;
    }
    let idx = args.iter().skip(1).position(|a| !a.starts_with('-'))? + 1;
    let abs = std::fs::canonicalize(&args[idx]).ok()?;
    args[idx] = abs.to_string_lossy().into_owned();
    Some(abs)
}

/// `wvm exec [--] <args>` — resolve the user's selected runtime and replace
/// this process with it, forwarding the arguments.
fn exec_runtime(layout: &Layout, raw: &[String]) -> Result<()> {
    let forwarded: &[String] = match raw.first() {
        Some(first) if first == "--" => &raw[1..],
        _ => raw,
    };

    let cwd = std::env::current_dir().context("getting current directory")?;

    // Activation-time auto-install: a floating spec (e.g. `24`) pulls the newest
    // matching version if one has appeared. Best-effort — network failures fall
    // back to the best already-installed match.
    if let Err(e) = ensure_active_runtime(layout, &cwd) {
        if std::env::var_os("WVM_VERBOSE").is_some() {
            eprintln!("wvm: auto-install check skipped: {e:#}");
        }
    }

    let resolved = wvm_core::discovery::resolve(layout, &cwd)?;
    if std::env::var_os("WVM_VERBOSE").is_some() {
        eprintln!(
            "wvm: using wasmtime from {} [{}]",
            resolved.binary.display(),
            resolved.source
        );
    }

    // Materialized runtime files are copies (symlink-free under wasm) and may
    // lack the executable bit; restore it before exec.
    ensure_executable(&resolved.binary);

    let mut cmd = Command::new(&resolved.binary);
    cmd.args(forwarded);
    exec_or_run(cmd, &resolved.binary)
}

/// Ensure the runtime the active spec selects is present, auto-installing the
/// newest match for a floating spec. Delegates the actual install to the app
/// (which owns `wasi:http`), but decides *whether* to bother using the cached
/// release list so a plain `exec` stays offline and fast within the refresh
/// interval.
fn ensure_active_runtime(layout: &Layout, cwd: &Path) -> Result<()> {
    use wvm_core::{cache, discovery, VersionSpec};

    let Some((spec_str, _src)) = discovery::effective_spec_at(layout, cwd)? else {
        return Ok(());
    };
    let spec = VersionSpec::parse(&spec_str).map_err(|e| anyhow::anyhow!(e))?;
    let installed_best = discovery::resolve_installed(layout, &spec_str);

    // Exact spec: install only if entirely absent.
    if !spec.is_floating() {
        if installed_best.is_some() {
            return Ok(());
        }
        return delegate_ensure(layout, &spec_str);
    }

    // Floating spec. `WVM_REFRESH_INTERVAL=0` means stay offline at activation.
    let ttl = cache::refresh_interval();
    if ttl == 0 {
        return Ok(());
    }

    // With a fresh cache we can tell locally whether a newer match exists and
    // skip launching the app entirely when already up to date.
    if let Some(c) = cache::read(layout, false) {
        if c.is_fresh(now_epoch(), ttl) {
            let remote_best = spec.resolve(&c.versions).map(str::to_string);
            if remote_best.is_none() || remote_best == installed_best {
                return Ok(());
            }
        }
    }
    delegate_ensure(layout, &spec_str)
}

/// Run the app's `ensure <spec>` step (materializing the app + seed first).
fn delegate_ensure(layout: &Layout, spec_str: &str) -> Result<()> {
    materialize_app(layout)?;
    seed::ensure(layout)?;
    let args = ["ensure".to_string(), spec_str.to_string()];
    let status = run_app_and_wait(layout, &args)?;
    if !status.success() && std::env::var_os("WVM_VERBOSE").is_some() {
        eprintln!("wvm: auto-install step exited with {status}");
    }
    Ok(())
}

fn now_epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        std::fs::write(&dest, APP_WASM).with_context(|| format!("writing {}", dest.display()))?;
    }
    Ok(())
}

/// Build the `wasmtime run … app.wasm <args>` command for the app component.
/// `extra_dir`, when set, is preopened in addition to `WVM_HOME`.
fn app_command(layout: &Layout, args: &[String], extra_dir: Option<&Path>) -> Result<Command> {
    let seed_bin = layout.seed_bin();
    let app_wasm = layout.app_wasm();
    let home = layout
        .root
        .to_str()
        .context("WVM_HOME is not valid UTF-8")?;

    let mut cmd = Command::new(&seed_bin);
    cmd.arg("run")
        .arg("-S")
        .arg("http") // enable wasi:http host support
        .arg("--dir")
        .arg(format!("{home}::{home}"))
        .arg("--env")
        .arg(format!("WVM_HOME={home}"))
        .arg("--env")
        .arg(format!("WVM_HOST_ARCH={}", std::env::consts::ARCH))
        .arg("--env")
        .arg(format!("WVM_HOST_OS={}", std::env::consts::OS));
    if let Some(dir) = extra_dir.and_then(|d| d.to_str()) {
        cmd.arg("--dir").arg(format!("{dir}::{dir}"));
    }
    if let Ok(v) = std::env::var("WVM_VERBOSE") {
        cmd.arg("--env").arg(format!("WVM_VERBOSE={v}"));
    }
    if let Ok(v) = std::env::var("WVM_REFRESH_INTERVAL") {
        cmd.arg("--env").arg(format!("WVM_REFRESH_INTERVAL={v}"));
    }
    // Forward the per-session override so the app reflects it in
    // list/current and resolution.
    if let Ok(v) = std::env::var(wvm_core::discovery::SESSION_VAR) {
        cmd.arg("--env")
            .arg(format!("{}={v}", wvm_core::discovery::SESSION_VAR));
    }
    // Everything after the module path is passed to the guest as argv[1..].
    cmd.arg(&app_wasm);
    cmd.args(args);
    Ok(cmd)
}

/// Run the app component on the seed Wasmtime, forwarding `args`, replacing this
/// process (`exec`) on success.
fn launch_app(layout: &Layout, args: &[String], extra_dir: Option<&Path>) -> Result<()> {
    let cmd = app_command(layout, args, extra_dir)?;
    exec_or_run(cmd, &layout.seed_bin())
}

/// Run the app component and wait for it to finish (does not replace this
/// process). The child's stdout is discarded so a following `exec` keeps a
/// pristine stdout; progress and notices (on the child's stderr) pass through.
fn run_app_and_wait(layout: &Layout, args: &[String]) -> Result<std::process::ExitStatus> {
    let mut cmd = app_command(layout, args, None)?;
    cmd.stdout(std::process::Stdio::null());
    cmd.status()
        .with_context(|| format!("running app {:?}", args.first()))
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
