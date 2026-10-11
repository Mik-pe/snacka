// SPDX-License-Identifier: MIT
use snacka::{Decoder, FRAME_SAMPLES, FrameDuration, Packet};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const FRAME: Duration = Duration::from_millis(20);
const MAX_AHEAD: u64 = 32;

pub struct Jitter {
    packets: BTreeMap<u64, Vec<u8>>,
    decoder: Decoder,
    next: Option<u64>,
    deadline: Option<Instant>,
    last_received: Option<Instant>,
    started: bool,
    buffer: Duration,
    pub decoded: usize,
    pub concealed: usize,
    pub dropped: usize,
}

impl Jitter {
    pub fn new(buffer_ms: u64) -> Self {
        Self {
            packets: BTreeMap::new(),
            decoder: Decoder::new(),
            next: None,
            deadline: None,
            last_received: None,
            started: false,
            buffer: Duration::from_millis(buffer_ms),
            decoded: 0,
            concealed: 0,
            dropped: 0,
        }
    }

    pub fn reset(&mut self) {
        self.packets.clear();
        self.decoder.reset();
        self.next = None;
        self.deadline = None;
        self.last_received = None;
        self.started = false;
    }

    fn align(&mut self, index: u64) {
        self.decoder.reset();
        // Codec sequence is modulo 256. Joining mid-stream needs at most 255
        // concealed frames to align; never iterate an untrusted u64 counter.
        for _ in 0..(index % 256) {
            self.decoder
                .conceal(&mut [0.0; FRAME_SAMPLES])
                .expect("fixed output length");
        }
    }

    pub fn insert(&mut self, index: u64, bytes: &[u8], now: Instant) -> bool {
        let Ok(packet) = Packet::parse(bytes) else {
            self.dropped += 1;
            return false;
        };
        if packet.frame_duration() != FrameDuration::Ms20
            || packet.is_final()
            || packet.sequence() != index as u8
            || index == u64::MAX
        {
            self.dropped += 1;
            return false;
        }
        if let Some(next) = self.next {
            if index < next {
                if self.started || next - index >= MAX_AHEAD {
                    self.dropped += 1;
                    return false;
                }
                self.align(index);
                self.next = Some(index);
                self.packets.retain(|&queued, _| queued - index < MAX_AHEAD);
            }
            if index.saturating_sub(next) >= MAX_AHEAD {
                // Recover after a long outage or a stalled consumer without
                // allowing the receive queue to grow without bound.
                self.reset();
            }
        }
        if self.next.is_none() {
            self.align(index);
            self.next = Some(index);
            self.deadline = Some(now + self.buffer);
        }
        if self.packets.contains_key(&index) {
            self.dropped += 1;
            return false;
        }
        self.packets.insert(index, bytes.to_vec());
        self.last_received = Some(now);
        true
    }

    pub fn poll(&mut self, now: Instant, output: &mut [f32; FRAME_SAMPLES]) -> bool {
        if self
            .last_received
            .is_some_and(|last| now.duration_since(last) > Duration::from_secs(2))
        {
            self.reset();
        }
        let (Some(deadline), Some(index)) = (self.deadline, self.next) else {
            return false;
        };
        if now < deadline {
            return false;
        }
        self.started = true;
        let decoded = self
            .packets
            .remove(&index)
            .is_some_and(|bytes| self.decoder.decode(&bytes, output).is_ok());
        if decoded {
            self.decoded += 1;
        } else {
            self.decoder.conceal(output).expect("fixed output length");
            self.concealed += 1;
        }
        self.next = index.checked_add(1);
        // A stalled process must not burst seconds of stale audio into playback.
        self.deadline = Some(if now.duration_since(deadline) > FRAME * 5 {
            now + FRAME
        } else {
            deadline + FRAME
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snacka::{Config, Encoder, MAX_PACKET_BYTES};

    fn packets(count: usize) -> Vec<Vec<u8>> {
        let mut encoder = Encoder::new(Config::default());
        (0..count)
            .map(|_| {
                let mut packet = [0; MAX_PACKET_BYTES];
                let len = encoder.encode(&[0.1; FRAME_SAMPLES], &mut packet).unwrap();
                packet[..len].to_vec()
            })
            .collect()
    }

    #[test]
    fn reorders_conceals_and_rejects_late_audio() {
        let packets = packets(5);
        let now = Instant::now();
        let mut jitter = Jitter::new(60);
        let mut output = [0.0; FRAME_SAMPLES];
        for index in [0, 3, 1, 4] {
            assert!(jitter.insert(index, &packets[index as usize], now));
        }
        assert!(!jitter.poll(now + Duration::from_millis(59), &mut output));
        for index in 0..5 {
            assert!(jitter.poll(now + Duration::from_millis(60 + index * 20), &mut output));
        }
        assert_eq!((jitter.decoded, jitter.concealed), (4, 1));
        assert!(!jitter.insert(2, &packets[2], now));
    }

    #[test]
    fn initial_prebuffer_can_reorder_the_first_packets_too() {
        let packets = packets(4);
        let now = Instant::now();
        let mut jitter = Jitter::new(60);
        let mut output = [0.0; FRAME_SAMPLES];
        for index in [2, 3, 0, 1] {
            assert!(jitter.insert(index, &packets[index as usize], now));
        }
        for index in 0..4 {
            assert!(jitter.poll(now + Duration::from_millis(60 + index * 20), &mut output));
        }
        assert_eq!(
            (jitter.decoded, jitter.concealed, jitter.dropped),
            (4, 0, 0)
        );
    }

    #[test]
    fn late_join_wrap_restart_and_bounded_outage_recovery() {
        let packets = packets(300);
        let now = Instant::now();
        let mut jitter = Jitter::new(40);
        let mut output = [0.0; FRAME_SAMPLES];
        for index in 254..258 {
            assert!(jitter.insert(index, &packets[index as usize], now));
            assert!(jitter.poll(
                now + Duration::from_millis(40 + (index - 254) * 20),
                &mut output
            ));
        }
        assert_eq!(jitter.decoded, 4);
        assert!(jitter.insert(299, &packets[299], now));
        assert!(jitter.poll(now + Duration::from_millis(40), &mut output));
        assert_eq!(jitter.decoded, 5);
        jitter.reset();
        assert!(jitter.insert(0, &packets[0], now));
        assert!(jitter.poll(now + Duration::from_millis(40), &mut output));
        assert_eq!(jitter.decoded, 6);
        assert!(!jitter.insert(u64::MAX, &packets[255], now));
        assert!(jitter.packets.len() <= MAX_AHEAD as usize);
    }
}
