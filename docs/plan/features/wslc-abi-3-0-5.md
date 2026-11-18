# Plan: Admit WSL 3.0.5.0 to the WSLC native COM allowlist

- **Slug:** `wslc-abi-3-0-5`
- **Status:** done (2026-11-18; the user asked to resolve issue #81, following the [3.0.3 admission](wslc-abi-3-0-3.md) template)
- **Spec:** [Backends §5.3/§5.7/§5.8](../../spec/20-engine-backends.md), [Engines](../../spec/features/engines.md) (ENG-013, ENG-132)
- **Requirement IDs:** ENG-013 (allowlist entries), ENG-132 (evidence); no new IDs
- **Created:** 2026-11-18

## 1. Goal

[Issue #81](https://github.com/pavel-purma/dockering/issues/81): the weekly `wslc-abi-check` found that WSL 3.0.5 differs from the vendored 3.0.3 IDL. Native COM is trusted for exact versions only (ENG-132), so every user on 3.0.5 ran on the `wslc.exe` CLI fallback. After this change `3.0.5.0` is trusted as its own exact entry, served by the existing `v3_0` module.

## 2. Scope

**In:** vendor `idl/3.0.5/` verbatim (tag commit `e7ef9a88…`); pin the IDL review as an xtask test; admit `3.0.5.0`; map the new `WSLC_E_NETWORK_PREFIX_AMBIGUOUS` like its container sibling; a live network inspect/remove test for the renamed slots; live evidence on a real 3.0.5.0 machine; fix the CLI exec regression the live run found; update the spec.

**Out:** WSL 3.0.4 (pre-release only); `StartWithProcess` (not declared, not needed); `CapAdd`/`CapDrop`/`RestartPolicy`/`EnableIPv6` (only `CreateContainer`/`CreateNetwork` take them; native Run stays disabled, ENG-128).

## 3. IDL review (3.0.3 → 3.0.5)

| Change | Effect on `v3_0` |
|---|---|
| `IWSLCContainer` slot 20 `StartWithProcess` appended | None: the module declares slots 0–19, and a longer server vtable is compatible |
| `DisconnectFromNetwork` / `DeleteNetwork` / `InspectNetwork` parameter renamed to `NetworkNameOrId` | Same `LPCSTR` type and slot; ids now accepted too |
| `WSLCContainerOptions` + `CapAdd`, `CapDrop`, `RestartPolicy`; `WSLCNetworkOptions` + `EnableIPv6` | Taken only by `CreateContainer` (never called) and `CreateNetwork` (declared opaque, never called) |
| `WSLCMountSpecFlagsConfigVolume`, `WSLCContainerRestartPolicy` enum | Not used |
| `WSLC_E_CAPABILITY_ADDITIONS_DISABLED`, `WSLC_E_RUNTIME_UPDATE_NEEDED`, `WSLC_E_NETWORK_PREFIX_AMBIGUOUS` | Constants added; the network one maps to `Conflict`, the rest fall to `Api 500` with the HRESULT |

`WSLCShared.idl` is identical.

## 4. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Vendor `idl/3.0.5/`, README row, xtask review test | release-engineer | ENG-132 | `vendored_3_0_3_to_3_0_5_leaves_called_slots_alone` |
| 2 | `MODULES`/`VERIFIED` entry, HRESULT constants and mapping | windows-platform | ENG-013, ENG-132 | `eng_132_exact_allowlist`, `module_metadata_matches_submodule`, `wslc_codes_match_idl_formula`, `mapping_table` |
| 3 | Fake self-check for 3.0.5; zero activation for 3.0.5.1, 3.0.6.0; live network test | qa-engineer | ENG-132 | `eng_132_self_check_for_every_admitted_version`, `eng_132_unknown_version_zero_activation`, `live_network_inspect_remove` |
| 4 | Live evidence on 3.0.5.0 | windows-platform | ENG-132 | spec 20 §5.8 |
| 5 | CLI exec: keep the post-exit output tail (found by task 4) | windows-platform | ENG-131 | `cli_live` 6/6 on 3.0.5.0 |
| 6 | Spec 20/21, engines.md, plan index | architect | ENG-013, ENG-132 | spec matches code |

## 5. Revision log

- 2026-11-18: created and implemented. Live evidence on WSL 3.0.5.0 recorded in spec 20 §5.8. The live CLI suite showed that on wslc 3.0.5 a short exec's output reaches the reader only after `wslc.exe` exits; the reader now forwards that tail instead of discarding it.
