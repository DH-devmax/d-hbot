//! Outer NIM header observed in the SDK bundled with WangShangLiao 2.8.0.
//! This parser does not validate packetLength semantics or decode inner headers.
#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub declared_length: u64,
    pub service: u8,
    pub command: u8,
    pub serial: u16,
    pub tag: u8,
    pub response_code: u16,
    pub consumed: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub enum HeaderError {
    Truncated,
    VarintTooLong,
}
impl Header {
    /// SDK WebSocket request header; the first varint is zero, not frame size.
    /// WebSocket message boundaries must be preserved by the caller.
    pub fn request(service: u8, command: u8, serial: u16) -> [u8; 6] {
        let [lo, hi] = serial.to_le_bytes();
        [0, service, command, lo, hi, 0]
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, HeaderError> {
        let mut pos = 0;
        let mut length = 0u64;
        loop {
            if pos == 5 {
                return Err(HeaderError::VarintTooLong);
            }
            let b = *bytes.get(pos).ok_or(HeaderError::Truncated)?;
            length += u64::from(b & 127) << (pos * 7);
            pos += 1;
            if b & 128 == 0 {
                break;
            }
        }
        let fields = bytes.get(pos..pos + 5).ok_or(HeaderError::Truncated)?;
        let tag = fields[4];
        let mut code = 200;
        let consumed = pos + 5;
        if tag & 2 != 0 {
            let c = bytes
                .get(consumed..consumed + 2)
                .ok_or(HeaderError::Truncated)?;
            code = u16::from_le_bytes([c[0], c[1]]);
        }
        Ok(Self {
            declared_length: length,
            service: fields[0],
            command: fields[1],
            serial: u16::from_le_bytes([fields[2], fields[3]]),
            tag,
            response_code: code,
            consumed: consumed + if tag & 2 != 0 { 2 } else { 0 },
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outgoing_sdk_vectors() {
        assert_eq!(Header::request(1, 2, 0), [0, 1, 2, 0, 0, 0]);
        assert_eq!(Header::request(4, 10, 4660), [0, 4, 10, 52, 18, 0]);
    }
    #[test]
    fn header_byte_order_optional_status_and_bounds() {
        let h = Header::parse(&[0xac, 2, 4, 10, 0x34, 0x12, 2, 0xc8, 0]).unwrap();
        assert_eq!(
            h,
            Header {
                declared_length: 300,
                service: 4,
                command: 10,
                serial: 0x1234,
                tag: 2,
                response_code: 200,
                consumed: 9
            }
        );
        assert_eq!(Header::parse(&[5, 1, 2, 0, 0, 0]).unwrap().consumed, 6);
        for n in 0..9 {
            assert_eq!(
                Header::parse(&[0xac, 2, 4, 10, 0x34, 0x12, 2, 0xc8, 0][..n]),
                Err(HeaderError::Truncated)
            );
        }
        assert_eq!(Header::parse(&[128; 6]), Err(HeaderError::VarintTooLong));
    }
}
