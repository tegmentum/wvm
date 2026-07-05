---
name: wvm
description: >-
  How to use wvm, the Wasmtime Version Manager, to install, pin, switch, run,
  and manage versioned Wasmtime runtimes. Use when the user wants to install or
  manage Wasmtime / WebAssembly runtime versions, pin a project to a runtime,
  run a .wasm module through a managed runtime, set up the wvm shim or shell
  integration, inspect runtime usage, or update wvm itself.
---

# Using wvm

`wvm` manages [Wasmtime](https://wasmtime.dev) runtimes the way nvm/rustup manage
their toolchains: install multiple versions, select one per project or per
shell, and run modules against the selected one. It is self-hosting — a small
native binary runs the wvm app as a WebAssembly component on a protected,
auto-downloaded "seed" runtime.

## Install wvm

```sh
curl -fsSL https://raw.githubusercontent.com/tegmentum/wvm/main/install.sh | sh
# or:  brew tap tegmentum/wvm https://github.com/tegmentum/wvm && brew install wvm
```

The installer sets up `PATH`, installs shell completions, and wires the shim +
`wvm use` hook into a sourced env file — so `wasmtime`, `wvm use`, and
completions work in new shells with no extra steps. On first run wvm downloads
and locks the seed runtime.

**Zero-setup:** the seed is adopted as the initial default, so `wvm exec -- --version`
works immediately after install, before any `wvm install`.

## Version specs (the core concept)

Every version argument accepts a **spec** — a floating channel or an exact pin.
`default`/`use`/pins store the *spec*, so a floating one keeps tracking its line
and auto-installs a newer match at activation.

| Spec | Means | Resolves to |
| --- | --- | --- |
| `latest` | newest overall | e.g. `46.0.1` |
| `lts` | newest LTS line | e.g. `24.0.11` |
| `24` (or `24.x`) | latest major line | newest `24.*` |
| `24.0` (or `24.0.x`) | latest major/minor | newest `24.0.*` |
| `24.0.1` | exact / frozen | exactly `24.0.1` |

## Common tasks

```sh
wvm list                     # all available versions; installed/default/seed/lts marked
wvm install 24               # install newest 24.x (spec-aware)
wvm default 24               # persistent default for new shells (floats within 24.x)
wvm use 24.0                 # switch THIS shell only (needs the shell hook, set up by installer)
wvm deactivate               # drop the per-shell override, back to default
wvm current                  # print the effective version (resolves the spec)
wvm path 24                  # filesystem path of a runtime
wvm upgrade                  # pull the newest match for the default's floating line NOW
wvm upgrade --all            # bump every installed major line to its newest patch
wvm uninstall 24             # remove (spec resolves to newest installed 24.x); --force past app deps
wvm verify                   # check installed runtimes against their manifests
wvm gc --prune               # reclaim unreferenced store objects; also hints stale runtimes
```

**Selection order** (pin → session → default): a project pin wins, then
`WVM_VERSION` (set by `wvm use`), then the default. Add `WVM_VERBOSE=1` to see
which runtime was chosen and why.

Pin a project by creating `wvm.toml` (searched upward from the cwd):

```toml
[wvm]
runtime = "24"   # a spec; floats within 24.x
```

## Running a module

Two equivalent ways; both honor the selection order and record usage.

```sh
# 1. Transparent — after install, `wasmtime` on PATH IS the wvm shim:
wasmtime run module.wasm

# 2. Explicit:
wvm exec -- run module.wasm
```

The shim resolves the active version, records the run, and execs the real
runtime. An app just calls `wasmtime` and needs to know nothing about wvm.

## Usage tracking

Every run through the shim or `wvm exec` is recorded (version, runtime path,
module + absolute path + sha256, full argv, app, caller, cwd, time).

```sh
wvm usage                    # per-version counts + recent invocations
wvm usage --limit 50
```

Opt a run out with a leading `--no-usage` flag or `WVM_NO_USAGE=1`. Set
`WVM_APP=<name>` in an app's environment for clean attribution. A large-module
hashing warning (interactive only) points at the opt-outs; `WVM_HASH_WARN_MB`
tunes the threshold.

## App integration (loose coupling)

An application declares the runtimes it was tested against in its own `wvm.toml`;
it works with **no wvm installed** and never depends on wvm at runtime.

```toml
[app]
name = "my-app"
runtimes = ["24.0.0", "25.0.0"]     # wvm-managed versions
# runtime-path = "/opt/my-app/bin/wasmtime"   # OR a custom runtime it ships
```

Running such an app through the shim / `wvm exec` **auto-registers** it, so
`wvm apps` lists it and `wvm uninstall` refuses to remove a runtime an app still
needs (`--force` overrides). `wvm register <dir>` / `wvm unregister <name>` do it
manually.

## Managing wvm itself

- `wvm --version` — print the wvm version.
- `wvm --upgrade [--check]` — **self-update the wvm binary** (`--check` only
  reports). Distinct from `wvm upgrade <spec>`, which updates managed *runtimes*.
- `wvm completions <bash|zsh|fish>` — print a completion script (the installer
  does this automatically).
- `wvm shell-init` — print the shell integration (PATH + `use` hook) if you need
  to wire it up manually.

## Gotchas

- **`wvm --upgrade` (binary) vs `wvm upgrade <spec>` (runtimes)** — the dash
  matters.
- The shim only sees runtimes reached through `PATH`. An app that hardcodes an
  absolute runtime path is invisible to usage tracking — that is what app
  registration is for.
- `wvm use` can't mutate the parent shell directly; it relies on the hook the
  installer set up (or `wvm shell-init`). Without it, `use` prints guidance.
- Floating auto-install at activation uses a cached release list
  (`WVM_REFRESH_INTERVAL` seconds, default 3600; `0` stays offline).
- The seed runtime is protected: it cannot be uninstalled and `gc` never prunes
  it.
