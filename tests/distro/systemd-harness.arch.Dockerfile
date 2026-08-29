FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed systemd sudo curl sqlite iproute2 procps-ng \
 && yes | pacman -Scc --noconfirm 2>/dev/null || true
# systemd-firstboot.service tries interactive/first-boot machine setup
# (locale, timezone, hostname prompts) that Docker already resolves
# for the container -- fails and reports the whole system "degraded"
# on every fresh Arch container boot regardless of anything CloudDesk
# does. Masked so `systemctl is-system-running` stays a meaningful
# signal for genuine failures.
RUN systemctl mask systemd-firstboot.service
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
