//! Registry credentials from the user's Docker config (IMG-007). Never logged (NFR-020).
//! SKELETON — `engine-integrator`.

use dk_core::RegistryAuth;

/// Resolve credentials for the registry of `image_ref` from `~/.docker/config.json`
/// (`auths`, `credsStore`, `credHelpers` via `docker-credential-<helper> get`, argv, 10 s
/// timeout). `None` → anonymous pull.
pub async fn resolve_auth(image_ref: &str) -> Option<RegistryAuth> {
    let _ = image_ref;
    unimplemented!("engine-integrator")
}

/// Registry host of an image reference (`docker.io` for Hub images).
pub fn registry_of(image_ref: &str) -> String {
    let _ = image_ref;
    unimplemented!("engine-integrator")
}
