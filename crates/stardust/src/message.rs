use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Pesan yang dikirim melalui Stardust bus.
/// Semua field diserialize sebagai CBOR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// ID unik untuk request-response correlation
    pub id: Uuid,

    /// Service pengirim (contoh: "netmd", "otad")
    pub src: String,

    /// Service tujuan — kosong untuk broadcast/topic
    #[serde(default)]
    pub dst: String,

    /// Topic atau method name (contoh: "network.status", "ota.check_update")
    pub method: String,

    /// Parameter sebagai CBOR-encoded bytes (raw, tidak di-decode broker)
    #[serde(with = "serde_bytes")]
    pub params: Vec<u8>,

    /// UNIX timestamp (detik) saat pesan dibuat
    #[serde(default)]
    pub ts: u64,
}

/// Method call descriptor — digunakan oleh Client::call()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
}

/// Response dari method call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Correlation ID — harus sama dengan request ID
    pub correlation_id: Uuid,

    /// Result sebagai CBOR bytes (Ok) atau error message (Err)
    pub result: std::result::Result<Vec<u8>, String>,
}

impl Message {
    /// Buat pesan publish baru.
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            src: String::new(),
            dst: String::new(),
            method: method.into(),
            params: Vec::new(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Set service pengirim.
    pub fn src(mut self, src: impl Into<String>) -> Self {
        self.src = src.into();
        self
    }

    /// Set service tujuan (direct RPC).
    pub fn dst(mut self, dst: impl Into<String>) -> Self {
        self.dst = dst.into();
        self
    }

    /// Attach parameter yang sudah diserialize ke CBOR.
    pub fn params_raw(mut self, params: Vec<u8>) -> Self {
        self.params = params;
        self
    }

    /// Attach parameter dengan serialisasi otomatis.
    /// Accumulates: multiple param() calls build up the params object.
    pub fn param<T: Serialize>(mut self, key: &str, value: &T) -> Result<Self, serde_json::Error> {
        // Parse existing params if any
        let mut map: serde_json::Map<String, serde_json::Value> = if self.params.is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_slice(&self.params).unwrap_or_default()
        };
        map.insert(key.to_string(), serde_json::to_value(value)?);
        let json = serde_json::Value::Object(map);
        let raw = serde_json::to_vec(&json)?;
        self.params = raw;
        Ok(self)
    }

    /// Check apakah pesan ini cocok dengan pattern topic.
    /// Pattern support wildcard: "network.*" cocok dengan "network.status", dll.
    pub fn matches_topic(&self, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if let Some(prefix) = pattern.strip_suffix(".*") {
            self.method.starts_with(prefix)
        } else {
            self.method == pattern
        }
    }
}

// Helper module untuk serde_bytes
mod serde_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        bytes.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        <Vec<u8>>::deserialize(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_matching() {
        let msg = Message::new("network.status");
        assert!(msg.matches_topic("network.*"));
        assert!(msg.matches_topic("network.status"));
        assert!(msg.matches_topic("*"));
        assert!(!msg.matches_topic("audio.*"));
    }

    #[test]
    fn test_message_params() {
        let msg = Message::new("test.event")
            .src("testd")
            .param("connected", &true)
            .unwrap();

        assert_eq!(msg.src, "testd");
        assert!(!msg.params.is_empty());
    }
}
