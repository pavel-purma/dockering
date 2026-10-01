//! Mappers from Docker-Engine-API-shaped JSON (`serde_json::Value`) to DTOs. Shared by
//! `dk-engine-docker` and `dk-engine-wslc` (WSLC COM/CLI return Docker-shaped JSON for
//! inspect, stats, volumes, and networks). Tolerant (NFR-031): never panic, unknown → default.
//!
//! Owned by `engine-integrator`.
