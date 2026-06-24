//! HTTP abstraction. Implemented natively with `ureq` (for the seed download)
//! and in the app over `wasi:http`.

use anyhow::Result;
use std::path::Path;

/// A minimal HTTP client: fetch a body as text, or stream a download to a file.
/// Implementations must follow 3xx redirects (GitHub redirects release assets).
pub trait Http {
    /// GET a URL and return the response body as a string.
    fn get_string(&self, url: &str) -> Result<String>;

    /// GET a URL and write the body to `dest`, returning bytes written.
    fn download(&self, url: &str, dest: &Path) -> Result<u64>;
}
