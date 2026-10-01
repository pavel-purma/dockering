# ADR-0005: Backend extensibility and the Apple `container` runtime

- **Status:** accepted (extensibility) · Apple backend **deferred** · 2026-10-01

## Context

Dockering's UI must work with several container runtimes that differ in protocol and model:

| Runtime | Protocol | Model |
|---|---|---|
| Docker Engine (Linux, Docker Desktop, Colima, OrbStack, Podman) | Docker Engine HTTP API | shared VM/host, Compose labels |
| Docker in a WSL distro | Docker API via the `wsl.exe` bridge | same as Docker |
| WSLC | Internal COM (`IWSLCSession`) / `wslc.exe` | per-session VM, dockerd inside, no exposed API |
| **Apple `container`** (macOS 26, Apple silicon) | XPC to `com.apple.container.apiserver` / `container` CLI | **one VM per container**, no Docker API, no Compose |

Apple `container` (`apple/container` @ `0a48a1b`, release 1.5.0) documents that its
`container-apiserver` XPC API "preserves forward and backward compatibility within a major
version". The image helper and other XPC helpers make no such promise. The CLI supports
`--format json` for its list commands.

## Decision

1. **One contract, many backends.** The UI and hub depend only on `dk_core::Engine` +
   `Capabilities`. Each runtime is a separate crate with an `EngineFactory` (spec 21 §0, §8). The
   transports inside a backend (COM/CLI, XPC/CLI) are private to that crate.
2. **Make Apple `container` possible from day one, but don't build it in v1.** Requirements
   ENG-030…033 are binding now: a non-exhaustive `EngineKind`, optional engine-wide facts,
   per-container IPs, and factory-based discovery.
3. **When scheduled**, `dk-engine-apple` follows the WSLC pattern: a native **XPC** primary transport
   (Rust → `xpc_*` C API, JSON payloads mirroring Apple's `Codable` types, version-gated per
   major version), with a **CLI fallback**. A Swift shim was rejected because it would add a Swift toolchain
   to the build and couple us to Apple's internal Swift API.

## Consequences

- v1 on macOS supports Docker-compatible engines only.
- Adding the Apple backend later is a self-contained milestone: a new crate, a factory, fixtures, and
  spikes S-6/S-7. It needs no UI refactor.
- The contract tests (spec 21 §7) become the acceptance gate for any new backend.
