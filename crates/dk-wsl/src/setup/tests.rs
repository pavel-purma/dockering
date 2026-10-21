//! Setup tests: fake `wsl.exe`, loopback HTTP server, temp dirs. No test runs a real `wsl.exe`
//! mutation or touches the real `%LOCALAPPDATA%\Dockering`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use dk_core::{
    EngineError, EngineFactory, EngineResult, EngineStream, SetupOffer, SetupPhase, SetupProgress,
};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::download::{Source, url_allowed};
use super::import::{self, Step, error_token};
use super::pins::{self, PINS, PLACEHOLDER_SHA256, Pin};
use super::{DISTRO, OFFER_ID, SetupEnv, messages};
use crate::factory::{WslDistroFactory, discovered_engine};
#[cfg(windows)]
use crate::runner::Wsl;
use crate::runner::WslOutput;
use crate::runner::fake::FakeWsl;

// ── loopback server ──────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Redirect(String),
    /// Sends `first`, waits for the gate, sends `rest` (no Content-Length).
    Stalled {
        first: Vec<u8>,
        rest: Vec<u8>,
        gate: Arc<Notify>,
    },
    /// Keeps sending `chunk` (no Content-Length) until the client hangs up.
    Endless(Vec<u8>),
}

struct Server {
    base: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut seen = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&chunk[..n]),
                        }
                    }
                    match reply {
                        Reply::Body(body) => {
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = socket.write_all(head.as_bytes()).await;
                            let _ = socket.write_all(&body).await;
                        }
                        Reply::Redirect(to) => {
                            let head = format!(
                                "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            );
                            let _ = socket.write_all(head.as_bytes()).await;
                        }
                        Reply::Stalled { first, rest, gate } => {
                            let _ = socket
                                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                                .await;
                            let _ = socket.write_all(&first).await;
                            let _ = socket.flush().await;
                            gate.notified().await;
                            let _ = socket.write_all(&rest).await;
                        }
                        Reply::Endless(piece) => {
                            let _ = socket
                                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                                .await;
                            for _ in 0..10_000 {
                                if socket.write_all(&piece).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { base, hits, task }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

// ── harness ──────────────────────────────────────────────────────────────────────────────

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A published pin for `image` whose size and digest match `body`.
fn test_pin(image: &'static str, body: &[u8]) -> Pin {
    Pin {
        image,
        arch: "x86_64",
        label: if image == "ubuntu" {
            "Ubuntu"
        } else {
            "Alpine"
        },
        description: "test",
        url: leak(format!(
            "https://github.com/pavel-purma/dockering/releases/download/rootfs-1/dockering-engine-{image}-x86_64.tar.xz"
        )),
        size_bytes: body.len() as u64,
        sha256: leak(sha256_hex(body)),
        disk_bytes: 1_000,
        published: true,
    }
}

fn payload() -> Vec<u8> {
    (0..64u32 * 1024).map(|i| (i % 251) as u8).collect()
}

struct Harness {
    fake: Arc<FakeWsl>,
    factory: WslDistroFactory,
    tmp: tempfile::TempDir,
    server: Server,
    free: Arc<AtomicU64>,
}

impl Harness {
    async fn new(reply: Reply, pins: Vec<Pin>) -> Self {
        Self::with(reply, pins, |_| {}).await
    }

    async fn with(reply: Reply, pins: Vec<Pin>, tweak: impl FnOnce(&mut SetupEnv)) -> Self {
        let fake = FakeWsl::new(&[]);
        fake.pings(&[false, false, true]);
        let server = Server::start(reply).await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let free = Arc::new(AtomicU64::new(u64::MAX / 2));
        let reader = free.clone();
        let mut env = SetupEnv::new(
            fake.clone(),
            tmp.path().join("cache"),
            tmp.path().join("Dockering").join("wsl"),
            Box::new(move |_| Ok(reader.load(Ordering::SeqCst))),
        );
        env.source = Some(Source::with_base(&server.base).expect("client"));
        env.pins = pins;
        env.arch = "x86_64";
        env.settle = Duration::from_millis(1);
        env.ready_poll = Duration::from_millis(1);
        env.ready_timeout = Duration::from_secs(2);
        tweak(&mut env);
        let factory = WslDistroFactory::with_wsl(fake.clone()).with_setup(env);
        Self {
            fake,
            factory,
            tmp,
            server,
            free,
        }
    }

    fn setup(&self, choice: &str) -> EngineStream<SetupProgress> {
        self.factory.setup(OFFER_ID, choice)
    }

    fn cache(&self) -> PathBuf {
        self.tmp.path().join("cache")
    }

    fn distro_dir(&self) -> PathBuf {
        self.tmp.path().join("Dockering").join("wsl").join(DISTRO)
    }

    fn cache_files(&self) -> Vec<String> {
        std::fs::read_dir(self.cache())
            .map(|dir| {
                dir.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn unregisters(&self) -> usize {
        self.fake
            .calls()
            .iter()
            .filter(|call| call.first().map(String::as_str) == Some("--unregister"))
            .count()
    }

    fn imports(&self) -> usize {
        self.fake
            .calls()
            .iter()
            .filter(|call| call.first().map(String::as_str) == Some("--import"))
            .count()
    }
}

async fn drain(mut stream: EngineStream<SetupProgress>) -> Vec<EngineResult<SetupProgress>> {
    let mut items = Vec::new();
    let collect = async {
        while let Some(item) = stream.next().await {
            items.push(item);
        }
    };
    tokio::time::timeout(Duration::from_secs(30), collect)
        .await
        .expect("setup finished");
    items
}

fn last_error(items: &[EngineResult<SetupProgress>]) -> EngineError {
    match items.last() {
        Some(Err(error)) => error.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn done_config(items: &[EngineResult<SetupProgress>]) -> dk_core::EngineConfig {
    match items.last() {
        Some(Ok(SetupProgress::Done(cfg))) => cfg.clone(),
        other => panic!("expected Done, got {other:?}"),
    }
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Reads events until one satisfies `pick`.
async fn next_matching(
    stream: &mut EngineStream<SetupProgress>,
    mut pick: impl FnMut(&SetupProgress) -> bool,
) {
    let wait = async {
        while let Some(item) = stream.next().await {
            if pick(&item.expect("no error yet")) {
                return;
            }
        }
        panic!("stream ended");
    };
    tokio::time::timeout(Duration::from_secs(20), wait)
        .await
        .expect("event arrived");
}

fn phases(items: &[EngineResult<SetupProgress>]) -> Vec<SetupPhase> {
    let mut out: Vec<SetupPhase> = Vec::new();
    for item in items {
        if let Ok(SetupProgress::Phase { phase, .. }) = item
            && out.last() != Some(phase)
        {
            out.push(*phase);
        }
    }
    out
}

fn both(body: &[u8]) -> Vec<Pin> {
    vec![test_pin("ubuntu", body), test_pin("alpine", body)]
}

/// Every argv Dockering may issue during setup.
fn allowed(argv: &[String]) -> bool {
    let argv = argv.iter().map(String::as_str).collect::<Vec<_>>();
    match argv.as_slice() {
        ["--import", name, _, _, "--version", "2"] => *name == DISTRO,
        ["--unregister", name] => *name == DISTRO,
        ["-d", name, "--exec", "true"] => *name == DISTRO,
        _ => false,
    }
}

// ── ENG-150 offers ───────────────────────────────────────────────────────────────────────

async fn offers_of(h: &Harness) -> Vec<SetupOffer> {
    h.factory.setup_offers().await
}

#[tokio::test]
async fn eng_150_offer_only_on_windows_with_wsl2_and_no_distro() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    let offers = offers_of(&h).await;
    assert_eq!(offers.len(), 1);
    let offer = &offers[0];
    assert_eq!(offer.id, "docker-in-wsl");
    assert_eq!(offer.title, "Set up Docker in WSL");
    assert_eq!(offer.action_label, "Set up Docker in WSL");
    let ids = offer
        .choices
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["ubuntu", "alpine"]);
    assert_eq!(offer.choices[0].label, "Ubuntu");
    assert_eq!(offer.choices[0].download_bytes, body.len() as u64);
    assert_eq!(offer.choices[0].disk_bytes, 1_000);

    // WSL missing, too old, exactly the minimum, or unsupported.
    h.fake.version(None);
    assert!(offers_of(&h).await.is_empty());
    h.fake.version(Some((1, 2, 5, 0)));
    assert!(offers_of(&h).await.is_empty());
    h.fake.version(Some((1, 99, 0, 0)));
    assert!(offers_of(&h).await.is_empty());
    h.fake.version(Some((2, 0, 0, 0)));
    assert_eq!(offers_of(&h).await.len(), 1);
    h.fake.unsupported();
    assert!(offers_of(&h).await.is_empty());
}

#[tokio::test]
async fn eng_150_offer_hidden_when_distro_exists() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.distros(&["Ubuntu-22.04"]);
    assert_eq!(offers_of(&h).await.len(), 1);
    h.fake.distros(&["Ubuntu-22.04", "Dockering-Engine"]);
    assert!(offers_of(&h).await.is_empty());
}

#[tokio::test]
async fn eng_150_offer_hidden_when_no_pin_is_published() {
    // The production table has no published asset yet.
    let h = Harness::with(Reply::Body(vec![]), PINS.to_vec(), |_| {}).await;
    assert!(offers_of(&h).await.is_empty());
    // A single published choice is still offered, alone (A13); another arch has none.
    let body = payload();
    let one = vec![test_pin("alpine", &body)];
    let h = Harness::new(Reply::Body(body.clone()), one.clone()).await;
    let offers = offers_of(&h).await;
    assert_eq!(offers[0].choices.len(), 1);
    assert_eq!(offers[0].choices[0].id, "alpine");
    let h = Harness::with(Reply::Body(body), one, |env| env.arch = "aarch64").await;
    assert!(offers_of(&h).await.is_empty());
}

#[tokio::test]
async fn eng_163_consent_text_states_lifetime() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    let text = offers_of(&h).await[0].body.clone();
    for needle in [
        "run only while Dockering, or another WSL session, is open",
        "\"dockering-engine\"",
        "Docker Engine",
        "%LOCALAPPDATA%\\Dockering\\wsl\\dockering-engine",
        "github.com",
        "No administrator rights",
        "other distros are not touched",
    ] {
        assert!(
            text.to_lowercase().contains(&needle.to_lowercase()),
            "{needle}: {text}"
        );
    }
    for mechanism in ["wsl.exe", "rootfs", "import", "tarball", "--"] {
        assert!(!text.contains(mechanism), "{mechanism}: {text}");
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn eng_142_non_windows_stub_offers_nothing() {
    let factory = WslDistroFactory::new();
    assert!(factory.setup_offers().await.is_empty());
    let items = drain(factory.setup(OFFER_ID, "alpine")).await;
    assert!(matches!(items.last(), Some(Err(_))));
}

#[tokio::test]
async fn eng_142_default_factory_offers_nothing_while_no_pin_is_published() {
    assert!(WslDistroFactory::new().setup_offers().await.is_empty());
}

// ── ENG-152 pins ─────────────────────────────────────────────────────────────────────────

#[test]
fn eng_152_each_choice_and_arch_has_its_own_pin() {
    let mut urls = std::collections::BTreeSet::new();
    for image in pins::IMAGES {
        for arch in ["x86_64", "aarch64"] {
            let pin = pins::find(&PINS, image, arch).expect("one pin per image and arch");
            assert!(urls.insert(pin.url), "{}", pin.url);
            assert_eq!(
                pin.url,
                format!(
                    "https://github.com/pavel-purma/dockering/releases/download/rootfs-1/dockering-engine-{image}-{arch}.tar.xz"
                )
            );
            assert_eq!(
                pin.file_name(),
                format!("dockering-engine-{image}-{arch}.tar.xz")
            );
        }
    }
    assert_eq!(PINS.len(), 4);
    assert_eq!(pins::IMAGES, ["ubuntu", "alpine"]);
    let ubuntu = pins::find(&PINS, "ubuntu", "x86_64").unwrap();
    let alpine = pins::find(&PINS, "alpine", "x86_64").unwrap();
    assert_eq!(
        (ubuntu.label, ubuntu.description),
        ("Ubuntu", "Familiar Ubuntu LTS. Larger download.")
    );
    assert_eq!(
        (alpine.label, alpine.description),
        ("Alpine", "Smallest and fastest to set up.")
    );
    assert_eq!(alpine.size_bytes, 58_477_916);
    assert_eq!(ubuntu.size_bytes, 355_357_616);
    assert_eq!(alpine.disk_bytes, 370_000_000);
    assert_eq!(ubuntu.disk_bytes, 1_800_000_000);
    assert!(pins::find(&PINS, "debian", "x86_64").is_none());
    assert!(pins::find(&PINS, "ubuntu", "riscv64").is_none());
}

#[test]
fn eng_152_production_pins_are_consistent() {
    for pin in &PINS {
        let url = reqwest::Url::parse(pin.url).expect("url");
        assert_eq!(url.scheme(), "https", "{}", pin.url);
        assert_eq!(url.host_str(), Some("github.com"), "{}", pin.url);
        assert!(
            url.path()
                .starts_with("/pavel-purma/dockering/releases/download/rootfs-"),
            "{}",
            pin.url
        );
        assert!(pin.size_bytes > 0 && pin.disk_bytes > 0);
        if pin.published {
            assert!(pins::is_sha256_hex(pin.sha256), "{}", pin.url);
            assert_ne!(pin.sha256, PLACEHOLDER_SHA256, "{}", pin.url);
        } else {
            assert_eq!(pin.sha256, PLACEHOLDER_SHA256, "{}", pin.url);
        }
    }
    let keys = PINS
        .iter()
        .map(|pin| (pin.image, pin.arch))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), PINS.len());
    assert!(
        include_str!("../../../../Cargo.toml").contains("https://github.com/pavel-purma/dockering")
    );
}

#[test]
fn eng_152_placeholder_digest_can_never_be_published() {
    let mut pin = PINS[0];
    pin.published = true;
    assert_eq!(pin.sha256, PLACEHOLDER_SHA256);
    assert!(!pin.usable());
    assert!(pins::choices(&[pin], "x86_64").is_empty());
    pin.sha256 = "ABC";
    assert!(!pin.usable());
    pin.published = false;
    pin.sha256 = leak("a".repeat(64));
    assert!(!pin.usable());
    pin.published = true;
    assert!(pin.usable());
}

#[tokio::test]
async fn eng_152_unknown_choice_id_rejected() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    for choice in [
        "",
        "debian",
        "Ubuntu",
        "ubuntu ",
        "../ubuntu",
        "ubuntu\0",
        "alpine;calc",
        "-d",
    ] {
        let items = drain(h.setup(choice)).await;
        assert_eq!(items.len(), 1, "{choice:?}");
        assert!(
            matches!(items[0], Err(EngineError::Api { .. })),
            "{choice:?}"
        );
    }
    let items = drain(h.factory.setup("other-offer", "alpine")).await;
    assert!(matches!(items[0], Err(EngineError::Api { .. })));
    assert!(h.fake.calls().is_empty());
    assert_eq!(h.server.hits(), 0);
    assert!(!h.cache().exists(), "no I/O before validation");
}

#[tokio::test]
async fn eng_152_unpublished_pin_refused() {
    let body = payload();
    let mut unpublished = test_pin("alpine", &body);
    unpublished.published = false;
    let h = Harness::new(
        Reply::Body(body.clone()),
        vec![unpublished, test_pin("ubuntu", &body)],
    )
    .await;
    let items = drain(h.setup("alpine")).await;
    assert!(matches!(last_error(&items), EngineError::Api { .. }));
    assert!(h.fake.calls().is_empty());
    assert_eq!(h.server.hits(), 0);
    // The placeholder digest is refused even if someone flips the marker.
    let mut placeholder = test_pin("alpine", &body);
    placeholder.sha256 = PLACEHOLDER_SHA256;
    let h = Harness::new(Reply::Body(body), vec![placeholder]).await;
    assert!(matches!(
        last_error(&drain(h.setup("alpine")).await),
        EngineError::Api { .. }
    ));
    assert_eq!(h.server.hits(), 0);
}

#[tokio::test]
async fn eng_152_digest_mismatch_deletes_and_fails() {
    let body = payload();
    let mut served = body.clone();
    served[100] ^= 0xff;
    let h = Harness::new(Reply::Body(served), both(&body)).await;
    let items = drain(h.setup("alpine")).await;
    let error = last_error(&items);
    assert!(
        error
            .to_string()
            .contains("The download failed verification."),
        "{error}"
    );
    assert!(error.hint().is_some());
    assert!(h.cache_files().is_empty(), "{:?}", h.cache_files());
    assert_eq!(h.imports(), 0);
    assert_eq!(h.unregisters(), 0);
}

#[tokio::test]
async fn eng_152_file_changed_after_download_is_refused() {
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        env.after_verify = Some(Box::new(|file| {
            let mut bytes = std::fs::read(file).unwrap();
            bytes[7] ^= 0xff;
            std::fs::write(file, bytes).unwrap();
        }));
    })
    .await;
    let items = drain(h.setup("alpine")).await;
    assert!(
        last_error(&items)
            .to_string()
            .contains("The download failed verification."),
    );
    assert_eq!(h.imports(), 0);
    assert_eq!(h.unregisters(), 0);
    assert!(h.cache_files().is_empty(), "{:?}", h.cache_files());
}

#[tokio::test]
async fn eng_152_streams_to_disk_not_memory() {
    let body = payload();
    let half = body.len() / 2;
    let gate = Arc::new(Notify::new());
    let reply = Reply::Stalled {
        first: body[..half].to_vec(),
        rest: body[half..].to_vec(),
        gate: gate.clone(),
    };
    let h = Harness::new(reply, both(&body)).await;
    let mut stream = h.setup("alpine");
    next_matching(&mut stream, |event| {
        matches!(event, SetupProgress::Phase { phase: SetupPhase::Downloading, fraction: Some(f), .. } if *f > 0.0)
    })
    .await;
    // The server hasn't finished, yet the bytes received so far are already on disk.
    let part = h.cache().join("dockering-engine-alpine-x86_64.tar.xz.part");
    wait_until("partial bytes on disk", || {
        std::fs::metadata(&part).is_ok_and(|m| m.len() > 0 && m.len() <= half as u64)
    })
    .await;
    gate.notify_one();
    let rest = drain(stream).await;
    assert!(matches!(rest.last(), Some(Ok(SetupProgress::Done(_)))));
    assert!(h.cache_files().is_empty(), "cache is deleted at the end");
}

#[tokio::test]
async fn eng_152_size_bound_aborts_oversized_body() {
    let body = payload();
    // No Content-Length and an endless body: the pinned size is a hard upper bound.
    let h = Harness::new(Reply::Endless(vec![7u8; 8192]), both(&body)).await;
    let items = drain(h.setup("alpine")).await;
    assert!(
        last_error(&items)
            .to_string()
            .contains("failed verification")
    );
    assert!(h.cache_files().is_empty());
    assert_eq!(h.imports(), 0);
    // A Content-Length above the pin is refused before the body is read.
    let mut bigger = body.clone();
    bigger.extend_from_slice(&[0u8; 10]);
    let h = Harness::new(Reply::Body(bigger), both(&body)).await;
    assert!(
        last_error(&drain(h.setup("ubuntu")).await)
            .to_string()
            .contains("failed verification")
    );
    assert_eq!(h.imports(), 0);
}

#[tokio::test]
async fn eng_152_constants_are_not_user_input() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    let items = drain(h.setup("ubuntu")).await;
    done_config(&items);
    let import = h.fake.calls().remove(0);
    let file = h.cache().join("dockering-engine-ubuntu-x86_64.tar.xz");
    assert_eq!(import[3], file.to_string_lossy());
    assert_eq!(import[1], DISTRO);
    assert_eq!(import[2], h.distro_dir().to_string_lossy());
}

// ── ENG-153 steps ────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn eng_153_happy_path_with_fakes() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    let items = drain(h.setup("alpine")).await;
    let config = done_config(&items);

    let expected = discovered_engine(DISTRO, true, None).config;
    assert_eq!(config, expected);
    assert_eq!(config.id.as_str(), "wsl-dockering-engine");
    assert_eq!(
        config.endpoint,
        dk_core::EngineEndpoint::WslDistro {
            distro: "dockering-engine".into(),
            mode: dk_core::WslMode::DialStdio
        }
    );
    assert_eq!(config.origin, dk_core::EngineOrigin::Discovered);
    assert!(config.enabled && !config.hidden);

    let dir = h.distro_dir().to_string_lossy().into_owned();
    let file = h
        .cache()
        .join("dockering-engine-alpine-x86_64.tar.xz")
        .to_string_lossy()
        .into_owned();
    let strings = |parts: &[&str]| parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        h.fake.calls(),
        vec![
            strings(&[
                "--import",
                "dockering-engine",
                &dir,
                &file,
                "--version",
                "2"
            ]),
            strings(&["-d", "dockering-engine", "--exec", "true"]),
        ]
    );
    assert_eq!(h.fake.pinged(), vec![DISTRO; 3]);
    assert_eq!(
        h.fake.timeouts(),
        vec![Duration::from_secs(300), Duration::from_secs(60)]
    );
    assert_eq!(h.fake.registered(), [DISTRO], "success keeps the distro");
    assert_eq!(h.unregisters(), 0);
    assert!(
        h.cache_files().is_empty(),
        "the cache file is always deleted"
    );
    assert_eq!(h.server.hits(), 1, "one GET");
}

#[tokio::test]
async fn eng_153_free_space_uses_per_image_figure() {
    let body = payload();
    let alpine = test_pin("alpine", &body);
    let mut ubuntu = test_pin("ubuntu", &body);
    ubuntu.disk_bytes = 5_000_000;
    let h = Harness::new(Reply::Body(body.clone()), vec![ubuntu, alpine]).await;
    let need = |pin: &Pin| pin.size_bytes + pin.disk_bytes + (1 << 30);

    h.free.store(need(&alpine) - 1, Ordering::SeqCst);
    let error = last_error(&drain(h.setup("alpine")).await);
    assert!(error.to_string().contains("enough free space"), "{error}");
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.starts_with("Free up space on"))
    );
    assert_eq!(h.server.hits(), 0);

    // The same figure is too little for the larger image …
    h.free.store(need(&alpine), Ordering::SeqCst);
    assert!(
        drain(h.setup("ubuntu"))
            .await
            .last()
            .is_some_and(Result::is_err)
    );
    assert_eq!(h.server.hits(), 0);
    // … and exactly enough for Alpine.
    done_config(&drain(h.setup("alpine")).await);
}

#[tokio::test]
async fn eng_156_low_disk_message() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.free.store(10, Ordering::SeqCst);
    let error = last_error(&drain(h.setup("ubuntu")).await);
    let EngineError::Unreachable { reason, hint } = &error else {
        panic!("{error:?}");
    };
    assert!(
        reason.contains("There isn't enough free space on"),
        "{reason}"
    );
    assert!(
        hint.as_deref()
            .is_some_and(|h| h.contains("Free up space on") && h.ends_with("and try again."))
    );
    assert_eq!(h.imports(), 0);
    assert!(h.cache_files().is_empty());
}

#[tokio::test]
async fn eng_153_parent_dir_is_created_leaf_must_be_empty() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    assert!(!h.tmp.path().join("Dockering").exists());
    done_config(&drain(h.setup("alpine")).await);
    assert!(h.tmp.path().join("Dockering").join("wsl").is_dir());
    assert!(h.cache().is_dir());
    assert!(!h.distro_dir().exists(), "the leaf is left to wsl.exe");

    // A leaf with a file in it fails before anything is downloaded.
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    std::fs::create_dir_all(h.distro_dir()).unwrap();
    std::fs::write(h.distro_dir().join("ext4.vhdx"), b"x").unwrap();
    let error = last_error(&drain(h.setup("alpine")).await);
    assert!(
        error.to_string().contains("already contains files"),
        "{error}"
    );
    assert_eq!(h.server.hits(), 0);
    assert_eq!(h.imports(), 0);
    assert_eq!(h.unregisters(), 0);
    assert!(h.distro_dir().join("ext4.vhdx").exists(), "not touched");

    // An empty leaf is fine.
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    std::fs::create_dir_all(h.distro_dir()).unwrap();
    done_config(&drain(h.setup("alpine")).await);
}

// ── ENG-154 progress, cancel, cleanup ────────────────────────────────────────────────────

#[tokio::test]
async fn eng_154_progress_phases_in_order() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    let items = drain(h.setup("alpine")).await;
    assert_eq!(
        phases(&items),
        [
            SetupPhase::Checking,
            SetupPhase::Downloading,
            SetupPhase::Verifying,
            SetupPhase::Importing,
            SetupPhase::Starting
        ]
    );
    assert!(matches!(items.last(), Some(Ok(SetupProgress::Done(_)))));
    let fractions = items
        .iter()
        .filter_map(|item| match item {
            Ok(SetupProgress::Phase {
                phase: SetupPhase::Downloading,
                fraction: Some(f),
                ..
            }) => Some(*f),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
    assert!((fractions.last().copied().unwrap_or(0.0) - 1.0).abs() < 1e-6);
    for item in &items {
        if let Ok(SetupProgress::Phase { detail, .. }) = item {
            assert!(!detail.is_empty());
        }
    }
}

#[tokio::test]
async fn eng_154_cancel_before_import_removes_cache() {
    let body = payload();
    let half = body.len() / 2;
    let gate = Arc::new(Notify::new());
    let reply = Reply::Stalled {
        first: body[..half].to_vec(),
        rest: body[half..].to_vec(),
        gate,
    };
    let h = Harness::new(reply, both(&body)).await;
    let mut stream = h.setup("alpine");
    next_matching(&mut stream, |event| {
        matches!(event, SetupProgress::Phase { phase: SetupPhase::Downloading, fraction: Some(f), .. } if *f > 0.0)
    })
    .await;
    wait_until("a partial file", || !h.cache_files().is_empty()).await;
    drop(stream);
    wait_until("the cache to be empty", || h.cache_files().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.imports(), 0);
    assert_eq!(
        h.unregisters(),
        0,
        "nothing was imported, nothing is unregistered"
    );
    assert!(h.fake.calls().is_empty());
}

#[tokio::test]
async fn eng_154_cancel_after_import_unregisters_own_distro() {
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        env.ready_timeout = Duration::from_secs(30);
    })
    .await;
    h.fake.pings(&[false]);
    let mut stream = h.setup("alpine");
    next_matching(&mut stream, |event| {
        matches!(
            event,
            SetupProgress::Phase {
                phase: SetupPhase::Starting,
                ..
            }
        )
    })
    .await;
    assert_eq!(h.fake.registered(), [DISTRO], "the import ran");
    drop(stream);
    wait_until("the unregister", || h.fake.registered().is_empty()).await;
    assert_eq!(h.unregisters(), 1);
    assert!(
        h.fake
            .calls()
            .contains(&vec!["--unregister".to_owned(), DISTRO.to_owned()])
    );
    wait_until("the cache to be empty", || h.cache_files().is_empty()).await;
}

#[tokio::test]
async fn eng_154_cancel_during_import_still_unregisters() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.import_delay(Duration::from_secs(30));
    let mut stream = h.setup("alpine");
    next_matching(&mut stream, |event| {
        matches!(
            event,
            SetupProgress::Phase {
                phase: SetupPhase::Importing,
                ..
            }
        )
    })
    .await;
    wait_until("the import call", || {
        h.fake.timeouts().contains(&Duration::from_secs(300))
    })
    .await;
    drop(stream);
    wait_until("the unregister", || h.unregisters() == 1).await;
    wait_until("the cache to be empty", || h.cache_files().is_empty()).await;
}

#[tokio::test]
async fn eng_154_failure_after_import_unregisters() {
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        env.ready_timeout = Duration::from_millis(60);
    })
    .await;
    h.fake.pings(&[false]);
    let items = drain(h.setup("alpine")).await;
    let error = last_error(&items);
    assert!(error.to_string().contains("Docker didn't start"), "{error}");
    assert!(error.hint().is_some());
    wait_until("the unregister", || h.fake.registered().is_empty()).await;
    assert_eq!(h.unregisters(), 1);
    assert!(h.cache_files().is_empty());
}

#[tokio::test]
async fn eng_154_failed_unregister_is_retried() {
    // The first `--unregister` runs while the service is busy: the distro is still listed, so
    // the cleanup tries again.
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        env.ready_timeout = Duration::from_millis(30);
    })
    .await;
    h.fake.pings(&[false]);
    drain(h.setup("alpine")).await;
    wait_until("the distro to be gone", || h.fake.registered().is_empty()).await;
}

// ── ENG-155 safety ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn eng_155_existing_distro_never_touched() {
    let body = payload();
    for existing in ["dockering-engine", "Dockering-Engine"] {
        let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
        h.fake.distros(&[existing]);
        let items = drain(h.setup("alpine")).await;
        assert!(last_error(&items).to_string().contains("already exists"));
        assert_eq!(h.imports(), 0);
        assert_eq!(h.unregisters(), 0);
        assert!(h.fake.calls().is_empty());
        assert_eq!(h.fake.registered(), [existing]);
        assert_eq!(h.server.hits(), 0);
    }
}

#[tokio::test]
async fn eng_155_distro_appearing_during_download_is_never_imported_over() {
    let body = payload();
    let half = body.len() / 2;
    let gate = Arc::new(Notify::new());
    let reply = Reply::Stalled {
        first: body[..half].to_vec(),
        rest: body[half..].to_vec(),
        gate: gate.clone(),
    };
    let h = Harness::new(reply, both(&body)).await;
    let mut stream = h.setup("alpine");
    next_matching(&mut stream, |event| {
        matches!(event, SetupProgress::Phase { phase: SetupPhase::Downloading, fraction: Some(f), .. } if *f > 0.0)
    })
    .await;
    h.fake.distros(&["dockering-engine"]);
    gate.notify_one();
    let items = drain(stream).await;
    assert!(last_error(&items).to_string().contains("already exists"));
    assert_eq!(h.imports(), 0);
    assert_eq!(h.unregisters(), 0);
    assert_eq!(
        h.fake.registered(),
        [DISTRO],
        "the other distro is untouched"
    );
}

#[tokio::test]
async fn eng_155_failure_before_import_never_unregisters() {
    let body = payload();
    // Digest mismatch.
    let mut served = body.clone();
    served[0] ^= 1;
    let h = Harness::new(Reply::Body(served), both(&body)).await;
    drain(h.setup("alpine")).await;
    assert_eq!(h.unregisters(), 0);
    // HTTP error.
    let h = Harness::new(Reply::Redirect("http://127.0.0.1:1/x".into()), both(&body)).await;
    drain(h.setup("alpine")).await;
    assert_eq!(h.unregisters(), 0);
    // `ERROR_ALREADY_EXISTS` from the import itself: it created nothing, so it is not undone.
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.import_fails(
        -1,
        "A distribution with the supplied name already exists.\nError code: Wsl/Service/RegisterDistro/ERROR_ALREADY_EXISTS",
    );
    let items = drain(h.setup("alpine")).await;
    assert!(last_error(&items).to_string().contains("already exists"));
    assert_eq!(h.imports(), 1);
    assert_eq!(h.unregisters(), 0);
    assert!(h.cache_files().is_empty());
}

#[tokio::test]
async fn eng_155_failed_import_is_unregistered_only_for_this_run() {
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        let target = env.distro_dir();
        env.after_verify = Some(Box::new(move |_| {
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("ext4.vhdx"), b"disk").unwrap();
        }));
    })
    .await;
    h.fake.import_fails(
        -1,
        "Importing the distribution failed.\nError code: Wsl/Service/RegisterDistro/WSL_E_IMPORT_FAILED",
    );
    let items = drain(h.setup("alpine")).await;
    assert!(
        last_error(&items)
            .to_string()
            .contains("couldn't be created")
    );
    wait_until("the unregister", || h.unregisters() == 1).await;
}

/// Gives the hook a handle on the fake once the harness exists.
type FakeSlot = Arc<std::sync::Mutex<Option<Arc<FakeWsl>>>>;

fn slot_of(h: &Harness, slot: &FakeSlot) {
    *slot.lock().unwrap() = Some(h.fake.clone());
}

const UNPARSABLE: &str = "Something went wrong in a language nobody parses.";

#[tokio::test]
async fn eng_155_failed_import_without_our_vhdx_never_unregisters() {
    let body = payload();
    let slot = FakeSlot::default();
    let hook_slot = slot.clone();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), move |env| {
        // Someone else registers the name after the preflight, before our import.
        env.after_verify = Some(Box::new(move |_| {
            if let Some(fake) = hook_slot.lock().unwrap().as_ref() {
                fake.distros(&[DISTRO]);
            }
        }));
    })
    .await;
    slot_of(&h, &slot);
    h.fake.import_fails(-1, UNPARSABLE);
    let items = drain(h.setup("alpine")).await;
    assert!(last_error(&items).to_string().contains("couldn't create"));
    assert_eq!(h.imports(), 1);
    // Let a (wrong) cleanup task run before asserting.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.unregisters(), 0);
    assert_eq!(
        h.fake.registered(),
        [DISTRO],
        "the other distro is untouched"
    );
    assert!(h.cache_files().is_empty());
}

#[tokio::test]
async fn eng_155_failed_import_with_our_vhdx_is_unregistered() {
    let body = payload();
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        let target = env.distro_dir();
        env.after_verify = Some(Box::new(move |_| {
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("ext4.vhdx"), b"disk").unwrap();
        }));
    })
    .await;
    h.fake.import_fails(-1, UNPARSABLE);
    let items = drain(h.setup("alpine")).await;
    assert!(last_error(&items).to_string().contains("couldn't create"));
    wait_until("the unregister", || h.unregisters() == 1).await;
}

#[tokio::test]
async fn eng_155_panic_cleanup_holds_the_gate() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.panic_once_on_boot();
    h.fake.unregister_delay(Duration::from_millis(300));
    // The first run panics after the import; its stream just ends.
    let first = drain(h.setup("alpine")).await;
    assert!(!matches!(first.last(), Some(Ok(SetupProgress::Done(_)))));
    // The second run must wait for the cleanup: the name is free again when it starts.
    let second = drain(h.setup("alpine")).await;
    done_config(&second);
    assert_eq!(h.unregisters(), 1, "exactly one unregister");
    assert_eq!(h.imports(), 2);
    assert_eq!(h.fake.registered(), [DISTRO]);
}

// ── ENG-156 guidance ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn eng_156_missing_wsl_gives_hint_no_install() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.version(None);
    let error = last_error(&drain(h.setup("alpine")).await);
    assert!(error.to_string().contains("isn't installed"), "{error}");
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("Microsoft Store"))
    );

    h.fake.version(Some((1, 2, 5, 0)));
    let error = last_error(&drain(h.setup("alpine")).await);
    assert!(error.to_string().contains("too old"), "{error}");
    assert_eq!(error.hint(), Some("Run `wsl --update` and try again."));

    // Nothing ran: no install, no update, no download, no import.
    assert!(h.fake.calls().is_empty());
    assert_eq!(h.server.hits(), 0);
    assert!(!h.cache().exists());
}

#[test]
fn eng_156_import_error_codes_map_to_hints() {
    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
    let fake = FakeWsl::new(&[]);
    let env = SetupEnv::new(
        fake,
        PathBuf::from("cache"),
        PathBuf::from("wsl"),
        Box::new(|_: &Path| Ok(0)),
    );
    let cases = [
        (
            "ERROR_ALREADY_EXISTS",
            "already exists",
            "Nothing was changed",
        ),
        (
            "ERROR_FILE_EXISTS",
            "already contains files",
            "Remove or rename",
        ),
        ("WSL_E_IMPORT_FAILED", "couldn't be created", "wsl --update"),
        ("ERROR_FILE_NOT_FOUND", "went missing", "Try again"),
        (
            "ERROR_PATH_NOT_FOUND",
            "folder couldn't be created",
            "Free up space",
        ),
        ("E_INVALIDARG", "didn't accept", "wsl --update"),
        ("WSL_E_DISTRO_NOT_FOUND", "disappeared", "Try again"),
    ];
    for (token, reason, hint) in cases {
        // The sentence is localised (here Czech), only the token is stable.
        let text =
            format!("Operace selhala.\r\nError code: Wsl/Service/RegisterDistro/{token}\r\n");
        let output = WslOutput {
            code: Some(-1),
            stdout: utf16(&text),
            stderr: vec![],
        };
        let error = import::map_failure(Step::Import, &output, &env, "drive C:");
        let EngineError::Unreachable {
            reason: r,
            hint: Some(h),
        } = &error
        else {
            panic!("{token}: {error:?}");
        };
        assert!(r.contains(reason), "{token}: {r}");
        assert!(h.contains(hint), "{token}: {h}");
    }
    // Unknown token: generic message, still a hint.
    let output = WslOutput {
        code: Some(-1),
        stdout: utf16("Error code: Wsl/Weird"),
        stderr: vec![],
    };
    let error = import::map_failure(Step::Boot, &output, &env, "drive C:");
    assert!(error.to_string().contains("couldn't start the engine"));
    assert!(error.hint().is_some());
    assert_eq!(
        error_token("x Error code: Wsl/Service/WSL_E_DISTRO_NOT_FOUND\n"),
        Some("WSL_E_DISTRO_NOT_FOUND")
    );
    assert_eq!(
        error_token("Error code: Wsl/ERROR_PATH_NOT_FOUND."),
        Some("ERROR_PATH_NOT_FOUND")
    );
    assert_eq!(error_token("no token here"), None);
}

#[tokio::test]
async fn eng_156_import_error_end_to_end_cleans_up_only_this_run() {
    let body = payload();
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    h.fake.import_fails(
        -1,
        "The system cannot find the path specified.\nError code: Wsl/ERROR_PATH_NOT_FOUND",
    );
    let error = last_error(&drain(h.setup("alpine")).await);
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("Free up space"))
    );
    assert!(h.cache_files().is_empty());
}

// ── ENG-160 argv and HTTPS only ──────────────────────────────────────────────────────────

#[test]
fn eng_160_https_and_host_allowlist() {
    let ok = |url: &str| url_allowed(&reqwest::Url::parse(url).unwrap());
    assert!(ok("https://github.com/a/b"));
    assert!(ok("https://objects.githubusercontent.com/x"));
    assert!(ok("https://release-assets.githubusercontent.com/x"));
    assert!(!ok("http://github.com/a"));
    assert!(!ok("https://raw.githubusercontent.com/a"));
    assert!(!ok("https://github.com.evil.example/a"));
    assert!(!ok("https://evilgithub.com/a"));
    assert!(!ok("https://127.0.0.1/a"));
}

#[tokio::test]
async fn eng_160_production_client_is_https_only() {
    let server = Server::start(Reply::Body(vec![1, 2, 3])).await;
    let source = Source::production().expect("client");
    // The pinned-URL check refuses it first …
    let pin = Pin {
        url: leak(format!("{}/x.tar.xz", server.base)),
        ..test_pin("alpine", &[1, 2, 3])
    };
    let dir = tempfile::tempdir().unwrap();
    let result = super::download::fetch(&source, &pin, &dir.path().join("a.part"), |_| {}).await;
    assert!(matches!(result, Err(super::download::FetchError::BadUrl)));
    // … and the client itself refuses plain HTTP even if that check were bypassed.
    let direct = source
        .client_for_test()
        .get(format!("{}/x", server.base))
        .send()
        .await;
    assert!(direct.is_err());
    assert_eq!(server.hits(), 0, "nothing was sent");
}

#[tokio::test]
async fn eng_160_argv_only_and_https_only() {
    let body = payload();
    // A redirect to plain http or to a foreign host is refused, and no wsl.exe call follows.
    for target in [
        "http://github.com/x",
        "https://evil.example/x",
        "http://127.0.0.1:9/x",
    ] {
        let h = Harness::new(Reply::Redirect(target.into()), both(&body)).await;
        let items = drain(h.setup("alpine")).await;
        assert!(
            last_error(&items)
                .to_string()
                .contains("download didn't finish")
        );
        assert!(h.fake.calls().is_empty(), "{target}");
        assert!(h.cache_files().is_empty());
    }
    // Every argv of a full run is in the allowed set; a failing run too.
    let h = Harness::new(Reply::Body(body.clone()), both(&body)).await;
    done_config(&drain(h.setup("ubuntu")).await);
    for call in h.fake.calls() {
        assert!(allowed(&call), "{call:?}");
    }
    let h = Harness::with(Reply::Body(body.clone()), both(&body), |env| {
        env.ready_timeout = Duration::from_millis(20);
    })
    .await;
    h.fake.pings(&[false]);
    drain(h.setup("alpine")).await;
    wait_until("the unregister", || h.unregisters() == 1).await;
    for call in h.fake.calls() {
        assert!(allowed(&call), "{call:?}");
        assert!(
            !call
                .iter()
                .any(|arg| arg.contains("install") || arg.contains("--update"))
        );
    }
}

#[test]
fn eng_160_errors_never_carry_the_url() {
    for error in [
        messages::no_network(),
        messages::http_status(404),
        messages::verification_failed(),
    ] {
        let text = format!("{error} {:?}", error.hint());
        assert!(!text.contains("http"), "{text}");
    }
}

// ── live (opt-in) ────────────────────────────────────────────────────────────────────────

/// Real `wsl.exe --import` of a **local** test asset into a disposable distro. Not run in CI.
///
/// Set `DOCKERING_SETUP_LIVE_URL` (`http://127.0.0.1:<port>/<file>` serving the asset),
/// `DOCKERING_SETUP_LIVE_SHA256`, `DOCKERING_SETUP_LIVE_SIZE` (bytes) and
/// `DOCKERING_SETUP_LIVE_DISTRO` (must start with `dockering-setup-test-`). Roots are temp dirs;
/// the distro is unregistered at the end. Never use the name `dockering-engine` here.
#[cfg(windows)]
#[tokio::test]
#[ignore = "live: real wsl.exe --import of a local asset into a disposable distro"]
async fn setup_live() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let url = var("DOCKERING_SETUP_LIVE_URL");
    let distro = var("DOCKERING_SETUP_LIVE_DISTRO");
    assert!(
        distro.starts_with("dockering-setup-test-"),
        "disposable names only"
    );
    let size: u64 = var("DOCKERING_SETUP_LIVE_SIZE").parse().expect("size");
    let origin = reqwest::Url::parse(&url).expect("url");
    let base = format!(
        "{}://{}:{}",
        origin.scheme(),
        origin.host_str().expect("host"),
        origin.port_or_known_default().expect("port")
    );
    let pin = Pin {
        image: "alpine",
        arch: "x86_64",
        label: "Alpine",
        description: "live",
        url: leak(format!(
            "https://github.com/pavel-purma/dockering/releases/download/rootfs-1{}",
            origin.path()
        )),
        size_bytes: size,
        sha256: leak(var("DOCKERING_SETUP_LIVE_SHA256")),
        disk_bytes: 400_000_000,
        published: true,
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let real: Arc<dyn Wsl> = Arc::new(crate::runner::RealWsl);
    let mut env = SetupEnv::new(
        real.clone(),
        tmp.path().join("cache"),
        tmp.path().join("wsl"),
        Box::new(crate::win32::free_space),
    );
    env.distro = distro.clone();
    env.source = Some(Source::with_base(&base).expect("client"));
    env.pins = vec![pin];
    env.arch = "x86_64";
    let factory = WslDistroFactory::with_wsl(real.clone()).with_setup(env);
    let items = drain(factory.setup(OFFER_ID, "alpine")).await;
    let result = items.last().cloned();
    // Always remove the disposable distro, whatever happened.
    let _ = real
        .run_timeout(&["--unregister", &distro], Duration::from_secs(60))
        .await;
    match result {
        Some(Ok(SetupProgress::Done(cfg))) => assert_eq!(cfg.id.as_str(), format!("wsl-{distro}")),
        other => panic!("setup failed: {other:?}"),
    }
}
