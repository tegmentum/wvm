# WVM - WebAssembly Version Manager

## Status

Draft

## Overview

WVM (WebAssembly Version Manager) is a lightweight runtime manager for
WebAssembly runtimes and toolchains.

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

## Simple CAS Store

WVM may use a simple file-level content-addressable store for deduplication.
This is an implementation detail, not the public identity model of WVM.

### Goals

- Avoid duplicate file storage across runtime versions.
- Preserve simple version directories.
- Support integrity verification.
- Avoid dependency resolution, graph modeling, or package-manager behavior.

### Non-Goals

- Global Tegmentum CAS.
- Cross-project artifact registry.
- Garbage-collected package graph.
- OCI/image-style layering.
- Distributed synchronization.

### Layout

```text
~/.tegmentum/wvm/
  store/
    sha256/
      ab/
        cd/
          abcdef...
  runtimes/
    wasmtime/
      versions/
        39.0.0/
          bin/wasmtime -> ../../../../store/sha256/ab/cd/abcdef...
          manifest.json
```

### Object Identity

Each stored file is addressed by:

```
sha256(file_bytes)
```

The object path is:

```
store/sha256/<first-2>/<next-2>/<full-digest>
```

Example:

```
store/sha256/8f/21/8f21c0...
```

### Install Flow

```
download archive
  ↓
verify archive checksum/signature
  ↓
extract to staging directory
  ↓
hash each file
  ↓
copy unique files into store
  ↓
materialize version directory
  ↓
write manifest
  ↓
atomically publish version
```

### Materialization

V1 should use symlinks by default.

Optional future strategies:

```
symlink
hardlink
reflink
copy
```

The materialization strategy should be configurable, but the default should
remain simple and inspectable.

### Manifest

```json
{
  "runtime": "wasmtime",
  "version": "39.0.0",
  "platform": "linux-x86_64",
  "archive_sha256": "...",
  "materialization": "symlink",
  "files": [
    {
      "path": "bin/wasmtime",
      "sha256": "...",
      "mode": "0755",
      "size": 12345678
    }
  ]
}
```

### Verification

`wvm verify` should:

```
read manifest
check each file path exists
resolve symlink target
hash target bytes
compare sha256
verify mode
```

### Garbage Collection

V1 garbage collection can be conservative.

Algorithm:

```
collect all sha256 values referenced by installed manifests
walk store/sha256
delete unreferenced objects only when --prune is passed
```

Command:

```
wvm gc --prune
```

Default `wvm gc` should only report reclaimable space.

### Index Database

WVM maintains a small SQLite index at `~/.tegmentum/wvm/index.db` to track
object backlinks and version metadata.

The index is a **derived cache, not a source of truth.** The store and the
per-version manifests on disk remain authoritative; the index can always be
rebuilt from them. A missing, stale, or corrupt index is never fatal — it is
reconciled from disk before any destructive operation.

Schema (conceptually):

```text
objects(digest PRIMARY KEY, size)
versions(id, runtime, version, platform, archive_sha256, materialization, installed_at)
object_refs(version_id -> versions, digest -> objects, path, mode, size)   # backlinks
```

The index serves two purposes:

- **Backlinks.** `object_refs` records which versions reference each object, so
  GC is the indexed query "objects with no backlinks" rather than a full
  manifest scan. `wvm objects` surfaces these backlinks for inspection.
- **Metadata.** Per-version platform, archive digest, materialization strategy,
  and install time are queryable without re-reading every manifest.

Lifecycle:

- `install` / `uninstall` update the index live (best-effort).
- `gc` reindexes from disk first (objects from the store, backlinks from
  manifests), guaranteeing correctness and catching orphans from interrupted
  installs, then deletes objects with zero backlinks.

This keeps the CAS itself boring: the index accelerates and enriches GC without
becoming the source of truth.

### Rule

The CAS must stay boring.

It stores files by hash and materializes runtime directories.

It does not resolve dependencies, own global package identity, or become the
center of the WVM architecture.

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
  std command shape, which pins `run@0.2.0`). It additionally imports
  `wasi:http` (downloads, via `waki`) and `sqlite:wasm/high-level` (the index),
  the latter satisfied by composing in a vendored SQLite component with `wac`.
  Because it is a standard wasi:cli command, it also runs directly under any
  wasi:cli host (e.g. `wasmtime run … wvm-app.composed.wasm -- list`). The WASI
  WIT is vendored under `crates/wvm-app/wit/deps` (fetched with `wkg`).

### Protected seed runtime

The seed is the Wasmtime that runs the app. It lives in `WVM_HOME/seed/`,
separate from user-managed versions, is downloaded once and recorded in a
`SEED` marker, and is set read-only. WVM's own commands never list or delete
it: `wvm uninstall <seed>` is refused and `wvm gc` only walks the object store,
never the seed directory. Users still install and select their own runtimes
independently.

### Version selection: default vs. session

Two layers, nvm-style:

- **default** — persistent (`runtimes/wasmtime/default`), used by new shells;
  set with `wvm default <version>`.
- **session** — the `WVM_VERSION` environment variable, set per shell by
  `wvm use <version>`, overriding the default for the current session only.

Resolution order (`wvm exec` and `wvm current`): project pin (`wvm.toml`) →
session (`WVM_VERSION`) → default → `WASMTIME_HOME` → `PATH`.

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

### Index (cache)

```text
apps(name PRIMARY KEY, path, runtime_path, registered_at)
app_runtimes(app -> apps, version)         # app depends on a wvm-managed version
```

An app that sets `runtime-path` is fully decoupled — it is recorded for
visibility but has no `app_runtimes` rows and pins no wvm-managed runtime.

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

A lightweight content-addressable store may be introduced later if storage
becomes a practical concern.

This is intentionally deferred.

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
