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

# Browser launcher. Priority: Cog (WPE) > Surf (WebKitGTK) > Chromium.
# Cog is lightest (DRM direct) but may not work on all DRM drivers.
# Surf is ~40% lighter than Chromium and preferred fallback.
sudo tee "$ROOT_MNT/usr/bin/uos-browser" >/dev/null <<'BROWSEREOF'
#!/usr/bin/env bash
set -euo pipefail
URL="http://127.0.0.1:8080/index.html"

# Best: Cog WPE (DRM direct, no X needed)
if command -v cog >/dev/null 2>&1; then
  exec cog "$URL"
fi

# Good: Surf (WebKitGTK under X, ~40% lighter than Chromium)
if command -v surf >/dev/null 2>&1; then
  exec surf "${URL}"
fi

# Fallback: Chromium (heavy, full-featured)
if command -v chromium >/dev/null 2>&1; then
  exec chromium \
    --no-sandbox \
    --kiosk \
    --disable-dev-shm-usage \
    --force-device-scale-factor=1 \
    --window-size=1920,1080 \
    --disable-gpu \
    --disable-software-rasterizer \
    --disable-features=TranslateUI,VizDisplayCompositor \
    --disable-extensions \
    --disable-background-networking \
    --disable-sync \
    --disable-default-apps \
    --no-first-run \
    --noerrdialogs \
    --disable-translate \
    --disable-password-manager \
    --disable-crash-reporter \
    --disable-component-update \
    --renderer-process-limit=2 \
    --max_old_space_size=128 \
    --js-flags="--max-old-space-size=128" \
    "${URL}"
fi

echo "No browser runtime found." >&2
while true; do sleep 3600; done
BROWSEREOF
sudo chmod 0755 "$ROOT_MNT/usr/bin/uos-browser"

# Shell launcher: start X with DPI override for TV readability.
sudo tee "$ROOT_MNT/usr/bin/uos-shell-launcher" >/dev/null <<'LAUNCHEREOF'
#!/usr/bin/env bash
set -euo pipefail
export HOME="${HOME:-/home/uos}"
mkdir -p "$HOME"
if command -v startx >/dev/null 2>&1; then
  exec startx /usr/bin/uos-browser -- :0 -nocursor -dpi 160 vt7
fi
exec /usr/bin/uos-browser
LAUNCHEREOF
sudo chmod 0755 "$ROOT_MNT/usr/bin/uos-shell-launcher"

# Xresources — DPI for TV.
sudo mkdir -p "$ROOT_MNT/root" "$ROOT_MNT/home/uos"
printf 'Xft.dpi: 160\n' | sudo tee "$ROOT_MNT/root/.Xresources" "$ROOT_MNT/home/uos/.Xresources" >/dev/null 2>&1 || true

# Dedicated user for UI and deterministic credentials for non-interactive boot.
sudo chroot "$ROOT_MNT" /bin/bash -c "id -u uos >/dev/null 2>&1 || useradd -m -s /bin/bash -G audio,video,input,netdev,sudo uos" || true
sudo chroot "$ROOT_MNT" /bin/bash -c "echo 'root:uosroot1234' | chpasswd; echo 'uos:uos1234' | chpasswd; chsh -s /bin/bash root; chsh -s /bin/bash uos" || true

# X11 input config — use libinput for keyboard/mouse (fixes "No input driver specified").
sudo mkdir -p "$ROOT_MNT/etc/X11/xorg.conf.d"
sudo tee "$ROOT_MNT/etc/X11/xorg.conf.d/10-input.conf" >/dev/null <<'XINPEOF'
Section "InputClass"
    Identifier "Keyboard catchall"
    MatchIsKeyboard "on"
    Driver "libinput"
    Option "XkbLayout" "us"
EndSection

Section "InputClass"
    Identifier "Mouse catchall"
    MatchIsPointer "on"
    Driver "libinput"
EndSection
XINPEOF

# Minimal systemd units. Keep existing Armbian services intact for debug-full.
# uos-shell runs as root so Xorg can open vt7.
sudo tee "$ROOT_MNT/etc/systemd/system/uos-shell.service" >/dev/null <<'SHELLUNIT'
[Unit]
Description=UOS TV Shell
After=multi-user.target network-online.target luna-httpd.service
Wants=network-online.target luna-httpd.service

[Service]
Type=simple
User=root
Environment=HOME=/home/uos
Environment=XDG_RUNTIME_DIR=/run/user/1000
ExecStart=/usr/bin/uos-shell-launcher
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
SHELLUNIT

# stardustd with WebSocket bridge for Luna UI (ws://127.0.0.1:9090).
sudo tee "$ROOT_MNT/etc/systemd/system/stardustd.service" >/dev/null <<'BUSUNIT'
[Unit]
Description=UOS Stardust Bus
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/stardustd --ws-addr 127.0.0.1:9090
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
BUSUNIT

# Luna HTTP server — serves UI assets so browser can use same-origin WebSocket.
sudo tee "$ROOT_MNT/etc/systemd/system/luna-httpd.service" >/dev/null <<'HTTPUNIT'
[Unit]
Description=Luna HTTP Server
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/python3 -m http.server 8080 --directory /usr/share/uos/luna
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
HTTPUNIT

sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl enable stardustd.service luna-httpd.service uos-shell.service" || true
# Mask inputd — X11/libinput handles keyboard directly to avoid evdev grab conflicts.
sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl mask inputd.service 2>/dev/null || true" || true
sudo chroot "$ROOT_MNT" /bin/bash -c "systemctl enable NetworkManager.service 2>/dev/null || true" || true
# CSS TV-native polish: hide scrollbars, remove web-like cursors, glass cards.
if [[ -f "$ROOT_MNT/usr/share/uos/luna/css/shell.css" ]]; then
  sudo tee -a "$ROOT_MNT/usr/share/uos/luna/css/shell.css" >/dev/null <<'LUNACSS'

/* === TV-Native Polish — CI injected === */
::-webkit-scrollbar { display: none !important; width: 0 !important; height: 0 !important; }
* {
  scrollbar-width: none !important;
  -ms-overflow-style: none !important;
  outline: none !important;
  cursor: none !important;
}
.card {
  border-radius: var(--radius-lg) !important;
  border: 1px solid rgba(255,255,255,0.06) !important;
  background: linear-gradient(135deg, rgba(255,255,255,0.04), rgba(255,255,255,0.01)) !important;
  backdrop-filter: blur(10px) !important;
  transform: scale(1) !important;
}
.card:focus, .card:hover {
  border-color: rgba(247, 129, 102, 0.5) !important;
  background: linear-gradient(135deg, rgba(247,129,102,0.1), rgba(247,129,102,0.03)) !important;
  transform: scale(1.03) !important;
  box-shadow: 0 0 30px rgba(247,129,102,0.15) !important;
}
.section-label {
  font-size: 1rem !important;
  letter-spacing: 3px !important;
  padding-bottom: 8px !important;
  border-bottom: 1px solid rgba(255,255,255,0.06) !important;
  margin-bottom: 20px !important;
}
.card-grid { gap: 20px !important; padding: 4px !important; }
.card-icon { font-size: 2.5rem !important; margin-bottom: 12px !important; }
.card-title { font-size: 1rem !important; font-weight: 700 !important; }
.card-desc { font-size: 0.75rem !important; opacity: 0.6 !important; }
#topbar {
  background: linear-gradient(180deg, rgba(11,11,18,0.95), rgba(11,11,18,0.8)) !important;
  backdrop-filter: blur(15px) !important;
  border-bottom: 1px solid rgba(255,255,255,0.04) !important;
}
.footer { opacity: 0.3 !important; }
LUNACSS
fi

# Inject RTL8189FS WiFi driver if built by CI.
if [[ -f "$PROJECT_DIR/build/wifi/8189fs.ko" ]]; then
  echo "Installing RTL8189FS WiFi driver..."
  MODDIR="$ROOT_MNT/lib/modules/6.12.91-ophub/kernel/drivers/net/wireless"
  sudo mkdir -p "$MODDIR"
  sudo install -m 0644 "$PROJECT_DIR/build/wifi/8189fs.ko" "$MODDIR/8189fs.ko"
  printf '8189fs\n' | sudo tee "$ROOT_MNT/etc/modules-load.d/rtl8189fs.conf" >/dev/null
  sudo chroot "$ROOT_MNT" /bin/bash -c "depmod -a 6.12.91-ophub 2>/dev/null || depmod" || true
fi

# Inject Cog (WPE WebKit) if pre-built debs from CI.
COG_DEB_DIR="$PROJECT_DIR/build/cog-debs"
if [[ -f "$COG_DEB_DIR/cog.deb" ]]; then
  echo "Installing Cog + WPE WebKit (Debian trixie pre-built)..."
  sudo cp "$COG_DEB_DIR"/*.deb "$ROOT_MNT/tmp/"
  sudo chroot "$ROOT_MNT" /bin/bash -c "cd /tmp && dpkg --force-depends -i *.deb 2>&1 || true"
  sudo rm -f "$ROOT_MNT/tmp/"*.deb
  # Verify
  sudo chroot "$ROOT_MNT" /bin/bash -c "cog --version 2>&1 || ldd /usr/bin/cog 2>&1 | head -5" || echo "WARN: cog not runnable, but debs installed"
fi

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
