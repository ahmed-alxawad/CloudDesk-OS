install_packages() {
    pacman -Syu --noconfirm --needed ca-certificates openssh openssl sqlite util-linux
}

create_service_account() {
    useradd --system --home-dir /var/lib/clouddesk --shell /usr/bin/nologin clouddesk
}
