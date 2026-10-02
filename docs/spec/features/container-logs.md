# Feature: Container logs

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** LOG
- **Plan:** no per-feature plan; built in milestone M3 of the [v1 plan](../../plan/README.md)

## Requirements

| ID | Requirement |
|---|---|
| LOG-001 | On open, load the last 1,000 lines (configurable, SET-030) and then follow (`follow=true`) while the container runs. |
| LOG-002 | Render with GPUI's virtualised `list` (variable row heights, so wrapped lines work) in a monospaced font. stderr lines are tinted. Basic ANSI SGR colours are rendered; other escapes are stripped. |
| LOG-003 | Auto-scroll ("tail") is on by default. Scrolling up pauses it and shows a "Jump to bottom (N new)" pill. |
| LOG-004 | Toolbar: search (highlights matches, ↑/↓ navigate), *Timestamps* toggle, *Wrap lines* toggle, *Clear view* (client-side only), *Copy all*, *Save to file…* (native save dialog; the write runs on the hub runtime). |
| LOG-005 | The buffer is capped at 50,000 lines in memory (ring buffer; configurable, SET-030). Dropped head lines are indicated. |
| LOG-006 | When the container stops, the stream ends and a footer shows "Container exited (code N)". A `start`/`restart`/`unpause` event resumes following automatically with `since=<last ts>`. Because `since` has one-second granularity on Docker, lines at or before the last line already shown are dropped as duplicates. Without a known timestamp, the resume takes a fresh tail (LOG-001). |
| LOG-007 | Batching per [10 §3.3](../10-architecture.md#33-the-ui--hub-bridge-contract): flush every 50 ms or 500 lines. ANSI parsing happens in `background_spawn`, one parser per stream, so escapes split across chunks survive. |
| LOG-008 | Containers with `tty=true` produce a raw stream without stdout/stderr multiplexing. The engine maps it to `LogStream::Console`. |
| LOG-009 | Keyboard: search, follow, toggles, and save per [keyboard.md](keyboard.md) KBD-050…053. |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| LOG-001 | `log_001_initial_tail_then_follow`, `log_001_logs_non_follow_split_streams` (WSLC CLI) |
| LOG-002 | `log_002_*` (dk-core `ansi`, logs view, colours and stderr) |
| LOG-003 | `log_003_follow_pill` |
| LOG-004 | `log_004_search_next_prev`, `log_004_toggles_clear_and_copy_all`, `log_004_save_writes_through_hub`, `log_004_match_overlay_splits_sgr_spans` |
| LOG-005 | `log_005_ring_cap_with_dropped_indicator` |
| LOG-006 | `log_006_exit_footer_then_resume_on_start` |
| LOG-007 | `log_007_batching_many_pushes_few_notifies` |
| LOG-008 | `log_008_splits_frames_and_parses_timestamps`, `log_008_console_stream_label` |
| LOG-009 | Covered by the LOG-003/004 tests (`End`, `F3`, `Alt+T`/`Alt+W`, `Mod+Shift+K`, `Mod+Shift+C`, `Mod+F`) |

## Known gaps (v1)

- The LOG-006 duplicate filter compares timestamps. Distinct lines that share the last line's timestamp exactly are dropped on resume.
