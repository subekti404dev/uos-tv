//! devmand — UOS TV Device Manager
//! ================================
//! devmand menangani device hotplug dan hardware detection:
//!   - USB storage detection → auto-mount
//!   - Bluetooth adapter detection
//!   - HDMI hotplug → audio routing
//!   - Gamepad connection
//!   - Sensor detection (ambient light, proximity)
//!
//! Di production: listen udev events via netlink socket.
//! Di development: polling /sys/class + /dev/
//!
//! API via stardust:
//!   device.added { type, path, vendor, model, serial }
//!   device.removed { type, path }
//!   device.list → response with all devices

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub devtype: String,   // "usb", "bluetooth", "hdmi", "storage", "input"
    pub subsystem: String, // "block", "input", "net", "tty", "video4linux"
    pub path: String,      // /dev/sda1, /dev/input/event0
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub mounted: bool,
    pub mount_point: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("devmand starting...");

    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("devmand").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust: {e}");
            None
        }
    };

    // For development: detect some mock devices
    let mut devices = vec![
        Device {
            devtype: "storage".into(),
            subsystem: "block".into(),
            path: "/dev/mmcblk0p1".into(),
            vendor: Some("Samsung".into()),
            model: Some("eMMC 32GB".into()),
            serial: Some("00000000".into()),
            mounted: true,
            mount_point: Some("/".into()),
        },
        Device {
            devtype: "bluetooth".into(),
            subsystem: "bluetooth".into(),
            path: "/dev/ttyBT0".into(),
            vendor: Some("Realtek".into()),
            model: Some("RTL8761B".into()),
            serial: None,
            mounted: false,
            mount_point: None,
        },
    ];

    // Publish initial devices
    if let Some(ref c) = client {
        publish_devices(c, &devices);
    }

    // Subscribe ke commands
    if let Some(ref c) = client {
        if let Ok(mut rx) = c.subscribe("device.command.*").await {
            let client = c.clone();
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if msg.method == "device.command.list" {
                        // Respond with device list
                    }
                }
            });
        }
    }

    tracing::info!("devmand ready — monitoring device events...");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;

        // Di production: listen udev via netlink (libudev binding)
        #[cfg(target_os = "linux")]
        {
            // udev monitor placeholder
            // let mut monitor = udev::MonitorBuilder::new()?.listen()?;
            // for event in monitor.iter() { ... }
        }
    }
}

fn publish_devices(client: &stardust::Client, devices: &[Device]) {
    let payload = serde_json::json!({ "devices": devices });
    if let Ok(msg) = stardust::Message::new("device.list")
        .src("devmand".to_string())
        .param("payload", &payload)
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}
