//! `hytale.toml` — per-instance settings.
//!
//! Writing goes through [`Document`] rather than re-serialising [`Config`]: `[server]
//! version` is rewritten on every update, which would otherwise discard the operator's
//! comments and formatting each time.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

pub const CONFIG_FILE: &str = "hytale.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub java: JavaSection,
    #[serde(default)]
    pub backup: BackupSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    #[serde(default)]
    pub patchline: Patchline,

    /// Installed version stamp, e.g. `0.5.9`.
    #[serde(default)]
    pub version: Option<String>,

    /// e.g. `0.0.0.0:5520` — QUIC over UDP, not TCP.
    #[serde(default)]
    pub bind: Option<String>,

    #[serde(default)]
    pub hot_backup: HotBackup,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JavaSection {
    /// A requirement, e.g. `>=25`. The resolved pin lives in `.java-version`.
    #[serde(default)]
    pub version: Option<String>,

    #[serde(default)]
    pub options: Vec<String>,

    #[serde(default = "yes")]
    pub aot: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSection {
    #[serde(default = "default_keep")]
    pub keep: usize,

    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotBackup {
    #[serde(default = "yes")]
    pub enabled: bool,

    /// Minutes.
    #[serde(default = "default_frequency")]
    pub frequency: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Patchline {
    #[default]
    Release,
    PreRelease,
}

impl Patchline {
    pub fn as_str(self) -> &'static str {
        match self {
            Patchline::Release => "release",
            Patchline::PreRelease => "pre-release",
        }
    }
}

impl std::fmt::Display for Patchline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn yes() -> bool {
    true
}

fn default_keep() -> usize {
    10
}

fn default_frequency() -> u32 {
    30
}

/// Re-downloadable or regenerated, so not worth snapshotting.
fn default_exclude() -> Vec<String> {
    ["Assets.zip", ".cache", "logs"]
        .map(str::to_string)
        .to_vec()
}

impl Default for JavaSection {
    fn default() -> Self {
        Self {
            version: None,
            options: Vec::new(),
            aot: true,
        }
    }
}

impl Default for BackupSection {
    fn default() -> Self {
        Self {
            keep: default_keep(),
            exclude: default_exclude(),
        }
    }
}

impl Default for HotBackup {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: default_frequency(),
        }
    }
}

impl Config {
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Ok(None);
        };
        let config = toml::from_str(&contents).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Some(config))
    }
}

/// A `hytale.toml` opened for editing, preserving comments and formatting.
pub struct Document {
    path: PathBuf,
    document: toml_edit::DocumentMut,
}

impl Document {
    /// Opens an empty document if the file is absent.
    pub fn open(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).unwrap_or_default();
        let document = contents.parse().map_err(|source| Error::Edit {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            document,
        })
    }

    pub fn from_template(path: &Path, template: &str) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            document: template.parse().map_err(|source| Error::Edit {
                path: path.to_path_buf(),
                source,
            })?,
        })
    }

    pub fn set_server_version(&mut self, version: &str) {
        self.document["server"]["version"] = toml_edit::value(version);
    }

    pub fn set_java_version(&mut self, requirement: &str) {
        self.document["java"]["version"] = toml_edit::value(requirement);
    }

    pub fn set_java_options(&mut self, options: &[String]) {
        let mut array = toml_edit::Array::new();
        for option in options {
            array.push(option.as_str());
        }
        self.document["java"]["options"] = toml_edit::value(array);
    }

    /// Atomic: a crash mid-write would leave the instance unable to parse its own
    /// settings. The temporary file is a sibling so the rename stays on one filesystem.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let temporary = self.path.with_file_name(format!(".{name}.tmp"));

        std::fs::write(&temporary, self.document.to_string())?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

/// Defaults are written as comments rather than values, so an instance created today does
/// not freeze today's defaults.
pub fn template(java_requirement: &str) -> String {
    format!(
        "\
# Hytale server instance settings.
# Values shown commented out are the defaults.

[server]
# patchline = \"release\"        # or \"pre-release\"
# version   = \"0.0.0\"          # installed version; hy maintains this
# bind      = \"0.0.0.0:5520\"   # QUIC over UDP

[java]
version = \"{java_requirement}\"           # requirement; the resolved pin goes in .java-version
options = []               # JVM arguments, e.g. [\"-Xms2G\", \"-Xmx4G\"]
# aot   = true             # use HytaleServer.aot

[backup]
# keep    = 10
# exclude = [\"Assets.zip\", \".cache\", \"logs\"]

# Periodic backups, performed by the server itself.
[server.hot_backup]
# enabled   = true
# frequency = 30           # minutes
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_an_empty_file() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.server.patchline, Patchline::Release);
        assert!(config.java.aot);
        assert_eq!(config.backup.keep, 10);
        assert!(config.server.hot_backup.enabled);
        assert_eq!(config.server.hot_backup.frequency, 30);
    }

    #[test]
    fn parses_the_documented_shape() {
        let config: Config = toml::from_str(
            r#"
            [server]
            patchline = "pre-release"
            version = "0.6.0-pre.13"
            bind = "0.0.0.0:5520"

            [java]
            version = ">=25"
            options = ["-Xms2G", "-Xmx4G"]
            aot = false

            [backup]
            keep = 5

            [server.hot_backup]
            frequency = 15
            "#,
        )
        .unwrap();

        assert_eq!(config.server.patchline, Patchline::PreRelease);
        assert_eq!(config.server.version.as_deref(), Some("0.6.0-pre.13"));
        assert_eq!(config.java.options, ["-Xms2G", "-Xmx4G"]);
        assert!(!config.java.aot);
        assert_eq!(config.backup.keep, 5);
        assert_eq!(config.server.hot_backup.frequency, 15);
        // Untouched keys keep their defaults rather than resetting to empty.
        assert!(config.server.hot_backup.enabled);
        assert_eq!(config.backup.exclude, default_exclude());
    }

    #[test]
    fn a_typo_is_an_error_not_a_silent_no_op() {
        let error = toml::from_str::<Config>("[backup]\nkeeps = 5\n").unwrap_err();
        assert!(error.to_string().contains("keeps"), "{error}");
    }

    #[test]
    fn the_template_parses_and_round_trips() {
        let config: Config = toml::from_str(&template(">=25")).unwrap();
        assert_eq!(config.java.version.as_deref(), Some(">=25"));
        // Commented-out keys still yield the documented defaults.
        assert!(config.java.aot);
        assert_eq!(config.backup.keep, 10);
    }

    #[test]
    fn editing_preserves_comments_and_untouched_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(
            &path,
            "# operator's note: 8G because of the modpack\n\
             [java]\n\
             options = [\"-Xmx8G\"]\n\
             \n\
             [server]\n\
             patchline = \"pre-release\"\n",
        )
        .unwrap();

        let mut document = Document::open(&path).unwrap();
        document.set_server_version("0.5.9");
        document.save().unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("# operator's note: 8G because of the modpack"),
            "the comment should survive a version stamp:\n{contents}"
        );

        let config = Config::read(&path).unwrap().unwrap();
        assert_eq!(config.server.version.as_deref(), Some("0.5.9"));
        assert_eq!(config.java.options, ["-Xmx8G"]);
        assert_eq!(config.server.patchline, Patchline::PreRelease);
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let mut document = Document::from_template(&path, &template(">=25")).unwrap();
        document.set_java_options(&["-Xmx4G".to_string()]);
        document.save().unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, [CONFIG_FILE]);
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Config::read(&dir.path().join(CONFIG_FILE))
                .unwrap()
                .is_none()
        );
    }
}
