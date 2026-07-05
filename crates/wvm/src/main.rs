//! WVM native bootstrapper.
//!
//! The only native component of WVM. It establishes the protected seed Wasmtime
//! runtime (downloading it once), materializes the embedded app component, and
//! runs the app on the seed runtime — forwarding all arguments. All real WVM
//! logic lives in the app (`wvm-app`), executed as a WebAssembly component.

mod completions;
mod doctor;
mod seed;
mod selfupdate;
mod shim;

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use wvm_core::layout::Layout;

/// The composed app component, embedded at build time (see `build.rs`).
static APP_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.wasm"));

fn main() {
    // Busybox-style dispatch: when invoked under the shim name (`shims/wasmtime`
    // is a link to this binary), act as the pass-through runtime shim rather
    // than the `wvm` CLI.
    let invoked_as = std::env::args()
        .next()
        .and_then(|p| {
            Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .unwrap_or_default();

    let result = if invoked_as == "wasmtime" {
        shim::run()
    } else {
        run()
    };
    if let Err(e) = result {
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

    // Self-upgrade the wvm binary in place. Handled natively (like --version):
    // it replaces this executable, so it must not depend on the seed runtime.
    // Distinct from `wvm upgrade <spec>`, which upgrades managed runtimes.
    if args.first().map(String::as_str) == Some("--upgrade") {
        let check_only = args.iter().any(|a| a == "--check");
        return selfupdate::run(check_only);
    }

    // Emit a shell completion script. Native (no runtime needed) so the
    // installer can generate completions right after placing the binary.
    if args.first().map(String::as_str) == Some("completions") {
        // Hidden helper the generated scripts call for dynamic version lists.
        if args.get(1).map(String::as_str) == Some("--installed") {
            return completions::installed();
        }
        return completions::print(args.get(1).map(String::as_str));
    }

    // Print the shell integration natively (no runtime needed), so the
    // installer can fold it into the sourced env file offline.
    if args.first().map(String::as_str) == Some("shell-init") {
        let layout = Layout::discover()?;
        print!("{}", wvm_core::shell::integration(&layout.shims_dir()));
        return Ok(());
    }

    let layout = Layout::discover()?;
    std::fs::create_dir_all(&layout.root)
        .with_context(|| format!("creating {}", layout.root.display()))?;

    // Keep the pass-through shim pointing at this binary (best-effort; inert
    // until the user adds shims/ to PATH via `wvm shell-init`).
    let _ = ensure_shim(&layout);

    // `exec` is handled natively: a wasm guest cannot spawn a process, and
    // project-pin discovery needs the user's real working directory (which is
    // not preopened into the app sandbox).
    if args.first().map(String::as_str) == Some("exec") {
        return exec_runtime(&layout, &args[1..]);
    }

    // `seed` — inspect or update the protected seed runtime. Native: the seed is
    // what runs the app, so it must be managed without the app.
    if args.first().map(String::as_str) == Some("seed") {
        return match args.get(1).map(String::as_str) {
            Some("upgrade") => seed::upgrade(&layout, args.iter().any(|a| a == "--check")),
            Some("status") | None => seed::status(&layout),
            Some(other) => {
                eprintln!("unknown seed subcommand '{other}' (try: status, upgrade [--check])");
                std::process::exit(2);
            }
        };
    }

    // `doctor` — diagnose install + shell integration. Native: needs the real
    // PATH, rc files, and to run external wasmtime binaries.
    if args.first().map(String::as_str) == Some("doctor") {
        return doctor::run(&layout);
    }

    // Throttled, best-effort check for a newer wvm release. Only on ordinary
    // management commands — never the `exec` hot path above — and never fatal.
    selfupdate::notify(&layout);

    // `register <app-dir>` reads a manifest outside WVM_HOME; preopen that
    // directory for the app and pass it the canonical absolute path.
    let extra_dir = register_dir(&mut args);

    materialize_app(&layout)?;
    let _seed_version = ensure_seed(&layout)?;

    launch_app(&layout, &args, extra_dir.as_deref())
}

/// Ensure the protected seed runtime exists, and on the very first bootstrap
/// adopt its version as the initial persistent default. This makes `wvm exec`
/// work out of the box: the seed Wasmtime is downloaded anyway to run the app,
/// so a fresh install has a usable runtime without a separate `wvm install` +
/// `wvm default`. Only sets the default when none exists, so a user's later
/// choice is never overwritten.
fn ensure_seed(layout: &Layout) -> Result<String> {
    let version = seed::ensure(layout)?;
    if wvm_core::discovery::default_version(layout).is_none() {
        if let Err(e) = wvm_core::discovery::set_default_version(layout, &version) {
            if std::env::var_os("WVM_VERBOSE").is_some() {
                eprintln!("wvm: could not set seed as initial default: {e:#}");
            }
        }
    }
    Ok(version)
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

/// `wvm exec [--no-usage] [--] <args>` — resolve the user's selected runtime and
/// replace this process with it, forwarding the arguments. Leading `--no-usage`
/// opts the invocation out of usage recording; a `--` separates wvm options from
/// the runtime's own arguments.
fn exec_runtime(layout: &Layout, raw: &[String]) -> Result<()> {
    let mut no_usage = false;
    let mut rest = raw;
    loop {
        match rest.first().map(String::as_str) {
            Some("--no-usage") => {
                no_usage = true;
                rest = &rest[1..];
            }
            Some("--") => {
                rest = &rest[1..];
                break;
            }
            _ => break,
        }
    }
    let forwarded = rest;

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

    // Record the invocation, same as the shim — `wvm exec` is just as much a
    // real runtime use as a call routed through `shims/wasmtime`.
    if !no_usage {
        shim::record_invocation(layout, &resolved, Some(&cwd), forwarded);
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
        // The protected seed runtime may already provide this exact version
        // (the initial default adopted at first bootstrap points at it), so
        // don't download a redundant managed copy — `resolve` will run the seed.
        if let Some((seed_ver, _)) = discovery::seed_runtime(layout) {
            if spec.resolve(std::slice::from_ref(&seed_ver)) == Some(seed_ver.as_str()) {
                return Ok(());
            }
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
    ensure_seed(layout)?;
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

/// Ensure `shims/wasmtime` points at the current `wvm` binary, so a `wasmtime`
/// call routed through `PATH` re-enters this binary in shim mode.
#[cfg(unix)]
fn ensure_shim(layout: &Layout) -> Result<()> {
    let exe = std::env::current_exe().context("locating the wvm binary")?;
    let shim = layout.shim_bin("wasmtime");
    if let Some(parent) = shim.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Re-point only when missing or stale.
    match std::fs::read_link(&shim) {
        Ok(target) if target == exe => return Ok(()),
        _ => {
            let _ = std::fs::remove_file(&shim);
        }
    }
    std::os::unix::fs::symlink(&exe, &shim)
        .with_context(|| format!("linking shim {}", shim.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_shim(_layout: &Layout) -> Result<()> {
    Ok(())
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
        bail!("no app component embedded; build with `make` (which builds and embeds wvm-app)");
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
    // The user's login shell, so `wvm use`/`shell-init` can name the right rc
    // file (e.g. ~/.bashrc vs ~/.zshrc) in their guidance.
    if let Ok(v) = std::env::var("SHELL") {
        cmd.arg("--env").arg(format!("SHELL={v}"));
    }
    if let Ok(v) = std::env::var("WVM_REFRESH_INTERVAL") {
        cmd.arg("--env").arg(format!("WVM_REFRESH_INTERVAL={v}"));
    }
    if let Ok(v) = std::env::var("WVM_STALE_DAYS") {
        cmd.arg("--env").arg(format!("WVM_STALE_DAYS={v}"));
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
