//! Shared WVM logic, compiled for both the native bootstrapper and the
//! `wasm32-wasip2` application component.
//!
//! Everything here is pure logic over `std::fs` plus pure-Rust crates. The one
//! environment-specific concern — HTTP — is abstracted behind the
//! [`http::Http`] trait, implemented natively in the `wvm` binary and over WASI
//! in `wvm-app`. Persistence is plain files: runtime versions extracted into
//! their directories, `apps.json` for registrations, and `usage.log` for
//! observed invocations.

pub mod appmanifest;
pub mod apps;
pub mod archive;
pub mod cache;
pub mod discovery;
pub mod hash;
pub mod http;
pub mod layout;
pub mod manifest;
pub mod platform;
pub mod shell;
pub mod spec;
pub mod usage;
pub mod util;

pub use spec::VersionSpec;
pub use util::{human_bytes, is_lts, normalize_version, version_cmp};
