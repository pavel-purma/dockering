# Feature: macOS native engine (Apple `container`)

- **Status:** deferred <!-- researched; not scheduled for v1 -->
- **Requirement prefix:** ENG (ENG-030…033 are binding in v1; ENG-120…125 are future)
- **Backend spec:** [20-engine-backends.md §7](../20-engine-backends.md#7-future-backend-apple-container-macos-native-deferred-not-in-v1)
- **Decision:** [ADR-0005](../../plan/adr/0005-backend-extensibility-apple-container.md)

## Why deferred

v1 already covers macOS through Docker-compatible engines (Docker Desktop, Colima, OrbStack,
Rancher Desktop, Podman). Apple `container` needs a new XPC transport, and its model differs from
Docker (a VM per container, no Compose, macOS 26 + Apple silicon only). It's planned as a
post-v1 backend once the core app ships.

## Binding in v1 (architecture must allow it)

| ID | Requirement |
|---|---|
| ENG-030 | `EngineKind` is `#[non_exhaustive]` with `AppleContainer` reserved. UI behaviour depends only on `Capabilities` and `EngineInfo`. |
| ENG-031 | DTOs tolerate missing engine-wide facts (`Option`s) and carry per-container IPs. |
| ENG-032 | Backends register via `EngineFactory` (spec 21 §8). Adding one requires no UI crate changes beyond icons and strings. |
| ENG-033 | Discovery is per factory. On macOS, v1 registers only `DockerFactory`. |

## Future requirements (when scheduled)

| ID | Requirement |
|---|---|
| ENG-120 | On macOS 26+ / Apple silicon with `container` installed, Dockering discovers it and lists it in the switcher group *macOS containers*. |
| ENG-121 | If the `container-apiserver` isn't running, show *Start system service*. It runs `container system start` after confirmation, and is never started silently. |
| ENG-122 | Primary transport is XPC to `com.apple.container.apiserver`, gated by version + self-check. Fallback is the `container` CLI with `--format json`. Transport selection works like WSLC (ENG-013/110). |
| ENG-123 | Containers, images, volumes, and networks (macOS 26) are listed and managed through the standard pages, with the capabilities from spec 20 §7.3. |
| ENG-124 | Container detail shows the container IP(s) prominently, since there's no shared host. *Boot logs* MAY be added as a Logs sub-view. |
| ENG-125 | Terminal uses XPC `containerCreateProcess` + `containerResize`, or `container exec -it` under a PTY in fallback. |

## Open questions (resolve in spikes S-6/S-7)

- Semantics of the `containerEvent` route: is it a subscription or a per-container wait?
- Whether image operations over the non-public image helper are worth it, compared with the CLI.
- How published ports and per-container IPs should be shown (CON-002 Ports column).
