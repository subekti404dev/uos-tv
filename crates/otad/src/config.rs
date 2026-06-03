//! OTA Configuration

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaConfig {
    /// URL update server
    #[serde(default = "default_server_url")]
    pub server_url: String,

    /// Interval polling (detik)
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Release channel: stable, beta, dev
    #[serde(default = "default_channel")]
    pub channel: String,

    /// Device identifier
    #[serde(default = "default_device")]
    pub device: String,

    /// Direktori download
    #[serde(default = "default_download_dir")]
    pub download_dir: String,

    /// Auto-install (false = download only, tunggu konfirmasi user)
    #[serde(default = "default_auto_install")]
    pub auto_install: bool,

    /// Auto-reboot setelah install
    #[serde(default = "default_auto_reboot")]
    pub auto_reboot: bool,

    /// Maximum download bandwidth (bytes/detik, 0 = unlimited)
    #[serde(default)]
    pub max_bandwidth: u64,

    /// Retry count untuk download gagal
    #[serde(default = "default_retry_count")]
    pub retry_count: u32,

    /// Retry delay (detik)
    #[serde(default = "default_retry_delay")]
    pub retry_delay_secs: u64,

    /// Hanya download via unmetered network (WiFi = metered)
    #[serde(default)]
    pub unmetered_only: bool,

    /// CA certificate bundle for TLS verification.
    /// Default: system CA store (/etc/ssl/certs/ca-certificates.crt).
    #[serde(default = "default_tls_ca_bundle")]
    pub tls_ca_bundle: String,
}

fn default_server_url() -> String {
    "https://ota.uos-tv.example.com".to_string()
}
fn default_poll_interval() -> u64 {
    21600
} // 6 jam
fn default_channel() -> String {
    "stable".to_string()
}
fn default_device() -> String {
    "qemu-virt".to_string()
}
fn default_download_dir() -> String {
    "/data/ota".to_string()
}
fn default_auto_install() -> bool {
    false
}
fn default_auto_reboot() -> bool {
    false
}
fn default_retry_count() -> u32 {
    3
}
fn default_retry_delay() -> u64 {
    30
}
fn default_tls_ca_bundle() -> String {
    "/etc/ssl/certs/ca-certificates.crt".to_string()
}

impl Default for OtaConfig {
    fn default() -> Self {
        Self {
            server_url: default_server_url(),
            poll_interval_secs: default_poll_interval(),
            channel: default_channel(),
            device: default_device(),
            download_dir: default_download_dir(),
            auto_install: default_auto_install(),
            auto_reboot: default_auto_reboot(),
            max_bandwidth: 0,
            retry_count: default_retry_count(),
            retry_delay_secs: default_retry_delay(),
            unmetered_only: false,
            tls_ca_bundle: default_tls_ca_bundle(),
        }
    }
}

impl OtaConfig {
    /// Load config dari file YAML.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Simpan config ke file.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}
