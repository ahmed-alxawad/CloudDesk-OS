FROM fedora:41
RUN dnf install -y systemd sudo policycoreutils curl sqlite iproute procps-ng && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
