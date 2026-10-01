# Feature: Container logs

- **Status:** planned
- **Requirement prefix:** LOG
- **Plan:** [docs/plan/features/container-logs.md](../../plan/features/container-logs.md) (to be written)

## Requirements

| ID | Requirement |
|---|---|
| LOG-001 | On open, load the last 1,000 lines (configurable) and then follow (`follow=true`) while the container runs. |
| LOG-002 | Render with a virtualised list (`VirtualList` / uniform list) in a monospaced font. stderr lines are tinted. Basic ANSI SGR colours are rendered; other escapes are stripped. |
| LOG-003 | Auto-scroll ("tail") is on by default. Scrolling up pauses it and shows a "Jump to bottom (N new)" pill. |
| LOG-004 | Toolbar: search (highlights matches, ↑/↓ navigate), *Timestamps* toggle, *Wrap lines* toggle, *Clear view* (client-side only), *Copy all*, *Save to file…* (native save dialog; the write runs on the hub runtime). |
| LOG-005 | The buffer is capped at 50,000 lines in memory (ring buffer). Dropped head lines are indicated. |
| LOG-006 | When the container stops, the stream ends and a footer shows "Container exited (code N)". A restart resumes following automatically with `since=<last ts>`. |
| LOG-007 | Batching per [10 §3.3](../10-architecture.md#33-the-ui--hub-bridge-contract): flush every 50 ms or 500 lines. ANSI parsing happens in `background_spawn`. |
| LOG-008 | Containers with `tty=true` produce a raw stream without stdout/stderr multiplexing. The engine maps it to `LogStream::Console`. |
| LOG-009 | Keyboard: search, follow, toggles, and save per [keyboard.md](keyboard.md) KBD-050…053. |
