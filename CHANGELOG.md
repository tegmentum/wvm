# Changelog

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
