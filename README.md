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
wvm install latest      # download (over wasi:http) + verify + install
wvm use latest          # make it the active runtime
wvm exec -- --version   # run the selected runtime
```

## Commands

| Command | Description |
| --- | --- |
| `wvm install <version>` | Install a runtime (`latest` for the newest). `--use` to activate it. |
| `wvm uninstall <version>` | Remove an installed runtime (the seed cannot be removed). |
| `wvm list` | List installed runtimes (`*` marks the active one; the seed is shown separately). |
| `wvm use <version>` | Select the active runtime. |
| `wvm current` | Print the active version. |
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
2. **Active runtime** — the version selected by `wvm use`.
3. **Environment override** — `WASM_RUNTIME_HOME` or `WASMTIME_HOME`.
4. **System / PATH** — a `wasmtime` already on `PATH`.

Set `WVM_VERBOSE=1` to print which runtime was selected.

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
    active                             # active version (plain text)
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
