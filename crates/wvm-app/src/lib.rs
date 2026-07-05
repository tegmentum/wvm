//! WVM application component — an explicit `wasi:cli` command.
//!
//! Built as a `cdylib` for `wasm32-wasip2` that owns the `wasi:cli/run` export
//! (consistent at 0.2.6) rather than relying on the Rust std command adapter.
//! Launched inside the protected seed Wasmtime by the native `wvm`
//! bootstrapper. All logic comes from `wvm-core`; persistence is plain files.

mod commands;
mod http_wasi;
mod install;
mod progress;

wit_bindgen::generate!({
    world: "app",
    path: "wit",
    generate_all,
});

struct Component;

impl exports::wasi::cli::run::Guest for Component {
    /// `wasi:cli/run` entry point. `Ok` exits 0, `Err` exits 1; commands that
    /// need another exit code call `std::process::exit` (via `wasi:cli/exit`).
    fn run() -> Result<(), ()> {
        let args: Vec<String> = std::env::args().collect();
        let cmd = args.get(1).map(String::as_str).unwrap_or("help");
        // First non-flag argument after the subcommand.
        let positional = args
            .iter()
            .skip(2)
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        let flag = |name: &str| args.iter().skip(2).any(|a| a == name);

        let result = match cmd {
            "install" => match positional {
                Some(v) => install::install(v, flag("--default") || flag("--use")),
                None => missing_arg("install <version>"),
            },
            // Internal: resolve a spec and auto-install the newest match if
            // missing. Invoked by the bootstrapper before `exec` for floating
            // specs. Prints nothing to stdout on success.
            "ensure" => match positional {
                Some(v) => install::ensure(v).map(|_| ()),
                None => missing_arg("ensure <version>"),
            },
            "list" => commands::list(flag("--all")),
            "current" => commands::current(),
            "path" => commands::path(positional),
            "default" => match positional {
                Some(v) => commands::set_default(v),
                None => missing_arg("default <version>"),
            },
            "use" => match positional {
                Some(v) => commands::use_version(v),
                None => missing_arg("use <version>"),
            },
            "upgrade" => commands::upgrade(positional, flag("--all")),
            "deactivate" => commands::deactivate(),
            "shell-init" => commands::shell_init(),
            "register" => match positional {
                Some(dir) => commands::register(dir),
                None => missing_arg("register <app-dir>"),
            },
            "unregister" => match positional {
                Some(name) => commands::unregister(name),
                None => missing_arg("unregister <name>"),
            },
            "apps" => commands::apps(),
            "usage" => {
                let limit = flag_value(&args, "--limit").unwrap_or(20);
                commands::usage(limit)
            }
            "uninstall" => match positional {
                Some(v) => commands::uninstall(v, flag("--force")),
                None => missing_arg("uninstall <version>"),
            },
            "verify" => commands::verify(positional),
            "help" | "--help" | "-h" => {
                print_help();
                Ok(())
            }
            other => {
                eprintln!("error: unknown command `{other}`");
                print_help();
                std::process::exit(2);
            }
        };

        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("error: {e:#}");
                Err(())
            }
        }
    }
}

export!(Component);

fn missing_arg(usage: &str) -> anyhow::Result<()> {
    anyhow::bail!("usage: wvm {usage}")
}

/// Parse an integer option in either `--name N` or `--name=N` form.
fn flag_value(args: &[String], name: &str) -> Option<i64> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().and_then(|v| v.parse().ok());
        }
        if let Some(rest) = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return rest.parse().ok();
        }
    }
    None
}

fn print_help() {
    println!("wvm — Wasmtime Version Manager");
    println!();
    println!("Commands:");
    println!("  install <spec>       Install a runtime (spec: latest, lts, 24, 24.0, or 24.0.1)");
    println!("  list [--all]         List all available versions (installed ones marked)");
    println!("  current              Show the effective runtime version (session or default)");
    println!("  path [spec]          Print a runtime's filesystem path");
    println!("  default <spec>       Set the persistent default (floats: latest/lts/24/24.0)");
    println!("  use <spec>           Switch the runtime for the current shell (needs shell-init)");
    println!("  upgrade [spec] [--all]  Pull the newest match for a floating line now");
    println!("  deactivate           Clear the per-shell override (revert to default)");
    println!("  shell-init           Print the shell hook enabling per-shell `use`");
    println!("  uninstall <version>  Remove an installed runtime (--force past app deps)");
    println!("  register <app-dir>   Record an app's runtime dependency (reads its wvm.toml)");
    println!("  unregister <name>    Drop an application's registration");
    println!("  apps                 List registered applications and their runtimes");
    println!("  usage [--limit N]    Show runtime invocations observed via the shim");
    println!("  verify [version]     Validate installation integrity");
    println!("  completions <shell>  Print a completion script (bash, zsh, fish)");
    println!();
    println!("Self-management:");
    println!("  --version, -V        Print the wvm version");
    println!(
        "  --upgrade [--check]  Update wvm itself to the latest release (--check only reports)"
    );
}
