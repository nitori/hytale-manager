//! The Hytale maven repository.
//!
//! ```text
//! https://maven.hytale.com/<patchline>/com/hypixel/hytale/Server/maven-metadata.xml
//! https://maven.hytale.com/<patchline>/com/hypixel/hytale/Server/<v>/Server-<v>.jar
//! ```
//!
//! Only five versions are retained per patchline, so a pinned old version eventually 404s.

use quick_xml::events::Event;

use crate::error::{Error, Result};

pub const BASE_URL: &str = "https://maven.hytale.com";
const ARTIFACT_PATH: &str = "com/hypixel/hytale/Server";

/// Versions published on one patchline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// The newest published version, prereleases included.
    pub latest: Option<String>,
    /// The version the repository considers current.
    pub release: Option<String>,
    /// Oldest first, as maven writes them.
    pub versions: Vec<String>,
}

impl Metadata {
    /// What an unqualified install should pick.
    pub fn current(&self) -> Option<&str> {
        self.release
            .as_deref()
            .or(self.latest.as_deref())
            .or_else(|| self.versions.last().map(String::as_str))
    }

    pub fn contains(&self, version: &str) -> bool {
        self.versions.iter().any(|v| v == version)
    }
}

/// Extract `<latest>`, `<release>`, and `<versions>` without modelling the whole document.
pub fn parse(xml: &str) -> std::result::Result<Metadata, quick_xml::Error> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut metadata = Metadata {
        latest: None,
        release: None,
        versions: Vec::new(),
    };
    // `<version>` appears only inside `<versions>`, but `<latest>` and `<release>` are
    // siblings of it, so tracking the current element name is enough.
    let mut element = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                element = String::from_utf8_lossy(tag.name().as_ref()).into_owned();
            }
            Ok(Event::End(_)) => element.clear(),
            Ok(Event::Text(text)) => {
                let raw = String::from_utf8_lossy(&text.into_inner()).into_owned();
                // Version strings never carry entities, but decoding them is free.
                let value = match quick_xml::escape::unescape(&raw) {
                    Ok(unescaped) => unescaped.trim().to_string(),
                    Err(_) => raw.trim().to_string(),
                };
                if value.is_empty() {
                    continue;
                }
                match element.as_str() {
                    "latest" => metadata.latest = Some(value),
                    "release" => metadata.release = Some(value),
                    "version" => metadata.versions.push(value),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(err),
        }
    }

    Ok(metadata)
}

pub fn metadata_url(patchline: &str) -> String {
    format!("{BASE_URL}/{patchline}/{ARTIFACT_PATH}/maven-metadata.xml")
}

pub fn jar_url(patchline: &str, version: &str) -> String {
    format!("{BASE_URL}/{patchline}/{ARTIFACT_PATH}/{version}/Server-{version}.jar")
}

pub fn sha1_url(patchline: &str, version: &str) -> String {
    format!("{}.sha1", jar_url(patchline, version))
}

/// Order two versions, treating unparsable ones as equal so a format change degrades to
/// "no opinion" rather than a wrong answer.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Pick the version to install: `requested` if it is published, otherwise the current one.
pub fn select(metadata: &Metadata, patchline: &str, requested: Option<&str>) -> Result<String> {
    match requested {
        Some(version) => {
            if metadata.contains(version) {
                Ok(version.to_string())
            } else {
                Err(Error::NotPublished {
                    version: version.to_string(),
                    patchline: patchline.to_string(),
                    available: metadata.versions.join(", "),
                })
            }
        }
        None => metadata
            .current()
            .map(str::to_string)
            .ok_or_else(|| Error::NoVersions(patchline.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live document, retrieved 2026-08-22.
    const RELEASE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>com.hypixel.hytale</groupId>
  <artifactId>Server</artifactId>
  <versioning>
    <latest>0.5.9</latest>
    <release>0.5.9</release>
    <versions>
      <version>0.5.5</version>
      <version>0.5.6</version>
      <version>0.5.7</version>
      <version>0.5.8</version>
      <version>0.5.9</version>
    </versions>
    <lastUpdated>20260818221739</lastUpdated>
  </versioning>
</metadata>"#;

    #[test]
    fn parses_the_live_release_metadata() {
        let metadata = parse(RELEASE_XML).unwrap();
        assert_eq!(metadata.latest.as_deref(), Some("0.5.9"));
        assert_eq!(metadata.release.as_deref(), Some("0.5.9"));
        assert_eq!(metadata.versions.len(), 5);
        assert_eq!(metadata.current(), Some("0.5.9"));
        // `lastUpdated` must not be mistaken for a version.
        assert!(!metadata.versions.iter().any(|v| v.starts_with("2026")));
    }

    #[test]
    fn parses_prerelease_versions() {
        let xml = RELEASE_XML
            .replace("0.5.9</latest>", "0.6.0-pre.13</latest>")
            .replace("0.5.9</release>", "0.6.0-pre.13</release>")
            .replace("<version>0.5.5</version>", "<version>0.5.0-pre.9.2</version>");
        let metadata = parse(&xml).unwrap();
        assert_eq!(metadata.current(), Some("0.6.0-pre.13"));
        assert!(metadata.contains("0.5.0-pre.9.2"));
    }

    #[test]
    fn urls_follow_the_maven_layout() {
        assert_eq!(
            jar_url("release", "0.5.9"),
            "https://maven.hytale.com/release/com/hypixel/hytale/Server/0.5.9/Server-0.5.9.jar"
        );
        assert_eq!(
            metadata_url("pre-release"),
            "https://maven.hytale.com/pre-release/com/hypixel/hytale/Server/maven-metadata.xml"
        );
    }

    #[test]
    fn select_defaults_to_the_current_release() {
        let metadata = parse(RELEASE_XML).unwrap();
        assert_eq!(select(&metadata, "release", None).unwrap(), "0.5.9");
        assert_eq!(select(&metadata, "release", Some("0.5.7")).unwrap(), "0.5.7");
    }

    #[test]
    fn an_unpublished_version_lists_what_is_available() {
        let metadata = parse(RELEASE_XML).unwrap();
        // Only five versions are retained, so pinned old versions do disappear.
        let error = select(&metadata, "release", Some("0.5.1")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("0.5.1"), "{message}");
        assert!(message.contains("0.5.5, 0.5.6"), "{message}");
    }

    #[test]
    fn prereleases_order_below_their_release() {
        assert_eq!(compare("0.6.0-pre.13", "0.6.0"), std::cmp::Ordering::Less);
        assert_eq!(compare("0.5.9", "0.5.8"), std::cmp::Ordering::Greater);
        assert_eq!(
            compare("0.6.0-pre.12.2", "0.6.0-pre.13"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn an_unrecognised_version_format_yields_no_opinion() {
        // The manual's stale example used a date; a format change must not invert order.
        assert_eq!(compare("2026.01.22-6f8bd", "0.5.9"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn metadata_without_versions_is_reported() {
        let metadata = parse("<metadata><versioning/></metadata>").unwrap();
        assert!(matches!(
            select(&metadata, "release", None),
            Err(Error::NoVersions(_))
        ));
    }
}
