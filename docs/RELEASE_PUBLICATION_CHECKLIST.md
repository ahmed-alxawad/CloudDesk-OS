# Release Publication Checklist (operator-run, no secrets recorded here)

This is the exact sequence a maintainer should follow to publish a
CloudDesk-OS release candidate once GitHub hosting and a signing identity
are established. Nothing on this list has been executed as an actual
publication by any engineering pass — see `PHASE17_RELEASE_PUBLICATION_CLOSURE.md`
for what has and has not been done.

1. **Confirm the exact tag target.** `git rev-parse <tag>^{commit}` and
   compare against the release evidence manifest's `source_commit`. They
   must match exactly.
2. **Confirm the tag is immutable.** `git cat-file -p <tag>` shows an
   annotated tag object, unsigned unless intentionally signed, and has not
   been force-updated (`git reflog` on the tag ref, if concerned).
3. **Clean exact-source build.** Run `packaging/build-release.sh` and
   `packaging/build-release-musl.sh` from a clean checkout of exactly that
   tag (not the working tree, not a later commit) — twice each — and
   confirm both pairs are byte-identical.
4. **Verify hashes** against any previously published/expected values for
   that exact candidate, if this is a rebuild rather than a first build.
5. **Generate and verify the SBOM** (`packaging/gen-sbom.py <version>`) —
   confirm the component count matches expectations or explain the delta.
6. **Verify LICENSE and third-party notices** are present at the tagged
   commit (`git cat-file -e <tag>:LICENSE`, etc.) and are current.
7. **Run the release-staging validator**
   (`tests/distro/release-staging-validation.sh <tag> <version>`) — must
   PASS before continuing.
8. **Produce the authenticated signature/attestation.** For GitHub
   Attestations: push the tag to the real remote, let
   `.github/workflows/release-attest.yml` run, confirm it succeeded. For
   minisign (complementary): sign `SHA256SUMS` with the offline private
   key, producing `SHA256SUMS.minisig`.
9. **Verify the signature/attestation yourself** before publishing it —
   `gh attestation verify` and/or `minisign -Vm` against the actual
   produced files, not a copy-pasted expectation.
10. **Publish the immutable artifacts** (binaries, installer, SHA256SUMS,
    SBOM, attestation/signature) to the chosen host, at a versioned path
    that will never be overwritten in place.
11. **Verify the public download path** from a machine with no special
    trust of the build environment: fetch each file over the real public
    URL, re-verify checksums and signature/attestation against the freshly
    downloaded bytes.
12. **Only then** publish the install command (e.g. update README/docs
    with the real `curl -fsSL <url> | sudo bash` line). Never publish the
    install command before step 11 has passed.

No credentials, keys, or tokens are recorded in this document or should
ever be added to it.
