// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]
//! Packet-duration changes preserve codec history and recover only compatible audio.

use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl, Recovery,
};

const RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];

fn config(
    mode: Mode,
    bandwidth: Bandwidth,
    channels: usize,
    duration: FrameDuration,
    policy: RateControl,
) -> EncoderConfig {
    let config = EncoderConfig::new(
        96_000 * channels as u32,
        channels,
        duration,
        bandwidth,
        Application::Voice,
    )
    .unwrap()
    .with_mode(mode)
    .unwrap()
    .with_rate_control(policy);
    if mode == Mode::Celt {
        config
    } else {
        config.with_fec(true).unwrap()
    }
}

fn speech(samples: usize, channels: usize, start: usize) -> Vec<f32> {
    (0..samples * channels)
        .map(|index| {
            let time = (start + index / channels) as f32 / 48_000.0;
            let pitch = 137.0 + 29.0 * (std::f32::consts::TAU * 1.3 * time).sin();
            let phase = std::f32::consts::TAU * pitch * time + (index % channels) as f32 * 0.4;
            0.16 * phase.sin() + 0.055 * (2.0 * phase).sin() + 0.024 * (7.0 * phase).sin()
        })
        .collect()
}

struct Receiver {
    whole: Decoder,
    split: Decoder,
    sample_rate: u32,
    channels: usize,
    whole_pcm: Vec<f32>,
    split_pcm: Vec<f32>,
}

impl Receiver {
    fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            whole: Decoder::with_sample_rate(channels, sample_rate).unwrap(),
            split: Decoder::with_sample_rate(channels, sample_rate).unwrap(),
            sample_rate,
            channels,
            whole_pcm: vec![0.0; 5_760 * channels],
            split_pcm: vec![0.0; 5_760 * channels],
        }
    }

    fn recover(&mut self, packet: &[u8], lost_samples_48khz: usize, fec: bool) {
        let samples = lost_samples_48khz * self.sample_rate as usize / 48_000;
        let expected = if fec {
            Recovery::Fec
        } else {
            Recovery::Concealed
        };
        assert_eq!(
            self.whole
                .recover(packet, samples, &mut self.whole_pcm)
                .unwrap(),
            expected
        );
        if fec {
            // A composed long packet protects its final 20 ms. Recovering the
            // complete loss must equal explicit prefix PLC followed by that tail.
            let frame = Packet::parse(packet)
                .unwrap()
                .frame_samples(self.sample_rate)
                .unwrap();
            let tail = frame.min(samples);
            let prefix = samples - tail;
            if prefix != 0 {
                assert_eq!(
                    self.split
                        .conceal(prefix, &mut self.split_pcm[..prefix * self.channels])
                        .unwrap(),
                    prefix
                );
            }
            assert_eq!(
                self.split
                    .recover(packet, tail, &mut self.split_pcm[prefix * self.channels..],)
                    .unwrap(),
                Recovery::Fec
            );
        } else {
            assert_eq!(
                self.split.conceal(samples, &mut self.split_pcm).unwrap(),
                samples
            );
        }
        assert_eq!(
            self.whole_pcm[..samples * self.channels],
            self.split_pcm[..samples * self.channels]
        );
        assert!(
            self.whole_pcm[..samples * self.channels]
                .iter()
                .all(|sample| sample.is_finite())
        );
    }

    fn decode(&mut self, packet: &[u8], samples_48khz: usize, final_range: u32) {
        let samples = samples_48khz * self.sample_rate as usize / 48_000;
        assert_eq!(
            self.whole.decode(packet, &mut self.whole_pcm).unwrap(),
            samples
        );
        assert_eq!(
            self.split.decode(packet, &mut self.split_pcm).unwrap(),
            samples
        );
        assert_eq!(self.whole.final_range(), final_range);
        assert_eq!(self.split.final_range(), final_range);
        assert_eq!(
            self.whole_pcm[..samples * self.channels],
            self.split_pcm[..samples * self.channels]
        );
        assert!(
            self.whole_pcm[..samples * self.channels]
                .iter()
                .all(|sample| sample.is_finite())
        );
    }
}

#[test]
fn silk_duration_switches_recover_compatible_tails_and_conceal_changed_geometry() {
    let durations = [
        FrameDuration::Ms60,
        FrameDuration::Ms80,
        FrameDuration::Ms120,
        FrameDuration::Ms60,
    ];
    for bandwidth in [
        Bandwidth::Narrowband,
        Bandwidth::Mediumband,
        Bandwidth::Wideband,
    ] {
        for channels in [1, 2] {
            for policy in [RateControl::Constant, RateControl::Variable] {
                let initial = config(Mode::Silk, bandwidth, channels, durations[0], policy);
                let mut encoder = Encoder::new(initial).unwrap();
                let mut receivers: Vec<_> = RATES
                    .into_iter()
                    .map(|rate| Receiver::new(channels, rate))
                    .collect();
                let mut packet = vec![0; 4_096];
                let mut start = 0;
                let mut pending_loss = None;
                let mut recovered = 0;
                let mut incompatible = 0;
                for (stage, duration) in durations.into_iter().enumerate() {
                    encoder
                        .reconfigure(config(Mode::Silk, bandwidth, channels, duration, policy))
                        .unwrap();
                    for index in 0..4 {
                        let samples = duration.samples();
                        let input = speech(samples, channels, start);
                        let length = encoder.encode(&input, &mut packet).unwrap();
                        start += samples;
                        let bytes = &packet[..length];
                        let parsed = Packet::parse(bytes).unwrap();
                        assert_eq!(parsed.samples(48_000).unwrap(), samples);
                        assert_eq!(parsed.toc().mode(), Mode::Silk);
                        assert_eq!(
                            parsed.frame_samples(48_000).unwrap(),
                            if duration == FrameDuration::Ms60 {
                                2_880
                            } else {
                                960
                            }
                        );
                        // 80 -> 120 ms keeps the same 20 ms SILK geometry.
                        // 60 <-> composed packets changes the cached geometry.
                        let expected_fec = index != 0 || stage == 2;
                        assert_eq!(
                            encoder.has_fec(),
                            expected_fec,
                            "{bandwidth:?}/{channels}ch/{policy:?}/{duration:?}/{index}"
                        );
                        if let Some(lost) = pending_loss.take() {
                            for receiver in &mut receivers {
                                receiver.recover(bytes, lost, expected_fec);
                            }
                            recovered += usize::from(expected_fec);
                            incompatible += usize::from(!expected_fec);
                        }
                        // Drop one interval within every stage and the final
                        // interval before every duration change.
                        if index == 1 || (index == 3 && stage + 1 < durations.len()) {
                            pending_loss = Some(samples);
                        } else {
                            for receiver in &mut receivers {
                                receiver.decode(bytes, samples, encoder.final_range());
                            }
                        }
                    }
                }
                assert!(pending_loss.is_none());
                assert_eq!(recovered, 5);
                assert_eq!(incompatible, 2);
            }
        }
    }
}

#[test]
fn queued_celt_lp_switches_at_120_ms_preserve_emitted_geometry_and_following_ranges() {
    for (mode, bandwidth) in [
        (Mode::Silk, Bandwidth::Narrowband),
        (Mode::Silk, Bandwidth::Mediumband),
        (Mode::Silk, Bandwidth::Wideband),
        (Mode::Hybrid, Bandwidth::Superwideband),
        (Mode::Hybrid, Bandwidth::Fullband),
    ] {
        for channels in [1, 2] {
            for policy in [RateControl::Constant, RateControl::Variable] {
                let celt = config(
                    Mode::Celt,
                    Bandwidth::Fullband,
                    channels,
                    FrameDuration::Ms120,
                    policy,
                );
                let lp = config(mode, bandwidth, channels, FrameDuration::Ms120, policy);
                let mut encoder = Encoder::new(celt).unwrap();
                let mut receivers: Vec<_> = RATES
                    .into_iter()
                    .map(|rate| Receiver::new(channels, rate))
                    .collect();
                let mut packet = vec![0; celt.max_packet_bytes()];
                let samples = FrameDuration::Ms120.samples();
                for index in 0..9 {
                    let emitted = encoder.config();
                    let queued = match index {
                        1 | 6 => Some(lp),
                        4 => Some(celt),
                        _ => None,
                    };
                    if let Some(config) = queued {
                        encoder.prepare_reconfigure(config).unwrap();
                        assert_eq!(encoder.config(), emitted);
                    }
                    let input = speech(samples, channels, index * samples);
                    let length = encoder.encode(&input, &mut packet).unwrap();
                    let parsed = Packet::parse(&packet[..length]).unwrap();
                    assert_eq!(parsed.samples(48_000).unwrap(), samples);
                    assert_eq!(parsed.frame_count(), 6);
                    assert_eq!(parsed.frame_samples(48_000).unwrap(), 960);
                    assert_eq!(parsed.toc().mode(), emitted.mode());
                    assert_eq!(parsed.toc().bandwidth(), emitted.bandwidth());
                    assert_eq!(encoder.config(), queued.unwrap_or(emitted));
                    assert_eq!(
                        encoder.has_transition_redundancy(),
                        matches!(index, 2 | 4 | 7),
                        "{mode:?}/{bandwidth:?}/{channels}ch/{policy:?}/{index}"
                    );
                    for receiver in &mut receivers {
                        receiver.decode(&packet[..length], samples, encoder.final_range());
                    }
                }
                assert_eq!(encoder.config(), lp);
            }
        }
    }
}
