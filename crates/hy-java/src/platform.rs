//! Host OS and architecture, and the names the Adoptium API uses for them.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
    Arm,
    X86,
    Ppc64le,
    S390x,
    Riscv64,
}

impl Os {
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "linux" => Some(Self::Linux),
            "macos" => Some(Self::MacOs),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }

    /// The `os=` value the Adoptium API expects.
    pub fn adoptium(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "mac",
            Self::Windows => "windows",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Windows => "windows",
        }
    }

    /// Temurin ships zip archives for Windows and tarballs everywhere else.
    pub fn archive_kind(self) -> ArchiveKind {
        match self {
            Self::Windows => ArchiveKind::Zip,
            _ => ArchiveKind::TarGz,
        }
    }

    pub fn java_executable(self) -> &'static str {
        match self {
            Self::Windows => "java.exe",
            _ => "java",
        }
    }
}

impl Arch {
    pub fn current() -> Option<Self> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Self::X86_64),
            "aarch64" => Some(Self::Aarch64),
            "arm" => Some(Self::Arm),
            "x86" => Some(Self::X86),
            "powerpc64" => Some(Self::Ppc64le),
            "s390x" => Some(Self::S390x),
            "riscv64" => Some(Self::Riscv64),
            _ => None,
        }
    }

    /// The `architecture=` value the Adoptium API expects. Note this differs from the Rust
    /// target name: Adoptium calls x86_64 "x64".
    pub fn adoptium(self) -> &'static str {
        match self {
            Self::X86_64 => "x64",
            Self::Aarch64 => "aarch64",
            Self::Arm => "arm",
            Self::X86 => "x86",
            Self::Ppc64le => "ppc64le",
            Self::S390x => "s390x",
            Self::Riscv64 => "riscv64",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Arm => "arm",
            Self::X86 => "x86",
            Self::Ppc64le => "ppc64le",
            Self::S390x => "s390x",
            Self::Riscv64 => "riscv64",
        }
    }
}

impl std::str::FromStr for Os {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "linux" => Ok(Self::Linux),
            "macos" | "mac" => Ok(Self::MacOs),
            "windows" => Ok(Self::Windows),
            _ => Err(()),
        }
    }
}

impl std::str::FromStr for Arch {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x86_64" | "x64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            "arm" => Ok(Self::Arm),
            "x86" => Ok(Self::X86),
            "ppc64le" => Ok(Self::Ppc64le),
            "s390x" => Ok(Self::S390x),
            "riscv64" => Ok(Self::Riscv64),
            _ => Err(()),
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Zip,
}
