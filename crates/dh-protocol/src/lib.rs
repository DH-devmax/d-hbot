//! Business authentication/envelopes and NIM discovery, auth and heartbeats.
//! Deployment configuration and human validation input are supplied by the caller.
//! MessageCodec is not the IETF ChaCha20-Poly1305 construction.
pub mod message_envelope;
pub mod nim_message;

use chacha20::{
    cipher::{KeyIvInit, StreamCipher, StreamCipherSeek},
    ChaCha20Legacy,
};
use poly1305::{universal_hash::KeyInit, Poly1305};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// Deliberately opaque: errors must not include message contents or key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    InvalidLength,
    AuthenticationFailed,
}

/// Primitive recovered from the 2.7.7 arm64 decoder, not a network envelope.
/// Callers are responsible for never reusing a nonce with the same key.
/// Accepts the eight-byte message and sixteen-byte business-envelope AADs.
pub struct MessageCodec {
    key: Zeroizing<[u8; 32]>,
}

impl MessageCodec {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key: Zeroizing::new(key),
        }
    }

    fn cipher(&self, nonce: &[u8; 8]) -> ChaCha20Legacy {
        ChaCha20Legacy::new((&*self.key).into(), nonce.into())
    }

    fn tag(&self, nonce: &[u8; 8], aad: &[u8], ciphertext: &[u8]) -> Result<[u8; 16], CodecError> {
        let length = u64::try_from(ciphertext.len()).map_err(|_| CodecError::InvalidLength)?;
        let capacity = ciphertext
            .len()
            .checked_add(aad.len())
            .and_then(|n| n.checked_add(16))
            .ok_or(CodecError::InvalidLength)?;
        let mut block = Zeroizing::new([0u8; 64]);
        self.cipher(nonce).apply_keystream(&mut block[..]);
        let mac = Poly1305::new_from_slice(&block[..32]).expect("fixed Poly1305 key length");
        // Original libsodium construction: no IETF padding between these fields.
        let mut authenticated = Vec::with_capacity(capacity);
        authenticated.extend_from_slice(aad);
        authenticated.extend_from_slice(&(aad.len() as u64).to_le_bytes());
        authenticated.extend_from_slice(ciphertext);
        authenticated.extend_from_slice(&length.to_le_bytes());
        Ok(mac.compute_unpadded(&authenticated).into())
    }

    /// Returns ciphertext followed by its 16-byte tag, for primitive testing only.
    /// This concatenation is NOT claimed to be the application's wire format.
    pub fn seal(
        &self,
        nonce: &[u8; 8],
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CodecError> {
        plaintext
            .len()
            .checked_add(16)
            .ok_or(CodecError::InvalidLength)?;
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = self.cipher(nonce);
        cipher.seek(64);
        cipher
            .try_apply_keystream(&mut ciphertext)
            .map_err(|_| CodecError::InvalidLength)?;
        let tag = self.tag(nonce, aad, &ciphertext)?;
        ciphertext.extend_from_slice(&tag);
        Ok(ciphertext)
    }

    /// Authenticates before allocating/decrypting plaintext. On failure, no
    /// plaintext is returned. Successful plaintext is zeroized when dropped.
    pub fn open(
        &self,
        nonce: &[u8; 8],
        aad: &[u8],
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CodecError> {
        let split = sealed
            .len()
            .checked_sub(16)
            .ok_or(CodecError::InvalidLength)?;
        let (ciphertext, supplied_tag) = sealed.split_at(split);
        let expected = self.tag(nonce, aad, ciphertext)?;
        if !bool::from(expected.ct_eq(supplied_tag)) {
            return Err(CodecError::AuthenticationFailed);
        }
        let mut plaintext = Zeroizing::new(ciphertext.to_vec());
        let mut cipher = self.cipher(nonce);
        cipher.seek(64);
        cipher
            .try_apply_keystream(&mut plaintext)
            .map_err(|_| CodecError::InvalidLength)?;
        Ok(plaintext)
    }
}

pub mod business_client;
pub mod business_wire;
pub mod credentials;
pub mod nim_client;
pub mod nim_header;
pub mod nim_login;
pub mod nim_property;
pub mod nim_transport;
pub mod request_metadata;
pub mod sealed_response;

/// Provider-specific adapters implement these boundaries once wire evidence is verified.
pub trait BusinessClient {
    type Error;
    fn login_supported(&self) -> bool {
        false
    }
    fn refresh_supported(&self) -> bool {
        false
    }
}

/// NIM transport is deliberately capability-gated until the inner frame is recovered.
pub trait NimTransport {
    type Error;
    fn connect_supported(&self) -> bool {
        false
    }
    fn send_supported(&self) -> bool {
        false
    }
}

/// Transport-independent lifecycle used by the future pure-Rust clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    LoggedOut,
    Verifying,
    Authenticated,
    Refreshing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub state: SessionState,
    pub generation: u64,
    pub has_business: bool,
    pub has_nim: bool,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            state: SessionState::LoggedOut,
            generation: 0,
            has_business: false,
            has_nim: false,
        }
    }
}

/// Small state machine shared by HTTP business and NIM adapters.
#[derive(Debug, Default)]
pub struct SessionMachine {
    snapshot: SessionSnapshot,
}

impl SessionMachine {
    pub fn snapshot(&self) -> &SessionSnapshot {
        &self.snapshot
    }
    pub fn begin_verification(&mut self) {
        self.snapshot.state = SessionState::Verifying;
    }
    pub fn authenticated(&mut self) {
        self.snapshot.generation += 1;
        self.snapshot.state = SessionState::Authenticated;
        self.snapshot.has_business = true;
        self.snapshot.has_nim = true;
    }
    pub fn begin_refresh(&mut self) -> bool {
        if self.snapshot.state == SessionState::Authenticated {
            self.snapshot.state = SessionState::Refreshing;
            true
        } else {
            false
        }
    }
    pub fn refresh_ok(&mut self) {
        if self.snapshot.state == SessionState::Refreshing {
            self.snapshot.state = SessionState::Authenticated;
        }
    }
    pub fn invalidate(&mut self) {
        self.snapshot.state = SessionState::Invalid;
        self.snapshot.has_business = false;
        self.snapshot.has_nim = false;
    }
    pub fn logout(&mut self) {
        self.snapshot = SessionSnapshot {
            generation: self.snapshot.generation + 1,
            ..Default::default()
        };
    }
}
