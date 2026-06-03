//! Audio device management (placeholder)

pub struct DeviceManager;

impl DeviceManager {
    pub fn new() -> Self {
        Self
    }

    pub fn list_outputs(&self) -> Vec<AudioDevice> {
        vec![
            AudioDevice {
                name: "HDMI".into(),
                available: true,
                default: true,
            },
            AudioDevice {
                name: "Analog".into(),
                available: true,
                default: false,
            },
            AudioDevice {
                name: "Bluetooth".into(),
                available: false,
                default: false,
            },
        ]
    }
}

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub available: bool,
    pub default: bool,
}
