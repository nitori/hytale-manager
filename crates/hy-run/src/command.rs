//! Building the JVM command line.
//!
//! Mirrors what `start.sh` does, with the argfile replaced by `[java] options`:
//!
//! ```text
//! cd Server
//! java <options> -jar HytaleServer.jar --assets ../Assets.zip --backup ... "$@"
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use hy_instance::Instance;

use crate::error::{Error, Result};
use crate::shell::Shell;

/// `--assets` is relative because the server resolves it against `Server/`.
const ASSETS_ARG: &str = "../Assets.zip";
const AOT_FLAG: &str = "-XX:AOTCache=";

#[derive(Debug, Clone)]
pub struct ServerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub working_dir: PathBuf,
}

impl ServerCommand {
    /// The command line as `shell` would take it, for showing the operator what ran.
    /// Quoted for reading and pasting, not for re-parsing.
    pub fn display(&self, shell: Shell) -> String {
        std::iter::once(shell.quoted_path(&self.program))
            .chain(
                self.args
                    .iter()
                    .map(|a| shell.quote(&a.to_string_lossy())),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `extra` is passed through verbatim, after ours, so an operator can override a default.
pub fn build(instance: &Instance, java: &Path, extra: &[String]) -> Result<ServerCommand> {
    let layout = instance.layout();
    let config = instance.config();

    if !layout.jar().is_file() {
        return Err(Error::MissingJar(layout.server_dir()));
    }
    if !layout.assets().is_file() {
        return Err(Error::MissingAssets(layout.root().to_path_buf()));
    }

    let mut args: Vec<OsString> = Vec::new();

    if let Some(flag) = config
        .java
        .options
        .iter()
        .find(|o| o.starts_with(AOT_FLAG) || o.as_str() == "-XX:+AOTClassLinking")
        && config.java.aot
    {
        return Err(Error::AotConflict { flag: flag.clone() });
    }
    args.extend(config.java.options.iter().map(OsString::from));

    // The cache is version-stamped against the JVM that wrote it; passing a path that does
    // not exist makes the JVM fail rather than fall back.
    if config.java.aot && layout.aot_cache().is_file() {
        args.push(OsString::from(format!("{AOT_FLAG}HytaleServer.aot")));
    }

    args.push(OsString::from("-jar"));
    args.push(OsString::from("HytaleServer.jar"));
    args.push(OsString::from("--assets"));
    args.push(OsString::from(ASSETS_ARG));

    if let Some(bind) = &config.server.bind {
        args.push(OsString::from("--bind"));
        args.push(OsString::from(bind));
    }

    let hot = &config.server.hot_backup;
    if hot.enabled {
        args.push(OsString::from("--backup"));
        args.push(OsString::from("--backup-dir"));
        args.push(OsString::from("backups"));
        args.push(OsString::from("--backup-frequency"));
        args.push(OsString::from(hot.frequency.to_string()));
    }

    args.extend(extra.iter().map(OsString::from));

    Ok(ServerCommand {
        program: java.to_path_buf(),
        args,
        working_dir: layout.server_dir(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance_with(config: &str) -> (tempfile::TempDir, Instance) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Server")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"").unwrap();
        std::fs::write(root.join("start.sh"), b"").unwrap();
        std::fs::write(root.join("hytale.toml"), config).unwrap();
        let instance = Instance::at(root).unwrap();
        (dir, instance)
    }

    fn strings(command: &ServerCommand) -> Vec<String> {
        command
            .args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn matches_the_wrapper_defaults() {
        let (_dir, instance) = instance_with("");
        let command = build(&instance, Path::new("/jdk/bin/java"), &[]).unwrap();

        assert_eq!(
            strings(&command),
            [
                "-jar",
                "HytaleServer.jar",
                "--assets",
                "../Assets.zip",
                "--backup",
                "--backup-dir",
                "backups",
                "--backup-frequency",
                "30",
            ]
        );
        assert!(command.working_dir.ends_with("Server"));
    }

    #[test]
    fn jvm_options_precede_the_jar() {
        let (_dir, instance) = instance_with("[java]\noptions = [\"-Xms2G\", \"-Xmx4G\"]\n");
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());

        let jar = args.iter().position(|a| a == "-jar").unwrap();
        assert_eq!(&args[..jar], ["-Xms2G", "-Xmx4G"]);
    }

    #[test]
    fn aot_is_used_only_when_the_cache_exists() {
        let (dir, instance) = instance_with("");
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(!args.iter().any(|a| a.starts_with(AOT_FLAG)));

        std::fs::write(dir.path().join("Server/HytaleServer.aot"), b"").unwrap();
        let instance = Instance::at(dir.path()).unwrap();
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(args.contains(&"-XX:AOTCache=HytaleServer.aot".to_string()));
    }

    #[test]
    fn aot_can_be_disabled() {
        let (dir, instance) = instance_with("[java]\naot = false\n");
        std::fs::write(dir.path().join("Server/HytaleServer.aot"), b"").unwrap();
        let instance = Instance::at(instance.root()).unwrap();
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(!args.iter().any(|a| a.starts_with(AOT_FLAG)));
        let _ = dir;
    }

    #[test]
    fn a_hand_written_aot_flag_conflicts() {
        let (_dir, instance) = instance_with("[java]\noptions = [\"-XX:AOTCache=other.aot\"]\n");
        assert!(matches!(
            build(&instance, Path::new("java"), &[]),
            Err(Error::AotConflict { .. })
        ));
    }

    #[test]
    fn a_hand_written_aot_flag_is_allowed_when_aot_is_off() {
        let (_dir, instance) =
            instance_with("[java]\naot = false\noptions = [\"-XX:AOTCache=other.aot\"]\n");
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(args.contains(&"-XX:AOTCache=other.aot".to_string()));
    }

    #[test]
    fn bind_and_hot_backup_come_from_config() {
        let (_dir, instance) = instance_with(
            "[server]\nbind = \"0.0.0.0:3500\"\n\n[server.hot_backup]\nfrequency = 15\n",
        );
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(args.windows(2).any(|w| w == ["--bind", "0.0.0.0:3500"]));
        assert!(args.windows(2).any(|w| w == ["--backup-frequency", "15"]));
    }

    #[test]
    fn hot_backup_can_be_disabled() {
        let (_dir, instance) = instance_with("[server.hot_backup]\nenabled = false\n");
        let args = strings(&build(&instance, Path::new("java"), &[]).unwrap());
        assert!(!args.iter().any(|a| a.starts_with("--backup")));
    }

    #[test]
    fn passthrough_args_come_last_so_they_win() {
        let (_dir, instance) = instance_with("");
        let extra = [
            "--disable-sentry".to_string(),
            "--bind".to_string(),
            "1:2".to_string(),
        ];
        let args = strings(&build(&instance, Path::new("java"), &extra).unwrap());
        assert_eq!(
            &args[args.len() - 3..],
            ["--disable-sentry", "--bind", "1:2"]
        );
    }

    #[test]
    fn display_renders_a_pasteable_command_line() {
        let (_dir, instance) = instance_with("[java]\noptions = [\"-Xmx4G\"]\n");
        let line = build(&instance, Path::new("/opt/jdk/bin/java"), &[])
            .unwrap()
            .display(Shell::Posix);

        assert!(
            line.starts_with("/opt/jdk/bin/java -Xmx4G -jar HytaleServer.jar"),
            "{line}"
        );
        assert!(line.contains("--assets ../Assets.zip"), "{line}");
    }

    #[test]
    fn display_follows_the_shell_for_the_java_path() {
        let (_dir, instance) = instance_with("");
        let command = build(&instance, Path::new(r"C:\Program Files\jdk\bin\java.exe"), &[])
            .unwrap();

        assert!(
            command.display(Shell::Msys).starts_with("'/c/Program Files/jdk/bin/java.exe'"),
            "{}",
            command.display(Shell::Msys)
        );
        assert!(
            command
                .display(Shell::WindowsNative)
                .starts_with("\"C:\\Program Files\\jdk\\bin\\java.exe\""),
            "{}",
            command.display(Shell::WindowsNative)
        );
    }

    #[test]
    fn a_missing_jar_is_reported_before_spawning() {
        let (dir, _instance) = instance_with("");
        std::fs::remove_file(dir.path().join("Server/HytaleServer.jar")).unwrap();
        let instance = Instance::at(dir.path()).unwrap();
        assert!(matches!(
            build(&instance, Path::new("java"), &[]),
            Err(Error::MissingJar(_))
        ));
    }
}
