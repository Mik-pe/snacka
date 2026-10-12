// SPDX-License-Identifier: MIT
//! Authentication belongs to Connection; freshness and RTT correlation live here.
use super::transport::{self, FeedbackReport, Session};
use snacka::{
    FrameDuration,
    adaptive::{BitrateController, Feedback, packet_overhead_bps},
};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(250);

pub struct RateControl {
    pub automatic: bool,
    pub maximum: u32,
    pub rtt: Option<Duration>,
    pub loss_percent: f64,
    controller: BitrateController,
    epoch: Instant,
    overhead: u32,
    next_probe: Instant,
    token: u64,
    probes: [Option<(u64, Instant)>; 16],
    last_report: Option<FeedbackReport>,
    last_reply: Option<Instant>,
}

impl RateControl {
    pub fn new(initial: u32, maximum: u32, automatic: bool, ipv6: bool, now: Instant) -> Self {
        let ip_udp = if ipv6 { 48 } else { 28 };
        let header = ip_udp + u32::from(transport::DATAGRAM_OVERHEAD);
        let overhead = packet_overhead_bps((header + 8) as u16, FrameDuration::Ms20)
            + 4 * 8 * (2 * header + (transport::PROBE_BYTES + transport::FEEDBACK_BYTES) as u32)
            + 8 * header; // One HELLO per second; relay STATUS is inbound only.
        Self {
            automatic,
            maximum,
            rtt: None,
            loss_percent: 0.0,
            controller: BitrateController::new(
                if automatic {
                    initial.min(maximum)
                } else {
                    initial
                },
                12_000,
                if automatic {
                    maximum
                } else {
                    maximum.max(initial)
                },
                overhead,
                Duration::ZERO,
            )
            .expect("validated codec bounds"),
            epoch: now,
            overhead,
            next_probe: now,
            token: 0,
            probes: [None; 16],
            last_report: None,
            last_reply: None,
        }
    }

    pub fn bitrate(&self) -> u32 {
        self.controller.bitrate()
    }

    pub fn reset_link(&mut self, ipv6: bool, now: Instant) {
        let mut fresh = Self::new(self.bitrate(), self.maximum, self.automatic, ipv6, now);
        // A receiver can reconnect while our transport session remains alive.
        // Never reuse probe IDs: a delayed old report must not match a new probe.
        fresh.token = self.token;
        *self = fresh;
    }
    pub fn wire_bitrate(&self) -> u32 {
        self.controller.wire_bitrate()
    }
    pub fn reason(&self) -> snacka::adaptive::RateReason {
        self.controller.reason()
    }

    pub fn set_maximum(&mut self, maximum: u32) {
        self.maximum = maximum;
        self.controller
            .set_maximum(maximum)
            .expect("clamped codec bounds");
    }

    pub fn set_fixed(&mut self, bitrate: u32, now: Instant) {
        self.automatic = false;
        self.controller = BitrateController::new(
            bitrate,
            12_000,
            self.maximum.max(bitrate),
            self.overhead,
            now.duration_since(self.epoch),
        )
        .expect("clamped codec bounds");
    }

    pub fn enable(&mut self, now: Instant) {
        self.automatic = true;
        self.controller = BitrateController::new(
            self.bitrate().min(self.maximum),
            12_000,
            self.maximum,
            self.overhead,
            now.duration_since(self.epoch),
        )
        .expect("validated bounds");
        self.last_report = None;
    }

    pub fn probe(&mut self, now: Instant) -> Option<[u8; transport::PROBE_BYTES]> {
        if now < self.next_probe {
            return None;
        }
        self.next_probe = now + INTERVAL;
        self.token = self.token.checked_add(1)?;
        self.probes[self.token as usize % self.probes.len()] = Some((self.token, now));
        Some(transport::probe_body(self.token))
    }

    pub fn reply(
        &mut self,
        recipient: Session,
        token: u64,
        frames: u64,
        lost: u64,
        now: Instant,
    ) -> Option<[u8; transport::FEEDBACK_BYTES]> {
        // Bound replies even if an authenticated peer floods probes.
        if self
            .last_reply
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(200))
        {
            return None;
        }
        self.last_reply = Some(now);
        Some(
            FeedbackReport {
                recipient,
                serial: token,
                probe: token,
                frames,
                lost,
            }
            .encode(),
        )
    }

    pub fn receive(&mut self, report: FeedbackReport, now: Instant) -> bool {
        let slot = &mut self.probes[report.probe as usize % 16];
        let Some((token, sent)) = *slot else {
            return false;
        };
        if token != report.probe || now < sent || now.duration_since(sent) >= Duration::from_secs(2)
        {
            return false;
        }
        if let Some(old) = self.last_report
            && (report.serial <= old.serial || report.frames < old.frames || report.lost < old.lost)
        {
            return false;
        }
        let previous = self.last_report.unwrap_or(report);
        let frames = report.frames - previous.frames;
        let lost = report.lost - previous.lost;
        if lost > frames || frames > 10_000 {
            return false;
        }
        *slot = None; // One RTT observation per probe.
        self.last_report = Some(report);
        let rtt = now.duration_since(sent).max(Duration::from_micros(1));
        self.rtt = Some(rtt);
        if frames > 0 {
            self.loss_percent = lost as f64 * 100.0 / frames as f64;
        }
        if self.automatic {
            self.controller.update(
                now.duration_since(self.epoch),
                Feedback {
                    frames: frames as u32,
                    lost: lost as u32,
                    rtt: Some(rtt),
                },
            );
        }
        true
    }

    pub fn tick(&mut self, now: Instant) {
        if self.automatic {
            self.controller.tick(now.duration_since(self.epoch));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correlates_probes_and_rejects_stale_regressing_duplicate_or_unmeasured_feedback() {
        let now = Instant::now();
        let mut rate = RateControl::new(24_000, 32_000, true, false, now);
        let mut report = FeedbackReport {
            recipient: [1; 16],
            serial: 1,
            probe: 1,
            frames: 100,
            lost: 0,
        };
        assert!(!rate.receive(report, now));
        rate.probe(now).unwrap();
        assert!(rate.receive(report, now + Duration::from_millis(30)));
        assert!(!rate.receive(report, now + Duration::from_millis(40)));
        rate.probe(now + INTERVAL).unwrap();
        report.serial = 2;
        report.probe = 2;
        report.frames = 99;
        assert!(!rate.receive(report, now + INTERVAL * 2));
        report.frames = 110;
        report.lost = 11;
        assert!(!rate.receive(report, now + INTERVAL * 2));
        report.lost = 1;
        assert!(rate.receive(report, now + INTERVAL * 2));
        rate.probe(now + INTERVAL * 3).unwrap();
        report.serial = 3;
        report.probe = 3;
        assert!(!rate.receive(report, now + Duration::from_secs(3)));
    }

    #[test]
    fn replies_are_bounded_and_fixed_mode_ignores_feedback_timeouts() {
        let now = Instant::now();
        let mut rate = RateControl::new(24_000, 32_000, false, true, now);
        assert!(rate.reply([1; 16], 1, 1, 0, now).is_some());
        assert!(rate.reply([1; 16], 2, 1, 0, now).is_none());
        for seconds in 0..10 {
            rate.tick(now + Duration::from_secs(seconds));
        }
        assert_eq!(rate.bitrate(), 24_000);
        rate.enable(now + Duration::from_secs(10));
        rate.tick(now + Duration::from_secs(12));
        assert!(rate.bitrate() < 24_000);
    }

    #[test]
    fn resetting_peer_measurements_cannot_reuse_a_delayed_probe_and_preserves_fixed_rate() {
        let now = Instant::now();
        let mut rate = RateControl::new(24_000, 16_000, false, false, now);
        assert_eq!(transport::probe_token(&rate.probe(now).unwrap()), Some(1));
        rate.reset_link(true, now + INTERVAL);
        assert_eq!(rate.bitrate(), 24_000);
        assert_eq!(
            transport::probe_token(&rate.probe(now + INTERVAL).unwrap()),
            Some(2)
        );
        let report = FeedbackReport {
            recipient: [1; 16],
            serial: 1,
            probe: 1,
            frames: 50,
            lost: 0,
        };
        assert!(!rate.receive(report, now + INTERVAL * 2));
        rate.enable(now + INTERVAL * 2);
        assert_eq!(rate.bitrate(), 16_000);
    }
}
