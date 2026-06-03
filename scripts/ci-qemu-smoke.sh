#!/usr/bin/env bash
# ci-qemu-smoke.sh — CI-friendly QEMU boot smoke test
# ===================================================
# Boots the prepared Alpine/UOS rootfs image and validates:
#   - monitord reaches "Boot complete"
#   - expected number of services report [OK]
#   - Luna HTTP returns 200 via hostfwd :8080
#
# Prerequisites:
#   - build/kernel/alpine-vmlinuz
#   - build/kernel/alpine-initramfs
#   - build/alpine-rootfs-extracted with UOS overlay
#   - qemu-system-aarch64 + genext2fs

set -euo pipefail

ROOTFS_DIR="${ROOTFS_DIR:-build/alpine-rootfs-extracted}"
KERNEL="${KERNEL:-build/kernel/Image}"
INITRD="${INITRD:-build/kernel/alpine-initramfs}"
IMAGE="${IMAGE:-build/uos-ci-smoke.img}"
LOG="${LOG:-build/qemu-smoke.log}"
SIZE_MB="${SIZE_MB:-900}"
MIN_SERVICES="${MIN_SERVICES:-12}"
TIMEOUT_SECS="${TIMEOUT_SECS:-30}"

mkdir -p build

fail() {
  echo "[CI-SMOKE] FAIL: $*" >&2
  if [[ -f "$LOG" ]]; then
    echo "--- QEMU log tail ---" >&2
    tail -120 "$LOG" | tr '\r' '\n' >&2 || true
  fi
  exit 1
}

[[ -f "$KERNEL" ]] || fail "Missing $KERNEL. Run scripts/fetch-qemu-kernel.sh build first."
if [[ ! -f "$INITRD" ]]; then
  echo "[CI-SMOKE] No initrd found at $INITRD; booting kernel without initrd"
  INITRD=""
fi
[[ -d "$ROOTFS_DIR" ]] || fail "Missing rootfs dir: $ROOTFS_DIR"

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  fail "qemu-system-aarch64 not found"
fi
if ! command -v genext2fs >/dev/null 2>&1; then
  fail "genext2fs not found"
fi

pkill -9 -f qemu-system-aarch64 2>/dev/null || true

BLOCKS=$((SIZE_MB * 1024))
echo "[CI-SMOKE] Creating ext2 image: $IMAGE (${SIZE_MB}MB)"
genext2fs -d "$ROOTFS_DIR" -b "$BLOCKS" -L UOS_ROOT -N 20000 "$IMAGE" >/dev/null

echo "[CI-SMOKE] Starting QEMU (timeout ${TIMEOUT_SECS}s)..."
QEMU_ARGS=(
  -machine virt,highmem=off
  -cpu cortex-a57
  -smp 2
  -m 512
  -kernel "$KERNEL"
  -append "console=ttyAMA0,115200 root=/dev/vda rw rootfstype=ext2 init=/sbin/init"
  -drive "file=$IMAGE,format=raw,if=none,id=drive0"
  -device virtio-blk-device,drive=drive0
  -nic user,hostfwd=tcp::8080-:80,hostfwd=tcp::9090-:9090
  -nographic
  -no-reboot
)
if [[ -n "$INITRD" ]]; then
  QEMU_ARGS+=(-initrd "$INITRD")
fi

qemu-system-aarch64 "${QEMU_ARGS[@]}" > "$LOG" 2>&1 &
QEMU_PID=$!

cleanup() {
  kill "$QEMU_PID" 2>/dev/null || true
  wait "$QEMU_PID" 2>/dev/null || true
}
trap cleanup EXIT

BOOT_OK=0
for _ in $(seq 1 "$TIMEOUT_SECS"); do
  if grep -q "Boot complete" "$LOG" 2>/dev/null; then
    BOOT_OK=1
    break
  fi
  sleep 1
done

[[ "$BOOT_OK" == "1" ]] || fail "Boot did not complete within ${TIMEOUT_SECS}s"

SERVICES_OK=$(tr '\r' '\n' < "$LOG" | grep -c '\[OK\]' || true)
if [[ "$SERVICES_OK" -lt "$MIN_SERVICES" ]]; then
  fail "Only ${SERVICES_OK} services OK (minimum ${MIN_SERVICES})"
fi

HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://localhost:8080/ || echo 000)
if [[ "$HTTP_CODE" != "200" ]]; then
  fail "Luna HTTP returned $HTTP_CODE (expected 200)"
fi

BOOT_LINE=$(tr '\r' '\n' < "$LOG" | grep "Boot complete" | tail -1)
echo "[CI-SMOKE] PASS: $BOOT_LINE"
echo "[CI-SMOKE] PASS: services OK = $SERVICES_OK"
echo "[CI-SMOKE] PASS: Luna HTTP 200"
