//! Engine errors (spec 10 §5).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capabilities::Capabilities;
use crate::model::ResourceKind;

pub type EngineResult<T> = Result<T, EngineError>;

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
            _ => None,
        }
    }

    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Protocol(format!("invalid JSON: {e}"))
    }
}
