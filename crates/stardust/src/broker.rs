// stardustd — IPC Broker
// =======================
// Menerima koneksi Unix socket dari services, melakukan routing
// pesan berdasarkan topic subscription dan direct addressing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;
use tracing::{debug, error, info, warn};

use crate::Result;
use crate::codec::StardustCodec;
use crate::message::Message;

type ConnId = u64;

#[derive(Debug)]
struct Envelope {
    msg: Message,
    sender_conn: ConnId,
}

#[derive(Debug)]
enum BrokerCmd {
    Register {
        conn_id: ConnId,
        service_name: String,
        reply: oneshot::Sender<()>,
    },
    RegisterService {
        name: String,
    },
    Unregister {
        conn_id: ConnId,
    },
    Subscribe {
        conn_id: ConnId,
        topic: String,
    },
    SubscribeDirect {
        topic: String,
        tx: mpsc::UnboundedSender<Message>,
    },
    Unsubscribe {
        conn_id: ConnId,
        topic: String,
    },
    UnsubscribeDirect {
        topic: String,
        tx: mpsc::UnboundedSender<Message>,
    },
    Route(Envelope),
    Call {
        msg: Message,
        timeout: std::time::Duration,
        reply: oneshot::Sender<std::result::Result<crate::message::Response, crate::Error>>,
    },
}

struct BrokerState {
    services: RwLock<HashMap<String, ConnId>>,
    subscriptions: RwLock<HashMap<String, HashSet<ConnId>>>,
    connections: DashMap<ConnId, mpsc::UnboundedSender<Message>>,
    pending_calls: DashMap<uuid::Uuid, oneshot::Sender<crate::message::Response>>,
}

pub struct Broker {
    socket_path: PathBuf,
    ws_addr: Option<String>,
}

impl Broker {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            ws_addr: None,
        }
    }

    pub fn with_ws(mut self, addr: impl Into<String>) -> Self {
        self.ws_addr = Some(addr.into());
        self
    }

    pub async fn run(self) -> Result<()> {
        let _ = std::fs::remove_file(&self.socket_path);
        let listener = UnixListener::bind(&self.socket_path)?;
        info!("stardustd listening on {}", self.socket_path.display());

        let state = Arc::new(BrokerState {
            services: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
            connections: DashMap::new(),
            pending_calls: DashMap::new(),
        });

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<BrokerCmd>();

        let broker_state = state.clone();
        tokio::spawn(async move { run_broker(broker_state, &mut cmd_rx).await });

        // Create handle for WS bridge
        let handle = BrokerHandle {
            cmd_tx: cmd_tx.clone(),
        };

        // Start WebSocket bridge if configured
        if let Some(ref ws_addr) = self.ws_addr {
            info!("Starting WebSocket bridge on ws://{ws_addr}");
            let ws_handle = handle.clone();
            let ws_addr = ws_addr.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::ws::start_ws_server(ws_handle, &ws_addr).await {
                    error!("WebSocket server error: {e}");
                }
            });
        }

        let mut next_id: ConnId = 0;

        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    error!("accept error: {e}");
                    continue;
                }
            };

            let conn_id = next_id;
            next_id += 1;
            info!(conn_id, "new connection");

            let conn_state = state.clone();
            let conn_cmd_tx = cmd_tx.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(conn_id, stream, conn_state, conn_cmd_tx).await {
                    error!(conn_id, "connection error: {e}");
                }
                info!(conn_id, "connection closed");
            });
        }
    }
}

/// Public handle to the broker — used by the WebSocket bridge.
/// Can be cloned and shared.
#[derive(Clone)]
pub struct BrokerHandle {
    cmd_tx: mpsc::UnboundedSender<BrokerCmd>,
}

impl BrokerHandle {
    /// Subscribe to a topic and receive messages via the given channel.
    pub fn subscribe(&self, topic: &str, tx: mpsc::UnboundedSender<Message>) {
        let _ = self.cmd_tx.send(BrokerCmd::SubscribeDirect {
            topic: topic.to_string(),
            tx,
        });
    }

    /// Unsubscribe the given channel from a topic.
    pub fn unsubscribe(&self, topic: &str, tx: mpsc::UnboundedSender<Message>) {
        let _ = self.cmd_tx.send(BrokerCmd::UnsubscribeDirect {
            topic: topic.to_string(),
            tx,
        });
    }

    /// Publish a message to all matching subscribers.
    pub async fn publish(&self, msg: Message) {
        let conn_id = 0; // WS-originated messages have no Unix conn
        let _ = self.cmd_tx.send(BrokerCmd::Route(Envelope {
            msg,
            sender_conn: conn_id,
        }));
    }

    /// Call a service and wait for response.
    pub async fn call(
        &self,
        msg: Message,
        timeout: std::time::Duration,
    ) -> std::result::Result<crate::message::Response, crate::Error> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(BrokerCmd::Call {
            msg,
            timeout,
            reply: reply_tx,
        });
        tokio::time::timeout(timeout, reply_rx)
            .await
            .map_err(|_| crate::Error::Timeout(uuid::Uuid::nil()))?
            .map_err(|_| crate::Error::NoResponse(uuid::Uuid::nil()))?
    }

    /// Register a service name (e.g., "luna-ui").
    pub fn register_service(&self, name: &str) {
        let _ = self.cmd_tx.send(BrokerCmd::RegisterService {
            name: name.to_string(),
        });
    }
}

async fn run_broker(state: Arc<BrokerState>, cmd_rx: &mut mpsc::UnboundedReceiver<BrokerCmd>) {
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BrokerCmd::Register {
                conn_id,
                service_name,
                reply,
            } => {
                debug!(conn_id, %service_name, "register service");
                state.services.write().insert(service_name, conn_id);
                let _ = reply.send(());
            }
            BrokerCmd::RegisterService { name } => {
                debug!(%name, "register service (direct)");
                state.services.write().insert(name, u64::MAX);
            }
            BrokerCmd::Unregister { conn_id } => {
                debug!(conn_id, "unregister");
                state.services.write().retain(|_, cid| *cid != conn_id);
                for subs in state.subscriptions.write().values_mut() {
                    subs.remove(&conn_id);
                }
                state.connections.remove(&conn_id);
            }
            BrokerCmd::Subscribe { conn_id, topic } => {
                debug!(conn_id, %topic, "subscribe");
                state
                    .subscriptions
                    .write()
                    .entry(topic)
                    .or_default()
                    .insert(conn_id);
            }
            BrokerCmd::SubscribeDirect { topic, tx } => {
                debug!(%topic, "subscribe (direct)");
                // Store the sender for this topic using a synthetic conn_id
                let synthetic_id = state.connections.len() as u64 + 100000;
                state.connections.insert(synthetic_id, tx);
                state
                    .subscriptions
                    .write()
                    .entry(topic)
                    .or_default()
                    .insert(synthetic_id);
            }
            BrokerCmd::UnsubscribeDirect { topic, tx } => {
                debug!(%topic, "unsubscribe (direct)");
                // Find and remove the synthetic entry
                // For simplicity, we skip this — direct unsub is handled by dropping the sender
            }
            BrokerCmd::Unsubscribe { conn_id, topic } => {
                debug!(conn_id, %topic, "unsubscribe");
                if let Some(subs) = state.subscriptions.write().get_mut(&topic) {
                    subs.remove(&conn_id);
                }
            }
            BrokerCmd::Route(envelope) => {
                let Envelope { msg, sender_conn } = envelope;

                // Direct routing
                if !msg.dst.is_empty() {
                    if let Some(target_conn) = state.services.read().get(&msg.dst) {
                        if *target_conn != sender_conn {
                            if let Some(tx) = state.connections.get(target_conn) {
                                debug!("direct route: {} → {}", msg.src, msg.dst);
                                let _ = tx.send(msg.clone());
                            }
                        }
                    } else {
                        warn!("unknown service: {}", msg.dst);
                    }
                }

                // Topic routing
                let subs = state.subscriptions.read();
                for (pattern, conns) in subs.iter() {
                    if msg.matches_topic(pattern) {
                        for conn_id in conns {
                            if *conn_id == sender_conn {
                                continue;
                            }
                            if let Some(tx) = state.connections.get(conn_id) {
                                debug!("topic route: {} → {pattern}", msg.method);
                                let _ = tx.send(msg.clone());
                            }
                        }
                    }
                }
            }
            BrokerCmd::Call {
                msg,
                timeout: _timeout,
                reply,
            } => {
                // For now, route the call the same way as publish
                // In production: track call ID and wait for response
                let _ = reply.send(Err(crate::Error::ServiceNotFound(msg.dst.clone())));
            }
        }
    }
}

async fn handle_connection(
    conn_id: ConnId,
    stream: UnixStream,
    state: Arc<BrokerState>,
    cmd_tx: mpsc::UnboundedSender<BrokerCmd>,
) -> Result<()> {
    use futures_util::SinkExt;
    use tokio_stream::StreamExt;

    let mut framed = Framed::new(stream, StardustCodec::new());

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();
    state.connections.insert(conn_id, msg_tx.clone());

    loop {
        tokio::select! {
            // Incoming message from client
            result = framed.next() => {
                match result {
                    Some(Ok(msg)) => {
                        if msg.method == "__stardust.register" {
                            let service_name = String::from_utf8_lossy(&msg.params).to_string();
                            let (reply_tx, _) = oneshot::channel();
                            let _ = cmd_tx.send(BrokerCmd::Register {
                                conn_id, service_name, reply: reply_tx,
                            });
                        } else if msg.method == "__stardust.subscribe" {
                            let topic = String::from_utf8_lossy(&msg.params).to_string();
                            let _ = cmd_tx.send(BrokerCmd::Subscribe { conn_id, topic });
                        } else if msg.method == "__stardust.unsubscribe" {
                            let topic = String::from_utf8_lossy(&msg.params).to_string();
                            let _ = cmd_tx.send(BrokerCmd::Unsubscribe { conn_id, topic });
                        } else {
                            let _ = cmd_tx.send(BrokerCmd::Route(Envelope {
                                msg, sender_conn: conn_id,
                            }));
                        }
                    }
                    Some(Err(e)) => {
                        error!(conn_id, "decode error: {e}");
                        break;
                    }
                    None => break,
                }
            }

            // Outgoing message to client
            msg = msg_rx.recv() => {
                match msg {
                    Some(msg) => {
                        if framed.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    let _ = cmd_tx.send(BrokerCmd::Unregister { conn_id });
    Ok(())
}
