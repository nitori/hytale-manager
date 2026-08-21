//! Directory keys for managed installs, e.g. `temurin-25.0.4.1+1-linux-x86_64`.

use std::fmt;
use std::str::FromStr;

use crate::distribution::JavaDistribution;
use crate::platform::{Arch, Os};
use crate::version::JavaVersion;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstallKey {
    pub distribution: JavaDistribution,
    pub version: JavaVersion,
    pub os: Os,
    pub arch: Arch,
}

impl InstallKey {
    pub fn new(distribution: JavaDistribution, version: JavaVersion, os: Os, arch: Arch) -> Self {
        Self {
            distribution,
            version,
            os,
            arch,
        }
    }
}

impl fmt::Display for InstallKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}",
            self.distribution,
            self.version,
            self.os.as_str(),
            self.arch.as_str()
        )
    }
}

impl FromStr for InstallKey {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Exactly four fields; no field contains a '-'. Version may contain '+'.
        let parts: Vec<&str> = s.split('-').collect();
        let [distribution, version, os, arch] = parts.as_slice() else {
            return Err(());
        };
        Ok(Self {
            distribution: distribution.parse()?,
            version: version.parse().map_err(|_| ())?,
            os: os.parse()?,
            arch: arch.parse()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let key = "temurin-25.0.4.1+1-linux-x86_64";
        let parsed: InstallKey = key.parse().unwrap();
        assert_eq!(parsed.version.major(), 25);
        assert_eq!(parsed.os, Os::Linux);
        assert_eq!(parsed.arch, Arch::X86_64);
        assert_eq!(parsed.to_string(), key);
    }

    #[test]
    fn rejects_malformed() {
        assert!("temurin-25.0.4.1+1-linux".parse::<InstallKey>().is_err());
        assert!("nonsense".parse::<InstallKey>().is_err());
    }
}
