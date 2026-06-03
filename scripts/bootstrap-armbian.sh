#!/bin/bash
# bootstrap-armbian.sh — Fetch Armbian rootfs + kernel for UOS TV
# ================================================================
# Downloads a minimal Armbian rootfs tarball and kernel for aarch64.
# Used as the base for UOS TV disk images.
#
# Usage:
#   ./scripts/bootstrap-armbian.sh [target_dir]
#
# Sources:
#   - Armbian minimal CLI images (bookworm)
#   - QEMU-ready EDK2 UEFI firmware
#
# After running:
#   build/armbian-rootfs/  — extracted rootfs
#   build/kernel/          — kernel Image + dtb/

set -euo pipefail

TARGET="${1:-build}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/$TARGET"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[ARMBIAN]${NC} $*"; }
warn() { echo -e "${YELLOW}[ARMBIAN]${NC} $*"; }

# ── Configuration ─────────────────────────────────────
ARMBIAN_URL="https://github.com/armbian/build"
ARMBIAN_RELEASE="bookworm"
ARMBIAN_BOARD="virt"  # QEMU virtual machine board

ROOTFS_DIR="$BUILD_DIR/armbian-rootfs"
KERNEL_DIR="$BUILD_DIR/kernel"
UEFI_DIR="$BUILD_DIR/uefi"

mkdir -p "$ROOTFS_DIR" "$KERNEL_DIR" "$UEFI_DIR"

# ── Option 1: Manual Instructions ─────────────────────
log "Armbian Bootstrap Assistant"
log "============================"
echo ""
echo "  Armbian uses its own build framework. The recommended approach:"
echo ""
echo "  1. Clone Armbian build:"
echo "     git clone --depth=1 $ARMBIAN_URL $BUILD_DIR/armbian-build"
echo ""
echo "  2. Build minimal image:"
echo "     cd $BUILD_DIR/armbian-build"
echo "     ./compile.sh BOARD=$ARMBIAN_BOARD \\"
echo "       BRANCH=current \\"
echo "       RELEASE=$ARMBIAN_RELEASE \\"
echo "       BUILD_MINIMAL=yes \\"
echo "       BUILD_DESKTOP=no \\"
echo "       KERNEL_CONFIGURE=no"
echo ""
echo "  3. Extract rootfs from output image:"
echo "     LOOP=\$(losetup -fP --show output/images/*.img)"
echo "     mount \${LOOP}p1 /mnt"
echo "     tar czf $ROOTFS_DIR/armbian-rootfs.tar.gz -C /mnt ."
echo "     umount /mnt"
echo ""
echo "  4. Copy kernel:"
echo "     cp output/images/Image $KERNEL_DIR/"
echo "     cp -r output/images/dtb $KERNEL_DIR/"
echo ""
echo "  OR: Download pre-built Armbian for QEMU from archive.armbian.com"
echo ""
echo "  ── Quick Development (No Armbian) ──"
echo "  For testing without full Armbian boot:"
echo "    make qemu  →  boots UOS TV with basic kernel"
echo ""

# ── Option 2: Download QEMU UEFI ──────────────────────
log "Fetching EDK2 UEFI firmware for aarch64..."

UEFI_URLS=(
    "https://github.com/qemu/qemu/raw/master/pc-bios/edk2-aarch64-code.fd"
    "https://retrage.github.io/edk2-nightly/bin/RELEASEAARCH64_QEMU_EFI.fd"
)

for url in "${UEFI_URLS[@]}"; do
    fname=$(basename "$url")
    dest="$UEFI_DIR/$fname"
    if [ -f "$dest" ]; then
        log "UEFI already downloaded: $dest"
        break
    fi
    log "Trying: $url"
    if curl -sL "$url" -o "$dest" 2>/dev/null; then
        log "Downloaded UEFI: $dest"
        # Symlink as canonical name
        ln -sf "$fname" "$UEFI_DIR/QEMU_EFI.fd"
        break
    else
        warn "Failed to download from $url"
    fi
done

# ── Option 3: Create Minimal Rootfs from Scratch ──────
ROOTFS_TARBALL="$ROOTFS_DIR/armbian-rootfs.tar.gz"

if [ ! -f "$ROOTFS_TARBALL" ]; then
    warn "No rootfs tarball found. Creating minimal placeholder..."
    warn "For a real boot, download a proper Armbian rootfs."

    TMP_ROOTFS=$(mktemp -d)

    # Minimal directory structure
    mkdir -p "$TMP_ROOTFS"/{bin,dev,etc,lib,proc,sys,tmp,usr/{bin,lib,share},var/log,data,run}

    # Busybox init (placeholder)
    echo '#!/bin/sh' > "$TMP_ROOTFS/init"
    echo 'echo "UOS TV — Minimal RootFS"' >> "$TMP_ROOTFS/init"
    echo 'mount -t proc proc /proc' >> "$TMP_ROOTFS/init"
    echo 'mount -t sysfs sysfs /sys' >> "$TMP_ROOTFS/init"
    echo 'mount -t devtmpfs devtmpfs /dev' >> "$TMP_ROOTFS/init"
    echo 'mount -t tmpfs tmpfs /tmp' >> "$TMP_ROOTFS/init"
    echo 'mkdir -p /dev/pts /run' >> "$TMP_ROOTFS/init"
    echo 'mount -t devpts devpts /dev/pts' >> "$TMP_ROOTFS/init"
    echo 'mkdir -p /run/uos' >> "$TMP_ROOTFS/init"
    echo 'echo "Starting UOS TV... (minimal)"' >> "$TMP_ROOTFS/init"
    echo 'exec /usr/bin/inis' >> "$TMP_ROOTFS/init"
    chmod +x "$TMP_ROOTFS/init"

    # Compress
    tar czf "$ROOTFS_TARBALL" -C "$TMP_ROOTFS" .
    rm -rf "$TMP_ROOTFS"
    log "Created minimal rootfs: $ROOTFS_TARBALL ($(du -h "$ROOTFS_TARBALL" | cut -f1))"
fi

# ── Summary ───────────────────────────────────────────
log "=== Bootstrap Complete ==="
echo ""
echo "  RootFS:       $ROOTFS_TARBALL"
echo "  Kernel dir:   $KERNEL_DIR/"
echo "  UEFI dir:     $UEFI_DIR/"
echo ""
echo "  Next steps:"
echo "    UOS_ROOTFS=$ROOTFS_TARBALL ./scripts/create-image.sh"
echo "    make qemu"
