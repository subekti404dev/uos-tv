//! UOS TV — WiFi module loader daemon
//!
//! One-shot service: loads SDIO WiFi kernel modules at boot,
//! waits for wlan0 interface, publishes "wifi.ready" event.
//!
//! Module loading strategy:
//!   1. Read kernel release from /proc/version or uname
//!   2. Check /sys/bus/sdio/devices/ for Realtek/Broadcom SDIO device
//!   3. Load matching .ko — try versioned paths first, then legacy flat
//!   4. Wait up to 10s for wlan0 to appear
//!   5. Publish stardust event and exit

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    // ── Determine kernel version ──
    let kernel_ver = kernel_release();
    let module_base = format!("/lib/modules/{}", kernel_ver);
    println!("wifid: kernel release = {}", kernel_ver);

    // ── Detect SDIO WiFi chip ──
    let chip = detect_sdio_wifi();
    if chip.is_none() {
        eprintln!("wifid: no SDIO WiFi device found — exiting");
        std::process::exit(0);
    }
    let (chip_name, ko_name) = chip.unwrap();
    println!("wifid: detected {} → loading {}", chip_name, ko_name);

    // ── Load kernel module ──
    // Priority: 1) versioned extra/  2) versioned base  3) legacy flat
    let candidates = vec![
        format!("{}/extra/{}", module_base, ko_name),
        format!("{}/{}", module_base, ko_name),
        format!("/lib/modules/{}", ko_name), // legacy flat path
    ];

    let mut loaded = false;
    for ko_path in &candidates {
        if Path::new(ko_path).exists() {
            println!("wifid: found {}", ko_path);
            match insmod(ko_path) {
                Ok(()) => {
                    loaded = true;
                    println!("wifid: loaded {}", ko_name);
                    break;
                }
                Err(e) => eprintln!("wifid: insmod {} failed: {}", ko_path, e),
            }
        }
    }

    if !loaded {
        // Try modprobe fallback (searches under /lib/modules/<release>/)
        let name = ko_name.trim_end_matches(".ko");
        eprintln!("wifid: direct insmod failed, trying modprobe {}...", name);
        let status = Command::new("modprobe").arg(name).status();
        if status.map(|s| s.success()).unwrap_or(false) {
            println!("wifid: modprobe {} succeeded", name);
        } else {
            eprintln!("wifid: all load methods failed for {}", ko_name);
            eprintln!("wifid: searched: {:?}", candidates);
            std::process::exit(1);
        }
    }

    // ── Wait for wlan0 ──
    println!("wifid: waiting for wlan0...");
    for _ in 0..20 {
        if Path::new("/sys/class/net/wlan0").exists() {
            println!("wifid: wlan0 ready!");
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    if !Path::new("/sys/class/net/wlan0").exists() {
        eprintln!("wifid: wlan0 did not appear — timeout");
    }

    // ── Publish stardust event ──
    publish_ready();
}

/// Read kernel release string (e.g., "6.6.63-0-lts") from /proc/version
/// or from uname -r as fallback.
fn kernel_release() -> String {
    // Try /proc/sys/kernel/osrelease first (simplest)
    if let Ok(s) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    // Fallback: uname -r
    if let Ok(out) = Command::new("uname").arg("-r").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    // Last resort: parse /proc/version
    if let Ok(ver) = fs::read_to_string("/proc/version") {
        // Linux version 6.6.63-0-lts (...) ...
        if let Some(start) = ver.find("version ") {
            let rest = &ver[start + 8..];
            if let Some(end) = rest.find(' ') {
                return rest[..end].to_string();
            }
        }
    }
    "unknown".to_string()
}

/// Detect SDIO WiFi chip via /sys/bus/sdio/devices/
fn detect_sdio_wifi() -> Option<(&'static str, &'static str)> {
    let sdio_dir = "/sys/bus/sdio/devices";
    if let Ok(entries) = fs::read_dir(sdio_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let vendor = fs::read_to_string(path.join("vendor"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let device = fs::read_to_string(path.join("device"))
                .unwrap_or_default()
                .trim()
                .to_string();

            match (vendor.as_str(), device.as_str()) {
                // Realtek SDIO WiFi — covers RTL8189ES, RTL8189FS, RTL8723BS
                ("0x024c", _) => {
                    let modalias = fs::read_to_string(path.join("modalias")).unwrap_or_default();
                    if modalias.contains("rtl8723") {
                        return Some(("RTL8723BS", "r8723bs.ko"));
                    }
                    if modalias.contains("rtl8189") {
                        return Some(("RTL8189ES/FS", "8189es.ko"));
                    }
                    return Some(("Realtek SDIO", "8189es.ko"));
                }
                // Broadcom SDIO WiFi
                ("0x02d0", _) => {
                    return Some(("Broadcom SDIO", "brcmfmac.ko"));
                }
                _ => {}
            }
        }
    }
    None
}

fn insmod(path: &str) -> Result<(), String> {
    let output = Command::new("insmod")
        .arg(path)
        .output()
        .map_err(|e| format!("insmod: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("File exists") || stderr.contains("already") {
            Ok(()) // module already loaded
        } else {
            Err(stderr.trim().to_string())
        }
    }
}

fn publish_ready() {
    let _ = std::process::Command::new("stardust")
        .args([
            "pub",
            "network.wifi_ready",
            r#"{"interface":"wlan0","status":"ready"}"#,
        ])
        .spawn();
}
