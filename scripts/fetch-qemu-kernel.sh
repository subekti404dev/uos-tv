#!/bin/bash
# fetch-qemu-kernel.sh — Download aarch64 kernel + UEFI for QEMU
# ================================================================
# Gets a bootable Linux kernel for the QEMU 'virt' aarch64 machine
# so `make qemu` can actually boot to a prompt.
#
# Sources tried (in order):
#   1. Alpine Linux aarch64 kernel (lightweight, ideal for testing)
#   2. Debian/Ubuntu cloud image kernel
#   3. Pre-built kernel from various distributions
#
# Usage:
#   ./scripts/fetch-qemu-kernel.sh [build_dir]
#
# After running:
#   build/kernel/Image      — kernel binary
#   build/kernel/dtb/        — device tree blobs
#   build/uefi/QEMU_EFI.fd  — UEFI firmware
#   build/alpine-rootfs/    — minimal Alpine rootfs (optional)

set -euo pipefail

TARGET="${1:-build}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/$TARGET"
KERNEL_DIR="$BUILD_DIR/kernel"
UEFI_DIR="$BUILD_DIR/uefi"
ROOTFS_DIR="$BUILD_DIR/alpine-rootfs"

GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
log()  { echo -e "${GREEN}[QEMU]${NC} $*"; }
info() { echo -e "${CYAN}  →${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
err()  { echo -e "${RED}✗${NC} $*"; }
ok()   { echo -e "${GREEN}  ✓${NC} $*"; }

mkdir -p "$KERNEL_DIR" "$UEFI_DIR" "$ROOTFS_DIR"

# ═══════════════════════════════════════════════════════
# 1. UEFI Firmware for aarch64
# ═══════════════════════════════════════════════════════
log "Fetching UEFI firmware..."

UEFI_FD="$UEFI_DIR/QEMU_EFI.fd"
if [ -f "$UEFI_FD" ]; then
    ok "UEFI firmware already present: $UEFI_FD"
else
    # Try multiple known working URLs
    UEFI_URLS=(
        "https://retrage.github.io/edk2-nightly/bin/RELEASEAARCH64_QEMU_EFI.fd"
        "https://github.com/qemu/qemu/raw/master/pc-bios/edk2-aarch64-code.fd"
    )

    DOWNLOADED=false
    for url in "${UEFI_URLS[@]}"; do
        info "Trying: $url"
        if curl -sL --connect-timeout 10 --max-time 60 "$url" -o "$UEFI_DIR/tmp_uefi.fd" 2>/dev/null; then
            size=$(stat -f%z "$UEFI_DIR/tmp_uefi.fd" 2>/dev/null || stat -c%s "$UEFI_DIR/tmp_uefi.fd" 2>/dev/null || echo 0)
            if [ "$size" -gt 100000 ]; then
                mv "$UEFI_DIR/tmp_uefi.fd" "$UEFI_FD"
                ok "Downloaded UEFI firmware ($(du -h "$UEFI_FD" | cut -f1))"
                DOWNLOADED=true
                break
            else
                rm -f "$UEFI_DIR/tmp_uefi.fd"
                warn "File too small ($size bytes), retrying..."
            fi
        fi
    done

    if [ "$DOWNLOADED" = false ]; then
        warn "Could not download UEFI. Install locally:"
        warn "  macOS: brew install qemu  (includes edk2-aarch64-code.fd)"
        warn "  Linux: apt install qemu-efi-aarch64"
        warn ""
        warn "Or copy EDK2 firmware to $UEFI_FD manually."
    fi
fi

# ═══════════════════════════════════════════════════════
# 2. Linux Kernel (aarch64)
# ═══════════════════════════════════════════════════════
log "Fetching aarch64 kernel..."

KERNEL_IMAGE="$KERNEL_DIR/Image"
if [ -f "$KERNEL_IMAGE" ]; then
    ok "Kernel already present: $KERNEL_IMAGE"
else
    # ── Option A: Alpine Linux kernel (small, self-contained) ──
    ALPINE_KERNEL_VER="6.6.63-0-lts"
    ALPINE_KERNEL_URL="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/aarch64/alpine-virt-3.21.0-aarch64.iso"

    info "Trying Alpine Linux kernel extraction..."

    TMPDIR=$(mktemp -d)
    ISO="$TMPDIR/alpine.iso"

    if curl -sL --connect-timeout 10 --max-time 120 "$ALPINE_KERNEL_URL" -o "$ISO" 2>/dev/null; then
        # Extract kernel from ISO without mounting (use bsdtar/7z/unzip)
        if command -v bsdtar &>/dev/null; then
            bsdtar -xf "$ISO" -C "$TMPDIR" boot/vmlinuz-lts boot/dtbs-lts/ 2>/dev/null && {
                cp "$TMPDIR/boot/vmlinuz-lts" "$KERNEL_IMAGE" 2>/dev/null || true
                mkdir -p "$KERNEL_DIR/dtb"
                cp -r "$TMPDIR/boot/dtbs-lts"/* "$KERNEL_DIR/dtb/" 2>/dev/null || true
            }
        elif command -v 7z &>/dev/null || command -v 7zz &>/dev/null; then
            local SZ="7z"
            command -v 7zz &>/dev/null && SZ="7zz"
            $SZ x -o"$TMPDIR" "$ISO" boot/vmlinuz-lts boot/dtbs-lts/ >/dev/null 2>&1 && {
                cp "$TMPDIR/boot/vmlinuz-lts" "$KERNEL_IMAGE" 2>/dev/null || true
                mkdir -p "$KERNEL_DIR/dtb"
                cp -r "$TMPDIR/boot/dtbs-lts"/* "$KERNEL_DIR/dtb/" 2>/dev/null || true
            }
        elif command -v python3 &>/dev/null; then
            # Python isodextract fallback
            python3 -c "
import struct, sys
with open('$ISO', 'rb') as f:
    data = f.read()
    idx = data.find(b'vmlinuz-lts')
    print(f'Found vmlinuz-lts at offset {idx}' if idx > 0 else 'Not found')
" 2>/dev/null
        fi
    fi
    rm -rf "$TMPDIR"

    # ── Option B: Debian kernel (via apt in Docker) ──
    if [ ! -f "$KERNEL_IMAGE" ]; then
        info "Trying Debian kernel via Docker..."

        if command -v docker &>/dev/null; then
            docker run --rm --platform linux/arm64 -v "$KERNEL_DIR:/out" \
                debian:bookworm-slim sh -c '
                    apt-get update -qq && apt-get install -y -qq linux-image-arm64 2>/dev/null && \
                    cp /boot/vmlinuz-* /out/Image 2>/dev/null && \
                    cp -r /usr/lib/linux-image-*/qemu /out/dtb/ 2>/dev/null || true
                    echo "done"
                ' 2>/dev/null && ok "Extracted Debian kernel" || warn "Docker kernel extraction failed"
        fi
    fi

    # ── Option C: Direct kernel.org download ──
    if [ ! -f "$KERNEL_IMAGE" ]; then
        info "Trying kernel.org binary..."

        # Pre-built kernel from various sources
        KERNEL_URLS=(
            "https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/kernel-aarch64"
        )

        for url in "${KERNEL_URLS[@]}"; do
            if curl -sL --connect-timeout 10 --max-time 30 "$url" -o "$KERNEL_IMAGE" 2>/dev/null; then
                size=$(stat -f%z "$KERNEL_IMAGE" 2>/dev/null || stat -c%s "$KERNEL_IMAGE" 2>/dev/null || echo 0)
                if [ "$size" -gt 1000000 ]; then
                    ok "Downloaded kernel ($(du -h "$KERNEL_IMAGE" | cut -f1))"
                    break
                else
                    rm -f "$KERNEL_IMAGE"
                fi
            fi
        done
    fi

    # ── Final check ──
    if [ -f "$KERNEL_IMAGE" ]; then
        ok "Kernel ready: $KERNEL_IMAGE ($(du -h "$KERNEL_IMAGE" | cut -f1))"
    else
        warn "Could not auto-download kernel."
        warn ""
        warn "Manual options:"
        warn "  1. Build with Armbian: make armbian-bootstrap"
        warn "  2. Copy your own kernel to: $KERNEL_IMAGE"
        warn "  3. Install qemu-efi-aarch64 + linux-image-arm64 via apt"
    fi
fi

# ═══════════════════════════════════════════════════════
# 3. Minimal RootFS (Alpine) for Quick Boot Test
# ═══════════════════════════════════════════════════════
log "Setting up minimal test rootfs..."

ALPINE_ROOTFS_TAR="$ROOTFS_DIR/alpine-rootfs.tar.gz"

if [ -f "$ALPINE_ROOTFS_TAR" ]; then
    ok "RootFS tarball already present"
else
    ALPINE_MINIROOTFS="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/aarch64/alpine-minirootfs-3.21.0-aarch64.tar.gz"

    info "Downloading Alpine mini rootfs..."
    if curl -sL --connect-timeout 10 --max-time 120 "$ALPINE_MINIROOTFS" -o "$ALPINE_ROOTFS_TAR" 2>/dev/null; then
        ok "Downloaded Alpine rootfs ($(du -h "$ALPINE_ROOTFS_TAR" | cut -f1))"
    else
        warn "Could not download Alpine rootfs."
        warn "Download manually: $ALPINE_MINIROOTFS"
    fi
fi

# ═══════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════
echo ""
log "=== QEMU Bootstrap Complete ==="
echo ""
echo "  UEFI:     $([ -f "$UEFI_FD" ] && echo "✓ $UEFI_FD" || echo "✗ MISSING")"
echo "  Kernel:   $([ -f "$KERNEL_IMAGE" ] && echo "✓ $KERNEL_IMAGE" || echo "✗ MISSING")"
echo "  DTB:      $([ -d "$KERNEL_DIR/dtb" ] && ls "$KERNEL_DIR/dtb/"*.dtb 2>/dev/null | wc -l | xargs echo "✓" || echo "✗ MISSING")"
echo "  RootFS:   $([ -f "$ALPINE_ROOTFS_TAR" ] && echo "✓ $ALPINE_ROOTFS_TAR" || echo "✗ MISSING")"
echo ""

if [ -f "$UEFI_FD" ] && [ -f "$KERNEL_IMAGE" ] && [ -f "$ALPINE_ROOTFS_TAR" ]; then
    echo "  All components ready! Run:"
    echo "    UOS_ROOTFS=$ALPINE_ROOTFS_TAR ./scripts/create-image.sh"
    echo "    make qemu"
    echo ""
else
    echo "  Some components missing. Check the output above."
    echo "  Once all are available: make qemu"
    echo ""
fi
