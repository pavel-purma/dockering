# Feature: Container stats & charts

- **Status:** in-progress <!-- UI in progress -->
- **Requirement prefix:** STA
- **Plan:** [docs/plan/features/container-stats.md](../../plan/features/container-stats.md) (to be written)

## Layout (Stats tab)

```
┌ CPU ─────────────────────────── 3.7% ┐ ┌ Memory ─────────────── 128 MB / 2 GB ┐
│  AreaChart (0–100% × online CPUs)    │ │  AreaChart (bytes, limit as max)     │
└──────────────────────────────────────┘ └──────────────────────────────────────┘
┌ Network I/O ───── ↓ 1.2 MB/s ↑ 40 KB/s┐ ┌ Disk I/O ────── R 0 B/s  W 220 KB/s  ┐
│  LineChart, 2 series (rx, tx)        │ │  LineChart, 2 series (read, write)   │
└──────────────────────────────────────┘ └──────────────────────────────────────┘
 Window: [1m] [5m] [15m]   ·  PIDs: 12  ·  Totals: ↓ 340 MB ↑ 12 MB · R 1.1 GB W 230 MB
 Processes (top) table — optional, Capability::TOP
```

## Requirements

| ID | Requirement |
|---|---|
| STA-001 | The Stats tab shows four charts: CPU %, Memory (used vs limit), Network rx/tx rate, and Block I/O read/write rate. Each card shows the current value in its header. |
| STA-002 | Samples come from `Engine::stats(id)`: about 1 s for Docker streams, 2 s for WSLC polling. They are normalised to `StatsSample` (rates, not counters) in the engine layer. |
| STA-003 | Each container keeps a ring buffer covering the configured history window (SET-050, default 15 min; at most 3,600 samples). The time window selector offers 1m, 5m, or 15m (default 5m). Points beyond 300 per chart are downsampled (LTTB or min/max bucket), in `dk-core::stats`. |
| STA-004 | Charts use GPUI Kit `AreaChart` / `LineChart`. On each sample a new chart element is built from a cloned `Vec<Point>`, per the GPUI Kit docs. Stable `.id()`s per chart. |
| STA-005 | The Y axis auto-scales with human units (%, B/KB/MB/GB, B/s…). CPU optionally normalises to 0–100% of the whole host (setting "CPU % relative to all cores"). |
| STA-006 | Stats streaming starts only while the Stats tab is visible, **or** while a CPU/memory column is enabled in the list. The maximum number of concurrent list streams is per transport: Docker/WSLC-COM 20; WSL-distro bridge 8 (each stream holds a `wsl.exe`); WSLC-CLI 0 (the columns are hidden, because a CLI stats call takes ~1 s). A background `StatsService` in the hub dedups subscribers, so one engine stream feeds N UI consumers. Buffers live in the hub, so reopening the tab within 15 min shows history. |
| STA-007 | A stopped container shows the last buffer greyed out with "Container not running". |
| STA-008 | *Processes* table from `top()`, refreshed every 5 s while visible (capability-gated). |
| STA-009 | Rendering cost: chart rebuild ≤ 2 ms per frame for 300 points × 2 series (benchmark in M5). |
| STA-010 | Disk usage (static): the container's writable layer size (`size_rw`) and rootfs size are shown on the Stats tab. They are fetched on demand because `size=true` is expensive. |
| STA-011 | Keyboard: the window selector and chart cards are focusable, and the current values are readable as text (KBD-070). |
