// SPDX-License-Identifier: MIT
//! One encrypted two-person room per relay. The relay is trusted with the key.
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use std::io;

pub const MAX_DATAGRAM: usize = 512;
const HEADER: usize = 30;
pub const HELLO: u8 = 1;
pub const AUDIO: u8 = 2;
pub const BYE: u8 = 3;
pub const STATUS: u8 = 4;
pub const PROBE: u8 = 5;
pub const FEEDBACK: u8 = 6;
pub const PROBE_BYTES: usize = 9;
pub const FEEDBACK_BYTES: usize = 49;
#[allow(dead_code)] // Client rate accounting; not used by the relay binary.
pub const DATAGRAM_OVERHEAD: u16 = (HEADER + 16) as u16;
pub const WAITING: u8 = 0;
pub const CONNECTED: u8 = 1;
pub const OCCUPIED: u8 = 2;
pub type Session = [u8; 16];

/// Versioned cumulative receiver counters; scoped to the sender's session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedbackReport {
    pub recipient: Session,
    pub serial: u64,
    pub probe: u64,
    pub frames: u64,
    pub lost: u64,
}

impl FeedbackReport {
    #[allow(dead_code)] // Only clients originate feedback.
    pub fn encode(self) -> [u8; FEEDBACK_BYTES] {
        let mut bytes = [0; FEEDBACK_BYTES];
        bytes[0] = 1;
        bytes[1..17].copy_from_slice(&self.recipient);
        for (slot, value) in bytes[17..].as_chunks_mut::<8>().0.iter_mut().zip([
            self.serial,
            self.probe,
            self.frames,
            self.lost,
        ]) {
            slot.copy_from_slice(&value.to_be_bytes());
        }
        bytes
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != FEEDBACK_BYTES || bytes[0] != 1 {
            return None;
        }
        let word =
            |start| u64::from_be_bytes(bytes[start..start + 8].try_into().expect("fixed length"));
        let report = Self {
            recipient: bytes[1..17].try_into().ok()?,
            serial: word(17),
            probe: word(25),
            frames: word(33),
            lost: word(41),
        };
        (report.lost <= report.frames).then_some(report)
    }
}

#[allow(dead_code)] // Only clients originate probes.
pub fn probe_body(token: u64) -> [u8; PROBE_BYTES] {
    let mut bytes = [0; PROBE_BYTES];
    bytes[0] = 1;
    bytes[1..].copy_from_slice(&token.to_be_bytes());
    bytes
}

pub fn probe_token(bytes: &[u8]) -> Option<u64> {
    if bytes.len() != PROBE_BYTES || bytes[0] != 1 {
        return None;
    }
    Some(u64::from_be_bytes(bytes[1..].try_into().ok()?))
}

pub fn random_session() -> io::Result<Session> {
    let mut session = [0; 16];
    getrandom::fill(&mut session).map_err(io::Error::other)?;
    Ok(session)
}

pub fn generate_key() -> io::Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[allow(dead_code)] // The relay reads the environment; the client also accepts a TUI key.
pub fn key_from_env() -> io::Result<[u8; 32]> {
    parse_key(&std::env::var("SNACKA_CALL_KEY").map_err(|_| {
        io::Error::other("Set SNACKA_CALL_KEY to the 64 hex digits from call_relay --generate-key")
    })?)
}

pub fn parse_key(text: &str) -> io::Result<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other(
            "SNACKA_CALL_KEY must contain exactly 64 hex digits",
        ));
    }
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(io::Error::other)?;
    }
    Ok(key)
}

pub struct Message {
    pub kind: u8,
    pub role: u8,
    pub session: Session,
    pub counter: u64,
    pub body: Vec<u8>,
}

pub struct Transport {
    cipher: XChaCha20Poly1305,
    pub session: Session,
    role: u8,
    counter: u64,
}

impl Transport {
    pub fn new(key: &[u8; 32], role: u8) -> io::Result<Self> {
        Ok(Self {
            cipher: XChaCha20Poly1305::new(key.into()),
            session: random_session()?,
            role,
            counter: 0,
        })
    }

    pub fn seal(&mut self, kind: u8, body: &[u8]) -> io::Result<Vec<u8>> {
        if body.len() + HEADER + 16 > MAX_DATAGRAM {
            return Err(io::Error::other("Datagram too large"));
        }
        let counter = self.counter;
        self.counter = counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Nonce exhausted"))?;
        let mut header = Vec::with_capacity(HEADER);
        header.extend_from_slice(b"SNC1");
        header.extend_from_slice(&[kind, self.role]);
        header.extend_from_slice(&self.session);
        header.extend_from_slice(&counter.to_be_bytes());
        let mut nonce = [0; 24];
        nonce[..16].copy_from_slice(&self.session);
        nonce[16..].copy_from_slice(&counter.to_be_bytes());
        let encrypted = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: body,
                    aad: &header,
                },
            )
            .map_err(|_| io::Error::other("Encryption failed"))?;
        header.extend_from_slice(&encrypted);
        Ok(header)
    }

    pub fn open(&self, bytes: &[u8]) -> Option<Message> {
        if !(HEADER + 16..=MAX_DATAGRAM).contains(&bytes.len()) || &bytes[..4] != b"SNC1" {
            return None;
        }
        let session = bytes[6..22].try_into().ok()?;
        let counter = u64::from_be_bytes(bytes[22..30].try_into().ok()?);
        let body = self
            .cipher
            .decrypt(
                XNonce::from_slice(&bytes[6..30]),
                Payload {
                    msg: &bytes[HEADER..],
                    aad: &bytes[..HEADER],
                },
            )
            .ok()?;
        Some(Message {
            kind: bytes[4],
            role: bytes[5],
            session,
            counter,
            body,
        })
    }
}

/// Reject duplicates without rejecting modest UDP reordering.
#[derive(Default)]
pub struct ReplayWindow {
    newest: Option<u64>,
    seen: u128,
}

impl ReplayWindow {
    pub fn accept(&mut self, counter: u64) -> bool {
        let Some(newest) = self.newest else {
            self.newest = Some(counter);
            self.seen = 1;
            return true;
        };
        if counter > newest {
            let delta = counter - newest;
            self.seen = if delta >= 128 {
                1
            } else {
                (self.seen << delta) | 1
            };
            self.newest = Some(counter);
            return true;
        }
        let age = newest - counter;
        if age >= 128 || self.seen & (1 << age) != 0 {
            return false;
        }
        self.seen |= 1 << age;
        true
    }
}

// The binaries share this module but use different entry points.
#[allow(dead_code)]
pub fn status(message: &Message, recipient: Session) -> Option<(u8, Session)> {
    if message.role != 0
        || message.kind != STATUS
        || message.body.len() != 33
        || message.body[..16] != recipient
    {
        return None;
    }
    Some((message.body[16], message.body[17..].try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_wire_format_rejects_malformed_counts_lengths_and_versions() {
        let report = FeedbackReport {
            recipient: [7; 16],
            serial: 10,
            probe: 12,
            frames: 50,
            lost: 2,
        };
        let bytes = report.encode();
        assert_eq!(FeedbackReport::parse(&bytes), Some(report));
        for length in 0..FEEDBACK_BYTES {
            assert!(FeedbackReport::parse(&bytes[..length]).is_none());
        }
        let mut unknown = bytes;
        unknown[0] = 2;
        assert!(FeedbackReport::parse(&unknown).is_none());
        assert!(FeedbackReport::parse(&FeedbackReport { lost: 51, ..report }.encode()).is_none());
        assert_eq!(probe_token(&probe_body(u64::MAX)), Some(u64::MAX));
        assert!(probe_token(&[0; PROBE_BYTES]).is_none());
    }

    #[test]
    fn authenticates_headers_payload_and_key_and_uses_distinct_nonces() {
        let mut sender = Transport::new(&[7; 32], 1).unwrap();
        let receiver = Transport::new(&[7; 32], 2).unwrap();
        let packet = sender.seal(AUDIO, b"voice").unwrap();
        assert_eq!(receiver.open(&packet).unwrap().body, b"voice");
        assert_ne!(packet, sender.seal(AUDIO, b"voice").unwrap());
        for index in [4, 5, 6, 22, 30, packet.len() - 1] {
            let mut damaged = packet.clone();
            damaged[index] ^= 1;
            assert!(receiver.open(&damaged).is_none());
        }
        assert!(Transport::new(&[8; 32], 0).unwrap().open(&packet).is_none());
        for length in 0..46 {
            assert!(receiver.open(&packet[..length]).is_none());
        }
    }

    #[test]
    fn rejects_replays_and_stale_counters_but_accepts_reordering() {
        let mut replay = ReplayWindow::default();
        for counter in [0, 3, 2, 1, 140, 139] {
            assert!(replay.accept(counter));
        }
        for counter in [0, 3, 2, 140, 139] {
            assert!(!replay.accept(counter));
        }
        assert!(replay.accept(u64::MAX));
        assert!(!replay.accept(u64::MAX));
    }

    #[test]
    fn key_validation_is_strict() {
        assert!(parse_key(&"ab".repeat(32)).is_ok());
        for key in ["short", &"gg".repeat(32), &"é".repeat(32)] {
            assert!(parse_key(key).is_err());
        }
    }
}
