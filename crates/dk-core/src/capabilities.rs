//! Capability flags (spec 21 §2). The UI gates every non-universal op on these.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct Capabilities: u32 {
        /// Live event stream.
        const EVENTS          = 1 << 0;
        const PAUSE           = 1 << 1;
        /// Interactive terminal.
        const EXEC_TTY        = 1 << 2;
        const EXEC_RESIZE     = 1 << 3;
        /// Native streaming stats (else polled).
        const STATS_STREAM    = 1 << 4;
        const TOP             = 1 << 5;
        const IMAGE_HISTORY   = 1 << 6;
        const DISK_USAGE      = 1 << 7;
        /// Structured per-layer pull progress.
        const PULL_PROGRESS   = 1 << 8;
        const NETWORK_MGMT    = 1 << 9;
        const LOGS_FOLLOW     = 1 << 10;
    }
}

impl Capabilities {
    /// Full Docker Engine API (>= 1.41) capability set.
    pub const DOCKER: Capabilities = Capabilities::all();

    /// Human-readable names of the set flags, for diagnostics.
    pub fn names(self) -> Vec<&'static str> {
        self.iter_names().map(|(n, _)| n).collect()
    }
}
