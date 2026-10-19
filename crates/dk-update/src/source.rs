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

/// `Dockering/<version> (<os>; <arch>)`: the only identifying header (UPD-010).
fn user_agent(app_version: &str) -> String {
    format!(
        "Dockering/{app_version} ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// The latest GitHub Release, anonymous HTTPS only (UPD-001/010).
pub struct GithubSource {
    client: reqwest::Client,
    manifest_url: String,
    signature_url: String,
    /// Test seam (REL-086): scheme and host of this URL replace the request's, after the
    /// request URL has passed the pins. Always `None` for [`GithubSource::new`].
    #[cfg(any(test, feature = "test-support", feature = "update-e2e"))]
    base: Option<reqwest::Url>,
}

impl GithubSource {
    /// `app_version` goes into the `User-Agent` (UPD-010). No cookies, ids, or other headers.
    pub fn new(app_version: &str) -> Result<Self, UpdateError> {
        let user_agent = user_agent(app_version);
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
            #[cfg(any(test, feature = "test-support", feature = "update-e2e"))]
            base: None,
        })
    }

    /// Test seam (REL-086): like [`GithubSource::new`], but every fetch (manifest, signature and
    /// asset download) goes to `base` (for example `http://127.0.0.1:8080`) instead of GitHub.
    ///
    /// Only the scheme, host and port of the request URL are replaced, **after** the original
    /// URL has passed the same checks as in production (HTTPS, host allow-list), so a manifest
    /// that names another host is still rejected and the signed manifest keeps its GitHub URLs.
    /// The client may speak plain HTTP and follows up to 5 redirects that stay on `base`. It
    /// never uses a proxy, so a loopback server is reached directly.
    #[cfg(any(test, feature = "test-support", feature = "update-e2e"))]
    pub fn with_base(app_version: &str, base: &str) -> Result<Self, UpdateError> {
        let base = reqwest::Url::parse(base)
            .map_err(|e| UpdateError::Network(format!("invalid base URL: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(UpdateError::Network(
                "the base URL must be http(s)://host[:port]".into(),
            ));
        }
        let origin = base.clone();
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            let u = attempt.url();
            let same_origin = u.scheme() == origin.scheme()
                && u.host_str() == origin.host_str()
                && u.port_or_known_default() == origin.port_or_known_default();
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if !same_origin {
                attempt.error("redirect outside the test base URL")
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(user_agent(app_version))
            .redirect(policy)
            .no_proxy()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        Ok(Self {
            client,
            manifest_url: crate::MANIFEST_URL.to_owned(),
            signature_url: crate::SIGNATURE_URL.to_owned(),
            base: Some(base),
        })
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, UpdateError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| UpdateError::Manifest(e.to_string()))?;
        if parsed.scheme() != "https" || !parsed.host_str().is_some_and(allowed_host) {
            return Err(UpdateError::Manifest(format!(
                "unexpected download URL {url}"
            )));
        }
        #[cfg(any(test, feature = "test-support", feature = "update-e2e"))]
        let parsed = match &self.base {
            Some(base) => rewrite_origin(parsed, base)?,
            None => parsed,
        };
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

/// Replaces scheme, host and port of `url` with those of `base`; path and query stay.
#[cfg(any(test, feature = "test-support", feature = "update-e2e"))]
fn rewrite_origin(mut url: reqwest::Url, base: &reqwest::Url) -> Result<reqwest::Url, UpdateError> {
    let shown = url.to_string();
    let cannot =
        |what: &str| UpdateError::Network(format!("cannot point {shown} at {base}: {what}"));
    url.set_scheme(base.scheme())
        .map_err(|()| cannot("scheme"))?;
    url.set_host(base.host_str())
        .map_err(|e| cannot(&format!("host ({e})")))?;
    url.set_port(base.port()).map_err(|()| cannot("port"))?;
    Ok(url)
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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

    /// What a [`LoopbackServer`] answers for one path.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Reply {
        /// `200` with this body.
        Body(Vec<u8>),
        /// `302` to this `Location`.
        Redirect(String),
    }

    /// One request a [`LoopbackServer`] received.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SeenRequest {
        /// `GET`, …
        pub method: String,
        /// Request target (path and query).
        pub path: String,
        /// Header names (lower case) and values, in order.
        pub headers: Vec<(String, String)>,
    }

    impl SeenRequest {
        /// The value of header `name` (case-insensitive).
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    /// A tiny HTTP/1.1 responder on `127.0.0.1:<free port>` for tests of the real client (the
    /// REL-086 seam): fixed replies per path, `404` otherwise, one connection at a time, every
    /// request recorded. It runs on its own thread and needs no runtime (so it can't be
    /// starved by a paused tokio clock). Dropping it stops the thread.
    pub struct LoopbackServer {
        addr: std::net::SocketAddr,
        seen: Arc<Mutex<Vec<SeenRequest>>>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl LoopbackServer {
        /// Starts serving `routes` (request path → reply).
        pub fn start(routes: Vec<(String, Reply)>) -> std::io::Result<Self> {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            let addr = listener.local_addr()?;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let worker = {
                let (seen, stop) = (seen.clone(), stop.clone());
                std::thread::Builder::new()
                    .name("loopback-server".into())
                    .spawn(move || {
                        for stream in listener.incoming() {
                            if stop.load(Ordering::SeqCst) {
                                break;
                            }
                            if let Ok(mut stream) = stream {
                                let _ = answer(&mut stream, &routes, &seen);
                            }
                        }
                    })?
            };
            Ok(Self {
                addr,
                seen,
                stop,
                worker: Some(worker),
            })
        }

        /// `http://127.0.0.1:<port>`: what `GithubSource::with_base` takes.
        pub fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }

        /// Every request received so far, oldest first.
        pub fn requests(&self) -> Vec<SeenRequest> {
            self.seen.lock().map(|s| s.clone()).unwrap_or_default()
        }
    }

    impl Drop for LoopbackServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wake the blocking `accept`.
            let _ = std::net::TcpStream::connect(self.addr);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn answer(
        stream: &mut std::net::TcpStream,
        routes: &[(String, Reply)],
        seen: &Mutex<Vec<SeenRequest>>,
    ) -> std::io::Result<()> {
        use std::io::{Read, Write};

        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf)?;
            if n == 0 || head.len() > 16 * 1024 {
                return Ok(());
            }
            head.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let mut lines = text.split("\r\n");
        let mut request_line = lines.next().unwrap_or_default().split(' ');
        let method = request_line.next().unwrap_or_default().to_owned();
        let path = request_line.next().unwrap_or_default().to_owned();
        let headers = lines
            .take_while(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        if let Ok(mut seen) = seen.lock() {
            seen.push(SeenRequest {
                method,
                path: path.clone(),
                headers,
            });
        }
        let reply = routes.iter().find(|(p, _)| *p == path).map(|(_, r)| r);
        let (status, extra, body): (&str, String, &[u8]) = match reply {
            Some(Reply::Body(body)) => ("200 OK", String::new(), body),
            Some(Reply::Redirect(to)) => ("302 Found", format!("Location: {to}\r\n"), &[]),
            None => ("404 Not Found", String::new(), &[]),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes())?;
        stream.write_all(body)?;
        stream.flush()?;
        stream.shutdown(std::net::Shutdown::Both)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{LoopbackServer, Reply};
    use super::*;
    use sha2::Digest;

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

    const VERSION: &str = "0.2.0";
    const ASSET_NAME: &str = "Dockering-x86_64.AppImage";

    fn path_of(url: &str) -> String {
        reqwest::Url::parse(url).expect("url").path().to_owned()
    }

    fn asset_url() -> String {
        format!("https://github.com/{REPO}/releases/download/v{VERSION}/{ASSET_NAME}")
    }

    fn asset_for(body: &[u8], url: String) -> PlatformAsset {
        PlatformAsset {
            kind: crate::AssetKind::Appimage,
            url,
            sha256: hex::encode(sha2::Sha256::digest(body)),
            size: body.len() as u64,
        }
    }

    /// Serves a manifest, its signature and one asset at the paths GitHub would use.
    fn release_server(body: &[u8]) -> LoopbackServer {
        LoopbackServer::start(vec![
            (
                path_of(crate::MANIFEST_URL),
                Reply::Body(b"{\"manifest\":true}".to_vec()),
            ),
            (
                path_of(crate::SIGNATURE_URL),
                Reply::Body(b"signature text".to_vec()),
            ),
            (path_of(&asset_url()), Reply::Body(body.to_vec())),
        ])
        .expect("loopback server")
    }

    #[test]
    fn upd_010_user_agent_exact() {
        assert_eq!(
            user_agent("0.0.1"),
            format!(
                "Dockering/0.0.1 ({}; {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
        // The shape the docs promise, with no extra tokens.
        let ua = user_agent("1.2.3");
        let (name, rest) = ua.split_once(' ').expect("name and platform");
        assert_eq!(name, "Dockering/1.2.3");
        assert!(
            rest.starts_with('(') && rest.ends_with(')') && rest.contains("; "),
            "{ua}"
        );
    }

    #[tokio::test]
    async fn upd_010_user_agent_exact_on_the_wire() {
        let server = release_server(b"x");
        let source = GithubSource::with_base("0.0.1", &server.base_url()).expect("source");
        source.fetch_manifest().await.expect("fetch");
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "manifest + signature");
        let expected = format!(
            "Dockering/0.0.1 ({}; {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for request in &requests {
            assert_eq!(request.header("user-agent"), Some(expected.as_str()));
            // No cookies, ids or credentials (UPD-010).
            for name in ["cookie", "authorization", "referer", "x-request-id"] {
                assert_eq!(request.header(name), None, "{name}");
            }
        }
    }

    #[test]
    fn upd_010_rewrite_keeps_path_and_query_only() {
        let base = reqwest::Url::parse("http://127.0.0.1:8123").expect("base");
        let url = reqwest::Url::parse("https://github.com/a/b/c.json?x=1").expect("url");
        let out = rewrite_origin(url, &base).expect("rewrite");
        assert_eq!(out.as_str(), "http://127.0.0.1:8123/a/b/c.json?x=1");
        // A base without a port clears the original's.
        let base = reqwest::Url::parse("https://localhost").expect("base");
        let url = reqwest::Url::parse("https://github.com:444/a").expect("url");
        assert_eq!(
            rewrite_origin(url, &base).expect("rewrite").as_str(),
            "https://localhost/a"
        );
    }

    #[test]
    fn upd_010_production_source_has_no_base_and_with_base_validates() {
        assert!(GithubSource::new("1.0.0").expect("source").base.is_none());
        for bad in ["", "not a url", "ftp://127.0.0.1", "file:///tmp"] {
            assert!(GithubSource::with_base("1.0.0", bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn upd_010_seam_rewrites_host_but_keeps_url_pin() {
        let body = b"appimage bytes".to_vec();
        let server = release_server(&body);
        let source = GithubSource::with_base(VERSION, &server.base_url()).expect("source");

        // Manifest and signature come from the loopback server although the constants name GitHub.
        let (manifest, signature) = source.fetch_manifest().await.expect("manifest");
        assert_eq!(manifest, b"{\"manifest\":true}");
        assert_eq!(signature, "signature text");
        let host = server.base_url().trim_start_matches("http://").to_owned();
        let requests = server.requests();
        assert_eq!(
            requests.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            [path_of(crate::MANIFEST_URL), path_of(crate::SIGNATURE_URL)]
        );
        assert!(
            requests
                .iter()
                .all(|r| r.header("host") == Some(host.as_str()))
        );

        // The asset keeps its GitHub URL in the manifest and is downloaded from the server.
        let asset = asset_for(&body, asset_url());
        let name = release_asset_name(&asset.url, VERSION).expect("pinned url");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = source
            .download(&asset, dir.path(), &name, Box::new(|_, _| {}))
            .await
            .expect("download");
        assert_eq!(std::fs::read(path).expect("file"), body);
        assert_eq!(server.requests().len(), 3);

        // The pins still apply to the original URL, so none of these reaches the server.
        let before = server.requests().len();
        let loopback_url = format!("{}{}", server.base_url(), path_of(&asset_url()));
        for url in [
            // another host, even one that looks like GitHub
            format!("https://evil.example{}", path_of(&asset_url())),
            format!("https://github.com.evil.example{}", path_of(&asset_url())),
            format!("https://raw.githubusercontent.com{}", path_of(&asset_url())),
            // plain http to an allowed host
            format!("http://github.com{}", path_of(&asset_url())),
            // a URL already pointing at the loopback server (not an allowed host)
            loopback_url,
        ] {
            let asset = asset_for(&body, url.clone());
            let result = source
                .download(&asset, dir.path(), &name, Box::new(|_, _| {}))
                .await;
            assert!(
                matches!(result, Err(UpdateError::Manifest(_))),
                "{url}: {result:?}"
            );
        }
        // The manifest-level pin (repo, version, file name) is checked before any download.
        for url in [
            format!(
                "https://github.com/attacker/dockering/releases/download/v{VERSION}/{ASSET_NAME}"
            ),
            format!("https://github.com/{REPO}/releases/download/v0.1.0/{ASSET_NAME}"),
            format!("https://github.com/{REPO}/releases/download/v{VERSION}/../{ASSET_NAME}"),
        ] {
            assert!(release_asset_name(&url, VERSION).is_err(), "{url}");
        }
        assert_eq!(
            server.requests().len(),
            before,
            "rejected before any request"
        );
    }

    #[tokio::test]
    async fn upd_010_seam_follows_redirects_only_inside_base() {
        let manifest_path = path_of(crate::MANIFEST_URL);
        let server = LoopbackServer::start(vec![
            (manifest_path, Reply::Redirect("/moved".into())),
            ("/moved".into(), Reply::Body(b"moved".to_vec())),
            (
                path_of(crate::SIGNATURE_URL),
                Reply::Redirect("https://github.com/elsewhere".into()),
            ),
        ])
        .expect("loopback server");
        let source = GithubSource::with_base(VERSION, &server.base_url()).expect("source");
        assert_eq!(
            source.get_small(crate::MANIFEST_URL).await,
            Ok(b"moved".to_vec())
        );
        let result = source.get_small(crate::SIGNATURE_URL).await;
        assert!(matches!(result, Err(UpdateError::Network(_))), "{result:?}");
        assert!(
            server.requests().iter().all(|r| r.path != "/elsewhere"),
            "the redirect target was never contacted"
        );
    }
}
