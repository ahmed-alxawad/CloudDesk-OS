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

# Publication Pass D found this live: vite's own build output is
# byte-identical across clean builds (content-hashed filenames, no
# embedded build timestamps), but a plain `tar -czf` is not -- each
# entry's mtime and gzip's own header timestamp both capture wall-clock
# build time, so two verified-identical builds still produced different
# archive bytes. Fixed by pinning every entry's mtime to the exact
# candidate commit's own commit time (not build time, and not "0" --
# using the commit timestamp keeps the archive's own metadata
# traceable to the source it was built from), a deterministic entry
# order, and numeric/zeroed ownership; `gzip -n` drops gzip's own
# timestamp/filename header fields.
SOURCE_EPOCH=${SOURCE_DATE_EPOCH:-$(cd "$REPO_ROOT" && git log -1 --format=%ct 2>/dev/null || echo 0)}

info "Packaging apps/web/dist -> $OUT_DIR/clouddesk-web.tar.gz (SOURCE_DATE_EPOCH=$SOURCE_EPOCH)"
tar -C "$REPO_ROOT/apps/web/dist" \
    --sort=name --mtime="@$SOURCE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -cf - . | gzip -n -9 >"$OUT_DIR/clouddesk-web.tar.gz"

info "Web bundle SHA256:"
sha256sum "$OUT_DIR/clouddesk-web.tar.gz"
