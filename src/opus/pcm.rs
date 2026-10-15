// SPDX-License-Identifier: MIT
//! Allocation-free PCM sample-format conversion for every Opus codec surface.
//!
//! Conversion operates on interleaved or planar samples without interpreting
//! channels. Prepare one floating-point buffer before entering an audio callback
//! and reuse it with ordinary, mapped or projection encoding, decoding,
//! concealment and recovery. The destination must hold the entire input;
//! additional destination elements remain untouched. Errors leave it unchanged.
//!
//! Signed 16-bit PCM uses a scale of 32768. Signed 24-bit PCM is stored in an
//! `i32`, right-justified with sign extension, and uses a scale of 8388608. The
//! 24-bit conversions also accept and preserve values beyond that nominal
//! range, up to the `i32` limits. Floating point cannot distinguish every such
//! integer beyond 24 significant bits. Conversion never normalizes a frame or
//! silently reduces its amplitude to fit an encoder's accepted range.
//!
//! Floating-point output conversion rounds to nearest, with half ties to even,
//! then saturates at the destination integer type's limits. NaN is rejected;
//! infinities saturate. This is a deterministic sample conversion, without
//! dithering or stateful soft clipping. In particular it is not an exact replica
//! of libopus's 16-bit decoder API, which can apply soft clipping. Snacka's
//! encoders currently accept only finite input in `[-1, 1]`, so integer
//! conversion does not establish extended-dynamic-range encoder parity.
//!
//! ```
//! use snacka::opus::{Encoder, EncoderConfig, pcm};
//!
//! let mut encoder = Encoder::new(EncoderConfig::default()).unwrap();
//! // Allocate this workspace once, alongside the encoder.
//! let mut workspace = vec![0.0; encoder.config().input_samples()];
//! let input = vec![0i16; workspace.len()];
//! let mut packet = vec![0; encoder.config().max_packet_bytes()];
//! pcm::i16_to_f32(&input, &mut workspace).unwrap();
//! let length = encoder.encode(&workspace, &mut packet).unwrap();
//! assert!(length > 0);
//! ```

/// Invalid source samples or insufficient destination capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Floating-point source contains NaN.
    Input,
    /// Destination cannot hold every source sample.
    Output,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Input => "NaN PCM sample",
            Self::Output => "insufficient PCM conversion output",
        })
    }
}

impl std::error::Error for Error {}

fn check_output(input_len: usize, output_len: usize) -> Result<(), Error> {
    if output_len < input_len {
        return Err(Error::Output);
    }
    Ok(())
}

fn check_float(input: &[f32], output_len: usize) -> Result<(), Error> {
    check_output(input.len(), output_len)?;
    if input.iter().any(|sample| sample.is_nan()) {
        return Err(Error::Input);
    }
    Ok(())
}

/// Convert signed 16-bit PCM to normalized float: `sample / 32768`.
///
/// Every input is represented exactly, including `i16::MIN` as `-1.0`.
/// `i16::MAX` is `1.0 - 1.0 / 32768.0`.
pub fn i16_to_f32(input: &[i16], output: &mut [f32]) -> Result<(), Error> {
    check_output(input.len(), output.len())?;
    for (&sample, destination) in input.iter().zip(output) {
        *destination = f32::from(sample) * (1.0 / 32768.0);
    }
    Ok(())
}

/// Convert right-justified signed 24-bit PCM in `i32`: `sample / 8388608`.
///
/// Every value in `[-8388608, 8388607]` is represented exactly. The full `i32`
/// range is accepted for extended dynamic range; precision can be lost outside
/// the nominal 24-bit range. `i32::MAX` consequently rounds to `256.0`, while
/// `i32::MIN` is represented exactly as `-256.0`. The converted sample may exceed
/// the input range currently accepted by Snacka's encoders.
pub fn i24_to_f32(input: &[i32], output: &mut [f32]) -> Result<(), Error> {
    check_output(input.len(), output.len())?;
    for (&sample, destination) in input.iter().zip(output) {
        *destination = (f64::from(sample) * (1.0 / 8388608.0)) as f32;
    }
    Ok(())
}

/// Convert float to signed 16-bit PCM using scale 32768 and half ties to even.
///
/// Results saturate to `[-32768, 32767]`. NaN rejects the entire conversion
/// before any destination is written; infinities saturate. This does not apply
/// the stateful soft clipping used by libopus's 16-bit decoder API.
pub fn f32_to_i16(input: &[f32], output: &mut [i16]) -> Result<(), Error> {
    check_float(input, output.len())?;
    for (&sample, destination) in input.iter().zip(output) {
        *destination = (f64::from(sample) * 32768.0).round_ties_even() as i16;
    }
    Ok(())
}

/// Convert float to right-justified signed 24-bit PCM in `i32`.
///
/// Uses scale 8388608 and rounds half ties to even. Values beyond the nominal
/// signed 24-bit range are retained; saturation occurs only at the `i32` limits
/// `[-2147483648, 2147483647]`. NaN rejects the complete conversion before any
/// writes, and infinities saturate. This matches the scaling and full-width
/// storage of the public libopus 1.6 24-bit PCM API, without claiming identical
/// decoded audio from different codec implementations.
pub fn f32_to_i24(input: &[f32], output: &mut [i32]) -> Result<(), Error> {
    check_float(input, output.len())?;
    for (&sample, destination) in input.iter().zip(output) {
        *destination = (f64::from(sample) * 8388608.0).round_ties_even() as i32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_i16_value_round_trips_exactly() {
        let input: Vec<_> = (i16::MIN..=i16::MAX).collect();
        let mut floating = vec![0.0; input.len()];
        let mut result = vec![0; input.len()];
        i16_to_f32(&input, &mut floating).unwrap();
        assert_eq!(floating[0], -1.0);
        assert_eq!(*floating.last().unwrap(), 32767.0 / 32768.0);
        f32_to_i16(&floating, &mut result).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn signed24_extrema_and_extended_i32_scale_are_explicit() {
        let input = [i32::MIN, -8388608, -1, 0, 1, 8388607, 8388608, i32::MAX];
        let mut floating = [0.0; 8];
        i24_to_f32(&input, &mut floating).unwrap();
        assert_eq!(
            floating,
            [
                -256.0,
                -1.0,
                -2.0f32.powi(-23),
                0.0,
                2.0f32.powi(-23),
                1.0 - 2.0f32.powi(-23),
                1.0,
                256.0
            ]
        );
        let mut result = [0; 8];
        f32_to_i24(&floating, &mut result).unwrap();
        assert_eq!(result, input);
        // Beyond 24 significant bits, conversion cannot retain every integer.
        i24_to_f32(&[i32::MAX - 1], &mut floating).unwrap();
        f32_to_i24(&floating[..1], &mut result).unwrap();
        assert_eq!(result[0], i32::MAX);
    }

    #[test]
    fn half_ties_use_even_for_both_signs_and_formats() {
        // Independent integer oracle: n/2 rounds to the adjacent even integer.
        for n in -33i32..=33 {
            let lower = n.div_euclid(2);
            let expected = if n.rem_euclid(2) == 0 || lower % 2 == 0 {
                lower
            } else {
                lower + 1
            };
            let mut result16 = [0];
            let mut result24 = [0];
            f32_to_i16(&[n as f32 / 65536.0], &mut result16).unwrap();
            f32_to_i24(&[n as f32 / 16777216.0], &mut result24).unwrap();
            assert_eq!(i32::from(result16[0]), expected, "i16 numerator {n}");
            assert_eq!(result24[0], expected, "i24 numerator {n}");
        }
    }

    #[test]
    fn integer_limits_saturate_without_overflow() {
        let input = [
            f32::NEG_INFINITY,
            -f32::MAX,
            -257.0,
            -256.0,
            -1.0,
            -0.0,
            1.0,
            255.0,
            256.0,
            f32::MAX,
            f32::INFINITY,
        ];
        let mut result16 = [0; 11];
        let mut result24 = [0; 11];
        f32_to_i16(&input, &mut result16).unwrap();
        f32_to_i24(&input, &mut result24).unwrap();
        assert_eq!(
            result16,
            [
                i16::MIN,
                i16::MIN,
                i16::MIN,
                i16::MIN,
                i16::MIN,
                0,
                i16::MAX,
                i16::MAX,
                i16::MAX,
                i16::MAX,
                i16::MAX
            ]
        );
        assert_eq!(
            result24,
            [
                i32::MIN,
                i32::MIN,
                i32::MIN,
                i32::MIN,
                -8388608,
                0,
                8388608,
                2139095040,
                i32::MAX,
                i32::MAX,
                i32::MAX
            ]
        );
    }

    #[test]
    fn all_preflight_errors_preserve_the_entire_destination() {
        let mut floating = [7.0; 2];
        assert_eq!(i16_to_f32(&[0; 3], &mut floating), Err(Error::Output));
        assert_eq!(i24_to_f32(&[0; 3], &mut floating), Err(Error::Output));
        assert_eq!(floating, [7.0; 2]);
        let mut result16 = [7; 2];
        let mut result24 = [7; 2];
        for input in [&[0.0, 0.0, 0.0][..], &[0.0, f32::NAN][..]] {
            let error = if input.len() == 3 {
                Error::Output
            } else {
                Error::Input
            };
            assert_eq!(f32_to_i16(input, &mut result16), Err(error));
            assert_eq!(f32_to_i24(input, &mut result24), Err(error));
            assert_eq!(result16, [7; 2]);
            assert_eq!(result24, [7; 2]);
        }
    }

    #[test]
    fn suffixes_and_empty_conversions_are_untouched() {
        let mut floating = [7.0; 2];
        i16_to_f32(&[-32768], &mut floating).unwrap();
        assert_eq!(floating, [-1.0, 7.0]);
        i24_to_f32(&[8388608], &mut floating).unwrap();
        assert_eq!(floating, [1.0, 7.0]);
        let mut result16 = [7; 2];
        let mut result24 = [7; 2];
        f32_to_i16(&[1.0], &mut result16).unwrap();
        f32_to_i24(&[1.0], &mut result24).unwrap();
        assert_eq!(result16, [32767, 7]);
        assert_eq!(result24, [8388608, 7]);
        i16_to_f32(&[], &mut floating).unwrap();
        i24_to_f32(&[], &mut floating).unwrap();
        f32_to_i16(&[], &mut result16).unwrap();
        f32_to_i24(&[], &mut result24).unwrap();
        assert_eq!(floating, [1.0, 7.0]);
        assert_eq!(result16, [32767, 7]);
        assert_eq!(result24, [8388608, 7]);
    }
}
