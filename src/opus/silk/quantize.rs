// SPDX-License-Identifier: MIT
//! Causal scalar noise-shaping quantization for the owned SILK encoder.

use super::frame::gain_q16;
use super::pitch::Pitch;
use super::{Frame, FrameType, QuantizationOffset, SignalType};

const MAX_SAMPLES: usize = 320;
const MAX_ORDER: usize = 16;
const OUTPUT_HISTORY: usize = 306;
// A stable encoder-only feedback pole colors quantization error toward lower
// frequencies, rather than spreading equal-power error across quiet high bands.
const NOISE_POLE: f64 = 0.65;

fn noise_pole(samples: &[f32], gain: f64) -> f64 {
    let energy: f64 = samples
        .iter()
        .map(|&sample| f64::from(sample).powi(2))
        .sum();
    // Leave already fine quantization alone. For unit cells, the unshaped
    // noise-power estimate is step^2/12; color only when estimated noise exceeds
    // one percent of input energy (estimated quantization SNR below 20 dB).
    let noise = (gain / 32_768.0).powi(2) * samples.len() as f64 / 12.0;
    if noise <= 0.01 * energy {
        return 0.0;
    }
    let correlation: f64 = samples
        .windows(2)
        .map(|pair| f64::from(pair[0]) * f64::from(pair[1]))
        .sum();
    // A low-pass error spectrum only matches a sufficiently dark input.
    // Retain unshaped cells for bright/noisy audio and silence.
    if energy > 1e-15 && correlation > 0.9 * energy {
        NOISE_POLE
    } else {
        0.0
    }
}

#[derive(Clone, Copy)]
pub(super) struct History {
    pub(super) lpc: [f64; MAX_ORDER],
    pub(super) gain: Option<i32>,
    pub(super) output: [f64; OUTPUT_HISTORY],
    pub(super) voiced: bool,
    pub(super) shaping_error: f64,
}

impl History {
    pub(super) const fn new() -> Self {
        Self {
            lpc: [0.0; MAX_ORDER],
            gain: None,
            output: [0.0; OUTPUT_HISTORY],
            voiced: false,
            shaping_error: 0.0,
        }
    }
}

pub(super) struct Quantized {
    pub(super) pulses: [i16; MAX_SAMPLES],
    pub(super) gain_symbols: [u8; 4],
    pub(super) gain_index: i32,
    pub(super) history: History,
    pub(super) distortion: f64,
}

fn delta(previous: i32, index: i32) -> i32 {
    (2 * index - 16).max(previous + index - 4).clamp(0, 63)
}

fn gain_symbols(previous: Option<i32>, requested: i32, conditional: bool) -> ([u8; 4], i32) {
    let (wire, effective) = if conditional {
        let previous = previous.unwrap();
        let mut best = 0;
        let mut distance = i32::MAX;
        for index in 0..=40 {
            let difference = (delta(previous, index) - requested).abs();
            if difference < distance {
                best = index;
                distance = difference;
            }
        }
        (best, delta(previous, best))
    } else {
        let effective = previous.map_or(requested, |index| requested.max(index - 16));
        (effective, effective)
    };
    // Keeping the gain within a frame constant makes the remaining delta
    // symbols use index four, the highest-probability neutral step.
    let mut symbols = [4; 4];
    symbols[0] = wire as u8;
    (symbols, effective)
}

pub(super) struct Parameters<'a> {
    pub(super) frame: Frame,
    pub(super) kind: FrameType,
    pub(super) coefficients: &'a [i16],
    pub(super) previous: History,
    pub(super) requested_gain: i32,
    pub(super) conditional: bool,
    pub(super) seed: u8,
    pub(super) pitch: Pitch,
    pub(super) low_rate: bool,
}

fn nearest_pulse(center: f64) -> i16 {
    // Reconstruction is n - sign(n)*20/256. Its two central cells are
    // wider than the ordinary unit cells; ties choose the lower integer.
    let zero_boundary = (1.0 - 20.0 / 256.0) * 0.5;
    let value = if center > zero_boundary {
        (center + 20.0 / 256.0 - 0.5).ceil().max(1.0)
    } else if center <= -zero_boundary {
        (center - 20.0 / 256.0 - 0.5).ceil().min(-1.0)
    } else {
        0.0
    };
    value.clamp(-17_407.0, 17_407.0) as i16
}

pub(super) fn quantize(samples: &[f32], parameters: Parameters<'_>) -> Quantized {
    // Analysis produces the two standardized predictor orders. Dispatch once
    // so the sample loop has a fixed dot-product width.
    match parameters.coefficients.len() {
        10 => quantize_order::<10>(samples, parameters),
        16 => quantize_order::<16>(samples, parameters),
        _ => unreachable!("validated SILK predictor order"),
    }
}

fn quantize_order<const ORDER: usize>(samples: &[f32], parameters: Parameters<'_>) -> Quantized {
    let Parameters {
        frame,
        kind,
        coefficients,
        previous,
        requested_gain,
        conditional,
        seed,
        pitch,
        low_rate,
    } = parameters;
    let (gain_symbols, gain_index) = gain_symbols(previous.gain, requested_gain, conditional);
    let gain = f64::from(gain_q16(gain_index)) / 65_536.0;
    let offset = match (kind.signal, kind.offset) {
        (SignalType::Voiced, QuantizationOffset::Low) => 8.0,
        (SignalType::Voiced, QuantizationOffset::High) | (_, QuantizationOffset::Low) => 25.0,
        (_, QuantizationOffset::High) => 60.0,
    } / 256.0;
    // Power-of-two scaling is exact for these bounded predictor values.
    // Convert once per frame instead of once per sample and predictor tap.
    let mut scaled = [0.0; MAX_ORDER];
    for (destination, &value) in scaled.iter_mut().zip(coefficients) {
        *destination = f64::from(value) / 4096.0;
    }
    let ltp_scaled = pitch
        .taps
        .map(|taps| taps.map(|value| f64::from(value) / 16384.0));
    let mut history = previous;
    let noise_pole = if low_rate {
        noise_pole(samples, gain)
    } else {
        0.0
    };
    let mut seed = u32::from(seed);
    let mut pulses = [0; MAX_SAMPLES];
    let mut distortion = 0.0;
    let mut outputs = [0.0; OUTPUT_HISTORY + MAX_SAMPLES];
    outputs[..OUTPUT_HISTORY].copy_from_slice(&previous.output);
    let mut residual = [0.0; OUTPUT_HISTORY + MAX_SAMPLES];
    if pitch.lag != 0 {
        for index in OUTPUT_HISTORY - pitch.lag - 2..OUTPUT_HISTORY {
            let prediction = scaled[..ORDER]
                .iter()
                .enumerate()
                .map(|(tap, &value)| outputs[index - tap - 1] * value)
                .sum::<f64>();
            residual[index] =
                (outputs[index] - prediction).clamp(-1.0, 1.0) * (15565.0 / 16384.0) / gain;
        }
    }
    let subframe_samples = frame.samples()
        / match frame.duration {
            super::FrameDuration::Ms10 => 2,
            super::FrameDuration::Ms20 => 4,
        };
    for (index, &sample) in samples.iter().enumerate() {
        let mut predicted = 0.0;
        for (&coefficient, &previous) in scaled[..ORDER].iter().zip(&history.lpc) {
            predicted += coefficient * previous;
        }
        let position = OUTPUT_HISTORY + index;
        let mut ltp_prediction = 0.0;
        if pitch.lag != 0 {
            for (tap, &coefficient) in ltp_scaled[index / subframe_samples].iter().enumerate() {
                ltp_prediction += residual[position - pitch.lag + 2 - tap] * coefficient;
            }
        }
        let unshaped_target = f64::from(sample) - predicted - gain * ltp_prediction;
        let target = unshaped_target + noise_pole * history.shaping_error;
        seed = seed.wrapping_mul(196_314_165).wrapping_add(907_633_515);
        let modulation = if seed & 0x8000_0000 == 0 { 1.0 } else { -1.0 };
        let chosen = nearest_pulse(target * 32_768.0 * modulation / gain - offset);
        let excitation =
            (f64::from(chosen) - f64::from(chosen.signum()) * (20.0 / 256.0) + offset) * modulation;
        let reconstructed = excitation * gain / 32_768.0;
        pulses[index] = chosen;
        distortion += (unshaped_target - reconstructed).powi(2);
        seed = seed.wrapping_add_signed(i32::from(chosen));
        history.lpc.copy_within(0..MAX_ORDER - 1, 1);
        residual[position] = reconstructed / gain + ltp_prediction;
        history.lpc[0] = predicted + gain * residual[position];
        history.shaping_error = history.lpc[0] - f64::from(sample);
        outputs[position] = history.lpc[0].clamp(-1.0, 1.0);
    }
    // The final MB tail is coded as zero, but never contributes predictor
    // history: only the meaningful samples pass through the quantizer.
    debug_assert!(samples.len() == frame.samples());
    history.gain = Some(gain_index);
    history
        .output
        .copy_from_slice(&outputs[samples.len()..samples.len() + OUTPUT_HISTORY]);
    history.voiced = pitch.lag != 0;
    Quantized {
        pulses,
        gain_symbols,
        gain_index,
        history,
        distortion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::silk::{Bandwidth, FrameDuration, reconstruct_excitation};

    #[test]
    fn direct_scalar_quantizer_is_nearest_at_every_wire_cell_boundary() {
        let reconstructed =
            |value: i32| f64::from(value) - f64::from(value.signum()) * 20.0 / 256.0;
        for lower in -17_407..17_407 {
            let left = reconstructed(lower);
            let right = reconstructed(lower + 1);
            let boundary = (left + right) * 0.5;
            assert_eq!(i32::from(nearest_pulse(boundary)), lower);
            assert_eq!(i32::from(nearest_pulse(boundary - 1e-8)), lower);
            assert_eq!(i32::from(nearest_pulse(boundary + 1e-8)), lower + 1);
            assert_eq!(i32::from(nearest_pulse(left)), lower);
        }
        assert_eq!(nearest_pulse(-1e12), -17407);
        assert_eq!(nearest_pulse(1e12), 17407);
        assert_eq!(nearest_pulse(0.0), 0);
    }

    #[test]
    fn scalar_quantizer_reconstruction_matches_decoder_seed_and_offset() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let samples: [f32; 80] = std::array::from_fn(|index| 0.2 * (index as f32 * 0.17).sin());
        for signal in [
            SignalType::Inactive,
            SignalType::Unvoiced,
            SignalType::Voiced,
        ] {
            for offset in [QuantizationOffset::Low, QuantizationOffset::High] {
                for seed in 0..4 {
                    let kind = FrameType { signal, offset };
                    let quantized = quantize(
                        &samples,
                        Parameters {
                            frame,
                            kind,
                            coefficients: &[0; 10],
                            previous: History::new(),
                            requested_gain: 32,
                            conditional: false,
                            seed,
                            pitch: Pitch::NONE,
                            low_rate: true,
                        },
                    );
                    let mut excitation = [0; 80];
                    reconstruct_excitation(&quantized.pulses[..80], kind, seed, &mut excitation)
                        .unwrap();
                    let gain = f64::from(gain_q16(32)) / 65_536.0;
                    let expected = f64::from(excitation[79]) * gain / 8_388_608.0;
                    assert!((quantized.history.lpc[0] - expected).abs() < 1e-12);
                    let error: f64 = samples
                        .iter()
                        .zip(&excitation)
                        .map(|(&sample, &value)| {
                            (f64::from(sample) - f64::from(value) * gain / 8_388_608.0).powi(2)
                        })
                        .sum();
                    assert!((error - quantized.distortion).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn shaped_cells_are_nearest_in_independent_decoder_reconstruction() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let kind = FrameType {
            signal: SignalType::Unvoiced,
            offset: QuantizationOffset::Low,
        };
        let samples: [f32; 80] =
            std::array::from_fn(|i| 0.2 * (i as f32 * 0.29).sin() + 0.03 * (i as f32 * 1.31).cos());
        let chosen = quantize(
            &samples,
            Parameters {
                frame,
                kind,
                coefficients: &[0; 10],
                previous: History::new(),
                requested_gain: 48,
                conditional: false,
                seed: 2,
                pitch: Pitch::NONE,
                low_rate: true,
            },
        );
        let mut excitation = [0; 80];
        reconstruct_excitation(&chosen.pulses[..80], kind, 2, &mut excitation).unwrap();
        let scale = f64::from(gain_q16(48)) / (65_536.0 * 8_388_608.0);
        let mut previous_error = 0.0;
        for i in 0..80 {
            let actual = f64::from(excitation[i]) * scale;
            let wanted = f64::from(samples[i]) + 0.65 * previous_error;
            let actual_error = (actual - wanted).powi(2);
            for delta in [-2, -1, 1, 2] {
                let mut alternative = chosen.pulses;
                alternative[i] += delta;
                let mut oracle = [0; 80];
                reconstruct_excitation(&alternative[..=i], kind, 2, &mut oracle).unwrap();
                let value = f64::from(oracle[i]) * scale;
                assert!(actual_error <= (value - wanted).powi(2) + 1e-16);
            }
            previous_error = actual - f64::from(samples[i]);
        }
        assert!((chosen.history.shaping_error - previous_error).abs() < 1e-12);
    }

    #[test]
    fn error_coloring_only_follows_dark_nonzero_input() {
        assert_eq!(noise_pole(&[0.0; 80], 3000.0), 0.0);
        assert_eq!(noise_pole(&[0.1; 80], 3000.0), 0.65);
        assert_eq!(noise_pole(&[0.1; 80], 1.25), 0.0);
        let bright: [f32; 80] = std::array::from_fn(|i| if i % 2 == 0 { 0.1 } else { -0.1 });
        assert_eq!(noise_pole(&bright, 3000.0), 0.0);
        let mut impulse = [0.0; 80];
        impulse[40] = 0.1;
        assert_eq!(noise_pole(&impulse, 3000.0), 0.0);
    }

    #[test]
    fn gain_wire_symbols_obey_decoder_clamps_and_conditional_steps() {
        for previous in 0..64 {
            for requested in 0..64 {
                let (symbols, effective) = gain_symbols(Some(previous), requested, false);
                assert_eq!(effective, requested.max(previous - 16));
                assert_eq!(i32::from(symbols[0]), effective);
                let (symbols, effective) = gain_symbols(Some(previous), requested, true);
                assert_eq!(effective, delta(previous, i32::from(symbols[0])));
                assert!(symbols[1..].iter().all(|&value| value == 4));
            }
        }
    }
}
