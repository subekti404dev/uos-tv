//! lumind — Lumina Display Server
//! ================================
//! Minimal ARM64/Linux compositor launcher for UOS TV.
//!
//! Production target:
//!   - Detect DRM/KMS + input availability
//!   - Prepare Wayland/Cog environment
//!   - Launch Cog WPE WebKit with Luna UI
//!   - Keep supervisor-visible process alive
//!
//! Full Smithay compositor was deferred because Smithay 0.7 API changed
//! significantly. This implementation is a pragmatic P1 bridge: Cog can render
//! on real DRM hardware, while lumind owns lifecycle and hardware checks.

#[cfg(target_os = "linux")]
use std::time::Duration;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    tracing::info!("lumind v{} starting...", env!("CARGO_PKG_VERSION"));

    #[cfg(target_os = "linux")]
    run_linux();

    #[cfg(not(target_os = "linux"))]
    run_dev_stub();
}

#[cfg(target_os = "linux")]
fn run_linux() {
    tracing::info!("Lumina display manager starting (Linux/DRM mode)...");

    let drm_devices = find_drm_devices();
    if drm_devices.is_empty() {
        tracing::warn!("No /dev/dri/card* devices found — running in headless/QEMU-safe mode");
    } else {
        tracing::info!("DRM devices: {}", drm_devices.join(", "));
    }

    let input_devices = find_input_devices();
    if input_devices.is_empty() {
        tracing::warn!("No /dev/input/event* devices found");
    } else {
        tracing::info!("Input devices detected: {}", input_devices.len());
    }

    let luna_url = std::env::var("LUNA_URL")
        .unwrap_or_else(|_| "file:///usr/share/uos/luna/index.html".to_string());
    let cog_platform = std::env::var("COG_PLATFORM").unwrap_or_else(|_| {
        if drm_devices.is_empty() {
            "headless".to_string()
        } else {
            "drm".to_string()
        }
    });

    if std::path::Path::new("/usr/bin/cog").exists() {
        tracing::info!("/usr/bin/cog found — cog service will render Luna UI");
    } else {
        tracing::warn!("/usr/bin/cog not found — Luna UI renderer unavailable");
    }

    tracing::info!("lumind ready (platform={cog_platform}, url={luna_url})");

    // Keep process alive. monitord handles lifecycle/restart.
    loop {
        std::thread::sleep(Duration::from_secs(60));
        tracing::debug!("lumind heartbeat");
    }
}

#[cfg(target_os = "linux")]
fn find_drm_devices() -> Vec<String> {
    std::fs::read_dir("/dev/dri")
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") {
                Some(entry.path().display().to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn find_input_devices() -> Vec<String> {
    std::fs::read_dir("/dev/input")
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("event") {
                Some(entry.path().display().to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn run_dev_stub() {
    tracing::info!("=== Lumina Dev Stub ===");
    tracing::info!("Full display path requires Linux/ARM64 + DRM/KMS.");
    tracing::info!("Cog integration is packaged under /usr/bin/cog + /usr/share/uos/cog.ini.");
    tracing::info!("Stub exiting normally.");
}
