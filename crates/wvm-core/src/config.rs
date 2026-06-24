//! User configuration (`~/.tegmentum/wvm/config.toml`).

use crate::layout::Layout;
use anyhow::{Context, Result};
use serde::Deserialize;

/// How version directories reference stored objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Materialization {
    Symlink,
    Copy,
}

impl Materialization {
    pub fn as_str(self) -> &'static str {
        match self {
            Materialization::Symlink => "symlink",
            Materialization::Copy => "copy",
        }
    }

    pub fn parse(s: &str) -> Result<Materialization> {
        match s {
            "symlink" => Ok(Materialization::Symlink),
            "copy" => Ok(Materialization::Copy),
            "hardlink" | "reflink" => {
                anyhow::bail!("materialization strategy '{s}' is not yet implemented (v1 supports: symlink, copy)")
            }
            other => anyhow::bail!("unknown materialization strategy: {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub materialization: Materialization,
}

impl Default for Config {
    fn default() -> Self {
        Config { materialization: Materialization::Symlink }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawFile {
    wvm: Option<RawWvm>,
}

#[derive(Debug, Default, Deserialize)]
struct RawWvm {
    materialization: Option<String>,
}

impl Config {
    /// Load `config.toml` if present, otherwise return defaults.
    pub fn load(layout: &Layout) -> Result<Config> {
        let path = layout.config_file();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let raw: RawFile = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;

        let mut cfg = Config::default();
        if let Some(m) = raw.wvm.and_then(|w| w.materialization) {
            cfg.materialization = Materialization::parse(&m)?;
        }
        Ok(cfg)
    }
}
