//! Client construction per target and API version negotiation (spec 20 §3, ENG-010).

use std::time::Duration;

use bollard::{ClientVersion, Docker};
use dk_core::{EngineError, EngineResult};

use crate::DockerTarget;
use crate::errors::{ErrCtx, HintCtx, map_err};

/// Request/response timeout (spec 20 §3).
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Discovery / probe timeout (spec 20 §2).
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Highest API version this client speaks (the version bollard's models describe).
pub(crate) fn client_max() -> ClientVersion {
    *bollard::API_DEFAULT_VERSION
}

pub(crate) fn min_version() -> ClientVersion {
    parse_api_version(crate::MIN_API_VERSION).unwrap_or(ClientVersion {
        major_version: 1,
        minor_version: 41,
    })
}

/// `"1.41"` → `1.41`.
pub(crate) fn parse_api_version(s: &str) -> Option<ClientVersion> {
    let (major, minor) = s.trim().split_once('.')?;
    Some(ClientVersion {
        major_version: major.trim().parse().ok()?,
        minor_version: minor.trim().parse().ok()?,
    })
}

pub(crate) fn version_string(v: &ClientVersion) -> String {
    format!("{}.{}", v.major_version, v.minor_version)
}

/// `Maximum supported API version is 1.43` in a 400 "client version too new" error.
pub(crate) fn max_version_from_error(message: &str) -> Option<ClientVersion> {
    let idx = message.find("Maximum supported API version is")?;
    let rest = &message[idx + "Maximum supported API version is".len()..];
    let token = rest.split_whitespace().next()?;
    parse_api_version(token.trim_end_matches(['.', ',', ')']))
}

pub(crate) fn hint_ctx(target: &DockerTarget, wsl: bool) -> HintCtx {
    if wsl {
        return HintCtx::WslBridge;
    }
    match target {
        DockerTarget::Unix(_) => HintCtx::Unix,
        DockerTarget::NamedPipe(_) => HintCtx::Pipe,
        DockerTarget::Tcp { host, port, .. } => HintCtx::Tcp {
            host: host.clone(),
            port: *port,
        },
    }
}

/// Default `EngineInfo.transport` label for a target.
pub(crate) fn transport_label(target: &DockerTarget) -> &'static str {
    match target {
        DockerTarget::Unix(_) => "unix",
        DockerTarget::NamedPipe(_) => "npipe",
        DockerTarget::Tcp { tls: None, .. } => "tcp",
        DockerTarget::Tcp { tls: Some(_), .. } => "tls",
    }
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Build a bollard client for `target`. Doesn't touch the network.
pub(crate) fn build_client(
    target: &DockerTarget,
    timeout: Duration,
    version: &ClientVersion,
    ctx: &ErrCtx,
) -> EngineResult<Docker> {
    let secs = timeout.as_secs().max(1);
    let res = match target {
        DockerTarget::Unix(path) => {
            #[cfg(unix)]
            {
                Docker::connect_with_unix(&path.to_string_lossy(), secs, version)
            }
            #[cfg(not(unix))]
            {
                let _ = (path, secs, version);
                return Err(EngineError::unreachable(
                    "Unix sockets are not supported on this OS",
                ));
            }
        }
        DockerTarget::NamedPipe(path) => {
            #[cfg(windows)]
            {
                Docker::connect_with_named_pipe(path, secs, version)
            }
            #[cfg(not(windows))]
            {
                let _ = (path, secs, version);
                return Err(EngineError::unreachable(
                    "named pipes are only supported on Windows",
                ));
            }
        }
        DockerTarget::Tcp {
            host,
            port,
            tls: None,
        } => Docker::connect_with_http(&format!("tcp://{}", host_port(host, *port)), secs, version),
        DockerTarget::Tcp {
            host,
            port,
            tls: Some(tls),
        } => Docker::connect_with_ssl(
            &host_port(host, *port),
            &tls.key,
            &tls.cert,
            &tls.ca,
            secs,
            version,
        ),
    };
    res.map_err(|e| map_err(e, ctx, None))
}

/// Result of `/version` + negotiation.
#[derive(Debug, Clone)]
pub(crate) struct Negotiated {
    pub api: ClientVersion,
    pub server_version: Option<String>,
}

/// `min(server ApiVersion, client max)`; error when below 1.41 (ENG-010).
pub(crate) fn negotiate_versions(
    server_api: ClientVersion,
    server_version: Option<String>,
) -> EngineResult<Negotiated> {
    let max = client_max();
    let api = if server_api < max { server_api } else { max };
    if api < min_version() {
        return Err(EngineError::Api {
            status: 0,
            message: format!(
                "unsupported API version {} (need ≥ {})",
                version_string(&server_api),
                crate::MIN_API_VERSION
            ),
        });
    }
    Ok(Negotiated {
        api,
        server_version,
    })
}

/// Call `/version` with the client max and negotiate. Daemons older than the client max
/// reject versioned paths with 400 "client version … is too new. Maximum supported API
/// version is X" — X is then the server version.
pub(crate) async fn negotiate(docker: &Docker, ctx: &ErrCtx) -> EngineResult<Negotiated> {
    match docker.version().await {
        Ok(v) => {
            let server_api = v
                .api_version
                .as_deref()
                .and_then(parse_api_version)
                .ok_or_else(|| EngineError::protocol("/version: missing ApiVersion"))?;
            negotiate_versions(server_api, v.version)
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 400,
            message,
        }) if max_version_from_error(&message).is_some() => {
            let server_api = max_version_from_error(&message).unwrap_or_else(client_max);
            negotiate_versions(server_api, None)
        }
        Err(e) => Err(map_err(e, ctx, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_010_version_parsing_and_negotiation() {
        assert_eq!(
            parse_api_version("1.56"),
            Some(ClientVersion {
                major_version: 1,
                minor_version: 56
            })
        );
        assert_eq!(parse_api_version("x"), None);
        let n = negotiate_versions(parse_api_version("1.56").unwrap(), None).unwrap();
        assert_eq!(n.api, client_max());
        let n = negotiate_versions(parse_api_version("1.43").unwrap(), None).unwrap();
        assert_eq!(version_string(&n.api), "1.43");
        let err = negotiate_versions(parse_api_version("1.40").unwrap(), None).unwrap_err();
        assert_eq!(
            err,
            EngineError::Api {
                status: 0,
                message: "unsupported API version 1.40 (need ≥ 1.41)".into()
            }
        );
    }

    #[test]
    fn eng_010_max_version_from_too_new_error() {
        let msg = "client version 1.53 is too new. Maximum supported API version is 1.43";
        assert_eq!(
            version_string(&max_version_from_error(msg).unwrap()),
            "1.43"
        );
        assert!(max_version_from_error("something else").is_none());
    }

    #[test]
    fn host_port_brackets_ipv6() {
        assert_eq!(host_port("::1", 2375), "[::1]:2375");
        assert_eq!(host_port("example.com", 2376), "example.com:2376");
    }
}
