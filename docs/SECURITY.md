# CloudDesk-OS Security Architecture & Threat Model

CloudDesk-OS is engineered around **strict defense-in-depth principles**, minimizing the blast radius of any individual compromised component.

---

## 1. Privilege Separation

```
┌─────────────────────────────────────────────────────────────┐
│                    User Browser Client                      │
└──────────────────────────────┬──────────────────────────────┘
                               │ HTTPS / WSS :9870
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                   clouddeskd (Unprivileged)                 │
│         Runs as user 'clouddesk' (UID != 0, GID != 0)       │
│  - Web server, session management, routing, VFS proxy       │
│  - Cannot directly perform root actions                     │
└──────────────────────────────┬──────────────────────────────┘
                               │ Root-owned Unix Socket
                               │ (/run/clouddesk/privd.sock)
                               │ + HMAC-SHA256 Grant Token
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 cloudesk-privd (Privileged)                 │
│         Runs as root (UID 0) with strict capabilities       │
│  - Spawns mapped Linux identity workers (`setpriv`)         │
│  - Strict typed enum operations only (No generic root shell)│
└─────────────────────────────────────────────────────────────┘
```

1. **`clouddeskd` is Never Root**: The web server process runs permanently as an unprivileged service user (`clouddesk`). It drops all root capabilities upon startup.
2. **Narrow Typed IPC**: `cloudesk-privd` only accepts strongly-typed, schema-validated IPC requests (`PrivdRequest`). There is no endpoint or mechanism for executing arbitrary shell strings or commands as root.
3. **Signed Grant Tokens**: Every privileged request requires an HMAC-SHA256 signed grant with strict expiration (default 30 seconds), client IP binding, and capability verification.

---

## 2. Vault Per-Record Envelope Encryption

```
  Master KEK (/etc/clouddesk/keys/master.key)
              │
              ▼ AES-256-GCM [AAD: vault:key:{owner}:{id}]
  Wrapped Per-Record DEK (32 bytes)
              │
              ▼ AES-256-GCM [AAD: vault:value:{owner}:{id}]
       Secret Ciphertext
```

1. **Zero Plaintext Secrets**: Passwords, API tokens, and SSH private keys are never stored in plaintext in SQLite or memory longer than strictly needed.
2. **Per-Record Isolation**: Each secret record is encrypted with a distinct, cryptographically random 32-byte Data Encryption Key (DEK).
3. **Authenticated Additional Data (AAD)**: Prevents ciphertext or DEK transposition across user boundaries or record IDs.
4. **Cryptographic Erasure**: Deleting a record overwrites the ciphertext and wrapped DEK with random tombstones inside a transaction before deleting the row.
5. **Zero-Downtime KEK Rotation**: Master keys can be rotated by rewrapping only the 32-byte DEKs without exposing or decrypting secret values.

---

## 3. Sandboxed Virtual Filesystem (VFS)

1. **Capability-Based Roots**: VFS access is bounded using `cap_std` ambient capability drop.
2. **Anti-Traversal & Symlink Escapes**: Path normalization strictly forbids `../`, encoded traversals, and following symlinks outside the mapped user home or assigned storage roots.
3. **Direct Linux Identity Mapping**: File operations are executed with the user's authentic Linux UID/GID.

---

## 4. Web & Session Security

- **Argon2id Password Hashing**: State-of-the-art memory-hard password verification.
- **TOTP Step-Up Authentication**: High-risk operations (power, services, password rotations, secret reveal) require recent step-up authentication.
- **Cookies**: `HttpOnly`, `SameSite=Lax` (or `Strict`), `Secure`.
- **Anti-CSRF & CORS**: `sec-fetch-site` cross-site validation and origin verification.
- **Security Headers**: `x-content-type-options: nosniff`, `x-frame-options: DENY`, `referrer-policy: no-referrer`.

---

## 5. Security Vulnerability Reporting

Please report security vulnerabilities directly to `security@clouddesk.internal` or via private GitHub Security Advisory.
