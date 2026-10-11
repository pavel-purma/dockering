# Feature plans

Plans in this checkout; a draft is not approval to implement. Historical spec links may reference plans/ADRs not present here. There is no milestone README/table in this checkout.

| Feature | Plan | Status | Requirements |
|---|---|---|---|
| Engine scan lifecycle, stable listing, default engine | [engine-scan-and-defaults](engine-scan-and-defaults.md) | done (2026-10-04) | ENG-113…116; ENG-103/009 changed |
| WSLC existing integration repair, COM-first per-operation fallback | [wslc-integration-repair](wslc-integration-repair.md) | done (2026-10-04 core repair; cross-OS release gates open) | ENG-126…136; ENG-013/109/110, IMG-005, CDT-030/040 changed/clarified |
| First unsigned cross-platform release | [unsigned-first-release](unsigned-first-release.md) | done (v0.1.0 published and verified on 2026-10-04) | REL-010/013/016/017 |
| `/release` skill, release-time changelogs, Windows signing (SignPath Foundation) | [release-skill-and-signing](release-skill-and-signing.md) | in progress (built 2026-10-06; signing setup is the maintainer's) | REL-018/019/033/034 new; REL-010/011/031/032/060 changed |
| Status bar names the engine; WSLC route notes become rare and short | [status-bar-engine-label](status-bar-engine-label.md) | approved, in progress (2026-10-05) | ENG-108/110/136 changed |
| Keyboard flow: palette height, search → table hand-off, list state kept on return, keyboard viewing of detail tabs | [keyboard-flow-polish](keyboard-flow-polish.md) | in progress (approved and implemented 2026-10-10; review fixes applied) | KBD-094…098, SHL-014, CDT-083 new; KBD-006/020/030/042/051/061, CDT-082, CON-034 changed |
| Group by *Combined* (Compose project, else a label) as the default; the label key moves to Settings with a standard default | [group-by-combined](group-by-combined.md) | done (2026-10-11) | CON-014 new; CON-010/011/013, SET-020 changed |
| WSLC "no session" state: a calm start page with a *Start* action and a quiet recheck, instead of a failure page | [wslc-no-session-start](wslc-no-session-start.md) | in progress (2026-10-11) | ENG-137…141 new; ENG-106/107/127 changed |
