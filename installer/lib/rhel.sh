install_packages() {
    dnf install -y ca-certificates openssh-clients openssl sqlite util-linux
}

create_service_account() {
    useradd --system --home-dir /var/lib/clouddesk --shell /sbin/nologin clouddesk
}
