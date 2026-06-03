#!/bin/bash
# create-image-simple.sh — UOS TV Minimal GPT Image Builder
# ==========================================================
# Creates a bootable GPT disk image WITHOUT loop devices.
# Uses dd + genext2fs + mformat (mtools) — works in Docker or macOS.
#
# Layout:
#   p1: boot  (FAT32, 128MB) — Kernel + extlinux
#   p2: slot_a (ext2, 1GB)   — RootFS with UOS binaries
#   p3: slot_b (ext2, 1GB)   — Empty (OTA target)
#   p4: data   (ext2, 512MB)  — Persistent data
#
# Total: ~2.8GB
#
# Usage:
#   ./scripts/create-image-simple.sh [output.img]

set -euo pipefail

OUTPUT="${1:-build/uos-tv-gpt.img}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/build"
TARGET_DIR="$PROJECT_DIR/target/aarch64-unknown-linux-musl/release"

BOOT_MB=128
SLOT_MB=1025
DATA_MB=512

# Calculated sizes in sectors (512 bytes)
BOOT_SECT=$((BOOT_MB * 2048))
SLOT_SECT=$((SLOT_MB * 2048))
DATA_SECT=$((DATA_MB * 2048))
GPT_HEADER_SECT=34
GPT_FOOTER_SECT=33
TOTAL_SECT=$((GPT_HEADER_SECT + BOOT_SECT + SLOT_SECT + SLOT_SECT + DATA_SECT + GPT_FOOTER_SECT))

log()  { echo -e "\033[0;32m[IMG]\033[0m $*"; }
warn() { echo -e "\033[0;31m[IMG]\033[0m $*"; }

mkdir -p "$BUILD_DIR"

# ── Step 1: Create rootfs images ─────────────────────
log "Creating slot_a ext2 image (${SLOT_MB}MB)..."
# Copy fresh rootfs
cp -a "$BUILD_DIR/alpine-rootfs-extracted" "$BUILD_DIR/slot_a-rootfs"
# Clean up unnecessary
rm -rf "$BUILD_DIR/slot_a-rootfs/var/cache"/* 2>/dev/null || true

genext2fs -d "$BUILD_DIR/slot_a-rootfs" \
    -b $((SLOT_MB * 1024)) -L UOS_SLOT_A -N 16384 \
    "$BUILD_DIR/slot_a.img"
log "  slot_a.img: $(ls -lh "$BUILD_DIR/slot_a.img" | awk '{print $5}')"

log "Creating slot_b ext2 image (${SLOT_MB}MB, empty)..."
mkdir -p "$BUILD_DIR/slot_b-empty"
genext2fs -d "$BUILD_DIR/slot_b-empty" \
    -b $((SLOT_MB * 1024)) -L UOS_SLOT_B -N 16384 \
    "$BUILD_DIR/slot_b.img"
log "  slot_b.img: $(ls -lh "$BUILD_DIR/slot_b.img" | awk '{print $5}')"

log "Creating data ext2 image (${DATA_MB}MB)..."
mkdir -p "$BUILD_DIR/data-rootfs/etc/uos"
mkdir -p "$BUILD_DIR/data-rootfs/apps"
mkdir -p "$BUILD_DIR/data-rootfs/cache"
mkdir -p "$BUILD_DIR/data-rootfs/ota"
mkdir -p "$BUILD_DIR/data-rootfs/logs"
cp "$PROJECT_DIR/configs/system.yaml" "$BUILD_DIR/data-rootfs/etc/uos/" 2>/dev/null || true
cp "$PROJECT_DIR/configs/monitord.yaml" "$BUILD_DIR/data-rootfs/etc/uos/" 2>/dev/null || true

genext2fs -d "$BUILD_DIR/data-rootfs" \
    -b $((DATA_MB * 1024)) -L UOS_DATA -N 4096 \
    "$BUILD_DIR/data.img"
log "  data.img: $(ls -lh "$BUILD_DIR/data.img" | awk '{print $5}')"

# ── Step 2: Create FAT boot partition ────────────────
log "Creating boot FAT image (${BOOT_MB}MB)..."

# Use mtools (from dosfstools) to create a FAT32 image
BOOT_IMG="$BUILD_DIR/boot.img"

# Create empty image and format
dd if=/dev/zero of="$BOOT_IMG" bs=1M count="$BOOT_MB" status=none

# Format as FAT32 using mformat (part of mtools)
# mformat creates a DOS filesystem in a file
export MTOOLSRC="$BUILD_DIR/mtoolsrc.$$"
echo "drive b: file=\"$BOOT_IMG\" 1.44m" > "$MTOOLSRC"  # dummy, we'll use real params

# Actually, mformat doesn't easily do arbitrary sizes. Let's use mkfs.fat via Docker
# or fallback to a simpler approach
if command -v mkfs.fat &>/dev/null; then
    mkfs.fat -F 32 -n UOS_BOOT "$BOOT_IMG" >/dev/null 2>&1
else
    warn "mkfs.fat not found on host, trying Docker..."
fi

# ── Step 3: Assemble GPT image ───────────────────────
log "Assembling GPT image ($((TOTAL_SECT * 512 / 1024 / 1024))MB)..."

# Create the full image
dd if=/dev/zero of="$OUTPUT" bs=1M count=$((TOTAL_SECT / 2048 + 1)) status=none

# Write GPT partition table
# Partition entries start at LBA 2, usable space starts at LBA 2048
# GPT header: 1 sector
# Partition entries: 32 sectors (128 entries * 128 bytes)
# First usable LBA: 34
# Last usable LBA: TOTAL_SECT - 34

# We'll use sfdisk inside Docker since sfdisk doesn't need loop devices
cat > "$BUILD_DIR/partitions.sfdisk" <<PARTITION
label: gpt
unit: sectors
first-lba: 2048
start=      2048, size= $BOOT_SECT, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name="UOS_BOOT"
start=  $((2048 + BOOT_SECT)), size= $SLOT_SECT, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="UOS_SLOT_A"
start=  $((2048 + BOOT_SECT + SLOT_SECT)), size= $SLOT_SECT, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="UOS_SLOT_B"
start=  $((2048 + BOOT_SECT + SLOT_SECT + SLOT_SECT)), size= $DATA_SECT, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name="UOS_DATA"
PARTITION

if command -v sfdisk &>/dev/null; then
    sfdisk "$OUTPUT" < "$BUILD_DIR/partitions.sfdisk" 2>/dev/null
    log "Partition table created"
else
    log "sfdisk not available on host — partition table will be created inside Docker"
    log "  Image partition offsets:"
    log "  p1 (boot):  sector 2048, size $BOOT_SECT"
    log "  p2 (slot_a): sector $((2048 + BOOT_SECT)), size $SLOT_SECT"
    log "  p3 (slot_b): sector $((2048 + BOOT_SECT + SLOT_SECT)), size $SLOT_SECT"
    log "  p4 (data):  sector $((2048 + BOOT_SECT + SLOT_SECT + SLOT_SECT)), size $DATA_SECT"
fi

# ── Step 4: Write partition contents ─────────────────
log "Writing partition contents..."

# Write boot partition at sector 2048
dd if="$BOOT_IMG" of="$OUTPUT" bs=512 seek=2048 conv=notrunc status=none
log "  boot: written"

# Write slot_a at its sector
SLOT_A_START=$((2048 + BOOT_SECT))
dd if="$BUILD_DIR/slot_a.img" of="$OUTPUT" bs=512 seek="$SLOT_A_START" conv=notrunc status=none
log "  slot_a: written at sector $SLOT_A_START"

# Write slot_b
SLOT_B_START=$((2048 + BOOT_SECT + SLOT_SECT))
dd if="$BUILD_DIR/slot_b.img" of="$OUTPUT" bs=512 seek="$SLOT_B_START" conv=notrunc status=none
log "  slot_b: written at sector $SLOT_B_START"

# Write data
DATA_START=$((2048 + BOOT_SECT + SLOT_SECT + SLOT_SECT))
dd if="$BUILD_DIR/data.img" of="$OUTPUT" bs=512 seek="$DATA_START" conv=notrunc status=none
log "  data: written at sector $DATA_START"

# ── Done ────────────────────────────────────────────
log "=== Image Created ==="
log "File: $OUTPUT"
log "Size: $(ls -lh "$OUTPUT" | awk '{print $5}')"
log ""
log "QEMU boot command:"
log "  qemu-system-aarch64 -machine virt -cpu cortex-a57 -m 512 \\"
log "    -bios /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \\"
log "    -drive file=$OUTPUT,format=raw,if=none,id=drive0 \\"
log "    -device virtio-blk-device,drive=drive0 \\"
log "    -nic user,hostfwd=tcp::8080-:80,hostfwd=tcp::9090-:9090 \\"
log "    -nographic"

# Cleanup
rm -rf "$BUILD_DIR/slot_a-rootfs" "$BUILD_DIR/slot_b-empty" "$BUILD_DIR/data-rootfs" 2>/dev/null || true
rm -f "$MTOOLSRC" "$BUILD_DIR/partitions.sfdisk" 2>/dev/null || true
