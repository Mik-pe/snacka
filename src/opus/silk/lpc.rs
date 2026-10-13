// SPDX-License-Identifier: MIT AND BSD-3-Clause
//! SILK spectral prediction coefficients from RFC 6716 section 4.2.7.5.
//!
//! The integer recurrences here are independently written from the equations
//! and numerical tables in the specification. Intermediate products use i64
//! to retain the specified rounding without relying on signed overflow. Tables
//! 14-28 provide format constants; no codec implementation is a dependency.

use super::{Bandwidth, Error, Frame, FrameDuration, SignalType};
use crate::entropy::{Decoder, Model};

/// Maximum predictor order, used by wideband SILK.
pub const MAX_ORDER: usize = 16;

fn order(bandwidth: Bandwidth) -> usize {
    if bandwidth == Bandwidth::Wide { 16 } else { 10 }
}

/// Decoded spectral indices for one internal SILK frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NlsfIndices {
    /// First-stage vector index, from 0 through 31.
    pub first: u8,
    /// Second-stage residual indices, from -10 through 10.
    pub residual: [i8; MAX_ORDER],
    /// First-half interpolation weight in Q2. Ten-ms frames always use 4.
    pub interpolation: u8,
}

/// Decode spectral symbols in bitstream order, without allocating.
pub fn decode_nlsf_indices(
    decoder: &mut Decoder<'_>,
    frame: Frame,
    signal: SignalType,
) -> NlsfIndices {
    let wide = usize::from(frame.bandwidth == Bandwidth::Wide);
    let voiced = usize::from(signal == SignalType::Voiced);
    let first = decoder.symbol(STAGE_ONE[wide][voiced]);
    let selection: &[u8] = if wide != 0 {
        &WB_SELECT[first]
    } else {
        &NB_SELECT[first]
    };
    let mut residual = [0; MAX_ORDER];
    for (destination, &model) in residual.iter_mut().zip(selection) {
        let index = decoder.symbol(STAGE_TWO[wide * 8 + usize::from(model)]) as i8 - 4;
        *destination = if index.abs() == 4 {
            index + index.signum() * decoder.symbol(EXTENSION) as i8
        } else {
            index
        };
    }
    let interpolation = if frame.duration == FrameDuration::Ms20 {
        decoder.symbol(INTERPOLATION) as u8
    } else {
        4
    };
    NlsfIndices {
        first: first as u8,
        residual,
        interpolation,
    }
}

/// Reconstruct and stabilize Q15 normalized spectral frequencies.
///
/// Output contains exactly 10 NB/MB or 16 WB coefficients. Invalid arguments
/// leave output untouched. Trailing unused residual entries are ignored.
pub fn reconstruct_nlsf(
    indices: NlsfIndices,
    bandwidth: Bandwidth,
    output: &mut [i16],
) -> Result<(), Error> {
    let length = order(bandwidth);
    if output.len() != length {
        return Err(Error::Output);
    }
    if indices.first > 31
        || indices.interpolation > 4
        || indices.residual[..length]
            .iter()
            .any(|index| !(-10..=10).contains(index))
    {
        return Err(Error::Argument);
    }
    let first = usize::from(indices.first);
    let wide = bandwidth == Bandwidth::Wide;
    let codebook: &[u8] = if wide {
        &WB_CODEBOOK[first]
    } else {
        &NB_CODEBOOK[first]
    };
    let prediction: &[u8] = if wide {
        &WB_PREDICT[first]
    } else {
        &NB_PREDICT[first]
    };
    let weights: &[[u8; 2]] = if wide {
        &WB_PRED_WEIGHTS
    } else {
        &NB_PRED_WEIGHTS
    };
    let spectral_weights: &[u16] = if wide {
        &WB_LSF_WEIGHTS[first]
    } else {
        &NB_LSF_WEIGHTS[first]
    };
    let mut residual = [0i32; MAX_ORDER];
    let step = if wide { 9830 } else { 11796 };
    for k in (0..length).rev() {
        let index = i32::from(indices.residual[k]);
        let predicted = if k + 1 < length {
            (residual[k + 1] * i32::from(weights[k][usize::from(prediction[k])])) >> 8
        } else {
            0
        };
        residual[k] = predicted + (((index * 1024 - index.signum() * 102) * step) >> 16);
    }
    for k in 0..length {
        let center = i32::from(codebook[k]);
        output[k] = ((center << 7) + (residual[k] << 14) / i32::from(spectral_weights[k]))
            .clamp(0, 32767) as i16;
    }
    stabilize_nlsf(output, bandwidth)?;
    Ok(())
}

const fn spectral_weights<const N: usize>(codebook: &[[u8; N]; 32]) -> [[u16; N]; 32] {
    let mut weights = [[0; N]; 32];
    let mut vector = 0;
    while vector < 32 {
        let mut k = 0;
        while k < N {
            let left = if k == 0 {
                0
            } else {
                codebook[vector][k - 1] as u32
            };
            let right = if k + 1 == N {
                256
            } else {
                codebook[vector][k + 1] as u32
            };
            let center = codebook[vector][k] as u32;
            let squared = (1024 / (center - left) + 1024 / (right - center)) << 16;
            let bits = 32 - squared.leading_zeros();
            let fraction = (squared >> (bits - 8)) & 127;
            let root = (if bits & 1 != 0 { 32768u32 } else { 46214u32 }) >> ((32 - bits) >> 1);
            weights[vector][k] = (root + ((213 * fraction * root) >> 16)) as u16;
            k += 1;
        }
        vector += 1;
    }
    weights
}

const NB_LSF_WEIGHTS: [[u16; 10]; 32] = spectral_weights(&NB_CODEBOOK);
const WB_LSF_WEIGHTS: [[u16; 16]; 32] = spectral_weights(&WB_CODEBOOK);

fn spacings(bandwidth: Bandwidth) -> &'static [i16] {
    if bandwidth == Bandwidth::Wide {
        &WB_MIN_SPACING
    } else {
        &NB_MIN_SPACING
    }
}

/// Enforce the minimum spacings of RFC 6716 table 25, without allocating.
pub fn stabilize_nlsf(nlsf: &mut [i16], bandwidth: Bandwidth) -> Result<(), Error> {
    let length = order(bandwidth);
    if nlsf.len() != length {
        return Err(Error::Output);
    }
    let delta = spacings(bandwidth);
    for _ in 0..20 {
        let mut minimum = i32::from(nlsf[0]) - i32::from(delta[0]);
        let mut index = 0;
        for k in 1..=length {
            let next = if k == length {
                32768
            } else {
                i32::from(nlsf[k])
            };
            let margin = next - i32::from(nlsf[k - 1]) - i32::from(delta[k]);
            if margin < minimum {
                minimum = margin;
                index = k;
            }
        }
        if minimum >= 0 {
            return Ok(());
        }
        if index == 0 {
            nlsf[0] = delta[0];
        } else if index == length {
            nlsf[length - 1] = (32768 - i32::from(delta[length])) as i16;
        } else {
            let half = i32::from(delta[index]) >> 1;
            let low = half
                + delta[..index]
                    .iter()
                    .map(|&value| i32::from(value))
                    .sum::<i32>();
            let high = 32768
                - half
                - delta[index + 1..]
                    .iter()
                    .map(|&value| i32::from(value))
                    .sum::<i32>();
            let center =
                ((i32::from(nlsf[index - 1]) + i32::from(nlsf[index]) + 1) >> 1).clamp(low, high);
            nlsf[index - 1] = (center - half) as i16;
            nlsf[index] = (center - half + i32::from(delta[index])) as i16;
        }
    }
    nlsf.sort_unstable();
    // The forward pass may temporarily exceed the signed Q15 range. Keep
    // those intermediates wide until the backward pass applies the upper edge.
    let mut adjusted = [0i32; MAX_ORDER];
    let mut previous = 0;
    for ((destination, &coefficient), &distance) in adjusted.iter_mut().zip(nlsf.iter()).zip(delta)
    {
        previous = i32::from(coefficient).max(previous + i32::from(distance));
        *destination = previous;
    }
    let mut next = 32768;
    for ((destination, &coefficient), &distance) in nlsf
        .iter_mut()
        .rev()
        .zip(adjusted[..length].iter().rev())
        .zip(delta[1..].iter().rev())
    {
        next = coefficient.min(next - i32::from(distance));
        *destination = next as i16;
    }
    Ok(())
}

/// Interpolate spectral frequencies exactly in Q2/Q15 arithmetic.
pub fn interpolate_nlsf(
    previous: &[i16],
    current: &[i16],
    weight: u8,
    output: &mut [i16],
) -> Result<(), Error> {
    if previous.len() != current.len()
        || current.len() != output.len()
        || ![10, 16].contains(&current.len())
    {
        return Err(Error::Output);
    }
    if weight > 4 {
        return Err(Error::Argument);
    }
    for ((destination, &old), &new) in output.iter_mut().zip(previous).zip(current) {
        *destination = (i32::from(old)
            + ((i32::from(weight) * (i32::from(new) - i32::from(old))) >> 2))
            as i16;
    }
    Ok(())
}

fn polynomial(cosines: &[i32], parity: usize, length: usize) -> [i32; 9] {
    let mut previous = [0i32; 9];
    previous[0] = 1 << 16;
    previous[1] = -cosines[parity];
    for pair in 1..length / 2 {
        let mut next = [0; 9];
        let cosine = i64::from(cosines[2 * pair + parity]);
        for coefficient in 0..=pair + 1 {
            let center = if coefficient > pair {
                previous[2 * pair - coefficient]
            } else {
                previous[coefficient]
            };
            let delayed = if coefficient >= 2 {
                previous[coefficient - 2]
            } else {
                0
            };
            let product = if coefficient > 0 {
                ((cosine * i64::from(previous[coefficient - 1]) + 32768) >> 16) as i32
            } else {
                0
            };
            next[coefficient] = center + delayed - product;
        }
        previous = next;
    }
    previous
}

fn bandwidth_expand(coefficients: &mut [i32], chirp: i32) {
    let mut scale = i64::from(chirp);
    for coefficient in coefficients {
        *coefficient = ((i64::from(*coefficient) * scale) >> 16) as i32;
        scale = (i64::from(chirp) * scale + 32768) >> 16;
    }
}

/// Inverse prediction gain in Q30; zero denotes excessive gain or instability.
pub fn inverse_prediction_gain(coefficients: &[i16]) -> Result<i32, Error> {
    if ![10, 16].contains(&coefficients.len()) {
        return Err(Error::Output);
    }
    Ok(inverse_gain(coefficients))
}

fn inverse_gain(coefficients: &[i16]) -> i32 {
    if coefficients
        .iter()
        .map(|&value| i32::from(value))
        .sum::<i32>()
        >= 4096
    {
        return 0;
    }
    let mut row = [0i64; MAX_ORDER];
    for (destination, &coefficient) in row.iter_mut().zip(coefficients) {
        *destination = i64::from(coefficient) << 12;
    }
    let mut gain = 1i64 << 30;
    for k in (0..coefficients.len()).rev() {
        if row[k].abs() > 16_773_022 {
            return 0;
        }
        let reflection = -row[k] << 7;
        let divisor = (1i64 << 30) - ((reflection * reflection) >> 32);
        gain = ((gain * divisor) >> 32) << 2;
        if gain < 107_374 {
            return 0;
        }
        if k == 0 {
            break;
        }
        let bits = 64 - divisor.leading_zeros();
        let low_bits = bits - 16;
        let reciprocal = ((1i64 << 29) - 1) / (divisor >> (low_bits + 1));
        let error = (1i64 << 29) - (((divisor << (15 - low_bits)) * reciprocal) >> 16);
        let factor = (reciprocal << 16) + ((error * reciprocal) >> 13);
        let mut next = [0i64; MAX_ORDER];
        for n in 0..k {
            let numerator = row[n] - ((row[k - n - 1] * reflection + (1i64 << 30)) >> 31);
            next[n] = (numerator * factor + (1i64 << (bits - 1))) >> bits;
            if next[n] < i64::from(i32::MIN) || next[n] > i64::from(i32::MAX) {
                return 0;
            }
        }
        row = next;
    }
    gain as i32
}

/// Convert stabilized Q15 spectral frequencies to bounded, stable Q12 LPC.
pub fn nlsf_to_lpc(nlsf: &[i16], output: &mut [i16]) -> Result<(), Error> {
    let length = nlsf.len();
    if ![10, 16].contains(&length) || output.len() != length {
        return Err(Error::Output);
    }
    if nlsf.iter().any(|&value| value < 0) || nlsf.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::Argument);
    }
    let ordering: &[usize] = if length == 16 {
        &WB_ORDERING
    } else {
        &NB_ORDERING
    };
    let mut cosines = [0i32; MAX_ORDER];
    for (&nlsf, &destination) in nlsf.iter().zip(ordering) {
        let position = nlsf as usize;
        let integer = position >> 8;
        let fraction = (position & 255) as i32;
        let cosine = i32::from(COS_Q12[integer]);
        let difference = i32::from(COS_Q12[integer + 1]) - cosine;
        cosines[destination] = (cosine * 256 + difference * fraction + 4) >> 3;
    }
    let p = polynomial(&cosines, 0, length);
    let q = polynomial(&cosines, 1, length);
    let mut coefficients = [0i32; MAX_ORDER];
    for k in 0..length / 2 {
        let symmetric = p[k + 1] + p[k];
        let antisymmetric = q[k + 1] - q[k];
        coefficients[k] = -antisymmetric - symmetric;
        coefficients[length - k - 1] = antisymmetric - symmetric;
    }
    for round in 0..10 {
        let mut peak = 0;
        let mut position = 0;
        for (k, &value) in coefficients[..length].iter().enumerate() {
            if value.abs() > peak {
                peak = value.abs();
                position = k;
            }
        }
        let peak_q12 = ((peak + 16) >> 5).min(163_838);
        if peak_q12 <= 32767 {
            break;
        }
        let chirp = 65_470 - ((peak_q12 - 32767) << 14) / ((peak_q12 * (position as i32 + 1)) >> 2);
        bandwidth_expand(&mut coefficients[..length], chirp);
        if round == 9 {
            for coefficient in &mut coefficients[..length] {
                *coefficient = ((*coefficient + 16) >> 5).clamp(-32768, 32767) << 5;
            }
        }
    }
    let mut quantized = [0i16; MAX_ORDER];
    for round in 0..16 {
        for (destination, &coefficient) in quantized[..length].iter_mut().zip(&coefficients) {
            *destination = ((coefficient + 16) >> 5) as i16;
        }
        if inverse_gain(&quantized[..length]) != 0 {
            output.copy_from_slice(&quantized[..length]);
            return Ok(());
        }
        bandwidth_expand(&mut coefficients[..length], 65536 - (2 << round));
    }
    output.fill(0);
    Ok(())
}

/// LPC coefficients for both halves of a SILK frame; trailing slots are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LpcCoefficients {
    /// Predictor order (10 or 16).
    pub order: u8,
    /// Current normalized spectral frequencies in Q15.
    pub nlsf: [i16; MAX_ORDER],
    /// First-half LPC coefficients in Q12.
    pub first: [i16; MAX_ORDER],
    /// Second-half LPC coefficients in Q12.
    pub second: [i16; MAX_ORDER],
    /// Effective interpolation weight; reset and bandwidth changes force 4.
    pub interpolation: u8,
}

/// Per-channel spectral history. Resetting or changing bandwidth drops history.
#[derive(Debug, Default)]
pub struct NlsfState {
    previous: [i16; MAX_ORDER],
    bandwidth: Option<Bandwidth>,
}

impl NlsfState {
    /// Start with no spectral history.
    pub const fn new() -> Self {
        Self {
            previous: [0; MAX_ORDER],
            bandwidth: None,
        }
    }
    /// Clear interpolation history without allocating.
    pub fn reset(&mut self) {
        self.previous.fill(0);
        self.bandwidth = None;
    }
    /// Decode spectral indices and produce both halves' LPC coefficients.
    pub fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        frame: Frame,
        signal: SignalType,
    ) -> Result<LpcCoefficients, Error> {
        self.reconstruct(decode_nlsf_indices(decoder, frame, signal), frame.bandwidth)
    }
    /// Reconstruct already decoded indices and commit valid spectral history.
    pub fn reconstruct(
        &mut self,
        indices: NlsfIndices,
        bandwidth: Bandwidth,
    ) -> Result<LpcCoefficients, Error> {
        let length = order(bandwidth);
        let interpolation = if self.bandwidth == Some(bandwidth) {
            indices.interpolation
        } else {
            4
        };
        let mut coefficients = LpcCoefficients {
            order: length as u8,
            nlsf: [0; MAX_ORDER],
            first: [0; MAX_ORDER],
            second: [0; MAX_ORDER],
            interpolation,
        };
        reconstruct_nlsf(indices, bandwidth, &mut coefficients.nlsf[..length])?;
        nlsf_to_lpc(
            &coefficients.nlsf[..length],
            &mut coefficients.second[..length],
        )?;
        if interpolation == 4 {
            coefficients.first = coefficients.second;
        } else {
            let mut first_nlsf = [0; MAX_ORDER];
            interpolate_nlsf(
                &self.previous[..length],
                &coefficients.nlsf[..length],
                interpolation,
                &mut first_nlsf[..length],
            )?;
            nlsf_to_lpc(&first_nlsf[..length], &mut coefficients.first[..length])?;
        }
        self.previous = coefficients.nlsf;
        self.bandwidth = Some(bandwidth);
        Ok(coefficients)
    }
}

const STAGE_ONE: [[Model<'static>; 2]; 2] = [
    [
        Model::from_static(&[
            0, 44, 78, 108, 127, 148, 160, 171, 174, 177, 179, 195, 197, 199, 200, 205, 207, 208,
            211, 214, 215, 216, 218, 220, 222, 225, 226, 235, 244, 246, 253, 255, 256,
        ]),
        Model::from_static(&[
            0, 1, 11, 12, 20, 23, 31, 39, 53, 66, 80, 81, 95, 107, 120, 131, 142, 154, 165, 175,
            185, 196, 204, 213, 221, 228, 236, 237, 238, 244, 245, 251, 256,
        ]),
    ],
    [
        Model::from_static(&[
            0, 31, 52, 55, 72, 73, 81, 98, 102, 103, 121, 137, 141, 143, 146, 147, 157, 158, 161,
            177, 188, 204, 206, 208, 211, 213, 224, 225, 229, 238, 246, 253, 256,
        ]),
        Model::from_static(&[
            0, 1, 5, 21, 26, 44, 55, 60, 74, 89, 90, 93, 105, 118, 132, 146, 152, 166, 178, 180,
            186, 187, 199, 211, 222, 232, 235, 245, 250, 251, 252, 253, 256,
        ]),
    ],
];

// Entropy models require cumulative arrays; constructed below as explicit
// cumulative constants so the processing path never builds a probability model.
const EXTENSION: Model<'static> = Model::from_static(&[0, 156, 216, 240, 249, 253, 255, 256]);
const INTERPOLATION: Model<'static> = Model::from_static(&[0, 13, 35, 64, 75, 256]);
const NB_PRED_WEIGHTS: [[u8; 2]; 9] = [
    [179, 116],
    [138, 67],
    [140, 82],
    [148, 59],
    [151, 92],
    [149, 72],
    [153, 100],
    [151, 89],
    [163, 92],
];
const WB_PRED_WEIGHTS: [[u8; 2]; 15] = [
    [175, 68],
    [148, 62],
    [160, 66],
    [176, 60],
    [178, 72],
    [173, 117],
    [174, 85],
    [164, 90],
    [177, 118],
    [174, 136],
    [196, 151],
    [182, 142],
    [198, 160],
    [192, 142],
    [182, 155],
];
const NB_MIN_SPACING: [i16; 11] = [250, 3, 6, 3, 3, 3, 4, 3, 3, 3, 461];
const WB_MIN_SPACING: [i16; 17] = [100, 3, 40, 3, 3, 3, 5, 14, 14, 10, 11, 3, 8, 9, 7, 3, 347];
const NB_ORDERING: [usize; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
const WB_ORDERING: [usize; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];

// RFC 6716 tables 15 and 16.
const STAGE_TWO: [Model<'static>; 16] = [
    Model::from_static(&[0, 1, 2, 3, 18, 242, 253, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 4, 38, 221, 253, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 6, 48, 197, 252, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 10, 62, 185, 246, 254, 255, 256]),
    Model::from_static(&[0, 1, 4, 20, 73, 174, 248, 254, 255, 256]),
    Model::from_static(&[0, 1, 4, 21, 76, 166, 239, 254, 255, 256]),
    Model::from_static(&[0, 1, 8, 32, 85, 159, 226, 252, 255, 256]),
    Model::from_static(&[0, 1, 2, 20, 83, 161, 219, 249, 255, 256]),
    Model::from_static(&[0, 1, 2, 3, 12, 244, 253, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 4, 32, 218, 253, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 5, 47, 199, 252, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 12, 61, 187, 252, 254, 255, 256]),
    Model::from_static(&[0, 1, 5, 24, 72, 172, 249, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 16, 70, 170, 242, 254, 255, 256]),
    Model::from_static(&[0, 1, 2, 17, 78, 165, 226, 251, 255, 256]),
    Model::from_static(&[0, 1, 8, 29, 79, 156, 237, 254, 255, 256]),
];

const NB_CODEBOOK: [[u8; 10]; 32] = [
    [12, 35, 60, 83, 108, 132, 157, 180, 206, 228],
    [15, 32, 55, 77, 101, 125, 151, 175, 201, 225],
    [19, 42, 66, 89, 114, 137, 162, 184, 209, 230],
    [12, 25, 50, 72, 97, 120, 147, 172, 200, 223],
    [26, 44, 69, 90, 114, 135, 159, 180, 205, 225],
    [13, 22, 53, 80, 106, 130, 156, 180, 205, 228],
    [15, 25, 44, 64, 90, 115, 142, 168, 196, 222],
    [19, 24, 62, 82, 100, 120, 145, 168, 190, 214],
    [22, 31, 50, 79, 103, 120, 151, 170, 203, 227],
    [21, 29, 45, 65, 106, 124, 150, 171, 196, 224],
    [30, 49, 75, 97, 121, 142, 165, 186, 209, 229],
    [19, 25, 52, 70, 93, 116, 143, 166, 192, 219],
    [26, 34, 62, 75, 97, 118, 145, 167, 194, 217],
    [25, 33, 56, 70, 91, 113, 143, 165, 196, 223],
    [21, 34, 51, 72, 97, 117, 145, 171, 196, 222],
    [20, 29, 50, 67, 90, 117, 144, 168, 197, 221],
    [22, 31, 48, 66, 95, 117, 146, 168, 196, 222],
    [24, 33, 51, 77, 116, 134, 158, 180, 200, 224],
    [21, 28, 70, 87, 106, 124, 149, 170, 194, 217],
    [26, 33, 53, 64, 83, 117, 152, 173, 204, 225],
    [27, 34, 65, 95, 108, 129, 155, 174, 210, 225],
    [20, 26, 72, 99, 113, 131, 154, 176, 200, 219],
    [34, 43, 61, 78, 93, 114, 155, 177, 205, 229],
    [23, 29, 54, 97, 124, 138, 163, 179, 209, 229],
    [30, 38, 56, 89, 118, 129, 158, 178, 200, 231],
    [21, 29, 49, 63, 85, 111, 142, 163, 193, 222],
    [27, 48, 77, 103, 133, 158, 179, 196, 215, 232],
    [29, 47, 74, 99, 124, 151, 176, 198, 220, 237],
    [33, 42, 61, 76, 93, 121, 155, 174, 207, 225],
    [29, 53, 87, 112, 136, 154, 170, 188, 208, 227],
    [24, 30, 52, 84, 131, 150, 166, 186, 203, 229],
    [37, 48, 64, 84, 104, 118, 156, 177, 201, 230],
];

const WB_CODEBOOK: [[u8; 16]; 32] = [
    [
        7, 23, 38, 54, 69, 85, 100, 116, 131, 147, 162, 178, 193, 208, 223, 239,
    ],
    [
        13, 25, 41, 55, 69, 83, 98, 112, 127, 142, 157, 171, 187, 203, 220, 236,
    ],
    [
        15, 21, 34, 51, 61, 78, 92, 106, 126, 136, 152, 167, 185, 205, 225, 240,
    ],
    [
        10, 21, 36, 50, 63, 79, 95, 110, 126, 141, 157, 173, 189, 205, 221, 237,
    ],
    [
        17, 20, 37, 51, 59, 78, 89, 107, 123, 134, 150, 164, 184, 205, 224, 240,
    ],
    [
        10, 15, 32, 51, 67, 81, 96, 112, 129, 142, 158, 173, 189, 204, 220, 236,
    ],
    [
        8, 21, 37, 51, 65, 79, 98, 113, 126, 138, 155, 168, 179, 192, 209, 218,
    ],
    [
        12, 15, 34, 55, 63, 78, 87, 108, 118, 131, 148, 167, 185, 203, 219, 236,
    ],
    [
        16, 19, 32, 36, 56, 79, 91, 108, 118, 136, 154, 171, 186, 204, 220, 237,
    ],
    [
        11, 28, 43, 58, 74, 89, 105, 120, 135, 150, 165, 180, 196, 211, 226, 241,
    ],
    [
        6, 16, 33, 46, 60, 75, 92, 107, 123, 137, 156, 169, 185, 199, 214, 225,
    ],
    [
        11, 19, 30, 44, 57, 74, 89, 105, 121, 135, 152, 169, 186, 202, 218, 234,
    ],
    [
        12, 19, 29, 46, 57, 71, 88, 100, 120, 132, 148, 165, 182, 199, 216, 233,
    ],
    [
        17, 23, 35, 46, 56, 77, 92, 106, 123, 134, 152, 167, 185, 204, 222, 237,
    ],
    [
        14, 17, 45, 53, 63, 75, 89, 107, 115, 132, 151, 171, 188, 206, 221, 240,
    ],
    [
        9, 16, 29, 40, 56, 71, 88, 103, 119, 137, 154, 171, 189, 205, 222, 237,
    ],
    [
        16, 19, 36, 48, 57, 76, 87, 105, 118, 132, 150, 167, 185, 202, 218, 236,
    ],
    [
        12, 17, 29, 54, 71, 81, 94, 104, 126, 136, 149, 164, 182, 201, 221, 237,
    ],
    [
        15, 28, 47, 62, 79, 97, 115, 129, 142, 155, 168, 180, 194, 208, 223, 238,
    ],
    [
        8, 14, 30, 45, 62, 78, 94, 111, 127, 143, 159, 175, 192, 207, 223, 239,
    ],
    [
        17, 30, 49, 62, 79, 92, 107, 119, 132, 145, 160, 174, 190, 204, 220, 235,
    ],
    [
        14, 19, 36, 45, 61, 76, 91, 108, 121, 138, 154, 172, 189, 205, 222, 238,
    ],
    [
        12, 18, 31, 45, 60, 76, 91, 107, 123, 138, 154, 171, 187, 204, 221, 236,
    ],
    [
        13, 17, 31, 43, 53, 70, 83, 103, 114, 131, 149, 167, 185, 203, 220, 237,
    ],
    [
        17, 22, 35, 42, 58, 78, 93, 110, 125, 139, 155, 170, 188, 206, 224, 240,
    ],
    [
        8, 15, 34, 50, 67, 83, 99, 115, 131, 146, 162, 178, 193, 209, 224, 239,
    ],
    [
        13, 16, 41, 66, 73, 86, 95, 111, 128, 137, 150, 163, 183, 206, 225, 241,
    ],
    [
        17, 25, 37, 52, 63, 75, 92, 102, 119, 132, 144, 160, 175, 191, 212, 231,
    ],
    [
        19, 31, 49, 65, 83, 100, 117, 133, 147, 161, 174, 187, 200, 213, 227, 242,
    ],
    [
        18, 31, 52, 68, 88, 103, 117, 126, 138, 149, 163, 177, 192, 207, 223, 239,
    ],
    [
        16, 29, 47, 61, 76, 90, 106, 119, 133, 147, 161, 176, 193, 209, 224, 240,
    ],
    [
        15, 21, 35, 50, 61, 73, 86, 97, 110, 119, 129, 141, 175, 198, 218, 237,
    ],
];

const NB_SELECT: [[u8; 10]; 32] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 3, 1, 2, 2, 1, 2, 1, 1, 1],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [1, 2, 2, 2, 2, 1, 2, 1, 1, 1],
    [2, 3, 3, 3, 3, 2, 2, 2, 2, 2],
    [0, 5, 3, 3, 2, 2, 2, 2, 1, 1],
    [0, 2, 2, 2, 2, 2, 2, 2, 2, 1],
    [2, 3, 6, 4, 4, 4, 5, 4, 5, 5],
    [2, 4, 5, 5, 4, 5, 4, 6, 4, 4],
    [2, 4, 4, 7, 4, 5, 4, 5, 5, 4],
    [4, 3, 3, 3, 2, 3, 2, 2, 2, 2],
    [1, 5, 5, 6, 4, 5, 4, 5, 5, 5],
    [2, 7, 4, 6, 5, 5, 5, 5, 5, 5],
    [2, 7, 5, 5, 5, 5, 5, 6, 5, 4],
    [3, 3, 5, 4, 4, 5, 4, 5, 4, 4],
    [2, 3, 3, 5, 5, 4, 4, 4, 4, 4],
    [2, 4, 4, 6, 4, 5, 4, 5, 5, 5],
    [2, 5, 4, 6, 5, 5, 5, 4, 5, 4],
    [2, 7, 4, 5, 4, 5, 4, 5, 5, 5],
    [2, 5, 4, 6, 7, 6, 5, 6, 5, 4],
    [3, 6, 7, 4, 6, 5, 5, 6, 4, 5],
    [2, 7, 6, 4, 4, 4, 5, 4, 5, 5],
    [4, 5, 5, 4, 6, 6, 5, 6, 5, 4],
    [2, 5, 5, 6, 5, 6, 4, 6, 4, 4],
    [4, 5, 5, 5, 3, 7, 4, 5, 5, 4],
    [2, 3, 4, 5, 5, 6, 4, 5, 5, 4],
    [2, 3, 2, 3, 3, 4, 2, 3, 3, 3],
    [1, 1, 2, 2, 2, 2, 2, 3, 2, 2],
    [4, 5, 5, 6, 6, 6, 5, 6, 4, 5],
    [3, 5, 5, 4, 4, 4, 4, 3, 3, 2],
    [2, 5, 3, 7, 5, 5, 4, 4, 5, 4],
    [4, 4, 5, 4, 5, 6, 5, 6, 5, 4],
];

const WB_SELECT: [[u8; 16]; 32] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 1, 1, 1, 0, 3],
    [2, 5, 5, 3, 7, 4, 4, 5, 2, 5, 4, 5, 5, 4, 3, 3],
    [0, 2, 1, 2, 2, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 1],
    [0, 6, 5, 4, 6, 4, 7, 5, 4, 4, 4, 5, 5, 4, 4, 3],
    [0, 3, 5, 5, 4, 3, 3, 5, 3, 3, 3, 3, 3, 3, 2, 4],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 2, 6, 3, 7, 2, 5, 3, 4, 5, 5, 4, 3, 3, 2, 3],
    [0, 6, 2, 6, 6, 4, 5, 4, 6, 5, 4, 4, 5, 3, 3, 3],
    [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [2, 2, 3, 4, 5, 3, 3, 3, 3, 3, 3, 3, 2, 2, 1, 3],
    [2, 2, 3, 3, 4, 3, 3, 3, 3, 3, 3, 3, 3, 2, 1, 3],
    [3, 4, 4, 4, 6, 4, 4, 5, 3, 5, 4, 4, 5, 4, 3, 4],
    [0, 6, 4, 5, 4, 7, 5, 2, 6, 5, 7, 4, 4, 3, 5, 3],
    [0, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0],
    [1, 6, 5, 7, 5, 4, 5, 3, 4, 5, 4, 4, 4, 3, 3, 4],
    [1, 3, 3, 4, 4, 3, 3, 5, 2, 3, 3, 5, 5, 5, 3, 4],
    [2, 3, 3, 2, 2, 2, 3, 2, 1, 2, 1, 2, 1, 1, 1, 4],
    [0, 2, 3, 5, 3, 3, 2, 2, 2, 1, 1, 0, 0, 0, 0, 0],
    [3, 4, 3, 5, 3, 3, 2, 2, 1, 1, 1, 1, 1, 2, 2, 4],
    [2, 6, 3, 7, 7, 4, 5, 4, 5, 3, 5, 3, 3, 2, 3, 3],
    [2, 3, 5, 6, 6, 3, 5, 3, 4, 4, 3, 3, 3, 3, 2, 4],
    [1, 3, 3, 4, 4, 4, 4, 3, 5, 5, 5, 3, 1, 1, 1, 1],
    [2, 5, 3, 6, 6, 4, 7, 4, 4, 5, 3, 4, 4, 3, 3, 3],
    [0, 6, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 6, 6, 3, 5, 2, 5, 5, 3, 4, 4, 7, 7, 4, 4, 4],
    [3, 3, 7, 3, 5, 4, 3, 3, 3, 2, 2, 3, 3, 3, 2, 3],
    [0, 0, 1, 0, 0, 0, 2, 1, 2, 1, 1, 2, 2, 2, 1, 1],
    [0, 3, 2, 5, 3, 3, 2, 3, 2, 1, 0, 0, 1, 0, 0, 1],
    [3, 5, 5, 4, 7, 5, 3, 3, 2, 3, 2, 2, 1, 0, 1, 0],
    [2, 3, 5, 3, 4, 3, 3, 3, 2, 1, 2, 6, 4, 0, 0, 0],
];

const NB_PREDICT: [[u8; 9]; 32] = [
    [0, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 0, 0, 0, 0, 1, 0],
    [0, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 1, 1, 0, 0, 0, 1, 0],
    [0, 1, 1, 0, 0, 1, 1, 0, 0],
    [0, 0, 1, 1, 0, 1, 0, 1, 1],
    [0, 0, 1, 1, 0, 0, 1, 1, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 0, 1, 1, 1, 1, 1, 0],
    [0, 1, 0, 1, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 1, 1, 0],
    [1, 0, 1, 1, 0, 1, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 0, 1, 0],
    [0, 0, 1, 1, 0, 1, 0, 1, 0],
    [0, 0, 1, 1, 1, 0, 1, 1, 1],
    [0, 1, 1, 0, 0, 1, 1, 1, 0],
    [0, 0, 0, 1, 1, 1, 0, 1, 0],
    [0, 1, 1, 0, 0, 1, 0, 1, 0],
    [0, 1, 1, 0, 0, 0, 1, 1, 0],
    [0, 0, 0, 0, 0, 1, 1, 1, 1],
    [0, 0, 1, 1, 0, 0, 0, 1, 1],
    [0, 0, 0, 1, 0, 1, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 1, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 1, 0, 1, 1, 0, 1, 0],
    [1, 0, 0, 1, 0, 0, 0, 0, 0],
    [0, 0, 0, 1, 1, 0, 1, 0, 1],
    [1, 0, 1, 1, 0, 1, 1, 1, 1],
];

const WB_PREDICT: [[u8; 15]; 32] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0],
    [0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1, 0],
    [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0],
    [0, 1, 1, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 1],
    [0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0],
    [0, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0, 0],
    [0, 0, 0, 0, 1, 1, 0, 1, 0, 1, 1, 1, 1, 0, 0],
    [0, 1, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1],
    [0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0],
    [0, 1, 1, 0, 1, 0, 1, 1, 1, 1, 1, 0, 1, 0, 0],
    [0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0],
    [0, 0, 1, 1, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0],
    [0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 1, 0],
    [0, 0, 0, 1, 1, 0, 1, 0, 1, 1, 1, 1, 1, 1, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [0, 1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1],
    [0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 1, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0],
    [0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 0],
];

const COS_Q12: [i16; 129] = [
    4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3997, 3973, 3948, 3920, 3889, 3857, 3822,
    3784, 3745, 3703, 3659, 3613, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
    2896, 2824, 2751, 2676, 2599, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
    1568, 1474, 1380, 1285, 1189, 1093, 995, 897, 799, 700, 601, 501, 401, 301, 201, 101, 0, -101,
    -201, -301, -401, -501, -601, -700, -799, -897, -995, -1093, -1189, -1285, -1380, -1474, -1568,
    -1660, -1751, -1842, -1931, -2019, -2106, -2191, -2276, -2359, -2440, -2520, -2599, -2676,
    -2751, -2824, -2896, -2967, -3035, -3102, -3166, -3229, -3290, -3349, -3406, -3461, -3513,
    -3564, -3613, -3659, -3703, -3745, -3784, -3822, -3857, -3889, -3920, -3948, -3973, -3997,
    -4017, -4036, -4052, -4065, -4076, -4085, -4091, -4095, -4096,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn random(state: &mut u32) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state
    }

    #[test]
    fn native_black_box_bitstreams_and_spectral_vectors_are_exact() {
        // Encoded and reconstructed by installed libopus 1.6.1 through its
        // exported binary API, with RFC probability data. No reference source
        // is compiled into Snacka. Includes escaped residuals and chirp limits.
        {
            let mut decoder = Decoder::new(&[20]);
            let frame = Frame {
                bandwidth: Bandwidth::Narrow,
                duration: FrameDuration::Ms10,
            };
            let indices = decode_nlsf_indices(&mut decoder, frame, SignalType::Unvoiced);
            assert_eq!(
                indices,
                NlsfIndices {
                    first: 0,
                    residual: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    interpolation: 4
                }
            );
            let mut nlsf = [0; 10];
            reconstruct_nlsf(indices, frame.bandwidth, &mut nlsf).unwrap();
            assert_eq!(
                nlsf,
                [
                    1536, 4480, 7680, 10624, 13824, 16896, 20096, 23040, 26368, 29184
                ]
            );
            let mut lpc = [0; 10];
            nlsf_to_lpc(&nlsf, &mut lpc).unwrap();
            assert_eq!(lpc, [2681, 30, 471, -120, 288, -82, 268, -184, 234, -164]);
            assert_eq!(inverse_prediction_gain(&lpc).unwrap(), 439649768);
        }
        {
            let mut decoder = Decoder::new(&[
                251, 4, 255, 255, 251, 4, 255, 255, 251, 4, 255, 255, 251, 4, 255, 255, 251, 4,
                255, 255, 251, 192,
            ]);
            let frame = Frame {
                bandwidth: Bandwidth::Narrow,
                duration: FrameDuration::Ms20,
            };
            let indices = decode_nlsf_indices(&mut decoder, frame, SignalType::Voiced);
            assert_eq!(
                indices,
                NlsfIndices {
                    first: 31,
                    residual: [
                        -10, 10, -10, 10, -10, 10, -10, 10, -10, 10, 0, 0, 0, 0, 0, 0
                    ],
                    interpolation: 2
                }
            );
            let mut nlsf = [0; 10];
            reconstruct_nlsf(indices, frame.bandwidth, &mut nlsf).unwrap();
            assert_eq!(
                nlsf,
                [
                    250, 6116, 6122, 14405, 14408, 17535, 17539, 24725, 24728, 32307
                ]
            );
            let mut lpc = [0; 10];
            nlsf_to_lpc(&nlsf, &mut lpc).unwrap();
            assert_eq!(
                lpc,
                [1591, -2311, 1573, 966, 8, -983, -1557, 2297, -1582, 4086]
            );
            assert_eq!(inverse_prediction_gain(&lpc).unwrap(), 1809676);
        }
        {
            let mut decoder = Decoder::new(&[60, 0, 8, 173, 220, 25, 168, 18, 103, 7, 16, 160]);
            let frame = Frame {
                bandwidth: Bandwidth::Wide,
                duration: FrameDuration::Ms20,
            };
            let indices = decode_nlsf_indices(&mut decoder, frame, SignalType::Voiced);
            assert_eq!(
                indices,
                NlsfIndices {
                    first: 7,
                    residual: [-4, -3, -2, -1, 0, 1, 2, 3, 4, -4, -3, -2, -1, 0, 1, 2],
                    interpolation: 0
                }
            );
            let mut nlsf = [0; 16];
            reconstruct_nlsf(indices, frame.bandwidth, &mut nlsf).unwrap();
            assert_eq!(
                nlsf,
                [
                    100, 247, 2598, 7427, 9541, 12200, 13565, 14776, 14790, 14800, 15207, 19594,
                    23722, 27362, 29839, 31991
                ]
            );
            let mut lpc = [0; 16];
            nlsf_to_lpc(&nlsf, &mut lpc).unwrap();
            assert_eq!(
                lpc,
                [
                    7260, -7045, 2023, 8077, -9917, 6012, 939, -4486, 2003, -145, -660, -692, 327,
                    493, -892, 794
                ]
            );
            assert_eq!(inverse_prediction_gain(&lpc).unwrap(), 114000);
        }
        {
            let mut decoder = Decoder::new(&[
                246, 6, 249, 13, 249, 4, 63, 222, 2, 54, 32, 2, 65, 248, 253, 61, 229, 110, 125,
                15, 186, 204, 86, 224,
            ]);
            let frame = Frame {
                bandwidth: Bandwidth::Wide,
                duration: FrameDuration::Ms20,
            };
            let indices = decode_nlsf_indices(&mut decoder, frame, SignalType::Unvoiced);
            assert_eq!(
                indices,
                NlsfIndices {
                    first: 30,
                    residual: [
                        -10, -3, 4, -10, -3, 4, -10, -3, 4, -10, -3, 4, -10, -3, 4, -10
                    ],
                    interpolation: 3
                }
            );
            let mut nlsf = [0; 16];
            reconstruct_nlsf(indices, frame.bandwidth, &mut nlsf).unwrap();
            assert_eq!(
                nlsf,
                [
                    100, 103, 714, 717, 3921, 4093, 4098, 9862, 9876, 9886, 15710, 15754, 15762,
                    22221, 24029, 24032
                ]
            );
            let mut lpc = [0; 16];
            nlsf_to_lpc(&nlsf, &mut lpc).unwrap();
            assert_eq!(
                lpc,
                [
                    15615, -28142, 32680, -28275, 20054, -12545, 7290, -3999, 2017, -888, 324, -95,
                    22, -4, 0, 0
                ]
            );
            assert_eq!(inverse_prediction_gain(&lpc).unwrap(), 1177488);
        }
        {
            let mut decoder = Decoder::new(&[221, 251, 247, 121, 34, 159, 88]);
            let frame = Frame {
                bandwidth: Bandwidth::Narrow,
                duration: FrameDuration::Ms10,
            };
            let indices = decode_nlsf_indices(&mut decoder, frame, SignalType::Unvoiced);
            assert_eq!(
                indices,
                NlsfIndices {
                    first: 23,
                    residual: [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0],
                    interpolation: 4
                }
            );
            let mut nlsf = [0; 10];
            reconstruct_nlsf(indices, frame.bandwidth, &mut nlsf).unwrap();
            assert_eq!(
                nlsf,
                [
                    6442, 6530, 11111, 16817, 20682, 22105, 25036, 26287, 30096, 31734
                ]
            );
            let mut lpc = [0; 10];
            nlsf_to_lpc(&nlsf, &mut lpc).unwrap();
            assert_eq!(
                lpc,
                [
                    -9693, -9051, -3763, -1929, -5911, -9078, -7927, -4785, -2549, -1004
                ]
            );
            assert_eq!(inverse_prediction_gain(&lpc).unwrap(), 8372604);
        }
    }

    #[test]
    fn arbitrary_q15_vectors_stabilize_and_produce_bounded_stable_predictors() {
        let mut state = 0x9254_ffe3;
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let length = order(bandwidth);
            for _ in 0..2000 {
                let mut nlsf = [0; MAX_ORDER];
                for coefficient in &mut nlsf[..length] {
                    *coefficient = random(&mut state) as i16;
                }
                stabilize_nlsf(&mut nlsf[..length], bandwidth).unwrap();
                let delta = spacings(bandwidth);
                assert!(nlsf[0] >= delta[0]);
                for k in 1..length {
                    assert!(i32::from(nlsf[k]) - i32::from(nlsf[k - 1]) >= i32::from(delta[k]));
                }
                assert!(32768 - i32::from(nlsf[length - 1]) >= i32::from(delta[length]));
                let before = nlsf;
                stabilize_nlsf(&mut nlsf[..length], bandwidth).unwrap();
                assert_eq!(nlsf, before);
                let mut lpc = [0; MAX_ORDER];
                nlsf_to_lpc(&nlsf[..length], &mut lpc[..length]).unwrap();
                assert!(inverse_prediction_gain(&lpc[..length]).unwrap() >= 107374);
            }
        }
    }

    #[test]
    fn interpolation_reset_and_invalid_indices_preserve_channel_history() {
        let zero = NlsfIndices {
            first: 0,
            residual: [0; MAX_ORDER],
            interpolation: 0,
        };
        let next = NlsfIndices {
            first: 23,
            residual: [2; MAX_ORDER],
            interpolation: 0,
        };
        let mut state = NlsfState::new();
        let first = state.reconstruct(zero, Bandwidth::Narrow).unwrap();
        assert_eq!(first.interpolation, 4);
        assert_eq!(first.first, first.second);
        let second = state.reconstruct(next, Bandwidth::Narrow).unwrap();
        assert_eq!(second.interpolation, 0);
        assert_eq!(second.first, first.second);
        assert_ne!(second.first, second.second);
        let previous = state.previous;
        let invalid = NlsfIndices { first: 32, ..zero };
        assert_eq!(
            state.reconstruct(invalid, Bandwidth::Narrow),
            Err(Error::Argument)
        );
        assert_eq!(state.previous, previous);
        let changed = state.reconstruct(next, Bandwidth::Medium).unwrap();
        assert_eq!(changed.interpolation, 4);
        state.reset();
        let reset = state.reconstruct(next, Bandwidth::Narrow).unwrap();
        assert_eq!(reset.interpolation, 4);
        assert_eq!(reset.first, reset.second);
        let mut output = [99; 10];
        assert_eq!(
            reconstruct_nlsf(invalid, Bandwidth::Narrow, &mut output),
            Err(Error::Argument)
        );
        assert_eq!(output, [99; 10]);
        assert_eq!(nlsf_to_lpc(&[-1; 10], &mut output), Err(Error::Argument));
        assert_eq!(output, [99; 10]);
        assert_eq!(
            interpolate_nlsf(&[10; 10], &[20; 10], 5, &mut output),
            Err(Error::Argument)
        );
        assert_eq!(output, [99; 10]);
    }
}
