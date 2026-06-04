# UOS TV — Embedded Linux OS for Smart TVs

[![CI](https://github.com/subekti404dev/uos-tv/actions/workflows/ci.yml/badge.svg)](https://github.com/subekti404dev/uos-tv/actions/workflows/ci.yml)

UOS TV is a **Rust-based embedded Linux operating system** for Smart TVs, targeting ARM64 devices. It features a microservice architecture with IPC messaging, A/B OTA updates, and a web-based UI.

## Architecture

```
S905X Boot: BootROM → u-boot → Linux + DTB → inis PID 1 → monitord
                  → wifid (modules) → stardustd → netmd + 11 services

QEMU Boot: Alpine kernel → inis PID 1 → monitord → 14 services
                  ↓
           stardustd (IPC broker)
            ↙        ↓        ↘
      logd, netmd  audiod  notifd, otad, ...
```

- **stardustd** — IPC message broker (Unix socket + WebSocket)
- **monitord** — Process supervisor with DAG boot, health checks, watchdog
- **inis** — PID 1 init system
- **wifid** — WiFi module loader (one-shot, loads Realtek/Broadcom SDIO .ko)
- **Luna** — HTML/CSS/JS web UI with D-pad navigation
- **Cog** — WPE WebKit browser engine for app rendering (needs real GPU)

## Quick Start

```bash
# Build (macOS/Linux)
cargo build --workspace

# Test
cargo test --workspace   # 56 passed, 0 failed

# Cross-compile for ARM64 (Docker)
docker build -t uos-builder -f Dockerfile.cross .
docker run --rm -v "$PWD:/work" uos-builder \
  cargo build --release --target aarch64-unknown-linux-musl --workspace

# Build S905X system image (for set-top boxes)
./scripts/bootstrap-s905x.sh --board b860h     # Build for ZTE B860H
./scripts/bootstrap-s905x.sh --board hg680p    # Build for HG680P
./scripts/bootstrap-s905x.sh --all             # Build for all boards

# Run in QEMU
KERNEL=build/kernel/alpine-vmlinuz \
  INITRD=build/kernel/alpine-initramfs \
  ./scripts/ci-qemu-smoke.sh

# Open Luna UI
open http://localhost:8080/
```

## Crates (19)

| Crate | Description |
|-------|-------------|
| `stardust` | IPC message broker + WebSocket bridge |
| `monitord` | Process supervisor + health checks |
| `inis` | PID 1 init system |
| `lunad` | HTTP static file server (Luna UI) |
| `lumind` | Display manager (DRM/Cog launcher) |
| `netmd` | Network daemon (WiFi/Ethernet) |
| `wifid` | WiFi module loader — SDIO probe, insmod, one-shot |
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

## Supported Hardware

| Target | SoC | GPU | WiFi | Status |
|--------|-----|-----|------|--------|
| **ZTE B860H** | S905X-B | Mali-450 | RTL8189ES SDIO | ✅ Build ready |
| **HG680P** | S905X | Mali-450 | RTL8189FS SDIO | ✅ Build ready |
| Nexbox A95X | S905X | Mali-450 | RTL8723BS SDIO | ✅ Build ready |
| LibreTech CC | S905X | Mali-450 | RTL8723BS SDIO | ✅ Build ready |
| Khadas VIM | S905X | Mali-450 | BCM43430 SDIO | ✅ Build ready |
| QEMU virt | Cortex-A57 | None (headless) | None | ✅ CI daily |

```bash
# Build for specific board (auto DTB + WiFi chip)
./scripts/bootstrap-s905x.sh --board b860h

# Build all WiFi drivers
make wifi-s905x
./scripts/build-s905x-wifi.sh --chip rtl8189es
```

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
