// Pure, unit-testable logic for the Remote Server Manager's SSH
// auth-method configuration (PASS SSH-A-2, Task 37). The Svelte
// component stays a thin fetch/render shell -- this project has no
// component-rendering harness, so anything that decides *what* to send
// lives here instead (`office.ts`/`runtime.ts`/`video.ts`/`music.ts`
// follow the same split).

export const AUTH_METHODS = [
  { value: 'ssh_agent', label: 'SSH agent' },
  { value: 'password', label: 'Password' },
  { value: 'private_key', label: 'Private key' },
  { value: 'certificate', label: 'Certificate (key + cert)' },
  { value: 'keyboard_interactive', label: 'Keyboard-interactive' }
] as const;

export type AuthMethod = (typeof AUTH_METHODS)[number]['value'];

export interface AuthMethodFields {
  agentSocketPath: string;
  password: string;
  privateKey: string;
  certKey: string;
  certCert: string;
  kiResponses: string[];
}

/** The Vault secret `kind` an auth method's credential is stored
 * under, or `null` for `ssh_agent` (no secret -- only a socket path,
 * never key material, lives on the `RemoteServer` itself). Matches
 * `worker.rs::resolve_auth`'s per-method expectations exactly. */
export function secretKindForAuthMethod(method: string): string | null {
  switch (method) {
    case 'ssh_agent':
      return null;
    case 'password':
      return 'ssh.password';
    case 'private_key':
      return 'ssh.private_key';
    case 'certificate':
      return 'ssh.certificate';
    case 'keyboard_interactive':
      return 'ssh.keyboard_interactive';
    default:
      throw new Error(`Unknown auth method: ${method}`);
  }
}

/** The exact raw bytes to send as a Vault secret's `value` for a given
 * auth method -- a plain string for password/private-key, a
 * JSON-encoded object for certificate (`{key_data, cert_data}`,
 * matching `CertificateCredential`), and a JSON-encoded array of
 * non-empty responses for keyboard-interactive (matching the
 * `Vec<String>` `worker.rs` deserializes). Never called for
 * `ssh_agent`, which has no secret. */
export function secretValueForAuthMethod(
  method: string,
  fields: AuthMethodFields
): string {
  switch (method) {
    case 'password':
      return fields.password;
    case 'private_key':
      return fields.privateKey;
    case 'certificate':
      return JSON.stringify({
        key_data: fields.certKey,
        cert_data: fields.certCert
      });
    case 'keyboard_interactive':
      return JSON.stringify(
        fields.kiResponses.filter((response) => response.length > 0)
      );
    default:
      throw new Error(`Auth method ${method} has no Vault secret`);
  }
}

/** Client-side mirror of the server's `validate_auth_fields` --
 * catches an incomplete form before a round trip, never the sole
 * enforcement (the API validates independently and is what actually
 * protects the data). */
export function authMethodFieldsAreComplete(
  method: string,
  fields: AuthMethodFields
): boolean {
  switch (method) {
    case 'ssh_agent':
      return fields.agentSocketPath.trim().length > 0;
    case 'password':
      return fields.password.length > 0;
    case 'private_key':
      return fields.privateKey.trim().length > 0;
    case 'certificate':
      return (
        fields.certKey.trim().length > 0 && fields.certCert.trim().length > 0
      );
    case 'keyboard_interactive':
      return fields.kiResponses.some((response) => response.length > 0);
    default:
      return false;
  }
}

/** The exact auth-related subset of the `RemoteServer` create/update
 * body -- built from an already-obtained opaque `secretId` (from
 * `POST /api/v1/vault/secrets`) or `agentSocketPath`, never from a raw
 * password/key/response directly. Asserting this shape is what proves
 * secret values are never sent to `/api/v1/remote/servers` itself
 * (Part G): only an ID, or a filesystem path that was never a secret. */
export function buildAuthPayload(
  method: string,
  secretId: string | null,
  agentSocketPath: string | null
): {
  auth_method: string;
  credential_secret_id: string | null;
  agent_socket_path: string | null;
} {
  return {
    auth_method: method,
    credential_secret_id: method === 'ssh_agent' ? null : secretId,
    agent_socket_path: method === 'ssh_agent' ? agentSocketPath : null
  };
}
