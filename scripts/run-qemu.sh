#!/bin/bash
# run-qemu.sh — Run UOS TV in QEMU (aarch64 virt)
# =================================================
# Two modes:
#   1. Direct kernel boot (--quick) — fast, no UEFI needed
#      Uses build/kernel/Image + extracted Alpine rootfs
#   2. Full UEFI boot — boots from disk image via UEFI firmware
#
# Usage:
#   ./scripts/run-qemu.sh [--quick] [--headless] [--debug]
#
# Quick mode prerequisites:
#   - Alpine rootfs at build/alpine-rootfs/ (extracted)
#   - Kernel at build/kernel/Image
#   Run: ./scripts/fetch-qemu-kernel.sh && make build
#
# Full mode prerequisites:
#   - UEFI firmware (edk2-aarch64-code.fd)
#   - Disk image at build/uos-tv.img
#   Run: make image

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/build"

GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
log()  { echo -e "${GREEN}[QEMU]${NC} $*"; }
info() { echo -e "${CYAN}  →${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
err()  { echo -e "${RED}✗${NC} $*"; }

# ── Defaults ──────────────────────────────────────────
QEMU_BIN="${QEMU_BIN:-qemu-system-aarch64}"
QEMU_MEM="${QEMU_MEM:-2048}"
QEMU_CORES="${QEMU_CORES:-4}"
QEMU_MACHINE="${QEMU_MACHINE:-virt}"
QEMU_CPU="${QEMU_CPU:-cortex-a57}"
SSH_PORT="${SSH_PORT:-2222}"
HTTP_PORT="${HTTP_PORT:-8080}"
HEADLESS=false
DEBUG=false
QUICK=false
KERNEL="${UOS_KERNEL:-$BUILD_DIR/kernel/Image}"
ROOTFS_DIR="${UOS_ROOTFS_DIR:-$BUILD_DIR/alpine-rootfs}"
IMAGE="${UOS_IMAGE:-$BUILD_DIR/uos-tv.img}"

# ── Parse Args ────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --quick)      QUICK=true; shift ;;
        --headless)   HEADLESS=true; shift ;;
        --debug)      DEBUG=true; shift ;;
        --image)      IMAGE="$2"; shift 2 ;;
        --kernel)     KERNEL="$2"; shift 2 ;;
        --rootfs)     ROOTFS_DIR="$2"; shift 2 ;;
        --mem)        QEMU_MEM="$2"; shift 2 ;;
        *)            echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# ── Checks ────────────────────────────────────────────
command -v "$QEMU_BIN" >/dev/null 2>&1 || {
    err "$QEMU_BIN not found."
    echo "  Install: brew install qemu  (macOS)"
    echo "           apt install qemu-system-arm  (Linux)"
    exit 1
}

# ═══════════════════════════════════════════════════════
# QEMU Arguments
# ═══════════════════════════════════════════════════════

QEMU_ARGS=(
    "$QEMU_BIN"
    -machine "$QEMU_MACHINE,highmem=off,gic-version=3"
    -cpu "$QEMU_CPU"
    -smp "$QEMU_CORES"
    -m "$QEMU_MEM"
    -netdev "user,id=net0,hostfwd=tcp::${SSH_PORT}-:22,hostfwd=tcp::${HTTP_PORT}-:8080"
    -device "virtio-net-device,netdev=net0"
    -serial "stdio"
    -no-reboot
)

# ── Graphics ──────────────────────────────────────────
if $HEADLESS; then
    QEMU_ARGS+=(-display none -nographic)
else
    if [[ "$OSTYPE" == "darwin"* ]]; then
        QEMU_ARGS+=(-display cocoa)
    else
        QEMU_ARGS+=(-display gtk,gl=off)
    fi
fi

# ── Debug ─────────────────────────────────────────────
if $DEBUG; then
    QEMU_ARGS+=(-s -S -d "guest_errors,unimp")
fi

# ═══════════════════════════════════════════════════════
# Boot Mode Selection
# ═══════════════════════════════════════════════════════

if $QUICK || [ ! -f "$IMAGE" ]; then
    # ──────────────────────────────────────────────────
    # Quick Mode: Direct Kernel Boot
    # ──────────────────────────────────────────────────
    log "=== UOS TV QEMU (Quick Kernel Boot) ==="

    if [ ! -f "$KERNEL" ]; then
        err "Kernel not found: $KERNEL"
        echo "  Run: ./scripts/fetch-qemu-kernel.sh"
        exit 1
    fi

    # Prepare rootfs if not extracted
    ROOTFS_TAR="$BUILD_DIR/alpine-rootfs/alpine-rootfs.tar.gz"
    if [ ! -f "$ROOTFS_DIR/etc/alpine-release" ] && [ -f "$ROOTFS_TAR" ]; then
        info "Extracting Alpine rootfs..."
        rm -rf "$ROOTFS_DIR"
        mkdir -p "$ROOTFS_DIR"
        tar xzf "$ROOTFS_TAR" -C "$ROOTFS_DIR"
        chmod 1777 "$ROOTFS_DIR/tmp" 2>/dev/null || true
    fi

    if [ ! -f "$ROOTFS_DIR/etc/alpine-release" ]; then
        warn "Alpine rootfs not found at $ROOTFS_DIR"
        warn "Quick mode needs an extracted rootfs."
        warn "Run: ./scripts/fetch-qemu-kernel.sh"
        warn "Or: tar xzf build/alpine-rootfs/alpine-rootfs.tar.gz -C build/alpine-rootfs/"
        exit 1
    fi

    # Apply UOS overlay if binaries are available
    RELEASE_DIR="$PROJECT_DIR/target/aarch64-unknown-linux-musl/release"
    if [ -d "$RELEASE_DIR" ] && [ -f "$RELEASE_DIR/stardustd" ]; then
        if [ ! -f "$ROOTFS_DIR/usr/bin/stardustd" ]; then
            info "Applying UOS overlay to rootfs..."
            chmod +x "$SCRIPT_DIR/overlay-alpine.sh"
            "$SCRIPT_DIR/overlay-alpine.sh" "$ROOTFS_DIR" "target/aarch64-unknown-linux-musl/release"
        fi
    fi

    # Create a minimal init if Alpine init is missing
    INIT_TARGET="$ROOTFS_DIR/init"
    if [ ! -x "$INIT_TARGET" ]; then
        info "Creating minimal init script..."
        cat > "$ROOTFS_DIR/init" <<'INIT'
#!/bin/sh
echo "=== UOS TV — Quick Boot ==="
mount -t proc proc /proc 2>/dev/null
mount -t sysfs sysfs /sys 2>/dev/null
mount -t devtmpfs devtmpfs /dev 2>/dev/null

mkdir -p /dev/pts /run /tmp
mount -t devpts devpts /dev/pts 2>/dev/null
mount -t tmpfs tmpfs /tmp 2>/dev/null

mkdir -p /run/uos /var/log/uos /data

# Network
ip link set lo up
ip link set eth0 up 2>/dev/null
udhcpc -i eth0 -n -q 2>/dev/null || true

# Start UOS services
if [ -x /usr/bin/stardustd ]; then
    echo "Starting stardustd..."
    /usr/bin/stardustd --socket /run/uos/bus.sock --ws-addr 0.0.0.0:9090 &
    sleep 0.5
fi

if [ -x /usr/bin/logd ]; then
    echo "Starting logd..."
    /usr/bin/logd --socket /run/uos/log.sock --log-dir /var/log/uos &
fi

if [ -x /usr/bin/monitord ]; then
    echo "Starting monitord..."
    /usr/bin/monitord --config /usr/share/uos/monitord.yaml &
    sleep 1
fi

echo "=== UOS TV Ready ==="
echo "  IPC Bus:  /run/uos/bus.sock"
echo "  Web UI:   http://localhost:8080"
echo ""

# Drop to shell
exec /bin/sh
INIT
        chmod +x "$INIT_TARGET"
    fi

    # Kernel command line
    QEMU_ARGS+=(
        -kernel "$KERNEL"
        -append "console=ttyAMA0,115200 earlycon=pl011,0x9000000 root=/dev/vda ro rootwait quiet loglevel=3"
        -drive "file=fat:rw:$ROOTFS_DIR,format=raw,if=none,id=drive0"
        -device "virtio-blk-device,drive=drive0"
    )

    log "  Kernel:   $KERNEL"
    log "  RootFS:   $ROOTFS_DIR"
    log "  Mode:     Direct kernel boot"

else
    # ──────────────────────────────────────────────────
    # Full Mode: UEFI + Disk Image
    # ──────────────────────────────────────────────────
    log "=== UOS TV QEMU (Full Disk Image) ==="

    if [ ! -f "$IMAGE" ]; then
        err "Disk image not found: $IMAGE"
        echo "  Run: make image"
        exit 1
    fi

    # Find UEFI firmware
    UEFI_CODE=""
    UEFI_VARS="$BUILD_DIR/uefi-vars.fd"

    for path in \
        "$BUILD_DIR/uefi/QEMU_EFI.fd" \
        /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
        /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/edk2/aarch64/QEMU_EFI.fd \
        /usr/share/OVMF/QEMU_EFI_aarch64.fd; do
        if [ -f "$path" ]; then
            UEFI_CODE="$path"
            break
        fi
    done

    if [ -z "$UEFI_CODE" ]; then
        warn "No UEFI firmware found. Falling back to quick mode."
        warn "Run: ./scripts/fetch-qemu-kernel.sh"
        warn ""
        # Re-run self in quick mode
        exec "$0" --quick
    fi

    # Copy UEFI vars (modifiable)
    if [ ! -f "$UEFI_VARS" ]; then
        cp "$UEFI_CODE" "$UEFI_VARS" 2>/dev/null || true
    fi

    QEMU_ARGS+=(
        -drive "file=$IMAGE,format=raw,if=none,id=drive0,cache=writeback"
        -device "virtio-blk-device,drive=drive0,bootindex=1"
        -drive "if=pflash,format=raw,file=$UEFI_CODE,readonly=on"
        -drive "if=pflash,format=raw,file=$UEFI_VARS"
    )

    log "  UEFI:     $UEFI_CODE"
    log "  Image:    $IMAGE"
    log "  Mode:     UEFI disk boot"
fi

# ── Summary ───────────────────────────────────────────
log "  Machine:  $QEMU_MACHINE"
log "  CPU:      $QEMU_CPU ($QEMU_CORES cores)"
log "  Memory:   ${QEMU_MEM}MB"
log "  Display:  $($HEADLESS && echo 'headless' || echo 'GUI')"
log "  SSH:      ssh -p $SSH_PORT root@localhost"
log "  Luna UI:  http://localhost:$HTTP_PORT"
[ $DEBUG = true ] && log "  GDB:      localhost:1234"
echo ""

exec "${QEMU_ARGS[@]}"
