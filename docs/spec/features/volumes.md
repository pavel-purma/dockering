# Feature: Volumes

- **Status:** planned
- **Requirement prefix:** VOL
- **Plan:** [docs/plan/features/volumes.md](../../plan/features/volumes.md) (to be written)

## Requirements

| ID | Requirement |
|---|---|
| VOL-001 | List columns: ☐ · Name · Driver · Compose project (if labelled) · Created · Size · Status (*In use* by N containers) · Actions (Delete). |
| VOL-002 | Size and in-use counts come from `disk_usage()`, loaded lazily after the list renders, with skeleton cells in the meantime. Without `DISK_USAGE` (e.g. WSLC), Size shows "—" and in-use is computed from container mounts. |
| VOL-003 | Sort by name, created, or size. Filter: *All* / *In use* / *Unused*. Search by name. |
| VOL-004 | *Create volume* dialog: name (optional), driver (default `local`), driver options (key/value rows), labels. |
| VOL-005 | Delete: confirm. If in use → error listing the containers. *Prune unused volumes* in overflow (confirm, shows reclaimable size). |
| VOL-010 | **Detail tabs**: *Overview* (name, driver, mountpoint, scope, created, size, labels, options, status). *Used by* (containers mounting it, with destination path and RW, as links). *Inspect* (raw JSON). |
| VOL-011 | Browsing volume contents is a **non-goal** for v1. |
