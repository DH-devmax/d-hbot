//! Parses a complete binary WebSocket message, not arbitrary TCP chunks.
//! The SDK does not use declared_length to delimit outgoing messages.
use crate::nim_header::{Header, HeaderError};
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHeader(HeaderError),
    FrameTooLarge,
}
// No Debug: body can contain account identifiers and credentials.
pub struct Packet {
    pub header: Header,
    pub body: Vec<u8>,
}
pub fn decode_message(frame: &[u8], max_bytes: usize) -> Result<Packet, Error> {
    if frame.len() > max_bytes {
        return Err(Error::FrameTooLarge);
    }
    let header = Header::parse(frame).map_err(Error::InvalidHeader)?;
    let body = frame[header.consumed..].to_vec();
    Ok(Packet { header, body })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Pending(u16),
    Confirmed(u16),
    Unknown(u16),
}
#[derive(Debug, Default)]
pub struct Correlator {
    pending: std::collections::BTreeSet<u16>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    Connected,
    Reconnecting,
    Kicked,
}
#[derive(Debug)]
pub struct ConnectionMachine {
    state: ConnectionState,
    generation: u64,
}
impl Default for ConnectionMachine {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            generation: 0,
        }
    }
}
impl ConnectionMachine {
    pub fn state(&self) -> ConnectionState {
        self.state
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.state = ConnectionState::Connecting;
        self.generation
    }
    pub fn transport_connected(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.state != ConnectionState::Connecting {
            return false;
        }
        self.state = ConnectionState::Authenticating;
        true
    }
    /// Call only after the wire response has passed authentication validation.
    pub fn authenticated(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.state != ConnectionState::Authenticating {
            return false;
        }
        self.state = ConnectionState::Connected;
        true
    }
    pub fn disconnect(&mut self) {
        self.state = ConnectionState::Reconnecting;
        self.generation += 1
    }
    pub fn kicked(&mut self) {
        self.state = ConnectionState::Kicked;
        self.generation += 1
    }
    pub fn logout(&mut self) {
        self.state = ConnectionState::Disconnected;
        self.generation += 1
    }
}
impl Correlator {
    pub fn sent(&mut self, serial: u16) -> Delivery {
        self.pending.insert(serial);
        Delivery::Pending(serial)
    }
    pub fn ack(&mut self, serial: u16) -> Delivery {
        if self.pending.remove(&serial) {
            Delivery::Confirmed(serial)
        } else {
            Delivery::Unknown(serial)
        }
    }
    pub fn timeout(&self, serial: u16) -> Delivery {
        Delivery::Unknown(serial)
    }
    pub fn pending(&self, serial: u16) -> bool {
        self.pending.contains(&serial)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn websocket_boundary_not_declared_length() {
        let mut bytes = Header::request(2, 3, 9).to_vec();
        bytes.extend_from_slice(&[1, 2]);
        let packet = decode_message(&bytes, 64).unwrap();
        assert_eq!(packet.header.declared_length, 0);
        assert_eq!(packet.header.serial, 9);
        assert_eq!(packet.body, [1, 2]);
        assert!(matches!(
            decode_message(&bytes, 7),
            Err(Error::FrameTooLarge)
        ));
        for n in 0..6 {
            assert!(matches!(
                decode_message(&bytes[..n], 64),
                Err(Error::InvalidHeader(HeaderError::Truncated))
            ));
        }
        // A failed message does not retain bytes that could corrupt the next message.
        assert_eq!(decode_message(&bytes, 64).unwrap().body, [1, 2]);
    }
    #[test]
    fn unknown_result_is_not_retried_and_duplicate_ack_is_ignored() {
        let mut c = Correlator::default();
        assert_eq!(c.sent(7), Delivery::Pending(7));
        assert_eq!(c.timeout(7), Delivery::Unknown(7));
        assert!(c.pending(7));
        assert_eq!(c.ack(7), Delivery::Confirmed(7));
        assert_eq!(c.ack(7), Delivery::Unknown(7));
        assert!(!c.pending(7));
    }
    #[test]
    fn stale_auth_cannot_restore_logout_or_replace_new_connection() {
        let mut m = ConnectionMachine::default();
        let old = m.begin();
        assert!(m.transport_connected(old));
        m.logout();
        assert!(!m.authenticated(old));
        assert_eq!(m.state(), ConnectionState::Disconnected);
        let current = m.begin();
        assert!(!m.transport_connected(old));
        assert!(m.transport_connected(current));
        assert!(!m.authenticated(old));
        assert!(m.authenticated(current));
        m.kicked();
        assert!(!m.authenticated(current));
        assert_eq!(m.state(), ConnectionState::Kicked);
    }
    #[test]
    fn connection_requires_auth_before_connected() {
        let mut m = ConnectionMachine::default();
        assert_eq!(m.state(), ConnectionState::Disconnected);
        let attempt = m.begin();
        assert_eq!(m.state(), ConnectionState::Connecting);
        assert!(!m.authenticated(attempt));
        assert!(m.transport_connected(attempt));
        assert!(m.authenticated(attempt));
        assert_eq!(m.state(), ConnectionState::Connected);
        let g = m.generation();
        m.disconnect();
        assert_eq!(m.state(), ConnectionState::Reconnecting);
        assert!(m.generation() > g);
        m.kicked();
        assert_eq!(m.state(), ConnectionState::Kicked);
        m.logout();
        assert_eq!(m.state(), ConnectionState::Disconnected);
    }
}
