# Dockering — Application Specification

This directory is the **single source of truth** for what Dockering is and how it behaves.
Code, plans and tests follow the spec — not the other way around. When behaviour changes,
the spec changes in the same change set (see [Spec workflow](#spec-workflow)).

## Index

| File | Contents |
|---|---|
| [00-product.md](00-product.md) | Vision, scope, non-goals, glossary |
| [10-architecture.md](10-architecture.md) | Workspace layout, layers, threading model, state, errors |
| [20-engine-backends.md](20-engine-backends.md) | Engine kinds (Docker socket/pipe/TCP, WSL distro, WSLC; Apple `container` deferred), discovery, connection lifecycle |
| [21-engine-api-contract.md](21-engine-api-contract.md) | Backend layering, internal `Engine` trait, DTOs, capability model, `EngineFactory`, mapping to Docker Engine API and WSLC (COM + CLI) |
| [30-ui-shell.md](30-ui-shell.md) | Window layout, navigation, theming, global UX rules |
| [40-non-functional.md](40-non-functional.md) | Responsiveness, performance budgets, security, platform support |
| [50-build-and-release.md](50-build-and-release.md) | Toolchain, CI matrix, packaging per OS |
| [60-quality.md](60-quality.md) | Testing strategy, fixtures, definition of done |
| [features/](features/) | One file per user-facing feature (requirements + UI + contract usage) |
| [CHANGELOG.md](CHANGELOG.md) | Chronological log of spec changes, written at release time (REL-019) |

### Feature specs

| Feature | File | Status |
|---|---|---|
| Engine connections & switching | [features/engines.md](features/engines.md) | implemented (2026-10-04 WSLC repair; cross-OS release gate open; 2026-10-05 status-bar change in progress) |
| Containers list & grouping | [features/containers.md](features/containers.md) | implemented |
| Container detail | [features/container-detail.md](features/container-detail.md) | implemented (2026-10-04 WSLC inspect parity; cross-OS release gate open) |
| Container logs | [features/container-logs.md](features/container-logs.md) | implemented |
| Container terminal (exec) | [features/container-terminal.md](features/container-terminal.md) | implemented |
| Container stats & charts | [features/container-stats.md](features/container-stats.md) | implemented |
| Images | [features/images.md](features/images.md) | implemented (2026-10-04 WSLC Run repair; cross-OS release gate open) |
| Volumes | [features/volumes.md](features/volumes.md) | implemented |
| Networks | [features/networks.md](features/networks.md) | implemented |
| Settings | [features/settings.md](features/settings.md) | implemented |
| Keyboard navigation & shortcuts | [features/keyboard.md](features/keyboard.md) | implemented (KBD-090 release walkthrough and KBD-092 beyond Settings open) |
| Distribution, releases & updates | [features/distribution.md](features/distribution.md) | in-progress (unsigned v0.1.0 and v0.2.0 published and verified; `/release` skill and SignPath signing flow built, signed channel not yet enabled; Linux install-and-launch smoke test built, first GitHub run pending, REL-070…077) |
| macOS native engine (Apple `container`) | [features/macos-native-engine.md](features/macos-native-engine.md) | deferred (ENG-030…033 binding in v1) |

Each implemented spec has a *Verification* table (requirement → tests) and a *Known gaps (v1)*
section. Manual checks live in [`docs/plan/release-checklist.md`](../plan/release-checklist.md).

## Conventions

### Status labels

Every feature spec carries a `Status:` line in its header:

| Status | Meaning |
|---|---|
| `draft` | Being written, not yet agreed |
| `planned` | Agreed; a plan exists or will be written in `docs/plan/features/` |
| `in-progress` | Implementation started |
| `implemented` | Shipped on `main`; spec matches code |
| `deferred` | Researched and specified, intentionally not scheduled; only its "binding" requirements apply now |
| `deprecated` | Kept for history; will be removed |

### Requirement IDs

Requirements are numbered so plans, code comments and tests can reference them:

```
<AREA>-<NNN>      e.g. CON-001, IMG-004, ENG-012, TRM-003
```

| Prefix | Area |
|---|---|
| `ENG` | Engines / connections |
| `CON` | Containers list |
| `CDT` | Container detail |
| `LOG` | Logs |
| `TRM` | Terminal |
| `STA` | Stats / charts |
| `IMG` | Images |
| `VOL` | Volumes |
| `NET` | Networks |
| `SET` | Settings |
| `SHL` | UI shell (global) |
| `KBD` | Keyboard navigation & shortcuts |
| `NFR` | Non-functional |
| `REL` | Release, packaging, signing, distribution |
| `UPD` | In-app updates |

IDs are never reused. A removed requirement is struck through and marked `(removed in <date>)`.

Requirement keywords follow RFC 2119: **MUST**, **SHOULD**, **MAY**.

## Spec workflow

1. New or changed feature → run the `feature-planning` skill (`/feature-planning <idea>`).
   It reads this spec, writes a plan in `docs/plan/features/`, and updates the relevant
   spec files with status `planned`.
2. Implementation PRs reference requirement IDs and move the status to `in-progress`.
3. When a feature lands, the spec is reconciled with what was actually built
   (`/feature-planning complete <slug>`) and the status becomes `implemented`.
4. Feature PRs add no line to [CHANGELOG.md](CHANGELOG.md) (concurrent PRs would conflict on it).
   The `/release` skill adds a line for every merged spec change when it prepares a release
   (REL-019), from the PR text and the diff of `docs/spec/**`.

A PR that changes user-visible behaviour without a spec update is incomplete.
