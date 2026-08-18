-- Phase 7 Task 2 (multiple Code workspaces). Code-specific persistence for
-- "last selected workspace" -- deliberately NOT added to the Phase 6
-- orchestrator's generic runtime_instances table, which stays runtime-kind
-- agnostic. Workspace identity itself continues to be the existing
-- assigned_roots.id (no raw host path ever crosses the browser boundary);
-- this table only remembers which assigned_roots row a user last opened in
-- Code, so a restart can reopen it. Revocation is enforced by re-resolving
-- last_workspace_id against assigned_roots on every use, not by anything
-- stored here.
CREATE TABLE code_user_state (
    owner_user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    last_workspace_id TEXT,
    updated_at INTEGER NOT NULL
);
