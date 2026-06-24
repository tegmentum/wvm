//! WVM application component — an explicit `wasi:cli` command.
//!
//! Built as a `cdylib` for `wasm32-wasip2` that owns the `wasi:cli/run` export
//! (consistent at 0.2.6) rather than relying on the Rust std command adapter.
//! Launched inside the protected seed Wasmtime by the native `wvm`
//! bootstrapper. Filesystem/CAS logic comes from `wvm-core`; the index is the
//! `sqlite:wasm/high-level` component.

mod commands;
mod http_wasi;
mod index_component;
mod install;
mod progress;

wit_bindgen::generate!({
    world: "app",
    path: "wit",
    generate_all,
});

/// Re-export of the SQLite component's high-level interface for submodules.
pub(crate) use sqlite::wasm::high_level as sql;

struct Component;

impl exports::wasi::cli::run::Guest for Component {
    /// `wasi:cli/run` entry point. `Ok` exits 0, `Err` exits 1; commands that
    /// need another exit code call `std::process::exit` (via `wasi:cli/exit`).
    fn run() -> Result<(), ()> {
        let args: Vec<String> = std::env::args().collect();
        let cmd = args.get(1).map(String::as_str).unwrap_or("help");
        // First non-flag argument after the subcommand.
        let positional = args.iter().skip(2).find(|a| !a.starts_with('-')).map(String::as_str);
        let flag = |name: &str| args.iter().skip(2).any(|a| a == name);

        let result = match cmd {
            "install" => match positional {
                Some(v) => install::install(v, flag("--default") || flag("--use")),
                None => missing_arg("install <version>"),
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
            "deactivate" => commands::deactivate(),
            "shell-init" => commands::shell_init(),
            "uninstall" => match positional {
                Some(v) => commands::uninstall(v),
                None => missing_arg("uninstall <version>"),
            },
            "verify" => commands::verify(positional),
            "gc" => commands::gc(flag("--prune")),
            "objects" => commands::objects(),
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

fn print_help() {
    println!("wvm — WebAssembly Version Manager");
    println!();
    println!("Commands:");
    println!("  install <version>    Install a runtime (`latest` for newest; --default to set default)");
    println!("  list [--all]         List all available versions (installed ones marked)");
    println!("  current              Show the effective runtime version (session or default)");
    println!("  path [version]       Print a runtime's filesystem path");
    println!("  default <version>    Set the persistent default (used by new shells)");
    println!("  use <version>        Switch the runtime for the current shell (needs shell-init)");
    println!("  deactivate           Clear the per-shell override (revert to default)");
    println!("  shell-init           Print the shell hook enabling per-shell `use`");
    println!("  uninstall <version>  Remove an installed runtime");
    println!("  verify [version]     Validate installation integrity");
    println!("  gc [--prune]         Report/reclaim unreferenced store objects");
    println!("  objects              List stored objects and their backlinks");
}
