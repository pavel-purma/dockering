//! Embedded minisign public keys for the update manifest (UPD-003).

/// Base64 public-key lines (the second line of a minisign `*.pub` file): the **current** release
/// key first, then the **next** one, so a key rotation never strands installed builds.
///
/// Empty until the release keys are generated (`cargo xtask gen-update-keys`, see
/// `keys/README.md`). With no keys every manifest is rejected: the updater fails closed.
pub const PUBLIC_KEYS: &[&str] = &[];
