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
#
# Real defect found and fixed this pass (Task 5/7 profile-persistence
# testing): stopping a container (whether graceful or forced) doesn't
# clean up Chromium's own `SingletonLock`/`SingletonSocket`/
# `SingletonCookie` files in the profile directory. Since every restart
# is a genuinely new container with a new hostname, a real, live
# Persistent-profile restart consistently hit Chromium's own
# process-singleton check ("The profile appears to be in use by
# another Brave process ... on another computer"), which blocks
# startup waiting for a user dialog that headless mode never shows --
# the adapter's health check then times out (502) even though nothing
# is actually still using the profile. Removing those three files is
# always safe here: a freshly started container's own Brave process is
# by definition not yet running, so any lock file already on disk
# cannot belong to a live process in *this* container.
#
# A second, related real defect found and fixed this pass: the OCI
# adapter's `stop()` sends `docker stop` (SIGTERM to PID 1 only, no
# forwarding to children). The original entrypoint backgrounded Brave
# with `&` and `exec`'d into `socat` as PID 1 -- so SIGTERM only ever
# reached `socat`, never Brave, which was then abruptly SIGKILLed once
# the grace period elapsed. Chromium needs a clean shutdown to flush
# its SQLite-backed cookie store to disk; an unflushed cookie set
# moments before a forced kill was silently lost on the very next
# restart (live-reproduced: `Network.setCookie` succeeded, but the
# cookie was gone after a real stop/restart cycle). Fixed by
# backgrounding `socat` instead and `exec`ing into Brave as PID 1, so
# `docker stop`'s SIGTERM reaches the real browser process directly and
# it gets its normal graceful-shutdown path.
ENTRYPOINT ["/bin/sh", "-c", "\
  rm -f /state/profile/SingletonLock /state/profile/SingletonSocket /state/profile/SingletonCookie; \
  socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 & \
  exec brave-browser-stable --headless=new --disable-gpu --no-first-run \
    --remote-debugging-port=9222 --user-data-dir=/state/profile \
    about:blank \
"]
