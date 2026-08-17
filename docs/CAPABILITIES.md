# Capability registry

Capabilities are stable backend authorization identifiers. Roles are collections
of capabilities; code must not grant behavior solely because a role has a familiar
name. The canonical machine-readable registry is `crates/permissions/src/lib.rs`.

| Domain | Capabilities |
| --- | --- |
| Local files | `files.local.read`, `files.local.write`, `files.root.request`, `files.permissions.change` |
| Remote systems | `remote.servers.read`, `remote.servers.manage`, `remote.terminal.open` |
| Transfers | `transfers.create`, `transfers.cancel` |
| Terminal | `terminal.local.open` |
| Optional apps | `apps.browser.use`, `apps.code.use`, `apps.office.use` |
| System | `system.services.manage`, `system.packages.manage`, `system.firewall.manage`, `system.power.manage` |
| Administration | `users.manage`, `roles.manage`, `audit.read`, `secrets.manage` |

Adding a capability requires backend enforcement tests, audit classification,
documentation, and an explicit migration when persisted grants already exist.
There is intentionally no generic root shell or command capability.

