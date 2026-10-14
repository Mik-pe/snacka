// SPDX-License-Identifier: MIT
//! Owned SILK short-term prediction analysis and spectral quantization.

use super::{Bandwidth, NlsfIndices, nlsf_to_lpc, reconstruct_nlsf, stabilize_nlsf};

const MAX_ORDER: usize = 16;
const ROOT_GRID: usize = 512;

const fn grid_cosines() -> [f64; ROOT_GRID + 1] {
    let mut cosines = [0.0; ROOT_GRID + 1];
    let mut index = 0;
    while index <= ROOT_GRID {
        let angle = index as f64 * std::f64::consts::PI / ROOT_GRID as f64;
        let squared = angle * angle;
        let mut term = 1.0;
        let mut value = 1.0;
        let mut degree = 1;
        while degree < 16 {
            term *= -squared / ((2 * degree - 1) * 2 * degree) as f64;
            value += term;
            degree += 1;
        }
        cosines[index] = value;
        index += 1;
    }
    cosines
}
const GRID_COSINES: [f64; ROOT_GRID + 1] = grid_cosines();

#[derive(Clone, Copy)]
pub(super) struct Spectrum {
    pub(super) indices: NlsfIndices,
    pub(super) lpc_q12: [i16; MAX_ORDER],
    pub(super) order: usize,
}

/// Analyze one channel's native-rate PCM without allocating.
pub(super) fn spectrum(samples: &[f32], bandwidth: Bandwidth) -> Spectrum {
    let order = if bandwidth == Bandwidth::Wide { 16 } else { 10 };
    let correlations = autocorrelation(samples, order);
    let mut predictor = levinson(&correlations, order);
    let mut chirp = 0.95;
    for coefficient in &mut predictor[..order] {
        *coefficient *= chirp;
        chirp *= 0.95;
    }
    quantized_spectrum(&predictor[..order], bandwidth)
}

/// Contract the transmitted predictor's poles before a budget-limited retry.
/// Multiplying lag-k coefficients by chirp^k broadens the spectral envelope;
/// it keeps the default analysis untouched and avoids discarding every formant
/// when a narrow predictor would leave too little excitation budget.
pub(super) fn broaden(spectrum: Spectrum, bandwidth: Bandwidth, chirp: f64) -> Spectrum {
    debug_assert!(chirp.is_finite() && (0.0..1.0).contains(&chirp));
    let order = if bandwidth == Bandwidth::Wide { 16 } else { 10 };
    debug_assert_eq!(spectrum.order, order);
    let mut predictor = [0.0; MAX_ORDER];
    let mut power = chirp;
    for (destination, &coefficient) in predictor[..order]
        .iter_mut()
        .zip(&spectrum.lpc_q12[..order])
    {
        *destination = f64::from(coefficient) / 4096.0 * power;
        power *= chirp;
    }
    quantized_spectrum(&predictor[..order], bandwidth)
}

fn quantized_spectrum(predictor: &[f64], bandwidth: Bandwidth) -> Spectrum {
    let order = predictor.len();
    let mut target = [0i16; MAX_ORDER];
    if !roots(predictor, &mut target[..order]) {
        for (index, frequency) in target[..order].iter_mut().enumerate() {
            *frequency = ((index + 1) * 32_768 / (order + 1)) as i16;
        }
    }
    stabilize_nlsf(&mut target[..order], bandwidth).unwrap();
    let indices = quantize(&target[..order], bandwidth);
    let mut quantized = [0; MAX_ORDER];
    reconstruct_nlsf(indices, bandwidth, &mut quantized[..order]).unwrap();
    let mut lpc_q12 = [0; MAX_ORDER];
    nlsf_to_lpc(&quantized[..order], &mut lpc_q12[..order]).unwrap();
    Spectrum {
        indices,
        lpc_q12,
        order,
    }
}

fn autocorrelation(samples: &[f32], order: usize) -> [f64; MAX_ORDER + 1] {
    let mut correlation = [0.0; MAX_ORDER + 1];
    // A weak quadratic lag window keeps near-sinusoidal inputs numerically
    // well conditioned while retaining the signal's formant locations.
    for (lag, output) in correlation[..=order].iter_mut().enumerate() {
        let mut value = 0.0;
        for i in lag..samples.len() {
            value += f64::from(samples[i]) * f64::from(samples[i - lag]);
        }
        *output = value * (-0.0003 * (lag * lag) as f64).exp();
    }
    correlation[0] = correlation[0] * 1.001 + 1e-9;
    correlation
}

fn levinson(correlation: &[f64; MAX_ORDER + 1], order: usize) -> [f64; MAX_ORDER] {
    let mut coefficients = [0.0; MAX_ORDER];
    let mut error = correlation[0];
    for length in 1..=order {
        let mut residual = correlation[length];
        for (tap, &coefficient) in coefficients[..length - 1].iter().enumerate() {
            residual -= coefficient * correlation[length - tap - 1];
        }
        let reflection = (residual / error.max(1e-12)).clamp(-0.985, 0.985);
        let previous = coefficients;
        for tap in 0..length - 1 {
            coefficients[tap] = previous[tap] - reflection * previous[length - tap - 2];
        }
        coefficients[length - 1] = reflection;
        error *= 1.0 - reflection * reflection;
    }
    coefficients
}

fn polynomial(coefficients: &[f64], cosine: f64, difference: bool) -> f64 {
    // Remove the trivial z=+/-1 roots of the antisymmetric/symmetric LPC
    // polynomials. The remaining half-order polynomial uses the half-angle
    // Chebyshev recurrence, requiring one cosine for each root-search point.
    let half = coefficients.len() / 2;
    let doubled = 2.0 * cosine;
    let mut basis_previous = 1.0;
    let mut basis = doubled + if difference { 1.0 } else { -1.0 };
    let pair = |index: usize| {
        coefficients[index]
            + if difference { -1.0 } else { 1.0 } * coefficients[coefficients.len() - index - 1]
    };
    let mut sum = -pair(half - 1);
    if half > 1 {
        sum -= pair(half - 2) * basis;
    }
    for degree in 2..=half {
        let next = doubled * basis - basis_previous;
        basis_previous = basis;
        basis = next;
        if degree == half {
            sum += basis;
        } else {
            sum -= pair(half - degree - 1) * basis;
        }
    }
    sum
}

fn roots(coefficients: &[f64], output: &mut [i16]) -> bool {
    let step = std::f64::consts::PI / ROOT_GRID as f64;
    let mut angle = 0.0f64;
    for (index, frequency) in output.iter_mut().enumerate() {
        let difference = index & 1 != 0;
        let mut low = angle + 1e-10;
        let mut before = polynomial(coefficients, low.cos(), difference);
        let mut grid = (low / step) as usize + 1;
        let mut found = false;
        while grid <= ROOT_GRID {
            let mut high = grid as f64 * step;
            let after = polynomial(coefficients, GRID_COSINES[grid], difference);
            if before * after <= 0.0 {
                // Bisection retains monotone LSF ordering without a derivative
                // singularity close to the ends of the cosine interval.
                for _ in 0..13 {
                    let middle = 0.5 * (low + high);
                    let value = polynomial(coefficients, middle.cos(), difference);
                    if before * value <= 0.0 {
                        high = middle;
                    } else {
                        low = middle;
                        before = value;
                    }
                }
                angle = 0.5 * (low + high);
                *frequency = (angle * (32_768.0 / std::f64::consts::PI))
                    .round()
                    .clamp(0.0, 32767.0) as i16;
                found = true;
                break;
            }
            low = high;
            before = after;
            grid += 1;
        }
        if !found {
            return false;
        }
    }
    true
}

fn spectral_cost(target: &[i16], candidate: &[i16], weights: &[f64]) -> f64 {
    target
        .iter()
        .zip(candidate)
        .zip(weights)
        .map(|((&target, &actual), &weight)| {
            let difference = f64::from(i32::from(target) - i32::from(actual));
            difference * difference * weight
        })
        .sum()
}

fn quantize(target: &[i16], bandwidth: Bandwidth) -> NlsfIndices {
    let order = target.len();
    let mut weights = [0.0; MAX_ORDER];
    for index in 0..order {
        let before = if index == 0 {
            0
        } else {
            i32::from(target[index - 1])
        };
        let after = if index + 1 == order {
            32_768
        } else {
            i32::from(target[index + 1])
        };
        let gap = (i32::from(target[index]) - before)
            .min(after - i32::from(target[index]))
            .max(32);
        weights[index] = 1.0 / f64::from(gap * gap);
    }
    let mut best = NlsfIndices {
        first: 0,
        residual: [0; MAX_ORDER],
        interpolation: 4,
    };
    let mut candidate = [0; MAX_ORDER];
    let mut best_cost = f64::INFINITY;
    // First-stage vectors are the 32 mandatory format entries. Reconstructing
    // them shares the exact decoder's scaling and stabilization rules.
    for first in 0..32 {
        let indices = NlsfIndices {
            first,
            residual: [0; MAX_ORDER],
            interpolation: 4,
        };
        reconstruct_nlsf(indices, bandwidth, &mut candidate[..order]).unwrap();
        let cost = spectral_cost(target, &candidate[..order], &weights[..order]);
        if cost < best_cost {
            best = indices;
            best_cost = cost;
        }
    }
    // The triangular residual predictor can be inverted backwards using the
    // exact decoder scale. A three-candidate scalar step avoids searching the
    // full extension alphabet independently at every component.
    let parameters = super::lpc::nlsf_quantization_parameters(best.first, bandwidth).unwrap();
    let mut reconstructed = [0i32; MAX_ORDER];
    for index in (0..order).rev() {
        let predicted = if index + 1 < order {
            (reconstructed[index + 1] * i32::from(parameters.prediction_q8[index])) >> 8
        } else {
            0
        };
        let wanted = (i32::from(target[index]) - i32::from(parameters.centers_q15[index]))
            * i32::from(parameters.weights_q9[index])
            / 16_384;
        let scale = f64::from(parameters.step_q16) * (1024.0 / 65_536.0);
        let center = ((f64::from(wanted - predicted) / scale).round() as i32).clamp(-10, 10);
        let mut smallest = i64::MAX;
        for value in center - 1..=center + 1 {
            let value = value.clamp(-10, 10);
            let residual =
                predicted + (((value * 1024 - value.signum() * 102) * parameters.step_q16) >> 16);
            let distance = i64::from(wanted - residual).pow(2);
            if distance < smallest {
                smallest = distance;
                best.residual[index] = value as i8;
                reconstructed[index] = residual;
            }
        }
    }
    reconstruct_nlsf(best, bandwidth, &mut candidate[..order]).unwrap();
    best_cost = spectral_cost(target, &candidate[..order], &weights[..order]);
    // One neighboring pass accounts for stabilization and the predictor's
    // coupling between components, using the canonical reconstruction once
    // per neighbor instead of twenty-one times per component.
    for index in (0..order).rev() {
        let center = best.residual[index];
        let mut selected = center;
        for residual in center.saturating_sub(1)..=center.saturating_add(1) {
            let mut trial = best;
            trial.residual[index] = residual.clamp(-10, 10);
            reconstruct_nlsf(trial, bandwidth, &mut candidate[..order]).unwrap();
            let cost = spectral_cost(target, &candidate[..order], &weights[..order]);
            if cost < best_cost {
                selected = trial.residual[index];
                best_cost = cost;
            }
        }
        best.residual[index] = selected;
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_angle_polynomials_match_direct_complex_lpc_equations() {
        for order in [10, 16] {
            let coefficients: [f64; MAX_ORDER] = std::array::from_fn(|index| {
                if index < order {
                    0.05 * ((index + 1) as f64).sin()
                } else {
                    0.0
                }
            });
            for grid in 1..256 {
                let angle = grid as f64 * std::f64::consts::PI / 256.0;
                for difference in [false, true] {
                    let half = (order + 1) as f64 * 0.5;
                    let function = |value: f64| if difference { value.sin() } else { value.cos() };
                    let direct = function(half * angle)
                        - coefficients[..order]
                            .iter()
                            .enumerate()
                            .map(|(index, coefficient)| {
                                coefficient * function((half - index as f64 - 1.0) * angle)
                            })
                            .sum::<f64>();
                    let denominator = function(angle * 0.5);
                    let actual = polynomial(&coefficients[..order], angle.cos(), difference);
                    assert!((actual * denominator - direct).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn white_predictor_roots_are_uniform_and_round_trip_q12_predictors() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Wide] {
            let order = if bandwidth == Bandwidth::Wide { 16 } else { 10 };
            let mut frequencies = [0; MAX_ORDER];
            assert!(roots(&[0.0; MAX_ORDER][..order], &mut frequencies[..order]));
            for (index, &actual) in frequencies[..order].iter().enumerate() {
                let expected = ((index + 1) as f64 * 32_768.0 / (order + 1) as f64).round() as i16;
                assert!((i32::from(actual) - i32::from(expected)).abs() <= 1);
            }
            let mut predictor = [0; MAX_ORDER];
            nlsf_to_lpc(&frequencies[..order], &mut predictor[..order]).unwrap();
            let floating: [f64; MAX_ORDER] =
                std::array::from_fn(|index| f64::from(predictor[index]) / 4096.0);
            let mut restored = [0; MAX_ORDER];
            assert!(roots(&floating[..order], &mut restored[..order]));
            for (&expected, &actual) in frequencies[..order].iter().zip(&restored[..order]) {
                assert!((i32::from(actual) - i32::from(expected)).abs() <= 4);
            }
        }
    }

    #[test]
    fn spectral_analysis_is_stable_for_silence_tones_and_white_noise() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let mut samples = [0.0; 320];
            let mut random = 0x67_16u32;
            for kind in 0..3 {
                for (index, sample) in samples.iter_mut().enumerate() {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    *sample = match kind {
                        0 => 0.0,
                        1 => 0.4 * (index as f32 * 0.14).sin(),
                        _ => 0.2 * (random as i32 as f32 / i32::MAX as f32),
                    };
                }
                let spectrum = spectrum(&samples, bandwidth);
                let inverse =
                    super::super::inverse_prediction_gain(&spectrum.lpc_q12[..spectrum.order])
                        .unwrap();
                assert!(inverse > 0);
                assert!(
                    spectrum.indices.residual[..spectrum.order]
                        .iter()
                        .all(|index| (-10..=10).contains(index))
                );
            }
        }
    }

    #[test]
    fn budget_retry_broadens_tonal_predictors_without_losing_stability() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for frequency in [0.08, 0.14, 0.24, 0.5] {
                let samples: [f32; 320] =
                    std::array::from_fn(|index| 0.4 * (index as f32 * frequency).sin());
                let original = spectrum(&samples, bandwidth);
                let retry = broaden(original, bandwidth, 0.8);
                let gain = |model: Spectrum| {
                    super::super::inverse_prediction_gain(&model.lpc_q12[..model.order]).unwrap()
                };
                // Broadening reduces narrow resonant prediction: the inverse
                // gain rises while both transmitted quantized filters remain
                // stable. This is the property required by budget retries.
                assert!(gain(original) > 0);
                assert!(gain(retry) > gain(original));
            }
        }
    }
}
