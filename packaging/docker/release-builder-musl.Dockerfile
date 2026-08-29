# Native musl release builder (Phase 10D).
#
# Base: Alpine Linux 3.20 (musl 1.2.5), pinned to an exact digest -- not
# the floating `alpine:3.20` tag, for the same reason the glibc builder
# pins rockylinux: the tag is mutable and would silently change the
# builder's musl baseline out from under whoever runs it next.
#
# This is a SEPARATE artifact family from release-builder.Dockerfile's
# glibc output, not a replacement for it: Alpine is musl-based, not an
# older glibc, and the existing glibc artifact (dist/linux-x86_64-glibc)
# is untouched by anything in this file.
FROM alpine@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc

RUN apk add --no-cache \
        musl-dev gcc make cmake perl pkgconf \
        openssl-dev git curl

ARG RUST_VERSION=1.97.1
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
        --target x86_64-unknown-linux-musl

WORKDIR /repo
ENTRYPOINT ["cargo"]
