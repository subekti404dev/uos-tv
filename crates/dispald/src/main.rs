//! dispald — UOS TV Display Manager
//! ==================================
//! dispald mengelola parameter display hardware:
//!   - Backlight brightness (via /sys/class/backlight/)
//!   - Resolution switching
//!   - HDR mode toggle
//!   - Color temperature (night mode)
//!   - Screen rotation
//!   - Multiple output management
//!
//! API via stardust:
//!   display.set_brightness { level: 0-100 }
//!   display.set_resolution { width, height, refresh }
//!   display.set_night_mode { enabled, temperature_k }
//!   display.status → { brightness, resolution, night_mode, hdr }

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("dispald starting...");

    let mut brightness: u32 = 80; // Default 80%
    let mut night_mode = false;
    let resolution = (1920u32, 1080u32, 60u32);

    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("dispald").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust: {e}");
            None
        }
    };

    // Subscribe commands
    if let Some(ref c) = client {
        if let Ok(mut rx) = c.subscribe("display.command.*").await {
            let client = c.clone();
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if let Ok(params) = serde_json::from_slice::<serde_json::Value>(&msg.params) {
                        match msg.method.as_str() {
                            "display.command.set_brightness" => {
                                if let Some(lvl) = params["level"].as_u64() {
                                    let _ = set_brightness(lvl as u32);
                                }
                            }
                            "display.command.set_night_mode" => {
                                if let Some(enabled) = params["enabled"].as_bool() {
                                    let _ = set_night_mode(enabled);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        }
    }

    // Publish initial state
    if let Some(ref c) = client {
        publish_state(c, brightness, night_mode, resolution);
    }

    tracing::info!("dispald ready");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

fn set_brightness(level: u32) -> Result<(), String> {
    let level = level.min(100);
    tracing::info!("Brightness → {level}%");

    #[cfg(target_os = "linux")]
    {
        // Tulis ke /sys/class/backlight/*/brightness
        if let Ok(entries) = std::fs::read_dir("/sys/class/backlight") {
            for entry in entries.flatten() {
                let path = entry.path().join("max_brightness");
                let max = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(255);
                let val = (max * level / 100).min(max);
                let _ = std::fs::write(entry.path().join("brightness"), val.to_string());
            }
        }
    }
    Ok(())
}

fn set_night_mode(enabled: bool) -> Result<(), String> {
    tracing::info!("Night mode → {enabled}");
    Ok(())
}

fn publish_state(
    client: &stardust::Client,
    brightness: u32,
    night_mode: bool,
    resolution: (u32, u32, u32),
) {
    let payload = serde_json::json!({
        "brightness": brightness,
        "night_mode": night_mode,
        "resolution": {
            "width": resolution.0,
            "height": resolution.1,
            "refresh": resolution.2,
        },
    });

    if let Ok(msg) = stardust::Message::new("display.status")
        .src("dispald".to_string())
        .param("payload", &payload)
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}
