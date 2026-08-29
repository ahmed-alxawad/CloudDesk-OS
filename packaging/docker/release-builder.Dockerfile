# Portable release builder (Phase 10B).
#
# Base: Rocky Linux 9 -- glibc 2.34, the OLDEST glibc across every
# currently declared v1 glibc-family target (RHEL 9 / Rocky 9 / Alma 9:
# 2.34; Debian 12: 2.36; Ubuntu 24.04: 2.39; Fedora 41: 2.40; Debian 13:
# 2.41), confirmed live against each distro's own real base image/UBI
# before this file was written. A binary linked against symbols no
# newer than glibc 2.34 runs unmodified on all of them (glibc's own
# versioned-symbol scheme is backward-additive: newer glibc always
# satisfies an older requirement, never the reverse).
#
# No repository-pinned Rust toolchain exists yet (no rust-toolchain.toml)
# -- this is reproducibility debt, not something this builder invents an
# opinion about. RUST_VERSION defaults to the version this repo's builds
# have actually been verified against (Phase 10B/10C both used it, two
# from-scratch builds this pass producing byte-identical output), but
# stays overridable via --build-arg so bumping it is a deliberate,
# visible act -- never a silent `latest`.
#
# Base image pinned to an exact digest (Phase 10C), not the floating
# `rockylinux:9` tag: that tag is mutable and will eventually point at
# a newer point release with a newer glibc, silently raising this
# builder's compatibility floor out from under whoever runs it next.
# Re-verify PHASE10_DISTRO_MATRIX.md's glibc table before bumping this.
FROM rockylinux@sha256:d7be1c094cc5845ee815d4632fe377514ee6ebcf8efaed6892889657e5ddaaa6

RUN dnf install -y \
        gcc gcc-c++ make cmake perl pkgconf-pkg-config \
        openssl-devel git \
    && dnf clean all

ARG RUST_VERSION=1.97.1
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}"

WORKDIR /repo
ENTRYPOINT ["cargo"]
