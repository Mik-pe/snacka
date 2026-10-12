// SPDX-License-Identifier: MIT
//! Transport-independent bitrate adaptation for interactive audio.
//!
//! Feed interval counts from the remote receiver, after its reordering/playout
//! deadline, and RTT samples measured with a local monotonic clock. Apply
//! [`BitrateController::bitrate`] through [`crate::Encoder::set_bitrate`].
//! This bounded heuristic is experimental, not a complete congestion-control
//! protocol. The application owns authentication, feedback freshness, pacing
//! and stopping traffic when the minimum rate cannot be supported.
//!
//! ```
//! use snacka::{Config, Encoder};
//! use snacka::adaptive::{BitrateController, Feedback};
//! use std::time::Duration;
//!
//! let mut encoder = Encoder::new(Config::new(24_000)?);
//! let mut rate = BitrateController::new(24_000, 12_000, 32_000, 39_984, Duration::ZERO)?;
//! rate.update(Duration::from_secs(1), Feedback {
//!     frames: 50, lost: 3, rtt: Some(Duration::from_millis(30)),
//! });
//! encoder.set_bitrate(rate.bitrate())?;
//! # Ok::<(), snacka::Error>(())
//! ```

use crate::{Config, Error, FrameDuration};
use std::time::Duration;

/// Receiver observations since the previous accepted report.
#[derive(Clone, Copy, Debug)]
pub struct Feedback {
    /// Frames whose playout deadline passed (received plus missing frames).
    pub frames: u32,
    /// Missing, late or undecodable frames among `frames`.
    pub lost: u32,
    /// Fresh RTT measured locally; remote clocks need not be synchronized.
    pub rtt: Option<Duration>,
}

/// Why the controller most recently selected its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateReason {
    /// Waiting for enough receiver observations.
    Startup,
    /// Holding the rate through noise or the recovery hold period.
    Stable,
    /// Slowly probing upward after sustained healthy feedback.
    Recovery,
    /// Receiver loss exceeded the threshold.
    Loss,
    /// Sustained RTT growth suggests a growing queue.
    Delay,
    /// Timely feedback stopped arriving.
    NoFeedback,
}

/// Allocation-free additive-increase/multiplicative-decrease controller.
///
/// Times are durations since one application-owned monotonic epoch. Call
/// `update` with fresh interval reports and `tick` regularly, including during
/// outages. Neither method resets codec state or allocates storage.
pub struct BitrateController {
    bitrate: u32,
    minimum: u32,
    maximum: u32,
    overhead: u32,
    clock: Duration,
    last_feedback: Duration,
    last_change: Duration,
    window_start: Duration,
    frames: u64,
    lost: u64,
    healthy_since: Option<Duration>,
    smoothed_rtt: Option<Duration>,
    base_rtt: Option<Duration>,
    base_epoch: Duration,
    next_base_rtt: Option<Duration>,
    delay_windows: u8,
    reason: RateReason,
}

impl BitrateController {
    /// Construct with codec bounds (12–32 kb/s), fixed non-codec overhead in
    /// bits/s, and the current time. Requires `minimum <= initial <= maximum`.
    /// Overhead includes transport headers and periodic control packets; it
    /// is included when reducing the total sending rate.
    pub fn new(
        initial: u32,
        minimum: u32,
        maximum: u32,
        overhead: u32,
        now: Duration,
    ) -> Result<Self, Error> {
        for rate in [minimum, initial, maximum] {
            Config::new(rate)?;
        }
        if minimum > initial || initial > maximum || overhead > 1_000_000 {
            return Err(Error::Config);
        }
        Ok(Self {
            bitrate: initial,
            minimum,
            maximum,
            overhead,
            clock: now,
            last_feedback: now,
            last_change: now,
            window_start: now,
            frames: 0,
            lost: 0,
            healthy_since: None,
            smoothed_rtt: None,
            base_rtt: None,
            base_epoch: now,
            next_base_rtt: None,
            delay_windows: 0,
            reason: RateReason::Startup,
        })
    }

    /// Current codec-packet target, including Snacka framing and checksum.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    /// Estimated total sending rate including the configured overhead.
    pub fn wire_bitrate(&self) -> u32 {
        self.bitrate + self.overhead
    }

    /// Most recent adaptation decision.
    pub fn reason(&self) -> RateReason {
        self.reason
    }

    /// Update the ceiling without resetting observations or recovery timers.
    pub fn set_maximum(&mut self, maximum: u32) -> Result<(), Error> {
        Config::new(maximum)?;
        if maximum < self.minimum {
            return Err(Error::Config);
        }
        self.maximum = maximum;
        self.bitrate = self.bitrate.min(maximum);
        Ok(())
    }

    /// Accept a fresh receiver interval. Invalid counts, backwards time and
    /// implausible RTT (>10 s or zero) are rejected without changing state.
    /// Empty intervals update RTT but never renew media liveness or probe upward.
    /// Returns whether the report was accepted.
    pub fn update(&mut self, now: Duration, feedback: Feedback) -> bool {
        if now < self.clock
            || feedback.lost > feedback.frames
            || feedback.frames > 10_000
            || feedback
                .rtt
                .is_some_and(|rtt| rtt.is_zero() || rtt > Duration::from_secs(10))
        {
            return false;
        }
        // Apply any elapsed feedback timeout before this report restores liveness.
        self.tick(now);
        if feedback.frames > 0 {
            self.last_feedback = now;
        }
        if let Some(rtt) = feedback.rtt {
            self.next_base_rtt = Some(self.next_base_rtt.map_or(rtt, |base| base.min(rtt)));
            if now - self.base_epoch >= Duration::from_secs(30) {
                self.base_rtt = self.next_base_rtt;
                self.next_base_rtt = Some(rtt);
                self.base_epoch = now;
            } else {
                self.base_rtt = Some(self.base_rtt.map_or(rtt, |base| base.min(rtt)));
            }
            self.smoothed_rtt = Some(self.smoothed_rtt.map_or(rtt, |old| (old * 3 + rtt) / 4));
        }
        self.frames += u64::from(feedback.frames);
        self.lost += u64::from(feedback.lost);
        if now - self.window_start < Duration::from_secs(1) {
            return true;
        }
        let frames = std::mem::take(&mut self.frames);
        let lost = std::mem::take(&mut self.lost);
        self.window_start = now;
        // Avoid treating one reordered/lost packet as a reliable loss estimate.
        if frames < 20 {
            self.healthy_since = None;
            return true;
        }
        let queue = self
            .smoothed_rtt
            .zip(self.base_rtt)
            .map_or(Duration::ZERO, |(rtt, base)| rtt.saturating_sub(base));
        self.delay_windows = if queue >= Duration::from_millis(40) {
            self.delay_windows.saturating_add(1)
        } else {
            0
        };
        if lost * 100 >= frames * 5 || self.delay_windows >= 2 {
            self.healthy_since = None;
            if now - self.last_change >= Duration::from_secs(1) {
                let reason = if lost * 100 >= frames * 5 {
                    RateReason::Loss
                } else {
                    RateReason::Delay
                };
                let percent = if lost * 100 >= frames * 20 { 70 } else { 85 };
                self.reduce(now, percent, reason);
            }
        } else if lost * 100 <= frames * 2 && queue < Duration::from_millis(20) {
            let since = *self.healthy_since.get_or_insert(now);
            if now - since >= Duration::from_secs(3)
                && now - self.last_change >= Duration::from_secs(1)
            {
                self.bitrate = (self.bitrate + 1_000).min(self.maximum);
                self.last_change = now;
                self.reason = RateReason::Recovery;
            } else {
                self.reason = RateReason::Stable;
            }
        } else {
            self.healthy_since = None;
            self.reason = RateReason::Stable;
        }
        true
    }

    /// Reduce toward the floor when no fresh feedback arrives for two seconds.
    /// A stalled caller performs one reduction, never a burst of reductions.
    pub fn tick(&mut self, now: Duration) {
        if now < self.clock {
            return;
        }
        self.clock = now;
        if now - self.last_feedback >= Duration::from_secs(2) {
            self.healthy_since = None;
            self.frames = 0;
            self.lost = 0;
            self.window_start = now;
            if now - self.last_change >= Duration::from_secs(1) {
                self.reduce(now, 85, RateReason::NoFeedback);
            }
        }
    }

    fn reduce(&mut self, now: Duration, percent: u32, reason: RateReason) {
        self.bitrate = ((self.wire_bitrate() * percent / 100).saturating_sub(self.overhead))
            .clamp(self.minimum, self.maximum);
        self.last_change = now;
        self.reason = reason;
    }
}

/// Constant packet overhead in bits/s for a selected frame duration.
/// Includes no link-layer headers; add periodic feedback/keepalive separately.
pub fn packet_overhead_bps(bytes_per_packet: u16, duration: FrameDuration) -> u32 {
    u32::from(bytes_per_packet) * 8 * (1_000 / duration.milliseconds())
}
