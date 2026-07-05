//! Shared shell-integration snippet.
//!
//! A child process cannot mutate its parent shell's environment, so `wvm use`
//! and `wvm deactivate` work by printing shell code for a wrapper function to
//! `eval`. That wrapper (plus putting the pass-through shim ahead of `PATH`) is
//! the "shell integration". It is emitted both by `wvm shell-init` (for manual
//! setup) and by the installer (folded into the sourced env file), so it lives
//! here as the single source of truth.

use std::path::Path;

/// The `wvm` wrapper function. For `use`/`deactivate` it eval's the command's
/// stdout (so the version override lands in the live shell); everything else is
/// forwarded untouched. Uses `local`, so it targets bash/zsh (and the many
/// `sh` implementations that support it).
pub const HOOK: &str = r#"wvm() {
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

/// The POSIX shell integration for a given shims directory: prepend the shim
/// dir to `PATH` (so apps that call `wasmtime` route through wvm) and define the
/// `use` hook. Suitable for sourcing from an rc file or the installer's env
/// file.
pub fn integration(shims_dir: &Path) -> String {
    format!(
        "# wvm shell integration: route `wasmtime` through wvm and enable `wvm use`.\n\
         export PATH=\"{}:$PATH\"\n{HOOK}",
        shims_dir.display()
    )
}
