#!/bin/bash
# overlay-alpine.sh — Apply UOS TV overlay onto Alpine rootfs
# ==============================================================
set -euo pipefail

ROOTFS="${1:?Usage: $0 <rootfs_dir>}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Auto-detect Rust target directory
RUST_TARGET_DIR="$PROJECT_DIR/target/aarch64-unknown-linux-musl/release"
[ ! -d "$RUST_TARGET_DIR" ] && RUST_TARGET_DIR="$PROJECT_DIR/target/release"

GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[OVERLAY]${NC} $*"; }
info() { echo -e "${CYAN}  →${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }

[ -d "$ROOTFS" ] || { echo "Error: $ROOTFS not found"; exit 1; }

# ── Directory Structure ──────────────────────────────
log "Creating UOS directory structure..."
mkdir -p "$ROOTFS"/{usr/bin,usr/lib,usr/share/uos/{services.d,luna}}
mkdir -p "$ROOTFS"/{etc/init.d,etc/runlevels/default}
mkdir -p "$ROOTFS"/{data/{apps,downloads,logs,config},var/log/uos,run/uos,tmp}
chmod 1777 "$ROOTFS/tmp" 2>/dev/null || warn "Cannot chmod tmp"

# ── Copy UOS Binaries ────────────────────────────────
log "Copying UOS service binaries..."
for bin in inis monitord logd stardustd otad netmd audiod pkgd inputd dispald notifd powermand devmand casync-rs rauc-rs update-verify; do
    src="$RUST_TARGET_DIR/$bin"
    if [ -f "$src" ]; then
        cp "$src" "$ROOTFS/usr/bin/$bin"
        chmod 755 "$ROOTFS/usr/bin/$bin" 2>/dev/null || true
        info "✓ $bin"
    else
        warn "✗ $bin"
    fi
done

# ── Copy Luna UI ─────────────────────────────────────
log "Copying Luna UI shell..."
if [ -d "$PROJECT_DIR/luna" ]; then
    cp -r "$PROJECT_DIR/luna"/* "$ROOTFS/usr/share/uos/luna/"
    info "✓ Luna UI"
fi

# ── Copy Service Manifests ───────────────────────────
log "Copying service manifests..."
if [ -d "$PROJECT_DIR/configs/services.d" ]; then
    cp "$PROJECT_DIR/configs/services.d"/*.yaml "$ROOTFS/usr/share/uos/services.d/" 2>/dev/null || true
    info "✓ $(ls "$PROJECT_DIR/configs/services.d"/*.yaml 2>/dev/null | wc -l | xargs) manifests"
fi

# ── Monitord Config ──────────────────────────────────
log "Creating monitord config..."
cat > "$ROOTFS/usr/share/uos/monitord.yaml" <<'EOF'
services_dir: /usr/share/uos/services.d
log_dir: /var/log/uos
binary_search_path:
  - /usr/bin
  - /usr/local/bin
startup_timeout_sec: 30
crash_window_sec: 60
max_crashes: 5
EOF

# ── System Config ────────────────────────────────────
log "Writing system configs..."
echo "uos-tv" > "$ROOTFS/etc/hostname"

cat > "$ROOTFS/etc/hosts" <<'EOF'
127.0.0.1   localhost
127.0.1.1   uos-tv
EOF

cat > "$ROOTFS/etc/resolv.conf" <<'EOF'
nameserver 8.8.8.8
nameserver 1.1.1.1
EOF

# ── UOS Init Script ──────────────────────────────────
log "Installing UOS init script..."
UOS_INIT="$PROJECT_DIR/configs/alpine-overlay/etc/init.d/uos-init"
if [ -f "$UOS_INIT" ]; then
    mkdir -p "$ROOTFS/etc/init.d"
    cp "$UOS_INIT" "$ROOTFS/etc/init.d/uos-init"
    chmod +x "$ROOTFS/etc/init.d/uos-init"
    info "✓ uos-init (OpenRC)"
else
    # Fallback: BusyBox init style
    warn "Creating minimal init..."
    cat > "$ROOTFS/etc/init.d/uos-init" <<'INITSCRIPT'
#!/sbin/openrc-run
depend() { need net localmount; after bootmisc; }
start() {
    ebegin "Starting UOS TV"
    mkdir -p /run/uos /var/log/uos
    [ -x /usr/bin/stardustd ] && /usr/bin/stardustd --socket /run/uos/bus.sock --ws-addr 127.0.0.1:9090 &
    sleep 1
    [ -x /usr/bin/logd ] && /usr/bin/logd --socket /run/uos/log.sock --log-dir /var/log/uos &
    [ -x /usr/bin/monitord ] && /usr/bin/monitord --config /usr/share/uos/monitord.yaml &
    eend 0
}
stop() {
    ebegin "Stopping UOS TV"
    killall monitord stardustd logd 2>/dev/null || true
    eend 0
}
INITSCRIPT
    chmod +x "$ROOTFS/etc/init.d/uos-init"
fi

# ── Enable at boot ───────────────────────────────────
mkdir -p "$ROOTFS/etc/runlevels/default"
ln -sf /etc/init.d/uos-init "$ROOTFS/etc/runlevels/default/uos-init" 2>/dev/null || true
info "✓ Enabled at default runlevel"

# ── Console autologin (ttyAMA0) ──────────────────────
if [ -f "$ROOTFS/etc/inittab" ]; then
    sed -i 's|ttyAMA0:.*|ttyAMA0::respawn:/sbin/getty -L 115200 ttyAMA0 vt100|' \
        "$ROOTFS/etc/inittab" 2>/dev/null || true
fi

# ── Summary ──────────────────────────────────────────
log "=== Overlay Complete ==="
echo "  RootFS:    $ROOTFS"
echo "  Binaries:  $(ls "$ROOTFS/usr/bin/" 2>/dev/null | wc -l | xargs) services"
echo "  Luna UI:   $ROOTFS/usr/share/uos/luna/"
echo "  Init:      /etc/init.d/uos-init"
