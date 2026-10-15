// SPDX-License-Identifier: MIT
//! SILK reconstruction equations from RFC 6716 section 4.2.7.9.
//!
//! SILK synthesis is explicitly not bit-exact in the specification. This
//! implementation evaluates those equations with f32 history, preserving the
//! distinct clamped LTP and unclamped LPC histories across frame boundaries.

use super::{Bandwidth, Error, Frame};

const OUTPUT_HISTORY: usize = 306;
const LPC_HISTORY: usize = 16;
const MAX_SAMPLES: usize = 320;

/// Prediction parameters of a fully parsed internal frame.
#[derive(Clone, Copy)]
pub(crate) struct Parameters {
    pub(crate) gains_q16: [i32; 4],
    pub(crate) lpc_q12: [[i16; 16]; 2],
    pub(crate) interpolation: u8,
    pub(crate) voiced: bool,
    pub(crate) pitch_lags: [usize; 4],
    pub(crate) ltp_q14: [[i16; 5]; 4],
    pub(crate) ltp_scale_q14: i16,
}

/// A channel's bounded, allocation-free SILK synthesis history.
pub(crate) struct Synthesis {
    output: [f32; OUTPUT_HISTORY],
    lpc: [f32; LPC_HISTORY],
}

impl Synthesis {
    pub(crate) const fn new() -> Self {
        Self {
            output: [0.0; OUTPUT_HISTORY],
            lpc: [0.0; LPC_HISTORY],
        }
    }

    pub(crate) fn reset(&mut self) {
        self.output.fill(0.0);
        self.lpc.fill(0.0);
    }

    /// Independent loss reconstruction: extrapolate the last stable LPC/LTP
    /// model, with gradual predictor damping and shaped noise for unvoiced audio.
    pub(crate) fn conceal_samples(
        &mut self,
        frame: Frame,
        samples: usize,
        previous: &Parameters,
        attenuation: f64,
        seed: &mut u32,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        if output.len() < samples {
            return Err(Error::Output);
        }
        let last = previous
            .gains_q16
            .iter()
            .rposition(|&gain| gain >= 81_920)
            .ok_or(Error::Argument)?;
        let mut parameters = *previous;
        parameters.interpolation = 4;
        parameters.gains_q16 = [previous.gains_q16[last]; 4];
        parameters.pitch_lags = [previous.pitch_lags[last]; 4];
        let order = if frame.bandwidth == Bandwidth::Wide {
            16
        } else {
            10
        };
        let mut chirp = 0.98;
        for (destination, &coefficient) in parameters.lpc_q12[1][..order]
            .iter_mut()
            .zip(&previous.lpc_q12[1])
        {
            *destination = (f64::from(coefficient) * chirp).round() as i16;
            chirp *= 0.98;
        }
        parameters.lpc_q12[0] = parameters.lpc_q12[1];
        for taps in &mut parameters.ltp_q14 {
            for (destination, &coefficient) in taps.iter_mut().zip(&previous.ltp_q14[last]) {
                *destination = (f64::from(coefficient) * attenuation).round() as i16;
            }
        }
        let mut excitation = [0; MAX_SAMPLES];
        if !parameters.voiced {
            let coefficients = &previous.lpc_q12[1][..order];
            let mut energy = 0.0;
            for position in order..OUTPUT_HISTORY {
                let residual = f64::from(self.output[position])
                    - prediction(&self.output, position, coefficients);
                energy += residual * residual;
            }
            let gain = f64::from(previous.gains_q16[last]) / 65_536.0;
            let amplitude =
                (3.0 * energy / (OUTPUT_HISTORY - order) as f64).sqrt() * attenuation * 8_388_608.0
                    / gain;
            for value in &mut excitation[..samples] {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = f64::from(*seed as i32) / 2_147_483_648.0;
                *value = (noise * amplitude)
                    .round()
                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                    as i32;
            }
        }
        self.reconstruct(frame, &parameters, &excitation, output, 0, samples)
    }

    pub(crate) fn frame(
        &mut self,
        frame: Frame,
        parameters: &Parameters,
        excitation_q23: &[i32],
        output: &mut [f32],
    ) -> Result<usize, Error> {
        self.frame_slice(
            frame,
            parameters,
            excitation_q23,
            0,
            frame.samples(),
            output,
        )
    }

    /// Reconstruct only the requested time interval from an already parsed
    /// internal frame. Skipped excitation does not advance synthesis history.
    pub(crate) fn frame_slice(
        &mut self,
        frame: Frame,
        parameters: &Parameters,
        excitation_q23: &[i32],
        origin: usize,
        samples: usize,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        if origin > frame.samples() || samples > frame.samples() - origin {
            return Err(Error::Argument);
        }
        self.reconstruct(frame, parameters, excitation_q23, output, origin, samples)
    }

    fn reconstruct(
        &mut self,
        frame: Frame,
        parameters: &Parameters,
        excitation_q23: &[i32],
        output: &mut [f32],
        origin: usize,
        samples: usize,
    ) -> Result<usize, Error> {
        let subframe = match frame.bandwidth {
            Bandwidth::Narrow => 40,
            Bandwidth::Medium => 60,
            Bandwidth::Wide => 80,
        };
        // Only voiced intervals spanning a subframe boundary reuse whitening.
        // Other calls keep their original path without touching cache scratch.
        if !parameters.voiced || samples <= subframe - origin % subframe {
            return if frame.bandwidth == Bandwidth::Wide {
                self.reconstruct_order::<16, false>(
                    frame,
                    parameters,
                    excitation_q23,
                    output,
                    origin,
                    samples,
                )
            } else {
                self.reconstruct_order::<10, false>(
                    frame,
                    parameters,
                    excitation_q23,
                    output,
                    origin,
                    samples,
                )
            };
        }
        if frame.bandwidth == Bandwidth::Wide {
            self.reconstruct_order::<16, true>(
                frame,
                parameters,
                excitation_q23,
                output,
                origin,
                samples,
            )
        } else {
            self.reconstruct_order::<10, true>(
                frame,
                parameters,
                excitation_q23,
                output,
                origin,
                samples,
            )
        }
    }

    fn reconstruct_order<const ORDER: usize, const REUSE_WHITENING: bool>(
        &mut self,
        frame: Frame,
        parameters: &Parameters,
        excitation_q23: &[i32],
        output: &mut [f32],
        origin: usize,
        samples: usize,
    ) -> Result<usize, Error> {
        if excitation_q23.len() < origin + samples {
            return Err(Error::Argument);
        }
        if output.len() < samples {
            return Err(Error::Output);
        }
        if samples == 0 {
            return Ok(0);
        }
        let n = match frame.bandwidth {
            Bandwidth::Narrow => 40,
            Bandwidth::Medium => 60,
            Bandwidth::Wide => 80,
        };
        let first_subframe = origin / n;
        let end_subframe = (origin + samples).div_ceil(n);
        if parameters.interpolation > 4
            || parameters.gains_q16[first_subframe..end_subframe]
                .iter()
                .any(|&gain| !(81_920..=1_686_110_208).contains(&gain))
            || (parameters.voiced
                && (parameters.pitch_lags[first_subframe..end_subframe]
                    .iter()
                    .any(|&lag| !(n * 2 / 5..=n * 18 / 5).contains(&lag))
                    || parameters.ltp_scale_q14 <= 0))
        {
            return Err(Error::Argument);
        }
        // Q12 and Q14 scales are exact powers of two. Prepare each predictor
        // once instead of multiplying by its scale at every history tap.
        let coefficient_sets = parameters
            .lpc_q12
            .map(|values| values.map(|value| f32::from(value) / 4096.0));

        let mut clamped = [0.0; OUTPUT_HISTORY + MAX_SAMPLES];
        clamped[..OUTPUT_HISTORY].copy_from_slice(&self.output);
        let mut lpc = [0.0; LPC_HISTORY + MAX_SAMPLES];
        lpc[..LPC_HISTORY].copy_from_slice(&self.lpc);
        let mut residual = [0.0; OUTPUT_HISTORY + MAX_SAMPLES];
        let mut whitened = [0.0; OUTPUT_HISTORY + MAX_SAMPLES];
        let mut whitened_begin = OUTPUT_HISTORY;
        let mut whitened_end = OUTPUT_HISTORY;
        let mut whitened_coefficients = usize::MAX;

        let mut first = 0;
        while first < samples {
            let absolute = origin + first;
            let subframe = absolute / n;
            let width = (n - absolute % n).min(samples - first);
            let j = OUTPUT_HISTORY + first;
            let gain = parameters.gains_q16[subframe] as f32 / 65_536.0;
            let coefficient_index = usize::from(subframe >= 2 || parameters.interpolation == 4);
            let coefficients = &coefficient_sets[coefficient_index];
            let ltp = parameters.ltp_q14[subframe].map(|value| f32::from(value) / 16_384.0);

            if parameters.voiced {
                let lag = parameters.pitch_lags[subframe];
                let interpolate = parameters.interpolation < 4 && subframe >= 2;
                let out_end = OUTPUT_HISTORY
                    + if interpolate {
                        (2 * n).saturating_sub(origin)
                    } else {
                        0
                    };
                let scale = if interpolate {
                    1.0
                } else {
                    f32::from(parameters.ltp_scale_q14) / 16_384.0
                };
                if REUSE_WHITENING {
                    // This clamped prefix is immutable within one LPC set.
                    // A new set also moves the clamped/unclamped boundary.
                    if whitened_coefficients != coefficient_index || whitened_end != out_end {
                        whitened_begin = out_end;
                        whitened_end = out_end;
                        whitened_coefficients = coefficient_index;
                    }
                    let begin = j - lag - 2;
                    for i in begin..whitened_begin {
                        whitened[i] =
                            clamped[i] - prediction_scaled::<ORDER>(&clamped, i, coefficients);
                    }
                    whitened_begin = whitened_begin.min(begin);
                }
                // Only history predating the active LPC coefficient set uses
                // the clamped output and the loss-robust LTP scale.
                for i in j - lag - 2..out_end {
                    let numerator = if REUSE_WHITENING {
                        whitened[i]
                    } else {
                        clamped[i] - prediction_scaled::<ORDER>(&clamped, i, coefficients)
                    };
                    residual[i] = numerator.clamp(-1.0, 1.0) * scale / gain;
                }
                // Within the current coefficient set, rewhitening uses the
                // unclamped LPC output, including its prior-frame prefix.
                // The five-tap predictor can read no earlier than j-lag-2.
                // Rebuild only that history window; every later subframe
                // rebuilds its own window with its active coefficients/gain.
                let first_needed = out_end.max(j - lag - 2);
                for (i, destination) in residual.iter_mut().enumerate().take(j).skip(first_needed) {
                    let position = LPC_HISTORY + i - OUTPUT_HISTORY;
                    let numerator = if REUSE_WHITENING {
                        whitened[i]
                    } else {
                        lpc[position] - prediction_scaled::<ORDER>(&lpc, position, coefficients)
                    };
                    *destination = numerator / gain;
                }
            }

            for offset in 0..width {
                let sample = first + offset;
                let position = j + offset;
                let mut value = excitation_q23[origin + sample] as f32 / 8_388_608.0;
                if parameters.voiced {
                    let lag = parameters.pitch_lags[subframe];
                    for (tap, &coefficient) in ltp.iter().enumerate() {
                        value += residual[position - lag + 2 - tap] * coefficient;
                    }
                }
                residual[position] = value;
                let lpc_position = LPC_HISTORY + sample;
                let predicted = prediction_scaled::<ORDER>(&lpc, lpc_position, coefficients);
                let reconstructed = gain * value + predicted;
                if REUSE_WHITENING && parameters.voiced {
                    // Rewhitening this sample under the same LPC set repeats
                    // precisely this subtraction; each later gain is still
                    // applied independently at its original subframe boundary.
                    whitened[position] = reconstructed - predicted;
                }
                lpc[lpc_position] = reconstructed;
                clamped[position] = reconstructed.clamp(-1.0, 1.0);
                output[sample] = clamped[position];
            }
            first += width;
        }
        self.output
            .copy_from_slice(&clamped[samples..samples + OUTPUT_HISTORY]);
        self.lpc
            .copy_from_slice(&lpc[samples..samples + LPC_HISTORY]);
        Ok(samples)
    }
}

#[inline]
fn prediction_scaled<const ORDER: usize>(
    history: &[f32],
    position: usize,
    coefficients: &[f32; 16],
) -> f32 {
    let past = &history[position - ORDER..position];
    // Independent accumulators shorten the recurrent dependency chain. The
    // Q12 coefficients are exactly representable in f32; accumulation retains
    // f32 precision, matching the PCM and the stored synthesis history.
    let mut partial = [0.0; 4];
    for (tap, &coefficient) in coefficients[..ORDER].iter().enumerate() {
        partial[tap % 4] += past[ORDER - tap - 1] * coefficient;
    }
    partial[0] + partial[1] + partial[2] + partial[3]
}

fn prediction(history: &[f32], position: usize, coefficients: &[i16]) -> f64 {
    let mut result = 0.0;
    for (tap, &coefficient) in coefficients.iter().enumerate() {
        result += f64::from(history[position - tap - 1]) * f64::from(coefficient) / 4096.0;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::silk::FrameDuration;

    fn geometry() -> Frame {
        Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        }
    }

    fn parameters() -> Parameters {
        Parameters {
            gains_q16: [81_920; 4],
            lpc_q12: [[0; 16]; 2],
            interpolation: 4,
            voiced: false,
            pitch_lags: [16; 4],
            ltp_q14: [[0; 5]; 4],
            ltp_scale_q14: 15_565,
        }
    }

    #[test]
    fn overlapping_rewhitening_preserves_every_pcm_and_history_bit() {
        let mut random = 0x6716_19abu32;
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
                let frame = Frame {
                    bandwidth,
                    duration,
                };
                let subframe = frame.samples()
                    / if duration == FrameDuration::Ms10 {
                        2
                    } else {
                        4
                    };
                let mut actual = Synthesis::new();
                let mut expected = Synthesis::new();
                for iteration in 0..60 {
                    let mut parameters = parameters();
                    parameters.voiced = iteration % 9 != 0;
                    parameters.interpolation = (iteration % 5) as u8;
                    for (set, coefficients) in parameters.lpc_q12.iter_mut().enumerate() {
                        coefficients[0] = 2_000 - set as i16 * 700;
                        for coefficient in &mut coefficients[1..] {
                            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            *coefficient = ((random >> 24) as i16 - 128) / 2;
                        }
                    }
                    for index in 0..4 {
                        parameters.gains_q16[index] =
                            [81_920, 98_304, 131_072, 196_608][(iteration + index) % 4];
                        parameters.pitch_lags[index] = if (iteration + index).is_multiple_of(2) {
                            subframe * 2 / 5 + index
                        } else {
                            subframe * 18 / 5 - index
                        };
                        parameters.ltp_q14[index] = [200, -400, 5_000, 300, -100];
                    }
                    let mut excitation = [0; MAX_SAMPLES];
                    for value in &mut excitation[..frame.samples()] {
                        random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        *value = (random as i32) >> 7;
                    }
                    let origin = match iteration % 6 {
                        0 | 1 => 0,
                        2 => subframe - 1,
                        3 => subframe,
                        4 => frame.samples() / 2 - 1,
                        _ => frame.samples() - subframe + 1,
                    };
                    let samples = frame.samples() - origin;
                    let mut actual_output = [123.0; MAX_SAMPLES + 1];
                    let mut expected_output = actual_output;
                    actual
                        .frame_slice(
                            frame,
                            &parameters,
                            &excitation,
                            origin,
                            samples,
                            &mut actual_output,
                        )
                        .unwrap();
                    if bandwidth == Bandwidth::Wide {
                        expected.reconstruct_order::<16, false>(
                            frame,
                            &parameters,
                            &excitation,
                            &mut expected_output,
                            origin,
                            samples,
                        )
                    } else {
                        expected.reconstruct_order::<10, false>(
                            frame,
                            &parameters,
                            &excitation,
                            &mut expected_output,
                            origin,
                            samples,
                        )
                    }
                    .unwrap();
                    assert_eq!(
                        actual_output.map(f32::to_bits),
                        expected_output.map(f32::to_bits),
                        "{bandwidth:?} {duration:?} iteration={iteration} origin={origin}"
                    );
                    assert_eq!(
                        actual.output.map(f32::to_bits),
                        expected.output.map(f32::to_bits)
                    );
                    assert_eq!(actual.lpc.map(f32::to_bits), expected.lpc.map(f32::to_bits));
                }
            }
        }
    }

    #[test]
    fn frame_slice_uses_original_subframe_and_excitation_positions() {
        let frame = Frame {
            bandwidth: Bandwidth::Wide,
            duration: FrameDuration::Ms20,
        };
        let mut parameters = parameters();
        parameters.interpolation = 0;
        parameters.lpc_q12[0][0] = 2048;
        parameters.lpc_q12[1][0] = 1024;
        let mut excitation = [0; 320];
        excitation[0] = 8_388_608;
        excitation[159] = 1_048_576;
        let mut synthesis = Synthesis::new();
        let mut output = [123.0; 2];
        assert_eq!(
            synthesis
                .frame_slice(frame, &parameters, &excitation, 159, 2, &mut output)
                .unwrap(),
            2
        );
        assert_eq!(output, [0.15625, 0.0390625]);
        assert_eq!(synthesis.lpc[LPC_HISTORY - 1], 0.0390625);

        let before = (synthesis.output, synthesis.lpc);
        assert_eq!(
            synthesis.frame_slice(frame, &parameters, &excitation, 321, 0, &mut output),
            Err(Error::Argument)
        );
        assert_eq!(
            synthesis.frame_slice(frame, &parameters, &excitation, 320, 1, &mut output),
            Err(Error::Argument)
        );
        assert_eq!(
            synthesis.frame_slice(frame, &parameters, &excitation, 0, 0, &mut []),
            Ok(0)
        );
        assert_eq!((synthesis.output, synthesis.lpc), before);
        assert_eq!(output, [0.15625, 0.0390625]);
    }

    #[test]
    fn lpc_feedback_uses_unclamped_history_including_frame_boundary() {
        let frame = geometry();
        let mut parameters = parameters();
        parameters.lpc_q12[1][0] = 2048;
        let mut synthesis = Synthesis::new();
        let mut excitation = [0; 80];
        excitation[79] = 16_777_216;
        let mut output = [0.0; 80];
        synthesis
            .frame(frame, &parameters, &excitation, &mut output)
            .unwrap();
        assert_eq!(output[79], 1.0);
        synthesis
            .frame(frame, &parameters, &[0; 80], &mut output)
            .unwrap();
        assert_eq!(&output[..4], &[1.0, 0.625, 0.3125, 0.15625]);
        synthesis.reset();
        synthesis
            .frame(frame, &parameters, &[0; 80], &mut output)
            .unwrap();
        assert_eq!(output, [0.0; 80]);
    }

    #[test]
    fn zero_prediction_is_scaled_excitation_for_both_signal_classes() {
        let frame = geometry();
        for voiced in [false, true] {
            let mut parameters = parameters();
            parameters.voiced = voiced;
            let mut excitation = [0; 80];
            excitation[0] = 8_388_608;
            excitation[1] = -4_194_304;
            let mut output = [123.0; 81];
            let samples = Synthesis::new()
                .frame(frame, &parameters, &excitation, &mut output)
                .unwrap();
            assert_eq!(samples, 80);
            assert_eq!(&output[..3], &[1.0, -0.625, 0.0]);
            assert_eq!(output[80], 123.0);
        }
    }

    #[test]
    fn nonzero_ltp_repeats_pitch_impulse_with_zero_lpc() {
        let frame = geometry();
        let mut parameters = parameters();
        parameters.voiced = true;
        for coefficients in &mut parameters.ltp_q14 {
            coefficients[2] = 8192;
        }
        let mut excitation = [0; 80];
        excitation[0] = 4_194_304;
        let mut output = [0.0; 80];
        Synthesis::new()
            .frame(frame, &parameters, &excitation, &mut output)
            .unwrap();
        for (index, &value) in output.iter().enumerate() {
            let expected = if index % 16 == 0 {
                0.625 * 0.5f32.powi((index / 16) as i32)
            } else {
                0.0
            };
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn partial_concealment_advances_only_the_requested_predictor_samples() {
        let frame = geometry();
        let mut parameters = parameters();
        parameters.voiced = true;
        parameters.lpc_q12[1][0] = 2048;
        let mut excitation = [0; 80];
        excitation[79] = 4_194_304;
        let mut synthesis = Synthesis::new();
        let mut output = [0.0; 80];
        synthesis
            .frame(frame, &parameters, &excitation, &mut output)
            .unwrap();
        assert_eq!(output[79], 0.625);
        let mut seed = 0;
        assert_eq!(
            synthesis
                .conceal_samples(frame, 20, &parameters, 0.9, &mut seed, &mut output)
                .unwrap(),
            20
        );
        let pole = (2048.0f64 * 0.98).round() / 4096.0;
        for (index, &sample) in output[..20].iter().enumerate() {
            assert!((f64::from(sample) - 0.625 * pole.powi(index as i32 + 1)).abs() < 1e-8);
        }
        // Preserve every recurrent f32 rounding, including the history
        // handed to the next frame. The closed-form f64 result above checks
        // the defining decay separately.
        let mut expected_history = 0.625f32;
        for _ in 0..20 {
            expected_history *= pole as f32;
        }
        assert_eq!(synthesis.lpc[LPC_HISTORY - 1], expected_history);
        assert_eq!(seed, 0, "voiced extrapolation consumes no noise generator");
    }

    #[test]
    fn malformed_geometry_and_short_output_are_transactional() {
        let frame = geometry();
        let mut synthesis = Synthesis::new();
        let mut parameters = parameters();
        let mut output = [123.0; 80];
        parameters.voiced = true;
        parameters.pitch_lags[0] = 0;
        assert_eq!(
            synthesis.frame(frame, &parameters, &[0; 80], &mut output),
            Err(Error::Argument)
        );
        assert_eq!(output, [123.0; 80]);
        parameters.pitch_lags[0] = 16;
        assert_eq!(
            synthesis.frame(frame, &parameters, &[0; 79], &mut output),
            Err(Error::Argument)
        );
        assert_eq!(
            synthesis.frame(frame, &parameters, &[0; 80], &mut output[..79]),
            Err(Error::Output)
        );
        assert_eq!(output, [123.0; 80]);
        assert_eq!(synthesis.output, [0.0; OUTPUT_HISTORY]);
        assert_eq!(synthesis.lpc, [0.0; LPC_HISTORY]);
    }
}
