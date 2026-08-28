FROM almalinux:9
RUN dnf install -y --allowerasing systemd sudo policycoreutils curl sqlite iproute procps-ng && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
