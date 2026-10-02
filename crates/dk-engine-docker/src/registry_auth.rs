//! Registry credentials from the user's Docker config (IMG-007). Never logged (NFR-020).
//!
//! Lookup order (Docker CLI semantics): `credHelpers[registry]`, then `credsStore`, run as
//! `docker-credential-<helper> get` (argv only, stdin = server, 10 s timeout); then the
//! inline `auths[key]` entry (`auth` = base64 `user:pass`, or `identitytoken`).

use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use dk_core::{RegistryAuth, SecretString};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HELPER_TIMEOUT: Duration = Duration::from_secs(10);
const HUB: &str = "docker.io";
const HUB_LEGACY_KEY: &str = "https://index.docker.io/v1/";

/// Resolve credentials for the registry of `image_ref` from `~/.docker/config.json`
/// (`auths`, `credsStore`, `credHelpers` via `docker-credential-<helper> get`, argv, 10 s
/// timeout). `None` → anonymous pull.
pub async fn resolve_auth(image_ref: &str) -> Option<RegistryAuth> {
    let path = docker_config_dir()?.join("config.json");
    let text = tokio::fs::read_to_string(&path).await.ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    let registry = registry_of(image_ref);
    resolve_from_config(&config, &registry, run_helper).await
}

/// Registry host of an image reference (`docker.io` for Hub images).
pub fn registry_of(image_ref: &str) -> String {
    let name = image_ref.split('@').next().unwrap_or(image_ref);
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            let host = first.to_ascii_lowercase();
            match host.as_str() {
                "index.docker.io" | "registry-1.docker.io" | "docker.io" => HUB.to_owned(),
                _ => host,
            }
        }
        _ => HUB.to_owned(),
    }
}

/// `$DOCKER_CONFIG`, else `~/.docker`.
pub(crate) fn docker_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DOCKER_CONFIG").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|h| h.join(".docker"))
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Config keys under which credentials for `registry` may be stored.
pub(crate) fn lookup_keys(registry: &str) -> Vec<String> {
    let mut keys = vec![registry.to_owned(), format!("https://{registry}")];
    if registry == HUB {
        keys.push(HUB_LEGACY_KEY.to_owned());
        keys.push("index.docker.io".to_owned());
        keys.push("https://index.docker.io".to_owned());
        keys.push("registry-1.docker.io".to_owned());
    }
    keys
}

/// Server string handed to a credential helper (what `docker login` stored).
fn helper_servers(registry: &str) -> Vec<String> {
    if registry == HUB {
        vec![HUB_LEGACY_KEY.to_owned()]
    } else {
        vec![registry.to_owned(), format!("https://{registry}")]
    }
}

fn valid_helper_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

/// The credential helper responsible for `registry`, if any (validated `[a-z0-9-]+`).
pub(crate) fn helper_for(config: &Value, registry: &str) -> Option<String> {
    let from_helpers = config
        .get("credHelpers")
        .and_then(Value::as_object)
        .and_then(|helpers| {
            lookup_keys(registry)
                .iter()
                .find_map(|k| helpers.get(k).and_then(Value::as_str))
        });
    let name = from_helpers.or_else(|| config.get("credsStore").and_then(Value::as_str))?;
    valid_helper_name(name).then(|| name.to_owned())
}

/// Inline `auths` entry for `registry`.
pub(crate) fn auth_from_auths(config: &Value, registry: &str) -> Option<RegistryAuth> {
    let auths = config.get("auths").and_then(Value::as_object)?;
    for key in lookup_keys(registry) {
        let Some(entry) = auths.get(&key) else {
            continue;
        };
        let identity = entry
            .get("identitytoken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let basic = entry
            .get("auth")
            .and_then(Value::as_str)
            .and_then(base64_decode)
            .and_then(|raw| String::from_utf8(raw).ok())
            .and_then(|s| s.split_once(':').map(|(u, p)| (u.to_owned(), p.to_owned())));
        if identity.is_none() && basic.is_none() {
            continue;
        }
        let (username, password) = match basic {
            Some((u, p)) => (
                Some(u).filter(|u| !u.is_empty()),
                Some(SecretString::new(p)),
            ),
            None => (None, None),
        };
        return Some(RegistryAuth {
            server: key,
            username,
            password: password.filter(|p| !p.is_empty()),
            identity_token: identity.map(SecretString::new),
        });
    }
    None
}

/// Parse `docker-credential-* get` stdout: `{"ServerURL","Username","Secret"}`.
/// Username `<token>` means `Secret` is an identity token.
pub(crate) fn parse_helper_output(server: &str, stdout: &[u8]) -> Option<RegistryAuth> {
    let v: Value = serde_json::from_slice(stdout).ok()?;
    let username = v.get("Username").and_then(Value::as_str).unwrap_or("");
    let secret = v.get("Secret").and_then(Value::as_str).unwrap_or("");
    if secret.is_empty() {
        return None;
    }
    let server = v
        .get("ServerURL")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(server)
        .to_owned();
    if username == "<token>" {
        return Some(RegistryAuth {
            server,
            username: None,
            password: None,
            identity_token: Some(SecretString::new(secret)),
        });
    }
    Some(RegistryAuth {
        server,
        username: Some(username.to_owned()).filter(|u| !u.is_empty()),
        password: Some(SecretString::new(secret)),
        identity_token: None,
    })
}

/// Pure resolution over a parsed config; `run` executes `(helper, server) → stdout`.
pub(crate) async fn resolve_from_config<F, Fut>(
    config: &Value,
    registry: &str,
    run: F,
) -> Option<RegistryAuth>
where
    F: Fn(String, String) -> Fut,
    Fut: Future<Output = Option<Vec<u8>>>,
{
    if let Some(helper) = helper_for(config, registry) {
        for server in helper_servers(registry) {
            if let Some(out) = run(helper.clone(), server.clone()).await
                && let Some(auth) = parse_helper_output(&server, &out)
            {
                return Some(auth);
            }
        }
    }
    auth_from_auths(config, registry)
}

/// `docker-credential-<helper> get` with `server` on stdin. argv only; output never logged.
async fn run_helper(helper: String, server: String) -> Option<Vec<u8>> {
    if !valid_helper_name(&helper) {
        return None;
    }
    let program = format!("docker-credential-{helper}");
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let fut = async move {
        let mut child = cmd.spawn().ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(server.as_bytes()).await.ok()?;
            drop(stdin);
        }
        let mut out = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_end(&mut out).await.ok()?;
        }
        let status = child.wait().await.ok()?;
        status.success().then_some(out)
    };
    match tokio::time::timeout(HELPER_TIMEOUT, fut).await {
        Ok(out) => out,
        Err(_) => {
            tracing::warn!(helper = %program, "credential helper timed out");
            None
        }
    }
}

/// Standard / URL-safe base64, padding optional, whitespace ignored.
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn img_007_registry_of() {
        assert_eq!(registry_of("nginx"), "docker.io");
        assert_eq!(registry_of("library/nginx:1"), "docker.io");
        assert_eq!(registry_of("me/app"), "docker.io");
        assert_eq!(registry_of("docker.io/library/nginx"), "docker.io");
        assert_eq!(registry_of("index.docker.io/me/app"), "docker.io");
        assert_eq!(registry_of("ghcr.io/owner/app:v1"), "ghcr.io");
        assert_eq!(registry_of("localhost:5000/app"), "localhost:5000");
        assert_eq!(registry_of("localhost/app"), "localhost");
        assert_eq!(
            registry_of("Registry.Example.com/a@sha256:00"),
            "registry.example.com"
        );
    }

    #[test]
    fn img_007_base64() {
        assert_eq!(base64_decode("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert_eq!(base64_decode("dXNlcjpwYXNzMQ==").unwrap(), b"user:pass1");
        assert_eq!(base64_decode("dXNlcjpwYXNzMTI").unwrap(), b"user:pass12");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert!(base64_decode("***").is_none());
    }

    #[test]
    fn img_007_auths_entries() {
        let cfg = json!({"auths": {
            "https://index.docker.io/v1/": {"auth": "dXNlcjpwYXNz"},
            "ghcr.io": {"identitytoken": "tok"},
            "empty.io": {}
        }});
        let a = auth_from_auths(&cfg, "docker.io").unwrap();
        assert_eq!(a.server, "https://index.docker.io/v1/");
        assert_eq!(a.username.as_deref(), Some("user"));
        assert_eq!(a.password.as_ref().unwrap().expose(), "pass");
        let g = auth_from_auths(&cfg, "ghcr.io").unwrap();
        assert_eq!(g.identity_token.as_ref().unwrap().expose(), "tok");
        assert!(auth_from_auths(&cfg, "empty.io").is_none());
        assert!(auth_from_auths(&cfg, "quay.io").is_none());
        assert!(auth_from_auths(&json!({"auths": []}), "quay.io").is_none());
    }

    #[test]
    fn img_007_helper_selection() {
        let cfg = json!({"credsStore": "desktop", "credHelpers": {"gcr.io": "gcloud", "evil.io": "x;rm -rf"}});
        assert_eq!(helper_for(&cfg, "docker.io").as_deref(), Some("desktop"));
        assert_eq!(helper_for(&cfg, "gcr.io").as_deref(), Some("gcloud"));
        assert_eq!(helper_for(&cfg, "evil.io"), None);
        assert_eq!(helper_for(&json!({}), "docker.io"), None);
        assert_eq!(
            helper_for(&json!({"credsStore": "../bin/sh"}), "docker.io"),
            None
        );
    }

    #[test]
    fn img_007_parse_helper_output() {
        let a = parse_helper_output(
            "ghcr.io",
            br#"{"ServerURL":"ghcr.io","Username":"me","Secret":"pw"}"#,
        )
        .unwrap();
        assert_eq!(a.username.as_deref(), Some("me"));
        assert_eq!(a.password.unwrap().expose(), "pw");
        let t = parse_helper_output("x", br#"{"Username":"<token>","Secret":"t"}"#).unwrap();
        assert_eq!(t.server, "x");
        assert_eq!(t.identity_token.unwrap().expose(), "t");
        assert!(t.username.is_none());
        assert!(parse_helper_output("x", b"credentials not found").is_none());
        assert!(parse_helper_output("x", br#"{"Username":"u","Secret":""}"#).is_none());
    }

    #[test]
    fn img_007_resolution_order() {
        let cfg = json!({"credsStore": "desktop", "auths": {"https://index.docker.io/v1/": {"auth": "dXNlcjpwYXNz"}}});
        // Helper wins.
        let a = futures::executor::block_on(resolve_from_config(
            &cfg,
            "docker.io",
            |h, s| async move {
                assert_eq!(h, "desktop");
                assert_eq!(s, "https://index.docker.io/v1/");
                Some(br#"{"Username":"helper","Secret":"s"}"#.to_vec())
            },
        ))
        .unwrap();
        assert_eq!(a.username.as_deref(), Some("helper"));
        // Helper has nothing → inline auths.
        let b = futures::executor::block_on(resolve_from_config(&cfg, "docker.io", |_, _| async {
            None
        }))
        .unwrap();
        assert_eq!(b.username.as_deref(), Some("user"));
        // Nothing anywhere → anonymous.
        assert!(
            futures::executor::block_on(resolve_from_config(&cfg, "quay.io", |_, _| async {
                None
            }))
            .is_none()
        );
    }

    #[test]
    fn nfr_020_debug_hides_credentials() {
        let a = parse_helper_output("x", br#"{"Username":"me","Secret":"hunter2"}"#).unwrap();
        let dbg = format!("{a:?}");
        assert!(!dbg.contains("hunter2"));
        assert!(!dbg.contains("me"));
    }
}
