// SPDX-License-Identifier: MIT
//! Fixed native-PCM delay for the ordinary encoder's direct SILK input path.
//!
//! At matching 8/12/16 kHz SILK rates, the owned codec contributes one native
//! coding sample and its native-rate decoder contributes 4/9/12 samples. An
//! exact integer delay of 31/44/59 input samples therefore gives the declared
//! 36/54/72-sample native-PCM delay. The shift preserves every PCM bit and adds
//! no frequency-dependent phase, ringing, overshoot, clipping, or allocation.
//!
//! Decoder output-rate filtering is separate from this native timing contract.
//! With the existing owned decoder's 48 kHz interpolation model, the resulting
//! low-frequency delays differ from 216 output samples by +1.824/-2.784/-2.112
//! samples for 8/12/16 kHz input. These are decoder filter phase differences;
//! this input delay does not fit a fractional phase correction to that model.
//! At 24/48 kHz this state is disabled and copies the input exactly.
//!
//! The parent validates standard sample rates, mono/stereo geometry, normalized
//! finite input and output capacity before processing. It advances this state
//! for every native-rate input block, including blocks encoded in other modes,
//! so a later direct SILK packet receives the same continuous delayed source.

const HISTORY: usize = 64;
const HISTORY_MASK: usize = HISTORY - 1;

/// Fixed-memory delay before encoding matching native-rate SILK PCM.
pub(super) struct NativeInputDelay {
    sample_rate: u32,
    delay: usize,
    history: [[f32; HISTORY]; 2],
    position: usize,
}

impl NativeInputDelay {
    pub(super) fn new(sample_rate: u32) -> Self {
        debug_assert!([8_000, 12_000, 16_000, 24_000, 48_000].contains(&sample_rate));
        Self {
            sample_rate,
            delay: Self::native_delay(sample_rate),
            history: [[0.0; HISTORY]; 2],
            position: 0,
        }
    }

    const fn native_delay(sample_rate: u32) -> usize {
        match sample_rate {
            8_000 => 31,
            12_000 => 44,
            16_000 => 59,
            _ => 0,
        }
    }

    pub(super) const fn enabled(&self) -> bool {
        self.delay != 0
    }

    /// Restart the native history while retaining the configured rate.
    pub(super) fn reset(&mut self) {
        self.history.fill([0.0; HISTORY]);
        self.position = 0;
    }

    /// Reset only when the validated input sample rate changes.
    pub(super) fn set_sample_rate(&mut self, sample_rate: u32) {
        debug_assert!([8_000, 12_000, 16_000, 24_000, 48_000].contains(&sample_rate));
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.delay = Self::native_delay(sample_rate);
            self.reset();
        }
    }

    /// Shift validated interleaved PCM by an exact integer sample count.
    pub(super) fn process(&mut self, input: &[f32], channels: usize, output: &mut [f32]) {
        debug_assert!((1..=2).contains(&channels));
        debug_assert!(input.len().is_multiple_of(channels));
        debug_assert_eq!(output.len(), input.len());
        if !self.enabled() {
            output.copy_from_slice(input);
            return;
        }
        for (input_frame, output_frame) in input
            .chunks_exact(channels)
            .zip(output.chunks_exact_mut(channels))
        {
            let source = self.position.wrapping_sub(self.delay) & HISTORY_MASK;
            for (channel, (&sample, output_sample)) in
                input_frame.iter().zip(output_frame.iter_mut()).enumerate()
            {
                self.history[channel][self.position] = sample;
                *output_sample = self.history[channel][source];
            }
            self.position = (self.position + 1) & HISTORY_MASK;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];

    fn signal(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|index| {
                let frame = index / channels;
                let channel = index % channels;
                (0.7 * (frame as f64 * (0.137 + channel as f64 * 0.21)).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn every_rate_and_channel_matches_literal_integer_shift() {
        for (rate, delay) in [
            (8_000, 31),
            (12_000, 44),
            (16_000, 59),
            (24_000, 0),
            (48_000, 0),
        ] {
            for channels in 1..=2 {
                let input = signal(519, channels);
                let mut actual = vec![0.0; input.len()];
                NativeInputDelay::new(rate).process(&input, channels, &mut actual);
                let mut expected = vec![0.0; input.len()];
                expected[delay * channels..]
                    .copy_from_slice(&input[..input.len() - delay * channels]);
                assert_eq!(actual, expected, "{rate} Hz, {channels} channels");
            }
        }
    }

    #[test]
    fn impulse_delay_plus_native_codec_delay_matches_declared_timing() {
        for (rate, input_delay, decoder_delay, declared) in [
            (8_000, 31, 4, 36),
            (12_000, 44, 9, 54),
            (16_000, 59, 12, 72),
        ] {
            for channels in 1..=2 {
                let mut input = vec![0.0; 128 * channels];
                input[0] = 1.0;
                let mut output = vec![0.0; input.len()];
                NativeInputDelay::new(rate).process(&input, channels, &mut output);
                assert_eq!(output[input_delay * channels], 1.0);
                for (index, &sample) in output.iter().enumerate() {
                    if index != input_delay * channels {
                        assert_eq!(sample, 0.0, "{rate} Hz, sample {index}");
                    }
                }
                // The one-sample coding and native decoder contributions are
                // independently exercised by the encoder/decoder integration
                // tests; this unit verifies the exact additional PCM shift.
                assert_eq!(input_delay + 1 + decoder_delay, declared);
                assert_eq!(declared, 216 * rate as usize / 48_000);
            }
        }
    }

    #[test]
    fn sinusoidal_phase_and_gain_match_exact_delay_through_near_nyquist() {
        const FRAMES: usize = 2048;
        const WARMUP: usize = 128;
        const AMPLITUDE: f64 = 0.25;
        for (rate, delay) in [(8_000, 31), (12_000, 44), (16_000, 59)] {
            for channels in 1..=2 {
                for cycles in [1, 16, 128, 1000] {
                    let omega = std::f64::consts::TAU * cycles as f64 / FRAMES as f64;
                    let input: Vec<_> = (0..(FRAMES + WARMUP) * channels)
                        .map(|index| {
                            let frame = index / channels;
                            let channel = index % channels;
                            let channel_phase = channel as f64 * std::f64::consts::PI / 7.0;
                            (AMPLITUDE * (omega * frame as f64 + channel_phase).sin()) as f32
                        })
                        .collect();
                    let mut output = vec![0.0; input.len()];
                    NativeInputDelay::new(rate).process(&input, channels, &mut output);
                    let settled = &output[WARMUP * channels..];
                    for channel in 0..channels {
                        let (mut cosine, mut sine) = (0.0, 0.0);
                        for (frame, samples) in settled.chunks_exact(channels).enumerate() {
                            let phase = omega * (frame + WARMUP) as f64;
                            cosine += f64::from(samples[channel]) * phase.cos();
                            sine += f64::from(samples[channel]) * phase.sin();
                        }
                        cosine *= 2.0 / FRAMES as f64;
                        sine *= 2.0 / FRAMES as f64;
                        let gain = cosine.hypot(sine) / AMPLITUDE;
                        let actual_phase = cosine.atan2(sine);
                        let expected_phase =
                            -omega * delay as f64 + channel as f64 * std::f64::consts::PI / 7.0;
                        let difference = actual_phase - expected_phase;
                        let phase_error = difference.sin().atan2(difference.cos()).abs();
                        assert!(
                            (gain - 1.0).abs() < 1e-6,
                            "{rate} Hz, tone {cycles}/{FRAMES}, channel {channel}: gain {gain}"
                        );
                        assert!(
                            phase_error < 1e-6,
                            "{rate} Hz, tone {cycles}/{FRAMES}, channel {channel}: phase error {phase_error} radians"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn chunking_empty_calls_and_inactive_mode_priming_preserve_history() {
        for rate in RATES {
            for channels in 1..=2 {
                let input = signal(519, channels);
                let mut expected = vec![0.0; input.len()];
                let mut actual = vec![0.0; input.len()];
                NativeInputDelay::new(rate).process(&input, channels, &mut expected);
                let mut delay = NativeInputDelay::new(rate);
                let mut start = 0;
                for frames in [1, 63, 128, 17, 310] {
                    let end = start + frames * channels;
                    delay.process(&[], channels, &mut []);
                    delay.process(&input[start..end], channels, &mut actual[start..end]);
                    start = end;
                }
                assert_eq!(start, input.len());
                assert_eq!(actual, expected);

                delay.reset();
                let split = 317 * channels;
                let mut discarded = vec![0.0; split];
                delay.process(&input[..split], channels, &mut discarded);
                delay.process(&input[split..], channels, &mut actual[split..]);
                assert_eq!(actual[split..], expected[split..]);
            }
        }
    }

    #[test]
    fn reset_and_sample_rate_changes_restart_only_when_requested() {
        for rate in RATES {
            let input = signal(197, 2);
            let mut delay = NativeInputDelay::new(rate);
            let mut uninterrupted = NativeInputDelay::new(rate);
            let mut actual = vec![0.0; input.len()];
            let mut expected = vec![0.0; input.len()];
            delay.process(&input, 2, &mut actual);
            uninterrupted.process(&input, 2, &mut expected);
            delay.set_sample_rate(rate);
            delay.process(&input, 2, &mut actual);
            uninterrupted.process(&input, 2, &mut expected);
            assert_eq!(actual, expected);
            delay.reset();
            NativeInputDelay::new(rate).process(&input, 2, &mut expected);
            delay.process(&input, 2, &mut actual);
            assert_eq!(actual, expected);
            for new_rate in RATES.into_iter().filter(|&new_rate| new_rate != rate) {
                delay.set_sample_rate(new_rate);
                delay.process(&input, 2, &mut actual);
                NativeInputDelay::new(new_rate).process(&input, 2, &mut expected);
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn full_scale_pcm_preserves_bits_without_clipping_or_overshoot() {
        let bits = [
            0.0_f32.to_bits(),
            (-0.0_f32).to_bits(),
            1.0_f32.to_bits(),
            (-1.0_f32).to_bits(),
            f32::MIN_POSITIVE.to_bits(),
            (-f32::MIN_POSITIVE).to_bits(),
        ];
        for (rate, samples) in [
            (8_000, 31),
            (12_000, 44),
            (16_000, 59),
            (24_000, 0),
            (48_000, 0),
        ] {
            let enabled = NativeInputDelay::new(rate).enabled();
            assert_eq!(enabled, rate < 24_000);
            for channels in 1..=2 {
                let input: Vec<_> = (0..257 * channels)
                    .map(|index| f32::from_bits(bits[index % bits.len()]))
                    .collect();
                let mut output = vec![0.0; input.len()];
                NativeInputDelay::new(rate).process(&input, channels, &mut output);
                assert!(
                    output
                        .iter()
                        .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
                );
                let offset = samples * channels;
                for (&actual, &expected) in
                    output[offset..].iter().zip(&input[..input.len() - offset])
                {
                    assert_eq!(actual.to_bits(), expected.to_bits());
                }
            }
        }
    }
}
