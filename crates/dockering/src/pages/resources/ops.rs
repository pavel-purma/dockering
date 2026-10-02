//! Engine operations shared by the Images, Volumes and Networks pages: parallel removal with
//! one outcome summary (limit 4, like CON-013), pruning, and small pure helpers. The pages
//! spawn the returned `HubCall`s and report through notifications.

use std::sync::Arc;

use dk_core::{Engine, EngineError, EngineId, EngineResult};
use dk_hub::{HubCall, HubHandle};
use futures::StreamExt;
use futures::future::BoxFuture;

/// Parallelism for bulk operations (spec 21 §4 rule 5).
pub const PARALLEL: usize = 4;

/// A removal target: `(engine argument, display label)`.
pub type Target = (String, String);

/// Per-target outcome: `(engine argument, label, result)`.
pub type Outcome = Vec<(String, String, Result<(), EngineError>)>;

type RemoveFn =
    Arc<dyn Fn(Arc<dyn Engine>, String) -> BoxFuture<'static, EngineResult<()>> + Send + Sync>;

/// Runs `remove` for every target on the hub runtime, `PARALLEL` at a time.
pub fn remove_many(
    hub: &HubHandle,
    engine: &EngineId,
    targets: Vec<Target>,
    remove: impl Fn(Arc<dyn Engine>, String) -> BoxFuture<'static, EngineResult<()>>
    + Send
    + Sync
    + 'static,
) -> HubCall<Outcome> {
    let remove: RemoveFn = Arc::new(remove);
    hub.call(engine, move |e| async move {
        let out: Outcome = futures::stream::iter(targets)
            .map(|(arg, label)| {
                let e = e.clone();
                let remove = remove.clone();
                async move {
                    let r = remove(e, arg.clone()).await;
                    (arg, label, r)
                }
            })
            .buffer_unordered(PARALLEL)
            .collect()
            .await;
        Ok(out)
    })
}

/// `(succeeded, [(label, error)])`.
pub fn summarize(outcome: &Outcome) -> (usize, Vec<(String, EngineError)>) {
    let ok = outcome.iter().filter(|o| o.2.is_ok()).count();
    let failed = outcome
        .iter()
        .filter_map(|o| o.2.as_ref().err().map(|e| (o.1.clone(), e.clone())))
        .collect();
    (ok, failed)
}

/// A removal refused because the resource is in use (HTTP 409 and equivalents).
pub fn is_in_use(err: &EngineError) -> bool {
    match err {
        EngineError::Conflict(_) => true,
        EngineError::Api { status, message } => {
            *status == 409 || message.to_ascii_lowercase().contains("in use")
        }
        _ => false,
    }
}

/// The registry host of an image reference (`docker.io` for Docker Hub references), for
/// the IMG-007 "docker login <registry>" hint.
pub fn registry_of(reference: &str) -> String {
    let name = reference.split('@').next().unwrap_or(reference);
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            first.to_ascii_lowercase()
        }
        _ => "docker.io".to_owned(),
    }
}

/// IMG-007: a 401 from a pull becomes "Authentication required: run `docker login <registry>`"
/// (the Docker backend already phrases it like that; other engines may not).
pub fn pull_error_message(reference: &str, err: &EngineError) -> String {
    match err {
        EngineError::Api {
            status: 401,
            message,
        } if message.contains("docker login") => message.clone(),
        EngineError::Api { status: 401, .. } => format!(
            "Authentication required: run `docker login {}`",
            registry_of(reference)
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_007_registry_and_auth_message() {
        assert_eq!(registry_of("nginx:latest"), "docker.io");
        assert_eq!(registry_of("library/nginx"), "docker.io");
        assert_eq!(registry_of("ghcr.io/owner/app:1"), "ghcr.io");
        assert_eq!(registry_of("localhost:5000/app"), "localhost:5000");
        let e = EngineError::Api {
            status: 401,
            message: "unauthorized".into(),
        };
        assert_eq!(
            pull_error_message("ghcr.io/o/a", &e),
            "Authentication required: run `docker login ghcr.io`"
        );
        let e = EngineError::Api {
            status: 401,
            message: "Authentication required: run `docker login x.io`".into(),
        };
        assert_eq!(
            pull_error_message("x.io/a", &e),
            "Authentication required: run `docker login x.io`"
        );
    }

    #[test]
    fn in_use_detection() {
        assert!(is_in_use(&EngineError::Conflict("busy".into())));
        assert!(is_in_use(&EngineError::Api {
            status: 409,
            message: String::new()
        }));
        assert!(!is_in_use(&EngineError::Cancelled));
    }
}
