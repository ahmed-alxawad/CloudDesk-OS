install_packages() {
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends ca-certificates openssh-client openssl sqlite3 util-linux
}

create_service_account() {
    useradd --system --home-dir /var/lib/clouddesk --shell /usr/sbin/nologin clouddesk
}
