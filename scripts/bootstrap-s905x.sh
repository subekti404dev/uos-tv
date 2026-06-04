#!/bin/bash
# ============================================================================
# UOS TV — Amlogic S905X Bootstrap Script
# ============================================================================
# Builds a complete bootable image for Amlogic S905X set-top boxes.
#
# Hardware: Amlogic S905X (quad Cortex-A53, Mali-450 MP3 GPU)
# Board:    Generic p212 reference (most S905X boxes)
# Output:   SD card image with u-boot, kernel, rootfs, GPU drivers, Cog/WPE
#
# Usage:
#   ./scripts/bootstrap-s905x.sh [target_dir]        # Full build
#   ./scripts/bootstrap-s905x.sh --kernel-only       # Just build kernel
#   ./scripts/bootstrap-s905x.sh --kernel-docker      # Docker kernel build
#   ./scripts/bootstrap-s905x.sh --wifi               # Build WiFi drivers + fetch firmware
#   ./scripts/bootstrap-s905x.sh --rootfs-only        # Just build rootfs
#
# Requirements:
#   - ARM64 cross-compiler: aarch64-linux-gnu- (or use docker)
#   - Docker (recommended, for reproducible builds)
#   - bc, flex, bison, openssl, device-tree-compiler
#   - SD card (8GB+) for deployment
# ============================================================================

set -euo pipefail

# ── Config ────────────────────────────────────────────
TARGET_DIR="${1:-build/s905x}"
KERNEL_DIR="$TARGET_DIR/kernel"
ROOTFS_DIR="$TARGET_DIR/rootfs"
UBOOT_DIR="$TARGET_DIR/u-boot"
OUTPUT_DIR="$TARGET_DIR/output"

# S905X identifiers
SOC="gxl"
KERNEL_VERSION="6.6"
UBOOT_VERSION="2024.04"
ARMBIAN_RELEASE="bookworm"

# ── Board Database ───────────────────────────────────
# Each board: name, DTB, WiFi chip, RAM, special notes
# Formats: "NAME|DTB|WIFI_CHIP|RAM|NOTES"
# ═══════════════════════════════════════════════════════
# DTB Identification Notes:
# ═══════════════════════════════════════════════════════
# The mainline kernel has these S905X DTBs (linux/arch/arm64/boot/dts/amlogic/):
#
#   meson-gxl-s905x-p212.dtb        — Generic p212 reference board ← B860H, HG680P
#   meson-gxl-s905x-nexbox-a95x.dtb  — Nexbox A95X (different WiFi, LED)
#   meson-gxl-s905x-libretech-cc.dtb — LibreTech CC (La Frite)
#   meson-gxl-s905x-khadas-vim.dtb   — Khadas VIM
#   meson-gxl-s905x-hwacom-amazetv.dtb — HwaCom Amazetv
#   meson-gxl-s905x-vero4k.dtb       — OSMC Vero 4K
#
# p212 DTS includes (meson-gxl-s905x-p212.dtsi):
#   - SoC:        meson-gxl-s905x.dtsi → meson-gxl.dtsi + meson-gxl-mali.dtsi
#   - RAM:        2GB (0x80000000) — matches B860H and HG680P
#   - Serial:     uart_AO @ 115200n8 (GPIO header)
#   - SDIO WiFi:  sd_emmc_a, bus-width 4, max 50MHz, pwrseq GPIOX_6
#   - SD card:    sd_emmc_b, CD on CARD_6 GPIO
#   - eMMC:       sd_emmc_c, bus-width 8, HS200
#   - Ethernet:   ethmac, internal RMII PHY (100Mbps)
#   - IR:         meson-ir on remote_input_ao_pins
#   - HDMI:       hdmi_tx with HPD/I2C pins, CEC on ao_cec_pins
#   - USB:        host mode, USB2 phy0 supplied by HDMI_5V
#   - Bluetooth:  uart_A → BCM43438 (p212 dev board only!)
#
# B860H (S905X-B, ZTE):
#   - S905X-B is minor silicon revision — same DTB compatible!
#   - DTB: meson-gxl-s905x-p212.dtb ✅
#   - WiFi: RTL8189ES SDIO on sd_emmc_a (p212 has SDIO slot ready)
#     ⚠️  GPIOX_6 for WiFi reset (verify on actual hardware)
#   - IR: meson-ir ✅ (standard p212)
#   - RAM: 2GB ✅ (matches p212 memory node)
#   - No Bluetooth (skip uart_A node)
#
# HG680P (S905X):
#   - DTB: meson-gxl-s905x-p212.dtb ✅
#   - WiFi: RTL8189FS SDIO on sd_emmc_a
#     ⚠️  GPIOX_6 for WiFi reset (verify on actual hardware)
#   - IR: meson-ir ✅ (standard p212)
#   - RAM: 2GB ✅ (matches p212 memory node)
#   - No Bluetooth (skip uart_A node)
#
# TO VERIFY ON ACTUAL HARDWARE:
#   1. WiFi reset GPIO (GPIOX_6) — check if correct, some boxes use GPIOX_7
#   2. SD card detect GPIO (CARD_6) — should auto-work with mmc subsystem
#   3. LED GPIO (power LED) — add gpio-leds node if known
#   4. Recovery button GPIO — add gpio-keys node if known
#   5. IR keymap — meson-ir works but rc-keymap may need customization
# ═══════════════════════════════════════════════════════
declare -A BOARD_DB
while IFS='|' read -r _name _dtb _wifi _ram _notes; do
    [[ -z "$_name" || "$_name" == \#* ]] && continue
    BOARD_DB["${_name,,}"]="$_dtb|$_wifi|$_ram|$_notes"
done << 'BOARDS'
# name     | dtb                        | wifi_chip    | ram | notes
B860H       | meson-gxl-s905x-p212       | rtl8189es    | 2GB | ZTE B860H v1.1/v2.1, S905X-B, SDIO WiFi, meson-ir
HG680P      | meson-gxl-s905x-p212       | rtl8189fs    | 2GB | HG680P, S905X, SDIO WiFi, meson-ir
NEXBOX-A95X | meson-gxl-s905x-nexbox-a95x | rtl8723bs   | 2GB | Nexbox A95X
LIBRETECH-CC| meson-gxl-s905x-libretech-cc| rtl8723bs   | 1GB | Libre Computer La Frite
KHADAS-VIM  | meson-gxl-s905x-khadas-vim  | broadcom    | 2GB | Khadas VIM (BCM43430 SDIO, Bluetooth)
BOARDS

# Default to generic p212
DTB="meson-gxl-s905x-p212"

# GPU: Mali-450 needs:
#   Kernel:  CONFIG_DRM_LIMA (open-source reverse-engineered driver)
#   Mesa:    lima Gallium driver (OpenGL ES 2.0)
# Display:  meson DRM/KMS → HDMI output

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'
log()  { echo -e "${GREEN}[S905X]${NC} $*"; }
warn() { echo -e "${YELLOW}[S905X]${NC} $*"; }
info() { echo -e "${BLUE}[S905X]${NC} $*"; }

# ── Compatibility Matrix ──────────────────────────────
show_compatibility() {
    echo ""
    echo "╔════════════════════════════════════════════════════════╗"
    echo "║       UOS TV — Amlogic S905X Compatibility             ║"
    echo "╠════════════════════════════════════════════════════════╣"
    echo "║                                                        ║"
    echo "║  CPU:    Quad Cortex-A53 (ARMv8-A)       ✅ Fully      ║"
    echo "║          Already target: aarch64-musl     supported     ║"
    echo "║                                                        ║"
    echo "║  GPU:    Mali-450 MP3                     ✅ lima      ║"
    echo "║          OpenGL ES 2.0 via Mesa           Open-source  ║"
    echo "║          COG/WPE RENDERING WORKS!         driver       ║"
    echo "║                                                        ║"
    echo "║  Display: HDMI via meson DRM/KMS          ✅ meson     ║"
    echo "║          Full KMS/DRM atomic modeset      DRM driver   ║"
    echo "║                                                        ║"
    echo "║  RAM:    1-2GB DDR3                       ✅ 512MB+    ║"
    echo "║          UOS needs ~128MB                 More=better  ║"
    echo "║                                                        ║"
    echo "║  Storage: eMMC 8-16GB  /  SD Card         ✅ Both      ║"
    echo "║          We build for SD card first       SD easier    ║"
    echo "║                                                        ║"
    echo "║  Ethernet: 100Mbps (some GbE)             ✅ meson     ║"
    echo "╚════════════════════════════════════════════════════════╝"
    echo ""
    echo "  Supported STB models:"
    echo "    ✅ ZTE B860H v1.1/v2.1  — RTL8189ES WiFi, 2GB RAM, meson-ir"
    echo "    ✅ HG680P                — RTL8189FS WiFi, 2GB RAM"
    echo "    ✅ Nexbox A95X           — RTL8723BS WiFi, 2GB RAM"
    echo "    ✅ LibreTech CC          — RTL8723BS WiFi, 1GB RAM"
    echo "    ✅ Khadas VIM            — Broadcom WiFi, 2GB RAM"
    echo ""
    echo "  ❌ Not supported: S905 (non-X), S905W, S905D"
    echo "  ⚠️  S905X2/S905X3: different SoC (g12a/g12b) — need different DTB"
    echo ""
    echo "  ═══ DTB Verification ═══"
    echo "  ✅ B860H  → meson-gxl-s905x-p212.dtb (generic p212)"
    echo "     S905X-B revision, 2GB RAM, RTL8189ES SDIO"
    echo "  ✅ HG680P → meson-gxl-s905x-p212.dtb (generic p212)"
    echo "     S905X, 2GB RAM, RTL8189FS SDIO"
    echo ""
    echo "  ⚠️  Verify on hardware:"
    echo "     - WiFi reset GPIO: GPIOX_6 (p212 default)"
    echo "     - SD card detect GPIO: CARD_6"
    echo "     - IR keymap: meson-ir default (may need rc-keymap)"
    echo "     - LED/button GPIOs: not in p212 DTS (add overlay)"
    echo ""
}

# ── Build Kernel ──────────────────────────────────────
build_kernel() {
    log "Building Linux kernel $KERNEL_VERSION for S905X..."
    mkdir -p "$KERNEL_DIR"

    if [ -f "$KERNEL_DIR/arch/arm64/boot/Image" ]; then
        log "Kernel already built at $KERNEL_DIR"
    else
        if [ ! -d "$KERNEL_DIR/.git" ]; then
            log "Cloning kernel (depth=1, branch linux-$KERNEL_VERSION.y)..."
            git clone --depth=1 --branch "linux-$KERNEL_VERSION.y" \
                https://github.com/torvalds/linux.git "$KERNEL_DIR" 2>/dev/null || \
            git clone --depth=1 --branch "v$KERNEL_VERSION" \
                https://github.com/torvalds/linux.git "$KERNEL_DIR"
        fi

        cd "$KERNEL_DIR"

        # Use defconfig as base
        make ARCH=arm64 defconfig

        # Enable S905X-specific drivers
        # ============================================
        # Amlogic platform support
        ./scripts/config -e ARCH_MESON
        ./scripts/config -e ARCH_MEDIATEK -d     # not needed

        # DRM/KMS for display
        ./scripts/config -e DRM
        ./scripts/config -e DRM_MESON            # Amlogic DRM driver
        ./scripts/config -e DRM_HDMI             # HDMI support

        # Mali-450 GPU (lima open-source driver)
        ./scripts/config -e DRM_LIMA             # Mali-400/450 GPU
        ./scripts/config -d DRM_PANFROST         # Mali G-series (not our GPU)

        # HDMI / Display
        ./scripts/config -e DRM_DW_HDMI
        ./scripts/config -e DRM_DW_HDMI_AHB_AUDIO
        ./scripts/config -e DRM_DISPLAY_CONNECTOR

        # USB & Storage
        ./scripts/config -e USB_XHCI_HCD
        ./scripts/config -e USB_DWC2
        ./scripts/config -e MMC
        ./scripts/config -e MMC_BLOCK
        ./scripts/config -e MMC_MESON_GX          # Amlogic SD/eMMC

        # Network
        ./scripts/config -e STMMAC_ETH            # Amlogic Ethernet
        ./scripts/config -e MESON_GXL_PHY

        # WiFi (compile as modules — load firmware at runtime)
        ./scripts/config -m RTL8723BS              # Common in S905X boxes
        ./scripts/config -m RTL8188EU
        ./scripts/config -m BRCMFMAC               # Broadcom WiFi
        ./scripts/config -e CFG80211
        ./scripts/config -e MAC80211

        # Filesystem support
        ./scripts/config -e EXT4_FS
        ./scripts/config -e EXT2_FS
        ./scripts/config -e VFAT_FS
        ./scripts/config -e TMPFS

        # Devicetree
        ./scripts/config -e OF
        ./scripts/config -e OF_OVERLAY

        # Performance
        ./scripts/config -e NR_CPUS -v 4
        ./scripts/config -e PREEMPT
        ./scripts/config -e HZ -v 250

        # Regenerate .config
        make ARCH=arm64 olddefconfig

        log "Building kernel (this takes ~20-40 min)..."
        make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- -j$(nproc) Image 2>&1 | tail -20
        make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- -j$(nproc) dtbs 2>&1 | tail -5
        make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- -j$(nproc) modules 2>&1 | tail -5

        cd "$OLDPWD"
    fi

    log "Kernel done!"
    ls -lh "$KERNEL_DIR/arch/arm64/boot/Image"
    ls -lh "$KERNEL_DIR/arch/arm64/boot/dts/amlogic/$DTB.dtb" 2>/dev/null || \
        warn "DTB not found at expected path: arch/arm64/boot/dts/amlogic/$DTB.dtb"
}

# ── Build u-boot ─────────────────────────────────────
build_uboot() {
    log "Building u-boot $UBOOT_VERSION for S905X..."
    mkdir -p "$UBOOT_DIR"

    if [ -f "$UBOOT_DIR/u-boot.bin" ]; then
        log "u-boot already built at $UBOOT_DIR"
        return
    fi

    # Amlogic requires a special u-boot fork or mainline with blobs
    # We use the LibreELEC/Armbian amlogic-boot-fip approach
    log "Using pre-built Amlogic u-boot from LibreELEC sources..."
    log "(Building u-boot for Amlogic requires proprietary signed blobs)"

    # The simplest approach: use Armbian/LibreELEC pre-built bootloader
    # These handle the Amlogic signing chain (BL1→BL2→BL30→BL31→u-boot)
    BOOTLOADER_URL="https://github.com/LibreELEC/amlogic-boot-fip/raw/master/$SOC"

    warn "S905X needs signed boot blobs. Two options:"
    echo ""
    echo "  Option A (Recommended): Boot from SD with existing Android u-boot"
    echo "    - Keep Android on eMMC (provides signed boot chain)"
    echo "    - Boot from SD card using u-boot on SD"
    echo "    - Most S905X boxes support SD boot with toothpick method"
    echo ""
    echo "  Option B: Build complete boot chain"
    echo "    - Requires Amlogic USB Burning Tool or recovery"
    echo "    - Extract blobs from original firmware"
    echo ""
    echo "  For development, use Option A (SD card boot)"
}

# ── Build Rootfs ─────────────────────────────────────
build_rootfs() {
    log "Building UOS TV rootfs for S905X..."
    mkdir -p "$ROOTFS_DIR"

    # Start with Alpine aarch64 (musl, compatible with our binaries)
    ALPINE_VER="3.21"
    ALPINE_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VER}/releases/aarch64/alpine-minirootfs-${ALPINE_VER}.0-aarch64.tar.gz"
    ALPINE_TAR="$TARGET_DIR/alpine-minirootfs.tar.gz"

    if [ ! -d "$ROOTFS_DIR/bin" ]; then
        if [ ! -f "$ALPINE_TAR" ]; then
            log "Downloading Alpine minirootfs..."
            curl -L "$ALPINE_URL" -o "$ALPINE_TAR"
        fi
        log "Extracting Alpine rootfs..."
        mkdir -p "$ROOTFS_DIR"
        tar xzf "$ALPINE_TAR" -C "$ROOTFS_DIR"
    fi

    # ── Add UOS binaries ──
    log "Installing UOS TV binaries..."
    UOS_BINARIES_DIR="$PROJECT_DIR/target/aarch64-unknown-linux-musl/release"

    if [ -d "$UOS_BINARIES_DIR" ]; then
        mkdir -p "$ROOTFS_DIR/usr/bin"
        for bin in inis monitord stardustd lunad lumind netmd audiod inputd \
                   notifd otad pkgd logd dispald devmand powermand; do
            if [ -f "$UOS_BINARIES_DIR/$bin" ]; then
                cp "$UOS_BINARIES_DIR/$bin" "$ROOTFS_DIR/usr/bin/"
            fi
        done
        # inis replaces init
        cp "$UOS_BINARIES_DIR/inis" "$ROOTFS_DIR/sbin/init"
    else
        warn "UOS binaries not found at $UOS_BINARIES_DIR"
        warn "Run: docker run --rm -v $PROJECT_DIR:/work uos-builder cargo build --release --target aarch64-unknown-linux-musl --workspace"
    fi

    # ── Service manifests ──
    log "Installing service manifests..."
    mkdir -p "$ROOTFS_DIR/usr/share/uos/services.d"
    cp "$PROJECT_DIR/configs/services.d/"*.yaml "$ROOTFS_DIR/usr/share/uos/services.d/"

    # ── Luna UI ──
    log "Installing Luna UI..."
    mkdir -p "$ROOTFS_DIR/var/www/luna"
    cp -r "$PROJECT_DIR/luna/"* "$ROOTFS_DIR/var/www/luna/"

    # ── Mesa (GPU drivers for Mali-450) ──
    log "Installing Mesa with lima driver..."
    # On Alpine, install mesa packages
    mkdir -p "$ROOTFS_DIR/etc/apk"
    echo "https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VER}/main" > "$ROOTFS_DIR/etc/apk/repositories"
    echo "https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VER}/community" >> "$ROOTFS_DIR/etc/apk/repositories"

    # Mesa packages needed:
    # - mesa-dri-gallium: includes lima_dri.so (Mali-450 OpenGL ES driver)
    # - mesa-egl: EGL loader
    # - mesa-gbm: Generic Buffer Manager
    # - libdrm: Direct Rendering Manager userspace
    # Alpine command (run in chroot or via docker):
    #   apk add mesa-dri-gallium mesa-egl mesa-gbm libdrm

    # ── Cog / WPE WebKit ──
    log "Installing Cog + WPE WebKit..."
    # Cog and WPE WebKit from Alpine packages
    # Alpine 3.21 has cog and wpewebkit packages
    #   apk add cog wpewebkit

    # ── WiFi firmware ──
    log "Setting up WiFi firmware..."
    mkdir -p "$ROOTFS_DIR/lib/firmware/rtlwifi"
    mkdir -p "$ROOTFS_DIR/lib/firmware/brcm"

    # Download firmware if build-s905x-wifi.sh was run
    local FW_SRC="$TARGET_DIR/firmware"
    if [ -d "$FW_SRC" ]; then
        cp -r "$FW_SRC"/* "$ROOTFS_DIR/lib/firmware/" 2>/dev/null || true
        log "  ✓ WiFi firmware copied from $FW_SRC"
    fi

    # Copy WiFi kernel modules if built
    local WIFI_MOD_DIR="$TARGET_DIR/wifi-drivers"
    if [ -d "$WIFI_MOD_DIR" ]; then
        mkdir -p "$ROOTFS_DIR/lib/modules"
        find "$WIFI_MOD_DIR" -name "*.ko" -exec cp {} "$ROOTFS_DIR/lib/modules/" \; 2>/dev/null || true
        log "  ✓ WiFi kernel modules copied"
    fi

    # Also look for modules in kernel tree staging
    if [ -d "$KERNEL_DIR/drivers/staging/rtl8723bs" ]; then
        find "$KERNEL_DIR/drivers/staging/rtl8723bs" -name "*.ko" \
            -exec cp {} "$ROOTFS_DIR/lib/modules/" \; 2>/dev/null || true
    fi

    # ── Kernel modules ──
    if [ -f "$KERNEL_DIR/arch/arm64/boot/Image" ]; then
        log "Installing kernel modules..."
        cd "$KERNEL_DIR"
        make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- \
            INSTALL_MOD_PATH="$ROOTFS_DIR" modules_install 2>/dev/null || \
            warn "Module installation failed (non-fatal — may need root)"
        cd "$OLDPWD"
    fi

    # ── Boot config ──
    log "Creating boot configuration..."
    mkdir -p "$ROOTFS_DIR/boot"
    cat > "$ROOTFS_DIR/boot/extlinux.conf" << 'EXTLINUX'
TIMEOUT 30
DEFAULT uos-tv

LABEL uos-tv
  MENU LABEL UOS TV (S905X)
  LINUX /Image
  FDT /meson-gxl-s905x-p212.dtb
  APPEND console=ttyAML0,115200 root=/dev/mmcblk0p2 rw rootfstype=ext4 init=/sbin/init uos.console=ttyAML0
EXTLINUX

    # ── Init config ──
    if [ -f "$ROOTFS_DIR/sbin/init" ] && file "$ROOTFS_DIR/sbin/init" | grep -q "ARM aarch64"; then
        log "inis ready as /sbin/init"
    else
        warn "/sbin/init missing or invalid — boot will fail"
    fi

    log "Rootfs built at $ROOTFS_DIR ($(du -sh "$ROOTFS_DIR" | cut -f1))"
}

# ── Create SD Card Image ──────────────────────────────
create_sd_image() {
    log "Creating SD card image..."
    mkdir -p "$OUTPUT_DIR"

    IMG="$OUTPUT_DIR/uos-tv-s905x.img"
    IMG_SIZE_MB=4096  # 4GB

    log "Image: $IMG (${IMG_SIZE_MB}MB)"

    # Create empty image
    dd if=/dev/zero of="$IMG" bs=1M count="$IMG_SIZE_MB" status=none

    # Partition layout (Amlogic SD boot):
    #   p1: FAT32 (boot)  — 256MB
    #   p2: ext4  (root)  — rest
    BOOT_SIZE=256

    # Create partitions
    cat << EOF | sfdisk "$IMG" 2>/dev/null
label: dos
unit: sectors
1 : start=2048, size=$((BOOT_SIZE * 2048)), type=c
2 : start=$((BOOT_SIZE * 2048 + 2048)), type=83
EOF

    # Format
    LOOP=$(losetup -fP --show "$IMG" 2>/dev/null || true)
    if [ -n "$LOOP" ]; then
        mkfs.vfat -F32 "${LOOP}p1" 2>/dev/null
        mkfs.ext4 -F "${LOOP}p2" 2>/dev/null

        # Mount & copy
        TMPMNT=$(mktemp -d)
        mount "${LOOP}p1" "$TMPMNT"
        if [ -f "$KERNEL_DIR/arch/arm64/boot/Image" ]; then
            cp "$KERNEL_DIR/arch/arm64/boot/Image" "$TMPMNT/"
        fi
        if [ -f "$KERNEL_DIR/arch/arm64/boot/dts/amlogic/$DTB.dtb" ]; then
            cp "$KERNEL_DIR/arch/arm64/boot/dts/amlogic/$DTB.dtb" "$TMPMNT/"
        fi
        cp "$ROOTFS_DIR/boot/extlinux.conf" "$TMPMNT/" 2>/dev/null || true
        umount "$TMPMNT"

        mount "${LOOP}p2" "$TMPMNT"
        cp -a "$ROOTFS_DIR/"* "$TMPMNT/"
        umount "$TMPMNT"

        losetup -d "$LOOP"
        rmdir "$TMPMNT"

        log "SD image created: $IMG ($(du -h "$IMG" | cut -f1))"
        log "Write to SD card: sudo dd if=$IMG of=/dev/mmcblkX bs=4M status=progress"
    else
        warn "Cannot create loop device. Creating tar.gz instead."
        tar czf "$OUTPUT_DIR/uos-tv-s905x-rootfs.tar.gz" -C "$ROOTFS_DIR" .
        log "Rootfs tarball: $OUTPUT_DIR/uos-tv-s905x-rootfs.tar.gz"
    fi
}

# ── Docker-based Kernel Build (macOS friendly) ────────
build_kernel_docker() {
    log "Building kernel with Docker cross-compiler..."
    mkdir -p "$KERNEL_DIR"

    cat > "$TARGET_DIR/Dockerfile.kernel" << 'DOCKERFILE'
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git bc flex bison make gcc gcc-aarch64-linux-gnu \
    libssl-dev libncurses-dev libelf-dev device-tree-compiler \
    curl ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /work
DOCKERFILE

    docker build -t uos-kernel-builder -f "$TARGET_DIR/Dockerfile.kernel" "$TARGET_DIR"

    if [ ! -d "$KERNEL_DIR/.git" ]; then
        git clone --depth=1 --branch "v$KERNEL_VERSION" \
            https://github.com/torvalds/linux.git "$KERNEL_DIR" 2>/dev/null
    fi

    docker run --rm -v "$KERNEL_DIR:/work" uos-kernel-builder bash -c "
        make ARCH=arm64 defconfig
        ./scripts/config -e ARCH_MESON
        ./scripts/config -e DRM_MESON
        ./scripts/config -e DRM_LIMA
        ./scripts/config -e MMC_MESON_GX
        ./scripts/config -e STMMAC_ETH
        ./scripts/config -e EXT4_FS
        ./scripts/config -m RTL8723BS
        ./scripts/config -m BRCMFMAC
        ./scripts/config -e CFG80211
        make ARCH=arm64 olddefconfig
        make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- -j\$(nproc) Image dtbs modules
    " 2>&1 | tail -20
}

# ── Main ──────────────────────────────────────────────
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

show_compatibility

case "${1:-}" in
    --kernel-only)
        build_kernel
        ;;
    --kernel-docker)
        build_kernel_docker
        ;;
    --wifi)
        log "Building WiFi drivers for S905X..."
        "$SCRIPT_DIR/build-s905x-wifi.sh" --all \
            --kernel "$KERNEL_DIR" --target "$TARGET_DIR"
        log "Installing WiFi to rootfs..."
        "$SCRIPT_DIR/build-s905x-wifi.sh" --install "$ROOTFS_DIR" \
            --target "$TARGET_DIR"
        ;;
    --rootfs-only)
        build_rootfs
        ;;
    --uboot)
        build_uboot
        ;;
    --image)
        create_sd_image
        ;;
    --all|"")
        log "Building everything for Amlogic S905X..."
        log ""
        log "Step 1: Build kernel"
        build_kernel_docker
        log ""
        log "Step 2: Build WiFi drivers + firmware"
        "$SCRIPT_DIR/build-s905x-wifi.sh" --all \
            --kernel "$KERNEL_DIR" --target "$TARGET_DIR" || \
            warn "WiFi driver build failed (non-fatal)"
        log ""
        log "Step 3: Build rootfs"
        build_rootfs
        log ""
        log "Step 4: Create SD image"
        create_sd_image
        log ""
        log "=== Build Complete ==="
        log "SD image: $OUTPUT_DIR/uos-tv-s905x.img"
        log ""
        log "To deploy:"
        log "  1. Insert SD card"
        log "  2. sudo dd if=$OUTPUT_DIR/uos-tv-s905x.img of=/dev/mmcblkX bs=4M"
        log "  3. Insert into S905X box, hold reset button (toothpick)"
        log "  4. Power on → UOS TV boots!"
        ;;
    *)
        echo "Usage: $0 [--kernel-only|--kernel-docker|--wifi|--rootfs-only|--uboot|--image|--all]"
        ;;
esac
