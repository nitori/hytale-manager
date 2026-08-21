//! JDK distributions.
//!
//! Only Temurin is implemented. The enum exists so that adding Corretto, Zulu, or Graal
//! later is a variant plus a fetch implementation, rather than a refactor of every call
//! site that names a JDK.
//!
//! Note that "Adoptium" and "Temurin" are the same thing: Eclipse Adoptium is the project,
//! Temurin is the JDK it builds. The Hytale manual's "we recommend Adoptium" means Temurin.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum JavaDistribution {
    #[default]
    Temurin,
}

impl JavaDistribution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Temurin => "temurin",
        }
    }

    /// The `vendor=` value the Adoptium API expects.
    pub fn vendor(self) -> &'static str {
        match self {
            Self::Temurin => "eclipse",
        }
    }
}

impl FromStr for JavaDistribution {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            // Accept "adoptium" as an alias, since the Hytale manual uses that name.
            "temurin" | "adoptium" | "eclipse" => Ok(Self::Temurin),
            _ => Err(()),
        }
    }
}

impl fmt::Display for JavaDistribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
