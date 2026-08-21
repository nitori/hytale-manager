//! Discovery of Java installations already present on the system.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::distribution::JavaDistribution;
use crate::error::{Error, Result};
use crate::platform::Os;
use crate::version::JavaVersion;

#[derive(Debug, Clone)]
pub struct SystemJava {
    pub home: PathBuf,
    pub executable: PathBuf,
    pub version: JavaVersion,
    /// The reported vendor, mapped to a known distribution when we recognise it.
    pub distribution: Option<JavaDistribution>,
    pub vendor: Option<String>,
}

/// Every distinct Java installation we can find, newest first.
pub fn discover(os: Os) -> Vec<SystemJava> {
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();

    for candidate in candidates(os) {
        let exe = executable_in(&candidate, os);
        if !exe.is_file() {
            continue;
        }
        match probe(&exe) {
            Ok(java) => {
                // Two PATH entries frequently symlink to the same JDK.
                let identity = std::fs::canonicalize(&java.home).unwrap_or(java.home.clone());
                if seen.insert(identity) {
                    found.push(java);
                }
            }
            Err(err) => tracing::debug!("ignoring {}: {err}", exe.display()),
        }
    }

    found.sort_by(|a, b| b.version.cmp(&a.version));
    found
}

/// Ask a JVM to describe itself.
///
/// `-XshowSettings:properties -version` prints the system properties to stderr and exits,
/// which gives us home, version, and vendor in a single spawn.
pub fn probe(executable: &Path) -> Result<SystemJava> {
    let output = Command::new(executable)
        .args(["-XshowSettings:properties", "-version"])
        .output()
        .map_err(|source| Error::Probe {
            path: executable.to_path_buf(),
            source,
        })?;

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let home = property(&text, "java.home")
        .map(PathBuf::from)
        .ok_or_else(|| Error::NotAJavaHome(executable.to_path_buf()))?;

    // `java.runtime.version` carries the build number (`25.0.4.1+1`); `java.version` does
    // not. Prefer the former and fall back.
    let version = property(&text, "java.runtime.version")
        .and_then(|v| v.parse::<JavaVersion>().ok())
        .or_else(|| property(&text, "java.version").and_then(|v| v.parse().ok()))
        .ok_or_else(|| Error::NotAJavaHome(executable.to_path_buf()))?;

    let vendor = property(&text, "java.vendor");
    let distribution = vendor.as_deref().and_then(recognise_vendor);

    Ok(SystemJava {
        executable: executable_in(&home, Os::current().unwrap_or(Os::Linux)),
        home,
        version,
        distribution,
        vendor,
    })
}

fn property(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn recognise_vendor(vendor: &str) -> Option<JavaDistribution> {
    let lower = vendor.to_ascii_lowercase();
    if lower.contains("temurin") || lower.contains("adoptium") || lower.contains("eclipse") {
        Some(JavaDistribution::Temurin)
    } else {
        None
    }
}

fn executable_in(home: &Path, os: Os) -> PathBuf {
    home.join("bin").join(os.java_executable())
}

/// Candidate JDK homes, in the order we prefer to discover them.
fn candidates(os: Os) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(java) = std::env::var_os("HY_JAVA") {
        roots.push(PathBuf::from(java));
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        roots.push(PathBuf::from(home));
    }

    // `java` on PATH: resolve the binary, then step up out of `bin/`.
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let exe = dir.join(os.java_executable());
            if exe.is_file() {
                let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
                if let Some(home) = resolved.parent().and_then(Path::parent) {
                    roots.push(home.to_path_buf());
                }
            }
        }
    }

    let system_dirs: &[&str] = match os {
        Os::Linux => &["/usr/lib/jvm", "/usr/java", "/opt/java"],
        Os::MacOs => &[
            "/Library/Java/JavaVirtualMachines",
            "/opt/homebrew/opt",
            "/usr/local/opt",
        ],
        Os::Windows => &[
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Java",
        ],
    };

    for dir in system_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // macOS wraps each JDK in a bundle.
            let nested = path.join("Contents").join("Home");
            roots.push(if nested.is_dir() { nested } else { path });
        }
    }

    roots
}
