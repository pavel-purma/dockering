//! Per-kind argument validation (NFR-022). Every value passed in an argv or URL path is
//! checked. Errors are `EngineError::Api { status: 400, .. }`.
//! SIGNATURES FIXED — bodies by `rust-core`.

use crate::error::EngineResult;

/// Container/volume/network names: `^[a-zA-Z0-9][a-zA-Z0-9_.-]*$`.
pub fn validate_name(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}

/// Hex ids (full or prefix), optionally `sha256:`-prefixed.
pub fn validate_id(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}

/// Either a valid id or a valid name (ops accept both).
pub fn validate_id_or_name(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}

/// OCI image reference grammar (`/`, `:`, `@` allowed), never starting with `-`.
pub fn validate_image_ref(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}

/// Signal names/numbers for kill (`SIGKILL`, `KILL`, `9`).
pub fn validate_signal(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}

/// Env/label key: non-empty, no `=`, no whitespace/control chars, not starting with `-`.
pub fn validate_env_key(s: &str) -> EngineResult<()> {
    let _ = s;
    unimplemented!("rust-core")
}
