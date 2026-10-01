//! `wslc.exe` CLI fallback transport (spec 20 §5.5). Owner: `engine-integrator`.
//! PUBLIC (crate) API FIXED — the factory (owned by `windows-platform`) calls these.

use dk_core::{EngineId, EngineResult};

/// `Engine` implementation over `wslc.exe`.
pub struct WslcCliEngine {
    _private: (),
}

impl WslcCliEngine {
    /// Verify `wslc.exe` works for `session` (None = the caller's default session) and build
    /// the engine. `transport_note` explains why the CLI is used (ENG-110 chip), e.g.
    /// "WSL 3.1.0 not yet verified — using CLI". `wsl_version` is shown in diagnostics.
    pub async fn connect(
        id: EngineId,
        session: Option<String>,
        wsl_version: Option<String>,
        transport_note: Option<String>,
    ) -> EngineResult<WslcCliEngine> {
        let _ = (id, session, wsl_version, transport_note);
        Err(dk_core::EngineError::unreachable("WSLC CLI transport not implemented yet"))
    }
}

/// `wslc system session list` → display names (table output, F-10).
pub async fn list_sessions() -> EngineResult<Vec<String>> {
    Ok(Vec::new())
}

/// Path of `wslc.exe` if present (`%ProgramFiles%\WSL\wslc.exe`, then `PATH`).
pub fn wslc_exe() -> Option<std::path::PathBuf> {
    None
}
