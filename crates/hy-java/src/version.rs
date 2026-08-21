//! Java version numbers.
//!
//! Java releases are not semver: `25.0.4.1+1` has four dotted components plus a build
//! number. We compare component-wise, then by build.

use std::fmt;
use std::str::FromStr;

/// Feature versions that are (or are scheduled as) long-term-support releases.
///
/// Used when the Adoptium API is unreachable; the live `available_lts_releases` list wins
/// whenever we have it.
pub const KNOWN_LTS: &[u32] = &[8, 11, 17, 21, 25];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JavaVersion {
    components: Vec<u32>,
    build: Option<u32>,
}

impl JavaVersion {
    pub fn new(components: Vec<u32>, build: Option<u32>) -> Self {
        Self { components, build }
    }

    /// The feature version, e.g. `25` for `25.0.4.1+1`.
    pub fn major(&self) -> u32 {
        self.components.first().copied().unwrap_or(0)
    }

    pub fn components(&self) -> &[u32] {
        &self.components
    }

    pub fn build(&self) -> Option<u32> {
        self.build
    }

    /// Whether this version starts with `prefix`, so `25.0.4.1` matches `25` and `25.0.4`.
    pub fn starts_with(&self, prefix: &[u32]) -> bool {
        prefix.len() <= self.components.len()
            && prefix.iter().zip(&self.components).all(|(a, b)| a == b)
    }

    /// Parse a release name as produced by the Adoptium API, e.g. `jdk-25.0.4.1+1`.
    pub fn from_release_name(name: &str) -> Option<Self> {
        let trimmed = name.strip_prefix("jdk-").unwrap_or(name);
        // Java 8 releases look like `jdk8u462-b08`; normalise to `8.0.462+8`.
        if let Some(rest) = trimmed.strip_prefix("jdk8u") {
            let (update, build) = rest.split_once("-b")?;
            return Some(Self {
                components: vec![8, 0, update.parse().ok()?],
                build: build.parse().ok(),
            });
        }
        trimmed.parse().ok()
    }
}

impl FromStr for JavaVersion {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (version, build) = match s.split_once('+') {
            Some((v, b)) => (v, Some(b.parse().map_err(|_| ParseVersionError)?)),
            None => (s, None),
        };
        let components = version
            .split('.')
            .map(|c| c.parse().map_err(|_| ParseVersionError))
            .collect::<Result<Vec<u32>, _>>()?;
        if components.is_empty() {
            return Err(ParseVersionError);
        }
        Ok(Self { components, build })
    }
}

impl Ord for JavaVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Pad the shorter component list with zeros so `25` == `25.0.0`.
        let len = self.components.len().max(other.components.len());
        for i in 0..len {
            let a = self.components.get(i).copied().unwrap_or(0);
            let b = other.components.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                std::cmp::Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.build.unwrap_or(0).cmp(&other.build.unwrap_or(0))
    }
}

impl PartialOrd for JavaVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for JavaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self
            .components
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".");
        write!(f, "{joined}")?;
        if let Some(build) = self.build {
            write!(f, "+{build}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseVersionError;

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid Java version")
    }
}

impl std::error::Error for ParseVersionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_roundtrips() {
        let v: JavaVersion = "25.0.4.1+1".parse().unwrap();
        assert_eq!(v.major(), 25);
        assert_eq!(v.build(), Some(1));
        assert_eq!(v.to_string(), "25.0.4.1+1");
    }

    #[test]
    fn orders_by_component_then_build() {
        let a: JavaVersion = "25.0.4+7".parse().unwrap();
        let b: JavaVersion = "25.0.4.1+1".parse().unwrap();
        let c: JavaVersion = "26.0.2+10".parse().unwrap();
        assert!(a < b, "25.0.4+7 should precede 25.0.4.1+1");
        assert!(b < c);
        // Padding: `25` and `25.0.0` compare equal on components, build breaks the tie.
        let short: JavaVersion = "25".parse().unwrap();
        let long: JavaVersion = "25.0.0".parse().unwrap();
        assert_eq!(short.cmp(&long), std::cmp::Ordering::Equal);
    }

    #[test]
    fn prefix_matching() {
        let v: JavaVersion = "25.0.4.1+1".parse().unwrap();
        assert!(v.starts_with(&[25]));
        assert!(v.starts_with(&[25, 0, 4]));
        assert!(!v.starts_with(&[25, 0, 3]));
        assert!(!v.starts_with(&[26]));
    }

    #[test]
    fn parses_release_names() {
        assert_eq!(
            JavaVersion::from_release_name("jdk-25.0.4.1+1").unwrap().to_string(),
            "25.0.4.1+1"
        );
        assert_eq!(
            JavaVersion::from_release_name("jdk8u462-b08").unwrap().to_string(),
            "8.0.462+8"
        );
    }
}
