#!/usr/bin/env bash
# Build the native musl CloudDesk release binaries (Phase 10D).
#
# Compiles clouddeskd/cloudesk-privd/cloudesk-sessiond natively inside
# packaging/docker/release-builder-musl.Dockerfile (Alpine 3.20, musl
# 1.2.5), targeting x86_64-unknown-linux-musl. This is a SEPARATE
# artifact family from build-release.sh's glibc output, not a
# replacement for it: musl and glibc are different libc implementations,
# not different versions of the same one -- a glibc-linked binary
# cannot even be loaded on a musl system (no /lib64/ld-linux-x86-64.so.2
# exists there), confirmed live against Alpine 3.20. Every declared
# glibc-family v1 target (Debian/Ubuntu/Fedora/RHEL9-family/Arch) keeps
# using dist/linux-x86_64-glibc, untouched by this script.
#
# Usage:
#   packaging/build-release-musl.sh [output-dir]
#
# output-dir defaults to dist/linux-x86_64-musl (gitignored).
# Requires Docker. Never touches the operator host's own toolchain or
# libc -- everything happens inside the disposable builder container.
set -Eeuo pipefail

REPO_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${1:-"$REPO_ROOT/dist/linux-x86_64-musl"}
mkdir -p "$OUT_DIR"
OUT_DIR=$(CDPATH= cd -- "$OUT_DIR" && pwd)
BUILDER_IMAGE="clouddesk-release-builder:musl"

RUST_VERSION=${CLOUDESK_RUST_VERSION:-1.97.1}

info() { printf '[+] %s\n' "$*"; }

info "Building musl release builder image ($BUILDER_IMAGE, Rust $RUST_VERSION)"
docker build \
    --build-arg "RUST_VERSION=$RUST_VERSION" \
    -t "$BUILDER_IMAGE" \
    -f "$REPO_ROOT/packaging/docker/release-builder-musl.Dockerfile" \
    "$REPO_ROOT/packaging/docker"

info "Builder musl / Rust:"
docker run --rm --entrypoint sh "$BUILDER_IMAGE" -c 'apk info musl 2>/dev/null | head -1'
docker run --rm --entrypoint rustc "$BUILDER_IMAGE" --version

TARGET_VOLUME="clouddesk-musl-target-$$"
docker volume create "$TARGET_VOLUME" >/dev/null
trap 'docker volume rm "$TARGET_VOLUME" >/dev/null 2>&1 || true' EXIT

info "Building clouddeskd / cloudesk-privd / cloudesk-sessiond (release, x86_64-unknown-linux-musl)"
docker run --rm \
    -v "$REPO_ROOT:/repo:ro" \
    -v "$TARGET_VOLUME:/build/target" \
    -e CARGO_TARGET_DIR=/build/target \
    "$BUILDER_IMAGE" \
    build --release --target x86_64-unknown-linux-musl \
    --manifest-path /repo/Cargo.toml \
    -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond

mkdir -p "$OUT_DIR"
docker run --rm \
    -v "$TARGET_VOLUME:/build/target:ro" \
    -v "$OUT_DIR:/out" \
    busybox sh -c \
    'cp /build/target/x86_64-unknown-linux-musl/release/clouddeskd /build/target/x86_64-unknown-linux-musl/release/cloudesk-privd /build/target/x86_64-unknown-linux-musl/release/cloudesk-sessiond /out/'

info "musl artifact SHA256:"
(cd "$OUT_DIR" && sha256sum clouddeskd cloudesk-privd cloudesk-sessiond | tee SHA256SUMS)

info "Linkage (expect static-pie, no dynamic dependencies):"
for bin in "$OUT_DIR"/clouddeskd "$OUT_DIR"/cloudesk-privd "$OUT_DIR"/cloudesk-sessiond; do
    printf '  %s: ' "$(basename "$bin")"
    file -b "$bin"
done

info "Done: $OUT_DIR"
