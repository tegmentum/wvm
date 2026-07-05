# AGENTS.md

Guidance for AI coding agents working in this repo. (`CLAUDE.md` is a symlink to
this file.)

WVM is a Wasmtime version manager that is **self-hosting**: a thin native
binary runs the real application as a WebAssembly component on a protected seed
runtime. See [`docs/design.md`](docs/design.md) for the full architecture.

## Build

**Use `make`, not `cargo build`.** A bare `cargo build -p wvm` embeds an empty
placeholder wasm and produces a non-working binary (it prints a `composed app
wasm not found` warning). The real build is a pipeline:

```sh
make            # app (wasm) → wac compose with sqlite → embed into native `wvm`
```

Prereqs (once):

```sh
rustup target add wasm32-wasip2
cargo install wac-cli --locked
```

The SQLite component (`vendor/sqlite-core.wasm`) is vendored and committed —
nothing to fetch.

## Verify (run before committing)

```sh
make ci     # fmt --check + build + clippy (native AND wasm, -D warnings) + test
make act    # optional: run the GitHub CI workflow locally in Docker (needs Colima/Docker)
```

Clippy must pass on **both** targets — CI gates native and `wasm32-wasip2`
separately.

## Architecture

Three crates (workspace `default-members` deliberately **excludes** `wvm-app`,
so `cargo build`/`test` at the root skip the wasm crate — build it via `make`):

| Crate | Target | Role |
| --- | --- | --- |
| `crates/wvm-core` | native **and** wasm | Shared logic over `std::fs`: layout, store (CAS), manifest, discovery, specs, usage, cache. HTTP and the index are behind traits. |
| `crates/wvm-app` | `wasm32-wasip2` (cdylib) | The real application — every command. Exports `wasi:cli/run`; imports `wasi:http` + the `sqlite:wasm` component. This is where command logic lives. |
| `crates/wvm` | native | The only thing on `PATH`: bootstrapper + `wasmtime` shim. Downloads/locks the seed, embeds the composed app, launches it, handles `exec`/`--upgrade`/`completions`/`shell-init` natively. |

## Conventions

- **Commits:** [Conventional Commits](https://conventionalcommits.org). No
  emojis. Do not reference the assistant/AI in commit messages.
- Match surrounding style; keep comments at the density of the file you're in.
- Commit or push only when asked.

## Gotchas (wasm sandbox + boundary)

- `wvm-app` runs sandboxed: **no symlinks** (materialize with `copy`),
  `std::process::id()` **panics**, and the cwd/host are invisible — host arch/OS
  arrive via `WVM_HOST_ARCH`/`WVM_HOST_OS`, and `WVM_HOME` must be preopened.
  cwd-dependent work (`exec`, project-pin discovery) is handled **natively** in
  `crates/wvm/src/main.rs`.
- **Env vars the app reads must be forwarded** in `app_command()`
  (`crates/wvm/src/main.rs`) — the guest only sees an explicit allowlist (e.g.
  `WVM_REFRESH_INTERVAL`, `WVM_STALE_DAYS`). A new env knob that isn't forwarded
  silently has no effect in the app.
- The index is SQLite via the `sqlite:wasm` component (app-only); schema changes
  need a migration (`ALTER TABLE` guarded by `PRAGMA table_info`, see
  `index_component.rs`).
- The seed runtime is protected: never uninstalled, never GC'd.

## Docs & tooling

- Architecture/design: [`docs/design.md`](docs/design.md)
- Release process + CI details: [`README.md`](README.md), `CHANGELOG.md`
- Usage guidance (also shipped as a plugin): `.claude/skills/wvm/SKILL.md`
