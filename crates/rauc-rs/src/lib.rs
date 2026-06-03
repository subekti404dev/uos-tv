//! rauc-rs — RAUC (Robust Auto-Update Controller) client
//! ======================================================
//!
//! RAUC adalah framework update untuk embedded Linux dengan
//! dukungan A/B dual-slot, atomic update, dan automatic rollback.
//!
//! Crate ini menyediakan Rust interface ke RAUC CLI dan service.
//! Untuk v1, kita menggunakan RAUC command-line interface
//! karena RAUC belum memiliki D-Bus API yang stabil.
//!
//! API:
//!   - status()      → info slot A/B saat ini
//!   - install()     → install bundle ke slot inaktif
//!   - mark_good()   → tandai slot aktif sebagai "good"
//!   - mark_bad()    → tandai slot sebagai "bad" (force rollback)
//!
//! Slot naming convention:
//!   - rootfs.0  = slot A
//!   - rootfs.1  = slot B
//!   - boot.0    = bootloader slot A
//!   - boot.1    = bootloader slot B

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RAUC command failed: {0}")]
    CommandFailed(String),

    #[error("RAUC status parse error: {0}")]
    StatusParse(String),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid slot: {0}")]
    InvalidSlot(String),

    #[error("No inactive slot available for update")]
    NoInactiveSlot,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Status dari satu slot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlotStatus {
    /// Nama slot (contoh: "rootfs.0")
    pub name: String,

    /// Device block (contoh: "/dev/mmcblk0p2")
    pub device: String,

    /// Tipe slot: "raw", "ext4", "squashfs", dll.
    #[serde(rename = "type")]
    pub slot_type: String,

    /// Status boot: "good", "bad", "active"
    pub boot_status: String,

    /// Versi yang terinstall di slot ini
    pub version: Option<String>,

    /// Apakah ini slot yang sedang aktif (boot)
    pub active: bool,
}

/// Status sistem RAUC secara keseluruhan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaucStatus {
    /// Slot yang tersedia
    pub slots: Vec<SlotStatus>,

    /// Status operasi saat ini (idle / installing)
    pub operation: String,

    /// Versi yang sedang berjalan (compatible)
    pub compatible: Option<String>,

    /// Variant yang didukung
    pub variant: Option<String>,

    /// Pesan terakhir
    pub last_error: Option<String>,
}

/// Opsi untuk install bundle.
pub struct InstallOptions {
    /// Abaikan kompatibilitas check
    pub ignore_compatible: bool,

    /// Progress callback: (percentage, message)
    pub progress_callback: Option<Box<dyn Fn(u32, &str) + Send + Sync>>,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            ignore_compatible: false,
            progress_callback: None,
        }
    }
}

impl std::fmt::Debug for InstallOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallOptions")
            .field("ignore_compatible", &self.ignore_compatible)
            .field(
                "progress_callback",
                &self.progress_callback.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

/// Client RAUC.
///
/// Berkomunikasi dengan RAUC service via:
///   1. `rauc status --output-format=json` untuk status
///   2. `rauc install <bundle>` untuk install
///   3. `rauc status mark-good` / `mark-bad` untuk menandai
pub struct RaucClient {
    /// Path ke binary rauc
    rauc_bin: String,
}

impl RaucClient {
    /// Buat client baru. Binary rauc default: /usr/bin/rauc.
    pub fn new() -> Self {
        Self {
            rauc_bin: "/usr/bin/rauc".to_string(),
        }
    }

    /// Buat dengan path binary kustom.
    pub fn with_binary(rauc_bin: impl Into<String>) -> Self {
        Self {
            rauc_bin: rauc_bin.into(),
        }
    }

    /// Dapatkan status sistem RAUC.
    pub fn status(&self) -> Result<RaucStatus> {
        let output = Command::new(&self.rauc_bin)
            .args(["status", "--output-format=json"])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed(stderr.to_string()));
        }

        let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        // Parse ke struktur internal
        self.parse_status_json(&status)
    }

    /// Parse JSON output RAUC ke RaucStatus.
    fn parse_status_json(&self, json: &serde_json::Value) -> Result<RaucStatus> {
        let slots: Vec<SlotStatus> = json
            .get("slots")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|slot| SlotStatus {
                        name: slot["name"].as_str().unwrap_or("").to_string(),
                        device: slot["device"].as_str().unwrap_or("").to_string(),
                        slot_type: slot["type"].as_str().unwrap_or("raw").to_string(),
                        boot_status: slot["boot_status"]
                            .as_str()
                            .unwrap_or("inactive")
                            .to_string(),
                        version: slot["version"].as_str().map(|s| s.to_string()),
                        active: slot["boot_status"].as_str() == Some("active")
                            || slot["state"].as_str() == Some("active"),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(RaucStatus {
            slots,
            operation: json["operation"].as_str().unwrap_or("idle").to_string(),
            compatible: json["compatible"].as_str().map(|s| s.to_string()),
            variant: json["variant"].as_str().map(|s| s.to_string()),
            last_error: json["last_error"].as_str().map(|s| s.to_string()),
        })
    }

    /// Install bundle OTA ke slot inaktif.
    ///
    /// Bundle harus berupa file .raucb (RAUC bundle) yang sudah
    /// ditandatangani dan diverifikasi sebelum dipanggil.
    pub fn install(&self, bundle_path: &str, options: &InstallOptions) -> Result<()> {
        let mut cmd = Command::new(&self.rauc_bin);
        cmd.arg("install");

        if options.ignore_compatible {
            cmd.arg("--ignore-compatible");
        }

        cmd.arg(bundle_path);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Bundle installed successfully: {}", bundle_path);
        Ok(())
    }

    /// Tandai slot yang sedang aktif sebagai "good".
    ///
    /// Ini harus dipanggil setelah boot berhasil ke slot baru.
    /// Jika tidak dipanggil dalam batas waktu, RAUC akan rollback.
    pub fn mark_good(&self) -> Result<()> {
        let output = Command::new(&self.rauc_bin)
            .args(["status", "mark-good"])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Slot marked as good");
        Ok(())
    }

    /// Tandai slot sebagai "bad" — memicu rollback.
    pub fn mark_bad(&self, slot_name: Option<&str>) -> Result<()> {
        let mut args = vec!["status", "mark-bad"];

        if let Some(name) = slot_name {
            args.push(name);
        }

        let output = Command::new(&self.rauc_bin).args(&args).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Slot marked as bad");
        Ok(())
    }

    /// Dapatkan informasi tentang slot yang tersedia untuk update.
    /// Return slot yang tidak aktif (target install).
    pub fn get_inactive_slot(&self) -> Result<SlotStatus> {
        let status = self.status()?;

        status
            .slots
            .iter()
            .find(|s| !s.active)
            .cloned()
            .ok_or(Error::NoInactiveSlot)
    }

    /// Dapatkan slot yang sedang aktif.
    pub fn get_active_slot(&self) -> Result<SlotStatus> {
        let status = self.status()?;

        status
            .slots
            .iter()
            .find(|s| s.active)
            .cloned()
            .ok_or(Error::InvalidSlot("No active slot found".into()))
    }

    /// Cek apakah ada operasi yang sedang berlangsung.
    pub fn is_busy(&self) -> Result<bool> {
        let status = self.status()?;
        Ok(status.operation != "idle")
    }

    /// Dapatkan versi yang terinstall di slot aktif.
    pub fn current_version(&self) -> Result<Option<String>> {
        let status = self.status()?;
        let active = status.slots.iter().find(|s| s.active);
        Ok(active.and_then(|s| s.version.clone()))
    }

    /// Verifikasi bundle signature tanpa install.
    pub fn verify_bundle(&self, bundle_path: &str) -> Result<()> {
        let output = Command::new(&self.rauc_bin)
            .args(["info", "--output-format=json", bundle_path])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Bundle verified: {}", bundle_path);
        Ok(())
    }
}

impl Default for RaucClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_json() {
        let json = serde_json::json!({
            "slots": [
                {
                    "name": "rootfs.0",
                    "device": "/dev/mmcblk0p2",
                    "type": "ext4",
                    "boot_status": "good",
                    "version": "1.0.0",
                    "state": "inactive"
                },
                {
                    "name": "rootfs.1",
                    "device": "/dev/mmcblk0p3",
                    "type": "ext4",
                    "boot_status": "active",
                    "version": "1.1.0",
                    "state": "active"
                }
            ],
            "operation": "idle",
            "compatible": "uos-tv-rk3566",
            "variant": null,
            "last_error": null
        });

        let client = RaucClient::new();
        let status = client.parse_status_json(&json).unwrap();

        assert_eq!(status.slots.len(), 2);
        assert_eq!(status.slots[0].name, "rootfs.0");
        assert!(status.slots[1].active);
        assert!(!status.slots[0].active);

        let inactive = status.slots.iter().find(|s| !s.active).unwrap();
        assert_eq!(inactive.name, "rootfs.0");
    }
}
