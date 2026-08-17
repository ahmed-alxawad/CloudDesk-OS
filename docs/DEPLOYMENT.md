# CloudDesk-OS Production Deployment Guide

This guide describes production deployment architectures, reverse proxy integrations, TLS termination, and operating system security hardening.

---

## 1. Direct HTTPS Deployment (Default)

By default, CloudDesk-OS listens on port `9870` with built-in Rustls TLS termination.

```
[Browser Client]
       │
       ▼ (HTTPS :9870 / WSS :9870)
[CloudDesk-OS Core (`clouddeskd`)]
```

### Configuration (`/etc/clouddesk/clouddesk.toml`)

```toml
[server]
address = "0.0.0.0"
port = 9870
development_http = false

[tls]
certificate = "/etc/clouddesk/tls/server.crt"
private_key = "/etc/clouddesk/tls/server.key"
```

During initial installation, a self-signed TLS certificate is generated automatically. Browsers will display a certificate warning on first connection until a trusted domain certificate is configured.

---

## 2. Reverse Proxy Deployment

In multi-service environments or enterprise networks, CloudDesk-OS can be deployed behind a front-end reverse proxy (Caddy, Nginx, or Traefik) terminating public TLS on port `443`.

```
[Browser Client]
       │
       ▼ (HTTPS :443 / WSS :443)
[Reverse Proxy (Caddy / Nginx / Traefik)]
       │
       ▼ (HTTPS / WSS :9870)
[CloudDesk-OS Core (`clouddeskd`)]
```

### A. Caddy Example (`/etc/caddy/Caddyfile`)

```caddy
clouddesk.example.com {
    reverse_proxy https://127.0.0.1:9870 {
        transport http {
            tls_insecure_skip_verify
        }
        header_up Host {host}
        header_up X-Real-IP {remote_host}
        header_up X-Forwarded-For {remote_host}
        header_up X-Forwarded-Proto {scheme}
    }
}
```

### B. Nginx Example (`/etc/nginx/sites-available/clouddesk`)

```nginx
server {
    listen 80;
    server_name clouddesk.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name clouddesk.example.com;

    ssl_certificate /etc/letsencrypt/live/clouddesk.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/clouddesk.example.com/privkey.pem;

    client_max_body_size 10G;

    location / {
        proxy_pass https://127.0.0.1:9870;
        proxy_ssl_verify off;
        proxy_http_version 1.1;

        # WebSocket support
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";

        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Disable buffering for streaming uploads/downloads
        proxy_buffering off;
        proxy_request_buffering off;
    }
}
```

---

## 3. SELinux & AppArmor Hardening

### SELinux (RHEL / Rocky / AlmaLinux / Fedora)
CloudDesk-OS ships with pre-configured SELinux policies in `packaging/selinux/`.
To compile and install the policy:
```bash
checkmodule -M -m -o clouddesk.mod packaging/selinux/clouddesk.te
semodule_package -o clouddesk.pp -m clouddesk.mod
semodule -i clouddesk.pp
```

### AppArmor (Debian / Ubuntu)
Profiles are provided in `packaging/apparmor/usr.bin.clouddeskd`:
```bash
cp packaging/apparmor/usr.bin.clouddeskd /etc/apparmor.d/
apparmor_parser -r /etc/apparmor.d/usr.bin.clouddeskd
```

---

## 4. Service Supervision

- **systemd** systems use `/etc/systemd/system/cloudesk-privd.service` and `/etc/systemd/system/clouddesk.service`.
- **OpenRC (Alpine Linux)** systems use `/etc/init.d/cloudesk-privd` and `/etc/init.d/clouddesk`.
