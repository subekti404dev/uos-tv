//! Network state tracking

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tipe interface jaringan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InterfaceType {
    Ethernet,
    WiFi,
    Bluetooth,
    Loopback,
    Unknown,
}

/// Status satu interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceState {
    pub name: String,
    pub iftype: InterfaceType,
    pub up: bool,
    pub connected: bool,
    pub ip_address: Option<String>,
    pub ssid: Option<String>,
    pub signal_strength: Option<i32>,
}

/// State jaringan secara keseluruhan.
pub struct NetworkState {
    pub interfaces: HashMap<String, InterfaceState>,
    pub internet_available: bool,
}

impl NetworkState {
    pub fn new() -> Self {
        Self {
            interfaces: HashMap::new(),
            internet_available: false,
        }
    }

    /// Deteksi interface dari /sys/class/net dan /proc/net/route.
    /// Return true jika ada perubahan dari sebelumnya.
    pub async fn detect_interfaces(&mut self) -> bool {
        // Untuk development (non-Linux), gunakan placeholder
        #[cfg(not(target_os = "linux"))]
        {
            if self.interfaces.is_empty() {
                self.interfaces.insert(
                    "lo".into(),
                    InterfaceState {
                        name: "lo".into(),
                        iftype: InterfaceType::Loopback,
                        up: true,
                        connected: false,
                        ip_address: Some("127.0.0.1".into()),
                        ssid: None,
                        signal_strength: None,
                    },
                );
                self.interfaces.insert(
                    "eth0".into(),
                    InterfaceState {
                        name: "eth0".into(),
                        iftype: InterfaceType::Ethernet,
                        up: true,
                        connected: true,
                        ip_address: Some("10.0.2.15".into()),
                        ssid: None,
                        signal_strength: None,
                    },
                );
                self.internet_available = true;
                return true;
            }
            return false;
        }

        #[cfg(target_os = "linux")]
        {
            self.detect_interfaces_linux()
        }
    }

    /// Deteksi di Linux — baca /sys/class/net/
    #[cfg(target_os = "linux")]
    fn detect_interfaces_linux(&mut self) -> bool {
        let mut changed = false;
        let mut seen = std::collections::HashSet::new();

        // Baca /sys/class/net/
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "." || name == ".." {
                    continue;
                }

                seen.insert(name.clone());

                let iftype = self.detect_iftype_linux(&name);
                let up = self.read_sysfs_flag(&name, "operstate", "up");
                let connected = self.read_sysfs_flag(&name, "carrier", "1");
                let ip = self.detect_ip_linux(&name);

                let new_iface = InterfaceState {
                    name: name.clone(),
                    iftype,
                    up,
                    connected,
                    ip_address: ip,
                    ssid: None, // wpa_supplicant integration needed
                    signal_strength: None,
                };

                match self.interfaces.get(&name) {
                    Some(old)
                        if old.connected != new_iface.connected
                            || old.ip_address != new_iface.ip_address
                            || old.up != new_iface.up =>
                    {
                        changed = true;
                    }
                    None => changed = true,
                    _ => {}
                }

                self.interfaces.insert(name, new_iface);
            }
        }

        // Hapus interface yang hilang
        let to_remove: Vec<String> = self
            .interfaces
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for name in to_remove {
            self.interfaces.remove(&name);
            changed = true;
        }

        // Set internet flag
        let new_internet = self.connected();
        if self.internet_available != new_internet {
            self.internet_available = new_internet;
            changed = true;
        }

        changed
    }

    /// Deteksi tipe interface berdasarkan direktori di /sys/class/net/{iface}/
    #[cfg(target_os = "linux")]
    fn detect_iftype_linux(&self, name: &str) -> InterfaceType {
        let base = format!("/sys/class/net/{name}");

        if name == "lo" {
            return InterfaceType::Loopback;
        }

        // Cek wireless directory
        if std::path::Path::new(&format!("{base}/wireless")).exists()
            || std::path::Path::new(&format!("{base}/phy80211")).exists()
        {
            return InterfaceType::WiFi;
        }

        // Cek device link mengandung "usb" atau "pci"
        if let Ok(link) = std::fs::read_link(format!("{base}/device")) {
            let link_str = link.to_string_lossy();
            if link_str.contains("usb") {
                return InterfaceType::Ethernet; // Bisa USB Ethernet
            }
        }

        InterfaceType::Ethernet
    }

    /// Baca flag dari sysfs file.
    #[cfg(target_os = "linux")]
    fn read_sysfs_flag(&self, iface: &str, file: &str, expected: &str) -> bool {
        let path = format!("/sys/class/net/{iface}/{file}");
        std::fs::read_to_string(&path)
            .map(|s| s.trim() == expected)
            .unwrap_or(false)
    }

    /// Deteksi IP address via /proc/net/fib_trie atau `ip addr`.
    #[cfg(target_os = "linux")]
    fn detect_ip_linux(&self, iface: &str) -> Option<String> {
        // Simple approach: baca /proc/net/fib_trie
        if let Ok(content) = std::fs::read_to_string("/proc/net/fib_trie") {
            // Skip untuk v1 — gunakan placeholder
            // Di production: parse fib_trie atau panggil `ip -j addr`
        }

        // Fallback: return placeholder untuk interface yang up
        if self.read_sysfs_flag(iface, "operstate", "up") {
            match iface {
                "lo" => Some("127.0.0.1".into()),
                _ => Some("0.0.0.0".into()), // Placeholder
            }
        } else {
            None
        }
    }

    /// Apakah ada koneksi yang aktif?
    pub fn connected(&self) -> bool {
        self.interfaces
            .values()
            .any(|i| i.connected && i.iftype != InterfaceType::Loopback)
    }

    /// Interface yang sedang aktif.
    pub fn active_interface(&self) -> Option<&str> {
        self.interfaces
            .iter()
            .find(|(_, i)| i.connected && i.iftype != InterfaceType::Loopback)
            .map(|(name, _)| name.as_str())
    }

    /// Tipe koneksi saat ini.
    pub fn connection_type(&self) -> &str {
        match self.active_interface().and_then(|n| self.interfaces.get(n)) {
            Some(iface) => match iface.iftype {
                InterfaceType::WiFi => "wifi",
                InterfaceType::Ethernet => "ethernet",
                InterfaceType::Bluetooth => "bluetooth",
                _ => "none",
            },
            None => "none",
        }
    }

    /// Apakah koneksi saat ini metered? (WiFi dianggap metered sampai dikonfirmasi)
    pub fn is_metered(&self) -> bool {
        match self.active_interface().and_then(|n| self.interfaces.get(n)) {
            Some(iface) => matches!(iface.iftype, InterfaceType::WiFi | InterfaceType::Bluetooth),
            None => true,
        }
    }
}
