// Stardust Client
// ================
// Client library untuk koneksi ke stardustd broker.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;
use tracing::{debug, warn};

use crate::codec::StardustCodec;
use crate::message::{Message, Response};
use crate::{Error, Result};

#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<HashMap<uuid::Uuid, oneshot::Sender<Response>>>>,
    service_name: Arc<Mutex<Option<String>>>,
    subscribers: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Message>>>>>,
}

impl Client {
    pub async fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        use futures_util::SinkExt;
        use tokio_stream::StreamExt;

        let stream = UnixStream::connect(socket_path).await?;
        let mut framed = Framed::new(stream, StardustCodec::new());

        let (tx, mut tx_rx) = mpsc::unbounded_channel::<Message>();
        let pending: Arc<Mutex<HashMap<uuid::Uuid, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let subscribers: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Message>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let service_name = Arc::new(Mutex::new(None::<String>));

        let reader_pending = pending.clone();
        let reader_subscribers = subscribers.clone();

        // Spawn I/O handler
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Incoming from broker
                    result = framed.next() => {
                        match result {
                            Some(Ok(msg)) => {
                                debug!("received: {} from {}", msg.method, msg.src);

                                // Check for pending RPC response
                                let mut pending_map = reader_pending.lock();
                                if let Some(sender) = pending_map.remove(&msg.id) {
                                    let response = match ciborium::from_reader(&msg.params[..]) {
                                        Ok(result) => Response {
                                            correlation_id: msg.id,
                                            result: Ok(result),
                                        },
                                        Err(_) => Response {
                                            correlation_id: msg.id,
                                            result: Err(format!(
                                                "Failed to parse response: {}",
                                                String::from_utf8_lossy(&msg.params)
                                            )),
                                        },
                                    };
                                    let _ = sender.send(response);
                                } else {
                                    // Deliver to subscribers
                                    let subs = reader_subscribers.lock();
                                    for (pattern, senders) in subs.iter() {
                                        if msg.matches_topic(pattern) {
                                            for tx in senders {
                                                let _ = tx.send(msg.clone());
                                            }
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                warn!("read error: {e}");
                                break;
                            }
                            None => break,
                        }
                    }

                    // Outgoing to broker
                    msg = tx_rx.recv() => {
                        match msg {
                            Some(msg) => {
                                debug!("sending: {} → {}", msg.src, msg.method);
                                if framed.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        Ok(Self {
            tx,
            pending,
            service_name,
            subscribers,
        })
    }

    pub async fn register(&self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        let cbor = serde_json::to_vec(&name)?;
        let msg = Message::new("__stardust.register")
            .src(name.clone())
            .params_raw(cbor);
        self.tx.send(msg).map_err(|_| Error::ConnectionClosed)?;
        *self.service_name.lock() = Some(name);
        Ok(())
    }

    pub async fn publish(&self, msg: Message) -> Result<()> {
        self.tx.send(msg).map_err(|_| Error::ConnectionClosed)?;
        Ok(())
    }

    pub async fn subscribe(
        &self,
        topic: impl Into<String>,
    ) -> Result<mpsc::UnboundedReceiver<Message>> {
        let topic = topic.into();
        let cbor = serde_json::to_vec(&topic)?;
        let msg = Message::new("__stardust.subscribe")
            .src(self.service_name.lock().clone().unwrap_or_default())
            .params_raw(cbor);
        self.tx.send(msg).map_err(|_| Error::ConnectionClosed)?;

        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers.lock().entry(topic).or_default().push(tx);
        Ok(rx)
    }

    pub async fn unsubscribe(&self, topic: &str) -> Result<()> {
        let cbor = serde_json::to_vec(topic)?;
        let msg = Message::new("__stardust.unsubscribe")
            .src(self.service_name.lock().clone().unwrap_or_default())
            .params_raw(cbor);
        self.tx.send(msg).map_err(|_| Error::ConnectionClosed)?;
        self.subscribers.lock().remove(topic);
        Ok(())
    }

    pub async fn call<T: serde::Serialize>(&self, method: &str, params: &T) -> Result<Vec<u8>> {
        use std::time::Duration;

        let msg = Message::new(method)
            .src(self.service_name.lock().clone().unwrap_or_default())
            .params_raw(serde_json::to_vec(params)?);

        let correlation_id = msg.id;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(correlation_id, tx);
        self.tx.send(msg).map_err(|_| Error::ConnectionClosed)?;

        let timeout = Duration::from_secs(30);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => response.result.map_err(|e| Error::Broker(e)),
            Ok(Err(_)) => Err(Error::NoResponse(correlation_id)),
            Err(_) => {
                self.pending.lock().remove(&correlation_id);
                Err(Error::Timeout(correlation_id))
            }
        }
    }

    pub async fn call_typed<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<R> {
        let raw = self.call(method, params).await?;
        let result: R = serde_json::from_slice(&raw)?;
        Ok(result)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let topics: Vec<String> = self.subscribers.lock().keys().cloned().collect();
        for topic in topics {
            let cbor = match serde_json::to_vec(&topic) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let msg = Message::new("__stardust.unsubscribe").params_raw(cbor);
            let _ = self.tx.send(msg);
        }
    }
}
