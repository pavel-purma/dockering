//! Per-kind argument validation (NFR-022). Every value passed in an argv or URL path is
//! checked. Errors are `EngineError::Api { status: 400, .. }`.

use crate::error::{EngineError, EngineResult};

fn invalid(what: &str, s: &str) -> EngineError {
    // Truncate: the value may be user input; keep errors short and single-line.
    let shown: String = s.chars().take(80).flat_map(char::escape_default).collect();
    EngineError::Api {
        status: 400,
        message: format!("invalid {what}: \"{shown}\""),
    }
}

/// Container/volume/network names: `^[a-zA-Z0-9][a-zA-Z0-9_.-]*$`.
pub fn validate_name(s: &str) -> EngineResult<()> {
    let mut chars = s.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && s.len() <= 255;
    if ok { Ok(()) } else { Err(invalid("name", s)) }
}

/// Hex ids (full or prefix), optionally `sha256:`-prefixed.
pub fn validate_id(s: &str) -> EngineResult<()> {
    let hex = s.strip_prefix("sha256:").unwrap_or(s);
    if !hex.is_empty() && hex.len() <= 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid("id", s))
    }
}

/// Either a valid id or a valid name (ops accept both).
pub fn validate_id_or_name(s: &str) -> EngineResult<()> {
    validate_id(s)
        .or_else(|_| validate_name(s))
        .map_err(|_| invalid("id or name", s))
}

/// OCI image reference grammar (`/`, `:`, `@` allowed), never starting with `-`.
///
/// Accepts `[registry[:port]/]path[:tag][@algo:hex]` and bare image ids.
pub fn validate_image_ref(s: &str) -> EngineResult<()> {
    if validate_id(s).is_ok() {
        return Ok(());
    }
    let err = || invalid("image reference", s);
    if s.is_empty() || s.len() > 512 || s.starts_with('-') {
        return Err(err());
    }
    let (name_tag, digest) = match s.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (s, None),
    };
    if let Some(d) = digest {
        let (algo, hex) = d.split_once(':').ok_or_else(err)?;
        let algo_ok = !algo.is_empty()
            && algo
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+._-".contains(c));
        if !algo_ok || hex.len() < 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
    }
    // Tag: after the last ':' that follows the last '/'.
    let last_slash = name_tag.rfind('/').map_or(0, |i| i + 1);
    let (name, tag) = match name_tag[last_slash..].rfind(':') {
        Some(i) => (&name_tag[..last_slash + i], Some(&name_tag[last_slash + i + 1..])),
        None => (name_tag, None),
    };
    if let Some(tag) = tag {
        let mut tc = tag.chars();
        let ok = matches!(tc.next(), Some(c) if c.is_ascii_alphanumeric() || c == '_')
            && tc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            && tag.len() <= 128;
        if !ok {
            return Err(err());
        }
    }
    if name.is_empty() {
        return Err(err());
    }
    for (i, comp) in name.split('/').enumerate() {
        if comp.is_empty() {
            return Err(err());
        }
        let is_registry = i == 0 && name.contains('/') && (comp.contains('.') || comp.contains(':') || comp == "localhost");
        let ok = if is_registry {
            comp.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        } else {
            comp.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            }) && comp.starts_with(|c: char| c.is_ascii_alphanumeric())
                && comp.ends_with(|c: char| c.is_ascii_alphanumeric())
        };
        if !ok {
            return Err(err());
        }
    }
    Ok(())
}

/// Signal names/numbers for kill (`SIGKILL`, `KILL`, `9`).
pub fn validate_signal(s: &str) -> EngineResult<()> {
    let ok = !s.is_empty()
        && s.len() <= 16
        && (s.bytes().all(|b| b.is_ascii_digit())
            || s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'+' || b == b'-')
                && s.starts_with(|c: char| c.is_ascii_uppercase()));
    if ok { Ok(()) } else { Err(invalid("signal", s)) }
}

/// Env/label key: non-empty, no `=`, no whitespace/control chars, not starting with `-`.
pub fn validate_env_key(s: &str) -> EngineResult<()> {
    let ok = !s.is_empty()
        && !s.starts_with('-')
        && s.len() <= 1024
        && !s.chars().any(|c| c == '=' || c.is_whitespace() || c.is_control());
    if ok { Ok(()) } else { Err(invalid("key", s)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_022_names() {
        for ok in ["web-1", "a", "my_app.v2", "0abc"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-rm", ".x", "a b", "a/b", "a;rm -rf", "é"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nfr_022_ids() {
        assert!(validate_id("a1b2c3").is_ok());
        assert!(validate_id(&"f".repeat(64)).is_ok());
        assert!(validate_id(&format!("sha256:{}", "a".repeat(64))).is_ok());
        for bad in ["", "xyz", "-1", &"a".repeat(65)] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nfr_022_image_refs() {
        for ok in [
            "nginx",
            "nginx:1.27",
            "library/nginx:latest",
            "ghcr.io/owner/app:v1.2.3",
            "localhost:5000/app",
            "localhost:5000/app:dev",
            "registry.example.com:443/a/b/c:tag_1",
            "alpine@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            assert!(validate_image_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-it", "--rm", "Nginx", "nginx:", "a//b", "nginx:la test", "nginx;ls", "a@sha256:zz"] {
            assert!(validate_image_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nfr_022_signals_and_keys() {
        for ok in ["SIGKILL", "KILL", "9", "SIGRTMIN+3"] {
            assert!(validate_signal(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-9", "kill", "SIG KILL"] {
            assert!(validate_signal(bad).is_err(), "{bad}");
        }
        assert!(validate_env_key("DB_PASSWORD").is_ok());
        assert!(validate_env_key("com.docker.compose.project").is_ok());
        for bad in ["", "-x", "A=B", "A B"] {
            assert!(validate_env_key(bad).is_err(), "{bad}");
        }
    }
}
