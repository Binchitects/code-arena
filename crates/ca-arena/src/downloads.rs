//! Code Arena's own builds, as the Arena serves them (Connect your tools): Arena serves the builds of its own
//! version, so a newer one there is the one to run.
//!
//! `GET /api/downloads/code-arena` answers `{version, builds: [{rid, size, sha256, ide?: {size, sha256}}],
//! signature?: {…}}`; a build is at `…/code-arena/{rid}` (its IDE launcher, on Windows, at `…/{rid}/ide`). When
//! Arena signs its builds, `signature` is there and beside them are `SHA256SUMS` (every build's SHA-256, as
//! `sha256sum` writes it, `{sum}  {rid}/{file}`), `SHA256SUMS.sig` (its RSA PKCS#1 v1.5 SHA-256 signature, raw
//! bytes) and `code-signing.crt` (the code-signing certificate that made it, PEM, its chain after it).

use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

use crate::client::{ArenaClient, ArenaError};

/// Where the builds are, under the Arena's address.
pub const ROOT: &str = "/api/downloads/code-arena";

/// What the Arena serves.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Builds {
    pub version: String,
    pub builds: Vec<Build>,
    /// There when Arena signs its builds.
    pub signature: Option<Value>,
}

/// One system's build.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Build {
    /// The system, as the builds are named (linux-x64, linux-x64-static, osx-arm64, win-x64…).
    pub rid: String,
    pub size: u64,
    pub sha256: Option<String>,
    /// The IDE's launcher beside the program (Windows).
    pub ide: Option<Beside>,
}

/// A file beside the build: its size and SHA-256.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Beside {
    pub size: u64,
    pub sha256: Option<String>,
}

impl Builds {
    /// The build for one system.
    pub fn of(&self, rid: &str) -> Option<&Build> {
        self.builds.iter().find(|b| b.rid == rid)
    }

    pub fn signed(&self) -> bool {
        self.signature.as_ref().is_some_and(Value::is_object)
    }
}

impl ArenaClient {
    /// `GET /api/downloads/code-arena`: the version Arena serves and its builds.
    pub async fn builds(&self) -> Result<Builds, ArenaError> {
        self.get(ROOT).await
    }

    /// A file of the builds' (`linux-x64`, `SHA256SUMS`, …) as it arrives: `chunk` is given each piece, and the
    /// answer's length when it says one, and says false to stop (then `Ok(false)`). No time limit: a build is large.
    pub async fn download(
        &self,
        name: &str,
        mut chunk: impl FnMut(&[u8], Option<u64>) -> bool,
    ) -> Result<bool, ArenaError> {
        let net = |e: reqwest::Error| ArenaError::Net(ca_net::explain(&e));
        let mut res = self.untimed(Method::GET, &format!("{ROOT}/{name}")).send().await.map_err(net)?;
        let status = res.status();
        if !status.is_success() {
            return Err(ArenaError::Status {
                status: status.as_u16(),
                code: None,
                message: format!("Arena did not give {name}"),
            });
        }
        let length = res.content_length();
        while let Some(piece) = res.chunk().await.map_err(net)? {
            if !chunk(&piece, length) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A small file of the builds' whole (`SHA256SUMS`, its signature, the certificate).
    pub async fn download_bytes(&self, name: &str) -> Result<Vec<u8>, ArenaError> {
        let mut all = Vec::new();
        self.download(name, |piece, _| {
            all.extend_from_slice(piece);
            true
        })
        .await?;
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_builds_are_read_as_arena_says_them() {
        let b: Builds = serde_json::from_value(json!({
            "version": "9.1.0",
            "builds": [{"rid": "linux-x64", "size": 12, "sha256": "ab"}, {"rid": "win-x64", "size": 3, "ide": {"size": 2}}],
            "signature": {"by": "Arena"}
        }))
        .expect("builds");
        assert_eq!(b.of("linux-x64").map(|x| x.size), Some(12));
        assert_eq!(b.of("win-x64").and_then(|x| x.ide.as_ref()).map(|i| i.size), Some(2));
        assert!(b.of("osx-arm64").is_none());
        assert!(b.signed());
        assert!(!Builds::default().signed());
    }
}
