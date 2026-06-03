#!/bin/bash
# build-cog.sh — Cross-compile WPE WebKit Cog for aarch64
# ==========================================================
# Cog is a single-window WPE WebKit launcher used as the
# Luna UI compositor replacement (instead of full Smithay).
#
# This script cross-compiles cog + libwpe + WPEBackend-fdo
# for aarch64 inside Docker.
#
# Prerequisites:
#   - Docker (for cross-compilation)
#   - Or: aarch64-linux-gnu toolchain on host
#
# Usage:
#   ./scripts/build-cog.sh [output_dir]
#
# Output:
#   build/cog/usr/bin/cog              — Cog launcher
#   build/cog/usr/lib/aarch64-linux-gnu/  — WPE libs

set -euo pipefail

OUTPUT="${1:-build/cog}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/$OUTPUT"

GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[COG]${NC} $*"; }
info() { echo -e "${CYAN}  →${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }

mkdir -p "$BUILD_DIR"

# ═══════════════════════════════════════════════════════
# Docker-based Cross-compilation
# ═══════════════════════════════════════════════════════

if command -v docker &>/dev/null; then
    log "Building Cog via Docker (aarch64 cross-compile)..."

    COG_DOCKERFILE=$(mktemp)
    cat > "$COG_DOCKERFILE" <<'DOCKERFILE'
FROM debian:bookworm-slim

RUN dpkg --add-architecture arm64 && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential \
        crossbuild-essential-arm64 \
        cmake \
        meson \
        ninja-build \
        pkg-config \
        git \
        ca-certificates \
        libglib2.0-dev:arm64 \
        libsoup-3.0-dev:arm64 \
        libwpe-1.0-dev:arm64 \
        libwpebackend-fdo-1.0-dev:arm64 \
        libcairo2-dev:arm64 \
        libegl1-mesa-dev:arm64 \
        libgles2-mesa-dev:arm64 \
        libwayland-dev:arm64 \
        wayland-protocols \
    && apt-get clean

WORKDIR /build

# Build Cog from source
RUN git clone --depth=1 --branch=cog-0.20.1 \
    https://github.com/Igalia/cog.git /build/cog 2>/dev/null || \
    git clone --depth=1 https://github.com/Igalia/cog.git /build/cog

WORKDIR /build/cog

RUN meson setup build \
    --cross-file /dev/stdin \
    --prefix=/output/usr \
    -Dplatforms=drm,wayland <<CROSS
[host_machine]
system = 'linux'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'

[binaries]
c = '/usr/bin/aarch64-linux-gnu-gcc'
cpp = '/usr/bin/aarch64-linux-gnu-g++'
ar = '/usr/bin/aarch64-linux-gnu-ar'
strip = '/usr/bin/aarch64-linux-gnu-strip'
pkgconfig = '/usr/bin/aarch64-linux-gnu-pkg-config'

[properties]
sys_root = '/usr/aarch64-linux-gnu'
pkg_config_libdir = '/usr/lib/aarch64-linux-gnu/pkgconfig'
CROSS

RUN DESTDIR=/output ninja -C build install

CMD ["sh", "-c", "echo 'Cog built!'; ls -la /output/usr/bin/"]
DOCKERFILE

    docker build -t uos-cog-builder -f "$COG_DOCKERFILE" "$PROJECT_DIR" 2>&1 | tail -10 || {
        warn "Docker build failed — Cog may need manual cross-compilation."
        warn "See: https://github.com/Igalia/cog"
    }

    # Extract binaries
    CID=$(docker create uos-cog-builder)
    docker cp "$CID:/output/." "$BUILD_DIR/" 2>/dev/null || true
    docker rm "$CID" >/dev/null 2>&1

    rm -f "$COG_DOCKERFILE"

    if [ -f "$BUILD_DIR/usr/bin/cog" ]; then
        log "Cog built successfully: $BUILD_DIR/usr/bin/cog"
    fi
else
    warn "Docker not found. Skipping Cog build."
    warn "Manual instructions:"
    warn "  git clone https://github.com/Igalia/cog.git"
    warn "  meson setup build --cross-file=aarch64-cross.txt"
    warn "  ninja -C build"
fi

# ═══════════════════════════════════════════════════════
# Cog Configuration for Luna UI
# ═══════════════════════════════════════════════════════

log "Creating cog configuration..."

CONFIG_DIR="$BUILD_DIR/usr/share/uos"
mkdir -p "$CONFIG_DIR"

cat > "$CONFIG_DIR/cog.ini" <<'CONF'
[cog]
# Luna UI — WPE WebKit Cog configuration
platform=drm
# use-fullscreen=true
# scale-factor=1.0
# view-backend=wl
app-manifest=/usr/share/uos/luna/luna.app

[shell]
# Luna UI index
home-uri=file:///usr/share/uos/luna/index.html
# No browser chrome — full kiosk mode
# bg-color=#000000
CONF

# Cog .app manifest
mkdir -p "$CONFIG_DIR/luna"
cat > "$CONFIG_DIR/luna/luna.app" <<'APP'
[App]
Name=Luna UI Shell
Exec=cog
StartupWMClass=cog
APP

log "Cog configuration written to $CONFIG_DIR/"
log ""
log "To use Cog with Luna UI:"
log "  cog --config=/usr/share/uos/cog.ini"
log "  or: WPE_BCMRX=1 cog file:///usr/share/uos/luna/index.html"
