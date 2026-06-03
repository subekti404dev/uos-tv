//! inputd — UOS TV Input Manager
//! ===============================
//!
//! Handles input device events:
//!   - Remote control (IR) via evdev + key code mapping
//!   - Keyboard
//!   - Gamepad placeholder
//!
//! On Linux, reads /dev/input/event* via evdev crate and
//! publishes events to the compositor (lumind) via stardust.
//!
//! API via stardust:
//!   input.key { device, key, state }     — key press/release
//!   input.remote { device, button }      — remote button

mod input_handler;

use input_handler::{InputEvent, IrRemoteMap};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("inputd starting...");

    let handler = Arc::new(input_handler::InputHandler::new());

    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("inputd").await.ok();
            Arc::new(c)
        }
        Err(e) => {
            tracing::warn!("No stardust: {e}");
            return;
        }
    };

    // Subscribe ke command dari apps
    if let Ok(mut rx) = client.subscribe("input.command.*").await {
        let client = client.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                tracing::debug!("Input command: {}", msg.method);
            }
        });
    }

    // Bridge: std thread sends via mpsc → tokio channel
    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<InputEvent>();
    let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::channel::<InputEvent>(256);

    // Reader thread: evdev → mpsc
    let handler_clone = handler.clone();
    std::thread::spawn(move || {
        if let Err(e) = handler_clone.read_loop(ev_tx) {
            tracing::error!("Input read loop failed: {e}");
        }
    });

    // Bridge thread: mpsc → tokio
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            while let Ok(event) = ev_rx.recv() {
                if tokio_tx.send(event).await.is_err() {
                    break;
                }
            }
        });
    });

    tracing::info!("inputd ready");

    // Async event dispatch loop
    while let Some(event) = tokio_rx.recv().await {
        match event.kind {
            input_handler::EventKind::RemoteButton(key) => {
                let payload = serde_json::json!({
                    "device": event.device,
                    "button": IrRemoteMap::name(&key),
                    "kind": "remote",
                });
                if let Ok(msg) = stardust::Message::new("input.remote")
                    .src("inputd".to_string())
                    .param("payload", &payload)
                {
                    let _ = client.publish(msg).await;
                }
            }
            input_handler::EventKind::KeyPress(code) => {
                let payload = serde_json::json!({
                    "device": event.device,
                    "key": code,
                    "state": "pressed",
                });
                if let Ok(msg) = stardust::Message::new("input.key")
                    .src("inputd".to_string())
                    .param("payload", &payload)
                {
                    let _ = client.publish(msg).await;
                }
            }
            input_handler::EventKind::KeyRelease(code) => {
                let payload = serde_json::json!({
                    "device": event.device,
                    "key": code,
                    "state": "released",
                });
                if let Ok(msg) = stardust::Message::new("input.key")
                    .src("inputd".to_string())
                    .param("payload", &payload)
                {
                    let _ = client.publish(msg).await;
                }
            }
        }
    }
}
