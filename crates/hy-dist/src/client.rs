use crate::error::{Error, Result};
use crate::maven::{self, Metadata};

pub const RELEASE: &str = "release";
pub const PRE_RELEASE: &str = "pre-release";

/// Reject anything else before it reaches a URL.
pub fn validate_patchline(patchline: &str) -> Result<&str> {
    match patchline {
        RELEASE | PRE_RELEASE => Ok(patchline),
        other => Err(Error::UnknownPatchline(other.to_string())),
    }
}

pub struct DistClient {
    http: reqwest::Client,
}

impl DistClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("hy/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    pub async fn metadata(&self, patchline: &str) -> Result<Metadata> {
        validate_patchline(patchline)?;
        let xml = self
            .http
            .get(maven::metadata_url(patchline))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        maven::parse(&xml).map_err(|source| Error::Metadata {
            patchline: patchline.to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_known_patchlines_are_accepted() {
        assert!(validate_patchline("release").is_ok());
        assert!(validate_patchline("pre-release").is_ok());
        // A stray value would otherwise be interpolated straight into a URL.
        assert!(matches!(
            validate_patchline("../../etc"),
            Err(Error::UnknownPatchline(_))
        ));
    }
}
