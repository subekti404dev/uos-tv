//! Ethernet management (placeholder)

pub struct EthernetManager;

impl EthernetManager {
    pub fn new() -> Self {
        Self
    }

    pub fn is_connected(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            std::path::Path::new("/sys/class/net/eth0/carrier").exists()
        }
        #[cfg(not(target_os = "linux"))]
        true
    }
}
