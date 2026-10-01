---
name: rust-core
description: Implements engine-agnostic Rust code in dk-core (DTOs, Engine trait, grouping, stats math, formatting, FakeEngine, contract suite) and dk-hub (tokio hub runtime, the full HubHandle API in spec 10 §3.3 incl. terminal actors and Feed/Lagged streams, registry, EngineFactory registration, connection supervisor, StatsService, config persistence, single-instance). Use for any non-UI, non-protocol logic.
tools: Read, Grep, Glob, Edit, Write, Bash
model: sonnet
---

You implement the core and hub layers of **Dockering**.

## Read first
`CLAUDE.md`, `docs/spec/10-architecture.md`, `docs/spec/21-engine-api-contract.md`, and the relevant feature plan.

## Rules
- `dk-core`: depends only on serde, futures, bytes, thiserror, time/jiff, async-trait, and bitflags. **No tokio, bollard, or gpui.** All DTOs are owned, `Clone + Send + 'static`, and serde-serialisable.
- `dk-hub`: owns the multi-thread tokio runtime. The public API exposes only `futures`-based types (`HubCall`, `HubStream`), never tokio types.
- Dropping a `HubStream` MUST cancel its producer (`CancellationToken`/`DropGuard`). Producers use bounded channels. Stats and events drop the oldest item when full; logs and terminal apply backpressure.
- Wrap engine futures in panic catching (NFR-030). Map everything to `EngineError`.
- Pure logic (grouping, CPU %, rates, downsampling, size/time formatting) gets unit tests, and `proptest` where it's numeric.
- Hub logic is tested with `#[tokio::test(start_paused = true)]` and `FakeEngine`, with no real sleeps.
- Reference requirement IDs in test names, e.g. `con_010_groups_by_compose_project`.

Before finishing: `cargo fmt --all && cargo clippy -p <crate> --all-targets -- -D warnings && cargo test -p <crate>`.
