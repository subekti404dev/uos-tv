#!/bin/bash
# dev-run.sh — Run UOS TV services locally for development
# ==========================================================
# Starts stardustd (IPC broker + WebSocket), logd, and serves
# Luna UI over HTTP. Test the Lua shell at http://127.0.0.1:8080
#
# Prerequisites: cargo build (debug mode)
# Cleanup: Ctrl+C kills all processes

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_DIR/build"

GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[UOS]${NC} $*"; }
info() { echo -e "${CYAN}  →${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }

# ── Cleanup ───────────────────────────────────────────
cleanup() {
    log "Shutting down UOS TV services..."
    for pid in "${PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    log "All services stopped."
}
trap cleanup EXIT INT TERM
PIDS=()

mkdir -p "$BUILD_DIR" /tmp/uos

# ── Build (if needed) ──────────────────────────────────
log "Checking build..."
cd "$PROJECT_DIR"
if [ ! -f "target/debug/stardustd" ]; then
    info "Building debug binaries..."
    cargo build 2>&1 | tail -3
fi

# ── 1. stardustd (IPC broker + WebSocket bridge) ──────
log "Starting stardustd (IPC + WebSocket bridge)..."
rm -f /tmp/uos-bus.sock
RUST_LOG=stardust=debug cargo run -p stardust -- \
    --socket /tmp/uos-bus.sock \
    --ws-addr 127.0.0.1:9090 &
PIDS+=($!)
info "stardustd PID ${PIDS[-1]} — ws://127.0.0.1:9090"
sleep 1

# ── 2. logd (logging daemon) ──────────────────────────
log "Starting logd..."
mkdir -p /tmp/uos/logs
rm -f /tmp/uos-log.sock
RUST_LOG=logd=debug ./target/debug/logd \
    --socket /tmp/uos-log.sock \
    --log-dir /tmp/uos/logs &
PIDS+=($!)
info "logd PID ${PIDS[-1]} — /tmp/uos-log.sock"
sleep 0.3

# ── 3. devmand (device manager) ───────────────────────
log "Starting devmand..."
RUST_LOG=devmand=info ./target/debug/devmand &
PIDS+=($!)
info "devmand PID ${PIDS[-1]}"

# ── 4. netmd (network monitor) ────────────────────────
log "Starting netmd..."
RUST_LOG=netmd=info ./target/debug/netmd &
PIDS+=($!)
info "netmd PID ${PIDS[-1]}"

# ── 5. dispald (display/backlight) ────────────────────
log "Starting dispald..."
RUST_LOG=dispald=info ./target/debug/dispald &
PIDS+=($!)
info "dispald PID ${PIDS[-1]}"

# ── 6. inputd (remote input handler) ──────────────────
log "Starting inputd..."
RUST_LOG=inputd=info ./target/debug/inputd &
PIDS+=($!)
info "inputd PID ${PIDS[-1]}"

# ── 7. audiod (audio manager) ─────────────────────────
log "Starting audiod..."
RUST_LOG=audiod=info ./target/debug/audiod &
PIDS+=($!)
info "audiod PID ${PIDS[-1]}"

# ── 8. powermand (power management) ───────────────────
log "Starting powermand..."
RUST_LOG=powermand=info ./target/debug/powermand &
PIDS+=($!)
info "powermand PID ${PIDS[-1]}"

# ── 9. notifd (notifications) ─────────────────────────
log "Starting notifd..."
RUST_LOG=notifd=info ./target/debug/notifd &
PIDS+=($!)
info "notifd PID ${PIDS[-1]}"

# ── 10. pkgd (package manager) ────────────────────────
log "Starting pkgd..."
RUST_LOG=pkgd=info ./target/debug/pkgd &
PIDS+=($!)
info "pkgd PID ${PIDS[-1]}"

# ── Summary ───────────────────────────────────────────
echo ""
log "=== UOS TV Services Running ==="
echo ""
echo "   IPC Bus:      /tmp/uos-bus.sock"
echo "   WebSocket:    ws://127.0.0.1:9090"
echo "   Log Socket:   /tmp/uos-log.sock"
echo "   Luna UI:      http://127.0.0.1:8080"
echo "   PIDs:         ${PIDS[*]}"
echo ""

# ── Luna UI ───────────────────────────────────────────
log "Serving Luna UI at http://127.0.0.1:8080"
echo "   Press Ctrl+C to stop all services"
echo ""
cd "$PROJECT_DIR/luna"
python3 -m http.server 8080 2>/dev/null &
PIDS+=($!)
cd "$PROJECT_DIR"

# ── Wait ──────────────────────────────────────────────
for pid in "${PIDS[@]}"; do
    wait "$pid" 2>/dev/null || true
done
