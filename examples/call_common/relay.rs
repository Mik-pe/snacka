// SPDX-License-Identifier: MIT
use super::transport::{self, Message, ReplayWindow, Session, Transport};
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

pub const LEASE: Duration = Duration::from_secs(10);

struct Peer {
    address: SocketAddr,
    session: Session,
    last_seen: Instant,
    replay: ReplayWindow,
}

pub struct Relay {
    transport: Transport,
    peers: [Option<Peer>; 2],
}

impl Relay {
    pub fn new(key: &[u8; 32]) -> io::Result<Self> {
        Ok(Self {
            transport: Transport::new(key, 0)?,
            peers: [None, None],
        })
    }

    pub fn expire(&mut self, now: Instant) {
        for peer in &mut self.peers {
            if peer
                .as_ref()
                .is_some_and(|p| now.duration_since(p.last_seen) >= LEASE)
            {
                *peer = None;
            }
        }
    }

    fn send_status(
        &mut self,
        socket: &UdpSocket,
        address: SocketAddr,
        session: Session,
        state: u8,
        other: Session,
    ) -> io::Result<()> {
        let mut body = Vec::from(session);
        body.push(state);
        body.extend_from_slice(&other);
        socket.send_to(&self.transport.seal(transport::STATUS, &body)?, address)?;
        Ok(())
    }

    fn notify(&mut self, socket: &UdpSocket) -> io::Result<()> {
        for index in 0..2 {
            if let Some(peer) = &self.peers[index] {
                let (address, session) = (peer.address, peer.session);
                let other = self.peers[1 - index].as_ref().map(|p| p.session);
                self.send_status(
                    socket,
                    address,
                    session,
                    if other.is_some() {
                        transport::CONNECTED
                    } else {
                        transport::WAITING
                    },
                    other.unwrap_or([0; 16]),
                )?;
            }
        }
        Ok(())
    }

    pub fn receive(
        &mut self,
        socket: &UdpSocket,
        bytes: &[u8],
        address: SocketAddr,
        now: Instant,
    ) -> io::Result<()> {
        self.expire(now);
        let Some(Message {
            kind,
            role,
            session,
            counter,
            body,
        }) = self.transport.open(bytes)
        else {
            return Ok(());
        };
        if !(1..=2).contains(&role) {
            return Ok(());
        }
        let index = usize::from(role - 1);
        match kind {
            transport::HELLO if body.is_empty() => {
                if let Some(peer) = &self.peers[index]
                    && (peer.address != address || peer.session != session)
                {
                    return self.send_status(
                        socket,
                        address,
                        session,
                        transport::OCCUPIED,
                        [0; 16],
                    );
                }
                let peer = self.peers[index].get_or_insert_with(|| Peer {
                    address,
                    session,
                    last_seen: now,
                    replay: ReplayWindow::default(),
                });
                if !peer.replay.accept(counter) {
                    return Ok(());
                }
                peer.last_seen = now;
                self.notify(socket)?;
            }
            transport::AUDIO | transport::BYE => {
                let Some(peer) = &mut self.peers[index] else {
                    return Ok(());
                };
                if peer.address != address
                    || peer.session != session
                    || !peer.replay.accept(counter)
                {
                    return Ok(());
                }
                peer.last_seen = now;
                if kind == transport::BYE && body.is_empty() {
                    self.peers[index] = None;
                    self.notify(socket)?;
                } else if kind == transport::AUDIO
                    && (9..=8 + snacka::MAX_PACKET_BYTES).contains(&body.len())
                    && let Some(other) = &self.peers[1 - index]
                {
                    socket.send_to(bytes, other.address)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::*;

    fn socket() -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        socket
    }
    fn recv(socket: &UdpSocket) -> Vec<u8> {
        let mut bytes = [0; MAX_DATAGRAM + 1];
        let length = socket.recv(&mut bytes).unwrap();
        bytes[..length].to_vec()
    }

    #[test]
    fn two_udp_clients_forward_both_ways_and_slots_are_leased() {
        let server = socket();
        let sockets = [socket(), socket()];
        let key = [9; 32];
        let mut clients = [
            Transport::new(&key, 1).unwrap(),
            Transport::new(&key, 2).unwrap(),
        ];
        let mut relay = Relay::new(&key).unwrap();
        let now = Instant::now();
        let a = sockets[0].local_addr().unwrap();
        let b = sockets[1].local_addr().unwrap();
        relay
            .receive(&server, &clients[0].seal(HELLO, &[]).unwrap(), a, now)
            .unwrap();
        assert_eq!(
            status(
                &clients[0].open(&recv(&sockets[0])).unwrap(),
                clients[0].session
            )
            .unwrap()
            .0,
            WAITING
        );
        relay
            .receive(&server, &clients[1].seal(HELLO, &[]).unwrap(), b, now)
            .unwrap();
        for index in 0..2 {
            assert_eq!(
                status(
                    &clients[index].open(&recv(&sockets[index])).unwrap(),
                    clients[index].session
                )
                .unwrap(),
                (CONNECTED, clients[1 - index].session)
            );
        }
        for index in 0..2 {
            let mut encoder = snacka::Encoder::new(snacka::Config::default());
            let mut codec = [0; snacka::MAX_PACKET_BYTES];
            let length = encoder
                .encode(&[0.2; snacka::FRAME_SAMPLES], &mut codec)
                .unwrap();
            let mut body = Vec::from(0u64.to_be_bytes());
            body.extend_from_slice(&codec[..length]);
            let packet = clients[index].seal(AUDIO, &body).unwrap();
            relay
                .receive(&server, &packet, sockets[index].local_addr().unwrap(), now)
                .unwrap();
            let received = clients[1 - index].open(&recv(&sockets[1 - index])).unwrap();
            let mut decoder = snacka::Decoder::new();
            let mut output = [0.0; snacka::FRAME_SAMPLES];
            assert!(
                !decoder
                    .decode(&received.body[8..], &mut output)
                    .unwrap()
                    .concealed
            );
            relay
                .receive(&server, &packet, sockets[index].local_addr().unwrap(), now)
                .unwrap();
            assert!(
                sockets[1 - index].recv(&mut [0; MAX_DATAGRAM]).is_err(),
                "replay was forwarded"
            );
        }
        let third = socket();
        let mut intruder = Transport::new(&key, 1).unwrap();
        relay
            .receive(
                &server,
                &intruder.seal(HELLO, &[]).unwrap(),
                third.local_addr().unwrap(),
                now,
            )
            .unwrap();
        assert_eq!(
            status(&intruder.open(&recv(&third)).unwrap(), intruder.session)
                .unwrap()
                .0,
            OCCUPIED
        );
        relay
            .receive(&server, &clients[0].seal(BYE, &[]).unwrap(), a, now)
            .unwrap();
        assert_eq!(
            status(
                &clients[1].open(&recv(&sockets[1])).unwrap(),
                clients[1].session
            )
            .unwrap()
            .0,
            WAITING
        );
        relay
            .receive(
                &server,
                &intruder.seal(HELLO, &[]).unwrap(),
                third.local_addr().unwrap(),
                now + LEASE,
            )
            .unwrap();
        assert_eq!(
            status(&intruder.open(&recv(&third)).unwrap(), intruder.session)
                .unwrap()
                .0,
            WAITING
        );
    }
}
