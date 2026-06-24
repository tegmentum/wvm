//! Host platform detection and Wasmtime release asset naming.

use anyhow::{bail, Result};

/// A resolved host target for selecting a Wasmtime release asset.
#[derive(Debug, Clone)]
pub struct Platform {
    /// Wasmtime arch token, e.g. `x86_64`, `aarch64`.
    pub arch: &'static str,
    /// Wasmtime OS token, e.g. `linux`, `macos`, `windows`.
    pub os: &'static str,
    /// Archive extension for this OS (`tar.xz` or `zip`).
    pub ext: &'static str,
}

impl Platform {
    /// Detect the host platform.
    ///
    /// `WVM_HOST_ARCH` / `WVM_HOST_OS` override compile-time constants. This is
    /// essential inside the wasm app: `std::env::consts` there reports
    /// `wasm32`/`wasi`, so the bootstrapper passes the *real* host platform via
    /// these variables.
    pub fn detect() -> Result<Platform> {
        let arch_token = env_or("WVM_HOST_ARCH", std::env::consts::ARCH);
        let os_token = env_or("WVM_HOST_OS", std::env::consts::OS);

        let arch = match arch_token.as_str() {
            "x86_64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            other => bail!("unsupported CPU architecture: {other}"),
        };
        let (os, ext) = match os_token.as_str() {
            "linux" => ("linux", "tar.xz"),
            "macos" => ("macos", "tar.xz"),
            "windows" => ("windows", "zip"),
            other => bail!("unsupported operating system: {other}"),
        };
        Ok(Platform { arch, os, ext })
    }

    /// Manifest platform label, e.g. `macos-aarch64`.
    pub fn label(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }

    /// Wasmtime release asset name for a version, e.g.
    /// `wasmtime-v39.0.0-aarch64-macos.tar.xz`.
    pub fn asset_name(&self, version: &str) -> String {
        format!("wasmtime-v{version}-{}-{}.{}", self.arch, self.os, self.ext)
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}
