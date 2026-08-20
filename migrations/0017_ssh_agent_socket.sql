-- Phase 2 closure: SSH agent authentication needs a socket path
-- distinct from the existing `credential_secret_id` (no key material
-- is ever stored for agent auth -- only where to reach a real running
-- agent). Never a secret itself; the real security boundary is
-- enforced at connection time by checking the socket's owning UID
-- matches the server's owning CloudDesk user's real Linux UID.
ALTER TABLE remote_servers ADD COLUMN agent_socket_path TEXT;
