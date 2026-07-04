# WVM — WebAssembly Version Manager

WVM is a lightweight runtime manager for [Wasmtime](https://wasmtime.dev). It
installs, selects, discovers, validates, and executes versioned WebAssembly
runtimes so that Wasmtime becomes an implementation detail rather than a
prerequisite.

WVM is **self-hosting**: a thin native bootstrapper downloads and locks a
protected seed Wasmtime, then runs the WVM application *as a WebAssembly
component* on that runtime. See [`docs/design.md`](docs/design.md) for the full
design.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/tegmentum/wvm/main/install.sh | sh
```

Or with Homebrew:

```sh
brew tap tegmentum/wvm https://github.com/tegmentum/wvm
brew install wvm
```

On first run, `wvm` downloads and locks a protected seed Wasmtime runtime and
runs as a WebAssembly component on it.

## Quickstart

```sh
wvm list                # all available versions (installed ones marked)
wvm install latest      # download (over wasi:http) + verify + install
wvm default latest      # default runtime for new shells
wvm exec -- --version   # run the selected runtime
```

Long operations show a progress bar / spinner on stderr when attached to a
terminal, and fall back to plain milestone lines when output is piped.

## Default vs. per-shell version

- `wvm default <version>` sets the **persistent default** used by new shells.
- `wvm use <version>` switches the runtime for the **current shell only**
  (reverting when you open a new one), via a `WVM_VERSION` environment variable.

Because `wvm` is a binary it can't change its parent shell directly, so per-shell
`use` needs a one-time shell hook (like nvm/pyenv):

```sh
wvm shell-init >> ~/.zshrc   # then restart your shell
```

After that, `wvm use 44.0.0` applies to the current shell and `wvm deactivate`
reverts it to the default.

## Version specifiers

`install`, `default`, `use`, `path`, and project pins all accept a **spec**
rather than only an exact version. A spec can lock a line and float within it:

| Spec | Locks to | Resolves to |
| --- | --- | --- |
| `latest` | newest overall | e.g. `46.0.1` |
| `lts` | newest LTS line | e.g. `24.0.11` |
| `24` (or `24.x`) | latest major line | newest `24.*` |
| `24.0` (or `24.0.x`) | latest major/minor | newest `24.0.*` |
| `24.0.1` | exact / frozen | exactly `24.0.1` |

`default`/`use` store the **spec**, not the resolved version, so `wvm default 24`
keeps tracking the newest `24.x` as patches land. Setting a floating default (or
`use`) installs the newest match immediately. At activation (`wvm exec`, a new
shell), a floating spec auto-installs a newer matching release if one has
appeared; the remote release list is cached (`WVM_REFRESH_INTERVAL` seconds,
default 3600) so this doesn't hit the network on every call, and
`WVM_REFRESH_INTERVAL=0` keeps activation fully offline. To advance a floating
line on demand (forcing a fresh check), run `wvm upgrade` — `wvm upgrade 24`
for one line, or `wvm upgrade --all` to bump every installed major line.

## Commands

| Command | Description |
| --- | --- |
| `wvm install <spec>` | Install a runtime (spec: `latest`, `lts`, `24`, `24.0`, or `24.0.1`). `--default` to set it as default. |
| `wvm list [--all]` | List all available versions; `lts`/installed/default/seed marked. `--all` includes prereleases. |
| `wvm uninstall <spec>` | Remove an installed runtime (spec resolves to the newest installed match; `--force` past app deps; the seed cannot be removed). |
| `wvm register <app-dir>` | Record an app's runtime dependency from its `wvm.toml` `[app]`. |
| `wvm unregister <name>` | Drop an application's registration. |
| `wvm apps` | List registered applications and the runtimes they depend on. |
| `wvm usage [--limit N]` | Show runtime invocations observed via the pass-through shim. |
| `wvm default <spec>` | Set the persistent default (used by new shells); floats when given `latest`/`lts`/`24`/`24.0`. |
| `wvm use <spec>` | Switch the runtime for the current shell (needs `shell-init`); accepts a floating spec. |
| `wvm upgrade [spec] [--all]` | Pull the newest match for a floating line now (default: the default's line; `--all`: every installed major line). |
| `wvm deactivate` | Clear the per-shell override, reverting to the default. |
| `wvm shell-init` | Print the shell hook that enables per-shell `use`. |
| `wvm current` | Print the effective version (session override, else default). |
| `wvm path [version]` | Print a runtime's filesystem path. |
| `wvm exec -- <args>` | Run the selected runtime, forwarding arguments. |
| `wvm verify [version]` | Validate installation integrity against manifests. |
| `wvm gc [--prune]` | Report (or delete) unreferenced store objects; also hints stale runtimes (unused, not seed/default/app-required). |
| `wvm objects` | List stored objects with sizes and the versions referencing them. |

## Architecture

```
wvm (native bootstrapper, on PATH)
  ├─ ensures the protected seed Wasmtime (downloads once, then locks)
  ├─ handles `wvm exec` natively (resolve runtime, then exec)
  └─ runs the app on the seed:  wasmtime run -S http --dir WVM_HOME wvm-app.wasm -- <args>

wvm-app (wasm32-wasip2 component) — all other commands
  ├─ explicit wasi:cli command: include wasi:cli/imports + export wasi:cli/run@0.2.6
  ├─ imports wasi:http               (downloads, via waki)
  └─ imports sqlite:wasm/high-level  (the index; composed in via `wac`)
```

`wvm-app` is a standard `wasi:cli` command (built as a `cdylib` that owns its
`wasi:cli/run@0.2.6` export), so it also runs directly under any wasi:cli host:

```sh
wasmtime run -S http --dir "$WVM_HOME::$WVM_HOME" --env WVM_HOME="$WVM_HOME" \
  target/wvm-app.composed.wasm -- list
```

The native binary embeds the composed app component; the ~50 MB runtime is
downloaded on bootstrap rather than bundled.

## Runtime discovery

`wvm exec` resolves a runtime in this order:

1. **Project pin** — nearest `wvm.toml` walking up from the working directory
   (the runtime may be a floating spec like `44`):
   ```toml
   [wvm]
   runtime = "44"
   ```
2. **Session** — `WVM_VERSION`, set per shell by `wvm use`.
3. **Default** — the persistent default set by `wvm default`.

Each of these holds a [version spec](#version-specifiers); a floating one
resolves to the newest matching installed release (and auto-installs a newer
match at activation).
4. **Environment override** — `WASM_RUNTIME_HOME` or `WASMTIME_HOME`.
5. **System / PATH** — a `wasmtime` already on `PATH`.

Set `WVM_VERBOSE=1` to print which runtime was selected.

## Application registration

Applications can declare which Wasmtime version(s) they were tested against, so
wvm knows whether a runtime is safe to remove and which apps are behind. An app
owns a small manifest (the `[app]` section of its `wvm.toml`) that it reads
itself — so it works with **no wvm installed** and may bring its own runtime:

```toml
[app]
name = "tegmentum-foo"
runtimes = ["44.0.0", "45.0.0"]        # wvm-managed versions tested against
# runtime-path = "/opt/foo/bin/wasmtime"   # OR a custom runtime the app supplies
```

```sh
wvm register ./my-app     # reads my-app/wvm.toml and records the dependency
wvm apps                  # list registered apps and their runtimes
```

Registration is **advisory bookkeeping** — apps never depend on wvm at runtime.
With it, `wvm uninstall <version>` refuses to remove a runtime a registered app
still needs (listing the dependents; `--force` overrides). An app that sets
`runtime-path` is fully decoupled: it's recorded for visibility but pins no
wvm-managed runtime.

## Transparent usage tracking

Registration is *declared* intent; the shim gives you *observed* usage with
**zero coupling**. `wvm shell-init` puts `shims/` on your `PATH`, where
`shims/wasmtime` is the `wvm` binary under another name. An app that simply
calls `wasmtime` therefore routes through wvm, which:

1. resolves the active version (pin → session → default, floating specs
   included, auto-installing a newer match if needed);
2. records the full run to `usage.log` — the resolved **version** and runtime
   **binary path**, the **module** run with its absolute path and **sha256**,
   the complete **argv** (flags and options), the **app** (`WVM_APP`), the
   **caller**, the **cwd**, and the **time** — one cheap append, no database on
   the hot path;
3. execs the real runtime, forwarding all arguments.

`wvm exec` records the same way. The app needs to know nothing about wvm — the
dependency arrow flips from app → wvm to wvm → (observing) → app. Set
`WVM_APP=<name>` in an app's environment for a clean self-identification;
otherwise the caller is best-effort (the parent process name where available).
`WVM_NO_USAGE=1` opts a process out of recording.

```sh
wvm usage            # per-version counts + recent invocations
wvm usage --limit 50
```

The log is ingested into the `usage` table (SQLite) the next time a wvm command
runs. `wvm list` annotates installed runtimes with when they were last used, and
`wvm gc` hints runtimes unused for a while (default 90 days, `WVM_STALE_DAYS`
overrides) that are safe to consider removing — excluding the seed, the default,
and any app-required version. Observation only covers runtimes reached through
`PATH`; an app that hardcodes an absolute runtime path is invisible here — which
is what registration is for.

## Storage layout

WVM stores everything under `~/.tegmentum/wvm` (override with `WVM_HOME`). Files
are kept once in a content-addressable store and referenced per version, so
multiple versions share identical files:

```text
~/.tegmentum/wvm/
  seed/
    bin/wasmtime                       # protected seed runtime (read-only)
    SEED                               # locked seed version
  store/sha256/<ab>/<cd>/<digest>      # deduplicated file objects
  runtimes/wasmtime/
    versions/44.0.0/
      bin/wasmtime                     # materialized from the store
      manifest.json
    default                            # persistent default spec (plain text, e.g. `24`)
  shims/wasmtime                       # pass-through shim (link to the wvm binary)
  downloads/
  cache/releases.json                  # cached remote release list (refresh-interval bounded)
  usage.log                            # shim invocation log, ingested into index.db
  index.db                             # SQLite backlink/metadata index (rebuildable cache)
  wvm-app.wasm                         # the app component
  config.toml
```

The protected **seed** runtime lives in `seed/`, separate from user-managed
versions; WVM never lists or deletes it. The `index.db` SQLite database tracks
object backlinks and version metadata; it is a derived cache that `wvm gc`
rebuilds from disk, so a missing or stale index is never fatal.

Materialization is `copy` by default (symlinks are unavailable under wasm); the
store still deduplicates shared files.

## Build from source

Requires the Rust `wasm32-wasip2` target and [`wac`](https://github.com/bytecodealliance/wac)
(`cargo install wac-cli`).

```sh
rustup target add wasm32-wasip2
make            # builds the app, composes it with the SQLite component,
                # then builds the native binary (target/release/wvm)
```

The vendored SQLite component (`vendor/sqlite-core.wasm`) provides
`sqlite:wasm/high-level`; it is built from
[`sqlite-wasm`](https://github.com/tegmentum/sqlite-wasm). The WASI WIT for the
app's `wasi:cli` command world is vendored under `crates/wvm-app/wit/deps`
(fetched with `wkg wit fetch`), so a normal build needs no network for WIT.

## Releasing

For each platform, `make` produces `target/release/wvm`; publish it on the
GitHub release as `wvm-<arch>-<os>` (e.g. `wvm-aarch64-macos`) alongside a
matching `wvm-<arch>-<os>.sha256`. Then bump `version` and the per-platform
`sha256` values in [`Formula/wvm.rb`](Formula/wvm.rb). The `install.sh` script
and the Homebrew formula both consume those `wvm-<arch>-<os>` assets.

[Wasmtime cuts an LTS](https://docs.wasmtime.dev/stability-release.html) every
12 releases (major divisible by 12 — 24, 36, 48, …), supported 24 months; wvm
marks these in `wvm list` and resolves `wvm install lts` to the newest one.

## Continuous integration

CI runs on GitHub Actions (`.github/workflows/ci.yml`): format check, the full
`make` build, clippy (`-D warnings`), and tests. Tagging `v*` triggers
`release.yml`, which builds the `wvm-<arch>-<os>` binaries + checksums for each
platform and attaches them to the release.

**Both workflows are gated on repository visibility** (`if: ${{ !github.event.repository.private }}`):
while the repo is **private** the jobs are skipped (don't burn Actions minutes on
work you can't see anyway) — iterate locally with `make act`. Flipping the repo
**public** makes them run automatically, with no edits to the workflows.

Run the same checks locally:

```sh
make ci      # fmt + build + clippy + test, no Docker
make act     # run the CI workflow in Docker via nektos/act (uses .actrc)
```

`make act` runs the workflow regardless of visibility: `act` supplies no
`repository.private` in its event payload, so the visibility gate evaluates
truthy and the job runs. It needs a running Docker daemon (e.g.
[Colima](https://github.com/abiosoft/colima): `colima start`). On Apple Silicon,
`.actrc` pins `linux/amd64` to match GitHub-hosted runners.

## License

Apache-2.0.
