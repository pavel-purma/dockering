# Feature: Volumes

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** VOL
- **Plan:** no per-feature plan; built in milestone M6 of the [v1 plan](../../plan/README.md)

## Requirements

| ID | Requirement |
|---|---|
| VOL-001 | List columns: ☐ · Name · Driver · Compose project (if labelled) · Size · Status (*In use* pill, then "by N containers" as plain text) · Created · Actions (`⋮`; Delete is in the `⋮` menu, SHL-026). |
| VOL-002 | Size and in-use counts come from `disk_usage()`, loaded lazily on the Volumes page only, after the list renders, with skeleton cells in the meantime. Without `DISK_USAGE` (e.g. WSLC), or when the call fails, Size shows "—" and in-use is computed from container mounts. |
| VOL-003 | Sort by name, created, or size. Filter: *All* / *In use* / *Unused*. Search by name. |
| VOL-004 | *Create volume* dialog: name (optional), driver (default `local`), driver options (key/value rows), labels. |
| VOL-005 | Delete: confirm. If in use → error listing the containers. *Prune unused volumes* in overflow (confirm; shows the reclaimable size when sizes are known). |
| VOL-010 | **Detail tabs**: *Overview* (name, driver, mountpoint, scope, created, size, labels, options, status). *Used by* (containers mounting it, with destination path and RW, as links). *Inspect* (raw JSON). |
| VOL-011 | Browsing volume contents is a **non-goal** for v1. |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| VOL-001 | `vol_volume_summary`, `vol_volumes_snapshot`, `vol_list_volumes_enriched_by_inspect` (WSLC CLI) |
| VOL-002 | `vol_002_lazy_sizes`, `vol_002_dash_without_disk_usage`, `vol_002_sizes_only_from_disk_usage`, `vol_002_disk_usage_snapshot`, `vol_disk_usage_*` |
| VOL-003 | `vol_003_filter_search_sort` |
| VOL-004 | `vol_004_create_volume_with_n`, `vol_004_spec` |
| VOL-005 | `vol_005_delete_and_prune`, `vol_005_in_use_delete_fails_with_containers`, `vol_005_prune_reclaimable` |
| VOL-010 | `vol_010_detail_tabs_and_used_by`, `vol_used_by` |
| VOL-011 | Non-goal; nothing to verify |
