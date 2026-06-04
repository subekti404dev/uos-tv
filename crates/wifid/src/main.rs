//! UOS TV — WiFi module loader daemon
//!
//! One-shot service: loads SDIO WiFi kernel modules at boot,
//! waits for wlan0 interface, publishes "wifi.ready" event.
//!
//! Module loading strategy:
//!   1. Check /sys/bus/sdio/devices/ for Realtek SDIO device
//!   2. Load matching .ko from /lib/modules/
//!   3. Wait up to 10s for wlan0 to appear
//!   4. Publish stardust event and exit

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    let module_dir = "/lib/modules";

    // ── Detect SDIO WiFi chip ──
    let chip = detect_sdio_wifi();
    if chip.is_none() {
        eprintln!("wifid: no SDIO WiFi device found — exiting");
        std::process::exit(0);
    }
    let (chip_name, ko_name) = chip.unwrap();
    println!("wifid: detected {} → loading {}", chip_name, ko_name);

    // ── Load kernel module ──
    let ko_path = format!("{}/{}", module_dir, ko_name);
    if Path::new(&ko_path).exists() {
        if let Err(e) = insmod(&ko_path) {
            eprintln!("wifid: failed to load {}: {}", ko_name, e);
            std::process::exit(1);
        }
        println!("wifid: loaded {}", ko_name);
    } else {
        eprintln!("wifid: module {} not found at {}", ko_name, ko_path);
        eprintln!("wifid: trying modprobe fallback...");
        if Command::new("modprobe")
            .arg(ko_name.trim_end_matches(".ko"))
            .status()
            .is_err()
        {
            eprintln!("wifid: modprobe also failed — continuing");
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

fn detect_sdio_wifi() -> Option<(&'static str, &'static str)> {
    let sdio_dir = "/sys/bus/sdio/devices";
    if let Ok(entries) = fs::read_dir(sdio_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Read vendor and device IDs
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
                    // Check which Realtek variant via modalias
                    let modalias = fs::read_to_string(path.join("modalias"))
                        .unwrap_or_default();
                    if modalias.contains("rtl8723") {
                        return Some(("RTL8723BS", "r8723bs.ko"));
                    }
                    if modalias.contains("rtl8189") {
                        // Both 8189ES and 8189FS share similar chip ID
                        // Try 8189es first, then 8189fs
                        return Some(("RTL8189ES/FS", "8189es.ko"));
                    }
                    // Fallback: try all
                    return Some(("Realtek SDIO WiFi", "8189es.ko"));
                }
                // Broadcom SDIO WiFi
                ("0x02d0", _) => {
                    return Some(("Broadcom SDIO WiFi", "brcmfmac.ko"));
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
        // Module already loaded is OK
        if stderr.contains("File exists") || stderr.contains("already") {
            Ok(())
        } else {
            Err(stderr.trim().to_string())
        }
    }
}

fn publish_ready() {
    // Best-effort stardust publish — don't fail if bus isn't up yet
    let _ = std::process::Command::new("stardust")
        .args([
            "pub",
            "network.wifi_ready",
            r#"{"interface":"wlan0","status":"ready"}"#,
        ])
        .spawn();
}
