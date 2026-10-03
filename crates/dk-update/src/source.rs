//! HTTPS client and update sources (UPD-001, UPD-010).

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;

use crate::UpdateError;
use crate::manifest::PlatformAsset;
use crate::verify::write_stream_verified;

/// Manifests and signatures are tiny; refuse anything larger.
const MAX_METADATA_BYTES: usize = 1024 * 1024;

/// Hosts a release download may be served from: GitHub and its release-asset CDN (not
/// `raw.githubusercontent.com`, which serves user content).
fn allowed_host(host: &str) -> bool {
    matches!(
        host,
        "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
    )
}

/// The repository whose releases the updater trusts (UPD-001).
pub const REPO: &str = "pavel-purma/dockering";

/// An asset URL must be one of *this* repo's release downloads for exactly `version`, with a
/// plain file name: `https://github.com/<REPO>/releases/download/v<version>/<file>`. Returns the
/// file name. A signed manifest can't redirect the updater to another repo's release.
pub fn release_asset_name(url: &str, version: &str) -> Result<String, UpdateError> {
    let prefix = format!("https://github.com/{REPO}/releases/download/v{version}/");
    let name = url
        .strip_prefix(&prefix)
        .filter(|n| {
            !n.is_empty()
                && !n.starts_with('.')
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .ok_or_else(|| UpdateError::Manifest(format!("unexpected asset URL {url}")))?;
    Ok(name.to_owned())
}

/// Where update metadata and installers come from.
pub trait UpdateSource: Send + Sync {
    /// `(manifest bytes, minisign signature text)`.
    fn fetch_manifest(&self) -> BoxFuture<'_, Result<(Vec<u8>, String), UpdateError>>;

    /// Downloads `asset` into `dir/<file_name>` and verifies its size and SHA-256.
    fn download<'a>(
        &'a self,
        asset: &'a PlatformAsset,
        dir: &'a Path,
        file_name: &'a str,
        progress: Box<dyn FnMut(u64, u64) + Send + 'a>,
    ) -> BoxFuture<'a, Result<PathBuf, UpdateError>>;
}

/// The latest GitHub Release, anonymous HTTPS only (UPD-001/010).
pub struct GithubSource {
    client: reqwest::Client,
    manifest_url: String,
    signature_url: String,
}

impl GithubSource {
    /// `app_version` goes into the `User-Agent` (UPD-010). No cookies, ids, or other headers.
    pub fn new(app_version: &str) -> Result<Self, UpdateError> {
        let user_agent = format!(
            "Dockering/{app_version} ({}; {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            let ok_host = attempt.url().host_str().is_some_and(allowed_host);
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" || !ok_host {
                attempt.error("redirect to an unexpected host")
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .https_only(true)
            .redirect(policy)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        Ok(Self {
            client,
            manifest_url: crate::MANIFEST_URL.to_owned(),
            signature_url: crate::SIGNATURE_URL.to_owned(),
        })
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, UpdateError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| UpdateError::Manifest(e.to_string()))?;
        if parsed.scheme() != "https" || !parsed.host_str().is_some_and(allowed_host) {
            return Err(UpdateError::Manifest(format!(
                "unexpected download URL {url}"
            )));
        }
        let response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|e| UpdateError::Network(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(UpdateError::Http(status.as_u16()));
        }
        Ok(response)
    }

    async fn get_small(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let mut stream = self.get(url).await?.bytes_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| UpdateError::Network(e.without_url().to_string()))?;
            if out.len() + chunk.len() > MAX_METADATA_BYTES {
                return Err(UpdateError::Manifest("response too large".into()));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

impl UpdateSource for GithubSource {
    fn fetch_manifest(&self) -> BoxFuture<'_, Result<(Vec<u8>, String), UpdateError>> {
        Box::pin(async move {
            let manifest = self.get_small(&self.manifest_url).await?;
            let signature = self.get_small(&self.signature_url).await?;
            let signature = String::from_utf8(signature).map_err(|_| UpdateError::Signature)?;
            Ok((manifest, signature))
        })
    }

    fn download<'a>(
        &'a self,
        asset: &'a PlatformAsset,
        dir: &'a Path,
        file_name: &'a str,
        progress: Box<dyn FnMut(u64, u64) + Send + 'a>,
    ) -> BoxFuture<'a, Result<PathBuf, UpdateError>> {
        Box::pin(async move {
            let response = self.get(&asset.url).await?;
            if response.content_length().is_some_and(|n| n != asset.size) {
                return Err(UpdateError::Integrity);
            }
            let stream = response
                .bytes_stream()
                .map(|r| r.map_err(|e| e.without_url().to_string()));
            write_stream_verified(stream, dir, file_name, asset.size, &asset.sha256, progress).await
        })
    }
}

/// In-memory source for tests; counts requests so callers can assert "no network".
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Serves fixed bytes.
    pub struct StaticSource {
        /// Manifest bytes and signature text, or an error to return.
        pub manifest: Mutex<Result<(Vec<u8>, String), UpdateError>>,
        /// The installer bytes served for any asset.
        pub file: Vec<u8>,
        /// Number of `fetch_manifest` + `download` calls.
        pub requests: AtomicUsize,
    }

    impl StaticSource {
        /// A source serving `manifest`/`signature` and `file`.
        pub fn new(manifest: Vec<u8>, signature: String, file: Vec<u8>) -> Self {
            Self {
                manifest: Mutex::new(Ok((manifest, signature))),
                file,
                requests: AtomicUsize::new(0),
            }
        }

        /// Number of requests made so far.
        pub fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    impl UpdateSource for StaticSource {
        fn fetch_manifest(&self) -> BoxFuture<'_, Result<(Vec<u8>, String), UpdateError>> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let result = self
                .manifest
                .lock()
                .map(|m| m.clone())
                .unwrap_or_else(|_| Err(UpdateError::Network("poisoned".into())));
            Box::pin(async move { result })
        }

        fn download<'a>(
            &'a self,
            asset: &'a PlatformAsset,
            dir: &'a Path,
            file_name: &'a str,
            progress: Box<dyn FnMut(u64, u64) + Send + 'a>,
        ) -> BoxFuture<'a, Result<PathBuf, UpdateError>> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let chunks = futures::stream::iter(
                    self.file
                        .chunks(4096)
                        .map(|c| Ok::<_, String>(c.to_vec()))
                        .collect::<Vec<_>>(),
                );
                write_stream_verified(chunks, dir, file_name, asset.size, &asset.sha256, progress)
                    .await
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upd_010_only_github_hosts() {
        assert!(allowed_host("github.com"));
        assert!(allowed_host("objects.githubusercontent.com"));
        assert!(allowed_host("release-assets.githubusercontent.com"));
        assert!(!allowed_host("raw.githubusercontent.com"));
        assert!(!allowed_host("evilgithub.com"));
        assert!(!allowed_host("github.com.evil.example"));
    }

    #[test]
    fn upd_001_asset_url_pinned_to_repo_and_version() {
        let ok = "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-x64.exe";
        assert_eq!(
            release_asset_name(ok, "0.2.0"),
            Ok("Dockering-Setup-x64.exe".to_owned())
        );
        for bad in [
            "https://github.com/attacker/x/releases/download/v0.2.0/Dockering-Setup-x64.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v0.1.0/Dockering-Setup-x64.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/../x.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/a/b.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/",
            "http://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-x64.exe",
        ] {
            assert!(release_asset_name(bad, "0.2.0").is_err(), "{bad}");
        }
    }
}
