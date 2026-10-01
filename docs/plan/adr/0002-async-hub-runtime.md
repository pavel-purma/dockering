# ADR-0002: Dedicated tokio "hub" runtime bridged to GPUI

- **Status:** accepted · 2026-10-01

## Context

GPUI runs its own foreground (main-thread) and background executors. They aren't tokio.
`bollard`, `hyper`, `tokio::process`, and tokio named pipes all need a tokio reactor. The user
requires that the UI never blocks.

## Options

1. Run tokio futures on GPUI's background executor using `tokio::runtime::Handle::enter` hacks. This is fragile, and reactor access from non-tokio threads isn't supported.
2. Use blocking clients on GPUI's background threads. This wastes threads on long-lived streams (logs, stats, events, terminals) and complicates cancellation.
3. **Run a dedicated multi-thread tokio runtime that owns all engine I/O. Communicate over executor-agnostic `futures::channel` oneshot/mpsc. GPUI tasks `.await` the receivers.**

## Decision

Option 3. `HubHandle::call` returns a future (`HubCall<T>`) and `subscribe` returns a stream
(`HubStream<T>`). Both can be awaited from GPUI's foreground executor without blocking.
Dropping a `HubStream` cancels the hub-side producer through a `CancellationToken`.

## Consequences

- The UI thread can't block on engine I/O by construction.
- Cancellation is RAII-based: dropping the GPUI `Task` → drops the stream → cancels the producer.
- There are two executors to reason about. The rule is simple: the UI awaits hub futures, and only the hub touches sockets and processes.
