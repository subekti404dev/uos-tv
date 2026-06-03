# UOS TV — Embedded Linux OS for Smart TVs

[![CI](https://github.com/subekti404dev/uos-tv/actions/workflows/ci.yml/badge.svg)](https://github.com/subekti404dev/uos-tv/actions/workflows/ci.yml)

UOS TV is a **Rust-based embedded Linux operating system** for Smart TVs, targeting ARM64 devices. It features a microservice architecture with IPC messaging, A/B OTA updates, and a web-based UI.

## Architecture

```
inis (PID 1) → monitord (supervisor) → 14 microservices
                  ↓
           stardustd (IPC broker)
            ↙        ↓        ↘
      logd, netmd  audiod  notifd, otad, ...
```

- **stardustd** — IPC message broker (Unix socket + WebSocket)
- **monitord** — Process supervisor with DAG boot, health checks, watchdog
- **inis** — PID 1 init system
- **Luna** — HTML/CSS/JS web UI with D-pad navigation
- **Cog** — WPE WebKit browser engine for app rendering (needs real GPU)

## Quick Start

```bash
# Build (macOS/Linux)
cargo build --workspace

# Test
cargo test --workspace   # 64 passed, 0 failed

# Cross-compile for ARM64 (Docker)
docker build -t uos-builder -f Dockerfile.cross .
docker run --rm -v "$PWD:/work" uos-builder \
  cargo build --release --target aarch64-unknown-linux-musl --workspace

# Run in QEMU
KERNEL=build/kernel/alpine-vmlinuz \
  INITRD=build/kernel/alpine-initramfs \
  ./scripts/ci-qemu-smoke.sh

# Open Luna UI
open http://localhost:8080/
```

## Crates (18)

| Crate | Description |
|-------|-------------|
| `stardust` | IPC message broker + WebSocket bridge |
| `monitord` | Process supervisor + health checks |
| `inis` | PID 1 init system |
| `lunad` | HTTP static file server (Luna UI) |
| `lumind` | Display manager (DRM/Cog launcher) |
| `netmd` | Network daemon (WiFi/Ethernet) |
| `audiod` | Audio daemon (ALSA volume/mute) |
| `inputd` | Input daemon (evdev/IR remote) |
| `notifd` | Notification daemon |
| `otad` | OTA update daemon (A/B slots) |
| `pkgd` | Package/App daemon |
| `logd` | Logging daemon |
| `dispald` | Display configuration daemon |
| `devmand` | Device manager |
| `powermand` | Power management daemon |
| `casync-rs` | Content-addressable chunking |
| `rauc-rs` | RAUC A/B update support |
| `update-verify` | OTA verification |

## Requirements

- Rust 1.70+
- QEMU aarch64 (`qemu-system-aarch64`)
- `genext2fs` (for disk images)
- Docker (for cross-compilation)
- Alpine Linux kernel + initrd for QEMU

## CI/CD

GitHub Actions pipeline:
1. `cargo fmt --check` + `cargo clippy`
2. `cargo test --workspace`
3. Docker cross-compile to aarch64-musl (15 ARM64 binaries)
4. QEMU smoke test (boot, 14 services, HTTP 200)

## Documentation

- **AI Developers**: Read [`AI-DEV.md`](AI-DEV.md) for full project context
- **Roadmap**: See [`uos-tv-docs.html`](uos-tv-docs.html)
- **PRD**: See [`uos-tvos-prd.html`](uos-tvos-prd.html)

## License

MIT
