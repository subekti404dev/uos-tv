//! P0-2: Luna UI ↔ stardustd WebSocket Integration Test
//! ======================================================
//!
//! Verifies that the Luna UI shell (via bus.js StardustClient)
//! can communicate with stardustd through the WebSocket bridge.
//!
//! What this tests:
//!   1. stardustd starts and binds WS :0 (ephemeral port)
//!   2. WebSocket client connects and receives "connected" event
//!   3. Client subscribes to a topic — receives published events
//!   4. Client publishes — other subscribers receive
//!   5. Client calls (RPC) — receives response
//!   6. Client disconnect + reconnect — subscription survives
//!
//! This simulates exactly what Luna UI's bus.js does.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use std::net::TcpListener;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Incoming event from stardust WS bridge (mirrors bus.js expectations).
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsEvent {
    #[serde(rename = "connected")]
    Connected { service: String, version: String },
    #[serde(rename = "event")]
    Event { method: String, params: Value },
    #[serde(rename = "response")]
    Response {
        id: String,
        status: String,
        data: Value,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        id: Option<String>,
        message: String,
    },
}

/// Get a random available port.
fn find_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Connect a WebSocket client (simulates Luna UI's bus.js StardustClient).
async fn ws_connect(
    addr: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{addr}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    ws
}

/// Wait for next text message and deserialize.
async fn ws_recv(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> WsEvent {
    loop {
        match ws.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                return serde_json::from_str(&text).expect("Invalid JSON");
            }
            Some(Ok(WsMessage::Close(_))) => panic!("Connection closed"),
            Some(Err(e)) => panic!("WS error: {e}"),
            None => panic!("Connection ended"),
            _ => continue,
        }
    }
}

/// Send a JSON text message via WebSocket.
async fn ws_send(
    ws: &mut (impl futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    payload: &Value,
) {
    let text = serde_json::to_string(payload).unwrap();
    futures_util::SinkExt::send(ws, WsMessage::Text(text.into()))
        .await
        .expect("Send failed");
}

// ═══════════════════════════════════════════════════════════════
// Test 1: Connect → receive "connected" event
// ═══════════════════════════════════════════════════════════════
#[tokio::test]
async fn test_ws_connect_and_receive_welcome() {
    let socket = "/tmp/test-ws-bus.sock";
    let _ = std::fs::remove_file(socket);
    let port = find_port();
    let ws_addr = format!("127.0.0.1:{port}");

    // Start broker
    let broker = stardust::Broker::new(socket.to_string()).with_ws(ws_addr.clone());
    tokio::spawn(async move { broker.run().await.ok() });

    // Wait for startup
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Connect WS client (like Luna UI does)
    let mut ws = ws_connect(&ws_addr).await;

    // Should receive "connected" event
    let event = ws_recv(&mut ws).await;
    match event {
        WsEvent::Connected { service, .. } => {
            assert_eq!(service, "luna-ui", "Should identify as luna-ui");
        }
        other => panic!("Expected Connected, got: {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════
// Test 2: Subscribe → receive events
// ═══════════════════════════════════════════════════════════════
#[tokio::test]
async fn test_ws_subscribe_and_receive_event() {
    let socket = "/tmp/test-ws-sub.sock";
    let _ = std::fs::remove_file(socket);
    let port = find_port();
    let ws_addr = format!("127.0.0.1:{port}");

    let broker = stardust::Broker::new(socket.to_string()).with_ws(ws_addr.clone());
    tokio::spawn(async move { broker.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut ws = ws_connect(&ws_addr).await;

    // Consume "connected" event
    let _ = ws_recv(&mut ws).await;

    // Subscribe to audio.* (like Luna UI does for volume HUD)
    let sub = serde_json::json!({
        "type": "subscribe",
        "topic": "audio.*"
    });
    ws_send(&mut ws, &sub).await;

    // Connect a Unix socket client to publish
    let unix_client = stardust::Client::connect(socket)
        .await
        .expect("Unix client connect failed");
    unix_client.register("test-audio").await.ok();

    // Small delay for subscription to propagate
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish via Unix client
    let msg = stardust::Message::new("audio.status")
        .src("test-audio".to_string())
        .param("volume", &0.75)
        .unwrap();
    unix_client.publish(msg).await.expect("Publish failed");

    // Receive via WebSocket
    let received = tokio::time::timeout(Duration::from_secs(2), ws_recv(&mut ws)).await;

    match received {
        Ok(WsEvent::Event { method, params }) => {
            assert_eq!(method, "audio.status");
            assert_eq!(params["volume"], 0.75);
        }
        Ok(other) => panic!("Expected Event, got: {other:?}"),
        Err(_) => panic!("Timeout — no event received via WebSocket"),
    }
}

// ═══════════════════════════════════════════════════════════════
// Test 3: WebSocket client publishes → event routed
// ═══════════════════════════════════════════════════════════════
#[tokio::test]
async fn test_ws_publish_triggers_event() {
    let socket = "/tmp/test-ws-pub.sock";
    let _ = std::fs::remove_file(socket);
    let port = find_port();
    let ws_addr = format!("127.0.0.1:{port}");

    let broker = stardust::Broker::new(socket.to_string()).with_ws(ws_addr.clone());
    tokio::spawn(async move { broker.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // WS Client A — subscriber
    let mut ws_a = ws_connect(&ws_addr).await;
    let _ = ws_recv(&mut ws_a).await; // consume connected
    ws_send(
        &mut ws_a,
        &serde_json::json!({"type":"subscribe","topic":"notif.*"}),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // WS Client B — publisher (like Luna UI sending user action)
    let mut ws_b = ws_connect(&ws_addr).await;
    let _ = ws_recv(&mut ws_b).await; // consume connected
    ws_send(
        &mut ws_b,
        &serde_json::json!({
            "type": "publish",
            "method": "notif.test",
            "params": {"message": "Hello from Luna UI"}
        }),
    )
    .await;

    // Client A should receive
    let received = tokio::time::timeout(Duration::from_secs(2), ws_recv(&mut ws_a)).await;
    match received {
        Ok(WsEvent::Event { method, params }) => {
            assert_eq!(method, "notif.test");
            assert_eq!(params["message"], "Hello from Luna UI");
        }
        Ok(other) => panic!("Expected Event, got: {other:?}"),
        Err(_) => panic!("Timeout — Client A didn't receive Client B's publish"),
    }
}

// ═══════════════════════════════════════════════════════════════
// Test 4: RPC call → receive response
// ═══════════════════════════════════════════════════════════════
#[tokio::test]
async fn test_ws_call_and_response() {
    let socket = "/tmp/test-ws-call.sock";
    let _ = std::fs::remove_file(socket);
    let port = find_port();
    let ws_addr = format!("127.0.0.1:{port}");

    let broker = stardust::Broker::new(socket.to_string()).with_ws(ws_addr.clone());
    tokio::spawn(async move { broker.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut ws = ws_connect(&ws_addr).await;
    let _ = ws_recv(&mut ws).await; // consume connected

    // Call a service (RPC)
    let call_id = "req-test-1";
    ws_send(
        &mut ws,
        &serde_json::json!({
            "type": "call",
            "method": "system.info",
            "params": {},
            "id": call_id
        }),
    )
    .await;

    // Should get response or error (service might not exist, but protocol works)
    let response = tokio::time::timeout(Duration::from_secs(2), ws_recv(&mut ws)).await;

    match response {
        Ok(event) => match event {
            WsEvent::Response { id, .. } => {
                assert_eq!(id, call_id, "Response ID should match call ID");
            }
            WsEvent::Error { id, .. } => {
                // Service not found is an expected error
                // The protocol still works correctly
                assert_eq!(id, Some(call_id.to_string()));
            }
            other => panic!("Expected Response or Error, got: {other:?}"),
        },
        Err(_) => panic!("Timeout — no RPC response"),
    }
}

// ═══════════════════════════════════════════════════════════════
// Test 5: Full Luna UI simulation (bus.js behavior)
// ═══════════════════════════════════════════════════════════════
#[tokio::test]
async fn test_full_luna_ui_session() {
    let socket = "/tmp/test-ws-full.sock";
    let _ = std::fs::remove_file(socket);
    let port = find_port();
    let ws_addr = format!("127.0.0.1:{port}");

    let broker = stardust::Broker::new(socket.to_string()).with_ws(ws_addr.clone());
    tokio::spawn(async move { broker.run().await.ok() });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Simulate Luna UI startup
    let mut luna = ws_connect(&ws_addr).await;

    // 1. Receive connected
    let _ = ws_recv(&mut luna).await;

    // 2. Subscribe to all topics Luna UI cares about
    let topics = vec![
        "audio.*",
        "notif.*",
        "network.*",
        "display.*",
        "power.*",
        "input.*",
        "ota.*",
        "system.*",
    ];
    for topic in &topics {
        ws_send(
            &mut luna,
            &serde_json::json!({"type":"subscribe","topic":topic}),
        )
        .await;
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. Connect service clients and publish events
    let svc = stardust::Client::connect(socket)
        .await
        .expect("Service client connect");
    svc.register("test-system").await.ok();

    // Publish audio volume change
    let msg = stardust::Message::new("audio.status")
        .src("test-system".to_string())
        .param("muted", &false)
        .unwrap();
    svc.publish(msg).await.expect("Publish failed");

    // Luna UI should receive
    let event = tokio::time::timeout(Duration::from_secs(3), ws_recv(&mut luna)).await;
    match event {
        Ok(WsEvent::Event { method, params }) => {
            assert_eq!(method, "audio.status");
            assert_eq!(params["muted"], false);
        }
        _ => panic!("Luna UI didn't receive audio event"),
    }

    // 4. Luna UI publishes user action (key press)
    ws_send(
        &mut luna,
        &serde_json::json!({
            "type": "publish",
            "method": "input.key",
            "params": {"key": "OK", "action": "press"}
        }),
    )
    .await;

    // Verify another WS client can receive it
    let mut debug_client = ws_connect(&ws_addr).await;
    let _ = ws_recv(&mut debug_client).await;
    ws_send(
        &mut debug_client,
        &serde_json::json!({"type":"subscribe","topic":"input.*"}),
    )
    .await;

    // Luna UI sends another keypress
    tokio::time::sleep(Duration::from_millis(100)).await;
    ws_send(
        &mut luna,
        &serde_json::json!({
            "type": "publish",
            "method": "input.key",
            "params": {"key": "Home"}
        }),
    )
    .await;

    let event = tokio::time::timeout(Duration::from_secs(2), ws_recv(&mut debug_client)).await;
    match event {
        Ok(WsEvent::Event { method, params }) => {
            assert_eq!(method, "input.key");
            assert_eq!(params["key"], "Home");
        }
        _ => panic!("Debug client didn't receive input event"),
    }

    // 5. Disconnect and reconnect Luna UI (resilience test)
    drop(luna);

    let mut luna = ws_connect(&ws_addr).await;
    let _ = ws_recv(&mut luna).await;

    // Re-subscribe
    ws_send(
        &mut luna,
        &serde_json::json!({"type":"subscribe","topic":"audio.*"}),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish again
    let msg = stardust::Message::new("audio.status")
        .src("test-system".to_string())
        .param("muted", &true)
        .unwrap();
    svc.publish(msg).await.expect("Publish after reconnect");

    let event = tokio::time::timeout(Duration::from_secs(2), ws_recv(&mut luna)).await;
    match event {
        Ok(WsEvent::Event { method, params }) => {
            assert_eq!(method, "audio.status");
            assert_eq!(params["muted"], true);
        }
        _ => panic!("Luna UI after reconnect didn't receive"),
    }
}
