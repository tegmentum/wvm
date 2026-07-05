# WVM - Wasmtime Version Manager

## Status

Draft

## Overview

WVM (Wasmtime Version Manager) is a lightweight manager for
[Wasmtime](https://wasmtime.dev) runtimes.

The initial focus is management of Wasmtime installations, version selection,
runtime discovery, and project-level version pinning.

WVM exists because Wasmtime is a critical dependency for a growing number of
Tegmentum projects and currently lacks a standardized runtime management
experience comparable to:

* Java + SDKMAN
* Node.js + NVM
* Go + GVM
* Rust + Rustup

WVM provides a consistent mechanism for installing, discovering, validating,
selecting, and executing WebAssembly runtimes without requiring users to
manually install or configure them.

---

## Goals

### Primary Goals

* Install Wasmtime runtimes.
* Manage multiple Wasmtime versions.
* Select active runtime versions.
* Discover runtime installations reliably.
* Support project-level runtime pinning.
* Provide a stable interface for Tegmentum tooling.

### Secondary Goals

* Support additional runtimes in the future.
* Support offline installations.
* Support reproducible toolchains.
* Support integration with Tegmentum tooling.

### Non-Goals (V1)

* Dependency resolution.
* Package management.
* Shared artifact repositories.
* Global content-addressable storage.
* Runtime source compilation.
* Cloud synchronization.
* OCI registry support.

---

## Design Principles

### Runtime Ownership

WVM owns the runtimes it installs.

Users should not be required to install Wasmtime independently.

### Simplicity First

Storage is inexpensive.

Operational complexity is expensive.

WVM intentionally duplicates files rather than introducing complex
deduplication systems.

### Local First

All runtime metadata is stored locally.

No online service is required after installation.

### Explicit Versioning

Every runtime installation is versioned.

Users always know which runtime is active.

### Reproducibility

Projects may pin specific runtime versions.

Builds should not depend on whatever runtime happens to be present on a machine.

---

## Filesystem Layout

### Runtime Root

`~/.tegmentum/wvm/`

### Structure

```text
~/.tegmentum/wvm/
  versions/
    38.0.3/
    39.0.0/
    40.0.0/
  current/
    wasmtime -> ../versions/39.0.0
  manifests/
  downloads/
  config.toml
```

### XDG Compatibility

Optional symlinks may be created:

`~/.local/bin/wasmtime`

pointing to:

`~/.tegmentum/wvm/current/wasmtime/bin/wasmtime`

WVM storage remains under:

`~/.tegmentum`

to preserve ownership and support non-XDG systems.

---

## Runtime Discovery

Discovery order:

1. Project pin
2. WVM active runtime
3. Explicit environment variable
4. System runtime
5. PATH lookup

### Environment Variable

`WASM_RUNTIME_HOME`

or

`WASMTIME_HOME`

may override discovery.

---

## Commands

### Install Runtime

```
wvm install 39.0.0
```

Downloads and installs a runtime.

### Remove Runtime

```
wvm uninstall 39.0.0
```

Removes a runtime installation.

### List Installed Versions

```
wvm list
```

Example:

```
Installed Runtimes
  38.0.3
* 39.0.0
  40.0.0
```

### Select Active Runtime

```
wvm use 39.0.0
```

Updates the active runtime reference.

### Show Active Runtime

```
wvm current
```

Output:

```
39.0.0
```

### Locate Runtime

```
wvm path
```

Output:

```
~/.tegmentum/wvm/versions/39.0.0
```

### Execute Runtime

```
wvm exec -- run hello.wasm
```

Equivalent to:

```
wasmtime run hello.wasm
```

using the selected runtime.

### Verify Runtime

```
wvm verify
```

Validates installation integrity.

---

## Project Pinning

Projects may declare runtime requirements.

Example:

```toml
[wvm]
runtime = "39.0.0"
```

When inside a project:

```
wvm exec
```

automatically selects the pinned version.

Discovery order:

```
project pin
↓
active runtime
↓
system runtime
```

---

## Installation Process

### Runtime Install

```
Download Release
    ↓
Verify Checksum
    ↓
Extract Archive
    ↓
Write Manifest
    ↓
Register Version
```

No content-addressable storage is used in V1.

No deduplication is performed in V1.

---

## Storage (plain files)

Each runtime version is extracted **directly** into its own directory. There is
no shared object store and no database — the filesystem is the source of truth.

```text
~/.tegmentum/wvm/
  seed/{bin/wasmtime, SEED}            # protected seed runtime
  runtimes/wasmtime/
    versions/<version>/               # extracted files: bin/wasmtime, wasmtime-min,
      manifest.json                   #   LICENSE, README.md + a manifest of digests
    default                           # persistent default spec (plain text)
  shims/wasmtime                      # pass-through shim (link to the wvm binary)
  apps.json                           # application registrations
  usage.log                           # shim invocation log (JSON Lines, compacted on read)
  cache/releases.json                 # cached remote release list
  downloads/                          # transient archive downloads
  wvm-app.wasm                        # the embedded app component (written on bootstrap)
```

### Why no content-addressable store

An earlier design used a `store/sha256/...` CAS with per-version backlinks and a
SQLite index to deduplicate files across versions. Measurement killed it: across
real installs the store deduplicated **~0.02%** — only `LICENSE` (and
occasionally `README`). Every wasmtime version is a distinct build, so its
~35-52 MB binary (99.95% of the bytes) is unique by construction. The CAS, the
backlink index, copy-materialization, and object GC existed to save a few
kilobytes, so they were removed. `install`/`uninstall` are now just "extract a
directory" / "remove a directory".

### Manifest and verification

Each version writes a `manifest.json` listing every file with its `sha256`, mode,
and size. `wvm verify` re-hashes the on-disk files and compares, catching
corruption or partial installs. This is the only integrity metadata kept.

### Registrations and usage

- `apps.json` — application registrations (`{ "apps": [ ... ] }`), upserted by
  name; read and rewritten whole (it is small).
- `usage.log` — one JSON object per runtime invocation, appended by the shim. It
  is the usage store itself (no ingest step); reads aggregate it in memory and
  compact it to the most recent entries once it grows past a cap.

Both are advisory bookkeeping: losing either is never fatal.

---

## Bootstrap Process

WVM is self-hosting: the WVM application runs as a WebAssembly component on a
Wasmtime runtime that WVM itself manages.

Bootstrap sequence:

```
Native Bootstrap (wvm)
    ↓
Seed Wasmtime (downloaded once, then locked)
    ↓
Materialize App Component (wvm-app.wasm)
    ↓
Launch App on Seed Runtime
    ↓
All Operations Performed In WebAssembly
```

### Components

- **`wvm` (native bootstrapper)** — the only thing on `PATH`. It downloads and
  locks the protected seed Wasmtime on first use, writes the embedded app
  component into `WVM_HOME`, and runs the app on the seed runtime:
  `wasmtime run -S http --dir WVM_HOME wvm-app.wasm -- <args>`. It handles
  `wvm exec` natively (a wasm guest cannot spawn a process, and project-pin
  discovery needs the user's working directory, which is not in the app
  sandbox).
- **`wvm-app` (WebAssembly component)** — implements every other command. It is
  an explicit **`wasi:cli` command** component (`wasm32-wasip2`, built as a
  `cdylib`): it formally `include`s `wasi:cli/imports@0.2.6` and owns the
  `wasi:cli/run@0.2.6` export via `wit-bindgen` (rather than relying on the Rust
  std command shape, which pins `run@0.2.0`). It additionally imports only
  `wasi:http` (downloads, via `waki`) — both host-satisfied, so there is no
  component-composition step. Because it is a standard wasi:cli command, it also
  runs directly under any wasi:cli host (e.g. `wasmtime run … wvm_app.wasm --
  list`). The WASI WIT is vendored under `crates/wvm-app/wit/deps` (fetched with
  `wkg`).

### Protected seed runtime

The seed is the Wasmtime that runs the app. It lives in `WVM_HOME/seed/`,
separate from user-managed versions, is downloaded once and recorded in a
`SEED` marker, and is set read-only. WVM's own commands never list or delete
it: `wvm uninstall <seed>` is refused, and the seed directory sits outside
`runtimes/wasmtime/versions/`. Users still install and select their own runtimes
independently.

### Version selection: default vs. session

Two layers, nvm-style:

- **default** — persistent (`runtimes/wasmtime/default`), used by new shells;
  set with `wvm default <spec>`.
- **session** — the `WVM_VERSION` environment variable, set per shell by
  `wvm use <spec>`, overriding the default for the current session only.

Resolution order (`wvm exec` and `wvm current`): project pin (`wvm.toml`) →
session (`WVM_VERSION`) → default → `WASMTIME_HOME` → `PATH`.

#### Version specifiers

Each selection layer stores a **spec**, not a frozen version, parsed by
`VersionSpec` in `wvm-core`:

| Spec | Meaning |
| --- | --- |
| `latest` | newest available |
| `lts` | newest LTS line (major divisible by 12) |
| `24` / `24.x` | latest `24.*` (float minor + patch) |
| `24.0` / `24.0.x` | latest `24.0.*` (float patch) |
| `24.0.1` | exact / frozen |

A spec resolves against a candidate set (installed, or the remote release list)
by picking the newest match. Storing the spec means `wvm default 24` keeps
tracking the newest installed `24.x` automatically.

**Offline vs. auto-install.** `discovery::resolve` (used by the native `exec`
path) is offline: a floating spec resolves against the *installed* set only, so
a plain `exec` never blocks on the network. Auto-install is layered on top:

- Setting a floating `default`/`use` installs the newest match immediately (the
  app has `wasi:http`), then stores the spec.
- At activation, the bootstrapper consults the cached release list
  (`cache/releases.json`, TTL from `WVM_REFRESH_INTERVAL`, default 3600s; `0`
  stays fully offline). If a newer matching release exists it delegates to the
  app's internal `ensure <spec>` command to install it before `exec`, discarding
  that step's stdout so the runtime's own stdout stays clean. A fresh cache with
  no newer match skips launching the app entirely.

Because `wvm` is a binary it cannot mutate its parent shell, so per-shell `use`
relies on a shell hook (`wvm shell-init`): when `wvm use` runs with stdout
captured by the hook it prints `export WVM_VERSION=<v>` for the shell to `eval`;
run directly in a terminal it instead explains how to enable the hook. The
`wvm list` command shows all available versions (from the GitHub releases),
marking installed/default/seed — there is no separate remote-listing command.

### Why download-on-bootstrap

The first runtime is fetched by native code because running the app requires a
runtime, and the seed *is* that runtime. Once the seed exists, the app performs
all further work — including installing additional runtimes — over `wasi:http`.

---

## Application Registration

WVM is the foundation of a Tegmentum install, so it tracks which applications
depend on which runtimes. This makes it possible to know whether a runtime is
safe to remove and which applications are behind and need to move forward.

### Loose coupling (non-negotiable)

Applications must **not** depend on wvm at runtime, and may supply their own
custom runtime. Therefore:

- The **manifest is canonical and app-owned**: an app declares its runtimes in
  the `[app]` section of its `wvm.toml`, which the app reads itself. It runs
  with no wvm present.
- **Registration is advisory bookkeeping**: `wvm register <app-dir>` reads that
  manifest and caches it in wvm's index. Registration is optional and only
  informs wvm's lifecycle decisions; it never becomes a runtime dependency.

```toml
[app]
name = "tegmentum-foo"
runtimes = ["44.0.0", "45.0.0"]            # wvm-managed versions tested against
# runtime-path = "/opt/foo/bin/wasmtime"   # OR a custom runtime the app supplies
```

### Registry (cache)

Registrations are cached in `apps.json` as `{ "apps": [ … ] }`, each entry:

```text
{ name, path, runtime_path?, runtimes: [version, …], registered_at }
```

An app that sets `runtime-path` is fully decoupled — it is recorded for
visibility but lists no wvm-managed `runtimes`.

### Lifecycle

- `wvm uninstall <version>` refuses when a registered app depends on it (listing
  the dependents); `--force` overrides. `gc` is inherently safe: an installed
  runtime's objects are always referenced, so it only ever reclaims objects of
  versions already uninstalled.
- `wvm apps` lists registered applications and their runtimes (annotating any
  not currently installed). It does not auto-flag migrations — the operator
  judges what to move forward.

Because the bootstrapper sandboxes the app to `WVM_HOME`, `wvm register <dir>`
is given an additional preopen of the (canonicalized) app directory so the app
can read the manifest there.

### Transparent usage tracking (the pass-through shim)

Registration captures *declared* intent. The **shim** captures *observed* usage
with zero coupling. `shims/wasmtime` is the `wvm` binary linked under another
name; `wvm shell-init` prepends `shims/` to `PATH`. An app that calls
`wasmtime` then re-enters wvm (busybox-style `argv[0]` dispatch), which:

1. resolves the active version (the same pin → session → default order and
   floating-spec auto-install as `wvm exec`);
2. appends one JSON line to `usage.log` — a single native append, deliberately
   avoiding a WASM boot or DB write on the hot path;
3. execs the real runtime (an absolute path, so `PATH` is not re-consulted and
   there is no recursion).

`usage.log` is the usage store itself — one JSON object per invocation, read and
aggregated in memory by `wvm usage` / `wvm list` and compacted to the most recent
entries once it grows past a cap. Each entry captures the full run:

```text
{ version, runtime_path, app, caller, cwd,
  args, module, module_path, module_sha256, invoked_at }
```

The module is identified best-effort (first positional that is a file or
`.wasm`/`.wat`/`.cwasm`), while the raw `args` remain the ground truth. Identity
is best-effort: `WVM_APP` is the app's own opt-in label, otherwise the parent
process name where the OS exposes it. `WVM_NO_USAGE=1` (or a leading
`--no-usage`) opts out.

This inverts the dependency arrow — instead of app → wvm, it is
wvm → (observing) → app — and complements registration: the shim sees only
runtimes reached through `PATH`, while registration covers apps that hardcode a
runtime and never touch the shim. `usage.log` and `apps.json` are
observed/declared history — plain files, not derived from anything else.

---

## Future Expansion

Potential future capabilities:

### Additional Runtimes

* Wasmtime
* Wasmer
* WasmEdge
* WAMR

### Runtime Channels

```
wvm install latest
wvm install lts
wvm install nightly
```

### Runtime Policies

```toml
[wvm]
channel = "lts"
```

### Optional Deduplication

Deduplication was tried (a content-addressable store) and removed after
measurement showed ~0.02% savings — wasmtime versions share no large files. It
could be revisited only if a future managed runtime actually shipped
substantial identical content across versions.

---

## Success Criteria

A user should be able to:

```
curl ... | sh
wvm install latest
wvm use latest
wasmtime --version
```

without manually downloading, configuring, locating, or managing a Wasmtime
installation.

WVM succeeds when Wasmtime becomes an implementation detail rather than a
prerequisite.
