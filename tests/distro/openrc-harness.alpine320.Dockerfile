FROM alpine:3.20
# openrc: the real init system under test.
# curl/sqlite/iproute2/procps: harness-only diagnostic tools, never
# installed by CloudDesk's own installer -- matches the systemd
# harnesses' convention exactly.
RUN apk add --no-cache openrc curl sqlite iproute2 procps
# Alpine ships no installable openrc-init: there is no standard
# "OpenRC-in-Docker" full-PID1-boot pattern the way there is for
# systemd (jrei/systemd-*, etc.). This container is kept alive by the
# CMD below (not a real init/PID1); `openrc sysinit` is run explicitly
# first -- OpenRC's own real, documented first init stage, which
# creates the full /run/openrc/{starting,started,exclusive,...} state
# tree its own `rc-service`/`rc-update` depend on. A naive
# `mkdir -p /run/openrc; touch softlevel` shortcut was tried first and
# confirmed live to be insufficient: rc-service reported every start
# attempt as "already starting" and never actually launched anything,
# because that missing state tree left OpenRC's own exclusive-locking
# logic unable to track real state. `openrc sysinit` makes
# rc-service/rc-update genuinely operate (real init scripts, real
# start-stop-daemon process supervision) rather than fake anything.
# The one claim this cannot prove is a genuine cold-boot bringing
# services up automatically; that is classified BLOCKED BY
# ENVIRONMENT, matching this project's existing "Reboot: BLOCKED BY
# ENVIRONMENT" pattern on every systemd harness so far.
CMD ["sh", "-c", "openrc sysinit && sleep infinity"]
