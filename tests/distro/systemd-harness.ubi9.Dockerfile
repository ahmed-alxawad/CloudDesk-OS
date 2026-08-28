FROM registry.access.redhat.com/ubi9/ubi:latest
RUN dnf install -y --allowerasing systemd sudo policycoreutils curl sqlite iproute procps-ng && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
