#!/usr/bin/env bash
# Build glibc-portable CloudDesk release binaries (Phase 10B).
#
# Compiles clouddeskd/cloudesk-privd/cloudesk-sessiond inside
# packaging/docker/release-builder.Dockerfile -- Rocky Linux 9, glibc
# 2.34, the OLDEST glibc across every currently declared v1 glibc-family
# target (confirmed live: RHEL 9 / Rocky 9 / Alma 9 = 2.34, Debian 12 =
# 2.36, Ubuntu 24.04 = 2.39, Fedora 41 = 2.40, Debian 13 = 2.41). A
# binary linked against symbols no newer than 2.34 runs unmodified on
# all of them.
#
# This exists because the operator host's own glibc (whatever it is)
# must never leak into the release artifact: a binary built on a
# bleeding-edge host requires that host's newer GLIBC symbols and
# fails to even load on an older-glibc target with
# "GLIBC_X.Y not found" -- confirmed live against Rocky Linux 9 with a
# host-built (glibc 2.43) binary before this builder existed.
#
# Usage:
#   packaging/build-release.sh [output-dir]
#
# output-dir defaults to dist/linux-x86_64-glibc (gitignored).
# Requires Docker. Never touches the operator host's own toolchain or
# glibc -- everything happens inside the disposable builder container.
set -Eeuo pipefail

REPO_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${1:-"$REPO_ROOT/dist/linux-x86_64-glibc"}
# `docker run -v` requires an absolute path for a host bind mount; a
# relative one is misparsed as a named-volume request instead.
mkdir -p "$OUT_DIR"
OUT_DIR=$(CDPATH= cd -- "$OUT_DIR" && pwd)
BUILDER_IMAGE="clouddesk-release-builder:rocky9"

# No repository-pinned Rust toolchain exists yet (no rust-toolchain.toml
# at the repo root) -- this is reproducibility debt, not something this
# script silently papers over. RUST_VERSION defaults to matching
# whatever toolchain last produced a verified-working build; override
# explicitly if that changes, and consider adding a real
# rust-toolchain.toml so this default stops being necessary.
RUST_VERSION=${CLOUDESK_RUST_VERSION:-1.97.1}

info() { printf '[+] %s\n' "$*"; }

info "Building portable release builder image ($BUILDER_IMAGE, Rust $RUST_VERSION)"
docker build \
    --build-arg "RUST_VERSION=$RUST_VERSION" \
    -t "$BUILDER_IMAGE" \
    -f "$REPO_ROOT/packaging/docker/release-builder.Dockerfile" \
    "$REPO_ROOT/packaging/docker"

info "Builder glibc / Rust:"
docker run --rm --entrypoint sh "$BUILDER_IMAGE" -c 'ldd --version | head -1'
docker run --rm --entrypoint rustc "$BUILDER_IMAGE" --version

TARGET_VOLUME="clouddesk-portable-target-$$"
docker volume create "$TARGET_VOLUME" >/dev/null
trap 'docker volume rm "$TARGET_VOLUME" >/dev/null 2>&1 || true' EXIT

info "Building clouddeskd / cloudesk-privd / cloudesk-sessiond (release)"
docker run --rm \
    -v "$REPO_ROOT:/repo:ro" \
    -v "$TARGET_VOLUME:/build/target" \
    -e CARGO_TARGET_DIR=/build/target \
    --entrypoint cargo \
    "$BUILDER_IMAGE" \
    build --release --manifest-path /repo/Cargo.toml \
    -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond

mkdir -p "$OUT_DIR"
docker run --rm \
    -v "$TARGET_VOLUME:/build/target:ro" \
    -v "$OUT_DIR:/out" \
    busybox sh -c \
    'cp /build/target/release/clouddeskd /build/target/release/cloudesk-privd /build/target/release/cloudesk-sessiond /out/'

info "Portable artifact SHA256:"
sha256sum "$OUT_DIR"/clouddeskd "$OUT_DIR"/cloudesk-privd "$OUT_DIR"/cloudesk-sessiond

info "Highest required GLIBC symbol per binary:"
for bin in "$OUT_DIR"/clouddeskd "$OUT_DIR"/cloudesk-privd "$OUT_DIR"/cloudesk-sessiond; do
    printf '  %s: ' "$(basename "$bin")"
    objdump -T "$bin" 2>/dev/null | grep -oE 'GLIBC_[0-9.]+' | sort -V -u | tail -1
done

info "Done: $OUT_DIR"
