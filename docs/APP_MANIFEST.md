# Application manifests

Built-in applications register through JSON manifests validated by both
`schemas/app-manifest.schema.json` and the `clouddesk-runtime` crate.

Required fields are:

- `id` — lowercase stable identifier;
- `name` and `icon` — presentation metadata only;
- `route` — frontend route beginning with `/`;
- `required_permissions` — entries from the backend capability registry;
- `file_associations` — MIME patterns handled by the app;
- `runtime_dependency` — `browser`, `code`, `office`, `media`, or `null`;
- `enabled` — administrative availability, never an authorization grant.

The backend must still authorize each operation. A valid or enabled manifest does
not grant a user access. Optional runtime processes must not remain resident while
their runtime is disabled.

