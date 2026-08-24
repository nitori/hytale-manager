//! Hytale server instances: the `game/` directory and its settings.
//!
//! An [`Instance`] pairs a [`Layout`] with a [`Config`]. Commands go through it rather
//! than joining paths themselves, because the layout is load-bearing: the server's updater
//! switches itself off if the shape is wrong.
//!
//! ```no_run
//! # fn example() -> Result<(), hy_instance::Error> {
//! let instance = hy_instance::Instance::discover(&std::env::current_dir()?)?;
//! println!("{}", instance.layout().jar().display());
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod error;
pub mod jvm_options;
pub mod layout;

use std::path::{Path, PathBuf};

pub use config::{CONFIG_FILE, Config, Document, Patchline};
pub use error::{Error, Result};
pub use layout::{Finding, Layout};

#[derive(Debug, Clone)]
pub struct Instance {
    layout: Layout,
    config: Config,
}

impl Instance {
    /// Searches upwards from `start`.
    pub fn discover(start: &Path) -> Result<Self> {
        let layout = layout::discover(start).ok_or_else(|| Error::NotFound(start.to_path_buf()))?;
        Self::load(layout)
    }

    pub fn at(root: &Path) -> Result<Self> {
        let layout = Layout::new(root);
        if !layout.is_initialised() && !layout.is_server_install() {
            return Err(Error::NotFound(root.to_path_buf()));
        }
        Self::load(layout)
    }

    /// An uninitialised instance still has a usable layout, so `hy status` can describe an
    /// install before `hy init` runs.
    fn load(layout: Layout) -> Result<Self> {
        let config = Config::read(&layout.config())?.unwrap_or_default();
        Ok(Self { layout, config })
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn root(&self) -> &Path {
        self.layout.root()
    }

    pub fn document(&self) -> Result<Document> {
        Document::open(&self.layout.config())
    }

    /// Returned unparsed: version requests belong to `hy-java`, and this crate stays about
    /// the filesystem.
    pub fn java_requirement(&self) -> Option<(&str, PathBuf)> {
        let version = self.config.java.version.as_deref()?;
        Some((version, self.layout.config()))
    }

    /// A `jvm.options` edited after adoption looks effective but is inert; reporting it
    /// beats letting the operator believe a setting applied.
    pub fn stray_jvm_options(&self) -> Result<Option<Vec<String>>> {
        let Some(options) = jvm_options::read(&self.layout.jvm_options())? else {
            return Ok(None);
        };
        if options.is_empty() || options == self.config.java.options {
            return Ok(None);
        }
        Ok(Some(options))
    }
}

pub struct Initialised {
    pub instance: Instance,
    /// An existing server install was adopted, rather than a bare directory set up.
    pub adopted: bool,
    pub imported_options: Vec<String>,
}

/// Imports any `jvm.options`; without that, switching from `start.sh` to `hy run` would
/// silently drop the operator's memory settings. The only time that file is read.
pub fn init(root: &Path, java_requirement: &str) -> Result<Initialised> {
    let layout = Layout::new(root);

    if layout.is_initialised() {
        return Err(Error::AlreadyInitialised(root.to_path_buf()));
    }

    std::fs::create_dir_all(root)?;

    let adopted = layout.is_server_install();
    let imported_options = jvm_options::read(&layout.jvm_options())?.unwrap_or_default();

    let mut document =
        Document::from_template(&layout.config(), &config::template(java_requirement))?;
    if !imported_options.is_empty() {
        document.set_java_options(&imported_options);
    }
    document.save()?;

    Ok(Initialised {
        instance: Instance::load(layout)?,
        adopted,
        imported_options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_install(root: &Path) {
        std::fs::create_dir_all(root.join("Server")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"").unwrap();
        std::fs::write(root.join("start.sh"), b"").unwrap();
    }

    #[test]
    fn init_writes_a_config_with_the_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let result = init(dir.path(), ">=25").unwrap();

        assert!(!result.adopted);
        assert!(result.imported_options.is_empty());
        assert_eq!(
            result.instance.config().java.version.as_deref(),
            Some(">=25")
        );
        assert!(dir.path().join(CONFIG_FILE).is_file());
    }

    #[test]
    fn init_adopts_an_existing_install_and_imports_jvm_options() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        std::fs::write(
            dir.path().join("jvm.options"),
            "# modpack needs headroom\n-Xms4G\n-Xmx8G\n",
        )
        .unwrap();

        let result = init(dir.path(), ">=25").unwrap();

        assert!(result.adopted, "an existing install should be adopted");
        assert_eq!(result.imported_options, ["-Xms4G", "-Xmx8G"]);
        assert_eq!(result.instance.config().java.options, ["-Xms4G", "-Xmx8G"]);
    }

    #[test]
    fn init_never_writes_jvm_options() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), ">=25").unwrap();
        assert!(
            !dir.path().join("jvm.options").exists(),
            "hy owns [java] options; it must not create an argfile"
        );
    }

    #[test]
    fn init_leaves_an_imported_jvm_options_in_place() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        std::fs::write(dir.path().join("jvm.options"), "-Xmx8G\n").unwrap();

        init(dir.path(), ">=25").unwrap();

        // Importing is not migrating: the file is the server launcher's, not ours.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("jvm.options")).unwrap(),
            "-Xmx8G\n"
        );
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), ">=25").unwrap();
        assert!(matches!(
            init(dir.path(), "26"),
            Err(Error::AlreadyInitialised(_))
        ));
    }

    #[test]
    fn a_commented_out_example_is_not_imported() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        std::fs::write(dir.path().join("jvm.options"), "# -Xms2G\n# -Xmx4G\n").unwrap();

        let result = init(dir.path(), ">=25").unwrap();
        assert!(result.imported_options.is_empty());
        assert!(result.instance.config().java.options.is_empty());
    }

    #[test]
    fn stray_jvm_options_are_reported_only_when_they_differ() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        std::fs::write(dir.path().join("jvm.options"), "-Xmx8G\n").unwrap();

        // Imported at init, so the file agrees with the config and is not stray.
        init(dir.path(), ">=25").unwrap();
        let instance = Instance::at(dir.path()).unwrap();
        assert_eq!(instance.stray_jvm_options().unwrap(), None);

        // Edited by hand afterwards, where it no longer has any effect.
        std::fs::write(dir.path().join("jvm.options"), "-Xmx16G\n").unwrap();
        let instance = Instance::at(dir.path()).unwrap();
        assert_eq!(
            instance.stray_jvm_options().unwrap(),
            Some(vec!["-Xmx16G".to_string()])
        );
    }

    #[test]
    fn discovery_reads_the_config_from_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        server_install(&root);
        init(&root, ">=25").unwrap();

        let instance = Instance::discover(&root.join("Server")).unwrap();
        assert_eq!(instance.root(), root);
        assert_eq!(instance.java_requirement().unwrap().0, ">=25");
    }

    #[test]
    fn an_unrelated_directory_is_not_an_instance() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(Instance::at(dir.path()), Err(Error::NotFound(_))));
    }
}
