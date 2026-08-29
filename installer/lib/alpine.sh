install_packages() {
    apk add --no-cache ca-certificates openssh-client-default openssl sqlite util-linux
}

create_service_account() {
    # Phase 10D found this live: unlike `useradd --system` on every
    # other distro family (Debian/RPM/Arch all auto-create a matching
    # same-named primary group), busybox `adduser -S` on Alpine does
    # NOT -- it silently falls back to the shared `nogroup` (gid
    # 65533) unless a group is explicitly given, and that group must
    # already exist. Without this, every `chown clouddesk:clouddesk`
    # later in the installer failed with "unknown user/group",
    # aborting the install outright on every fresh Alpine system.
    addgroup -S clouddesk
    adduser -S -D -H -h /var/lib/clouddesk -s /sbin/nologin -G clouddesk clouddesk
}
