#!/usr/bin/env bash
# Build the web frontend and package it as the one platform-independent
# release artifact the installer's direct-fetch mode needs alongside the
# three native binaries -- without it, a freshly fetched clouddeskd has
# no static_dir to serve and install.sh's existing
# `[ -f "$web_source/index.html" ] || fail` check would reject the install.
#
# Usage:
#   packaging/build-web.sh [output-dir]
#
# output-dir defaults to dist/ (gitignored). Produces
# <output-dir>/clouddesk-web.tar.gz containing exactly the contents of
# apps/web/dist (no top-level directory inside the tarball, so it
# extracts flat into whatever directory the installer points it at).
set -Eeuo pipefail

REPO_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${1:-"$REPO_ROOT/dist"}
mkdir -p "$OUT_DIR"
OUT_DIR=$(CDPATH= cd -- "$OUT_DIR" && pwd)

info() { printf '[+] %s\n' "$*"; }

info "Building web frontend (apps/web)"
(cd "$REPO_ROOT/apps/web" && npm ci && npm run build)

[ -f "$REPO_ROOT/apps/web/dist/index.html" ] || {
    printf 'build-web.sh: apps/web/dist/index.html missing after build\n' >&2
    exit 1
}

info "Packaging apps/web/dist -> $OUT_DIR/clouddesk-web.tar.gz"
tar -C "$REPO_ROOT/apps/web/dist" -czf "$OUT_DIR/clouddesk-web.tar.gz" .

info "Web bundle SHA256:"
sha256sum "$OUT_DIR/clouddesk-web.tar.gz"
