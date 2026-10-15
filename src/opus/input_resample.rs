// SPDX-License-Identifier: MIT
//! Owned causal interpolation from the standard Opus input rates to 48 kHz.
//!
//! This independently designed FIR uses a 193-tap Blackman-windowed sinc in
//! the 48 kHz domain, with cutoff at 90% of the input Nyquist frequency. Each
//! rational phase is normalized separately for unit DC gain. The symmetric
//! kernel adds 96 output samples (2 ms) of delay at every interpolated rate.
//! Construction and rate changes prepare the coefficients; processing has no
//! allocation, trigonometry, or dependency on an external codec or resampler.
//! The finite transition band intentionally rolls off near input Nyquist. For
//! 8 kHz input, gain is approximately -0.34 dB at 3.2 kHz, -6.02 dB at 3.6 kHz,
//! and -16.36 dB at 3.84 kHz; first-image rejection at 3.84 kHz is about 32 dB.
//! These are properties of this bounded-delay filter, not a libopus quality
//! equivalence claim. Callers must include its delayed tail when draining PCM.
//!
//! The caller validates finite normalized PCM, channels, and exact buffer
//! lengths before processing. A band-limited interpolation can overshoot even
//! when its input lies in [-1, 1]. Filtered output is clamped to that interval
//! for the encoder's normalized PCM contract; full-scale transients can thus
//! acquire clipping distortion in addition to the FIR's pre/post ringing.
//! The 48 kHz path is an exact, zero-delay copy, including signed zero.

use super::encoder::Error;

const OUTPUT_RATE: u32 = 48_000;
const FILTER_TAPS: usize = 193;
const FILTER_DELAY: usize = (FILTER_TAPS - 1) / 2;
const MAX_RATIO: usize = 6;
const MAX_PHASE_TAPS: usize = (FILTER_TAPS - 1) / 2 + 1;
const HISTORY: usize = 128;
const HISTORY_MASK: usize = HISTORY - 1;

/// Fixed-memory input-rate conversion owned by the ordinary encoder.
pub(super) struct InputResampler {
    sample_rate: u32,
    ratio: usize,
    coefficients: [[f32; MAX_PHASE_TAPS]; MAX_RATIO],
    phase_taps: [usize; MAX_RATIO],
    history: [[f32; HISTORY]; 2],
    position: usize,
}

impl InputResampler {
    /// Prepare interpolation for one of the five standard Opus input rates.
    pub(super) fn new(sample_rate: u32) -> Result<Self, Error> {
        let mut result = Self {
            sample_rate: OUTPUT_RATE,
            ratio: 1,
            coefficients: [[0.0; MAX_PHASE_TAPS]; MAX_RATIO],
            phase_taps: [0; MAX_RATIO],
            history: [[0.0; HISTORY]; 2],
            position: 0,
        };
        result.set_sample_rate(sample_rate)?;
        Ok(result)
    }

    pub(super) const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Additional delay in the encoder's 48 kHz processing domain.
    pub(super) const fn delay_samples_48khz(&self) -> usize {
        if self.ratio == 1 { 0 } else { FILTER_DELAY }
    }

    /// Clear the causal history while retaining the configured rate and taps.
    pub(super) fn reset(&mut self) {
        self.history.fill([0.0; HISTORY]);
        self.position = 0;
    }

    /// Reconfigure without allocation, resetting history only on a rate change.
    /// An invalid rate leaves every part of the previous state untouched.
    pub(super) fn set_sample_rate(&mut self, sample_rate: u32) -> Result<(), Error> {
        let ratio = match sample_rate {
            8_000 => 6,
            12_000 => 4,
            16_000 => 3,
            24_000 => 2,
            48_000 => 1,
            _ => return Err(Error::Argument),
        };
        if self.sample_rate == sample_rate {
            return Ok(());
        }

        self.coefficients.fill([0.0; MAX_PHASE_TAPS]);
        self.phase_taps.fill(0);
        if ratio != 1 {
            let cutoff = 0.45 / ratio as f64;
            for phase in 0..ratio {
                let taps = (FILTER_TAPS - 1 - phase) / ratio + 1;
                let mut sum = 0.0;
                let mut phase_coefficients = [0.0_f64; MAX_PHASE_TAPS];
                for (lag, coefficient) in phase_coefficients[..taps].iter_mut().enumerate() {
                    let tap = phase + lag * ratio;
                    let offset = tap as f64 - FILTER_DELAY as f64;
                    let angle = std::f64::consts::TAU * tap as f64 / (FILTER_TAPS - 1) as f64;
                    let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                    let sinc = if offset == 0.0 {
                        2.0 * cutoff
                    } else {
                        (std::f64::consts::TAU * cutoff * offset).sin()
                            / (std::f64::consts::PI * offset)
                    };
                    *coefficient = window * sinc;
                    sum += *coefficient;
                }
                for (target, coefficient) in self.coefficients[phase][..taps]
                    .iter_mut()
                    .zip(&phase_coefficients[..taps])
                {
                    *target = (coefficient / sum) as f32;
                }
                self.phase_taps[phase] = taps;
            }
        }
        self.sample_rate = sample_rate;
        self.ratio = ratio;
        self.reset();
        Ok(())
    }

    /// Interpolate validated interleaved mono or stereo PCM into an exact buffer.
    pub(super) fn process(&mut self, input: &[f32], channels: usize, output: &mut [f32]) {
        debug_assert!((1..=2).contains(&channels));
        debug_assert!(input.len().is_multiple_of(channels));
        debug_assert_eq!(output.len(), input.len() * self.ratio);
        if self.ratio == 1 {
            output.copy_from_slice(input);
            return;
        }

        for (input_frame, output_frames) in input
            .chunks_exact(channels)
            .zip(output.chunks_exact_mut(self.ratio * channels))
        {
            for (channel, &sample) in input_frame.iter().enumerate() {
                self.history[channel][self.position] = sample;
            }
            for (phase, output_frame) in output_frames.chunks_exact_mut(channels).enumerate() {
                let taps = self.phase_taps[phase];
                for (channel, output_sample) in output_frame.iter_mut().enumerate() {
                    let mut sum = 0.0;
                    for (lag, &coefficient) in self.coefficients[phase][..taps].iter().enumerate() {
                        let index = self.position.wrapping_sub(lag) & HISTORY_MASK;
                        sum += self.history[channel][index] * coefficient;
                    }
                    *output_sample = sum.clamp(-1.0, 1.0);
                }
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
                let x = frame as f64;
                (0.31 * (x * (0.173 + channel as f64 * 0.13)).sin() + 0.11 * (x * 0.377).cos())
                    as f32
            })
            .collect()
    }

    // Deliberately use literal zero insertion and full convolution, without
    // reading the production phase tables or its ring-buffer layout.
    fn zero_stuffed_convolution(input: &[f32], channels: usize, sample_rate: u32) -> Vec<f32> {
        let ratio = (48_000 / sample_rate) as usize;
        if ratio == 1 {
            return input.to_vec();
        }
        let mut kernel = [0.0_f64; 193];
        let mut phase_sums = vec![0.0; ratio];
        for (index, coefficient) in kernel.iter_mut().enumerate() {
            let relative = index as f64 - 96.0;
            let normalized_cutoff = 0.45 / ratio as f64;
            let argument = 2.0 * normalized_cutoff * relative;
            let sinc = if argument == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * argument).sin() / (std::f64::consts::PI * argument)
            };
            let turn = index as f64 / 192.0;
            let blackman = 0.42 - 0.5 * (std::f64::consts::TAU * turn).cos()
                + 0.08 * (2.0 * std::f64::consts::TAU * turn).cos();
            *coefficient = sinc * blackman;
            phase_sums[index % ratio] += *coefficient;
        }
        for (index, coefficient) in kernel.iter_mut().enumerate() {
            *coefficient /= phase_sums[index % ratio];
        }
        let mut zero_stuffed = vec![0.0; input.len() * ratio];
        for (frame, input_frame) in input.chunks_exact(channels).enumerate() {
            zero_stuffed[frame * ratio * channels..frame * ratio * channels + channels]
                .copy_from_slice(input_frame);
        }
        let mut output = vec![0.0; zero_stuffed.len()];
        for (index, output_frame) in output.chunks_exact_mut(channels).enumerate() {
            for (channel, output_sample) in output_frame.iter_mut().enumerate() {
                let sum: f64 = kernel[..kernel.len().min(index + 1)]
                    .iter()
                    .enumerate()
                    .map(|(lag, &coefficient)| {
                        f64::from(zero_stuffed[(index - lag) * channels + channel]) * coefficient
                    })
                    .sum();
                *output_sample = sum.clamp(-1.0, 1.0) as f32;
            }
        }
        output
    }

    #[test]
    fn all_rates_match_literal_zero_stuffed_convolution() {
        for rate in RATES {
            for channels in 1..=2 {
                let input = signal(311, channels);
                let expected = zero_stuffed_convolution(&input, channels, rate);
                let mut actual = vec![0.0; expected.len()];
                InputResampler::new(rate)
                    .unwrap()
                    .process(&input, channels, &mut actual);
                let error = actual
                    .iter()
                    .zip(expected)
                    .map(|(&actual, expected)| (actual - expected).abs())
                    .fold(0.0_f32, f32::max);
                assert!(error < 5e-7, "{rate} Hz, {channels} channels: {error}");
            }
        }
    }

    #[test]
    fn split_blocks_same_rate_update_and_reset_preserve_streaming_contract() {
        for rate in RATES {
            for channels in 1..=2 {
                let input = signal(521, channels);
                let ratio = (48_000 / rate) as usize;
                let mut expected = vec![0.0; input.len() * ratio];
                let mut actual = vec![0.0; expected.len()];
                let mut resampler = InputResampler::new(rate).unwrap();
                resampler.process(&input, channels, &mut expected);
                resampler.reset();
                let mut frame = 0;
                for block in [1, 3, 127, 2, 129, 1, 258] {
                    let start = frame * channels;
                    let end = (frame + block) * channels;
                    resampler.set_sample_rate(rate).unwrap();
                    resampler.process(
                        &input[start..end],
                        channels,
                        &mut actual[start * ratio..end * ratio],
                    );
                    frame += block;
                }
                assert_eq!(frame, 521);
                assert_eq!(actual, expected, "{rate} Hz, {channels} channels");
                resampler.reset();
                resampler.process(&input, channels, &mut actual);
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn rate_changes_reset_history_and_invalid_rates_are_transactional() {
        for rate in RATES {
            let mut actual = InputResampler::new(rate).unwrap();
            let mut expected = InputResampler::new(rate).unwrap();
            let ratio = (48_000 / rate) as usize;
            let mut warmup = vec![0.0; 97 * ratio];
            actual.process(&signal(97, 1), 1, &mut warmup);
            expected.process(&signal(97, 1), 1, &mut warmup);
            for invalid in [0, 1, 7_999, 44_100, 48_001, u32::MAX] {
                assert!(matches!(
                    actual.set_sample_rate(invalid),
                    Err(Error::Argument)
                ));
                assert!(matches!(InputResampler::new(invalid), Err(Error::Argument)));
                assert_eq!(actual.sample_rate(), rate);
            }
            let input = signal(23, 1);
            let mut actual_pcm = vec![0.0; input.len() * ratio];
            let mut expected_pcm = vec![0.0; input.len() * ratio];
            actual.process(&input, 1, &mut actual_pcm);
            expected.process(&input, 1, &mut expected_pcm);
            assert_eq!(actual_pcm, expected_pcm);

            for new_rate in RATES.into_iter().filter(|&new_rate| new_rate != rate) {
                actual.set_sample_rate(new_rate).unwrap();
                assert_eq!(actual.sample_rate(), new_rate);
                let mut fresh = InputResampler::new(new_rate).unwrap();
                let new_ratio = (48_000 / new_rate) as usize;
                actual_pcm.resize(input.len() * new_ratio, 0.0);
                expected_pcm.resize(input.len() * new_ratio, 0.0);
                actual.process(&input, 1, &mut actual_pcm);
                fresh.process(&input, 1, &mut expected_pcm);
                assert_eq!(actual_pcm, expected_pcm);
            }
        }
    }

    #[test]
    fn bypass_preserves_every_bit_and_has_no_delay() {
        let input = [0.0, -0.0, 1.0, -1.0, f32::MIN_POSITIVE, -f32::MIN_POSITIVE];
        let mut output = [0.0; 6];
        let mut resampler = InputResampler::new(48_000).unwrap();
        assert_eq!(resampler.delay_samples_48khz(), 0);
        for channels in 1..=2 {
            resampler.process(&input, channels, &mut output);
            assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
        }
    }

    #[test]
    fn impulse_peaks_at_reported_delay_and_channels_remain_independent() {
        for rate in RATES {
            for channels in 1..=2 {
                let ratio = (48_000 / rate) as usize;
                let mut input = vec![0.0; 128 * channels];
                input[0] = 0.5;
                let mut output = vec![0.0; input.len() * ratio];
                let mut resampler = InputResampler::new(rate).unwrap();
                resampler.process(&input, channels, &mut output);
                let peak = output
                    .chunks_exact(channels)
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a[0].abs().total_cmp(&b[0].abs()))
                    .unwrap()
                    .0;
                assert_eq!(peak, resampler.delay_samples_48khz(), "{rate} Hz");
                if channels == 2 {
                    assert!(
                        output
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .all(|frame| frame[1] == 0.0)
                    );
                }
                let sum: f32 = output.chunks_exact(channels).map(|frame| frame[0]).sum();
                assert!((sum - 0.5 * ratio as f32).abs() < 2e-6);
                let tail_end = if ratio == 1 { 1 } else { 193 };
                assert!(
                    output[tail_end * channels..]
                        .iter()
                        .all(|&sample| sample == 0.0)
                );
            }
        }
    }

    #[test]
    fn settled_dc_has_unit_gain_for_every_phase_and_channel() {
        for rate in RATES {
            for channels in 1..=2 {
                let ratio = (48_000 / rate) as usize;
                let input: Vec<_> = (0..256 * channels)
                    .map(|index| if index % channels == 0 { 0.25 } else { -0.375 })
                    .collect();
                let mut output = vec![0.0; input.len() * ratio];
                InputResampler::new(rate)
                    .unwrap()
                    .process(&input, channels, &mut output);
                for frame in output[193 * channels..].chunks_exact(channels) {
                    assert!((frame[0] - 0.25).abs() < 2e-7, "{rate} Hz");
                    if channels == 2 {
                        assert!((frame[1] + 0.375).abs() < 3e-7, "{rate} Hz");
                    }
                }
            }
        }
    }

    fn tone_projection(output: &[f32], channels: usize, channel: usize, cycles: usize) -> f64 {
        let frames = output.len() / channels;
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, frame) in output.chunks_exact(channels).enumerate() {
            let angle = std::f64::consts::TAU * cycles as f64 * index as f64 / frames as f64;
            real += f64::from(frame[channel]) * angle.cos();
            imaginary += f64::from(frame[channel]) * angle.sin();
        }
        2.0 * real.hypot(imaginary) / frames as f64
    }

    #[test]
    fn passband_tones_retain_gain_and_reject_interpolation_images() {
        // Integer cycles over both the input and output block avoid leakage
        // when projecting the wanted tone and each zero-insertion image.
        const FRAMES: usize = 2000;
        for rate in RATES {
            for channels in 1..=2 {
                let ratio = (48_000 / rate) as usize;
                for cycles in [200, 500, 700] {
                    let input: Vec<_> = (0..(FRAMES + 128) * channels)
                        .map(|index| {
                            let frame = index / channels;
                            let channel = index % channels;
                            let angle = std::f64::consts::TAU * cycles as f64 * frame as f64
                                / FRAMES as f64;
                            ((0.3 - 0.1 * channel as f64) * angle.sin()) as f32
                        })
                        .collect();
                    let mut output = vec![0.0; input.len() * ratio];
                    InputResampler::new(rate)
                        .unwrap()
                        .process(&input, channels, &mut output);
                    let settled = &output[128 * ratio * channels..];
                    for channel in 0..channels {
                        let expected_gain = 0.3 - 0.1 * channel as f64;
                        let gain = tone_projection(settled, channels, channel, cycles);
                        assert!(
                            (gain - expected_gain).abs() < 0.0015,
                            "{rate} Hz, tone {cycles}: {gain}"
                        );
                        for image in 1..ratio {
                            let image_cycles = FRAMES * image - cycles;
                            let gain = tone_projection(settled, channels, channel, image_cycles);
                            assert!(
                                gain < 0.0001,
                                "{rate} Hz, tone {cycles}, image {image}: {gain}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn narrowband_transition_gain_and_near_nyquist_image_match_documented_bounds() {
        const FRAMES: usize = 2000;
        const WARMUP: usize = 128;
        const RATIO: usize = 6;
        const AMPLITUDE: f64 = 0.25;
        for (cycles, expected_gain_db) in [(800, -0.34), (900, -6.02), (960, -16.36)] {
            let input: Vec<_> = (0..FRAMES + WARMUP)
                .map(|frame| {
                    let angle =
                        std::f64::consts::TAU * cycles as f64 * frame as f64 / FRAMES as f64;
                    (AMPLITUDE * angle.sin()) as f32
                })
                .collect();
            let mut output = vec![0.0; input.len() * RATIO];
            InputResampler::new(8_000)
                .unwrap()
                .process(&input, 1, &mut output);
            let settled = &output[WARMUP * RATIO..];
            let wanted = tone_projection(settled, 1, 0, cycles);
            let gain_db = 20.0 * (wanted / AMPLITUDE).log10();
            let frequency = 8_000 * cycles / FRAMES;
            println!("8 kHz input, {frequency} Hz tone: {gain_db:.3} dB gain");
            assert!(
                (gain_db - expected_gain_db).abs() < 0.05,
                "{frequency} Hz gain {gain_db} dB, expected approximately {expected_gain_db} dB"
            );
            if cycles == 960 {
                let image = tone_projection(settled, 1, 0, FRAMES - cycles);
                let rejection_db = 20.0 * (wanted / image).log10();
                println!(
                    "8 kHz input, {frequency} Hz tone: {rejection_db:.3} dB first-image rejection"
                );
                assert!(
                    (rejection_db - 32.0).abs() < 1.0,
                    "{frequency} Hz first-image rejection {rejection_db} dB, expected about 32 dB"
                );
            }
        }
    }

    #[test]
    fn full_scale_steps_are_finite_normalized_and_empty_calls_do_not_change_state() {
        for rate in RATES {
            for channels in 1_usize..=2 {
                let ratio = (48_000 / rate) as usize;
                for pattern in 0..3 {
                    let input: Vec<_> = (0..311 * channels)
                        .map(|index| {
                            let frame = index / channels;
                            let channel = index % channels;
                            let polarity = if channel == 0 { 1.0 } else { -1.0 };
                            match pattern {
                                0 => {
                                    if (frame / 13).is_multiple_of(2) {
                                        polarity
                                    } else {
                                        -polarity
                                    }
                                }
                                1 => {
                                    if frame.is_multiple_of(2) {
                                        polarity
                                    } else {
                                        -polarity
                                    }
                                }
                                _ => match frame {
                                    0 => polarity,
                                    151 => -polarity,
                                    _ => 0.0,
                                },
                            }
                        })
                        .collect();
                    let mut expected = vec![0.0; input.len() * ratio];
                    let mut actual = vec![0.0; expected.len()];
                    InputResampler::new(rate)
                        .unwrap()
                        .process(&input, channels, &mut expected);
                    let mut resampler = InputResampler::new(rate).unwrap();
                    let mut start = 0;
                    for frames in [1, 13, 127, 3, 167] {
                        let end = start + frames * channels;
                        resampler.process(&[], channels, &mut []);
                        resampler.process(
                            &input[start..end],
                            channels,
                            &mut actual[start * ratio..end * ratio],
                        );
                        start = end;
                    }
                    assert_eq!(start, input.len());
                    assert_eq!(actual, expected, "{rate} Hz, pattern {pattern}");
                    assert!(
                        actual
                            .iter()
                            .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
                    );
                }
            }
        }
    }
}
