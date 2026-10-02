# Feature: Images

- **Status:** in-progress <!-- UI in progress -->
- **Requirement prefix:** IMG
- **Plan:** [docs/plan/features/images.md](../../plan/features/images.md) (to be written)

## Requirements

| ID | Requirement |
|---|---|
| IMG-001 | List columns: ☐ · Name (repository) · Tag · Image ID (short) · Created (relative) · Size · Status (*In use* chip if containers > 0) · Actions (Run, Delete). An image with N tags shows as N rows, as Docker Desktop does. Dangling images show as `<none>`. |
| IMG-002 | Sort by name, tag, created, or size. Filter: *All* / *In use* / *Unused* / *Dangling*. Search over repo:tag and id. |
| IMG-003 | The header shows the total size and the count. Overflow: *Prune dangling*, *Prune unused* (confirm with reclaimable size from `disk_usage` when available). |
| IMG-004 | *Pull image* dialog: reference input (`nginx:latest`). Progress is shown as a notification with per-layer bars (structured when `PULL_PROGRESS`, otherwise a status line). Cancellable. |
| IMG-005 | *Run* dialog (simple): container name, port mappings (host:container rows), env vars (rows), volume mounts (rows), *Remove when stopped*, *Start*. → `run_image`. On success, navigate to the new container detail. |
| IMG-006 | Delete: confirm. If in use → error explaining which containers use it, with a *Force* option. |
| IMG-007 | **Registry auth (v1).** Pull/push credentials are read from the user's Docker config (`~/.docker/config.json`): `auths` entries directly, and `credsStore` / `credHelpers` by running `docker-credential-<helper> get` (argv, stdin = server, on the hub runtime, 10 s timeout). For WSLC, the engine's own registry store is used (`wslc registry login`, outside Dockering). If no credentials are found, the pull is anonymous. An auth failure (401, or registry errors such as ghcr's 500 `denied` and Hub's `pull access denied`; 21 §6) shows "Authentication required: run `docker login <registry>`". In-app login UI is post-v1. Credentials are never logged or persisted by Dockering (NFR-020). |
| IMG-010 | **Detail tabs**: *Overview* (id, digests, tags, created, size, arch/os/variant, author, entrypoint, cmd, env, exposed ports, workdir, user, volumes, labels). *Layers* (history table: created by, size, created, comment; capability-gated). *Used by* (containers list, links). *Inspect* (raw JSON). |
| IMG-011 | Detail actions: Run, Tag… (dialog), Delete, Copy id, Copy digest. |
