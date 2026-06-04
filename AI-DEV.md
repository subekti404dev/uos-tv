# UOS TV — AI Developer Context

> **Read this first** when starting a new AI session. This document provides everything needed to continue development of UOS TV (Unified Operating System for Smart TVs).

## Project Overview

**UOS TV** is a Rust-based embedded Linux operating system for Smart TVs, running on ARM64 (aarch64) devices. It uses a microservice architecture with an IPC message broker (Stardust), an init system (inis), a process supervisor (monitord), and a web-based UI (Luna).

- **Language**: Rust (2021 edition), statically linked via musl for ARM64
- **Target**: `aarch64-unknown-linux-musl`
- **QEMU Machine**: `virt`, `cortex-a57`, 512MB RAM
- **CI**: GitHub Actions (cross-compile + QEMU smoke test)

## Architecture

```
┌─────────────────────────────────────────────────┐
│                    Luna UI                       │
│         (HTML/CSS/JS served via lunad)           │
│         WebSocket ↔ ws://127.0.0.1:9090         │
└──────────────────┬──────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────┐
│              stardustd (IPC Broker)              │
│       Unix socket: /run/uos/bus.sock             │
│       WebSocket bridge: 0.0.0.0:9090             │
└──────┬───────┬───────┬───────┬──────────────────┘
       │       │       │       │
   ┌───▼──┐ ┌──▼──┐ ┌──▼───┐ ┌▼──────┐
   │logd  │ │netmd│ │audiod│ │notifd │  ...
   └──────┘ └─────┘ └──────┘ └───────┘
```

### Boot Chain
```
Alpine Linux kernel → Alpine Init → inis (PID 1) → monitord → 14 services
```

## Crate Map

| Crate | Binary | Purpose |
|-------|--------|---------|
| `stardust` | `stardustd` | **Core IPC broker** — Unix socket + WebSocket, pub/sub, topic routing |
| `monitord` | `monitord` | **Process supervisor** — DAG-based service boot, health checks, watchdog, capability dropping |
| `inis` | `inis` | **Init system (PID 1)** — mounts filesystems, configures network, forks monitord, zombie reaper, rootfs hardening |
| `lunad` | `lunad` | **HTTP static file server** — serves Luna UI from `/var/www/luna` on port 80 |
| `lumind` | `lumind` | **Display manager** — scans DRM/input devices, launches Cog (WPE WebKit) |
| `netmd` | `netmd` | **Network daemon** — WiFi scanning (wpa_cli), Ethernet, connectivity status |
| `audiod` | `audiod` | **Audio daemon** — ALSA mixer control, volume (0-100%), mute, card enumeration |
| `inputd` | `inputd` | **Input daemon** — evdev (/dev/input/event*), IR remote key mapping, key events via stardust |
| `notifd` | `notifd` | **Notification daemon** — stores/forwards notifications, Luna UI subscription |
| `otad` | `otad` | **OTA update daemon** — download, verify (TLS CA bundle), orchestrate A/B slot updates |
| `pkgd` | `pkgd` | **Package daemon** — app registry, install, launch |
| `logd` | `logd` | **Logging daemon** — collects and forwards logs |
| `dispald` | `dispald` | **Display daemon** — EDID, resolution, brightness |
| `devmand` | `devmand` | **Device manager** — udev-like device discovery |
| `powermand` | `powermand` | **Power daemon** — shutdown, reboot, sleep |
| `casync-rs` | — | Content-addressable chunking library |
| `rauc-rs` | — | RAUC A/B update library |
| `update-verify` | — | OTA update verification library |

## Key Files

### Configuration
- `configs/services.d/*.yaml` — Service manifests (14 services). Each defines binary, args, dependencies, caps, health_check
- `configs/monitord.yaml` — Supervisor config
- `configs/system.yaml` — System-level settings
- `configs/alpine-overlay/` — Alpine Linux overlay files

### Build & CI
- `Cargo.toml` — Workspace definition with 18 crates
- `Cargo.lock` — Locked dependency versions
- `Makefile` — Common tasks: `make build`, `make qemu`, `make ci-qemu-smoke`, `make cross-build`
- `Dockerfile.cross` — Docker image for cross-compiling to aarch64-musl
- `Dockerfile.lumind` — Docker image for lumind build (experimental)
- `.github/workflows/ci.yml` — CI pipeline: format, clippy, test, cross-build, QEMU smoke
- `scripts/` — Shell scripts for build, QEMU, OTA, image creation

### UI (Luna)
- `luna/index.html` — Luna shell HTML with screen definitions
- `luna/js/bus.js` — `StardustClient` — WebSocket connection to stardust (`ws://127.0.0.1:9090`)
- `luna/js/app.js` — `LunaApp` — app controller, screen management, app launching
- `luna/js/nav.js` — `NavigationManager` — **2D spatial D-pad navigation** (visual grid-based)
- `luna/css/shell.css` — All UI styles

### Documentation
- `uos-tv-docs.html` — Detailed development roadmap, P0/P1/P2 status (all 100% done)
- `uos-tvos-prd.html` — Original product requirements doc

## Critical Code Paths

### 1. Message Flow (Luna UI → Stardust → Service)
```
app.js: bus.publish('input.key', params)
  → bus.js: WebSocket JSON → ws://127.0.0.1:9090
  → ws.rs: WsRequest::Publish → broker.publish()
  → broker.rs: BrokerCmd::Route → topic matching → connection send
  → service process: Unix socket Frame → handle_connection()
```

### 2. Service Boot (monitord)
```
supervisor.rs: boot_sequence()
  → graph.rs: topological sort (DAG from dependencies)
  → fork + exec each service in order
  → health_check_all() every 5s
  → auto-restart after 3 failures
```

### 3. Capability Security
```
supervisor.rs: pre_exec() → sec.rs: apply_capability_bounds(keep_caps)
  → PR_CAPBSET_DROP for each cap NOT in keep list
  → PR_SET_SECUREBITS, PR_SET_NO_NEW_PRIVS
  → ServiceManifest.caps.keep: ["net_bind_service", ...]
```

## Build Commands

```bash
# Native build (macOS, for testing)
cargo build --workspace

# Full test suite
cargo test --workspace

# Cross-compile for ARM64 (requires Docker)
docker build -t uos-builder -f Dockerfile.cross .
docker run --rm -v "$PWD:/work" uos-builder cargo build --release --target aarch64-unknown-linux-musl --workspace

# Deploy binaries to rootfs
for bin in inis monitord stardustd lunad audiod inputd netmd notifd otad pkgd logd dispald devmand powermand lumind cog; do
  cp target/aarch64-unknown-linux-musl/release/$bin build/alpine-rootfs-extracted/usr/bin/
done

# Run QEMU smoke test
KERNEL=build/kernel/alpine-vmlinuz INITRD=build/kernel/alpine-initramfs TIMEOUT_SECS=25 MIN_SERVICES=13 ./scripts/ci-qemu-smoke.sh
```

## QEMU Testing

### Prerequisites
- `qemu-system-aarch64` installed
- `genext2fs` for building ext2 disk images
- Kernel + initrd in `build/kernel/` (downloaded via `scripts/fetch-qemu-kernel.sh`)

### Network
- Guest: `10.0.2.15/24`, gateway `10.0.2.2`
- Host forward: `:8080→:80` (Luna HTTP), `:9090→:9090` (Stardust WS)

### Smoke Test Script
`scripts/ci-qemu-smoke.sh`:
1. Builds ext2 image from `build/alpine-rootfs-extracted/` (900MB)
2. Launches QEMU with 25s timeout
3. Waits for "Boot complete" in log
4. Counts `[OK]` services (min 13)
5. Validates `curl http://localhost:8080/` returns 200

### Luna UI Testing
1. Start QEMU (via smoke test or `scripts/run-qemu.sh`)
2. Open browser: `http://localhost:8080/`
3. Hard refresh: `Cmd+Shift+R` (cache-bust)
4. D-pad: arrow keys, Enter/Space to click, Esc for home, N for notifications
5. Check `F12 → Console` for errors

## Known Limitations

1. **Cog/WPE WebKit** — Integrated but needs real ARM64 GPU/DRM for rendering. QEMU `virt` has no GPU, so Cog runs headless. **Test on real hardware** (S905X, Raspberry Pi, Rockchip) for full rendering.
2. **lumind (Smithay)** — Full Wayland compositor deferred. Current version is a minimal display manager that scans DRM/input and launches Cog.
3. **audiod** — ALSA backend works but needs real sound hardware. QEMU has no sound card. S905X has an HDMI audio output via meson audio driver.
4. **inputd** — evdev scanning works but QEMU has no input devices. IR remote on S905X is supported via meson-ir driver, exposed as `/dev/input/event*`.
5. **netmd WiFi** — Uses `wpa_cli`. No WiFi in QEMU; Ethernet-only. S905X boxes typically have RTL8723BS or Broadcom WiFi.
6. **Read-only rootfs** — inis remounts `/` as RO after boot. Busybox httpd incompatible; use Rust `lunad` instead.
7. **Capability dropping** — Services needing port bind must have `caps.keep: [net_bind_service]` in manifest.
8. **cap_last_cap** — macOS libc lacks newer Linux capabilities (BPF, CHECKPOINT_RESTORE). Use raw numbers (0-40) in `sec.rs`.

## Hardware Targets

### Amlogic S905X (Primary Target)

**SoC**: Amlogic S905X (quad Cortex-A53 @ 1.5GHz, Mali-450 MP3 GPU)

| Component | Status | Driver |
|-----------|--------|--------|
| CPU ARMv8-A | ✅ Fully supported | aarch64-musl target |
| GPU Mali-450 | ✅ Works with lima | Open-source Mesa driver (OpenGL ES 2.0) |
| Display HDMI | ✅ Works with meson DRM/KMS | Full modesetting, atomic |
| SD/eMMC | ✅ meson-gx-mmc | Boot from SD card |
| Ethernet | ✅ stmmac | 100Mbps (some have GbE) |
| WiFi | ⚠️ Varies per box | See WiFi driver table below |
| IR Remote | ✅ meson-ir | `/dev/input/event*` |
| Audio HDMI | ✅ meson audio | ALSA via meson driver |

**Build for S905X**:
```bash
./scripts/bootstrap-s905x.sh --all        # Full build (kernel + rootfs + SD image)
./scripts/bootstrap-s905x.sh --kernel-docker  # Docker-based kernel cross-compile
```

**Critical difference from QEMU**: With Mali-450 GPU + lima + Mesa, **Cog/WPE WebKit can render the Luna UI natively on screen via HDMI**. This is the target deployment platform.

**Board**: Generic `meson-gxl-s905x-p212` device tree. Most Android TV boxes use this reference design. DTB = `meson-gxl-s905x-p212.dtb`.

**WiFi Drivers**: Out-of-tree kernel modules for some chips.

| Box | WiFi Chip | Driver | Module | Repo |
|-----|-----------|--------|--------|------|
| B860H | RTL8189ES (SDIO) | Out-of-tree | `8189es.ko` | `openwetek/rtl8189es` |
| HG680P | RTL8189FS (SDIO) | Out-of-tree | `8189fs.ko` | `OpenIPC/realtek-wlan` (Amlogic S905 platform) |
| Nexbox-A95X | RTL8723BS (SDIO) | Mainline staging | `r8723bs.ko` | Kernel `drivers/staging/rtl8723bs` |
| LibreTech-CC | RTL8723BS (SDIO) | Mainline staging | `r8723bs.ko` | Kernel `drivers/staging/rtl8723bs` |
| Khadas-VIM | BCM43430 (SDIO) | Mainline | `brcmfmac.ko` | `CONFIG_BRCMFMAC=m` |

**WiFi firmware**: Downloaded from `linux-firmware` repo:
- `rtlwifi/rtl8188eufw.bin` — for RTL8189ES
- `rtlwifi/rtl8188fufw.bin` — for RTL8189FS
- `rtlwifi/rtl8723bs_nic.bin` — for RTL8723BS
- `brcm/brcmfmac43430-sdio.bin` — for BCM43430

**WiFi build**:
```bash
make wifi-s905x                    # Build kernel, then all WiFi drivers + firmware
./scripts/build-s905x-wifi.sh --all  # Standalone driver build
./scripts/build-s905x-wifi.sh --chip rtl8189es  # Single chip
```

**Boot flow on S905X**:
```
BootROM (eMMC/SD) → u-boot → Linux kernel + DTB → inis (PID 1) → monitord → services
→ meson DRM/KMS + lima GPU → Cog/WPE WebKit → Luna UI on HDMI screen!
```

### QEMU virt (Development)

Used for fast iteration and CI. No GPU/DRM — Cog runs headless. Luna UI accessed via web browser at `http://localhost:8080/`.

## Conventions

### Service Manifest Format
```yaml
name: myservice
description: "..."
binary: /usr/bin/myservice
args: ["--flag", "value"]
dependencies: [stardust, logd]
restart: always
restart_delay_ms: 3000
max_crash_count: 5
crash_window_secs: 30
critical: false
startup_timeout_secs: 5
health_check: tcp:80                # or: stardust.ping, rpc:*
caps:
  keep:
    - net_bind_service
```

### Stardust Message Protocol
```json
// Subscribe
{"type":"subscribe","topic":"notification.list"}

// Publish  
{"type":"publish","method":"input.key","params":{"key":"KEY_ENTER","state":"press"}}

// Event (server→client)
{"type":"event","method":"audio.status","params":{"volume":70,"muted":false}}

// Call/Response
{"type":"call","method":"system.info","params":{},"id":"req1"}
{"type":"response","id":"req1","status":"ok","data":{...}}
```

### Security: Capability Names → Numbers
```
net_bind_service=10, net_admin=12, net_raw=13, sys_admin=21, sys_boot=27
```

### D-pad Navigation (nav.js)
Uses spatial grid: groups visible focusable elements by `getBoundingClientRect()` into rows (±24px vertical tolerance). Arrow keys navigate by row/column position. Special handling for `<input type="range">` (adjust value with arrows, Enter to exit).

## File Structure (Git-tracked only)

```
uos/
├── .github/workflows/ci.yml
├── Cargo.toml
├── Cargo.lock
├── Makefile
├── Dockerfile.cross
├── Dockerfile.lumind
├── .gitignore
├── AI-DEV.md              ← This file
├── README.md
├── configs/
│   ├── monitord.yaml
│   ├── system.yaml
│   ├── services.d/        ← 14 service YAML manifests
│   └── alpine-overlay/
├── crates/
│   ├── stardust/          ← IPC broker (lib + bin)
│   ├── monitord/          ← Supervisor (lib + bin)
│   ├── inis/              ← Init system
│   ├── lunad/             ← HTTP server
│   ├── lumind/            ← Display manager
│   ├── netmd/             ← Network daemon
│   ├── audiod/            ← Audio daemon (ALSA)
│   ├── inputd/            ← Input daemon (evdev)
│   ├── notifd/            ← Notification daemon
│   ├── otad/              ← OTA update daemon
│   ├── pkgd/              ← Package daemon
│   ├── logd/              ← Logging daemon
│   ├── dispald/           ← Display daemon
│   ├── devmand/           ← Device manager
│   ├── powermand/         ← Power daemon
│   ├── casync-rs/         ← CASync library
│   ├── rauc-rs/           ← RAUC library
│   └── update-verify/     ← Update verification
├── luna/                   ← Luna UI (HTML/CSS/JS)
│   ├── index.html
│   ├── js/
│   │   ├── app.js
│   │   ├── bus.js
│   │   └── nav.js
│   └── css/
│       └── shell.css
├── scripts/
│   ├── ci-qemu-smoke.sh
│   ├── run-qemu.sh
│   ├── fetch-qemu-kernel.sh
│   ├── overlay-alpine.sh
│   ├── create-image.sh
│   ├── create-image-gpt.sh
│   ├── create-image-simple.sh
│   ├── ota-create-bundle.sh
│   ├── dev-run.sh
│   ├── bootstrap-armbian.sh
│   └── build-cog.sh
├── tests/
│   └── integration.rs
├── deploy/
│   └── docker-compose.dev.yml
├── docs/
├── uos-tv-docs.html       ← Development roadmap
└── uos-tvos-prd.html      ← Product requirements
```

## Quick Start (New Session)

```bash
# 1. Build
cargo build --workspace

# 2. Test
cargo test --workspace

# 3. Cross-compile for ARM64 (Docker)
docker build -t uos-builder -f Dockerfile.cross .
docker run --rm -v "$PWD:/work" uos-builder cargo build --release --target aarch64-unknown-linux-musl --workspace

# 4. Run QEMU
KERNEL=build/kernel/alpine-vmlinuz INITRD=build/kernel/alpine-initramfs TIMEOUT_SECS=25 ./scripts/ci-qemu-smoke.sh

# 5. Open Luna UI
open http://localhost:8080/
```

## Test Status (Last Verified: 2026-06-04)

- `cargo test --workspace`: **64 passed, 3 ignored, 0 failed**
- QEMU smoke: **14 services OK, boot ~10-13s, Luna HTTP 200**
- All P0 (3/3), P1 (6/6), P2 (8/8) tasks complete
- CI/CD pipeline: GitHub Actions (format, clippy, test, cross-build, QEMU smoke)
