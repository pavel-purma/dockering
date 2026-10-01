# Feature: Networks

- **Status:** planned
- **Requirement prefix:** NET
- **Plan:** [docs/plan/features/networks.md](../../plan/features/networks.md) (to be written)

## Requirements

| ID | Requirement |
|---|---|
| NET-001 | Sidebar entry *Networks*, which can be hidden in Settings ("Show Networks page", default on). |
| NET-002 | List columns: Name · Driver · Scope · Subnet(s) · Gateway · Containers (count) · Compose project · Created · Actions (Delete; disabled for the built-in `bridge`, `host`, and `none`). |
| NET-003 | **Detail tabs**: *Overview* (id, driver, scope, internal, attachable, IPv6, IPAM config, options, labels). *Containers* (name link, IPv4, IPv6, MAC). *Inspect* (raw JSON). |
| NET-004 | *Prune unused networks* in overflow (confirm). |
| NET-005 | Per-container networking is primarily shown in the container detail **Network** tab (CDT-030). Creating networks and connecting or disconnecting containers is out of scope for v1. |
