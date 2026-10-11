// SPDX-License-Identifier: MIT
use super::transport::{self, Message, ReplayWindow, Session, Transport};
use std::{
    collections::VecDeque,
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

pub struct Connection {
    pub socket: UdpSocket,
    pub transport: Transport,
    pub direct: bool,
    pub role: u8,
    pub registered: bool,
    pub peer: Option<Session>,
    pub changed: bool,
    pub received: usize,
    pub rejected: usize,
    pub sent: usize,
    pub last_contact: Instant,
    replay: ReplayWindow,
    relay_session: Option<Session>,
    relay_replay: ReplayWindow,
    retired: VecDeque<Session>,
    retired_relays: VecDeque<Session>,
}

impl Connection {
    pub fn new(
        key: &[u8; 32],
        role: u8,
        direct: bool,
        local: SocketAddr,
        remote: SocketAddr,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind(local)?;
        socket.connect(remote)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            transport: Transport::new(key, role)?,
            direct,
            role,
            registered: false,
            peer: None,
            changed: false,
            received: 0,
            rejected: 0,
            sent: 0,
            last_contact: Instant::now(),
            replay: ReplayWindow::default(),
            relay_session: None,
            relay_replay: ReplayWindow::default(),
            retired: VecDeque::new(),
            retired_relays: VecDeque::new(),
        })
    }

    pub fn send(&mut self, kind: u8, body: &[u8]) -> io::Result<()> {
        let packet = self.transport.seal(kind, body)?;
        match self.socket.send(&packet) {
            Ok(_) => {
                if kind == transport::AUDIO {
                    self.sent += 1;
                }
                Ok(())
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(io::Error::new(
                error.kind(),
                format!("UDP send: {error}; check peer address and UDP firewall"),
            )),
        }
    }

    fn set_peer(&mut self, peer: Option<Session>) -> bool {
        if peer == self.peer {
            return true;
        }
        if peer.is_some_and(|p| self.retired.contains(&p)) {
            return false;
        }
        if let (Some(old), Some(_)) = (self.peer, peer) {
            self.retired.push_back(old);
            if self.retired.len() > 32 {
                self.retired.pop_front();
            }
        }
        self.peer = peer;
        self.replay = ReplayWindow::default();
        self.changed = true;
        true
    }

    pub fn handle(&mut self, bytes: &[u8], now: Instant) -> io::Result<Option<(u64, Vec<u8>)>> {
        let Some(message) = self.transport.open(bytes) else {
            self.rejected += 1;
            return Ok(None);
        };
        if !self.direct
            && let Some((state, peer)) = transport::status(&message, self.transport.session)
        {
            if Some(message.session) != self.relay_session {
                if self.retired_relays.contains(&message.session) {
                    return Ok(None);
                }
                if let Some(old) = self.relay_session {
                    self.retired_relays.push_back(old);
                    if self.retired_relays.len() > 32 {
                        self.retired_relays.pop_front();
                    }
                }
                self.relay_session = Some(message.session);
                self.relay_replay = ReplayWindow::default();
            }
            if !self.relay_replay.accept(message.counter) {
                self.rejected += 1;
                return Ok(None);
            }
            match state {
                transport::OCCUPIED => {
                    return Err(io::Error::other(
                        "Role occupied; use the other role or wait 10 seconds for the lease to expire",
                    ));
                }
                transport::WAITING => {
                    self.set_peer(None);
                }
                transport::CONNECTED => {
                    self.set_peer(Some(peer));
                }
                _ => return Ok(None),
            }
            self.registered = true;
            self.last_contact = now;
            return Ok(None);
        }
        let Message {
            kind,
            role,
            session,
            counter,
            body,
        } = message;
        if role != 3 - self.role || session == self.transport.session {
            self.rejected += 1;
            return Ok(None);
        }
        if self.direct && kind == transport::HELLO && body.is_empty() {
            let needs_reply = !self.registered || self.peer != Some(session);
            if !self.set_peer(Some(session)) || !self.replay.accept(counter) {
                self.rejected += 1;
                return Ok(None);
            }
            self.registered = true;
            self.last_contact = now;
            if needs_reply {
                self.send(transport::HELLO, &[])?;
            }
        } else if self.peer == Some(session) && self.replay.accept(counter) {
            if self.direct {
                self.last_contact = now;
            }
            if kind == transport::AUDIO && body.len() >= 9 {
                self.received += 1;
                return Ok(Some((
                    u64::from_be_bytes(body[..8].try_into().expect("checked length")),
                    body[8..].to_vec(),
                )));
            }
            if self.direct && kind == transport::BYE && body.is_empty() {
                self.set_peer(None);
                self.registered = false;
            }
        } else {
            self.rejected += 1;
        }
        Ok(None)
    }

    pub fn timed_out(&self, now: Instant) -> bool {
        now.duration_since(self.last_contact) > Duration::from_secs(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_ipv4_and_ipv6_handshake_audio_and_restart_without_a_relay() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            direct_roundtrip(bind);
        }
    }

    fn direct_roundtrip(bind: &str) {
        let key = [11; 32];
        // CI platforms must provide the IPv6 loopback used by direct mode.
        let a = UdpSocket::bind(bind).unwrap();
        let b = UdpSocket::bind(bind).unwrap();
        let addresses = [a.local_addr().unwrap(), b.local_addr().unwrap()];
        drop(a);
        drop(b);
        let mut a = Connection::new(&key, 1, true, addresses[0], addresses[1]).unwrap();
        let mut b = Connection::new(&key, 2, true, addresses[1], addresses[0]).unwrap();
        a.socket.set_nonblocking(false).unwrap();
        b.socket.set_nonblocking(false).unwrap();
        a.socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        b.socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = [0; transport::MAX_DATAGRAM + 1];
        a.send(transport::HELLO, &[]).unwrap();
        b.send(transport::HELLO, &[]).unwrap();
        let len = a.socket.recv(&mut bytes).unwrap();
        a.handle(&bytes[..len], Instant::now()).unwrap();
        let len = b.socket.recv(&mut bytes).unwrap();
        b.handle(&bytes[..len], Instant::now()).unwrap();
        assert!(a.registered && b.registered);
        assert_eq!(a.peer, Some(b.transport.session));
        for (sender, receiver) in [(&mut a, &mut b)] {
            sender.send(transport::AUDIO, &[0; 9]).unwrap();
            let len = loop {
                let len = receiver.socket.recv(&mut bytes).unwrap();
                if let Some((index, _)) = receiver.handle(&bytes[..len], Instant::now()).unwrap() {
                    assert_eq!(index, 0);
                    break len;
                }
            };
            assert!(
                receiver
                    .handle(&bytes[..len], Instant::now())
                    .unwrap()
                    .is_none()
            );
        }
        let old = a.transport.seal(transport::HELLO, &[]).unwrap();
        a.transport = Transport::new(&key, 1).unwrap();
        a.send(transport::HELLO, &[]).unwrap();
        let len = b.socket.recv(&mut bytes).unwrap();
        b.handle(&bytes[..len], Instant::now()).unwrap();
        assert_eq!(b.peer, Some(a.transport.session));
        b.handle(&old, Instant::now()).unwrap();
        assert_eq!(
            b.peer,
            Some(a.transport.session),
            "old session replay reset the call"
        );
    }
}
