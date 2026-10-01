//! bollard error → `EngineError` mapping, with actionable hints (ENG-107).
//!
//! Status mapping (spec 21 §6): 404 → `NotFound`, 409 → `Conflict`, 304 on lifecycle actions
//! → success (bollard already treats 304 as success; [`is_not_modified`] is the safety net),
//! request timeout → `Timeout`, transport failures → `Unreachable { hint }`.

use std::io;
use std::time::Duration;

use bollard::errors::Error as BollardError;
use dk_core::{EngineError, ResourceKind};

/// Where the engine lives, for choosing the right hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HintCtx {
    Unix,
    Pipe,
    Tcp {
        host: String,
        port: u16,
    },
    /// WSL distro reached through the `dk-wsl` pipe bridge.
    WslBridge,
}

/// Static context for error mapping, kept by the engine.
#[derive(Debug, Clone)]
pub(crate) struct ErrCtx {
    pub hint: HintCtx,
    pub timeout: Duration,
}

/// What a 404 refers to.
pub(crate) type NotFoundCtx<'a> = Option<(ResourceKind, &'a str)>;

const HINT_DOCKER_GROUP: &str = "Add your user to the `docker` group: `sudo usermod -aG docker $USER`, then log out and back in";
const HINT_SYSTEMCTL: &str = "Is Docker running? `sudo systemctl start docker`";
const HINT_DESKTOP: &str = "Start Docker Desktop (or your Docker engine)";
const HINT_WSL: &str =
    "Is Docker running inside the WSL distro? Try `sudo service docker start` in the distro";
const HINT_TLS: &str = "Check the TLS CA, certificate, and key files of this engine";

/// Actionable hint for an unreachable engine (ENG-107).
pub(crate) fn unreachable_hint(ctx: &HintCtx, kind: Option<io::ErrorKind>) -> String {
    match ctx {
        HintCtx::Tcp { host, port } => format!(
            "Check that the Docker daemon is listening on {host}:{port} and is reachable from this machine"
        ),
        HintCtx::WslBridge => HINT_WSL.to_owned(),
        HintCtx::Unix | HintCtx::Pipe => {
            if cfg!(target_os = "linux") {
                if kind == Some(io::ErrorKind::PermissionDenied) {
                    HINT_DOCKER_GROUP.to_owned()
                } else {
                    HINT_SYSTEMCTL.to_owned()
                }
            } else {
                HINT_DESKTOP.to_owned()
            }
        }
    }
}

fn find_io_error<'a>(e: &'a (dyn std::error::Error + 'static)) -> Option<&'a io::Error> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<io::Error>() {
            return Some(io);
        }
        cur = err.source();
    }
    None
}

fn unreachable(ctx: &ErrCtx, reason: String, kind: Option<io::ErrorKind>) -> EngineError {
    EngineError::Unreachable {
        reason,
        hint: Some(unreachable_hint(&ctx.hint, kind)),
    }
}

/// Map a bollard error. `nf` names the resource a 404 refers to.
pub(crate) fn map_err(e: BollardError, ctx: &ErrCtx, nf: NotFoundCtx<'_>) -> EngineError {
    match e {
        BollardError::DockerResponseServerError {
            status_code,
            message,
        } => match status_code {
            404 => match nf {
                Some((kind, id)) => EngineError::not_found(kind, id),
                None => EngineError::Api {
                    status: 404,
                    message,
                },
            },
            409 => EngineError::Conflict(message),
            status => EngineError::Api { status, message },
        },
        BollardError::RequestTimeoutError => EngineError::Timeout(ctx.timeout),
        BollardError::SocketNotFoundError(path) => unreachable(
            ctx,
            format!("socket not found: {path}"),
            Some(io::ErrorKind::NotFound),
        ),
        BollardError::JsonDataError { message, .. } => {
            EngineError::protocol(format!("invalid JSON from engine: {message}"))
        }
        BollardError::JsonSerdeError { err } => {
            EngineError::protocol(format!("invalid JSON from engine: {err}"))
        }
        BollardError::APIVersionParseError {} => {
            EngineError::protocol("could not parse the engine API version")
        }
        BollardError::DockerStreamError { error } => EngineError::Api {
            status: 500,
            message: error,
        },
        BollardError::CertPathError { .. }
        | BollardError::CertParseError { .. }
        | BollardError::CertMultipleKeys { .. }
        | BollardError::NoNativeCertsError { .. }
        | BollardError::LoadNativeCertsErrors { .. }
        | BollardError::NoHomePathError => EngineError::Unreachable {
            reason: e.to_string(),
            hint: Some(HINT_TLS.to_owned()),
        },
        other => {
            if let Some(io) = find_io_error(&other) {
                let kind = io.kind();
                let reason = match io.raw_os_error() {
                    // ERROR_PIPE_BUSY
                    Some(231) if cfg!(windows) => "named pipe busy".to_owned(),
                    _ => io.to_string(),
                };
                return unreachable(ctx, reason, Some(kind));
            }
            match &other {
                BollardError::HyperResponseError { .. }
                | BollardError::HyperLegacyError { .. }
                | BollardError::IOError { .. } => unreachable(ctx, other.to_string(), None),
                _ => EngineError::protocol(other.to_string()),
            }
        }
    }
}

/// 304 Not Modified on start/stop/restart/pause/unpause means "already in that state".
pub(crate) fn is_not_modified(e: &BollardError) -> bool {
    matches!(
        e,
        BollardError::DockerResponseServerError {
            status_code: 304,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(hint: HintCtx) -> ErrCtx {
        ErrCtx {
            hint,
            timeout: Duration::from_secs(30),
        }
    }

    fn server(status_code: u16, message: &str) -> BollardError {
        BollardError::DockerResponseServerError {
            status_code,
            message: message.into(),
        }
    }

    #[test]
    fn eng_status_codes_map_per_spec() {
        let c = ctx(HintCtx::Unix);
        assert_eq!(
            map_err(
                server(404, "No such container: x"),
                &c,
                Some((ResourceKind::Container, "x"))
            ),
            EngineError::not_found(ResourceKind::Container, "x")
        );
        assert_eq!(
            map_err(server(404, "page not found"), &c, None),
            EngineError::Api {
                status: 404,
                message: "page not found".into()
            }
        );
        assert_eq!(
            map_err(server(409, "in use"), &c, None),
            EngineError::Conflict("in use".into())
        );
        assert_eq!(
            map_err(server(500, "boom"), &c, None),
            EngineError::Api {
                status: 500,
                message: "boom".into()
            }
        );
        assert_eq!(
            map_err(BollardError::RequestTimeoutError, &c, None),
            EngineError::Timeout(Duration::from_secs(30))
        );
        assert!(is_not_modified(&server(304, "")));
        assert!(!is_not_modified(&server(409, "")));
    }

    #[test]
    fn eng_107_unreachable_hints() {
        let refused = BollardError::IOError {
            err: io::Error::from(io::ErrorKind::ConnectionRefused),
        };
        let e = map_err(refused, &ctx(HintCtx::Unix), None);
        assert!(e.is_unreachable());
        let hint = e.hint().unwrap().to_owned();
        if cfg!(target_os = "linux") {
            assert!(hint.contains("systemctl"));
        } else {
            assert!(hint.contains("Docker Desktop"));
        }

        let denied = BollardError::IOError {
            err: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        let e = map_err(denied, &ctx(HintCtx::Unix), None);
        if cfg!(target_os = "linux") {
            assert!(e.hint().unwrap().contains("usermod -aG docker"));
        }

        let e = map_err(
            BollardError::SocketNotFoundError("/var/run/docker.sock".into()),
            &ctx(HintCtx::Unix),
            None,
        );
        assert!(e.is_unreachable());

        let tcp = HintCtx::Tcp {
            host: "10.0.0.5".into(),
            port: 2376,
        };
        let e = map_err(
            BollardError::IOError {
                err: io::Error::from(io::ErrorKind::TimedOut),
            },
            &ctx(tcp),
            None,
        );
        assert!(e.hint().unwrap().contains("10.0.0.5:2376"));
        let e = map_err(
            BollardError::IOError {
                err: io::Error::from(io::ErrorKind::NotFound),
            },
            &ctx(HintCtx::WslBridge),
            None,
        );
        assert!(e.hint().unwrap().contains("WSL"));
    }

    #[test]
    fn nfr_031_json_errors_are_protocol() {
        let e = map_err(
            BollardError::JsonDataError {
                message: "bad".into(),
                column: 1,
            },
            &ctx(HintCtx::Pipe),
            None,
        );
        assert!(matches!(e, EngineError::Protocol(_)));
    }
}
