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
# opinion about. RUST_VERSION below is passed at build time to match
# whatever toolchain last produced a verified-working build; it is not
# hardcoded so the debt stays visible instead of silently baked in.
FROM rockylinux:9

RUN dnf install -y \
        gcc gcc-c++ make cmake perl pkgconf-pkg-config \
        openssl-devel git \
    && dnf clean all

ARG RUST_VERSION
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}"

WORKDIR /repo
ENTRYPOINT ["cargo"]
