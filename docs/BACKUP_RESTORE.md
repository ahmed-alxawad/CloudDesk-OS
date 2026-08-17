# CloudDesk-OS Backup & Disaster Recovery Guide

This guide details the procedure for safely creating backups and performing disaster recovery restores of a CloudDesk-OS installation.

---

## 1. Critical Encryption Material Warning

> [!WARNING]
> CloudDesk-OS uses **per-record envelope encryption** for all stored credentials and Vault secrets.
>
> The master key located at `/etc/clouddesk/keys/master.key` (or your configured `security.master_key` path) is **required** to decrypt every Vault secret and Data Encryption Key (DEK).
>
> **A backup of `/var/lib/clouddesk/clouddesk.db` WITHOUT `/etc/clouddesk/keys/master.key` CANNOT decrypt any Vault secrets.**

---

## 2. Backup Scope

A complete production backup of CloudDesk-OS consists of:

| Component | Path | Description |
|---|---|---|
| **SQLite Database** | `/var/lib/clouddesk/clouddesk.db` | Users, permissions, audit log, transfers, remote servers, encrypted secrets |
| **Master Encryption Key** | `/etc/clouddesk/keys/master.key` | Primary KEK required to unlock Vault secrets |
| **Privilege Grant Key** | `/etc/clouddesk/keys/privd-grant.key` | Shared HMAC secret for `cloudesk-privd` communication |
| **Configuration** | `/etc/clouddesk/clouddesk.toml` | Host settings, network configuration, and path directives |
| **TLS Certificate & Key** | `/etc/clouddesk/tls/` | Server certificate and private key |
| **User Application State** | `/var/lib/clouddesk/` | File caches, background transfer staging, and user profile state |

---

## 3. Creating a Backup

### Step 1: Perform an Online SQLite Backup

Never copy `clouddesk.db` directly while the service is active to avoid SQLite database lock contention or partial WAL state. Use the SQLite online backup tool or temporary vacuum:

```bash
# Safely snapshot the SQLite database using sqlite3 VACUUM INTO or sqlite3 .backup
sqlite3 /var/lib/clouddesk/clouddesk.db ".backup /tmp/clouddesk-backup.db"
```

### Step 2: Archive Configuration and Keys

```bash
mkdir -p /var/backups/clouddesk
tar -czf /var/backups/clouddesk/clouddesk-backup-$(date +%Y%m%d%H%M%S).tar.gz \
    /tmp/clouddesk-backup.db \
    /etc/clouddesk \
    /var/lib/clouddesk/bootstrap.secret

# Remove temporary database snapshot
rm -f /tmp/clouddesk-backup.db

# Ensure restrictive permissions on the backup archive
chmod 0600 /var/backups/clouddesk/*.tar.gz
```

---

## 4. Disaster Recovery / Restore Procedure

To restore CloudDesk-OS onto a fresh or recovered host:

### Step 1: Stop Running Services

```bash
# On systemd systems:
systemctl stop clouddesk.service cloudesk-privd.service

# On OpenRC systems:
rc-service clouddesk stop
rc-service cloudesk-privd stop
```

### Step 2: Extract Backup Archive

```bash
# Extract configuration and keys
tar -xzf /path/to/clouddesk-backup-YYYYMMDDHHMMSS.tar.gz -C /

# Restore the SQLite database file
cp /tmp/clouddesk-backup.db /var/lib/clouddesk/clouddesk.db
```

### Step 3: Verify Permissions and Ownership

```bash
chown -R clouddesk:clouddesk /var/lib/clouddesk /var/log/clouddesk
chown root:clouddesk /etc/clouddesk/clouddesk.toml /etc/clouddesk/tls/server.key \
    /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key
chmod 0640 /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key
chmod 0600 /var/lib/clouddesk/bootstrap.secret
```

### Step 4: Run Migrations and Start Services

```bash
# Verify schema integrity
su -s /bin/sh clouddesk -c "/opt/clouddesk/bin/clouddeskd migrate --config /etc/clouddesk/clouddesk.toml"

# Restart services
systemctl start cloudesk-privd.service clouddesk.service
```

---

## 5. Master Key Rotation

If the master KEK has been rotated via `clouddeskd` KEK rewrapping API, immediately perform a new backup of `/etc/clouddesk/keys/master.key` alongside the new database snapshot.
