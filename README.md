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

## Commands

| Command | Description |
| --- | --- |
| `wvm install <version>` | Install a runtime (`latest` for the newest). `--default` to set it as default. |
| `wvm list [--all]` | List all available versions (most recent first); installed/default/seed marked. `--all` includes prereleases. |
| `wvm uninstall <version>` | Remove an installed runtime (`--force` past app deps; the seed cannot be removed). |
| `wvm register <app-dir>` | Record an app's runtime dependency from its `wvm.toml` `[app]`. |
| `wvm unregister <name>` | Drop an application's registration. |
| `wvm apps` | List registered applications and the runtimes they depend on. |
| `wvm default <version>` | Set the persistent default (used by new shells). |
| `wvm use <version>` | Switch the runtime for the current shell (needs `shell-init`). |
| `wvm deactivate` | Clear the per-shell override, reverting to the default. |
| `wvm shell-init` | Print the shell hook that enables per-shell `use`. |
| `wvm current` | Print the effective version (session override, else default). |
| `wvm path [version]` | Print a runtime's filesystem path. |
| `wvm exec -- <args>` | Run the selected runtime, forwarding arguments. |
| `wvm verify [version]` | Validate installation integrity against manifests. |
| `wvm gc [--prune]` | Report (or delete) unreferenced store objects. |
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

1. **Project pin** — nearest `wvm.toml` walking up from the working directory:
   ```toml
   [wvm]
   runtime = "44.0.0"
   ```
2. **Session** — `WVM_VERSION`, set per shell by `wvm use`.
3. **Default** — the persistent default set by `wvm default`.
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
    default                            # persistent default version (plain text)
  downloads/
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

## License

Apache-2.0.
