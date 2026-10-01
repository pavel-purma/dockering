# ADR-0001: Cargo workspace and layering

- **Status:** accepted · 2026-10-01

## Context

The app mixes three concerns that change at different rates: GPUI UI code, engine protocols
(Docker HTTP, WSL process bridging, the WSLC COM API and CLI), and pure domain logic (grouping, stats math).
GPUI compile times are significant, and UI code should be testable without a container engine.

## Decision

Use seven crates (see [spec 10 §2](../../spec/10-architecture.md#2-cargo-workspace-layout)) with strict
dependency rules. `dk-core` has no runtime, UI, or protocol dependencies and defines the
`Engine` trait and DTOs. Engines and the hub depend on tokio. Only `dockering` and
`dk-terminal` depend on GPUI Kit.

## Consequences

- Each engine can be tested and benchmarked without GPUI.
- `FakeEngine` in `dk-core` enables UI tests without Docker.
- Changes to the DTOs touch several crates. That's acceptable, because the DTOs are the contract and reviewers should see that.
