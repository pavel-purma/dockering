# Feature: Container stats & charts

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** STA
- **Plan:** no per-feature plan; built in milestone M5 of the [v1 plan](../../plan/README.md)

## Layout (Stats tab)

```
┌ CPU ─────────────────────────── 3.7% ┐ ┌ Memory ─────────────── 128 MB / 2 GB ┐
│  AreaChart (0–100% × online CPUs)    │ │  AreaChart (bytes, limit as max)     │
└──────────────────────────────────────┘ └──────────────────────────────────────┘
┌ Network I/O ───── ↓ 1.2 MB/s ↑ 40 KB/s┐ ┌ Disk I/O ────── R 0 B/s  W 220 KB/s  ┐
│  2 overlaid LineCharts (rx, tx)      │ │  2 overlaid LineCharts (read, write) │
└──────────────────────────────────────┘ └──────────────────────────────────────┘
 Window: [1m] [5m] [15m]   ·  PIDs: 12  ·  Totals: ↓ 340 MB ↑ 12 MB · R 1.1 GB W 230 MB
 Processes (top) table — optional, Capability::TOP
```

## Requirements

| ID | Requirement |
|---|---|
| STA-001 | The Stats tab shows four charts: CPU %, Memory (used vs limit), Network rx/tx rate, and Block I/O read/write rate. Each card shows the current value in its header. |
| STA-002 | Samples come from `Engine::stats(id)`: about 1 s for Docker streams, 2 s for WSLC COM polling, and at least 1 s for WSLC CLI polling. They are normalised to `StatsSample` (rates, not counters) in the engine layer. |
| STA-003 | Each container keeps a ring buffer covering the configured history window (SET-050, default 15 min; at most 3,600 samples). The time window selector offers 1m, 5m, or 15m (default 5m). Points beyond 300 per chart are downsampled with LTTB in `dk-core::stats`. The hub's history replay to a new subscriber is applied in batches (at most 256 samples per UI update), so a full ring doesn't cause one notify per sample. |
| STA-004 | Charts use GPUI Kit `AreaChart` (CPU, Memory) and `LineChart` (Network, Disk). On each sample a new chart element is built from a cloned `Vec<Point>`, per the GPUI Kit docs. Stable `.id()`s per chart. GPUI Kit has no multi-series line chart, so each two-series card overlays two single-series `LineChart`s that share one pinned y domain, axes, and gutter. |
| STA-005 | The Y axis auto-scales with human units (%, B/KB/MB/GB, B/s…). CPU optionally normalises to 0–100% of the whole host (setting "CPU % relative to all cores"). |
| STA-006 | Stats streaming starts only while the Stats tab is visible, **or** while a CPU/memory column is enabled in the list. The maximum number of concurrent list streams is per transport (`EngineInfo.list_stats_limit`): Docker/WSLC-COM 20; WSL-distro bridge 8 (each stream holds a `wsl.exe`); WSLC-CLI 0 (the columns are hidden, because a CLI stats call takes ~1 s). A background `StatsService` in the hub dedups subscribers, so one engine stream feeds N UI consumers. The upstream stops 5 s after the last subscriber leaves. Buffers live in the hub and are evicted after the history window (SET-050) without subscribers, so reopening the tab within that window shows history. |
| STA-007 | A stopped container shows the last buffer greyed out with "Container not running". |
| STA-008 | *Processes* table from `top()`, refreshed every 5 s while visible (capability-gated). |
| STA-009 | Rendering cost: chart rebuild ≤ 2 ms per frame for 300 points × 2 series (benchmark in M5). |
| STA-010 | Disk usage (static): the container's writable layer size (`size_rw`) and rootfs size are shown on the Stats tab. They are fetched on demand because `size=true` is expensive. |
| STA-011 | Keyboard: the window selector and chart cards are focusable, and the current values are readable as text (KBD-070). |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| STA-001 | `sta_001_samples_render_and_history_replays` |
| STA-002 | `sta_002_*` (dk-core formulas, cgroup v2 JSON, polled rates), `sta_002_stats_stream_normalises`, `sta_stats_from_cli_strings` |
| STA-003 | `sta_003_*` (ring cap, LTTB, window capacity, window + downsampling, batched replay) |
| STA-004 | `sta_009_chart_build_benchmark` builds the chart elements; the overlay alignment is checked visually |
| STA-005 | `sta_005_nice_axis_max`, `sta_005_cpu_relative_to_all_cores` |
| STA-006 | `sta_006_stats_service_dedups_subscribers`, `sta_006_history_replayed_to_new_subscriber`, `sta_006_stats_error_forwarded_and_buffer_kept`, `sta_006_stats_only_while_visible`, `set_020_cpu_columns_toggle_live` |
| STA-007 | `sta_007_stopped_container_keeps_buffer` |
| STA-008 | `sta_008_processes_gated_on_top` |
| STA-009 | `sta_009_chart_build_benchmark` (prints the time; asserts a generous 50 ms so CI never flakes) |
| STA-010 | `sta_010_disk_usage_on_demand` |
| STA-011 | `kbd_070_window_selector_keys` |

## Known gaps (v1)

- STA-009: the test asserts < 50 ms per build to keep CI stable. The ≤ 2 ms target is checked from the printed timing on a release build (release checklist, NFR-011).
- STA-004: overlaid charts show one tooltip per series, not a combined tooltip.
