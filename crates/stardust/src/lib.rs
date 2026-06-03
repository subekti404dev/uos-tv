// stardust — IPC pub/sub bus untuk UOS TV
// =============================================
// Arsitektur:
//
//   Service A ──Unix Socket──┐
//   Service B ──Unix Socket──┤
//   Service C ──Unix Socket──┤
//                             ▼
//                    ┌─────────────────┐
//                    │   stardustd     │
//                    │   (Broker)      │
//                    │                 │
//                    │  subscribers:   │
//                    │   HashMap<Topic,│
//                    │     Vec<Conn>>  │
//                    └─────────────────┘
//
// Protocol: Length-delimited CBOR frames over Unix stream sockets.
// Message types:
//   - Publish(topic, payload)    — fire and forget
//   - Subscribe(topic)           — receive matching messages
//   - Unsubscribe(topic)         — stop receiving
//   - Call(method, params) -> Response — request-response RPC

pub mod broker;
pub mod client;
pub mod codec;
pub mod error;
pub mod message;
pub mod ws;

pub use crate::message::Message;
pub use broker::{Broker, BrokerHandle};
pub use client::Client;
pub use error::{Error, Result};
