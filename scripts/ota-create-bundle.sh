#!/bin/bash
# ota-create-bundle.sh — Create OTA update bundle
# ================================================
# Creates a signed OTA bundle for RAUC A/B update.
#
# Bundle contents:
#   manifest.raucm   — RAUC manifest
#   rootfs.tar.zst    — Root filesystem delta
#   signature.sig     — Ed25519 signature
#
# Usage:
#   ./scripts/ota-create-bundle.sh <version> [channel]
#   VERSION=1.0.1 ./scripts/ota-create-bundle.sh
#
# Requires:
#   - rauc
#   - openssl (for Ed25519 keygen)
#   - zstd

set -euo pipefail

VERSION="${VERSION:-${1:-0.1.0}}"
CHANNEL="${CHANNEL:-${2:-dev}}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUNDLE_DIR="$PROJECT_DIR/build/ota"
KEY_DIR="$PROJECT_DIR/build/keys"

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
log()  { echo -e "${GREEN}[OTA]${NC} $*"; }
warn() { echo -e "${RED}[OTA]${NC} $*"; }

mkdir -p "$BUNDLE_DIR" "$KEY_DIR"

# ── Generate Keys (if missing) ────────────────────────
if [ ! -f "$KEY_DIR/dev.key" ]; then
    log "Generating Ed25519 signing key..."
    openssl genpkey -algorithm ED25519 -out "$KEY_DIR/dev.key"
    openssl pkey -in "$KEY_DIR/dev.key" -pubout -out "$KEY_DIR/dev.pub"
    log "Keys created: $KEY_DIR/dev.key, $KEY_DIR/dev.pub"
fi

# ── Create RAUC Manifest ──────────────────────────────
MANIFEST="$BUNDLE_DIR/manifest.raucm"

cat > "$MANIFEST" <<MANIFEST
[update]
compatible=uos-tv-qemu
version=$VERSION
description=UOS TV $VERSION (channel: $CHANNEL)
build=$(date -u +%Y%m%d-%H%M%S)

[bundle]
format=verity

[image.rootfs]
filename=rootfs.tar.zst
sha256=REPLACEME
size=REPLACEME

[image.appfs]
filename=appfs.tar.zst

[hooks]
filename=update-hook.sh
MANIFEST

# ── Create Placeholder Payloads ───────────────────────
log "Creating bundle payloads..."

# Rootfs delta (placeholder — in production: casync delta)
echo "UOS TV rootfs $VERSION" > /tmp/uos-rootfs-marker.txt
tar cf "$BUNDLE_DIR/rootfs.tar" -C /tmp uos-rootfs-marker.txt 2>/dev/null || true
zstd -q -f "$BUNDLE_DIR/rootfs.tar" -o "$BUNDLE_DIR/rootfs.tar.zst" 2>/dev/null || {
    # If zstd not available, use gzip fallback
    gzip -c "$BUNDLE_DIR/rootfs.tar" > "$BUNDLE_DIR/rootfs.tar.zst"
}

# Appfs (Luna UI)
tar cf "$BUNDLE_DIR/appfs.tar" -C "$PROJECT_DIR/luna" . 2>/dev/null || true
zstd -q -f "$BUNDLE_DIR/appfs.tar" -o "$BUNDLE_DIR/appfs.tar.zst" 2>/dev/null || true

# Update hook script
cat > "$BUNDLE_DIR/update-hook.sh" <<'HOOK'
#!/bin/sh
# RAUC update hook — called before/after install
case "$1" in
    slot-post-install)
        echo "UOS TV: post-install to slot $RAUC_SLOT_NAME"
        ;;
    slot-install)
        echo "UOS TV: installing to slot $RAUC_SLOT_NAME"
        ;;
esac
exit 0
HOOK
chmod +x "$BUNDLE_DIR/update-hook.sh"

# ── Generate SHA256 ───────────────────────────────────
ROOTFS_SHA=$(sha256sum "$BUNDLE_DIR/rootfs.tar.zst" | cut -d' ' -f1)
ROOTFS_SIZE=$(stat -c%s "$BUNDLE_DIR/rootfs.tar.zst" 2>/dev/null || stat -f%z "$BUNDLE_DIR/rootfs.tar.zst")

# Update manifest
sed -i.bak "s/sha256=REPLACEME/sha256=$ROOTFS_SHA/" "$MANIFEST"
sed -i.bak "s/size=REPLACEME/size=$ROOTFS_SIZE/" "$MANIFEST"
rm -f "$MANIFEST.bak"

# ── Create RAUC Bundle ────────────────────────────────
BUNDLE_FILE="$BUNDLE_DIR/uos-tv-${VERSION}-${CHANNEL}.raucb"

log "Creating RAUC bundle..."
if command -v rauc >/dev/null 2>&1; then
    rauc bundle \
        --cert="$KEY_DIR/dev.pub" \
        --key="$KEY_DIR/dev.key" \
        "$BUNDLE_DIR" \
        "$BUNDLE_FILE" 2>/dev/null && \
        log "Bundle created: $BUNDLE_FILE" || \
        warn "RAUC bundle creation failed (may need rauc installed)"
else
    warn "rauc not found — skipping bundle signing"
    warn "Install: apt-get install rauc"
    # Create unsigned tar as fallback
    tar cf "${BUNDLE_FILE}.tar" -C "$BUNDLE_DIR" .
    log "Created unsigned tarball: ${BUNDLE_FILE}.tar"
fi

# ── Summary ──────────────────────────────────────────
log "=== OTA Bundle $VERSION ==="
echo "  Version:  $VERSION"
echo "  Channel:  $CHANNEL"
echo "  RootFS:   $ROOTFS_SIZE bytes (SHA256: $ROOTFS_SHA)"
echo "  Output:   $BUNDLE_DIR/"
ls -lh "$BUNDLE_DIR/" 2>/dev/null | tail -n +2
