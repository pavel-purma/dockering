# Plan: <Feature title>

- **Slug:** `<slug>`
- **Status:** draft <!-- draft | approved | in-progress | done | abandoned -->
- **Spec:** [docs/spec/features/<area>.md](../../spec/features/<area>.md)
- **Milestone:** M<n>
- **Requirement IDs:** <e.g. CON-040, CON-041 (new) · CON-020 (changed)>
- **Created:** YYYY-MM-DD

## 1. Goal
One paragraph: the user-visible outcome and why it matters.

## 2. Scope
**In:** …
**Out:** … (link the non-goals if relevant)

## 3. Assumptions & open questions
| # | Assumption / question | Default if unanswered |
|---|---|---|

## 4. Requirements (as written into the spec)
| ID | Requirement | New/Changed |
|---|---|---|

## 5. Engine contract impact
```rust
// new / changed trait methods, DTO fields, capabilities
```

| Op | Docker (API / bollard) | WSL distro (Docker via bridge) | WSLC COM (`IWSLC*`) | WSLC CLI fallback (`wslc …`) | Capability |
|---|---|---|---|---|---|

## 6. Design
### 6.1 Data flow & threading
Hub calls and streams used · where `Task`s are stored · batching · stale-revision guard.

### 6.2 UI
Route(s) · GPUI Kit components · loading/empty/error/data states · actions + confirmations · shortcuts.
ASCII sketch if layout changes.

### 6.3 Errors
Which `EngineError`s are possible, and how each is shown.

## 7. Tasks
| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | | rust-core | | unit tests … |
| 2 | | engine-integrator | | fixtures … |
| 3 | | gpui-ui | | view test + screenshot |
| 4 | | qa-engineer | | ID→test table |
| 5 | Review | reviewer | all | verdict: approve |

## 8. Test plan
| ID | Layer | Test name(s) |
|---|---|---|

## 9. Risks & spikes
| Risk | Mitigation / spike |
|---|---|

## 10. Revision log
- YYYY-MM-DD: created
