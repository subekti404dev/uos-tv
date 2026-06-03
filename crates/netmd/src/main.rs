//! netmd — UOS TV Network Manager
//! ===============================
//!
//! netmd mengelola konektivitas jaringan perangkat:
//!   - Monitoring status koneksi (Ethernet, WiFi)
//!   - WiFi scanning + koneksi (via wpa_supplicant / iwd)
//!   - Network state publishing via stardust
//!   - Metered network detection
//!   - Connectivity check (internet reachability)
//!
//! Karena kita tidak pakai NetworkManager/connman, netmd
//! langsung menggunakan netlink + wpa_supplicant.

mod connectivity;
mod ethernet;
mod state;
mod wifi;

use crate::wifi::WifiManager;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let service_name = std::env::var("UOS_SERVICE_NAME").unwrap_or_else(|_| "netmd".to_string());
    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("{service_name} starting...");

    // Connect ke stardust
    let bus_client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            tracing::info!("Connected to stardust bus");
            if let Err(e) = c.register("netmd").await {
                tracing::warn!("Failed to register on stardust: {e}");
            }
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust bus available: {e}");
            None
        }
    };

    // Init network state manager
    let mut net_state = state::NetworkState::new();

    // Init WiFi manager
    let wifi = WifiManager::new();
    let wifi_available = wifi.available();
    if wifi_available {
        tracing::info!("WiFi: wpa_supplicant detected");
    } else {
        tracing::info!("WiFi: wpa_supplicant not available (Ethernet-only mode)");
    }

    // Subscribe ke stardust RPC events
    if let Some(client) = &bus_client {
        let mut rpc_rx = match client.subscribe("rpc.netmd.*").await {
            Ok(rx) => {
                tracing::info!("Listening for RPC: rpc.netmd.*");
                Some(rx)
            }
            Err(e) => {
                tracing::warn!("Failed to subscribe to RPC: {e}");
                None
            }
        };

        // Deteksi initial state
        net_state.detect_interfaces().await;
        publish_state(client, &net_state);

        // Main loop
        let mut interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let changed = net_state.detect_interfaces().await;
                    if changed {
                        tracing::info!(
                            "Network state: connected={}, iface={}, type={:?}",
                            net_state.connected(),
                            net_state.active_interface().unwrap_or("none"),
                            net_state.connection_type()
                        );
                        publish_state(client, &net_state);
                    }
                }
                msg = async {
                    match &mut rpc_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(msg) = msg {
                        handle_rpc(client, &wifi, &msg.method).await;
                    }
                }
            }
        }
    }
}

/// Handle incoming RPC requests from stardust.
async fn handle_rpc(client: &stardust::Client, wifi: &WifiManager, method: &str) {
    match method {
        "rpc.netmd.scan" => {
            tracing::info!("RPC: WiFi scan requested");
            let networks = wifi.scan().await;
            let payload = serde_json::json!({
                "networks": networks.iter().map(|n| serde_json::json!({
                    "ssid": n.ssid,
                    "bssid": n.bssid,
                    "frequency": n.frequency,
                    "signal_strength": n.signal_strength,
                    "security": n.security,
                })).collect::<Vec<_>>(),
            });
            let msg = stardust::Message::new("network.scan_results")
                .src("netmd".to_string())
                .param("payload", &payload)
                .unwrap_or_else(|_| {
                    stardust::Message::new("network.scan_results").src("netmd".to_string())
                });
            let _ = client.publish(msg).await;
        }
        "rpc.netmd.connect" => {
            tracing::info!("RPC: WiFi connect requested");
            // Params: { ssid, password }
            // For now, fire and forget — caller should provide params via message
            // TODO: parse ssid+password from message params
        }
        "rpc.netmd.disconnect" => {
            tracing::info!("RPC: WiFi disconnect requested");
            if let Err(e) = wifi.disconnect() {
                tracing::warn!("WiFi disconnect failed: {e}");
            }
        }
        "rpc.netmd.status" => {
            tracing::debug!("RPC: WiFi status requested");
            let status = wifi.status();
            let payload = serde_json::json!({
                "wifi": status.map(|s| serde_json::json!({
                    "ssid": s.ssid,
                    "bssid": s.bssid,
                    "wpa_state": s.wpa_state,
                    "ip_address": s.ip_address,
                })),
            });
            let msg = stardust::Message::new("network.wifi_status")
                .src("netmd".to_string())
                .param("payload", &payload)
                .unwrap_or_else(|_| {
                    stardust::Message::new("network.wifi_status").src("netmd".to_string())
                });
            let _ = client.publish(msg).await;
        }
        _ => {
            tracing::debug!("Unknown RPC: {method}");
        }
    }
}

/// Publish network state via stardust.
fn publish_state(client: &stardust::Client, state: &state::NetworkState) {
    let payload = serde_json::json!({
        "connected": state.connected(),
        "active_interface": state.active_interface(),
        "connection_type": state.connection_type(),
        "metered": state.is_metered(),
        "internet_available": state.internet_available,
        "interfaces": state.interfaces.values().map(|iface| {
            serde_json::json!({
                "name": iface.name,
                "iftype": format!("{:?}", iface.iftype),
                "up": iface.up,
                "connected": iface.connected,
                "ip_address": iface.ip_address,
                "ssid": iface.ssid,
                "signal_strength": iface.signal_strength,
            })
        }).collect::<Vec<_>>(),
    });

    let msg = match stardust::Message::new("network.status")
        .src("netmd".to_string())
        .param("payload", &payload)
    {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Failed to create status message: {e}");
            return;
        }
    };

    let client = client.clone();
    tokio::spawn(async move {
        let _ = client.publish(msg).await;
    });
}
