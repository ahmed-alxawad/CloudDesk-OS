#!/usr/bin/env bash
#
# CloudDesk OS — Uninstall Script
# Usage:
#   sudo bash uninstall.sh
#
# This script completely removes CloudDesk OS from your system.
# All configuration, data, and logs will be deleted.
#

set -euo pipefail

readonly APP_NAME="clouddesk"
readonly INSTALL_DIR="/opt/clouddesk"
readonly BIN_DIR="/usr/local/bin"
readonly ETC_DIR="/etc/clouddesk"
readonly VAR_LIB="/var/lib/clouddesk"
readonly VAR_RUN="/var/run/clouddesk"
readonly VAR_LOG="/var/log/clouddesk"
readonly SYSTEMD_UNIT="/etc/systemd/system/clouddesk.service"
readonly PAM_SERVICE="clouddesk"

readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly NC='\033[0m'

info()    { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

if [[ $EUID -ne 0 ]]; then
    error "This script must be run as root. Use: sudo $0"
    exit 1
fi

echo ""
echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${YELLOW}         CloudDesk OS Uninstaller                ${NC}"
echo -e "${YELLOW}═══════════════════════════════════════════════════════════════${NC}"
echo ""
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

# Detect nginx paths at runtime
if [[ -d /etc/nginx/sites-available ]]; then
    rm -f /etc/nginx/sites-available/clouddesk
    rm -f /etc/nginx/sites-enabled/clouddesk
else
    rm -f /etc/nginx/conf.d/clouddesk.conf
fi
info "Removing nginx configuration..."
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
echo -e "${GREEN}CloudDesk OS has been completely removed.${NC}"
echo ""
info "Remaining Go/Node.js/nginx installations were NOT removed."
echo ""