# CloudDesk-OS threat model

## Scope and security objectives

CloudDesk exposes Linux files, terminals, remote systems, and administrative
actions through a browser. Its security objective is to preserve Linux identity
and permission semantics while preventing a web user, compromised session, or
optional runtime from gaining authority beyond explicit backend grants.

Protected assets include local and remote files, Linux account authority,
credentials and encryption keys, session tokens, audit history, transfer data,
system configuration, and the availability of the host.

## Trust boundaries

1. The browser is untrusted presentation and control input.
2. `cloudeskd` is an unprivileged policy-enforcement and orchestration process.
3. `cloudesk-sessiond` will cross into a mapped user's UID/GID and filesystem view.
4. `cloudesk-privd` will be a minimal root-owned helper on a Unix socket.
5. Optional Browser, Code, Office, and media runtimes are isolated workers.
6. Remote SSH, WebDAV, and S3 systems have independent identities and trust.
7. SQLite and the encrypted vault are persistent security boundaries.

No frontend state, app manifest, path, MIME type, proxy header, or WebSocket
message is itself trusted authorization evidence.

## Threats and required controls

| Threat | Required controls |
| --- | --- |
| Stolen credentials or sessions | Argon2id, optional TOTP, rotation/revocation, rate limits, secure cookies, short-lived step-up grants |
| Horizontal or vertical privilege escalation | Backend RBAC and capability checks on every REST/WebSocket operation; explicit assigned roots |
| Root command injection | No generic command API; enumerated typed `privd` operations; strict argument validation; root-owned Unix socket |
| Path traversal, symlink races, TOCTOU | Opaque file IDs, provider-bound authorization, canonicalization beneath assigned roots, descriptor-relative operations, dedicated security tests |
| Secret disclosure | Envelope encryption at rest, master key outside SQLite, log redaction, authorization and audit on reveal/export |
| Audit tampering | Append-only events, tamper-evident hash chain, restricted writer/reader paths, export verification |
| Untrusted file previews and archives | Sandboxed/isolated parsing, safe response headers, no automatic extraction outside authorized roots |
| SSH impersonation | Host-key verification by default and fail-closed handling of changed keys |
| Malicious optional runtime | Per-user isolation, bounded resources, restricted filesystem mounts, no resident process while disabled |
| Transfer data exposure | Server-side data plane; never relay remote-to-remote payloads through the browser; encrypted credentials |
| Proxy/header spoofing | Honor forwarded headers only from configured trusted proxies |
| Denial of service | Bounded pools/queues, request limits, runtime resource controls, cancellation and backpressure |

## Architectural invariants

- `cloudeskd` never runs permanently as root and receives no Linux capabilities by default.
- There is no arbitrary privileged shell/command endpoint.
- Authorization is server-side and security-relevant operations are audited.
- Passwords, private keys, recovery material, and remote credentials are never
  stored plaintext or emitted to logs.
- Linux UID/GID, ownership, mode bits, and ACL behavior remain authoritative.
- Root-scope file access requires explicit permission and recent step-up approval.
- WebSockets repeat authentication and authorization checks.
- Remote-to-remote transfer data remains on the server-side path.
- Disabled optional runtimes consume no resident application resources.

Phase 0 represents enforceable invariants through root-process refusal, a closed
capability registry, strict config/manifest parsing, and security tests. Later
phases must extend those tests at every real boundary.

## Assumptions and limits

The host kernel, boot chain, root account, and installer are trusted. A fully
compromised root account or kernel can ultimately read process memory and master
keys; CloudDesk protects secrets at rest and from lower-privilege compromise, not
from an already-controlled host. Browser extensions, user endpoint compromise,
and remote service compromise are external risks mitigated but not eliminated by
CloudDesk.

TLS provisioning, authentication, `privd`, mapped-user workers, vault encryption,
and tamper-evident audit storage are not implemented in Phase 0. Their absence is
tracked as an implementation limitation, not treated as a relaxed security rule.

