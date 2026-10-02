# 40 — Non-functional Requirements

| ID | Requirement | Verification |
|---|---|---|
| NFR-001…005 | Threading rules (see [10 §3.1](10-architecture.md#31-rules-normative)) | Code review + CI grep for blocking calls in `crates/dockering/src` |
| NFR-010 | Cold start to first frame ≤ 500 ms (release build, warm disk). Engine data appears as it arrives. | Manual timing in the release checklist |
| NFR-011 | The UI stays at 60 fps while scrolling 1,000-row lists and while 4 charts and 1 log stream update | Manual check with GPUI's frame-time overlay, plus a perf note in the PR |
| NFR-012 | Idle CPU < 1% with an engine connected and only the list page open (no stats columns) | Manual |
| NFR-013 | Memory < 250 MB RSS with 1,000 containers, 1 log view at its cap, and 4 charts | Manual |
| NFR-014 | Any engine action shows feedback in ≤ 100 ms (SHL-001) | Review |
| NFR-020 | **Security.** Never send credentials anywhere except the configured engine. Never log env values, auth, or TLS keys. | Review |
| NFR-021 | **Security.** WSL bridge pipes are ACL-restricted to the current user and use randomised names. TCP engines without TLS show a persistent warning chip. | Test + review |
| NFR-022 | **Security.** No shell interpolation: every child process (`wsl.exe`, `wslc.exe`) gets an argv vector. Arguments are validated **per kind**: container/volume/network names `^[a-zA-Z0-9][a-zA-Z0-9_.-]*$`; ids are hex; image references follow the OCI reference grammar (`/`, `:`, `@` allowed). In every case a value MUST NOT start with `-`, and it's passed after `--` where the CLI supports it (`wsl.exe` yes; `wslc` 3.0.1 no — see 20 §5.5). | Unit tests |
| NFR-023 | No telemetry. No network access except to configured engines and, through the engine, to registries. | Review |
| NFR-030 | **Resilience.** An engine crash or disconnect never crashes the app. Every hub task catches panics (`catch_unwind` around the engine future) and reports `EngineError::Protocol`. | Test with a fake engine that panics |
| NFR-031 | Malformed JSON or unknown fields from an engine never panic. Unknown enum values map to `Unknown`. | Fuzz-ish fixture tests |
| NFR-040 | **Portability.** Every crate builds on all six targets in the CI matrix. Platform-specific code sits behind `cfg` inside crates, never in separate crates per OS. | CI |
| NFR-041 | HiDPI and mixed-DPI monitors render crisply (GPUI handles the scale factor; we don't hard-code pixel art). | Manual |
| NFR-042 | **Keyboard accessibility.** 100% of features are operable without a pointing device (KBD-001), with visible focus (KBD-003) and no focus traps except the documented terminal capture with its escape hatch (KBD-061). | KBD-090…093 |
| NFR-050 | **Logging** per [10 §8](10-architecture.md#8-logging--diagnostics) | — |
