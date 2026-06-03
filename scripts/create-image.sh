#!/bin/bash
# create-image.sh — UOS TV Disk Image Builder
# ============================================
# Creates a bootable GPT disk image with:
#   p1: boot  (FAT32, 256MB)   — U-Boot / UEFI + Kernel + DTB
#   p2: slot_a (ext4, 2.5GB)   — RootFS A (active)
#   p3: slot_b (ext4, 2.5GB)   — RootFS B (OTA standby)
#   p4: data   (ext4, 2GB)     — Persistent user data (/data)
#   p5: recovery (ext4, 1GB)   — Recovery rootfs
#
# Usage:
#   ./scripts/create-image.sh [output.img] [size_mb]
#
# Env:
#   UOS_ROOTFS=path   — Pre-built rootfs tarball (Armbian base)
#   SKIP_ROOTFS=1     — Don't populate rootfs, just partition
#   CROSS_TARGET=     — Rust cross-compilation target (default: aarch64-unknown-linux-musl)

set -euo pipefail

OUTPUT="${1:-build/uos-tv.img}"
SIZE_MB="${2:-8192}"
CROSS_TARGET="${CROSS_TARGET:-aarch64-unknown-linux-musl}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

RED='\033[0;31m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'; NC='\033[0m'

log()  { echo -e "${GREEN}[IMG]${NC} $*"; }
warn() { echo -e "${RED}[IMG]${NC} $*"; }

# ── Prerequisites ─────────────────────────────────────
check_tool() {
    command -v "$1" >/dev/null 2>&1 || { warn "Missing: $1"; return 1; }
}

REQUIRED_TOOLS=(dd sfdisk mkfs.vfat mkfs.ext4)
for t in "${REQUIRED_TOOLS[@]}"; do check_tool "$t" || exit 1; done

# ── Create Image ──────────────────────────────────────
# Set UOS_ROOTFS to auto-detected path if not specified
if [ -z "${UOS_ROOTFS:-}" ]; then
    DETECTED="$PROJECT_DIR/build/armbian-rootfs/armbian-rootfs.tar.gz"
    if [ -f "$DETECTED" ]; then
        UOS_ROOTFS="$DETECTED"
        log "Auto-detected rootfs: $UOS_ROOTFS"
    fi
fi

log "Creating UOS TV disk image: $OUTPUT (${SIZE_MB}MB)"

# Create sparse file
dd if=/dev/zero of="$OUTPUT" bs=1M count=0 seek="$SIZE_MB" status=none

# GPT partition layout
#   Start     Size (sectors)  Name
#   2048      524288          boot     (256 MB)
#   526336    5242880         slot_a   (2.5 GB)
#   5769216   5242880         slot_b   (2.5 GB)
#   11012096  4194304         data     (2 GB)
#   15206400  (fill)          recovery (≈1 GB)

sfdisk "$OUTPUT" <<PARTITION
label: gpt
unit: sectors
start=     2048, size=  524288, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name="boot"
start=   526336, size= 5242880, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="slot_a"
start=  5769216, size= 5242880, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="slot_b"
start= 11012096, size= 4194304, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="data"
start= 15206400,               type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="recovery"
PARTITION

log "Partition table:"
sfdisk -l "$OUTPUT" | grep -E "^(Device|$OUTPUT)" || true

# ── Setup Loopback ────────────────────────────────────
LOOP=$(losetup -fP --show "$OUTPUT")
log "Loop device: $LOOP"

cleanup() {
    log "Cleaning up..."
    for mp in "$BOOT_MNT" "$SLOT_MNT" "$DATA_MNT"; do
        mountpoint -q "${mp:-/nonexistent}" 2>/dev/null && umount "$mp" || true
        [ -d "${mp:-}" ] && rmdir "$mp" 2>/dev/null || true
    done
    losetup -d "$LOOP" 2>/dev/null || true
}
trap cleanup EXIT

# ── Format Partitions ─────────────────────────────────
log "Formatting partitions..."

mkfs.vfat -F 32 -n UOS_BOOT    "${LOOP}p1" >/dev/null 2>&1
mkfs.ext4 -L slot_a            "${LOOP}p2" >/dev/null 2>&1
mkfs.ext4 -L slot_b            "${LOOP}p3" >/dev/null 2>&1
mkfs.ext4 -L uos_data          "${LOOP}p4" >/dev/null 2>&1
mkfs.ext4 -L uos_recovery      "${LOOP}p5" >/dev/null 2>&1

# ── Mount ─────────────────────────────────────────────
BOOT_MNT=$(mktemp -d); SLOT_MNT=$(mktemp -d); DATA_MNT=$(mktemp -d)

mount "${LOOP}p1" "$BOOT_MNT"
mount "${LOOP}p2" "$SLOT_MNT"
mount "${LOOP}p4" "$DATA_MNT"

# ── Boot Partition ────────────────────────────────────
log "Populating boot partition..."

mkdir -p "$BOOT_MNT/extlinux"

cat > "$BOOT_MNT/extlinux/extlinux.conf" <<'EOF'
TIMEOUT 30
DEFAULT uos-slot-a

LABEL uos-slot-a
    KERNEL /Image
    FDTDIR /dtb
    APPEND root=/dev/vda2 ro rootwait console=ttyAMA0,115200 quiet loglevel=3 uos.slot=a

LABEL uos-slot-b
    KERNEL /Image
    FDTDIR /dtb
    APPEND root=/dev/vda3 ro rootwait console=ttyAMA0,115200 quiet loglevel=3 uos.slot=b

LABEL uos-recovery
    KERNEL /Image
    FDTDIR /dtb
    APPEND root=/dev/vda5 ro rootwait console=ttyAMA0,115200 uos.recovery
EOF

# Copy kernel + DTB from Armbian build (if available)
KERNEL_SRC="$PROJECT_DIR/build/kernel"
if [ -f "$KERNEL_SRC/Image" ]; then
    log "Copying kernel Image..."
    cp "$KERNEL_SRC/Image" "$BOOT_MNT/"
fi
if [ -d "$KERNEL_SRC/dtb" ]; then
    log "Copying device tree..."
    cp -r "$KERNEL_SRC/dtb" "$BOOT_MNT/"
fi

# Placeholder note
if [ ! -f "$BOOT_MNT/Image" ]; then
    warn "Kernel Image not found at $KERNEL_SRC/Image"
    warn "Build Armbian kernel first or place Image + dtb/ in $KERNEL_SRC/"
fi

# ── RootFS (Slot A) ───────────────────────────────────
log "Populating rootfs (slot A)..."

# Extract Armbian rootfs tarball if available
if [ -n "${UOS_ROOTFS:-}" ] && [ -f "$UOS_ROOTFS" ]; then
    log "Extracting rootfs from $UOS_ROOTFS..."
    tar xf "$UOS_ROOTFS" -C "$SLOT_MNT"
else
    warn "No rootfs tarball provided (set UOS_ROOTFS env)"
fi

# Essential directory structure
mkdir -p "$SLOT_MNT"/{usr/bin,usr/lib,usr/share/uos,etc,dev,proc,sys,run,var/log,tmp}
mkdir -p "$SLOT_MNT/usr/share/uos/services.d"
mkdir -p "$SLOT_MNT/usr/share/uos/luna"  # Luna UI shell files
chmod 1777 "$SLOT_MNT/tmp"

# Copy Rust binaries
RELEASE_DIR="$PROJECT_DIR/target/$CROSS_TARGET/release"
DEBUG_DIR="$PROJECT_DIR/target/debug"
# Fallback to debug if no release
if [ ! -d "$RELEASE_DIR" ] && [ -d "$DEBUG_DIR" ]; then
    RELEASE_DIR="$DEBUG_DIR"
    warn "No release build — using debug binaries"
fi
if [ -d "$RELEASE_DIR" ]; then
    log "Copying UOS binaries from $RELEASE_DIR..."

    BINARIES=(
        inis monitord logd stardustd
        otad netmd audiod pkgd inputd
        dispald notifd powermand devmand
    )

    for bin in "${BINARIES[@]}"; do
        if [ -f "$RELEASE_DIR/$bin" ]; then
            cp "$RELEASE_DIR/$bin" "$SLOT_MNT/usr/bin/"
            chmod 755 "$SLOT_MNT/usr/bin/$bin"
            log "  ✓ $bin"
        else
            warn "  ✗ $bin (not found)"
        fi
    done
else
    warn "No Rust binaries at $RELEASE_DIR"
    warn "Run: cargo build --release --target $CROSS_TARGET"
fi

# Copy Luna UI
LUNA_SRC="$PROJECT_DIR/luna"
if [ -d "$LUNA_SRC" ]; then
    log "Copying Luna UI shell..."
    cp -r "$LUNA_SRC"/* "$SLOT_MNT/usr/share/uos/luna/"
fi

# Copy service manifests
SERVICE_MANIFESTS="$PROJECT_DIR/configs/services.d"
if [ -d "$SERVICE_MANIFESTS" ]; then
    cp "$SERVICE_MANIFESTS"/*.yaml "$SLOT_MNT/usr/share/uos/services.d/" 2>/dev/null || true
fi

# Create monitord config
cat > "$SLOT_MNT/usr/share/uos/monitord.yaml" <<EOF
# UOS TV — Service Supervisor Configuration
services_dir: /usr/share/uos/services.d
log_dir: /var/log/uos
binary_search_path:
  - /usr/bin
  - /usr/local/bin
startup_timeout_sec: 30
crash_window_sec: 60
max_crashes: 5
EOF

# Create /etc symlink → /data/etc (persistent across OTA)
rm -rf "$SLOT_MNT/etc"
ln -s /data/etc "$SLOT_MNT/etc"

# /etc/resolv.conf placeholder
mkdir -p "$SLOT_MNT/run/resolvconf"
echo "nameserver 8.8.8.8" > "$SLOT_MNT/run/resolvconf/resolv.conf"

# ── Data Partition ────────────────────────────────────
log "Initializing data partition..."
mkdir -p "$DATA_MNT"/{etc,apps,downloads,logs,config}

# Default etc configs
cat > "$DATA_MNT/etc/hostname" <<EOF
uos-tv
EOF
cat > "$DATA_MNT/etc/hosts" <<EOF
127.0.0.1 localhost
127.0.1.1 uos-tv
EOF

# ── Stats ────────────────────────────────────────────
log "=== Image Summary ==="
echo "  Boot:     $(du -sh "$BOOT_MNT" 2>/dev/null | cut -f1)"
echo "  Slot A:   $(du -sh "$SLOT_MNT" 2>/dev/null | cut -f1)"
echo "  Data:     $(du -sh "$DATA_MNT" 2>/dev/null | cut -f1)"
echo "  Image:    $(ls -lh "$OUTPUT" | awk '{print $5}')"

log "Image created: $OUTPUT"
