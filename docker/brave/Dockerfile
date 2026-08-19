# CloudDesk Browser runtime image (Phase 9).
#
# Installs the real, official Brave Browser for Linux from Brave's own
# apt repository, pinned to an exact version -- never `latest`. No
# unofficial/community browser image is used (Task 1).
#
# Build: docker build -t clouddesk-brave:1.93.136 docker/brave
#
# Brave has no official Docker image of its own (unlike Collabora's
# `collabora/code`), so this Dockerfile *is* the pinned artifact --
# reproducible from Brave's own signed apt repository, not a
# third-party image of unknown provenance.
FROM debian:bookworm-slim

ARG BRAVE_VERSION=1.93.136

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl gnupg ca-certificates socat \
    && rm -rf /var/lib/apt/lists/*

# Brave's own official signed apt repository (brave-browser-apt-release.s3.brave.com).
RUN curl -fsSLo /usr/share/keyrings/brave-browser-archive-keyring.gpg \
    https://brave-browser-apt-release.s3.brave.com/brave-browser-archive-keyring.gpg && \
    echo "deb [signed-by=/usr/share/keyrings/brave-browser-archive-keyring.gpg arch=amd64] https://brave-browser-apt-release.s3.brave.com/ stable main" \
    > /etc/apt/sources.list.d/brave-browser-release.list

RUN apt-get update && \
    apt-get install -y --no-install-recommends brave-browser=${BRAVE_VERSION} && \
    apt-mark hold brave-browser && \
    rm -rf /var/lib/apt/lists/*

# Never root (Task 49).
RUN useradd -m -u 10001 brave
USER brave
WORKDIR /home/brave

# CDP binds to loopback only inside the container regardless of any
# --remote-debugging-address flag (a real, structural Chromium/Brave
# security hardening, live-verified this pass) -- `socat` relays it to
# a container-wide port so CloudDesk's own trusted broker (reaching it
# only via the private Docker bridge network, never published to the
# host) can connect, while the raw CDP port itself is never directly
# reachable by anything outside this container (Task 7).
ENTRYPOINT ["/bin/sh", "-c", "\
  brave-browser-stable --headless=new --disable-gpu --no-first-run \
    --remote-debugging-port=9222 --user-data-dir=/state/profile \
    about:blank & \
  sleep 2 && \
  exec socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 \
"]
