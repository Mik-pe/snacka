// SPDX-License-Identifier: MIT
use snacka::{
    Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES, Packet,
    adaptive::{BitrateController, Feedback, RateReason, packet_overhead_bps},
};
use std::time::Duration;

fn controller(initial: u32, overhead: u32) -> BitrateController {
    BitrateController::new(initial, 12_000, 32_000, overhead, Duration::ZERO).unwrap()
}
fn report(rate: &mut BitrateController, second: u64, lost: u32, rtt_ms: u64) {
    assert!(rate.update(
        Duration::from_secs(second),
        Feedback {
            frames: 50,
            lost,
            rtt: Some(Duration::from_millis(rtt_ms)),
        }
    ));
}

#[test]
fn fast_backoff_reduces_wire_rate_and_preserves_bounds() {
    let mut rate = controller(32_000, 20_000);
    report(&mut rate, 1, 3, 30);
    assert_eq!(rate.bitrate(), 24_200);
    assert_eq!(rate.reason(), RateReason::Loss);
    report(&mut rate, 2, 10, 30);
    assert_eq!(rate.bitrate(), 12_000);
    for second in 3..100 {
        report(&mut rate, second, 50, 30);
    }
    assert_eq!(rate.bitrate(), 12_000);
}
#[test]
fn stable_link_recovers_slowly_and_obeys_a_live_ceiling() {
    let mut rate = controller(24_000, 32_800);
    for second in 1..=3 {
        report(&mut rate, second, 0, 30);
    }
    assert_eq!(rate.bitrate(), 24_000);
    report(&mut rate, 4, 0, 30);
    assert_eq!(rate.bitrate(), 25_000);
    for second in 5..=15 {
        report(&mut rate, second, 0, 30);
    }
    assert_eq!(rate.bitrate(), 32_000);
    rate.set_maximum(16_000).unwrap();
    for second in 16..=20 {
        report(&mut rate, second, 0, 30);
    }
    assert_eq!(rate.bitrate(), 16_000);
    assert!(rate.set_maximum(11_999).is_err());
}
#[test]
fn delay_growth_backs_off_without_loss_but_one_spike_does_not() {
    let mut rate = controller(24_000, 0);
    report(&mut rate, 1, 0, 200);
    report(&mut rate, 2, 0, 400);
    assert_eq!(rate.bitrate(), 24_000);
    report(&mut rate, 3, 0, 400);
    assert!(rate.bitrate() < 24_000);
    assert_eq!(rate.reason(), RateReason::Delay);
    let mut spike = controller(24_000, 0);
    report(&mut spike, 1, 0, 20);
    report(&mut spike, 2, 0, 80);
    report(&mut spike, 3, 0, 20);
    assert_eq!(spike.bitrate(), 24_000);
}
#[test]
fn no_feedback_empty_reports_and_worker_stalls_cannot_probe_up() {
    let mut rate = controller(32_000, 32_800);
    rate.tick(Duration::from_millis(1_999));
    assert_eq!(rate.bitrate(), 32_000);
    rate.tick(Duration::from_secs(2));
    let reduced = rate.bitrate();
    assert!(reduced < 32_000);
    for _ in 0..10 {
        rate.tick(Duration::from_secs(2));
    }
    assert_eq!(rate.bitrate(), reduced);
    for second in 3..=10 {
        rate.update(
            Duration::from_secs(second),
            Feedback {
                frames: 0,
                lost: 0,
                rtt: Some(Duration::from_millis(20)),
            },
        );
    }
    assert_eq!(rate.bitrate(), 12_000);
    assert_eq!(rate.reason(), RateReason::NoFeedback);
    let mut stalled = controller(32_000, 0);
    stalled.tick(Duration::from_secs(60));
    assert_eq!(stalled.bitrate(), 27_200);
    report(&mut stalled, 61, 0, 20);
    let resumed = stalled.bitrate();
    for second in 62..=63 {
        report(&mut stalled, second, 0, 20);
    }
    assert_eq!(stalled.bitrate(), resumed);
    report(&mut stalled, 64, 0, 20);
    assert_eq!(stalled.bitrate(), resumed);
    report(&mut stalled, 65, 0, 20);
    assert_eq!(stalled.bitrate(), resumed + 1_000);
}
#[test]
fn invalid_feedback_and_backwards_clock_do_not_change_state() {
    let mut rate = controller(24_000, 0);
    report(&mut rate, 1, 0, 30);
    for (now, frames, lost, rtt) in [
        (Duration::ZERO, 50, 0, None),
        (Duration::from_secs(10), 5, 6, None),
        (Duration::from_secs(10), 10_001, 0, None),
        (Duration::from_secs(10), 50, 0, Some(Duration::ZERO)),
        (
            Duration::from_secs(10),
            50,
            0,
            Some(Duration::from_secs(11)),
        ),
    ] {
        assert!(!rate.update(now, Feedback { frames, lost, rtt }));
        assert_eq!(rate.bitrate(), 24_000);
    }
    rate.tick(Duration::ZERO);
    assert_eq!(rate.bitrate(), 24_000);
    for (initial, min, max, overhead) in [
        (24_000, 25_000, 32_000, 0),
        (24_000, 12_000, 20_000, 0),
        (11_999, 12_000, 32_000, 0),
        (24_000, 12_000, 32_000, u32::MAX),
    ] {
        assert!(BitrateController::new(initial, min, max, overhead, Duration::ZERO).is_err());
    }
}
#[test]
fn feedback_frequency_does_not_multiply_reductions_and_small_windows_do_not_probe() {
    let mut rate = controller(32_000, 0);
    for tick in 1..=4 {
        rate.update(
            Duration::from_millis(tick * 250),
            Feedback {
                frames: 12,
                lost: 3,
                rtt: None,
            },
        );
    }
    assert_eq!(rate.bitrate(), 22_400);
    let mut small = controller(24_000, 0);
    for second in 1..=20 {
        small.update(
            Duration::from_secs(second),
            Feedback {
                frames: 1,
                lost: 0,
                rtt: None,
            },
        );
    }
    assert_eq!(small.bitrate(), 24_000);
}
#[test]
fn bottleneck_step_down_and_recovery_keep_queue_and_rate_bounded() {
    let mut rate = controller(24_000, 39_984);
    let mut queued_bits = 0.0_f64;
    let mut constrained_peak_ms = 0.0_f64;
    let mut minimum_rate = 32_000;
    for step in 1..=240 {
        let capacity = if (40..160).contains(&step) {
            56_000.0
        } else {
            100_000.0
        };
        queued_bits = (queued_bits + (rate.wire_bitrate() as f64 - capacity) * 0.25).max(0.0);
        let queue_ms = queued_bits / capacity * 1_000.0;
        if (40..160).contains(&step) {
            constrained_peak_ms = constrained_peak_ms.max(queue_ms);
            minimum_rate = minimum_rate.min(rate.bitrate());
        }
        let lost = if queue_ms > 120.0 { 3 } else { 0 };
        queued_bits = queued_bits.min(capacity * 0.12);
        rate.update(
            Duration::from_millis(step * 250),
            Feedback {
                frames: 12,
                lost,
                rtt: Some(Duration::from_micros(
                    (30_000.0 + queue_ms * 1_000.0) as u64,
                )),
            },
        );
        assert!((12_000..=32_000).contains(&rate.bitrate()));
    }
    assert!(minimum_rate <= 16_000, "did not adapt: {minimum_rate}");
    assert!(constrained_peak_ms < 250.0, "queue: {constrained_peak_ms}");
    assert!(
        rate.bitrate() >= 24_000,
        "did not recover: {}",
        rate.bitrate()
    );
}
#[test]
fn adaptive_targets_roundtrip_with_exact_fractional_budgets_at_both_frame_durations() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let mut rate = controller(24_345, packet_overhead_bps(82, duration));
        let mut encoder = Encoder::new(
            Config::new(rate.bitrate())
                .unwrap()
                .with_frame_duration(duration),
        );
        let mut decoder = Decoder::with_frame_duration(duration);
        let samples = duration.samples();
        let input: Vec<_> = (0..samples)
            .map(|i| (i as f32 * 0.13).sin() * 0.2)
            .collect();
        let mut output = vec![0.0; samples];
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut budget = 0_u64;
        let mut bytes = 0_u64;
        let divisor = 8_000 / u64::from(duration.milliseconds());
        for frame in 0..300_u64 {
            if frame % (1_000 / u64::from(duration.milliseconds())) == 0 {
                report(
                    &mut rate,
                    frame * u64::from(duration.milliseconds()) / 1_000,
                    if frame < 100 { 5 } else { 0 },
                    30,
                );
            }
            encoder.set_bitrate(rate.bitrate()).unwrap();
            budget += u64::from(rate.bitrate());
            let length = encoder.encode(&input, &mut packet).unwrap();
            bytes += length as u64;
            assert_eq!(bytes, budget / divisor);
            assert_eq!(
                Packet::parse(&packet[..length]).unwrap().sequence(),
                frame as u8
            );
            decoder.decode(&packet[..length], &mut output).unwrap();
            assert!(
                output
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
            );
        }
    }
}
