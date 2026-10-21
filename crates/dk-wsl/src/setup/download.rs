//! The one HTTPS GET of the pinned asset, streamed to disk and hashed on the way (ENG-152/160).
//!
//! HTTPS only, redirects only to GitHub's hosts over https, no cookies, no auth headers, the
//! system proxy, connect 15 s / stall 30 s timeouts (same discipline as `dk-update`, UPD-010).

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use reqwest::Url;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::pins::Pin;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// Hosts a release asset may be served from.
fn allowed_host(host: &str) -> bool {
    matches!(
        host,
        "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
    )
}

/// Production rule for the pinned URL and for every redirect hop.
pub(crate) fn url_allowed(url: &Url) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(allowed_host)
}

#[derive(Debug)]
pub(crate) enum FetchError {
    Network(String),
    Http(u16),
    /// The body is, or claims to be, larger than the pinned size.
    TooLarge,
    /// The pinned URL itself isn't an allowed one.
    BadUrl,
    Io(std::io::Error),
}

pub(crate) struct Fetched {
    pub(crate) sha256: String,
}

/// An HTTP client with the pinned-host policy.
#[derive(Debug, Clone)]
pub(crate) struct Source {
    client: reqwest::Client,
    /// Test seam: scheme, host and port of this URL replace the request's, after the pinned URL
    /// passed [`url_allowed`]. Always `None` in production.
    #[cfg(test)]
    base: Option<Url>,
}

fn user_agent() -> String {
    format!(
        "Dockering ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

impl Source {
    pub(crate) fn production() -> Result<Self, FetchError> {
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if !url_allowed(attempt.url()) {
                attempt.error("redirect to an unexpected host")
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .https_only(true)
            .redirect(policy)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(STALL_TIMEOUT)
            .build()
            .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
        Ok(Self {
            client,
            #[cfg(test)]
            base: None,
        })
    }

    /// Loopback test server: plain HTTP allowed, no proxy, redirects only within `base`.
    #[cfg(test)]
    pub(crate) fn with_base(base: &str) -> Result<Self, FetchError> {
        let base = Url::parse(base).map_err(|e| FetchError::Network(e.to_string()))?;
        let origin = base.clone();
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            let u = attempt.url();
            let same = u.scheme() == origin.scheme()
                && u.host_str() == origin.host_str()
                && u.port_or_known_default() == origin.port_or_known_default();
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if !same {
                attempt.error("redirect outside the test base URL")
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .redirect(policy)
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(STALL_TIMEOUT)
            .build()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        Ok(Self {
            client,
            base: Some(base),
        })
    }

    #[cfg(test)]
    pub(crate) fn client_for_test(&self) -> &reqwest::Client {
        &self.client
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, FetchError> {
        let parsed = Url::parse(url).map_err(|_| FetchError::BadUrl)?;
        if !url_allowed(&parsed) {
            return Err(FetchError::BadUrl);
        }
        #[cfg(test)]
        let parsed = match &self.base {
            Some(base) => {
                let mut rewritten = parsed;
                rewritten
                    .set_scheme(base.scheme())
                    .map_err(|()| FetchError::BadUrl)?;
                rewritten
                    .set_host(base.host_str())
                    .map_err(|_| FetchError::BadUrl)?;
                rewritten
                    .set_port(base.port())
                    .map_err(|()| FetchError::BadUrl)?;
                rewritten
            }
            None => parsed,
        };
        let response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(FetchError::Http(status.as_u16()))
        }
    }
}

/// Streams `pin.url` into `part` (never into memory), hashing as it goes. The body may not
/// exceed `pin.size_bytes`. `on_progress` gets the byte count after each chunk.
pub(crate) async fn fetch(
    source: &Source,
    pin: &Pin,
    part: &Path,
    mut on_progress: impl FnMut(u64),
) -> Result<Fetched, FetchError> {
    let response = source.get(pin.url).await?;
    if response
        .content_length()
        .is_some_and(|len| len > pin.size_bytes)
    {
        return Err(FetchError::TooLarge);
    }
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(FetchError::Io)?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| FetchError::Network(e.without_url().to_string()))?;
        received += chunk.len() as u64;
        if received > pin.size_bytes {
            return Err(FetchError::TooLarge);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(FetchError::Io)?;
        on_progress(received);
    }
    file.flush().await.map_err(FetchError::Io)?;
    Ok(Fetched {
        sha256: hex::encode(hasher.finalize()),
    })
}
