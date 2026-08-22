import { describe, expect, it } from 'vitest';
import {
  AUTH_METHODS,
  authMethodFieldsAreComplete,
  buildAuthPayload,
  secretKindForAuthMethod,
  secretValueForAuthMethod,
  type AuthMethodFields
} from './remoteServers';

function fields(overrides: Partial<AuthMethodFields> = {}): AuthMethodFields {
  return {
    agentSocketPath: '',
    password: '',
    privateKey: '',
    certKey: '',
    certCert: '',
    kiResponses: [''],
    ...overrides
  };
}

describe('AUTH_METHODS', () => {
  it('exposes exactly the five SSH auth methods the backend supports', () => {
    expect(AUTH_METHODS.map((m) => m.value)).toEqual([
      'ssh_agent',
      'password',
      'private_key',
      'certificate',
      'keyboard_interactive'
    ]);
  });
});

describe('secretKindForAuthMethod', () => {
  it('has no Vault secret for ssh_agent', () => {
    expect(secretKindForAuthMethod('ssh_agent')).toBeNull();
  });

  it('maps every credential-backed method to its ssh.* kind', () => {
    expect(secretKindForAuthMethod('password')).toBe('ssh.password');
    expect(secretKindForAuthMethod('private_key')).toBe('ssh.private_key');
    expect(secretKindForAuthMethod('certificate')).toBe('ssh.certificate');
    expect(secretKindForAuthMethod('keyboard_interactive')).toBe(
      'ssh.keyboard_interactive'
    );
  });

  it('rejects an unknown method rather than silently guessing', () => {
    expect(() => secretKindForAuthMethod('nonsense')).toThrow();
  });
});

describe('secretValueForAuthMethod', () => {
  it('sends the password as a plain raw string, never JSON-quoted', () => {
    const value = secretValueForAuthMethod(
      'password',
      fields({ password: 'hunter2' })
    );
    expect(value).toBe('hunter2');
    expect(value).not.toContain('"');
  });

  it('sends the private key as raw PEM text', () => {
    const pem =
      '-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----';
    expect(
      secretValueForAuthMethod('private_key', fields({ privateKey: pem }))
    ).toBe(pem);
  });

  it('packs certificate auth as key_data + cert_data JSON, matching CertificateCredential', () => {
    const value = secretValueForAuthMethod(
      'certificate',
      fields({ certKey: 'KEY', certCert: 'CERT' })
    );
    expect(JSON.parse(value)).toEqual({ key_data: 'KEY', cert_data: 'CERT' });
  });

  it('never treats a certificate as a substitute for its private key', () => {
    const value = secretValueForAuthMethod(
      'certificate',
      fields({ certKey: 'KEY', certCert: 'CERT' })
    );
    const parsed = JSON.parse(value) as { key_data: string; cert_data: string };
    expect(parsed.key_data).not.toBe(parsed.cert_data);
    expect(parsed).toHaveProperty('key_data');
    expect(parsed).toHaveProperty('cert_data');
  });

  it('packs keyboard-interactive responses as an ordered JSON array', () => {
    const value = secretValueForAuthMethod(
      'keyboard_interactive',
      fields({ kiResponses: ['first', 'second'] })
    );
    expect(JSON.parse(value)).toEqual(['first', 'second']);
  });

  it('filters out empty keyboard-interactive rows before sending', () => {
    const value = secretValueForAuthMethod(
      'keyboard_interactive',
      fields({ kiResponses: ['first', '', 'second', ''] })
    );
    expect(JSON.parse(value)).toEqual(['first', 'second']);
  });

  it('is never called for ssh_agent (no secret to build)', () => {
    expect(() => secretValueForAuthMethod('ssh_agent', fields())).toThrow();
  });
});

describe('authMethodFieldsAreComplete', () => {
  it('requires an agent socket path for ssh_agent', () => {
    expect(authMethodFieldsAreComplete('ssh_agent', fields())).toBe(false);
    expect(
      authMethodFieldsAreComplete(
        'ssh_agent',
        fields({ agentSocketPath: '/run/user/1000/ssh-agent' })
      )
    ).toBe(true);
  });

  it('requires a non-empty password for password auth', () => {
    expect(authMethodFieldsAreComplete('password', fields())).toBe(false);
    expect(
      authMethodFieldsAreComplete('password', fields({ password: 'x' }))
    ).toBe(true);
  });

  it('requires both key and certificate for certificate auth -- neither substitutes for the other', () => {
    expect(
      authMethodFieldsAreComplete('certificate', fields({ certKey: 'KEY' }))
    ).toBe(false);
    expect(
      authMethodFieldsAreComplete('certificate', fields({ certCert: 'CERT' }))
    ).toBe(false);
    expect(
      authMethodFieldsAreComplete(
        'certificate',
        fields({ certKey: 'KEY', certCert: 'CERT' })
      )
    ).toBe(true);
  });

  it('requires at least one non-empty keyboard-interactive response', () => {
    expect(
      authMethodFieldsAreComplete(
        'keyboard_interactive',
        fields({ kiResponses: [''] })
      )
    ).toBe(false);
    expect(
      authMethodFieldsAreComplete(
        'keyboard_interactive',
        fields({ kiResponses: ['', 'x'] })
      )
    ).toBe(true);
  });
});

describe('buildAuthPayload', () => {
  it('never includes raw secret material -- only an opaque secret ID', () => {
    const payload = buildAuthPayload('password', 'secret-abc123', null);
    expect(payload).toEqual({
      auth_method: 'password',
      credential_secret_id: 'secret-abc123',
      agent_socket_path: null
    });
    expect(JSON.stringify(payload)).not.toContain('hunter2');
  });

  it('sends only the agent socket path for ssh_agent, never a secret ID', () => {
    const payload = buildAuthPayload(
      'ssh_agent',
      null,
      '/run/user/1000/ssh-agent'
    );
    expect(payload).toEqual({
      auth_method: 'ssh_agent',
      credential_secret_id: null,
      agent_socket_path: '/run/user/1000/ssh-agent'
    });
  });

  it('ignores an agent_socket_path passed for a non-agent method', () => {
    const payload = buildAuthPayload(
      'certificate',
      'secret-xyz',
      '/should/be/ignored'
    );
    expect(payload.agent_socket_path).toBeNull();
  });
});
