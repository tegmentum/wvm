# AGENTS.md

Guidance for AI coding agents working in this repo. (`CLAUDE.md` is a symlink to
this file.)

WVM is a Wasmtime version manager that is **self-hosting**: a thin native
binary runs the real application as a WebAssembly component on a protected seed
runtime. See [`docs/design.md`](docs/design.md) for the full architecture.

## Build

The native `wvm` binary embeds the wasm app component, so the build is two
steps (build the app for wasm, then the native binary that embeds it). One
command drives it:

```sh
cargo xtask build        # or `cargo xtask ci` for fmt + clippy (both targets) + test
```

Prereq (once): `rustup target add wasm32-wasip2`.

A bare `cargo build -p wvm` embeds an empty placeholder (it prints a warning and
the binary errors at runtime) — always use `cargo xtask build` for a working
binary.

## Verify (run before committing)

```sh
cargo xtask ci      # fmt --check + build + clippy (native AND wasm, -D warnings) + test
cargo xtask act     # optional: run the GitHub CI workflow locally in Docker (needs Colima/Docker)
```

Clippy must pass on **both** targets — CI gates native and `wasm32-wasip2`
separately.

## Architecture

| Crate | Target | Role |
| --- | --- | --- |
| `crates/wvm-core` | native **and** wasm | Shared logic over `std::fs`: layout, manifest, discovery, specs, apps registry, usage, cache. HTTP is behind a trait. |
| `crates/wvm-app` | `wasm32-wasip2` (cdylib) | The real application — every command. Exports `wasi:cli/run`; imports only WASI + `wasi:http`. This is where command logic lives. |
| `crates/wvm` | native | The only thing on `PATH`: bootstrapper + `wasmtime` shim. Downloads/locks the seed, embeds the app wasm, launches it; handles `exec`/`--upgrade`/`completions`/`shell-init` natively. |
| `xtask` | native | Build orchestration (`cargo xtask …`). |

**Storage is plain files** — no database, no content-addressable store. Each
runtime version is extracted directly into `runtimes/wasmtime/versions/<v>/`;
`apps.json` holds registrations and `usage.log` (JSONL) holds usage. Everything
else is derived by scanning version dirs + each `manifest.json`.

## Conventions

- **Commits:** [Conventional Commits](https://conventionalcommits.org). No
  emojis. Do not reference the assistant/AI in commit messages.
- Match surrounding style; keep comments at the density of the file you're in.
- Commit or push only when asked.

## Gotchas (the wasm boundary)

- `wvm-app` runs sandboxed: `std::process::id()` **panics**, and the cwd/host
  are invisible — host arch/OS arrive via `WVM_HOST_ARCH`/`WVM_HOST_OS`, and
  `WVM_HOME` must be preopened. cwd-dependent work (`exec`, project-pin
  discovery) is handled **natively** in `crates/wvm/src/main.rs`.
- **Env vars the app reads must be forwarded** in `app_command()`
  (`crates/wvm/src/main.rs`) — the guest only sees an explicit allowlist (e.g.
  `WVM_REFRESH_INTERVAL`, `WVM_STALE_DAYS`). A new env knob that isn't forwarded
  silently has no effect in the app.
- The seed runtime is protected: never uninstalled.

## Docs & tooling

- Architecture/design: [`docs/design.md`](docs/design.md)
- Release process + CI details: [`README.md`](README.md), `CHANGELOG.md`
- Usage guidance (also shipped as a plugin): `.claude/skills/wvm/SKILL.md`
