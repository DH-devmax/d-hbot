use dh_protocol::{CodecError, MessageCodec};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vector {
    key: String,
    nonce: String,
    aad: String,
    plaintext: String,
    sealed: String,
}

fn hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn independent_libsodium_vectors_and_tamper_rejection() {
    let vectors: Vec<Vector> = serde_json::from_str(include_str!("legacy-vectors.json")).unwrap();
    assert_eq!(vectors.len(), 20);
    for v in vectors {
        let key: [u8; 32] = hex(&v.key).try_into().unwrap();
        let nonce: [u8; 8] = hex(&v.nonce).try_into().unwrap();
        let aad: [u8; 8] = hex(&v.aad).try_into().unwrap();
        let plaintext = hex(&v.plaintext);
        let sealed = hex(&v.sealed);
        let codec = MessageCodec::new(key);
        assert_eq!(codec.seal(&nonce, &aad, &plaintext).unwrap(), sealed);
        assert_eq!(*codec.open(&nonce, &aad, &sealed).unwrap(), plaintext);
        let mut wrong_key = key;
        wrong_key[0] ^= 1;
        assert_eq!(
            MessageCodec::new(wrong_key).open(&nonce, &aad, &sealed),
            Err(CodecError::AuthenticationFailed)
        );
        let mut wrong_nonce = nonce;
        wrong_nonce[0] ^= 1;
        assert_eq!(
            codec.open(&wrong_nonce, &aad, &sealed),
            Err(CodecError::AuthenticationFailed)
        );
        let mut wrong_aad = aad;
        wrong_aad[0] ^= 1;
        assert_eq!(
            codec.open(&nonce, &wrong_aad, &sealed),
            Err(CodecError::AuthenticationFailed)
        );
        for index in 0..sealed.len() {
            let mut modified = sealed.clone();
            modified[index] ^= 1;
            assert_eq!(
                codec.open(&nonce, &aad, &modified),
                Err(CodecError::AuthenticationFailed)
            );
        }
        for length in 0..sealed.len() {
            assert!(codec.open(&nonce, &aad, &sealed[..length]).is_err());
        }
    }
}
