# Development standards

## Supported tools

- stable Rust, formatted with `rustfmt` and linted with Clippy;
- Svelte and strict TypeScript, built by Vite;
- Prettier and `svelte-check` for frontend formatting and static checks;
- Rust unit/integration tests and Vitest frontend unit tests.

Run `make check` and `make test` before submitting a change. CI runs the same
checks. New schema changes require a new immutable SQL file in `migrations/`;
never edit an already released migration.

## Module boundaries

Business rules belong in small workspace crates. `cloudeskd` owns process
startup, transport, and composition. The frontend never grants authority: it may
hide unavailable actions, but every API or WebSocket operation must be authorized
by the backend.

New dependencies need a concrete purpose and a license compatible with the
project. Always-on services are not introduced when an in-process or on-demand
component is sufficient.

## Security review checklist

- Does the operation have a named capability?
- Is authorization enforced at the backend boundary?
- Is every browser-provided identifier treated as untrusted?
- Does the operation need an audit event?
- Could a secret reach logs, SQLite plaintext, or an API response?
- Does local work run under the correct Linux UID/GID?
- Are symlink, traversal, and time-of-check/time-of-use cases tested?


## Running the tests

CloudDesk's acceptance suites drive real external fixtures -- an SSH
server, MinIO, WebDAV, Collabora, Brave, a disposable privileged Linux
identity for the Code runtime. Those fixtures are legitimately absent on
an ordinary developer machine, and Rust's test harness has no way to say
so: a test that returns is reported as `ok`, whether it exercised the
product or skipped entirely.

CloudDesk therefore records a third state itself. The vocabulary is the
project's existing one:

```text
PASS                    the product path ran and its assertions held
FAIL                    the product path ran and something was wrong
BLOCKED_BY_ENVIRONMENT  the fixture was unavailable; nothing was proven
```

`crates/test-support` emits a marker for the third case, both to stdout
and to an appended status log:

```text
CLOUDDESK_TEST_STATUS=BLOCKED_BY_ENVIRONMENT CLOUDDESK_TEST_REASON=<code> CLOUDDESK_TEST_NAME=<test>
```

The log is the reliable channel, because the harness swallows stdout for
tests it counts as passing -- which, in normal mode, is exactly the
blocked ones.

### Developer / environment-tolerant

Missing fixtures are reported and the run continues, so unrelated tests
still give useful signal:

```bash
rm -f target/clouddesk-test-status.log
cargo test --workspace --no-fail-fast 2>&1 | tee run.log
scripts/test-status.sh run.log
```

`scripts/test-status.sh` reports real PASS, BLOCKED BY ENVIRONMENT (by
reason code) and FAIL separately. Exit codes: `0` clean, `1` something
was blocked, `2` something failed.

**Do not read a green `cargo test` here as "everything passed."** Every
blocked test is counted by the harness as passed; only the marker log
distinguishes them. That is precisely the false green this mechanism
exists to remove -- `ssh_advanced_auth` once "passed" 12 tests in 0.11 s
against no SSH server at all, where a live run takes ~13 s.

### Strict release acceptance

Every mandatory fixture must be present; a missing one is a hard,
deterministic failure rather than a marker:

```bash
CLOUDDESK_REQUIRE_LIVE_ACCEPTANCE=1 cargo test --workspace --no-fail-fast
```

Strict mode keys off an exact `1`. It is deliberately **not** inferred
from `CI`, because this repository has no such convention and release
strictness should not depend on an unrelated variable.

Bring the disposable fixture stack up first:

```bash
cd tests/acceptance && docker compose up -d
```

Note that strict mode **cannot currently pass on a cleaned host**: the
privileged Code test identity (`clouddesk-code-test`, uid/gid 963,
`/var/lib/clouddesk-code-test`, and the root-owned
`cloudesk-sessiond-test` helper) was deliberately removed after Phase 7,
and recreating it is a privileged operation requiring explicit operator
approval. Until it is re-provisioned, the Code suites report
`CODE_PRIVILEGED_TEST_IDENTITY_UNAVAILABLE`.

### Reason codes

| Code | Meaning |
| --- | --- |
| `CODE_PRIVILEGED_TEST_IDENTITY_UNAVAILABLE` | the disposable privileged Code identity and/or its root-owned sessiond helper is not provisioned |
| `SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE` | `tests/acceptance/docker-compose.yml` stack is not running |
| `CONTAINER_RUNTIME_UNAVAILABLE` | Docker, or a required prebuilt image, is unavailable |
| `LINUX_IDENTITY_UNAVAILABLE` | the test process cannot map a real, non-root Linux user |
| `MEDIA_TOOLING_UNAVAILABLE` | `ffmpeg`/`ffprobe` is not installed |
