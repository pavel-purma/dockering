# Architecture Decision Records

| ADR | Title | Status |
|---|---|---|
| [0001](0001-workspace-and-layering.md) | Cargo workspace and layering | accepted |
| [0002](0002-async-hub-runtime.md) | Dedicated tokio "hub" runtime bridged to GPUI | accepted |
| [0003](0003-wslc-transport.md) | WSLC via native COM (internal session API) with CLI fallback | accepted, validated by S-3 on WSL 3.0.1 |
| [0004](0004-wsl-distro-bridge.md) | WSL distro Docker via named-pipe ⇄ `wsl.exe` stdio bridge | accepted (validate S-4) |
| [0005](0005-backend-extensibility-apple-container.md) | Backend extensibility; Apple `container` (macOS native) deferred | accepted / backend deferred |
| [0006](0006-windows-installer-and-updates.md) | Windows installer (Inno Setup), signing, and in-app updates from GitHub Releases | proposed |

New ADRs: copy the structure (Context, Options, Decision, Consequences), number them sequentially, and never delete
an ADR. Supersede it instead.
