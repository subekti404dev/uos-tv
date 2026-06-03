// Codec untuk frame-based protocol di atas Unix stream socket.
//
// Wire format:
//   ┌──────────────┬─────────────────────────┐
//   │  4 bytes LE  │  N bytes CBOR-encoded   │
//   │  frame len   │  Message                │
//   └──────────────┴─────────────────────────┘
//
// Ini memungkinkan multiple message dikirim dalam satu koneksi TCP/Unix
// tanpa perlu delimiter khusus.

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::Result;
use crate::message::Message;

pub struct StardustCodec;

impl StardustCodec {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StardustCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder<Message> for StardustCodec {
    type Error = crate::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<()> {
        // Serialize message ke CBOR bytes
        let mut cbor_bytes = Vec::new();
        ciborium::into_writer(&item, &mut cbor_bytes)?;

        // Tulis frame: 4 bytes length (little-endian) + payload
        let len = cbor_bytes.len() as u32;
        dst.reserve(4 + len as usize);
        dst.put_u32_le(len);
        dst.extend_from_slice(&cbor_bytes);

        Ok(())
    }
}

impl Decoder for StardustCodec {
    type Item = Message;
    type Error = crate::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        // Butuh minimal 4 bytes untuk baca frame length
        if src.len() < 4 {
            return Ok(None);
        }

        // Baca frame length (tanpa consume dulu)
        let len = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;

        // Belum semua data tersedia
        if src.len() < 4 + len {
            src.reserve(4 + len - src.len());
            return Ok(None);
        }

        // Consume header
        src.advance(4);

        // Decode CBOR payload
        let payload = src.split_to(len);
        let msg: Message = ciborium::from_reader(&payload[..])?;

        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_roundtrip() {
        let msg = crate::Message::new("test.hello")
            .src("tester")
            .dst("target");

        let mut codec = StardustCodec::new();
        let mut buf = BytesMut::new();

        // Encode
        codec.encode(msg.clone(), &mut buf).unwrap();

        // Decode
        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded.method, msg.method);
        assert_eq!(decoded.src, msg.src);
        assert_eq!(decoded.dst, msg.dst);
        assert_eq!(decoded.id, msg.id);
    }

    #[test]
    fn test_partial_frame() {
        let mut codec = StardustCodec::new();
        let mut buf = BytesMut::new();

        // Hanya header (4 bytes) — incomplete frame
        buf.put_u32_le(9999);
        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_none());
        assert_eq!(buf.len(), 4); // Header masih ada
    }

    #[test]
    fn test_multiple_messages() {
        let msg1 = crate::Message::new("first");
        let msg2 = crate::Message::new("second");

        let mut codec = StardustCodec::new();
        let mut buf = BytesMut::new();

        codec.encode(msg1.clone(), &mut buf).unwrap();
        codec.encode(msg2.clone(), &mut buf).unwrap();

        let d1 = codec.decode(&mut buf).unwrap().unwrap();
        let d2 = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(d1.method, "first");
        assert_eq!(d2.method, "second");
        assert!(buf.is_empty());
    }
}
