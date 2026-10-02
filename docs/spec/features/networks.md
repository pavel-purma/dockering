# Feature: Networks

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** NET
- **Plan:** no per-feature plan; built in milestone M6 of the [v1 plan](../../plan/README.md)

## Requirements

| ID | Requirement |
|---|---|
| NET-001 | Sidebar entry *Networks*, which can be hidden in Settings ("Show Networks page", default on). |
| NET-002 | List columns: Name · Driver · Scope · Subnet(s) · Gateway · Containers (count) · Compose project · Created · Actions (Delete; disabled for the built-in `bridge`, `host`, and `none`, and without `NETWORK_MGMT`). |
| NET-003 | **Detail tabs**: *Overview* (id, driver, scope, internal, attachable, IPv6, IPAM config, options, labels). *Containers* (name link, IPv4, IPv6, MAC). *Inspect* (raw JSON). |
| NET-004 | *Prune unused networks* in overflow (confirm). |
| NET-005 | Per-container networking is primarily shown in the container detail **Network** tab (CDT-030). Creating networks and connecting or disconnecting containers is out of scope for v1. |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| NET-001 | `net_001_hidden_networks_page`, `set_001_networks_page_hidden_from_sidebar` |
| NET-002 | `net_002_builtin_not_deletable`, `net_002_delete_custom_network_needs_network_mgmt`, `net_002_rows_counts_and_builtin`, `net_networks_snapshot`, `net_list_networks_enriched_by_inspect` (WSLC CLI) |
| NET-003 | `net_003_detail_tabs_and_container_links`, `net_network_summary_and_details` |
| NET-004 | `net_004_prune_unused_networks` |
| NET-005 | Scope statement; CDT-030 covers the container side |
