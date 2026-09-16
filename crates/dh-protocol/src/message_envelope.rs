//! Authenticated custom-message envelope from the pinned 2.7.7 native decoder.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use prost::Message;
use zeroize::Zeroizing;

const MAX_MESSAGE: usize = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Size,
    Envelope,
    Authentication,
    Compression,
    Binding,
}

#[derive(Clone, PartialEq, Message)]
struct Envelope {
    #[prost(fixed64, tag = "1")]
    header: u64,
    #[prost(fixed64, tag = "2")]
    timestamp: u64,
    #[prost(fixed64, tag = "3")]
    nonce: u64,
    #[prost(bytes = "vec", tag = "4")]
    body: Vec<u8>,
    #[prost(uint64, tag = "6")]
    aad: u64,
    #[prost(bytes = "vec", tag = "7")]
    tag: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Source {
    #[prost(int32, tag = "1")]
    pub id: i32,
    #[prost(string, tag = "2")]
    pub name: String,
}
#[derive(Clone, PartialEq, Message)]
pub struct Content {
    #[prost(string, tag = "1")]
    pub data: String,
}
#[derive(Clone, PartialEq, Message)]
pub struct Mention {
    #[prost(int32, tag = "1")]
    pub start: i32,
    #[prost(int32, tag = "2")]
    pub end: i32,
    #[prost(uint32, tag = "3")]
    pub uid: u32,
    #[prost(string, tag = "4")]
    pub nick: String,
}
#[derive(Clone, PartialEq, Message)]
pub struct Mentions {
    #[prost(message, optional, tag = "1")]
    pub content: Option<Content>,
    #[prost(message, repeated, tag = "2")]
    pub people: Vec<Mention>,
}
#[derive(Clone, PartialEq, Message)]
pub struct ApplicationMessage {
    #[prost(message, optional, tag = "1")]
    pub from: Option<Source>,
    #[prost(message, optional, tag = "2")]
    pub to: Option<Source>,
    #[prost(int64, tag = "3")]
    pub created_at: i64,
    #[prost(int32, tag = "6")]
    pub version: i32,
    #[prost(string, tag = "50")]
    pub client_id: String,
    #[prost(int32, tag = "4")]
    pub device: i32,
    #[prost(int32, tag = "5")]
    pub session: i32,
    #[prost(int32, tag = "11")]
    pub format: i32,
    #[prost(message, optional, tag = "100")]
    pub content: Option<Content>,
    #[prost(message, optional, tag = "105")]
    pub mentions: Option<Mentions>,
}

/// Authentication precedes decompression; outer routing must also match the caller.
pub fn open(
    key: [u8; 32],
    input: &str,
    sender: i32,
    target: i32,
    session: i32,
) -> Result<ApplicationMessage, Error> {
    let message = open_authenticated(key, input)?;
    if message.from.as_ref().map(|s| s.id) != Some(sender)
        || message.to.as_ref().map(|s| s.id) != Some(target)
        || message.session != session
    {
        return Err(Error::Binding);
    }
    Ok(message)
}

/// Validates the authenticated inner routing. The adapter must separately bind
/// business identifiers to the provider account and cloud group identifiers.
pub fn open_authenticated(key: [u8; 32], input: &str) -> Result<ApplicationMessage, Error> {
    if input.len() > MAX_MESSAGE * 2 {
        return Err(Error::Size);
    }
    let encoded = if input.starts_with('{') {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wrapper {
            b: String,
        }
        serde_json::from_str::<Wrapper>(input)
            .map_err(|_| Error::Envelope)?
            .b
    } else {
        input.to_owned()
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| Error::Envelope)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(Error::Size);
    }
    let envelope = Envelope::decode(bytes.as_slice()).map_err(|_| Error::Envelope)?;
    // Unsigned legacy payloads are deliberately not admitted to the AI pipeline.
    if envelope.aad == 0 || envelope.tag.len() != 16 {
        return Err(Error::Authentication);
    }
    let mut sealed = Zeroizing::new(envelope.body);
    sealed.extend_from_slice(&envelope.tag);
    let compressed = crate::MessageCodec::new(key)
        .open(
            &envelope.nonce.to_le_bytes(),
            &envelope.aad.to_le_bytes(),
            &sealed,
        )
        .map_err(|_| Error::Authentication)?;
    let size = compressed.get(..4).ok_or(Error::Compression)?;
    let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
    if size > MAX_MESSAGE {
        return Err(Error::Size);
    }
    let mut plain = Zeroizing::new(vec![0; size]);
    let written = lz4_flex::block::decompress_into(&compressed[4..], &mut plain)
        .map_err(|_| Error::Compression)?;
    if written != size {
        return Err(Error::Compression);
    }
    let message = ApplicationMessage::decode(plain.as_slice()).map_err(|_| Error::Envelope)?;
    let from = message.from.as_ref().ok_or(Error::Binding)?.id;
    let to = message.to.as_ref().ok_or(Error::Binding)?.id;
    if !(1..1 << 30).contains(&from)
        || !(1..1 << 30).contains(&to)
        || !(0..=3).contains(&message.device)
        || message.session < 0
        || envelope.header
            != ((from as u64) << 34
                | (to as u64) << 4
                | (message.session.min(3) as u64) << 2
                | message.device as u64)
    {
        return Err(Error::Binding);
    }
    Ok(message)
}

/// Encode new text messages only; never re-encode unknown incoming message types.
/// A caller must reserve a unique nonce before calling this function.
pub fn seal(
    key: [u8; 32],
    message: &ApplicationMessage,
    nonce: u64,
    aad: u64,
    timestamp: u64,
) -> Result<String, Error> {
    let from = message.from.as_ref().ok_or(Error::Binding)?.id;
    let to = message.to.as_ref().ok_or(Error::Binding)?.id;
    if !(1..1 << 30).contains(&from)
        || !(1..1 << 30).contains(&to)
        || !(1..=2).contains(&message.session)
        || message.device != 1
        || message.format != 0
        || message.mentions.is_some()
        || aad == 0
        || message
            .content
            .as_ref()
            .is_none_or(|c| c.data.trim().is_empty())
    {
        return Err(Error::Binding);
    }
    let plain = Zeroizing::new(message.encode_to_vec());
    // Native desktop decoder's output buffer is 4096 bytes.
    if plain.len() > 4096 {
        return Err(Error::Size);
    }
    let compressed = Zeroizing::new(lz4_flex::block::compress_prepend_size(&plain));
    let mut body = crate::MessageCodec::new(key)
        .seal(&nonce.to_le_bytes(), &aad.to_le_bytes(), &compressed)
        .map_err(|_| Error::Authentication)?;
    let tag = body.split_off(body.len() - 16);
    let envelope = Envelope {
        header: (from as u64) << 34 | (to as u64) << 4 | (message.session as u64) << 2 | 1,
        timestamp,
        nonce,
        aad,
        body,
        tag,
    };
    Ok(serde_json::json!({"b":URL_SAFE_NO_PAD.encode(envelope.encode_to_vec())}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outgoing_text_is_authenticated_and_bounded() {
        let message = ApplicationMessage {
            from: Some(Source {
                id: 12,
                name: "Alice".into(),
            }),
            to: Some(Source {
                id: 34,
                name: "Test".into(),
            }),
            device: 1,
            session: 2,
            version: 2,
            created_at: 1700000000000,
            client_id: "unique".into(),
            content: Some(Content {
                data: "@DH 测试".into(),
            }),
            ..Default::default()
        };
        let encoded = seal([7; 32], &message, 8, 9, 1700000000).unwrap();
        assert_eq!(open([7; 32], &encoded, 12, 34, 2).unwrap(), message);
        assert!(open([8; 32], &encoded, 12, 34, 2).is_err());
        assert!(open([7; 32], &encoded, 12, 35, 2).is_err());
        assert!(seal([7; 32], &message, 8, 0, 1).is_err());
        let mut large = message;
        large.content = Some(Content {
            data: "x".repeat(4096),
        });
        assert_eq!(seal([7; 32], &large, 8, 9, 1), Err(Error::Size));
    }
    fn fixture() -> Envelope {
        let message = ApplicationMessage {
            from: Some(Source {
                id: 12,
                name: "sender".into(),
            }),
            to: Some(Source {
                id: 34,
                name: String::new(),
            }),
            device: 1,
            session: 2,
            content: Some(Content {
                data: "@DH test".into(),
            }),
            ..Default::default()
        };
        let compressed = lz4_flex::block::compress_prepend_size(&message.encode_to_vec());
        let mut body = crate::MessageCodec::new([7; 32])
            .seal(&8u64.to_le_bytes(), &9u64.to_le_bytes(), &compressed)
            .unwrap();
        let tag = body.split_off(body.len() - 16);
        Envelope {
            header: 12 << 34 | 34 << 4 | 2 << 2 | 1,
            nonce: 8,
            aad: 9,
            body,
            tag,
            ..Default::default()
        }
    }
    fn wrap(e: &Envelope) -> String {
        serde_json::json!({"b":URL_SAFE_NO_PAD.encode(e.encode_to_vec())}).to_string()
    }
    #[test]
    fn authenticated_routing_and_tamper_rejection() {
        let mut e = fixture();
        assert_eq!(
            open([7; 32], &wrap(&e), 12, 34, 2)
                .unwrap()
                .content
                .unwrap()
                .data,
            "@DH test"
        );
        assert_eq!(open([7; 32], &wrap(&e), 12, 35, 2), Err(Error::Binding));
        e.header ^= 16;
        assert_eq!(open([7; 32], &wrap(&e), 12, 34, 2), Err(Error::Binding));
        e.body[0] ^= 1;
        assert_eq!(
            open([7; 32], &wrap(&e), 12, 34, 2),
            Err(Error::Authentication)
        );
    }
    #[test]
    fn malformed_and_unsigned_fail_closed() {
        for value in ["", "{}", "{\"b\":\"!\"}"] {
            assert!(open([7; 32], value, 12, 34, 2).is_err());
        }
        let mut e = fixture();
        e.aad = 0;
        assert_eq!(
            open([7; 32], &wrap(&e), 12, 34, 2),
            Err(Error::Authentication)
        );
    }
    #[test]
    fn independent_libsodium_lz4_fixture() {
        // Python PyNaCl + liblz4, with independently assembled protobuf fields.
        let input = "CSkCAAAwAAAAGQgAAAAAAAAAIh_VEC3xEchgKPYWqI6m77TaZXw2G3gjR8N4hEuLa_UdMAk6EN5fuQIJLALDoABHUjpguQA";
        assert_eq!(
            open([7; 32], input, 12, 34, 2)
                .unwrap()
                .content
                .unwrap()
                .data,
            "@DH test"
        );
    }
    #[test]
    fn authenticated_decompression_bomb_is_bounded() {
        let mut e = fixture();
        let mut body = crate::MessageCodec::new([7; 32])
            .seal(
                &8u64.to_le_bytes(),
                &9u64.to_le_bytes(),
                &u32::MAX.to_le_bytes(),
            )
            .unwrap();
        e.tag = body.split_off(body.len() - 16);
        e.body = body;
        assert_eq!(open([7; 32], &wrap(&e), 12, 34, 2), Err(Error::Size));
    }
}
