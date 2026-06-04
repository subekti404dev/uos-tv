#!/bin/bash
# ============================================================================
# UOS TV — Amlogic S905X WiFi Driver Builder
# ============================================================================
# Builds out-of-tree WiFi kernel modules for Amlogic S905X set-top boxes.
#
# Supported chips:
#   - RTL8189ES (SDIO) — B860H
#   - RTL8189FS (SDIO) — HG680P
#   - RTL8723BS (SDIO) — Nexbox-A95X, LibreTech-CC (mainline, optional)
#
# Sources:
#   - RTL8189ES: openwetek/rtl8189es (dedicated driver, CONFIG_RTL8188E=y)
#   - RTL8189FS: OpenIPC/realtek-wlan (includes Amlogic S905 SDIO platform)
#
# Firmware sources:
#   - linux-firmware (git.kernel.org) or armbian/firmware
#   - RTL8188EU/ES family: rtlwifi/rtl8188eufw.bin
#   - RTL8189FS: rtlwifi/rtl8188fufw.bin or similar
#
# Usage:
#   ./scripts/build-s905x-wifi.sh --all                    # Build all drivers
#   ./scripts/build-s905x-wifi.sh --chip rtl8189es         # Build RTL8189ES only
#   ./scripts/build-s905x-wifi.sh --chip rtl8189fs         # Build RTL8189FS only
#   ./scripts/build-s905x-wifi.sh --firmware-only           # Download firmware only
#   ./scripts/build-s905x-wifi.sh --kernel /path/to/kernel  # Specify kernel tree
#
# Environment:
#   CROSS_COMPILE=aarch64-linux-gnu- (default)
#   KERNEL_DIR=/path/to/kernel  (default: build/s905x/kernel)
#   TARGET_DIR=/path/to/output  (default: build/s905x)
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Config
KERNEL_DIR="${KERNEL_DIR:-$PROJECT_DIR/build/s905x/kernel}"
TARGET_DIR="${TARGET_DIR:-$PROJECT_DIR/build/s905x}"
WIFI_DIR="$TARGET_DIR/wifi-drivers"
FIRMWARE_DIR="$TARGET_DIR/firmware"
CROSS_COMPILE="${CROSS_COMPILE:-aarch64-linux-gnu-}"

GREEN='\033[0;32m'; BLUE='\033[0;34m'; YELLOW='\033[1;33m'
NC='\033[0m'
log()  { echo -e "${GREEN}[WIFI]${NC} $*"; }
info() { echo -e "${BLUE}  →${NC} $*"; }
warn() { echo -e "${YELLOW}  !${NC} $*"; }

# ── Check prerequisites ─────────────────────────
check_prereqs() {
    # Resolve relative paths to absolute (script may cd later)
    KERNEL_DIR="$(cd "$KERNEL_DIR" 2>/dev/null && pwd || echo "$KERNEL_DIR")"
    TARGET_DIR="$(cd "$TARGET_DIR" 2>/dev/null && pwd || echo "$TARGET_DIR")"

    if [ ! -d "$KERNEL_DIR" ]; then
        warn "Kernel directory not found: $KERNEL_DIR"
        warn "Build kernel first: ./scripts/bootstrap-s905x.sh --kernel-docker"
        return 1
    fi

    if [ ! -f "$KERNEL_DIR/Makefile" ]; then
        warn "Not a valid kernel tree: $KERNEL_DIR"
        return 1
    fi

    if ! command -v "${CROSS_COMPILE}gcc" &>/dev/null; then
        warn "Cross-compiler not found: ${CROSS_COMPILE}gcc"
        warn "Install: apt install gcc-aarch64-linux-gnu (or use Docker)"
        return 1
    fi

    log "Kernel tree: $KERNEL_DIR"
    log "Cross-compiler: ${CROSS_COMPILE}gcc"
    return 0
}

# ── Build RTL8189ES ─────────────────────────────
build_rtl8189es() {
    log "Building RTL8189ES (SDIO) — for B860H..."

    local DRV_DIR="$WIFI_DIR/rtl8189es"
    local MODULE_OUT="$DRV_DIR/8189es.ko"

    if [ -f "$MODULE_OUT" ]; then
        log "RTL8189ES module already built: $MODULE_OUT"
        return 0
    fi

    mkdir -p "$WIFI_DIR"

    # Clone driver
    if [ ! -d "$DRV_DIR/.git" ]; then
        info "Cloning openwetek/rtl8189es..."
        git clone --depth=1 https://github.com/openwetek/rtl8189es.git "$DRV_DIR" 2>/dev/null
    fi

    cd "$DRV_DIR/rtl8189ES"

    # This driver uses CONFIG_RTL8188E=y, CONFIG_SDIO_HCI=y by default
    # Module name: 8189es
    info "Module config: RTL8188E (8189ES), SDIO interface"

    # Cross-compile with kernel build system
    info "Compiling 8189es.ko..."
    make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" \
        -C "$KERNEL_DIR" M="$PWD" \
        modules -j"$(nproc 2>/dev/null || echo 4)" 2>&1 | tail -5

    if [ -f "$PWD/$MODULE_OUT" ] || [ -f "$PWD/8189es.ko" ]; then
        log "✓ RTL8189ES module built"
        ls -lh "$PWD/8189es.ko" 2>/dev/null || ls -lh "$MODULE_OUT" 2>/dev/null
    else
        warn "RTL8189ES build may have failed. Check output above."
        return 1
    fi

    cd "$PROJECT_DIR"
}

# ── Build RTL8189FS ─────────────────────────────
build_rtl8189fs() {
    log "Building RTL8189FS (SDIO) — for HG680P..."

    local DRV_DIR="$WIFI_DIR/rtl8189fs"
    local MODULE_OUT="$DRV_DIR/8189fs.ko"

    if [ -f "$MODULE_OUT" ]; then
        log "RTL8189FS module already built: $MODULE_OUT"
        return 0
    fi

    mkdir -p "$WIFI_DIR"

    # Clone driver (OpenIPC realtek-wlan — has Amlogic S905 SDIO platform support)
    if [ ! -d "$DRV_DIR/.git" ]; then
        info "Cloning OpenIPC/realtek-wlan (Amlogic S905 platform)..."
        git clone --depth=1 https://github.com/OpenIPC/realtek-wlan.git "$DRV_DIR" 2>/dev/null
    fi

    cd "$DRV_DIR"

    # Configure for RTL8188F (8189FS variant) + SDIO + Amlogic platform
    info "Patching config for RTL8189FS + Amlogic S905 SDIO..."

    # Create a copy of the Makefile with our config
    cp Makefile Makefile.orig

    # Set WiFi IC: RTL8188F = y (this covers 8189FS family)
    sed -i 's/^CONFIG_RTL8188F = .*/CONFIG_RTL8188F = y/' Makefile
    # Disable other chips
    sed -i 's/^CONFIG_RTL8188GTV = .*/CONFIG_RTL8188GTV = n/' Makefile
    sed -i 's/^CONFIG_RTL8822B = .*/CONFIG_RTL8822B = n/' Makefile
    sed -i 's/^CONFIG_RTL8723D = .*/CONFIG_RTL8723D = n/' Makefile
    sed -i 's/^CONFIG_RTL8821C = .*/CONFIG_RTL8821C = n/' Makefile
    sed -i 's/^CONFIG_RTL8710B = .*/CONFIG_RTL8710B = n/' Makefile
    sed -i 's/^CONFIG_RTL8192F = .*/CONFIG_RTL8192F = n/' Makefile
    sed -i 's/^CONFIG_RTL8822C = .*/CONFIG_RTL8822C = n/' Makefile
    sed -i 's/^CONFIG_RTL8814B = .*/CONFIG_RTL8814B = n/'

    # Set interface: SDIO = y
    sed -i 's/^CONFIG_SDIO_HCI = .*/CONFIG_SDIO_HCI = y/' Makefile
    sed -i 's/^CONFIG_USB_HCI = .*/CONFIG_USB_HCI = n/' Makefile
    sed -i 's/^CONFIG_PCI_HCI = .*/CONFIG_PCI_HCI = n/' Makefile

    # Module name: 8189fs
    if grep -q "MODULE_NAME" Makefile; then
        sed -i 's/MODULE_NAME = .*/MODULE_NAME = 8189fs/' Makefile
    else
        # Add MODULE_NAME after the interface config section
        sed -i '/^CONFIG_GSPI_HCI/a MODULE_NAME = 8189fs' Makefile
    fi

    # Set Amlogic S905 SDIO platform defines
    # The platform_aml_s905_sdio.c needs:
    #   CONFIG_PLATFORM_AML_S905 = y
    # This isn't in the Makefile by default, we need to add it
    #
    # The driver's platform selection is in platform/platform_ops.h
    # We pass the define via EXTRA_CFLAGS
    echo 'EXTRA_CFLAGS += -DCONFIG_PLATFORM_AML_S905' >> Makefile

    info "Compiling 8189fs.ko..."
    make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" \
        -C "$KERNEL_DIR" M="$PWD" \
        modules -j"$(nproc 2>/dev/null || echo 4)" 2>&1 | tail -5

    if [ -f "$PWD/8189fs.ko" ]; then
        log "✓ RTL8189FS module built"
        ls -lh "$PWD/8189fs.ko"
    else
        warn "RTL8189FS build may have failed. Check output above."
        # Restore original Makefile
        cp Makefile.orig Makefile
        return 1
    fi

    cd "$PROJECT_DIR"
}

# ── Build RTL8723BS (mainline, optional) ────────
build_rtl8723bs() {
    log "Building RTL8723BS (SDIO) — for Nexbox-A95X / LibreTech-CC..."

    local DRV_DIR="$WIFI_DIR/rtl8723bs"
    local MODULE_OUT="$DRV_DIR/r8723bs.ko"

    if [ -f "$MODULE_OUT" ]; then
        log "RTL8723BS module already built: $MODULE_OUT"
        return 0
    fi

    mkdir -p "$WIFI_DIR"

    # RTL8723BS is in mainline staging since ~4.12
    # We build it as external module from the kernel tree
    if [ -d "$KERNEL_DIR/drivers/staging/rtl8723bs" ]; then
        info "Building RTL8723BS from staging..."
        cd "$KERNEL_DIR"
        make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" \
            M=drivers/staging/rtl8723bs \
            modules -j"$(nproc 2>/dev/null || echo 4)" 2>&1 | tail -3
        if [ -f "$KERNEL_DIR/drivers/staging/rtl8723bs/r8723bs.ko" ]; then
            mkdir -p "$DRV_DIR"
            cp "$KERNEL_DIR/drivers/staging/rtl8723bs/r8723bs.ko" "$MODULE_OUT"
            log "✓ RTL8723BS module built"
            ls -lh "$MODULE_OUT"
        fi
    else
        warn "RTL8723BS staging not found in kernel tree"
        warn "Enable CONFIG_RTL8723BS=m in kernel config first"
        return 1
    fi

    cd "$PROJECT_DIR"
}

# ── Fetch Firmware ──────────────────────────────
fetch_firmware() {
    log "Fetching WiFi firmware blobs..."
    mkdir -p "$FIRMWARE_DIR/rtlwifi"

    local FW_DIR="$FIRMWARE_DIR"

    # ── RTL8189ES firmware ──
    # Uses rtl8188eu family firmware
    local FW_8188="rtlwifi/rtl8188eufw.bin"
    if [ ! -f "$FW_DIR/$FW_8188" ]; then
        info "Downloading RTL8188EU firmware (for 8189ES)..."
        mkdir -p "$(dirname "$FW_DIR/$FW_8188")"
        curl -sfL "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/rtlwifi/rtl8188eufw.bin" \
            -o "$FW_DIR/$FW_8188" 2>/dev/null || \
        curl -sfL "https://raw.githubusercontent.com/armbian/firmware/master/rtlwifi/rtl8188eufw.bin" \
            -o "$FW_DIR/$FW_8188" 2>/dev/null || \
        warn "Could not download rtl8188eufw.bin — will need manual placement"
    fi
    if [ -f "$FW_DIR/$FW_8188" ]; then
        log "  ✓ rtl8188eufw.bin ($(du -h "$FW_DIR/$FW_8188" | cut -f1))"
    fi

    # ── RTL8189FS firmware ──
    # Uses rtl8188f variant
    local FW_8188F="rtlwifi/rtl8188fufw.bin"
    if [ ! -f "$FW_DIR/$FW_8188F" ]; then
        info "Downloading RTL8188F firmware (for 8189FS)..."
        mkdir -p "$(dirname "$FW_DIR/$FW_8188F")"
        # Try linux-firmware first, then armbian
        curl -sfL "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/rtlwifi/rtl8188fufw.bin" \
            -o "$FW_DIR/$FW_8188F" 2>/dev/null || \
        curl -sfL "https://raw.githubusercontent.com/armbian/firmware/master/rtlwifi/rtl8188fufw.bin" \
            -o "$FW_DIR/$FW_8188F" 2>/dev/null || \
        warn "Could not download rtl8188fufw.bin — will need manual placement"
    fi
    if [ -f "$FW_DIR/$FW_8188F" ]; then
        log "  ✓ rtl8188fufw.bin ($(du -h "$FW_DIR/$FW_8188F" | cut -f1))"
    fi

    # ── RTL8723BS firmware ──
    local FW_8723="rtlwifi/rtl8723bs_nic.bin"
    if [ ! -f "$FW_DIR/$FW_8723" ]; then
        info "Downloading RTL8723BS firmware..."
        mkdir -p "$(dirname "$FW_DIR/$FW_8723")"
        curl -sfL "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/rtlwifi/rtl8723bs_nic.bin" \
            -o "$FW_DIR/$FW_8723" 2>/dev/null || \
        curl -sfL "https://raw.githubusercontent.com/armbian/firmware/master/rtlwifi/rtl8723bs_nic.bin" \
            -o "$FW_DIR/$FW_8723" 2>/dev/null || \
        warn "Could not download rtl8723bs_nic.bin"
    fi
    if [ -f "$FW_DIR/$FW_8723" ]; then
        log "  ✓ rtl8723bs_nic.bin ($(du -h "$FW_DIR/$FW_8723" | cut -f1))"
    fi

    # ── Broadcom firmware (Khadas VIM) ──
    # AP6212 uses BCM43430
    local FW_BRCM="brcm/brcmfmac43430-sdio.bin"
    local FW_BRCM_TXT="brcm/brcmfmac43430-sdio.txt"
    if [ ! -f "$FW_DIR/$FW_BRCM" ]; then
        info "Downloading Broadcom BCM43430 firmware..."
        mkdir -p "$(dirname "$FW_DIR/$FW_BRCM")"
        curl -sfL "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/brcm/brcmfmac43430-sdio.bin" \
            -o "$FW_DIR/$FW_BRCM" 2>/dev/null || \
        warn "Could not download brcmfmac43430-sdio.bin"
    fi
    if [ ! -f "$FW_DIR/$FW_BRCM_TXT" ]; then
        curl -sfL "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/brcm/brcmfmac43430-sdio.txt" \
            -o "$FW_DIR/$FW_BRCM_TXT" 2>/dev/null || true
    fi
    if [ -f "$FW_DIR/$FW_BRCM" ]; then
        log "  ✓ brcmfmac43430-sdio.bin ($(du -h "$FW_DIR/$FW_BRCM" | cut -f1))"
    fi

    log "Firmware fetched → $FW_DIR"
    echo ""
    echo "  Firmware tree:"
    find "$FW_DIR" -type f -name "*.bin" -o -name "*.txt" 2>/dev/null | while read f; do
        echo "    $(echo "$f" | sed "s|$FW_DIR/||") ($(du -h "$f" | cut -f1))"
    done
}

# ── Install to rootfs ───────────────────────────
install_to_rootfs() {
    local ROOTFS="${1:-$TARGET_DIR/rootfs}"
    log "Installing WiFi drivers + firmware to rootfs: $ROOTFS"

    # Determine kernel version for proper module path
    local KVER=""
    if [ -f "$KERNEL_DIR/include/generated/utsrelease.h" ]; then
        KVER=$(grep UTS_RELEASE "$KERNEL_DIR/include/generated/utsrelease.h" | \
            sed 's/.*"\(.*\)".*/\1/')
    else
        KVER="unknown"
    fi
    local MOD_DEST="$ROOTFS/lib/modules/$KVER/extra"
    mkdir -p "$MOD_DEST"

    log "Installing modules to: $MOD_DEST"

    # Copy .ko files to versioned extra/
    for drv_dir in "$WIFI_DIR"/*/; do
        find "$drv_dir" -name "*.ko" -exec cp -v {} "$MOD_DEST/" \; 2>/dev/null || true
    done

    # Also check kernel tree for staging modules
    if [ -d "$KERNEL_DIR" ]; then
        find "$KERNEL_DIR" -path "*/staging/rtl8723bs/r8723bs.ko" \
            -exec cp -v {} "$MOD_DEST/" \; 2>/dev/null || true
        find "$KERNEL_DIR/drivers/net/wireless" -name "*.ko" \
            -exec cp -v {} "$MOD_DEST/" \; 2>/dev/null || true
    fi

    # Note: depmod must be run after this — handled by bootstrap-s905x.sh

    # Firmware
    if [ -d "$FIRMWARE_DIR" ]; then
        mkdir -p "$ROOTFS/lib/firmware"
        cp -r "$FIRMWARE_DIR"/* "$ROOTFS/lib/firmware/" 2>/dev/null || true
    fi

    log "WiFi files installed to rootfs (kernel=$KVER)"
    echo ""
    echo "  Modules ($MOD_DEST):"
    ls -lh "$MOD_DEST"/*.ko 2>/dev/null || echo "    (none)"
    echo ""
    echo "  Firmware:"
    find "$ROOTFS/lib/firmware" -type f -name "*.bin" 2>/dev/null | while read f; do
        echo "    $(echo "$f" | sed "s|$ROOTFS||") ($(du -h "$f" | cut -f1))"
    done
}

# ── Help ────────────────────────────────────────
show_help() {
    echo "UOS TV — S905X WiFi Driver Builder"
    echo ""
    echo "Usage: $0 [options]"
    echo ""
    echo "Options:"
    echo "  --all               Build all drivers + fetch firmware (default)"
    echo "  --chip rtl8189es    Build RTL8189ES (B860H)"
    echo "  --chip rtl8189fs    Build RTL8189FS (HG680P)"
    echo "  --chip rtl8723bs    Build RTL8723BS (Nexbox-A95X, LibreTech-CC)"
    echo "  --firmware-only      Download firmware blobs only"
    echo "  --kernel PATH        Path to kernel tree (default: build/s905x/kernel)"
    echo "  --target PATH        Output directory (default: build/s905x)"
    echo "  --install ROOTFS     Install drivers+firmware to rootfs"
    echo "  --help               This help"
    echo ""
    echo "Environment:"
    echo "  CROSS_COMPILE        Cross-compiler prefix (default: aarch64-linux-gnu-)"
    echo "  KERNEL_DIR           Kernel tree path"
    echo "  TARGET_DIR           Output directory"
    echo ""
    echo "Supported boards:"
    echo "  B860H      → RTL8189ES (SDIO)"
    echo "  HG680P     → RTL8189FS (SDIO)"
    echo "  Nexbox-A95X→ RTL8723BS (SDIO, mainline)"
    echo "  LibreTech-CC→RTL8723BS (SDIO, mainline)"
    echo "  Khadas VIM → Broadcom BCM43430 (SDIO, mainline)"
}

# ── Main ────────────────────────────────────────
CHIP=""
FIRMWARE_ONLY=false
INSTALL_ROOTFS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --all)        CHIP="all" ;;
        --chip)       CHIP="$2"; shift ;;
        --firmware-only) FIRMWARE_ONLY=true ;;
        --kernel)     KERNEL_DIR="$2"; shift ;;
        --target)     TARGET_DIR="$2"; shift ;;
        --install)    INSTALL_ROOTFS="$2"; shift ;;
        --help)       show_help; exit 0 ;;
        *)            echo "Unknown option: $1"; show_help; exit 1 ;;
    esac
    shift
done

# Update dependent paths
WIFI_DIR="$TARGET_DIR/wifi-drivers"
FIRMWARE_DIR="$TARGET_DIR/firmware"

# Default: build all
if [ -z "$CHIP" ] && [ "$FIRMWARE_ONLY" = false ]; then
    CHIP="all"
fi

# Firmware always useful
if [ "$CHIP" = "all" ] || [ "$FIRMWARE_ONLY" = true ]; then
    fetch_firmware
fi

if [ "$FIRMWARE_ONLY" = true ]; then
    exit 0
fi

# Check prerequisites for compilation
if ! check_prereqs; then
    warn "Prerequisites not met. Try Docker-based build instead."
    exit 1
fi

# Build selected chip(s)
case "$CHIP" in
    all)
        build_rtl8189es
        build_rtl8189fs
        build_rtl8723bs
        ;;
    rtl8189es|8189es)
        build_rtl8189es
        ;;
    rtl8189fs|8189fs)
        build_rtl8189fs
        ;;
    rtl8723bs|8723bs)
        build_rtl8723bs
        ;;
    *)
        warn "Unknown chip: $CHIP"
        show_help
        exit 1
        ;;
esac

# Install to rootfs if requested
if [ -n "$INSTALL_ROOTFS" ]; then
    install_to_rootfs "$INSTALL_ROOTFS"
fi

log "WiFi driver build complete!"
echo ""
echo "  Output: $WIFI_DIR/"
echo "  Firmware: $FIRMWARE_DIR/"
echo ""
echo "  To install to rootfs:"
echo "    $0 --install $TARGET_DIR/rootfs"
