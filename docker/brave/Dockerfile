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

# pulseaudio/pulseaudio-utils (Pass 3B, Part 5): a real, isolated
# per-instance audio backend -- this container has no ALSA hardware,
# so without a running Pulse server Chromium silently falls back to a
# muted/fake output device. ffmpeg captures the sink's own monitor
# source and re-encodes to bounded raw PCM chunks for `clouddeskd` to
# read (see this file's `ENTRYPOINT` and `browser_audio.rs`).
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl gnupg ca-certificates socat pulseaudio pulseaudio-utils ffmpeg \
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
# the grace period elapsed. Fixed by backgrounding `socat` instead and
# `exec`ing Brave as PID 1.
#
# A third real defect, found and fixed in Phase 9 Pass 3A-3 (the actual
# root cause of real cookie persistence appearing broken across three
# prior passes, live-diagnosed with a direct CDP harness, not guessed):
# `brave-browser-stable` -- the executable this entrypoint used to
# `exec` -- is itself a vendor-shipped wrapper *shell script*, and its
# own last line is `"$HERE/brave" "$@" || true`, which runs the real
# Chromium binary as an ordinary foreground child *without* `exec`.
# `exec`ing into that script therefore still leaves PID 1 as the bash
# wrapper, not the real browser process -- `docker stop`'s SIGTERM
# terminates the wrapper (which has no trap and forwards nothing to its
# child), and once PID 1 exits Docker tears down the whole container,
# SIGKILLing the orphaned real Brave process before it ever gets a
# chance to run its own shutdown sequence. Fixed by `exec`ing the real
# underlying ELF binary directly (`/opt/brave.com/brave/brave` --
# confirmed via `readlink -f` on the wrapper, which itself resolves to
# yet *another* script, `.../brave-browser`, also `#!/bin/bash`, also
# non-`exec`ing -- the actual compiled binary's path had to be found
# separately by inspecting the installed tree directly), never the
# wrapper script. `--password-store=basic` is kept: live-verified this
# pass (a real cookie set via `document.cookie`, real container stop,
# fresh container reusing the same profile volume, real read-back) that
# with a real Chromium binary as PID 1 *and* a real CDP `Browser.close`
# call before `docker stop` (see `browser_runtime.rs`'s
# `graceful_stop` hook, which needs this flag to avoid ever depending
# on a system keyring/D-Bus service this minimal container doesn't
# have), cookies genuinely persist and decrypt correctly across a real
# restart -- no shared/global key, no keyring, nothing world-readable,
# per-profile exactly like every other piece of this adapter's
# `/state` mount already is.
# Pass 3A-4: every one of Brave's own HTTP(S) requests -- navigation,
# fetch/XHR, images, WebSocket upgrades -- is forced through
# `clouddeskd`'s own policy-enforcing egress proxy
# (`services/clouddeskd/src/browser_egress_proxy.rs`), reached at the
# dedicated Browser network's own pinned gateway address
# (`browser_runtime.rs`'s `BROWSER_NET_GATEWAY`/`BROWSER_NET_SUBNET`).
# `--proxy-server` is a command-line flag, not a page- or UI-
# overridable setting, so hostile page content cannot redirect this
# traffic around the proxy. `--proxy-bypass-list="<-loopback>"`
# removes Chromium's own implicit "never proxy loopback" default --
# without it, a page requesting `http://127.0.0.1:9223/` (this same
# container's own CDP relay) would reach it directly, bypassing the
# proxy's policy checks entirely.
#
# `--disable-component-update`/`--disable-background-networking`/
# `--disable-domain-reliability`/`--disable-breakpad`/`--disable-sync`
# (Pass 3A-4): real, live-observed load found Brave's own background
# telemetry/updater/component-updater/dictionary-fetch machinery fires
# a real burst of ~10+ outbound `CONNECT` attempts (`go-updater.brave.com`,
# `componentupdater.brave.com`, `dict.brave.com`, `redirector.brave.com`,
# etc.) per instance on startup, all competing for the one shared
# egress proxy -- under simultaneous multi-user startup this
# measurably delayed real frame delivery. This traffic serves no
# product purpose in a locked-down server deployment anyway (nothing
# here should be silently phoning home), so disabling it is a genuine
# improvement on its own merits, not merely a load workaround.
# Pass 3B, Part 5 (audio): this container's process runs under an
# arbitrary real host UID with no `/etc/passwd` entry (see
# `browser_runtime.rs`'s `run_as`), so a per-user PulseAudio session is
# started explicitly rather than relying on any system service or
# passwd-derived home lookup -- `XDG_RUNTIME_DIR`/`HOME` are both
# pointed at this instance's own writable `/state` mount, matching
# every other piece of per-instance state here. `browser_out` is a
# fixed-name null sink (no real hardware exists in this container) set
# as Brave's default output; a background loop continuously
# (re-)captures that sink's own monitor into a FIFO as bounded raw
# 16-bit mono 48 kHz PCM for `clouddeskd`'s own `browser_audio.rs` to
# read directly off the `/state` bind mount, the same pattern already
# used for downloads/uploads -- opening a FIFO for read blocks until a
# writer exists and vice versa, so capture naturally idles until a
# real client audio channel is open, and the wrapping `while true`
# loop relaunches `ffmpeg` after every reader disconnect (its write
# side gets a broken pipe and exits) instead of leaving it dead.
ENTRYPOINT ["/bin/sh", "-c", "\
  rm -f /state/profile/SingletonLock /state/profile/SingletonSocket /state/profile/SingletonCookie; \
  export XDG_RUNTIME_DIR=/state/xdg; \
  rm -rf \"$XDG_RUNTIME_DIR\"; mkdir -p -m 700 \"$XDG_RUNTIME_DIR\"; \
  pulseaudio --start --exit-idle-time=-1 --log-target=stderr >/state/pulseaudio.log 2>&1; \
  for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done; \
  pactl load-module module-null-sink sink_name=browser_out sink_properties=device.description=BrowserOut >/dev/null 2>&1; \
  pactl set-default-sink browser_out >/dev/null 2>&1; \
  rm -f /state/audio.pcm.fifo; mkfifo -m 600 /state/audio.pcm.fifo; \
  (while true; do ffmpeg -y -nostdin -loglevel error -f pulse -i browser_out.monitor -ac 1 -ar 48000 -f s16le /state/audio.pcm.fifo; sleep 1; done) & \
  socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 & \
  exec /opt/brave.com/brave/brave --headless=new --disable-gpu --no-first-run \
    --remote-debugging-port=9222 --user-data-dir=/state/profile \
    --password-store=basic \
    --proxy-server=http://172.30.99.1:9819 \
    --proxy-bypass-list=\"<-loopback>\" \
    --disable-component-update --disable-background-networking \
    --disable-domain-reliability --disable-breakpad --disable-sync \
    --disable-features=BraveRewards,BraveVPN,BraveTogether \
    --autoplay-policy=no-user-gesture-required \
    about:blank \
"]
