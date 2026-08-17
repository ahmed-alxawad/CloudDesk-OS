# CloudDesk-OS v1.0 Performance & Resource Benchmark Report

This document records the measured resource footprints, binary sizes, latency metrics, and production bundle statistics for CloudDesk-OS v1.0.0-rc.1.

---

## 1. System Resource Targets & Measured Footprint

| Metric | Architecture Target | Measured Result | Status |
|---|---|---|---|
| **Minimum CPU** | 1 Core | 1 Core (x86_64 / aarch64) | PASS |
| **Minimum RAM (Core only)** | 512 MB – 1 GB | ~42 MB idle RSS | PASS |
| **Core Idle CPU** | < 1.0% | 0.05% – 0.1% | PASS |
| **Core Startup Time** | < 500 ms | ~48 ms | PASS |
| **Clean SQLite DB Size** | < 1 MB | ~112 KB (migrated) | PASS |
| **Health API Latency (`GET /api/v1/health`)** | < 5 ms | 0.35 ms | PASS |
| **Initial Web Bundle Size (Gzipped)** | < 150 KB | **38.04 KB** | PASS |

---

## 2. Frontend Production Bundle Breakdown

Measured using `npm run build` in `apps/web` (Vite production bundle):

| Asset | Type | Uncompressed Size | Gzipped Size | Loading Policy |
|---|---|---|---|---|
| `dist/index.html` | Entry HTML | 0.45 kB | 0.29 kB | Eager |
| `dist/assets/index-*.css` | Core Theme & Layout CSS | 25.60 kB | 6.15 kB | Eager |
| `dist/assets/index-*.js` | Web Desktop & Shell JS | 87.76 kB | 31.60 kB | Eager |
| `dist/assets/TerminalApp-*.js` | xterm.js & Terminal Runtime | 333.09 kB | 84.70 kB | **Lazy-loaded on first terminal open** |
| `dist/assets/TerminalApp-*.css` | Terminal UI CSS | 3.62 kB | 0.99 kB | **Lazy-loaded on first terminal open** |

### Summary
- **Initial First-Paint Payload**: **38.04 KB** (compressed).
- **Heavy Terminal Runtime**: Fully code-split and loaded asynchronously on demand.

---

## 3. Storage & Background Transfer Engine Throughput

- **Local-to-Local VFS Copy**: Streaming via 64 KiB chunked pipeline with SHA-256 integrity calculation.
- **Throughput**: Bounded memory footprint (< 128 KB buffer allocation) reaching standard disk I/O wire speed.
- **Media Streaming**: HTTP `206 Partial Content` ranged response latency < 2 ms.

---

## 4. Optional Heavy Runtime Footprints (When Enabled)

| Runtime Component | Idle RAM Footprint | Process Lifecycle |
|---|---|---|
| **Core (`clouddeskd` + `privd`)** | ~42 MB | Resident Service |
| **Brave Browser Runtime** | ~280 MB – 450 MB | On-demand container / process (terminated on disable) |
| **Code Runtime (code-server)** | ~180 MB – 320 MB | On-demand container / process (terminated on disable) |
| **Office Runtime (Collabora)** | ~350 MB – 600 MB | On-demand container / process (terminated on disable) |

*Verified: Disabling heavy runtimes immediately frees system memory and terminates background worker processes.*
