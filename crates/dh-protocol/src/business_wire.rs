//! Native mode-1 business envelope, independently verified against live requests.
use crate::MessageCodec;
use std::io::Read;
use xxhash_rust::xxh3::{xxh3_128, xxh3_64};
use zeroize::Zeroizing;

pub const MAX_BODY: usize = 1024 * 1024;
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    InvalidEnvelope,
    Authentication,
    Compression,
}

#[derive(Clone, Copy)]
pub struct Binding {
    pub request_id: u64,
    pub plaintext_hash: u64,
    pub device_tag: u64,
}
impl Binding {
    pub fn new(request_id: u64, plaintext: &[u8], device_tag: u64) -> Self {
        Self {
            request_id,
            plaintext_hash: if plaintext.is_empty() {
                0
            } else {
                xxh3_64(plaintext)
            },
            device_tag,
        }
    }
    pub fn aad(&self) -> [u8; 16] {
        let mut input = [0; 32];
        for (out, value) in input.as_chunks_mut::<8>().0.iter_mut().zip([
            self.request_id,
            self.plaintext_hash,
            self.device_tag,
            0x019c411fdeaf,
        ]) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        xxh3_128(&input).to_le_bytes()
    }
}

/// Zstd raw-block frames with an explicit content size. Pure Rust and bounded.
/// Compression ratio is intentionally traded for a simple interoperable encoder.
fn zstd_frame(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 40);
    out.extend_from_slice(&[0x28, 0xb5, 0x2f, 0xfd, 0xa0]);
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    if input.is_empty() {
        out.extend_from_slice(&[1, 0, 0]);
    }
    let chunks = input.chunks(128 * 1024);
    let count = chunks.len();
    for (i, chunk) in chunks.enumerate() {
        let header = ((chunk.len() as u32) << 3) | u32::from(i + 1 == count);
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
    }
    out
}

pub fn seal(
    key: [u8; 32],
    binding: Binding,
    plaintext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    if plaintext.len() > MAX_BODY {
        return Err(Error::TooLarge);
    }
    if Binding::new(binding.request_id, plaintext, binding.device_tag).plaintext_hash
        != binding.plaintext_hash
    {
        return Err(Error::InvalidEnvelope);
    }
    let compressed = Zeroizing::new(zstd_frame(plaintext));
    let mut nonce_input = Zeroizing::new(binding.request_id.to_le_bytes().to_vec());
    nonce_input.extend_from_slice(&compressed);
    let material = xxh3_128(&nonce_input).to_le_bytes();
    let sealed = Zeroizing::new(
        MessageCodec::new(key)
            .seal(
                material[..8].try_into().unwrap(),
                &binding.aad(),
                &compressed,
            )
            .map_err(|_| Error::Authentication)?,
    );
    let split = sealed.len() - 16;
    let mut out = Zeroizing::new(Vec::with_capacity(sealed.len() + 16));
    out.extend_from_slice(&sealed[split..]);
    out.extend_from_slice(&material);
    out.extend_from_slice(&sealed[..split]);
    Ok(out)
}

pub fn open(key: [u8; 32], binding: Binding, body: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    if body.len() < 33 {
        return Err(Error::InvalidEnvelope);
    }
    if body.len() > MAX_BODY + 4096 {
        return Err(Error::TooLarge);
    }
    let mut sealed = Zeroizing::new(body[32..].to_vec());
    sealed.extend_from_slice(&body[..16]);
    let compressed = MessageCodec::new(key)
        .open(body[16..24].try_into().unwrap(), &binding.aad(), &sealed)
        .map_err(|_| Error::Authentication)?;
    let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
        compressed.as_slice(),
        MAX_BODY as u64,
    )
    .map_err(|_| Error::Compression)?;
    let mut plain = Zeroizing::new(Vec::new());
    decoder
        .by_ref()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut plain)
        .map_err(|_| Error::Compression)?;
    if plain.len() > MAX_BODY {
        return Err(Error::TooLarge);
    }
    if !decoder.into_inner().is_empty() {
        return Err(Error::Compression);
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_libsodium_zstd_envelope() {
        // PyNaCl/libsodium + libzstd + python-xxhash, synthetic input only.
        let decode = |s: &str| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let body=decode("2cb4326bbc156bf266de89f03baf3d668bae2141b46cf95e39f54a7b9dfc9774f814ac191a984faca4160b4fbce0a4d3c199b112c0df126ede8ff0");
        let plain = br#"{"synthetic":true}"#;
        let binding = Binding::new(42, plain, 99);
        assert_eq!(binding.plaintext_hash, 1_991_075_964_100_698_326);
        assert_eq!(
            binding.aad().as_slice(),
            decode("81a175cbdd38b3b2f20c00678eacef89")
        );
        let key = std::array::from_fn(|i| i as u8);
        assert_eq!(open(key, binding, &body).unwrap().as_slice(), plain);
    }
    #[test]
    fn boundaries_and_binding_authentication() {
        for len in [0, 1, 255, 256, 131072, 131073, MAX_BODY] {
            let plain = vec![b'x'; len];
            let binding = Binding::new(42, &plain, 99);
            let mut body = seal([7; 32], binding, &plain).unwrap();
            assert_eq!(&*open([7; 32], binding, &body).unwrap(), &plain);
            assert!(matches!(
                open(
                    [7; 32],
                    Binding {
                        request_id: 43,
                        ..binding
                    },
                    &body
                ),
                Err(Error::Authentication)
            ));
            body[0] ^= 1;
            assert!(matches!(
                open([7; 32], binding, &body),
                Err(Error::Authentication)
            ));
        }
    }
}
