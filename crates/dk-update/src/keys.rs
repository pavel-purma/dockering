//! Embedded minisign public keys for the update manifest (UPD-003).

/// Base64 public-key lines (the second line of a minisign `*.pub` file): the **current** release
/// key first, then the **next** one, so a key rotation never strands installed builds.
///
/// Empty while releases ship an unsigned manifest (ADR-0007, 2026-11-16): the manifest is then
/// accepted as served and each asset's size, SHA-256 and pinned GitHub URL are the trust. Once
/// keys are listed (`cargo xtask gen-update-keys`, see `keys/README.md`) a valid signature from
/// one of them is mandatory.
pub const PUBLIC_KEYS: &[&str] = &[];
