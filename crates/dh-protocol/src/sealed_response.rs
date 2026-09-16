//! Verified 2.8.0 response sealed-box layer only, not a login client.
use crate::CodecError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use crypto_box::SecretKey;
use zeroize::Zeroizing;

/// Strict canonical unpadded Base64URL profile for the verified response layer.
/// Unlike the native permissive decoder, reject whitespace, padding and suffixes.
/// This is an explicit profile, not a claim that all server responses use it.
pub fn open_base64url(
    encoded: &[u8],
    key: &SecretKey,
    max_plaintext: usize,
) -> Result<Zeroizing<Vec<u8>>, CodecError> {
    let max_ciphertext = max_plaintext
        .checked_add(48)
        .ok_or(CodecError::InvalidLength)?;
    let max_encoded =
        base64::encoded_len(max_ciphertext, false).ok_or(CodecError::InvalidLength)?;
    if encoded.len() > max_encoded {
        return Err(CodecError::InvalidLength);
    }
    let ciphertext = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| CodecError::AuthenticationFailed)?,
    );
    open(&ciphertext, key, max_plaintext)
}

/// Decode the layer observed at native +0x19f64. The caller supplies an
/// explicit resource limit and the independently established X25519 key.
/// No unauthenticated plaintext is returned and errors contain no input data.
pub fn open(
    ciphertext: &[u8],
    key: &SecretKey,
    max_plaintext: usize,
) -> Result<Zeroizing<Vec<u8>>, CodecError> {
    let length = ciphertext
        .len()
        .checked_sub(48)
        .ok_or(CodecError::InvalidLength)?;
    // The observed response caller requires at least one plaintext byte.
    if length == 0 || length > max_plaintext {
        return Err(CodecError::InvalidLength);
    }
    key.unseal(ciphertext)
        .map(Zeroizing::new)
        .map_err(|_| CodecError::AuthenticationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Corpus {
        synthetic_private_key_hex: String,
        public_key_hex: String,
        vectors: Vec<Vector>,
    }
    #[derive(Deserialize)]
    struct Vector {
        plaintext_hex: String,
        ciphertext_hex: String,
    }

    fn bytes(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn native_interop_and_failure_boundaries() {
        let corpus: Corpus =
            serde_json::from_str(include_str!("../tests/fixtures/sealed_box_280.json")).unwrap();
        let key = SecretKey::from(
            <[u8; 32]>::try_from(bytes(&corpus.synthetic_private_key_hex)).unwrap(),
        );
        assert_eq!(
            key.public_key().as_bytes().as_slice(),
            bytes(&corpus.public_key_hex)
        );
        assert_eq!(corpus.vectors.len(), 5);
        for vector in corpus.vectors {
            let plain = bytes(&vector.plaintext_hex);
            let cipher = bytes(&vector.ciphertext_hex);
            let encoded = URL_SAFE_NO_PAD.encode(&cipher);
            assert_eq!(
                &*open_base64url(encoded.as_bytes(), &key, plain.len()).unwrap(),
                &plain
            );
            for suffix in ["!ignored", "=", "\n", " "] {
                assert!(
                    open_base64url(format!("{encoded}{suffix}").as_bytes(), &key, 2048).is_err()
                );
            }
            assert_eq!(
                open_base64url(encoded.as_bytes(), &key, plain.len() - 1),
                Err(CodecError::InvalidLength)
            );
            assert_eq!(&*open(&cipher, &key, plain.len()).unwrap(), &plain);
            assert_eq!(
                open(&cipher, &key, plain.len() - 1),
                Err(CodecError::InvalidLength)
            );
            assert_eq!(
                open(&cipher, &SecretKey::from([99; 32]), 1024),
                Err(CodecError::AuthenticationFailed)
            );
            for index in [0, 31, 32, 47, cipher.len() - 1] {
                let mut damaged = cipher.clone();
                damaged[index] ^= 1;
                assert_eq!(
                    open(&damaged, &key, 1024),
                    Err(CodecError::AuthenticationFailed)
                );
            }
            for cut in 0..cipher.len() {
                assert!(open(&cipher[..cut], &key, 1024).is_err());
            }
        }
    }

    #[test]
    fn encoded_limits_and_noncanonical_input() {
        let key = SecretKey::from([0; 32]);
        assert_eq!(
            open_base64url(b"", &key, usize::MAX),
            Err(CodecError::InvalidLength)
        );
        for input in [b"TR".as_slice(), b"T", b"@@", b"+/", b"TQ==", b" TQ"] {
            assert!(open_base64url(input, &key, 1024).is_err());
        }
    }
}
