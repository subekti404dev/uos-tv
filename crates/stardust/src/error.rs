use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CBOR serialization error: {0}")]
    Cbor(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("CBOR deserialization error: {0}")]
    CborDe(#[from] ciborium::de::Error<std::io::Error>),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Timeout waiting for response (id: {0})")]
    Timeout(uuid::Uuid),

    #[error("No response received for request (id: {0})")]
    NoResponse(uuid::Uuid),

    #[error("Broker error: {0}")]
    Broker(String),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Service not found: {0}")]
    ServiceNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
