# Feature: Images

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** IMG
- **Plan:** no per-feature plan; built in milestone M6 of the [v1 plan](../../plan/README.md)

## Requirements

| ID | Requirement |
|---|---|
| IMG-001 | List columns: ☐ · Name (repository) · Tag · Image ID (short) · Created (relative) · Size · Status (*In use* chip if containers > 0) · Actions (Run, Delete). An image with N tags shows as N rows, as Docker Desktop does. Dangling images show as `<none>`. |
| IMG-002 | Sort by name, tag, created, or size. Filter: *All* / *In use* / *Unused* / *Dangling*. Search over repo:tag and id. |
| IMG-003 | The header shows the total size and the count (distinct images). Overflow: *Prune dangling*, *Prune unused*. The confirmation lists the candidates and the reclaimable size: for *Prune unused* on engines with `DISK_USAGE`, it comes from `disk_usage()`. Without `DISK_USAGE`, for *Prune dangling*, or when the `disk_usage` call fails, it falls back to the sum of the candidates' sizes. |
| IMG-004 | *Pull image* dialog: reference input (`nginx:latest`). The pull runs in an app-wide `PullManager`, so closing the dialog or leaving the page never cancels it. Progress is shown as one notification per pull, updated in place at most every 100 ms, with per-layer bars (structured when `PULL_PROGRESS`, otherwise a status line). Cancellable from the notification; cancel drops the stream, which cancels the pull on the hub. |
| IMG-005 | *Run* dialog (simple): container name, port mappings (host:container rows), env vars (rows), volume mounts (rows), *Remove when stopped*, *Start*. → `run_image`. On success, navigate to the new container detail. **WSLC (v1):** the CLI transport supports run. The COM transport returns `Api { status: 501 }` ("Run via COM is not verified for this WSL version", 20 §5.4), which shows as an error notification. The transport note says so (ENG-110). |
| IMG-006 | Delete: confirm. If in use → error explaining which containers use it, with a *Force* option. |
| IMG-007 | **Registry auth (v1).** Pull/push credentials are read from the user's Docker config (`~/.docker/config.json`): `auths` entries directly, and `credsStore` / `credHelpers` by running `docker-credential-<helper> get` (argv, stdin = server, on the hub runtime, 10 s timeout). For WSLC, the engine's own registry store is used (`wslc registry login`, outside Dockering). If no credentials are found, the pull is anonymous. An auth failure (401, or registry errors such as ghcr's 500 `denied` and Hub's `pull access denied`; 21 §6) shows "Authentication required: run `docker login <registry>`". In-app login UI is post-v1. Credentials are never logged or persisted by Dockering (NFR-020). |
| IMG-010 | **Detail tabs**: *Overview* (id, digests, tags, created, size, arch/os/variant, author, entrypoint, cmd, env, exposed ports, workdir, user, volumes, labels). *Layers* (history table: created by, size, created, comment; capability-gated, skipped without `IMAGE_HISTORY`). *Used by* (containers list, links). *Inspect* (raw JSON). |
| IMG-011 | Detail actions: Run, Tag… (dialog), Delete, Copy id, Copy digest. |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| IMG-001 | `img_001_one_row_per_tag_and_dangling`, `img_001_one_row_per_tag_and_dangling_none`, `img_001_split_repo_tag` |
| IMG-002 | `img_002_filters_and_search`, `img_002_filters_search_and_sort` |
| IMG-003 | `img_003_prune_confirm_shows_reclaimable`, `img_003_totals_count_distinct_images` |
| IMG-004 | `img_004_*` (progress parsing, dialog validation, notification, status line without `PULL_PROGRESS`, cancel, WSLC CLI text progress) |
| IMG-005 | `img_005_run_dialog_runs_and_navigates`, `img_005_ports`, `img_005_mounts`, `img_005_run_spec_and_errors`, `img_005_create_body` |
| IMG-006 | `img_006_delete_in_use_offers_force` |
| IMG-007 | `img_007_*` (registry key, auths, helpers, resolution order, denied → auth required, 401 login hint) |
| IMG-010 | `img_010_detail_tabs_and_history_gating`, `img_010_layers_tab_skipped_without_image_history`, `img_010_used_by_enter_follows_link` |
| IMG-011 | `img_011_tag_dialog_tags_image`, `img_004_011_reference_and_tag` |

Four states: `img_000_four_states`.

## Known gaps (v1)

- IMG-005 run over WSLC COM returns 501 until `CreateContainer` is verified (spike S-3 follow-up). Workaround: add the WSLC session again as a manual engine with transport *CLI* (ENG-105). The transport of an existing engine can't be changed in v1.
- IMG-011 *Copy digest* has no view test.
