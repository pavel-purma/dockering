//! Engine errors (spec 10 §5).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capabilities::Capabilities;
use crate::model::ResourceKind;

pub type EngineResult<T> = Result<T, EngineError>;

/// Message of the recoverable `EngineError::Protocol` an event stream yields when the engine
/// dropped events (WSLC `WSLC_E_EVENTS_LOST`, spec 20 §5.4). The stream continues; consumers
/// must refetch. Build it with [`EngineError::events_lost`], test with
/// [`EngineError::is_events_lost`].
pub const EVENTS_LOST_MESSAGE: &str = "events lost";

/// ENG-137: hint of a missing WSL containers session.
pub const SESSION_NOT_FOUND_HINT: &str = "That WSL containers session doesn't exist. Check the session name in Settings \u{2192} Engines, or use the default session.";

#[derive(thiserror::Error, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineError {
    #[error("engine unreachable: {reason}")]
    Unreachable {
        reason: String,
        /// Actionable text, e.g. "Is Docker running? `sudo systemctl start docker`".
        hint: Option<String>,
    },
    #[error("{kind} '{id}' not found")]
    NotFound { kind: ResourceKind, id: String },
    /// HTTP 409 and equivalents.
    #[error("conflict: {0}")]
    Conflict(String),
    /// `cap` is a single capability flag.
    #[error("not supported by this engine: {0:?}")]
    Unsupported(Capabilities),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("engine API error {status}: {message}")]
    Api { status: u16, message: String },
    /// Bad JSON, unexpected CLI output, a panicking engine future (NFR-030).
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

impl EngineError {
    pub fn unreachable(reason: impl Into<String>) -> Self {
        Self::Unreachable {
            reason: reason.into(),
            hint: None,
        }
    }

    pub fn unreachable_with_hint(reason: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::Unreachable {
            reason: reason.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn not_found(kind: ResourceKind, id: impl Into<String>) -> Self {
        Self::NotFound {
            kind,
            id: id.into(),
        }
    }

    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }

    /// Actionable hint, if any (ENG-107).
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Unreachable { hint, .. } => hint.as_deref(),
            Self::NotFound {
                kind: ResourceKind::Session,
                ..
            } => Some(SESSION_NOT_FOUND_HINT),
            _ => None,
        }
    }

    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }

    /// The recoverable "events lost" item of an event stream (spec 20 §5.4).
    pub fn events_lost() -> Self {
        Self::Protocol(EVENTS_LOST_MESSAGE.to_owned())
    }

    /// Whether this is the recoverable "events lost" item of an event stream (spec 20 §5.4):
    /// the stream continues and the consumer must refetch.
    pub fn is_events_lost(&self) -> bool {
        matches!(self, Self::Protocol(m) if m == EVENTS_LOST_MESSAGE)
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Protocol(format!("invalid JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_137_missing_session_has_a_hint_other_not_found_does_not() {
        let e = EngineError::not_found(ResourceKind::Session, "x");
        assert_eq!(e.hint(), Some(SESSION_NOT_FOUND_HINT));
        assert!(!SESSION_NOT_FOUND_HINT.contains("COM"));
        assert_eq!(
            EngineError::not_found(ResourceKind::Container, "x").hint(),
            None
        );
    }

    #[test]
    fn events_lost_is_recognised_exactly() {
        assert!(EngineError::events_lost().is_events_lost());
        assert!(EngineError::protocol(EVENTS_LOST_MESSAGE).is_events_lost());
        assert!(!EngineError::protocol("events lost: more").is_events_lost());
        assert!(!EngineError::protocol("boom").is_events_lost());
        assert!(!EngineError::unreachable(EVENTS_LOST_MESSAGE).is_events_lost());
    }
}
