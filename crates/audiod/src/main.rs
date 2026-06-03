//! audiod — UOS TV Audio Manager
//! ==============================
//!
//! audiod mengelola audio subsystem:
//!   - PipeWire session supervision
//!   - Volume control (system, per-app)
//!   - Audio device routing (HDMI, analog, Bluetooth)
//!   - Mute/unmute
//!
//! Untuk v1, audiod me-manage PipeWire process lifecycle
//! dan menyediakan API level kontrol via stardust.
//! PipeWire sudah menangani routing, mixing, dan device detection.

mod devices;
mod volume;

use parking_lot::Mutex;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let service_name = std::env::var("UOS_SERVICE_NAME").unwrap_or_else(|_| "audiod".to_string());
    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("{service_name} starting...");

    // Init volume controller
    let volume = Arc::new(Mutex::new(volume::VolumeController::new()));

    // Connect ke stardust
    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("audiod").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust bus: {e}");
            None
        }
    };

    // Subscribe ke audio commands
    if let Some(ref c) = client {
        if let Ok(mut rx) = c.subscribe("audio.command.*").await {
            let client = c.clone();
            let volume_ref = volume.clone();
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    {
                        let mut vol = volume_ref.lock();
                        handle_command(&client, &mut vol, &msg);
                    }
                }
            });
        }
    }

    // Publish initial state
    if let Some(ref c) = client {
        publish_state(c, &volume.lock());
    }

    tracing::info!("{service_name} ready");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        if let Some(ref c) = client {
            publish_state(c, &volume.lock());
        }
    }
}

fn handle_command(
    client: &stardust::Client,
    volume: &mut volume::VolumeController,
    msg: &stardust::Message,
) {
    match msg.method.as_str() {
        "audio.command.set_volume" => {
            if let Ok(vol) = serde_json::from_slice::<u32>(&msg.params) {
                volume.set_volume(vol);
                publish_state(client, volume);
            }
        }
        "audio.command.mute" => {
            volume.toggle_mute();
            publish_state(client, volume);
        }
        "audio.command.volume_up" => {
            volume.volume_up();
            publish_state(client, volume);
        }
        "audio.command.volume_down" => {
            volume.volume_down();
            publish_state(client, volume);
        }
        _ => {
            tracing::debug!("Unknown audio command: {}", msg.method);
        }
    }
}

fn publish_state(client: &stardust::Client, volume: &volume::VolumeController) {
    let payload = serde_json::json!({
        "volume": volume.volume(),
        "muted": volume.is_muted(),
        "min_volume": volume::MIN_VOLUME,
        "max_volume": volume::MAX_VOLUME,
    });

    if let Ok(msg) = stardust::Message::new("audio.status")
        .src("audiod".to_string())
        .param("payload", &payload)
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}
