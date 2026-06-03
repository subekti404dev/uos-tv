//! WebSocket bridge — Luna UI ↔ Stardust IPC
//! ===========================================
//!
//! Menerima koneksi WebSocket dari Luna UI shell (browser JS)
//! dan menerjemahkan frame JSON ↔ internal stardust pub/sub.
//!
//! Protocol (JSON over WebSocket):
//!
//! Client → Server:
//!   {"type":"subscribe","topic":"audio.*"}
//!   {"type":"unsubscribe","topic":"audio.*"}
//!   {"type":"publish","method":"input.key","params":{...}}
//!   {"type":"call","method":"system.info","params":{...},"id":"req1"}
//!
//! Server → Client:
//!   {"type":"event","method":"audio.status","params":{...}}
//!   {"type":"response","id":"req1","status":"ok","data":{...}}
//!   {"type":"error","id":"req1","message":"..."}

use crate::broker::BrokerHandle;
use crate::message::Message;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Incoming WS message from client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsRequest {
    #[serde(rename = "subscribe")]
    Subscribe { topic: String },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { topic: String },
    #[serde(rename = "publish")]
    Publish {
        method: String,
        #[serde(default)]
        params: Value,
    },
    #[serde(rename = "call")]
    Call {
        method: String,
        #[serde(default)]
        params: Value,
        id: String,
    },
    #[serde(rename = "register")]
    Register { name: String },
}

/// Outgoing WS message to client.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum WsEvent {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        message: String,
    },
    #[serde(rename = "connected")]
    Connected { service: String, version: String },
}

/// Start the WebSocket bridge server.
pub async fn start_ws_server(
    broker: BrokerHandle,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(bind_addr).await?;
    tracing::info!("WebSocket bridge listening on ws://{bind_addr}");

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tracing::debug!("WS client connected: {addr}");
                let broker = broker.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_ws_client(stream, broker).await {
                        tracing::warn!("WS client {addr} error: {e}");
                    }
                    tracing::debug!("WS client disconnected: {addr}");
                });
            }
            Err(e) => {
                tracing::error!("WS accept error: {e}");
            }
        }
    }
}

/// Handle a single WebSocket client connection.
async fn handle_ws_client(
    stream: TcpStream,
    broker: BrokerHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Channel: broker → WS client
    let (tx, mut rx) = mpsc::unbounded_channel::<WsEvent>();

    // Channel: subscribe handles (broker → us)
    let (sub_tx, mut sub_rx) = mpsc::unbounded_channel::<Message>();

    // Send connected event
    let connected = serde_json::to_string(&WsEvent::Connected {
        service: "luna-ui".into(),
        version: "0.1.0".into(),
    })?;
    ws_sink.send(WsMessage::Text(connected.into())).await?;

    // Forward outgoing events to WS
    let send_handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                if ws_sink.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Process incoming WS messages
    loop {
        tokio::select! {
            // Incoming WS frame
            ws_msg = ws_stream.next() => {
                match ws_msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Err(e) = handle_ws_message(
                            &text,
                            &broker,
                            &tx,
                            &sub_tx,
                        ).await {
                            tracing::warn!("WS message error: {e}");
                            let _ = tx.send(WsEvent::Error {
                                id: None,
                                message: e.to_string(),
                            });
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) => break,
                    Some(Err(e)) => {
                        tracing::warn!("WS error: {e}");
                        break;
                    }
                    None => break,
                    _ => {} // Ignore binary/ping/pong
                }
            }

            // Incoming broker messages (from subscriptions)
            broker_msg = sub_rx.recv() => {
                if let Some(msg) = broker_msg {
                    let params = parse_params_to_value(&msg.params);
                    let event = WsEvent::Event {
                        method: msg.method,
                        params,
                    };
                    let _ = tx.send(event);
                }
            }
        }
    }

    drop(send_handle);
    Ok(())
}

/// Parses message params (which may be JSON or CBOR) into a serde_json Value.
fn parse_params_to_value(params: &[u8]) -> Value {
    // Try JSON first (from WS-originated messages)
    if let Ok(v) = serde_json::from_slice(params) {
        return v;
    }
    // Fallback: try CBOR → JSON conversion (from Unix bus messages)
    if let Ok(v) = ciborium::de::from_reader::<ciborium::value::Value, _>(params) {
        if let Ok(json) = serde_json::to_value(v) {
            return json;
        }
    }
    Value::Null
}

/// Process a single incoming WebSocket JSON message.
async fn handle_ws_message(
    text: &str,
    broker: &BrokerHandle,
    tx: &mpsc::UnboundedSender<WsEvent>,
    sub_tx: &mpsc::UnboundedSender<Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let req: WsRequest = serde_json::from_str(text)?;

    match req {
        WsRequest::Subscribe { topic } => {
            tracing::debug!("WS subscribe: {topic}");
            let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
            broker.subscribe(&topic, msg_tx);

            // Forward broker messages to this client
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(msg) = msg_rx.recv().await {
                    let params = parse_params_to_value(&msg.params);
                    let event = WsEvent::Event {
                        method: msg.method,
                        params,
                    };
                    let _ = tx.send(event);
                }
            });
        }

        WsRequest::Unsubscribe { topic } => {
            tracing::debug!("WS unsubscribe: {topic}");
            broker.unsubscribe(&topic, sub_tx.clone());
        }

        WsRequest::Publish { method, params } => {
            let params_bytes = serde_json::to_vec(&params)?;
            let msg = crate::message::Message::new(method)
                .src("luna-ui".to_string())
                .params_raw(params_bytes);
            broker.publish(msg).await;
        }

        WsRequest::Call { method, params, id } => {
            let params_bytes = serde_json::to_vec(&params)?;
            let msg = crate::message::Message::new(method)
                .src("luna-ui".to_string())
                .params_raw(params_bytes);

            match broker.call(msg, std::time::Duration::from_secs(5)).await {
                Ok(response) => {
                    let (status, data) = match response.result {
                        Ok(bytes) => ("ok", serde_json::from_slice(&bytes).unwrap_or(Value::Null)),
                        Err(err) => ("error", Value::String(err)),
                    };
                    let _ = tx.send(WsEvent::Response {
                        id,
                        status: status.to_string(),
                        data,
                    });
                }
                Err(e) => {
                    let _ = tx.send(WsEvent::Error {
                        id: Some(id),
                        message: e.to_string(),
                    });
                }
            }
        }

        WsRequest::Register { name } => {
            tracing::debug!("WS register: {name}");
            broker.register_service(&name);
        }
    }

    Ok(())
}
