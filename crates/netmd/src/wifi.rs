//! WiFi management — wpa_supplicant integration
//! ================================================
//!
//! Uses wpa_cli to communicate with wpa_supplicant.
//! In production, this would use the D-Bus API directly,
//! but wpa_cli is much simpler and works on Alpine Linux.

use std::process::Output;
use std::time::Duration;

pub struct WifiManager {
    iface: String,
    cli_path: String,
}

impl WifiManager {
    pub fn new() -> Self {
        Self {
            iface: "wlan0".to_string(),
            cli_path: "/usr/sbin/wpa_cli".to_string(),
        }
    }

    pub fn with_interface(iface: &str) -> Self {
        Self {
            iface: iface.to_string(),
            cli_path: "/usr/sbin/wpa_cli".to_string(),
        }
    }

    /// Check if wpa_supplicant is available and running.
    pub fn available(&self) -> bool {
        self.wpa_cli(&["status"]).is_ok()
    }

    /// Scan WiFi networks.
    /// Returns list of visible networks with SSID, signal strength, and security type.
    pub async fn scan(&self) -> Vec<WifiNetwork> {
        // Trigger scan
        if self.wpa_cli(&["scan"]).is_err() {
            tracing::debug!("WiFi scan: wpa_supplicant not available");
            return vec![];
        }

        // Wait for scan to complete
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Get scan results
        let output = match self.wpa_cli(&["scan_results"]) {
            Ok(o) => o,
            Err(_) => return vec![],
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_scan_results(&stdout)
    }

    /// List saved/configured networks.
    pub fn list_networks(&self) -> Vec<SavedNetwork> {
        let output = match self.wpa_cli(&["list_networks"]) {
            Ok(o) => o,
            Err(_) => return vec![],
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_network_list(&stdout)
    }

    /// Connect to a WiFi network with PSK.
    pub async fn connect(&self, ssid: &str, password: &str) -> Result<(), String> {
        // Check if network already exists
        let existing = self.list_networks();
        let existing_id = existing.iter().find(|n| n.ssid == ssid).map(|n| n.id);

        let net_id = if let Some(id) = existing_id {
            // Update existing network's PSK
            self.wpa_cli(&[
                "set_network",
                &id.to_string(),
                "psk",
                &format!("\"{password}\""),
            ])
            .map_err(|e| format!("Failed to set PSK: {e}"))?;
            id
        } else {
            // Add new network
            let output = self
                .wpa_cli(&["add_network"])
                .map_err(|e| format!("Add network failed: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let net_id: u32 = stdout
                .trim()
                .parse()
                .map_err(|_| "Failed to parse network ID".to_string())?;

            // Set SSID and PSK
            self.wpa_cli(&[
                "set_network",
                &net_id.to_string(),
                "ssid",
                &format!("\"{ssid}\""),
            ])
            .map_err(|_| "Failed to set SSID".to_string())?;

            self.wpa_cli(&[
                "set_network",
                &net_id.to_string(),
                "psk",
                &format!("\"{password}\""),
            ])
            .map_err(|_| "Failed to set PSK".to_string())?;

            net_id
        };

        // Enable and select the network
        self.wpa_cli(&["enable_network", &net_id.to_string()])
            .map_err(|_| "Failed to enable network".to_string())?;

        self.wpa_cli(&["select_network", &net_id.to_string()])
            .map_err(|_| "Failed to select network".to_string())?;

        // Save config
        let _ = self.wpa_cli(&["save_config"]);

        tracing::info!("WiFi: connecting to '{ssid}' (network ID {net_id})");
        Ok(())
    }

    /// Disconnect from WiFi.
    pub fn disconnect(&self) -> Result<(), String> {
        self.wpa_cli(&["disconnect"])
            .map_err(|_| "Failed to disconnect".to_string())?;
        Ok(())
    }

    /// Get connection status.
    pub fn status(&self) -> Option<WifiStatus> {
        let output = self.wpa_cli(&["status"]).ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_wifi_status(&stdout)
    }

    /// Get signal strength for current connection.
    pub fn signal_poll(&self) -> Option<i32> {
        let output = self.wpa_cli(&["signal_poll"]).ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(val) = line.strip_prefix("RSSI=") {
                return val.trim().parse().ok();
            }
        }
        None
    }

    /// Run wpa_cli command.
    fn wpa_cli(&self, args: &[&str]) -> std::io::Result<Output> {
        let _socket = format!("/var/run/wpa_supplicant/{}", self.iface);
        let mut full_args = vec!["-i", &self.iface, "-p", "/var/run/wpa_supplicant"];
        full_args.extend(args);

        std::process::Command::new(&self.cli_path)
            .args(&full_args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
    }
}

/// Parsed WiFi network from scan results.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: String,
    pub frequency: String,
    pub signal_strength: i32,
    pub security: String,
}

/// Saved/configured WiFi network.
#[derive(Debug, Clone)]
pub struct SavedNetwork {
    pub id: u32,
    pub ssid: String,
    pub bssid: String,
    pub flags: String,
}

/// WiFi connection status.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WifiStatus {
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub freq: Option<u32>,
    pub ip_address: Option<String>,
    pub key_mgmt: Option<String>,
    pub wpa_state: Option<String>,
}

/// Parse wpa_cli scan_results output.
/// Format (tab-separated):
/// bssid / frequency / signal level / flags / ssid
fn parse_scan_results(output: &str) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();

    for line in output.lines().skip(1) {
        // Skip header line
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 5 {
            continue;
        }

        let signal: i32 = fields[2].parse().unwrap_or(0);
        let ssid = fields[4].to_string();

        // Skip hidden/empty SSIDs
        if ssid.is_empty() {
            continue;
        }

        // Parse security from flags
        let flags = fields[3];
        let security = if flags.contains("WPA2") {
            "WPA2".to_string()
        } else if flags.contains("WPA") {
            "WPA".to_string()
        } else if flags.contains("WEP") {
            "WEP".to_string()
        } else {
            "OPEN".to_string()
        };

        networks.push(WifiNetwork {
            bssid: fields[0].to_string(),
            frequency: fields[1].to_string(),
            signal_strength: signal,
            security,
            ssid,
        });
    }

    // Sort by signal strength (strongest first)
    networks.sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
    networks
}

/// Parse wpa_cli list_networks output.
/// Format (tab-separated):
/// network id / ssid / bssid / flags
fn parse_network_list(output: &str) -> Vec<SavedNetwork> {
    let mut networks = Vec::new();

    for line in output.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 4 {
            continue;
        }

        let id: u32 = match fields[0].parse() {
            Ok(id) => id,
            Err(_) => continue,
        };

        networks.push(SavedNetwork {
            id,
            ssid: fields[1].to_string(),
            bssid: fields[2].to_string(),
            flags: fields[3].to_string(),
        });
    }

    networks
}

/// Parse wpa_cli status output.
fn parse_wifi_status(output: &str) -> Option<WifiStatus> {
    let mut status = WifiStatus {
        ssid: None,
        bssid: None,
        freq: None,
        ip_address: None,
        key_mgmt: None,
        wpa_state: None,
    };

    let mut found = false;

    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ssid=") {
            status.ssid = Some(val.to_string());
            found = true;
        } else if let Some(val) = line.strip_prefix("bssid=") {
            status.bssid = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("freq=") {
            status.freq = val.parse().ok();
        } else if let Some(val) = line.strip_prefix("ip_address=") {
            status.ip_address = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("key_mgmt=") {
            status.key_mgmt = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("wpa_state=") {
            status.wpa_state = Some(val.to_string());
        }
    }

    if found { Some(status) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scan_results() {
        let output = "bssid / frequency / signal level / flags / ssid\n\
            00:11:22:33:44:55\t2412\t-45\t[WPA2-PSK-CCMP][ESS]\tMyWiFi\n\
            aa:bb:cc:dd:ee:ff\t5180\t-72\t[WPA-PSK-CCMP][ESS]\tNeighborNet\n\
            11:22:33:44:55:66\t2412\t-88\t[]\tOpenWrt";

        let networks = parse_scan_results(output);
        assert_eq!(networks.len(), 3);
        assert_eq!(networks[0].ssid, "MyWiFi");
        assert_eq!(networks[0].signal_strength, -45);
        assert_eq!(networks[0].security, "WPA2");
        assert_eq!(networks[1].ssid, "NeighborNet");
        assert_eq!(networks[1].security, "WPA");
        assert_eq!(networks[2].ssid, "OpenWrt");
        assert_eq!(networks[2].security, "OPEN");
    }

    #[test]
    fn test_parse_network_list() {
        let output = "network id / ssid / bssid / flags\n\
            0\tMyWiFi\tany\t[DISABLED]\n\
            1\tOffice\tany\t[CURRENT]";

        let networks = parse_network_list(output);
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].id, 0);
        assert_eq!(networks[0].ssid, "MyWiFi");
        assert_eq!(networks[1].ssid, "Office");
    }

    #[test]
    fn test_parse_wifi_status() {
        let output = "bssid=00:11:22:33:44:55\n\
            freq=2412\n\
            ssid=MyWiFi\n\
            id=0\n\
            mode=station\n\
            wpa_state=COMPLETED\n\
            ip_address=192.168.1.100\n\
            address=aa:bb:cc:dd:ee:ff\n";

        let status = parse_wifi_status(output).unwrap();
        assert_eq!(status.ssid.unwrap(), "MyWiFi");
        assert_eq!(status.wpa_state.unwrap(), "COMPLETED");
        assert_eq!(status.ip_address.unwrap(), "192.168.1.100");
    }

    #[test]
    fn test_parse_empty_scan() {
        let output = "bssid / frequency / signal level / flags / ssid\n";
        let networks = parse_scan_results(output);
        assert!(networks.is_empty());
    }
}
