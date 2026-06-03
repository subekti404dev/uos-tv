# UOS TV Makefile
# ================
# Sistem Operasi TV Cerdas berbasis Armbian + Rust
#
# Quick Start:
#   make dev          - Check & test (macOS/Linux dev)
#   make build        - Cross-compile semua crate (aarch64)
#   make image        - Assembly disk image (.img)
#   make qemu         - Jalankan di QEMU (aarch64 emulation)
#   make bundle       - Create OTA update bundle
#
# Env:
#   CROSS_TARGET    - Rust target triple (default: aarch64-unknown-linux-musl)
#   UOS_IMAGE_SIZE  - Disk image size in MB (default: 8192)

.PHONY: help dev build build-release image qemu bundle test clean \
        docker-build docker-build-all docker-build-full docker-shell docker-exec \
        lint serve armbian-bootstrap dev-stack docker-cross-test ci-qemu-smoke \
        fetch-kernel build-cog qemu-quick overlay-alpine build-all

CROSS_TARGET  ?= aarch64-unknown-linux-musl
IMAGE_SIZE    ?= 8192
BUILD_DIR     ?= build
IMAGE         ?= $(BUILD_DIR)/uos-tv.img
QEMU_MEM      ?= 2048
QEMU_CORES    ?= 4

# ── Help ──────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# ── Development (host machine) ────────────────────────

dev: ## Check & test on host machine
	cargo check --workspace
	cargo test --workspace
	@echo "=== Dev OK ==="

serve: ## Serve Luna UI locally for browser testing
	@echo "Serving Luna UI at http://localhost:8080"
	@echo "Press Ctrl+C to stop"
	@cd luna && python3 -m http.server 8080 2>/dev/null || \
		cd luna && python -m SimpleHTTPServer 8080

lint: ## Run clippy
	cargo clippy --workspace -- -D warnings

fix: ## Auto-fix clippy warnings
	cargo clippy --workspace --fix --allow-dirty

# ── Rust Build ────────────────────────────────────────

build: ## Cross-compile semua crate (release, exclude lumind)
	@echo "=== Building UOS TV for $(CROSS_TARGET) ==="
	@echo "  (lumind excluded — needs native aarch64 for Smithay/C deps)"
	cargo build --release --target $(CROSS_TARGET) --workspace --exclude lumind
	@echo "=== Build complete ==="
	@find target/$(CROSS_TARGET)/release/ -maxdepth 1 -type f -executable | sort | while read bin; do \
		printf "  %-20s %-8s %s\n" "$$(basename $$bin)" "$$(ls -lh $$bin | awk '{print $$5}')" "$$(file $$bin | cut -d: -f2- | cut -c1-60)"; \
	done

build-all: ## Cross-compile ALL crates (needs full aarch64 sysroot for lumind)
	cargo build --release --target $(CROSS_TARGET) --workspace

build-debug: ## Cross-compile (debug mode)
	cargo build --target $(CROSS_TARGET)

check: ## Type-check saja (cepat)
	cargo check --workspace
	@echo "=== Check OK ==="

test: ## Run all tests
	cargo test --workspace

test-verbose: ## Run tests with output
	cargo test --workspace -- --nocapture

# ── Docker Cross-Compilation ──────────────────────────

docker-build: ## Build cross-compilation Docker image
	docker build -t uos-builder -f Dockerfile.cross .

docker-build-all: docker-build ## Cross-compile in Docker (all crates excl. lumind)
	docker run --rm -v "$(shell pwd):/work" uos-builder cargo build --release --target $(CROSS_TARGET) --workspace --exclude lumind

docker-build-full: docker-build ## Cross-compile ALL crates (needs aarch64 sysroot)
	docker run --rm -v "$(shell pwd):/work" uos-builder cargo build --release --target $(CROSS_TARGET) --workspace

docker-shell: docker-build ## Enter cross-compilation shell
	docker run --rm -it -v "$(shell pwd):/work" uos-builder bash

docker-exec: docker-build ## Run a command in the builder (CMD="...")
	docker run --rm -v "$(shell pwd):/work" uos-builder $(CMD)

# ── Image Assembly ────────────────────────────────────

image: build ## Create bootable disk image
	@echo "=== Creating UOS TV disk image ==="
	mkdir -p $(BUILD_DIR)
	./scripts/create-image.sh $(IMAGE) $(IMAGE_SIZE)

image-quick: ## Create image without rebuilding
	mkdir -p $(BUILD_DIR)
	./scripts/create-image.sh $(IMAGE) $(IMAGE_SIZE)

# ── QEMU ──────────────────────────────────────────────

qemu: image ## Build + image + run in QEMU
	@echo "=== Starting QEMU ==="
	chmod +x scripts/run-qemu.sh
	QEMU_MEM=$(QEMU_MEM) QEMU_CORES=$(QEMU_CORES) ./scripts/run-qemu.sh

qemu-headless: image ## QEMU (headless, SSH on :2222)
	chmod +x scripts/run-qemu.sh
	QEMU_MEM=$(QEMU_MEM) QEMU_CORES=$(QEMU_CORES) ./scripts/run-qemu.sh --headless

qemu-debug: image ## QEMU with GDB stub on :1234
	chmod +x scripts/run-qemu.sh
	QEMU_MEM=$(QEMU_MEM) QEMU_CORES=$(QEMU_CORES) ./scripts/run-qemu.sh --debug

qemu-shell: qemu-headless ## Start QEMU + connect SSH
	@sleep 3
	@ssh -o StrictHostKeyChecking=no -p 2222 root@localhost || echo "SSH failed — is QEMU running?"

# ── OTA Bundle ────────────────────────────────────────

bundle: ## Create OTA update bundle (set VERSION=x.y.z)
	chmod +x scripts/ota-create-bundle.sh
	./scripts/ota-create-bundle.sh $(VERSION)

# ── Cleaning ──────────────────────────────────────────

clean: ## Full clean
	cargo clean
	rm -rf $(BUILD_DIR)/
	@echo "=== Clean complete ==="

clean-image: ## Clean only image artifacts
	rm -rf $(BUILD_DIR)/uos-tv.img $(BUILD_DIR)/uefi-vars.fd

# ── Stats ──────────────────────────────────────────────

stats: ## Show project statistics
	@echo "=== UOS TV Statistics ==="
	@echo "Rust source:"
	@find crates -name "*.rs" | xargs wc -l | tail -1
	@echo "Luna UI:"
	@find luna -name "*.html" -o -name "*.css" -o -name "*.js" | xargs wc -l | tail -1
	@echo "Scripts:"
	@find scripts -name "*.sh" | xargs wc -l | tail -1
	@echo "Total:"
	@(find crates -name "*.rs"; find luna -name "*.html" -o -name "*.css" -o -name "*.js"; find scripts -name "*.sh") | xargs wc -l | tail -1

# ── Armbian Bootstrap ─────────────────────────────────

armbian-bootstrap: ## Download/build Armbian rootfs for aarch64
	chmod +x scripts/bootstrap-armbian.sh
	./scripts/bootstrap-armbian.sh $(BUILD_DIR)

# ── Dev Stack (local) ─────────────────────────────────

dev-stack: ## Run full UOS stack locally (dev mode)
	@echo "=== Starting UOS TV dev stack ==="
	@echo ""
	@echo "  stardustd  →  /tmp/uos-bus.sock + ws://127.0.0.1:9090"
	@echo "  logd       →  /tmp/uos-log.sock"
	@echo "  Luna UI    →  http://127.0.0.1:8080"
	@echo ""
	chmod +x scripts/dev-run.sh
	./scripts/dev-run.sh

docker-cross-test: docker-build ## Cross-compile + run tests in Docker
	@echo "=== Cross-compilation test in Docker ==="
	docker run --rm -v "$(shell pwd):/work" uos-builder sh -c \
		"cd /work && cargo build --release --target $(CROSS_TARGET) && cargo test --workspace"

ci-qemu-smoke: ## CI QEMU boot smoke test (expects prepared rootfs/kernel)
	chmod +x scripts/ci-qemu-smoke.sh
	./scripts/ci-qemu-smoke.sh

# ── QEMU Kernel ───────────────────────────────────────

fetch-kernel: ## Download aarch64 kernel + UEFI for QEMU
	chmod +x scripts/fetch-qemu-kernel.sh
	./scripts/fetch-qemu-kernel.sh $(BUILD_DIR)

qemu-quick: build fetch-kernel ## Quick boot via Alpine rootfs (no disk image)
	@echo "=== UOS TV Quick Boot ==="
	chmod +x scripts/run-qemu.sh scripts/overlay-alpine.sh
	@if [ ! -d "$(BUILD_DIR)/alpine-rootfs/etc" ]; then \
		echo "Extracting Alpine rootfs..."; \
		mkdir -p "$(BUILD_DIR)/alpine-rootfs"; \
		tar xzf "$(BUILD_DIR)/alpine-rootfs/alpine-rootfs.tar.gz" -C "$(BUILD_DIR)/alpine-rootfs" 2>/dev/null || echo "No rootfs tarball — run make fetch-kernel first"; \
	fi
	./scripts/overlay-alpine.sh "$(BUILD_DIR)/alpine-rootfs"
	./scripts/run-qemu.sh --quick

overlay-alpine: build ## Apply UOS overlay to Alpine rootfs
	chmod +x scripts/overlay-alpine.sh
	@if [ ! -d "$(BUILD_DIR)/alpine-rootfs/etc" ]; then \
		echo "Extracting Alpine rootfs..."; \
		mkdir -p "$(BUILD_DIR)/alpine-rootfs"; \
		tar xzf "$(BUILD_DIR)/alpine-rootfs/alpine-rootfs.tar.gz" -C "$(BUILD_DIR)/alpine-rootfs" 2>/dev/null || echo "No rootfs tarball — run make fetch-kernel first"; \
	fi
	./scripts/overlay-alpine.sh "$(BUILD_DIR)/alpine-rootfs"

# ── WPE WebKit Cog ────────────────────────────────────

build-cog: ## Cross-compile WPE WebKit Cog for aarch64
	chmod +x scripts/build-cog.sh
	./scripts/build-cog.sh $(BUILD_DIR)/cog
