//! Dockering domain layer: DTOs, the `Engine` trait, capabilities, errors, and pure logic
//! (grouping, stats math, formatting, ANSI parsing, argument validation).
//!
//! Layering (ADR-0001): no tokio, bollard, or GPUI here.

pub mod ansi;
pub mod capabilities;
pub mod docker_json;
pub mod engine;
pub mod error;
pub mod format;
pub mod grouping;
pub mod model;
pub mod secret;
pub mod stats;
pub mod validate;

#[cfg(any(test, feature = "test-support"))]
pub mod contract;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;

pub use capabilities::Capabilities;
pub use engine::{
    DiscoveredEngine, Engine, EngineFactory, EngineStream, ProbeResult, TerminalSession,
    error_stream, unsupported_stream,
};
pub use error::{EngineError, EngineResult};
pub use model::*;
pub use secret::SecretString;
