install_packages() {
    apk add --no-cache ca-certificates openssh-client-default openssl sqlite util-linux
}

create_service_account() {
    adduser -S -D -H -h /var/lib/clouddesk -s /sbin/nologin clouddesk
}
