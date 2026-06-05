#!/usr/bin/env bash
set -euo pipefail

IMG="${1:?usage: customize-armbian-model-a.sh <image.img> <output.img>}"
OUT="${2:?usage: customize-armbian-model-a.sh <image.img> <output.img>}"
PROJECT_DIR="${PROJECT_DIR:-$(pwd)}"
BIN_DIR="${BIN_DIR:-$PROJECT_DIR/target/aarch64-unknown-linux-musl/release}"
PACKAGES_FILE="${PACKAGES_FILE:-$PROJECT_DIR/configs/armbian-model-a/packages.txt}"
IMAGE_SIZE_EXTRA_GB="${IMAGE_SIZE_EXTRA_GB:-0}"
INSTALL_PACKAGES="${INSTALL_PACKAGES:-true}"
CLEANUP_MODE="${CLEANUP_MODE:-debug-full}"

if [[ ! -f "$IMG" ]]; then
  echo "ERROR: image not found: $IMG" >&2
  exit 1
fi

cp "$IMG" "$OUT"

if [[ "$IMAGE_SIZE_EXTRA_GB" != "0" ]]; then
  truncate -s "+${IMAGE_SIZE_EXTRA_GB}G" "$OUT"
fi

LOOP=""
BOOT_MNT=""
ROOT_MNT=""
cleanup() {
  set +e
  if mountpoint -q "$ROOT_MNT/proc"; then sudo umount "$ROOT_MNT/proc"; fi
  if mountpoint -q "$ROOT_MNT/sys"; then sudo umount "$ROOT_MNT/sys"; fi
  if mountpoint -q "$ROOT_MNT/dev/pts"; then sudo umount "$ROOT_MNT/dev/pts"; fi
  if mountpoint -q "$ROOT_MNT/dev"; then sudo umount "$ROOT_MNT/dev"; fi
  if mountpoint -q "$BOOT_MNT"; then sudo umount "$BOOT_MNT"; fi
  if mountpoint -q "$ROOT_MNT"; then sudo umount "$ROOT_MNT"; fi
  if [[ -n "$LOOP" ]]; then sudo losetup -d "$LOOP"; fi
  [[ -n "$BOOT_MNT" ]] && rm -rf "$BOOT_MNT"
  [[ -n "$ROOT_MNT" ]] && rm -rf "$ROOT_MNT"
}
trap cleanup EXIT

sudo partprobe "$OUT" 2>/dev/null || true
LOOP=$(sudo losetup --find --partscan --show "$OUT")
echo "Loop device: $LOOP"
sleep 2

if [[ "$IMAGE_SIZE_EXTRA_GB" != "0" ]]; then
  echo "Expanding root partition to fill image..."
  sudo parted -s "$LOOP" resizepart 2 100%
  sudo partprobe "$LOOP" || true
  sleep 2
fi

BOOT_PART="${LOOP}p1"
ROOT_PART="${LOOP}p2"
if [[ ! -b "$BOOT_PART" || ! -b "$ROOT_PART" ]]; then
  echo "ERROR: expected partitions $BOOT_PART and $ROOT_PART" >&2
  sudo fdisk -l "$LOOP" || true
  exit 1
fi

if [[ "$IMAGE_SIZE_EXTRA_GB" != "0" ]]; then
  sudo e2fsck -fy "$ROOT_PART" || true
  sudo resize2fs "$ROOT_PART"
fi

BOOT_MNT=$(mktemp -d)
ROOT_MNT=$(mktemp -d)
sudo mount "$BOOT_PART" "$BOOT_MNT"
sudo mount "$ROOT_PART" "$ROOT_MNT"

# Preserve Armbian boot files, but enforce the known-good uEnv for S905X p212/HG680P.
echo "Writing UOS uEnv.txt..."
sudo tee "$BOOT_MNT/uEnv.txt" >/dev/null <<'EOF'
LINUX=/zImage
INITRD=/uInitrd
FDT=/dtb/amlogic/meson-gxl-s905x-p212.dtb
APPEND=root=/dev/mmcblk1p2 rootwait rw rootfstype=ext4 console=ttyAML0,115200n8 console=tty0 no_console_suspend consoleblank=0 net.ifnames=0
EOF

# Ensure no mainline u-boot chainload file is present for HG680P/B860H stock u-boot path.
sudo rm -f "$BOOT_MNT/u-boot.ext"

# Armbian first-login is triggered by /etc/profile.d when a login shell starts.
# Remove it BEFORE any chroot command. Do not use `bash -l` in this script.
sudo rm -f "$ROOT_MNT/root/.not_logged_in_yet" "$ROOT_MNT/etc/armbian_first_run.txt" "$ROOT_MNT/boot/armbian_first_run.txt" 2>/dev/null || true
sudo rm -f "$ROOT_MNT/etc/profile.d/armbian-check-first-login.sh" "$ROOT_MNT/etc/profile.d/armbian-check-first-login-reboot.sh" 2>/dev/null || true

# Install qemu static for optional chroot apt install.
if [[ "$INSTALL_PACKAGES" == "true" ]]; then
  echo "Installing packages into Armbian rootfs..."
  sudo cp /usr/bin/qemu-aarch64-static "$ROOT_MNT/usr/bin/" 2>/dev/null || true
  sudo mount --bind /dev "$ROOT_MNT/dev"
  sudo mount --bind /dev/pts "$ROOT_MNT/dev/pts"
  sudo mount -t proc proc "$ROOT_MNT/proc"
  sudo mount -t sysfs sysfs "$ROOT_MNT/sys"
  sudo rm -f "$ROOT_MNT/etc/resolv.conf"
  printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' | sudo tee "$ROOT_MNT/etc/resolv.conf" >/dev/null
  PKGS=$(grep -Ev '^\s*(#|$)' "$PACKAGES_FILE" | tr '\n' ' ')
  # Avoid interactive maintainer prompts in CI. Some Armbian/Debian packages may
  # ask for the default command shell; force bash/noninteractive defaults.
  sudo chroot "$ROOT_MNT" /bin/bash -c "printf 'dash dash/sh boolean false\n' | debconf-set-selections || true"
  sudo chroot "$ROOT_MNT" /bin/bash -c "export DEBIAN_FRONTEND=noninteractive DEBIAN_PRIORITY=critical APT_LISTCHANGES_FRONTEND=none UCF_FORCE_CONFFOLD=1 SHELL=/bin/bash; apt-get update && timeout 25m apt-get install -y --no-install-recommends -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold $PKGS" || {
    echo "WARN: package install failed or timed out; continuing with debug image" >&2
  }
fi

# Copy UOS binaries.
echo "Installing UOS binaries..."
sudo install -d -m 0755 "$ROOT_MNT/usr/bin"
if [[ -d "$BIN_DIR" ]]; then
  for bin in inis monitord stardustd lunad lumind netmd wifid notifd otad pkgd logd dispald devmand powermand audiod inputd casync-rs rauc-rs update-verify; do
    if [[ -f "$BIN_DIR/$bin" ]]; then
      sudo install -m 0755 "$BIN_DIR/$bin" "$ROOT_MNT/usr/bin/$bin"
    else
      echo "WARN: missing binary $bin" >&2
    fi
  done
fi

# Copy UOS configs and UI assets.
echo "Installing UOS configs/UI..."
sudo install -d -m 0755 "$ROOT_MNT/usr/share/uos" "$ROOT_MNT/etc/uos" "$ROOT_MNT/etc/systemd/system" "$ROOT_MNT/var/lib/uos"
sudo cp -r "$PROJECT_DIR/configs/services.d" "$ROOT_MNT/usr/share/uos/" 2>/dev/null || true
sudo cp "$PROJECT_DIR/configs/system.yaml" "$ROOT_MNT/etc/uos/system.yaml" 2>/dev/null || true
sudo cp "$PROJECT_DIR/configs/monitord.yaml" "$ROOT_MNT/etc/uos/monitord.yaml" 2>/dev/null || true
if [[ -d "$PROJECT_DIR/luna" ]]; then
  sudo rm -rf "$ROOT_MNT/usr/share/uos/luna"
  sudo cp -r "$PROJECT_DIR/luna" "$ROOT_MNT/usr/share/uos/luna"
fi

# Browser launcher. Model A uses Xorg + openbox + Chromium first because it is
# available from Debian repos and easier to debug than DRM-only embedded stacks.
sudo tee "$ROOT_MNT/usr/bin/uos-browser" >/dev/null <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
URL="file:///usr/share/uos/luna/index.html"
openbox >/tmp/openbox.log 2>&1 &
sleep 1
xset -dpms s off s noblank 2>/dev/null || true
if command -v chromium >/dev/null 2>&1; then
  exec chromium --no-sandbox --kiosk --disable-gpu --disable-dev-shm-usage "$URL"
elif command -v chromium-browser >/dev/null 2>&1; then
  exec chromium-browser --no-sandbox --kiosk --disable-gpu --disable-dev-shm-usage "$URL"
elif command -v cog >/dev/null 2>&1; then
  exec cog "$URL"
else
  echo "No browser runtime found. Install chromium or cog." >&2
  while true; do sleep 3600; done
fi
EOF
sudo chmod 0755 "$ROOT_MNT/usr/bin/uos-browser"

sudo tee "$ROOT_MNT/usr/bin/uos-shell-launcher" >/dev/null <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
export HOME="${HOME:-/home/uos}"
mkdir -p "$HOME"
if command -v startx >/dev/null 2>&1; then
  exec startx /usr/bin/uos-browser -- :0 -nocursor vt7
fi
exec /usr/bin/uos-browser
EOF
sudo chmod 0755 "$ROOT_MNT/usr/bin/uos-shell-launcher"

# Dedicated user for UI and deterministic credentials for non-interactive boot.
sudo chroot "$ROOT_MNT" /bin/bash -c "id -u uos >/dev/null 2>&1 || useradd -m -s /bin/bash -G audio,video,input,netdev,sudo uos" || true
sudo chroot "$ROOT_MNT" /bin/bash -c "echo 'root:uosroot1234' | chpasswd; echo 'uos:uos1234' | chpasswd; chsh -s /bin/bash root; chsh -s /bin/bash uos" || true

# Minimal systemd units. Keep existing Armbian services intact for debug-full.
sudo tee "$ROOT_MNT/etc/systemd/system/uos-shell.service" >/dev/null <<'EOF'
[Unit]
Description=UOS TV Shell
After=multi-user.target network-online.target
Wants=network-online.target

[Service]
Type=simple
User=uos
Group=uos
Environment=HOME=/home/uos
Environment=XDG_RUNTIME_DIR=/run/user/1000
ExecStart=/usr/bin/uos-shell-launcher
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

sudo tee "$ROOT_MNT/etc/systemd/system/stardustd.service" >/dev/null <<'EOF'
[Unit]
Description=UOS Stardust Bus
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/stardustd
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl enable stardustd.service uos-shell.service" || true
sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl enable NetworkManager.service 2>/dev/null || true" || true
# Disable Armbian interactive first-login / web setup wizard; UOS image must
# boot straight into services/browser without asking for shell/user setup on
# HDMI/serial or spawning the armbian-armbiansetup AP.
sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl disable armbian-firstlogin.service armbian-first-run.service armbian-resize-filesystem.service armbian-ramlog.service armbian-zram-config.service 2>/dev/null || true" || true
sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl mask armbian-firstlogin.service armbian-first-run.service armbian-resize-filesystem.service 2>/dev/null || true" || true
sudo find "$ROOT_MNT/etc/systemd" "$ROOT_MNT/lib/systemd" "$ROOT_MNT/usr/lib/systemd" -type f \( -iname '*first*' -o -iname '*setup*' -o -iname '*armbian*.service' \) -print 2>/dev/null | while read -r unit; do
  base=$(basename "$unit")
  case "$base" in
    *first*|*setup*|armbian-first*|armbian-resize*)
      sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl disable '$base' 2>/dev/null || true; systemctl mask '$base' 2>/dev/null || true" || true
      ;;
  esac
done
sudo find "$ROOT_MNT/etc/systemd/system" -type l \( -iname '*first*' -o -iname '*setup*' -o -iname '*armbian*' \) -delete 2>/dev/null || true
sudo rm -f "$ROOT_MNT/etc/profile.d/armbian-check-first-login.sh" "$ROOT_MNT/etc/profile.d/armbian-check-first-login-reboot.sh" 2>/dev/null || true
sudo rm -f "$ROOT_MNT/root/.not_logged_in_yet" "$ROOT_MNT/etc/armbian_first_run.txt" "$ROOT_MNT/boot/armbian_first_run.txt" 2>/dev/null || true
sudo rm -rf "$ROOT_MNT/etc/NetworkManager/system-connections"/*armbian* "$ROOT_MNT/etc/NetworkManager/system-connections"/*setup* 2>/dev/null || true

# First boot identity cleanup; preserve debug-full tools/log dirs.
sudo truncate -s 0 "$ROOT_MNT/etc/machine-id" 2>/dev/null || true
sudo rm -f "$ROOT_MNT/var/lib/dbus/machine-id" 2>/dev/null || true

if [[ "$CLEANUP_MODE" == "minimize" ]]; then
  echo "Applying minimize cleanup..."
  sudo chroot "$ROOT_MNT" /bin/bash -c "apt-get clean || true"
  sudo rm -rf "$ROOT_MNT/var/cache/apt/archives"/*.deb "$ROOT_MNT/tmp"/* "$ROOT_MNT/var/tmp"/* 2>/dev/null || true
  sudo find "$ROOT_MNT/var/log" -type f -delete 2>/dev/null || true
else
  echo "Keeping debug-full image (no aggressive cleanup)."
fi

sync
sudo umount "$BOOT_MNT"
sudo umount "$ROOT_MNT/proc" 2>/dev/null || true
sudo umount "$ROOT_MNT/sys" 2>/dev/null || true
sudo umount "$ROOT_MNT/dev/pts" 2>/dev/null || true
sudo umount "$ROOT_MNT/dev" 2>/dev/null || true
sudo umount "$ROOT_MNT"
sudo losetup -d "$LOOP"
LOOP=""

echo "Customized image: $OUT"
ls -lh "$OUT"
