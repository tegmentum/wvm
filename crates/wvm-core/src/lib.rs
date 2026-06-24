//! Shared WVM logic, compiled for both the native bootstrapper and the
//! `wasm32-wasip2` application component.
//!
//! Everything here is pure logic over `std::fs` plus pure-Rust crates. The two
//! environment-specific concerns — HTTP and the SQLite index — are abstracted
//! behind the [`http::Http`] and [`index::Index`] traits, implemented natively
//! in the `wvm` binary and over WASI/the SQLite component in `wvm-app`.

pub mod appmanifest;
pub mod archive;
pub mod config;
pub mod discovery;
pub mod hash;
pub mod http;
pub mod index;
pub mod layout;
pub mod manifest;
pub mod materialize;
pub mod platform;
pub mod store;
pub mod util;

pub use util::{human_bytes, normalize_version, version_cmp};
