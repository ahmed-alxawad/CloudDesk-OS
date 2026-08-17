detect_distribution() {
    if [ -n "${CLOUDESK_DISTRO_ID:-}" ]; then
        distro_id=$CLOUDESK_DISTRO_ID
        distro_like=${CLOUDESK_DISTRO_LIKE:-}
    else
        os_release=${CLOUDESK_OS_RELEASE:-/etc/os-release}
        [ -r "$os_release" ] || return 1
        distro_id=$(sed -n 's/^ID=//p' "$os_release" | tr -d '"' | head -n 1)
        distro_like=$(sed -n 's/^ID_LIKE=//p' "$os_release" | tr -d '"' | head -n 1)
    fi

    case "$distro_id" in
        debian | ubuntu)
            distro_family=debian
            ;;
        rhel | rocky | almalinux)
            distro_family=rhel
            ;;
        fedora)
            distro_family=fedora
            ;;
        arch | archlinux)
            distro_family=arch
            ;;
        alpine)
            distro_family=alpine
            ;;
        *)
            case " $distro_like " in
                *" debian "*) distro_family=debian ;;
                *" rhel "* | *" fedora "*) distro_family=rhel ;;
                *" arch "*) distro_family=arch ;;
                *) return 1 ;;
            esac
            ;;
    esac

    export distro_id distro_family
}

detect_service_manager() {
    if [ -n "${CLOUDESK_INIT_SYSTEM:-}" ]; then
        init_system=$CLOUDESK_INIT_SYSTEM
    elif command -v systemctl >/dev/null 2>&1; then
        init_system=systemd
    elif command -v rc-update >/dev/null 2>&1; then
        init_system=openrc
    else
        init_system=none
    fi

    case "$init_system" in
        systemd | openrc | none) ;;
        *) return 1 ;;
    esac
    export init_system
}

