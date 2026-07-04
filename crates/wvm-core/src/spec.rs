//! Version specifiers: floating channels and exact pins.
//!
//! A spec is what the user *asks for*; a concrete version is what it *resolves
//! to* against a set of candidates (installed or available). Floating specs let
//! a pin track a line automatically:
//!
//! | Spec              | Meaning              | Example resolution      |
//! |-------------------|----------------------|-------------------------|
//! | `latest`          | newest overall       | → `35.0.0`              |
//! | `lts`             | newest LTS line      | → `24.0.3`              |
//! | `24` / `24.x`     | latest major line    | newest `24.*`           |
//! | `24.0` / `24.0.x` | latest major/minor   | newest `24.0.*`         |
//! | `24.0.1`          | exact / frozen       | exactly `24.0.1`        |
//!
//! `default`, `use`, and project pins store the *spec* (not a frozen version)
//! so `default = "24"` floats forward as newer `24.x` patches are installed.

use crate::util::{is_lts, normalize_version, version_cmp};
use std::fmt;
use std::str::FromStr;

/// A requested version: a floating channel or an exact pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// Newest available version.
    Latest,
    /// Newest LTS version (major divisible by 12).
    Lts,
    /// Latest `major.*` (float minor and patch).
    Major(u64),
    /// Latest `major.minor.*` (float patch).
    MajorMinor(u64, u64),
    /// An exact `major.minor.patch`.
    Exact(String),
}

impl VersionSpec {
    /// Parse a spec string. Accepts `latest`, `lts`, a leading `v`, and `x`/`*`
    /// wildcards (`24.x`, `24.0.*`). Bare `24` / `24.0` are equivalent to their
    /// wildcard forms.
    pub fn parse(input: &str) -> Result<VersionSpec, String> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err("empty version spec".to_string());
        }
        match raw.to_ascii_lowercase().as_str() {
            "latest" | "*" | "x" => return Ok(VersionSpec::Latest),
            "lts" => return Ok(VersionSpec::Lts),
            _ => {}
        }

        // Collect the numeric prefix, stopping at a wildcard/empty part so
        // `24.x` yields `[24]` and `24.0.*` yields `[24, 0]`.
        let mut nums: Vec<u64> = Vec::new();
        for part in normalize_version(raw).split('.') {
            let p = part.trim();
            if p.is_empty() || p.eq_ignore_ascii_case("x") || p == "*" {
                break;
            }
            match p.parse::<u64>() {
                Ok(n) => nums.push(n),
                Err(_) => return Err(format!("invalid version spec '{input}'")),
            }
        }

        match nums.as_slice() {
            [m] => Ok(VersionSpec::Major(*m)),
            [m, mi] => Ok(VersionSpec::MajorMinor(*m, *mi)),
            [m, mi, pa, ..] => Ok(VersionSpec::Exact(format!("{m}.{mi}.{pa}"))),
            _ => Err(format!("invalid version spec '{input}'")),
        }
    }

    /// True for channels that can advance as new versions appear; `false` only
    /// for [`VersionSpec::Exact`].
    pub fn is_floating(&self) -> bool {
        !matches!(self, VersionSpec::Exact(_))
    }

    /// Whether a concrete version satisfies this spec.
    pub fn matches(&self, version: &str) -> bool {
        let comps = numeric_parts(version);
        match self {
            VersionSpec::Latest => true,
            VersionSpec::Lts => is_lts(version),
            VersionSpec::Major(m) => comps.first() == Some(m),
            VersionSpec::MajorMinor(m, mi) => comps.first() == Some(m) && comps.get(1) == Some(mi),
            VersionSpec::Exact(e) => normalize_version(version) == normalize_version(e),
        }
    }

    /// The newest candidate satisfying this spec, if any.
    pub fn resolve<'a, S: AsRef<str>>(&self, candidates: &'a [S]) -> Option<&'a str> {
        candidates
            .iter()
            .map(AsRef::as_ref)
            .filter(|c| self.matches(c))
            .max_by(|a, b| version_cmp(a, b))
    }
}

impl FromStr for VersionSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        VersionSpec::parse(s)
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionSpec::Latest => f.write_str("latest"),
            VersionSpec::Lts => f.write_str("lts"),
            VersionSpec::Major(m) => write!(f, "{m}"),
            VersionSpec::MajorMinor(m, mi) => write!(f, "{m}.{mi}"),
            VersionSpec::Exact(s) => f.write_str(s),
        }
    }
}

/// Numeric dotted components of a version, e.g. `"24.0.1"` → `[24, 0, 1]`.
fn numeric_parts(v: &str) -> Vec<u64> {
    normalize_version(v)
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_channels() {
        assert_eq!(VersionSpec::parse("latest").unwrap(), VersionSpec::Latest);
        assert_eq!(VersionSpec::parse("LATEST").unwrap(), VersionSpec::Latest);
        assert_eq!(VersionSpec::parse("lts").unwrap(), VersionSpec::Lts);
    }

    #[test]
    fn parses_major_forms() {
        for s in ["24", "24.x", "24.*", "v24", "24.X"] {
            assert_eq!(
                VersionSpec::parse(s).unwrap(),
                VersionSpec::Major(24),
                "for {s}"
            );
        }
    }

    #[test]
    fn parses_major_minor_forms() {
        for s in ["24.0", "24.0.x", "24.0.*", "v24.0"] {
            assert_eq!(
                VersionSpec::parse(s).unwrap(),
                VersionSpec::MajorMinor(24, 0),
                "for {s}"
            );
        }
    }

    #[test]
    fn parses_exact() {
        assert_eq!(
            VersionSpec::parse("24.0.1").unwrap(),
            VersionSpec::Exact("24.0.1".to_string())
        );
        assert_eq!(
            VersionSpec::parse("v35.0.0").unwrap(),
            VersionSpec::Exact("35.0.0".to_string())
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(VersionSpec::parse("").is_err());
        assert!(VersionSpec::parse("abc").is_err());
        assert!(VersionSpec::parse("24.foo").is_err());
    }

    #[test]
    fn floating_flag() {
        assert!(VersionSpec::parse("latest").unwrap().is_floating());
        assert!(VersionSpec::parse("24").unwrap().is_floating());
        assert!(VersionSpec::parse("24.0").unwrap().is_floating());
        assert!(!VersionSpec::parse("24.0.1").unwrap().is_floating());
    }

    #[test]
    fn resolves_latest_and_lts() {
        let all = v(&["23.0.5", "24.0.0", "24.0.3", "35.0.0"]);
        assert_eq!(VersionSpec::Latest.resolve(&all), Some("35.0.0"));
        assert_eq!(VersionSpec::Lts.resolve(&all), Some("24.0.3"));
    }

    #[test]
    fn resolves_major_line() {
        let all = v(&["24.0.0", "24.0.3", "24.1.0", "25.0.0"]);
        assert_eq!(VersionSpec::Major(24).resolve(&all), Some("24.1.0"));
        assert_eq!(VersionSpec::MajorMinor(24, 0).resolve(&all), Some("24.0.3"));
    }

    #[test]
    fn resolves_exact_and_misses() {
        let all = v(&["24.0.0", "24.0.3"]);
        assert_eq!(
            VersionSpec::Exact("24.0.3".to_string()).resolve(&all),
            Some("24.0.3")
        );
        assert_eq!(VersionSpec::Major(99).resolve(&all), None);
    }
}
