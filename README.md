# CloudDesk OS

> Transform your CLI-only Linux cloud server into a complete, browser-based graphical workspace.

![Version](https://img.shields.io/badge/version-0.1.0-blue)
![Phase](https://img.shields.io/badge/phase-1%20MVP-orange)
![Go](https://img.shields.io/badge/Go-1.22+-00ADD8)
![React](https://img.shields.io/badge/React-18-61DAFB)
![License](https://img.shields.io/badge/license-MIT-green)

---

## Overview

CloudDesk OS bridges the gap between raw Linux server power and modern web-based usability. It provides a self-hosted web OS that abstracts complex CLI operations, file transfers, and development environments into an intuitive browser interface.

**Phase 1 includes:**
- **PAM Authentication** — Authenticate using native Linux system credentials
- **File Manager** — Browse, upload, download, and manage files with drag-drop
- **Code Editor** — Persistent VS Code instance that survives logout
- **Media Preview** — Image, PDF, and HTML5 media playback

## Quick Install

```bash
# One-liner install
curl -sSL https://raw.githubusercontent.com/youruser/clouddesk-os/main/install.sh | sudo bash -s install
```

Or download the entire project and install from source:

```bash
git clone https://github.com/youruser/clouddesk-os.git
cd clouddesk-os
sudo bash install.sh install
```

### Requirements

- Linux server (Ubuntu 20.04+, Debian 11+, CentOS 8+, RHEL 8+)
- Root access
- Minimum 1GB RAM, 1 CPU core

### Uninstall

```bash
sudo bash install.sh uninstall
```

## Architecture

```
┌──────────────────────┐     HTTPS / HTTP
│  Browser (React SPA) │◄───────────────────┐
└──────────────────────┘                    │
           │                                 │
           ▼                                 │
┌──────────────────────┐                    │
│   Nginx (Reverse      │────────────────────┘
│   Proxy + TLS)        │
└──────────────────────┘
           │
           ▼
┌──────────────────────┐
│  Go Backend (Gin)    │
│  ┌────────────────┐  │
│  │ PAM Auth (CGO) │  │──► /etc/shadow
│  ├────────────────┤  │
│  │ Local VFS      │  │──► /home/* (privilege drop)
│  ├────────────────┤  │
│  │ IDE Manager    │  │──► code-server (background proc)
│  ├────────────────┤  │
│  │ AES-256 Crypto │  │
│  └────────────────┘  │
└──────────────────────┘
```

## Security Model

| Layer | Mechanism |
|-------|-----------|
| Authentication | Linux PAM (system credentials) |
| Sessions | JWT (HS512) in httpOnly cookies |
| File Access | Linux kernel enforcement via UID/GID privilege drop |
| SSH Keys | AES-256-GCM encryption at rest |
| Transport | Nginx TLS termination (HTTPS) |
| Audit | Every mutation logged with IP + timestamp |

## Project Structure

```
clouddesk-os/
├── backend/
│   ├── cmd/server/main.go        # Entry point
│   ├── internal/
│   │   ├── auth/pam.go           # PAM CGO authentication
│   │   ├── auth/jwt.go           # JWT token management
│   │   ├── vfs/local.go          # Local filesystem with privilege drop
│   │   ├── ide/manager.go        # code-server process lifecycle
│   │   ├── crypto/aes.go         # AES-256-GCM encryption
│   │   ├── api/                  # HTTP handlers + router
│   │   └── config/config.go      # Configuration management
│   └── pkg/models/models.go      # Data models
├── frontend/
│   ├── src/
│   │   ├── views/                # Login, Dashboard, FileManager, IDE
│   │   ├── components/           # Sidebar, Header
│   │   ├── store/                # Zustand state management
│   │   └── lib/api.ts            # API client
│   └── package.json
├── nginx/clouddesk.conf          # Nginx configuration
├── install.sh                    # Install / Uninstall script
└── README.md
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/auth/login` | PAM authentication → JWT |
| POST | `/api/auth/refresh` | Refresh JWT token |
| GET | `/api/v1/fs/list?path=` | List directory contents |
| POST | `/api/v1/fs/upload` | Upload file (multipart) |
| GET | `/api/v1/fs/download?path=` | Download/stream file |
| DELETE | `/api/v1/fs/delete?path=` | Delete file/directory |
| POST | `/api/v1/fs/mkdir` | Create directory |
| POST | `/api/v1/fs/rename` | Rename/move file |
| GET | `/api/v1/ide/status` | Code-server status |
| POST | `/api/v1/ide/start` | Start code-server |
| POST | `/api/v1/ide/stop` | Stop code-server |
| ANY | `/api/v1/ide/proxy/*` | WebSocket/HTTP proxy to IDE |

## Development

### Backend

```bash
cd backend
go mod download
CGO_ENABLED=1 go build -o clouddesk-server ./cmd/server/
sudo ./clouddesk-server --jwt-secret devsecret --master-key devkey1234567890123456
```

### Frontend

```bash
cd frontend
npm install
npm run dev    # Development server on :3000
npm run build  # Production build
```

## Roadmap

- **Phase 1** (Current): PAM auth, local file manager, IDE, media preview
- **Phase 2**: SSH key storage, VFS abstraction, SFTP/SCP, remote server manager
- **Phase 3**: Rsync, SMB, WebDAV, S3, file sharing, full-text search
- **Phase 4**: OAuth2, desktop sync client, multi-user workspaces

## License

MIT License. See [LICENSE](LICENSE) for details.
