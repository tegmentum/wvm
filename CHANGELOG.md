# Changelog

## 0.3.0

Zero-setup installs and self-management. A fresh install has a working runtime
and a working `wvm use` immediately, and wvm can update itself.

### Added

- **Runtime out of the box** — the protected seed Wasmtime is adopted as the
  initial default and discovery falls back to it, so `wvm exec` works before any
  `wvm install`/`wvm default`. Managed installs still take precedence.
- **`wvm --upgrade [--check]`** — self-update the native binary in place
  (distinct from `wvm upgrade <spec>`, which manages runtimes). A throttled
  notice points at it when a newer release is available; `WVM_NO_UPDATE_NOTIFIER`
  opts out.
- **Shell completions** — `wvm completions <bash|zsh|fish>`, installed
  automatically by the installer. `use`/`default`/`upgrade` complete installed
  versions (plus `latest`/`lts`); `uninstall` completes only installed ones.
- **Automatic app registration** — an app with an `[app]` section in its
  `wvm.toml` auto-registers when it runs through the shim or `wvm exec`, so
  `uninstall` dependency-gating and `wvm apps` work without a manual
  `wvm register`.

### Changed

- **Installer** now sets up `PATH` for your shell, installs completions, and
  folds the shim + `wvm use` hook into the sourced env file — `wvm use` works
  with no separate `wvm shell-init` step. Re-runs are idempotent and report
  fresh/reinstall/upgrade. Fixes a `set -e` abort in `wire_rc` that skipped
  completion and hook wiring on any re-run.
- `wvm shell-init` is now handled natively (no runtime bootstrap), and its hook
  lives in one place (`wvm-core::shell`).
- `wvm use` and `wvm shell-init` name the rc file for your actual shell
  (`~/.bashrc`, `~/.zshrc`, …) instead of assuming zsh.
- `wvm usage` prints an aligned table and notes that it aggregates every shell
  and `wvm exec`, not just the current one.
- `wvm list` separates version tags with a tab so they line up.
- The "Fetching available versions" spinner now animates (streamed response)
  instead of showing a single static frame.

## 0.2.0

The first release to publish platform binaries. Builds on the self-hosting
foundation (`v0.1.0`) with version-spec selection and transparent usage
tracking.

### Added

- **Floating version specs** — `latest`, `lts`, `24` (latest major), `24.0`
  (latest major/minor), or an exact `24.0.1`. `default`, `use`, and project
  pins store the *spec*, so a floating selection tracks its line and
  auto-installs a newer matching release at activation. The remote release list
  is cached (`WVM_REFRESH_INTERVAL`, `0` stays offline).
- **Pass-through shim + transparent usage tracking** — `wvm shell-init` puts
  `shims/wasmtime` on `PATH`; an app that calls `wasmtime` routes through wvm,
  which resolves the active version, records the full run, and execs the real
  runtime. Each record captures the resolved version and runtime binary path,
  the module (as given, its absolute path, and its `sha256`), the complete
  argv, and the app/caller/cwd/time. `wvm exec` records the same way.
- **`wvm upgrade [spec] [--all]`** — pull the newest match for a floating line
  on demand (forcing a fresh release check).
- **`wvm usage [--limit N]`** — observed invocations. `wvm list` annotates
  installed runtimes with last-used, and `wvm gc` hints stale runtimes
  (`WVM_STALE_DAYS`, default 90) that are safe to remove.
- **Opt-outs** — `--no-usage` (leading flag) or `WVM_NO_USAGE=1` skip recording;
  a large-module hashing warning (`WVM_HASH_WARN_MB`, default 100 MiB) points at
  them when interactive.
- Spec-aware `wvm uninstall` (`uninstall 24` → newest installed `24.x`).
- macOS caller detection for usage attribution (Linux already used `/proc`).

### Changed

- `wvm install <spec> --default` stores the spec, matching `wvm default`.
- GitHub CI and Release workflows are gated on public repository visibility
  (skipped while private; run automatically when public). `make act` resolves
  the active Docker context's socket so it works on Colima.

## 0.1.0

Initial self-hosting foundation: the native bootstrapper runs the wvm
application as a WebAssembly component on a protected, download-and-locked seed
Wasmtime; content-addressable store with a SQLite index; install/list/current/
use/default/exec, project pinning, and application registration.
