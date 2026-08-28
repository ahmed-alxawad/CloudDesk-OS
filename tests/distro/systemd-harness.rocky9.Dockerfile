FROM rockylinux:9
RUN dnf install -y systemd sudo policycoreutils && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
