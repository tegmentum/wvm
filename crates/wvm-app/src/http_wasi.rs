//! `Http` implementation over `wasi:http`, via the `waki` client.
//!
//! `wasi:http` does not follow redirects, and GitHub redirects both API and
//! release-asset requests, so this follows 3xx hops manually.

use anyhow::{anyhow, bail, Result};
use std::path::Path;
use waki::Client;
use wvm_core::http::Http;

const USER_AGENT: &str = concat!("wvm/", env!("CARGO_PKG_VERSION"));
const MAX_REDIRECTS: usize = 10;

pub struct WasiHttp;

impl WasiHttp {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut current = url.to_string();
        for _ in 0..MAX_REDIRECTS {
            let resp = Client::new()
                .get(&current)
                .headers([
                    ("User-Agent", USER_AGENT),
                    ("Accept", "application/vnd.github+json"),
                ])
                .send()
                .map_err(|e| anyhow!("GET {current}: {e}"))?;

            let status = resp.status_code();
            if (300..400).contains(&status) {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow!("redirect {status} from {current} had no Location"))?;
                current = resolve_url(&current, location);
                continue;
            }
            if !(200..300).contains(&status) {
                bail!("unexpected HTTP {status} for {current}");
            }
            return resp.body().map_err(|e| anyhow!("reading body of {current}: {e}"));
        }
        bail!("too many redirects starting from {url}")
    }
}

impl Http for WasiHttp {
    fn get_string(&self, url: &str) -> Result<String> {
        let bytes = self.get_bytes(url)?;
        String::from_utf8(bytes).map_err(|e| anyhow!("response was not UTF-8: {e}"))
    }

    fn download(&self, url: &str, dest: &Path) -> Result<u64> {
        let bytes = self.get_bytes(url)?;
        std::fs::write(dest, &bytes)
            .map_err(|e| anyhow!("writing {}: {e}", dest.display()))?;
        Ok(bytes.len() as u64)
    }
}

/// Resolve a possibly-relative redirect target against the request URL.
fn resolve_url(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    if let Some(scheme_end) = base.find("://") {
        let after = &base[scheme_end + 3..];
        let origin_len = after.find('/').map(|i| scheme_end + 3 + i).unwrap_or(base.len());
        let origin = &base[..origin_len];
        if location.starts_with('/') {
            return format!("{origin}{location}");
        }
        return format!("{origin}/{location}");
    }
    location.to_string()
}
