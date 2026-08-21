//! Parsing of Java version requests: `25`, `25.0.4`, `>=25`, `lts`, `latest`,
//! `temurin@25`, `temurin-25.0.4.1+1`.

use std::fmt;
use std::str::FromStr;

use crate::distribution::JavaDistribution;
use crate::error::Error;
use crate::version::{JavaVersion, KNOWN_LTS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// No constraint stated; the caller supplies a default.
    Any,
    /// The newest long-term-support release.
    Lts,
    /// The newest generally-available release, LTS or not.
    Latest,
    /// A dotted prefix: `25` matches `25.0.4.1`, `25.0.4` matches `25.0.4.1`.
    Prefix(Vec<u32>),
    /// An open lower bound, as in `>=25`.
    AtLeast(Vec<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRequest {
    pub distribution: Option<JavaDistribution>,
    pub spec: VersionSpec,
}

impl VersionRequest {
    pub fn any() -> Self {
        Self {
            distribution: None,
            spec: VersionSpec::Any,
        }
    }

    /// The requirement applied when nothing else states one: Java 25 or newer, per the
    /// Hytale server manual.
    pub fn default_requirement() -> Self {
        Self {
            distribution: None,
            spec: VersionSpec::AtLeast(vec![25]),
        }
    }

    /// Whether an already-present installation satisfies this request.
    ///
    /// Note that `>=25` is satisfied by 26. The preference for LTS applies to what we
    /// *install* when nothing suitable exists, not to what we *accept* when it does; see
    /// [`crate::resolve`].
    pub fn matches(&self, distribution: JavaDistribution, version: &JavaVersion) -> bool {
        if let Some(wanted) = self.distribution
            && wanted != distribution
        {
            return false;
        }
        match &self.spec {
            VersionSpec::Any | VersionSpec::Latest => true,
            VersionSpec::Lts => KNOWN_LTS.contains(&version.major()),
            VersionSpec::Prefix(prefix) => version.starts_with(prefix),
            VersionSpec::AtLeast(bound) => {
                *version >= JavaVersion::new(bound.clone(), None)
            }
        }
    }

    /// The lower bound this request implies, if any. Used to explain why a system JDK was
    /// rejected.
    pub fn lower_bound(&self) -> Option<JavaVersion> {
        match &self.spec {
            VersionSpec::Prefix(p) | VersionSpec::AtLeast(p) => {
                Some(JavaVersion::new(p.clone(), None))
            }
            _ => None,
        }
    }
}

impl FromStr for VersionRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw = s.trim();
        if raw.is_empty() {
            return Err(Error::InvalidRequest(s.to_string()));
        }

        // Split an optional distribution prefix: `temurin@25` or `temurin-25.0.4.1+1`.
        let (distribution, rest) = match raw.split_once('@') {
            Some((dist, rest)) => (
                Some(
                    dist.parse::<JavaDistribution>()
                        .map_err(|()| Error::InvalidRequest(s.to_string()))?,
                ),
                rest,
            ),
            None => match raw.split_once('-') {
                // Only treat the leading token as a distribution if it actually names one,
                // so `>=25` and bare versions are unaffected.
                Some((dist, rest)) if dist.parse::<JavaDistribution>().is_ok() => {
                    (Some(dist.parse().unwrap()), rest)
                }
                _ => (None, raw),
            },
        };

        let spec = match rest.to_ascii_lowercase().as_str() {
            "" | "any" | "*" => VersionSpec::Any,
            "lts" => VersionSpec::Lts,
            "latest" | "newest" => VersionSpec::Latest,
            _ => {
                if let Some(bound) = rest.strip_prefix(">=").or_else(|| rest.strip_prefix('>')) {
                    VersionSpec::AtLeast(parse_components(bound, s)?)
                } else {
                    // A trailing `+`, as in `25+`, reads as a lower bound too. Take care not
                    // to catch build numbers like `25.0.4.1+1`.
                    match rest.strip_suffix('+') {
                        Some(bound) if !bound.contains('+') => {
                            VersionSpec::AtLeast(parse_components(bound, s)?)
                        }
                        _ => VersionSpec::Prefix(parse_components(rest, s)?),
                    }
                }
            }
        };

        Ok(Self { distribution, spec })
    }
}

/// Parse a dotted version prefix, ignoring any `+build` suffix.
fn parse_components(input: &str, original: &str) -> Result<Vec<u32>, Error> {
    let trimmed = input.split('+').next().unwrap_or(input);
    if trimmed.is_empty() {
        return Err(Error::InvalidRequest(original.to_string()));
    }
    trimmed
        .split('.')
        .map(|c| {
            c.parse::<u32>()
                .map_err(|_| Error::InvalidRequest(original.to_string()))
        })
        .collect()
}

impl fmt::Display for VersionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(dist) = self.distribution {
            write!(f, "{dist}@")?;
        }
        match &self.spec {
            VersionSpec::Any => f.write_str("any"),
            VersionSpec::Lts => f.write_str("lts"),
            VersionSpec::Latest => f.write_str("latest"),
            VersionSpec::Prefix(p) => {
                write!(f, "{}", join(p))
            }
            VersionSpec::AtLeast(p) => write!(f, ">={}", join(p)),
        }
    }
}

fn join(parts: &[u32]) -> String {
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> VersionRequest {
        s.parse().unwrap()
    }

    #[test]
    fn parses_forms() {
        assert_eq!(parse("25").spec, VersionSpec::Prefix(vec![25]));
        assert_eq!(parse("25.0.4").spec, VersionSpec::Prefix(vec![25, 0, 4]));
        assert_eq!(parse(">=25").spec, VersionSpec::AtLeast(vec![25]));
        assert_eq!(parse("25+").spec, VersionSpec::AtLeast(vec![25]));
        assert_eq!(parse("lts").spec, VersionSpec::Lts);
        assert_eq!(parse("latest").spec, VersionSpec::Latest);
    }

    #[test]
    fn parses_distribution_prefix() {
        let r = parse("temurin@25");
        assert_eq!(r.distribution, Some(JavaDistribution::Temurin));
        assert_eq!(r.spec, VersionSpec::Prefix(vec![25]));

        // The pin-file form.
        let r = parse("temurin-25.0.4.1+1");
        assert_eq!(r.distribution, Some(JavaDistribution::Temurin));
        assert_eq!(r.spec, VersionSpec::Prefix(vec![25, 0, 4, 1]));

        // The manual calls it Adoptium; accept that spelling.
        assert_eq!(parse("adoptium@25").distribution, Some(JavaDistribution::Temurin));
    }

    #[test]
    fn matching_semantics() {
        let v25: JavaVersion = "25.0.4.1+1".parse().unwrap();
        let v26: JavaVersion = "26.0.2+10".parse().unwrap();
        let d = JavaDistribution::Temurin;

        // An open bound accepts a newer non-LTS release when it is already present.
        assert!(parse(">=25").matches(d, &v25));
        assert!(parse(">=25").matches(d, &v26));

        // A prefix does not.
        assert!(parse("25").matches(d, &v25));
        assert!(!parse("25").matches(d, &v26));

        assert!(parse("lts").matches(d, &v25));
        assert!(!parse("lts").matches(d, &v26));
    }

    #[test]
    fn rejects_garbage() {
        assert!("".parse::<VersionRequest>().is_err());
        assert!("twenty-five".parse::<VersionRequest>().is_err());
        assert!(">=".parse::<VersionRequest>().is_err());
    }
}
