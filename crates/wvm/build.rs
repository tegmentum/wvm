//! Embed the WASM app component into the native binary.
//!
//! The Makefile builds `wvm-app` for `wasm32-wasip2`, producing
//! `target/wasm32-wasip2/release/wvm_app.wasm`. This script copies that into
//! `OUT_DIR/app.wasm` for `include_bytes!`. If the wasm is absent (e.g. a bare
//! `cargo build` during development), an empty placeholder is written so the
//! crate still compiles; the binary reports a clear error at runtime if the app
//! is missing.

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let dest = out_dir.join("app.wasm");

    // Allow an explicit override, else default to the workspace target path.
    let candidate = std::env::var("WVM_APP_WASM").ok().map(PathBuf::from);
    let default = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("wasm32-wasip2")
        .join("release")
        .join("wvm_app.wasm");

    let source = candidate.filter(|p| p.exists()).or_else(|| {
        if default.exists() {
            Some(default.clone())
        } else {
            None
        }
    });

    match source {
        Some(src) => {
            std::fs::copy(&src, &dest).expect("copying composed app wasm");
            println!("cargo:rerun-if-changed={}", src.display());
        }
        None => {
            std::fs::write(&dest, []).expect("writing placeholder app wasm");
            println!("cargo:warning=composed app wasm not found; embedding empty placeholder (run `make` to build the app)");
        }
    }
    println!("cargo:rerun-if-env-changed=WVM_APP_WASM");
}
