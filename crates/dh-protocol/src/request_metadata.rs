//! 2.8.0 X-Request scalar wire encoding, verified against the native serializer.
//! Field meanings and required values remain unresolved: this is not a login API.
use std::hash::Hasher;
use zeroize::Zeroizing;

/// Native 2.8.0 field 9 primitive. Input is exactly three little-endian words.
/// Callers must establish their sources and the account key independently;
/// this function does not infer fields or authorize a request.
pub fn field9_mac(input: &[u8; 24], key: &[u8; 16]) -> u64 {
    let mut mac = siphasher::sip::SipHasher24::new_with_key(key);
    mac.write(input);
    mac.finish()
}

#[derive(Debug, PartialEq, Eq)]
pub struct OutOfRange {
    pub field: usize,
}

/// Accept raw integer bit patterns in field-number order. Signed int32 fields
/// use their u32 bit pattern; reject wider inputs rather than silently truncate.
/// This constructs new known-field metadata, not a decoder or unknown-field proxy.
pub fn encode(fields: &[u64; 14]) -> Result<Zeroizing<Vec<u8>>, OutOfRange> {
    for field in [2, 3, 4, 5, 7, 12, 13] {
        if fields[field - 1] > u32::MAX as u64 {
            return Err(OutOfRange { field });
        }
    }
    let mut out = Zeroizing::new(Vec::with_capacity(154));
    for (index, &raw) in fields.iter().enumerate() {
        if raw == 0 {
            continue;
        }
        let field = index + 1;
        let value = if matches!(field, 2 | 3 | 4 | 5 | 7 | 12) {
            raw as u32 as i32 as i64 as u64
        } else {
            raw
        };
        varint((field as u64) << 3, &mut out);
        varint(value, &mut out);
    }
    Ok(out)
}

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Corpus {
        vectors: Vec<Vector>,
    }
    #[derive(Deserialize)]
    struct Vector {
        fields: [u64; 14],
        wire_hex: String,
    }

    #[test]
    fn native_field9_mac_vectors() {
        // Matched against hash-pinned native +0x27d510 and libsodium by
        // tools/verify_native_signature.py; exclusively synthetic inputs.
        let expected = [
            0x77cda97233d8f304,
            0x80113eb1e3e30e2e,
            0x4e032477ec15d740,
            0x1d50511edbb70f6c,
            0xbfbdd8c1b9b8b951,
            0x1fea8e2c439acb3d,
            0x5242244124713f97,
            0xa16895fb40a26c3d,
        ];
        for (variant, expected) in expected.into_iter().enumerate() {
            let input = std::array::from_fn(|i| (i * 17 + variant * 31) as u8);
            let key = std::array::from_fn(|i| (i * 13 + variant * 29) as u8);
            assert_eq!(field9_mac(&input, &key), expected);
        }
    }

    #[test]
    fn native_280_serializer_vectors() {
        let corpus: Corpus =
            serde_json::from_str(include_str!("../tests/fixtures/request_metadata_280.json"))
                .unwrap();
        assert_eq!(corpus.vectors.len(), 148);
        for vector in corpus.vectors {
            let encoded = encode(&vector.fields).unwrap();
            let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();
            assert_eq!(hex, vector.wire_hex);
        }
    }

    #[test]
    fn rejects_integer_truncation() {
        for field in [2, 3, 4, 5, 7, 12, 13] {
            let mut fields = [0; 14];
            fields[field - 1] = 1 << 32;
            assert_eq!(encode(&fields), Err(OutOfRange { field }));
        }
    }
}
