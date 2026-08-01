#!/usr/bin/env bash
#
# CloudDesk OS — Install / Uninstall Script
# Usage:
#   curl -sSL https://raw.githubusercontent.com/ahmed-alxawad/Cloud-Desk/main/install.sh | bash -s install
#   curl -sSL https://raw.githubusercontent.com/ahmed-alxawad/Cloud-Desk/main/install.sh | bash -s uninstall
#
# Or download and run:
#   wget https://raw.githubusercontent.com/ahmed-alxawad/Cloud-Desk/main/install.sh
#   bash install.sh install
#   bash install.sh uninstall
#

set -euo pipefail

# ──────────────────────────────────────────────────────────
# Constants
# ──────────────────────────────────────────────────────────
readonly APP_NAME="clouddesk"
readonly APP_VERSION="0.2.0"
readonly INSTALL_DIR="/opt/clouddesk"
readonly BIN_DIR="/usr/local/bin"
readonly ETC_DIR="/etc/clouddesk"
readonly VAR_LIB="/var/lib/clouddesk"
readonly VAR_RUN="/var/run/clouddesk"
readonly VAR_LOG="/var/log/clouddesk"
readonly FRONTEND_DIST="/opt/clouddesk/frontend/dist"
readonly PAM_SERVICE="clouddesk"
readonly SYSTEMD_UNIT="/etc/systemd/system/clouddesk.service"
# NGINX_CONF / NGINX_ENABLED are detected at runtime — see install_nginx() and uninstall()
readonly GITHUB_REPO="https://github.com/ahmed-alxawad/Cloud-Desk.git"

# Colors
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m'

# ──────────────────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────────────────
info()    { echo -e "${BLUE}[INFO]${NC}  $*"; }
success() { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }
die()     { error "$*"; exit 1; }

check_root() {
    if [[ $EUID -ne 0 ]]; then
        die "This script must be run as root. Use: sudo $0 $1"
    fi
}

detect_os() {
    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        OS_ID="${ID}"
        OS_VERSION="${VERSION_ID}"
        case "${ID}" in
            ubuntu|debian|linuxmint|pop) OS_FAMILY="debian" ;;
            centos|rhel|fedora|rocky|almalinux|ol) OS_FAMILY="rhel" ;;
            arch|manjaro) OS_FAMILY="arch" ;;
            *) OS_FAMILY="unknown" ;;
        esac
    elif [[ -f /etc/redhat-release ]]; then
        OS_FAMILY="rhel"
    else
        OS_FAMILY="unknown"
    fi
    info "Detected OS: ${OS_ID:-unknown} (${OS_FAMILY}) ${OS_VERSION:-unknown}"
}

# Architecture detection helper (used by both Debian and RHEL Go download blocks)
detect_go_arch() {
    local arch
    arch="$(uname -m)"
    case "${arch}" in
        x86_64|amd64)  echo "amd64" ;;
        aarch64|arm64) echo "arm64" ;;
        *) die "Unsupported architecture: ${arch}" ;;
    esac
}

install_dependencies() {
    info "Installing build dependencies..."

    case "${OS_FAMILY}" in
        debian)
            export DEBIAN_FRONTEND=noninteractive
            apt-get update -qq

            local pkgs=(
                build-essential
                libpam0g-dev
                wget
                curl
                gnupg
                nginx
                systemd
            )

            # Check and install Go if not present
            if ! command -v go &>/dev/null; then
                info "Installing Go..."
                local GO_VERSION="1.22.5"
                local GO_ARCH
                GO_ARCH="$(detect_go_arch)"
                local tmp_dir
                tmp_dir="$(mktemp -d)"
                wget -q "https://go.dev/dl/go${GO_VERSION}.linux-${GO_ARCH}.tar.gz" -O "${tmp_dir}/go.tar.gz"
                tar -C /usr/local -xzf "${tmp_dir}/go.tar.gz"
                rm -f "${tmp_dir}/go.tar.gz"
                rmdir "${tmp_dir}"
                echo 'export PATH=$PATH:/usr/local/go/bin' > /etc/profile.d/go.sh
                export PATH=$PATH:/usr/local/go/bin
                success "Go ${GO_VERSION} installed (${GO_ARCH})"
            else
                success "Go already installed: $(go version)"
            fi

            # Check and install Node.js if not present (simplified)
            if ! command -v node &>/dev/null || [[ $(node --version | cut -d'v' -f2 | cut -d'.' -f1) -lt 18 ]]; then
                info "Installing Node.js 20.x..."
                curl -fsSL https://deb.nodesource.com/setup_20.x | bash -
                apt-get install -y nodejs
                success "Node.js $(node --version) installed"
            else
                success "Node.js already installed: $(node --version)"
            fi

            apt-get install -y -qq "${pkgs[@]}"
            ;;

        rhel)
            local pkgs=(
                gcc
                make
                pam-devel
                wget
                curl
                nginx
            )

            # Install Go if not present
            if ! command -v go &>/dev/null; then
                info "Installing Go..."
                local GO_VERSION="1.22.5"
                local GO_ARCH
                GO_ARCH="$(detect_go_arch)"
                local tmp_dir
                tmp_dir="$(mktemp -d)"
                wget -q "https://go.dev/dl/go${GO_VERSION}.linux-${GO_ARCH}.tar.gz" -O "${tmp_dir}/go.tar.gz"
                tar -C /usr/local -xzf "${tmp_dir}/go.tar.gz"
                rm -f "${tmp_dir}/go.tar.gz"
                rmdir "${tmp_dir}"
                echo 'export PATH=$PATH:/usr/local/go/bin' > /etc/profile.d/go.sh
                export PATH=$PATH:/usr/local/go/bin
                success "Go ${GO_VERSION} installed (${GO_ARCH})"
            fi

            # Install Node.js if not present
            if ! command -v node &>/dev/null || [[ $(node --version | cut -d'v' -f2 | cut -d'.' -f1) -lt 18 ]]; then
                info "Installing Node.js 20.x..."
                curl -fsSL https://rpm.nodesource.com/setup_20.x | bash -
                yum install -y nodejs
                success "Node.js $(node --version) installed"
            fi

            yum install -y "${pkgs[@]}" 2>/dev/null || dnf install -y "${pkgs[@]}"
            ;;

        arch)
            pacman -Sy --noconfirm
            pacman -S --noconfirm --needed go nodejs npm nginx pam
            ;;

        *)
            warn "Unsupported OS (${OS_FAMILY}). Please install manually:"
            warn "  - Go 1.22+"
            warn "  - Node.js 18+"
            warn "  - libpam-dev (or pam-devel)"
            warn "  - nginx"
            ;;
    esac

    success "All dependencies installed"
}

build_backend() {
    info "Building Go backend (this may take a minute)..."

    local src_dir="${INSTALL_DIR}/backend"
    local bin="${BIN_DIR}/clouddesk-server"

    # Enable CGO for PAM
    export CGO_ENABLED=1

    cd "${src_dir}"

    # Download dependencies and tidy
    go mod tidy
    go mod download

    # Build with version injected via ldflags
    go build \
        -ldflags="-s -w -X github.com/clouddesk-os/backend/internal/config.version=${APP_VERSION}" \
        -o "${bin}" \
        ./cmd/server/

    chmod 755 "${bin}"
    local bin_size
    bin_size="$(stat -c%s "${bin}" 2>/dev/null || stat -f%z "${bin}" 2>/dev/null || echo "unknown")"
    success "Backend built: ${bin} (${bin_size} bytes)"
}

build_frontend() {
    info "Building React frontend..."

    local src_dir="${INSTALL_DIR}/frontend"

    cd "${src_dir}"

    # Install dependencies
    npm ci --production=false 2>/dev/null || npm install

    # Build
    npm run build

    # Verify build output
    if [[ -f "${src_dir}/dist/index.html" ]]; then
        success "Frontend built: ${src_dir}/dist/"
    else
        die "Frontend build failed — dist/index.html not found"
    fi
}

create_directories() {
    info "Creating system directories..."

    mkdir -p "${VAR_LIB}/code-server"
    mkdir -p "${VAR_RUN}"
    mkdir -p "${VAR_LOG}"
    mkdir -p "${ETC_DIR}"

    success "Directories created"
}

# Note: The clouddesk user is retained for potential future use.
# The service itself runs as root because PAM authentication requires root privileges.
create_user() {
    info "Creating system user '${APP_NAME}'..."

    if id "${APP_NAME}" &>/dev/null; then
        success "User '${APP_NAME}' already exists"
        return 0
    fi

    local nologin_shell
    nologin_shell="$(command -v nologin 2>/dev/null || echo /bin/false)"

    if useradd \
        --system \
        --home "${VAR_LIB}" \
        --shell "${nologin_shell}" \
        --comment "CloudDesk OS Service" \
        "${APP_NAME}" 2>/dev/null; then
        success "User '${APP_NAME}' created"
    elif adduser \
        --system \
        --home "${VAR_LIB}" \
        --shell "${nologin_shell}" \
        --comment "CloudDesk OS Service" \
        "${APP_NAME}" 2>/dev/null; then
        success "User '${APP_NAME}' created"
    else
        warn "Could not create user '${APP_NAME}' (may already exist or insufficient privileges)"
    fi
}

install_pam_config() {
    info "Installing PAM configuration..."

    cat > "/etc/pam.d/${PAM_SERVICE}" << 'EOF'
# CloudDesk OS PAM Configuration
# This file authenticates users against the system's shadow database.
auth    required    pam_unix.so
account required    pam_unix.so
EOF

    chmod 644 "/etc/pam.d/${PAM_SERVICE}"
    success "PAM config installed: /etc/pam.d/${PAM_SERVICE}"
}

install_systemd() {
    info "Installing systemd service..."

    cat > "${SYSTEMD_UNIT}" << SYSTEMD_EOF
[Unit]
Description=CloudDesk OS — Browser-based Linux Workspace
Documentation=https://github.com/ahmed-alxawad/Cloud-Desk
After=network.target nginx.service
Wants=nginx.service
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
User=root
Group=root
WorkingDirectory=${INSTALL_DIR}/backend
ExecStart=${BIN_DIR}/clouddesk-server \
    --port 8080 \
    --jwt-secret-file ${ETC_DIR}/jwt.secret \
    --master-key-file ${ETC_DIR}/master.key \
    --code-server-data ${VAR_LIB}/code-server \
    --code-server-sock ${VAR_RUN} \
    --home-base /home \
    --audit

Restart=on-failure
RestartSec=5
TimeoutStartSec=30
TimeoutStopSec=30

# Security hardening
NoNewPrivileges=false
ProtectSystem=full
ProtectHome=false
ReadWritePaths=${VAR_LIB} ${VAR_RUN} ${VAR_LOG}

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=clouddesk

# Resource limits
LimitNOFILE=65535
LimitNPROC=4096

[Install]
WantedBy=multi-user.target
SYSTEMD_EOF

    chmod 644 "${SYSTEMD_UNIT}"
    systemctl daemon-reload
    success "Systemd service installed: ${SYSTEMD_UNIT}"
}

install_nginx() {
    info "Installing nginx configuration..."

    # Detect nginx config paths at runtime (not as readonly constants)
    local nginx_conf_path nginx_enabled_path
    if [[ -d /etc/nginx/sites-available ]]; then
        nginx_conf_path="/etc/nginx/sites-available/clouddesk"
        nginx_enabled_path="/etc/nginx/sites-enabled/clouddesk"
    else
        nginx_conf_path="/etc/nginx/conf.d/clouddesk.conf"
        nginx_enabled_path=""
    fi

    # Use default_server for conf.d style (RHEL), plain listen for sites-available style (Debian)
    local listen_directive
    if [[ -z "${nginx_enabled_path}" ]]; then
        listen_directive="listen 80 default_server;"
    else
        listen_directive="listen 80;"
    fi

    # Unified nginx config with identical security headers for all distros
    cat > "${nginx_conf_path}" << NGINX_EOF
server {
    ${listen_directive}
    server_name _;

    # Security headers
    add_header X-Frame-Options "DENY" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-XSS-Protection "1; mode=block" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    add_header Content-Security-Policy "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws: wss:; font-src 'self' data:; worker-src 'self' blob:" always;
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;

    # Client max body size (10 GB for file uploads)
    client_max_body_size 10G;

    # Proxy timeouts (for large file operations)
    proxy_read_timeout 300s;
    proxy_send_timeout 300s;
    proxy_connect_timeout 60s;

    # Frontend static files
    location / {
        root ${FRONTEND_DIST};
        index index.html;
        try_files \\$uri \\$uri/ /index.html;
    }

    # API proxy
    location /api/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host \\$host;
        proxy_set_header X-Real-IP \\$remote_addr;
        proxy_set_header X-Forwarded-For \\$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \\$scheme;
    }

    # WebSocket proxy for IDE and Terminal
    location /api/v1/ide/proxy/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \\$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \\$host;
        proxy_set_header X-Real-IP \\$remote_addr;
        proxy_set_header X-Forwarded-For \\$proxy_add_x_forwarded_for;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }

    # WebSocket proxy for Terminal PTY
    location /api/v1/terminal/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \\$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \\$host;
        proxy_set_header X-Real-IP \\$remote_addr;
        proxy_set_header X-Forwarded-For \\$proxy_add_x_forwarded_for;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }

    # Health check
    location /api/health {
        proxy_pass http://127.0.0.1:8080;
        access_log off;
    }

    # Block sensitive paths
    location ~ /\\.(git|env|htaccess|htpasswd) {
        return 404;
    }
}
NGINX_EOF

    # Enable the site (Debian-style sites-enabled)
    if [[ -n "${nginx_enabled_path}" && ! -L "${nginx_enabled_path}" ]]; then
        ln -sf "${nginx_conf_path}" "${nginx_enabled_path}"
    fi

    # Test and reload nginx
    nginx -t 2>/dev/null && {
        systemctl reload nginx 2>/dev/null || systemctl restart nginx 2>/dev/null || true
    }
    success "Nginx configuration installed"
}

generate_secrets() {
    info "Generating security secrets..."

    if [[ ! -f "${ETC_DIR}/jwt.secret" ]]; then
        openssl rand -base64 48 > "${ETC_DIR}/jwt.secret"
        chmod 600 "${ETC_DIR}/jwt.secret"
    fi

    if [[ ! -f "${ETC_DIR}/master.key" ]]; then
        openssl rand -base64 32 > "${ETC_DIR}/master.key"
        chmod 600 "${ETC_DIR}/master.key"
    fi

    success "Secrets generated in ${ETC_DIR}/"
}

set_permissions() {
    info "Setting permissions..."

    chown -R root:root "${INSTALL_DIR}"
    chmod -R 755 "${INSTALL_DIR}"

    # Service runs as root for PAM; keep data dirs world-accessible but owned by root
    chmod 775 "${VAR_LIB}"
    chmod 775 "${VAR_RUN}"

    # The binary needs to be owned by root (runs as root for PAM)
    chown root:root "${BIN_DIR}/clouddesk-server"
    chmod 755 "${BIN_DIR}/clouddesk-server"

    # Secrets must be root-only
    chown root:root "${ETC_DIR}"
    chmod 700 "${ETC_DIR}"
    chmod 600 "${ETC_DIR}/jwt.secret" "${ETC_DIR}/master.key"

    # Logs
    touch "${VAR_LOG}/clouddesk.log"
    chmod 644 "${VAR_LOG}/clouddesk.log"

    success "Permissions set"
}

start_service() {
    info "Enabling and starting CloudDesk OS service..."

    systemctl enable clouddesk
    systemctl start clouddesk

    # Wait for service to start
    local retries=10
    while [[ $retries -gt 0 ]]; do
        if systemctl is-active --quiet clouddesk; then
            success "CloudDesk OS is running!"
            return 0
        fi
        sleep 1
        ((retries--))
    done

    warn "Service may not have started. Check: systemctl status clouddesk"
    warn "View logs: journalctl -u clouddesk -f"
}

install() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}         CloudDesk OS Installer v${APP_VERSION}              ${NC}"
    echo -e "${BLUE}   Browser-based Linux Workspace for Your Server    ${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo ""

    check_root
    detect_os
    install_dependencies

    # Resolve script directory; detect piped execution
    local script_dir
    if [[ -f "${BASH_SOURCE[0]}" ]]; then
        script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    else
        script_dir=""
    fi

    if [[ -n "${script_dir}" && -d "${script_dir}/backend" && -d "${script_dir}/frontend" ]]; then
        info "Copying project files to ${INSTALL_DIR}..."
        mkdir -p "${INSTALL_DIR}"
        cp -r "${script_dir}/backend" "${INSTALL_DIR}/"
        cp -r "${script_dir}/frontend" "${INSTALL_DIR}/"
        # Exclude node_modules and build artifacts
        rm -rf "${INSTALL_DIR}/frontend/node_modules" 2>/dev/null || true
        rm -rf "${INSTALL_DIR}/frontend/dist" 2>/dev/null || true
        success "Project files copied"
    elif [[ -d "${INSTALL_DIR}/backend" && -d "${INSTALL_DIR}/frontend" ]]; then
        info "Using existing project files in ${INSTALL_DIR}"
    else
        # Piped execution or missing source — clone from GitHub
        info "Source files not found locally. Cloning from GitHub..."
        if ! command -v git &>/dev/null; then
            apt-get install -y git 2>/dev/null || yum install -y git 2>/dev/null || dnf install -y git 2>/dev/null || \
                pacman -S --noconfirm git 2>/dev/null || die "git is required but not installed"
        fi
        local clone_tmp
        clone_tmp="$(mktemp -d)"
        git clone --depth 1 "${GITHUB_REPO}" "${clone_tmp}/Cloud-Desk"
        mkdir -p "${INSTALL_DIR}"
        cp -r "${clone_tmp}/Cloud-Desk/backend" "${INSTALL_DIR}/"
        cp -r "${clone_tmp}/Cloud-Desk/frontend" "${INSTALL_DIR}/"
        rm -rf "${clone_tmp}"
        success "Project files cloned from GitHub"
    fi

    build_backend
    build_frontend
    create_user
    create_directories
    install_pam_config
    install_systemd
    install_nginx
    generate_secrets
    set_permissions
    start_service

    # ──── Success Banner ────
    local server_ip
    server_ip=$(hostname -I | awk '{print $1}') || server_ip="YOUR_SERVER_IP"

    echo ""
    echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}       CloudDesk OS installed successfully!       ${NC}"
    echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}                                                  ${NC}"
    echo -e "${GREEN}  Access URL:  http://${server_ip}               ${NC}"
    echo -e "${GREEN}                                                  ${NC}"
    echo -e "${GREEN}  Service:     systemctl status clouddesk         ${NC}"
    echo -e "${GREEN}  Logs:        journalctl -u clouddesk -f         ${NC}"
    echo -e "${GREEN}  Config:      ${ETC_DIR}/                     ${NC}"
    echo -e "${GREEN}  Binary:      ${BIN_DIR}/clouddesk-server      ${NC}"
    echo -e "${GREEN}                                                  ${NC}"
    echo -e "${GREEN}  To uninstall: bash install.sh uninstall        ${NC}"
    echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
    echo ""
}

uninstall() {
    echo ""
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${YELLOW}         CloudDesk OS Uninstaller                ${NC}"
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}"
    echo ""

    check_root

    echo -e "${RED}WARNING: This will completely remove CloudDesk OS from your system.${NC}"
    echo -e "${RED}         All configuration, data, and logs will be deleted.${NC}"
    echo ""
    read -rp "Are you sure you want to proceed? [y/N] " confirm </dev/tty

    if [[ "${confirm}" != "y" && "${confirm}" != "Y" ]]; then
        info "Uninstall cancelled."
        exit 0
    fi

    info "Stopping and disabling service..."
    systemctl stop clouddesk 2>/dev/null || true
    systemctl disable clouddesk 2>/dev/null || true

    info "Removing systemd service..."
    rm -f "${SYSTEMD_UNIT}"
    systemctl daemon-reload

    # Detect nginx paths at runtime (same logic as install_nginx)
    local nginx_conf_path nginx_enabled_path
    if [[ -d /etc/nginx/sites-available ]]; then
        nginx_conf_path="/etc/nginx/sites-available/clouddesk"
        nginx_enabled_path="/etc/nginx/sites-enabled/clouddesk"
    else
        nginx_conf_path="/etc/nginx/conf.d/clouddesk.conf"
        nginx_enabled_path=""
    fi

    info "Removing nginx configuration..."
    rm -f "${nginx_conf_path}"
    [[ -n "${nginx_enabled_path}" ]] && rm -f "${nginx_enabled_path}"
    nginx -t 2>/dev/null && systemctl reload nginx 2>/dev/null || true

    info "Removing PAM configuration..."
    rm -f "/etc/pam.d/${PAM_SERVICE}"

    info "Removing binary..."
    rm -f "${BIN_DIR}/clouddesk-server"

    info "Removing secrets..."
    rm -rf "${ETC_DIR}"

    info "Removing data and logs..."
    rm -rf "${VAR_LIB}"
    rm -rf "${VAR_LOG}"

    info "Removing runtime directory..."
    rm -rf "${VAR_RUN}"

    info "Removing installation directory..."
    rm -rf "${INSTALL_DIR}"

    info "Removing system user..."
    userdel "${APP_NAME}" 2>/dev/null || true

    info "Removing Go PATH profile script..."
    rm -f /etc/profile.d/go.sh

    echo ""
    success "CloudDesk OS has been completely removed."
    echo ""
    info "Remaining Go/Node.js/nginx installations were NOT removed."
    echo ""
}

# ──────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────
case "${1:-}" in
    install)
        install
        ;;
    uninstall|remove)
        uninstall
        ;;
    *)
        echo ""
        echo "CloudDesk OS — Install / Uninstall Script v${APP_VERSION}"
        echo ""
        echo "Usage:"
        echo "  sudo $0 install     Install CloudDesk OS"
        echo "  sudo $0 uninstall   Remove CloudDesk OS completely"
        echo ""
        echo "Quick install (one-liner):"
        echo "  curl -sSL https://raw.githubusercontent.com/ahmed-alxawad/Cloud-Desk/main/install.sh | sudo bash -s install"
        echo ""
        exit 1
        ;;
esac
