// SPDX-License-Identifier: MIT
//! Independent CELT pulse-vector primitives from RFC 6716 §§4.3.4 and 5.3.8.
//!
//! A compact shared codebook table is generated from the mathematical recurrence
//! during compilation. The ordering is the RFC's CWRS ordering; processing uses
//! caller-owned output. Larger codebooks must be split by the band coder.

use crate::entropy;
use std::fmt;

/// Largest CELT band at 20 ms (22 bins in each of eight short transforms).
pub const MAX_DIMENSIONS: usize = 176;
/// Largest pulse count represented by CELT's allocation index, 40.
pub const MAX_PULSES: usize = 128;
const ROW_SIZE: usize = MAX_PULSES + 2;

// U(n,k) = (V(n-1,k-1) + V(n,k-1))/2 for k > 0.
// V(n,k) = U(n,k) + U(n,k+1); U follows the same three-term
// recurrence, starting with U(0,0)=1 and zero axes otherwise.
// Saturation is confined to compilation and identifies the unused entries
// above the 32-bit codebook limit. Every stored entry remains exact.
const fn next_row(row: &mut [u64; ROW_SIZE]) {
    let mut previous = row[0];
    row[0] = 0;
    let mut k = 1;
    while k < ROW_SIZE {
        let old = row[k];
        let sum = old + row[k - 1] + previous;
        row[k] = if sum > u32::MAX as u64 {
            u32::MAX as u64 + 1
        } else {
            sum
        };
        previous = old;
        k += 1;
    }
}

const fn row_lengths() -> [usize; MAX_DIMENSIONS + 1] {
    let mut lengths = [0; MAX_DIMENSIONS + 1];
    let mut row = [0; ROW_SIZE];
    row[0] = 1;
    let mut n = 1;
    while n <= MAX_DIMENSIONS {
        next_row(&mut row);
        let mut k = 0;
        while k <= MAX_PULSES && row[k] + row[k + 1] <= u32::MAX as u64 {
            lengths[n] = k + 2;
            k += 1;
        }
        n += 1;
    }
    lengths
}
const ROW_LENGTHS: [usize; MAX_DIMENSIONS + 1] = row_lengths();
const TABLE_ENTRIES: usize = {
    let mut total = 0;
    let mut n = 1;
    while n <= MAX_DIMENSIONS {
        total += ROW_LENGTHS[n];
        n += 1;
    }
    total
};
const _: () = assert!(TABLE_ENTRIES <= u16::MAX as usize);
const fn codebooks() -> ([u16; MAX_DIMENSIONS + 2], [u32; TABLE_ENTRIES]) {
    let mut offsets = [0; MAX_DIMENSIONS + 2];
    let mut values = [0; TABLE_ENTRIES];
    let mut row = [0; ROW_SIZE];
    row[0] = 1;
    let mut cursor = 0;
    let mut n = 1;
    while n <= MAX_DIMENSIONS {
        next_row(&mut row);
        offsets[n] = cursor as u16;
        let mut k = 0;
        while k < ROW_LENGTHS[n] {
            values[cursor] = row[k] as u32;
            cursor += 1;
            k += 1;
        }
        n += 1;
    }
    offsets[MAX_DIMENSIONS + 1] = cursor as u16;
    (offsets, values)
}
static CODEBOOKS: ([u16; MAX_DIMENSIONS + 2], [u32; TABLE_ENTRIES]) = codebooks();

#[inline]
fn u(n: usize, k: usize) -> u32 {
    CODEBOOKS.1[usize::from(CODEBOOKS.0[n]) + k]
}

#[inline]
fn v(n: usize, k: usize) -> u32 {
    u(n, k) + u(n, k + 1)
}

/// Invalid pulse-vector parameters, samples or entropy input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Dimensions, pulse count, index or buffer lengths are invalid.
    Argument,
    /// This codebook needs more than 32 bits and must be split.
    Codebook,
    /// Input contains a non-finite sample or gain.
    Sample,
    /// Entropy input or output failed.
    Entropy(entropy::Error),
}
impl From<entropy::Error> for Error {
    fn from(value: entropy::Error) -> Self {
        Self::Entropy(value)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument => formatter.write_str("invalid CELT pulse-vector argument"),
            Self::Codebook => formatter.write_str("CELT pulse codebook requires splitting"),
            Self::Sample => formatter.write_str("invalid CELT pulse-vector sample"),
            Self::Entropy(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for Error {}

/// One exact, 32-bit CWRS codebook with allocation-free indexing.
#[derive(Debug, Clone, Copy)]
pub struct Codebook {
    dimensions: usize,
    pulses: usize,
    total: u32,
}
impl Codebook {
    /// Select an exact codebook in constant time. Reject sizes larger than u32.
    pub fn new(dimensions: usize, pulses: usize) -> Result<Self, Error> {
        if !(1..=MAX_DIMENSIONS).contains(&dimensions) || pulses > MAX_PULSES {
            return Err(Error::Argument);
        }
        let row_length = usize::from(CODEBOOKS.0[dimensions + 1] - CODEBOOKS.0[dimensions]);
        if pulses + 1 >= row_length {
            return Err(Error::Codebook);
        }
        let total = v(dimensions, pulses);
        Ok(Self {
            dimensions,
            pulses,
            total,
        })
    }

    /// Number of distinct signed integer vectors in this codebook.
    pub fn total(&self) -> u32 {
        self.total
    }

    /// Convert a signed pulse vector to its RFC 6716 CWRS index.
    pub fn index_of(&self, vector: &[i16]) -> Result<u32, Error> {
        if vector.len() != self.dimensions
            || vector
                .iter()
                .map(|value| usize::from(value.unsigned_abs()))
                .sum::<usize>()
                != self.pulses
        {
            return Err(Error::Argument);
        }
        let mut index = 0;
        let mut k = 0;
        for (j, &value) in vector.iter().rev().enumerate() {
            let n = j + 1;
            if k > 0 {
                index += u(n, k);
            }
            k += usize::from(value.unsigned_abs());
            if value < 0 {
                index += u(n, k + 1);
            }
        }
        debug_assert!(index < self.total);
        Ok(index)
    }

    /// Recover a signed pulse vector from its RFC 6716 CWRS index.
    pub fn vector_at(&self, mut index: u32, output: &mut [i16]) -> Result<(), Error> {
        if output.len() != self.dimensions || index >= self.total {
            return Err(Error::Argument);
        }
        if self.dimensions == 1 {
            output[0] = if index == 0 {
                self.pulses as i16
            } else {
                -(self.pulses as i16)
            };
            return Ok(());
        }
        let mut k = self.pulses;
        for (j, value) in output.iter_mut().enumerate() {
            let n = self.dimensions - j;
            if k <= 1 {
                let tail = &mut output[j..];
                tail.fill(0);
                if k == 1 {
                    // V(N,1)=2N: positive unit vectors run forward,
                    // followed by negative unit vectors in reverse order.
                    let (position, pulse) = if index < n as u32 {
                        (index as usize, 1)
                    } else {
                        (2 * n - index as usize - 1, -1)
                    };
                    tail[position] = pulse;
                }
                return Ok(());
            }
            if n == 2 {
                // V(2,K)=4K. The positive-first half has 2K+1
                // entries; every nonzero remaining magnitude has two
                // signs. Invert that arithmetic progression directly.
                let midpoint = 2 * k as u32 + 1;
                let sign = if index < midpoint {
                    1
                } else {
                    index -= midpoint;
                    -1
                };
                let remaining = index.div_ceil(2) as usize;
                *value = sign * (k - remaining) as i16;
                output[j + 1] = if index > 0 && index.is_multiple_of(2) {
                    -(remaining as i16)
                } else {
                    remaining as i16
                };
                return Ok(());
            }
            let mut threshold = u(n, k + 1);
            let sign = if index < threshold {
                1
            } else {
                index -= threshold;
                -1
            };
            let initial = k;
            // Rearranging the U recurrence collapses RFC step 3 to
            // U(n,k), and each subtraction in step 4 to U(n,k-1).
            // This removes two table accesses per dimension.
            threshold = u(n, k);
            if threshold > index && k > 16 {
                if n == 3 {
                    // U(3,K)=2K²-2K+1, with U(3,0)=0. Its exact
                    // integer-root inverse needs no floating arithmetic.
                    k = if index == 0 {
                        0
                    } else {
                        (2 * index - 1).isqrt().div_ceil(2) as usize
                    };
                } else {
                    // High-density small bands can otherwise scan all 128
                    // pulses. U(n,K) is strictly increasing for n>=2.
                    let mut low = 0;
                    let mut high = k;
                    while low < high {
                        let middle = (low + high).div_ceil(2);
                        if u(n, middle) <= index {
                            low = middle;
                        } else {
                            high = middle - 1;
                        }
                    }
                    k = low;
                }
                threshold = u(n, k);
            } else {
                while threshold > index {
                    k -= 1;
                    threshold = u(n, k);
                }
            }
            *value = sign * (initial - k) as i16;
            index -= threshold;
        }
        debug_assert_eq!(k, 0);
        debug_assert_eq!(index, 0);
        Ok(())
    }

    /// Encode one vector with Opus's uniform integer entropy coding.
    #[cfg(test)]
    pub fn encode(&self, vector: &[i16], coder: &mut entropy::Encoder<'_>) -> Result<(), Error> {
        let index = self.index_of(vector)?;
        if self.total > 1 {
            coder.uniform(index, self.total)?;
        }
        Ok(())
    }

    /// Decode one vector with Opus's uniform integer entropy coding.
    #[cfg(test)]
    pub fn decode(
        &self,
        coder: &mut entropy::Decoder<'_>,
        output: &mut [i16],
    ) -> Result<(), Error> {
        if output.len() != self.dimensions {
            return Err(Error::Argument);
        }
        let index = if self.total > 1 {
            coder.uniform(self.total)?
        } else {
            0
        };
        self.vector_at(index, output)
    }
}

/// Quantize a spectrum to exactly `pulses` signed pulses, returning squared norm.
///
/// The independent search projects to K-1 pulses and adds each remaining pulse
/// by maximum normalized correlation. Ties choose the lowest bin. Zero spectra
/// receive all pulses in the first bin. Non-finite input is rejected before
/// changing output; finite f32 input uses f64 accumulation for numeric headroom.
pub fn quantize(input: &[f32], pulses: usize, output: &mut [i16]) -> Result<u32, Error> {
    if !(1..=MAX_DIMENSIONS).contains(&input.len())
        || output.len() != input.len()
        || pulses > MAX_PULSES
    {
        return Err(Error::Argument);
    }
    if input.iter().any(|value| !value.is_finite()) {
        return Err(Error::Sample);
    }
    output.fill(0);
    if pulses == 0 {
        return Ok(0);
    }
    let sum: f64 = input.iter().map(|value| f64::from(value.abs())).sum();
    if sum == 0.0 {
        output[0] = pulses as i16;
        return Ok((pulses * pulses) as u32);
    }
    let scale = (pulses - 1) as f64 / sum;
    let mut assigned = 0;
    let mut dot = 0.0;
    let mut norm = 0;
    for (&sample, value) in input.iter().zip(output.iter_mut()) {
        let magnitude = (f64::from(sample.abs()) * scale).floor() as usize;
        // Bound accumulated truncations even in the presence of floating
        // rounding near an integer projection boundary.
        let magnitude = magnitude.min(pulses - 1 - assigned);
        *value = magnitude as i16;
        assigned += magnitude;
        norm += (magnitude * magnitude) as u32;
        dot += f64::from(sample.abs()) * magnitude as f64;
    }
    while assigned < pulses {
        let mut selected = 0;
        let mut best_numerator = -1.0;
        let mut best_denominator = 1;
        for (j, (&sample, &value)) in input.iter().zip(output.iter()).enumerate() {
            let correlation = dot + f64::from(sample.abs());
            let numerator = correlation * correlation;
            let denominator = norm + 2 * value as u32 + 1;
            if numerator * f64::from(best_denominator) > best_numerator * f64::from(denominator) {
                selected = j;
                best_numerator = numerator;
                best_denominator = denominator;
            }
        }
        norm += 2 * output[selected] as u32 + 1;
        output[selected] += 1;
        dot += f64::from(input[selected].abs());
        assigned += 1;
    }
    for (&sample, value) in input.iter().zip(output.iter_mut()) {
        if sample < 0.0 {
            *value = -*value;
        }
    }
    Ok(norm)
}

/// Scale a finite vector to the requested L2 norm; zero vectors remain zero.
pub fn normalize(vector: &mut [f32], gain: f32) -> Result<(), Error> {
    if !(1..=MAX_DIMENSIONS).contains(&vector.len()) {
        return Err(Error::Argument);
    }
    if !gain.is_finite() || gain < 0.0 || vector.iter().any(|value| !value.is_finite()) {
        return Err(Error::Sample);
    }
    let energy: f64 = vector
        .iter()
        .map(|&value| f64::from(value) * f64::from(value))
        .sum();
    if energy == 0.0 {
        return Ok(());
    }
    let scale = f64::from(gain) / energy.sqrt();
    for value in vector {
        *value = (f64::from(*value) * scale) as f32;
    }
    Ok(())
}

/// Reconstruct a pulse vector at the requested L2 norm.
pub fn synthesize(pulses: &[i16], gain: f32, output: &mut [f32]) -> Result<(), Error> {
    if !(1..=MAX_DIMENSIONS).contains(&pulses.len()) || output.len() != pulses.len() {
        return Err(Error::Argument);
    }
    if !gain.is_finite() || gain < 0.0 {
        return Err(Error::Sample);
    }
    for (&pulse, value) in pulses.iter().zip(output.iter_mut()) {
        *value = f32::from(pulse);
    }
    normalize(output, gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codebook_counts_match_explicit_combinations() {
        // Choose m nonzero positions, a positive composition of K into m
        // parts, and one of 2^m signs. This independently counts the vectors.
        fn choose(n: usize, k: usize) -> u64 {
            (0..k).fold(1, |result, j| result * (n - j) as u64 / (j + 1) as u64)
        }
        for n in 1..=10 {
            for k in 1..=10 {
                let expected: u64 = (1..=n.min(k))
                    .map(|m| choose(n, m) * choose(k - 1, m - 1) * (1 << m))
                    .sum();
                if expected <= u64::from(u32::MAX) {
                    assert_eq!(u64::from(Codebook::new(n, k).unwrap().total()), expected);
                } else {
                    assert_eq!(Codebook::new(n, k).unwrap_err(), Error::Codebook);
                }
            }
        }
        for n in 1..=MAX_DIMENSIONS {
            assert_eq!(Codebook::new(n, 0).unwrap().total(), 1);
            assert_eq!(Codebook::new(n, 1).unwrap().total(), 2 * n as u32);
        }
    }

    #[test]
    fn all_small_codewords_have_exact_rank_and_pulse_count() {
        for n in 1..=6 {
            for k in 0..=5 {
                let book = Codebook::new(n, k).unwrap();
                let mut output = [0i16; MAX_DIMENSIONS];
                for index in 0..book.total() {
                    book.vector_at(index, &mut output[..n]).unwrap();
                    assert_eq!(book.index_of(&output[..n]).unwrap(), index, "n={n}, k={k}");
                    assert_eq!(
                        output[..n]
                            .iter()
                            .map(|v| usize::from(v.unsigned_abs()))
                            .sum::<usize>(),
                        k
                    );
                }
            }
        }
        let book = Codebook::new(2, 1).unwrap();
        for (index, expected) in [[1, 0], [0, 1], [0, -1], [-1, 0]].into_iter().enumerate() {
            let mut output = [0; 2];
            book.vector_at(index as u32, &mut output).unwrap();
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn large_valid_codebooks_survive_checked_arithmetic() {
        let mut output = [0; MAX_DIMENSIONS];
        for n in 1..=MAX_DIMENSIONS {
            for k in 0..=MAX_PULSES {
                let Ok(book) = Codebook::new(n, k) else {
                    continue;
                };
                for index in [0, book.total() / 2, book.total() - 1] {
                    book.vector_at(index, &mut output[..n]).unwrap();
                    assert_eq!(book.index_of(&output[..n]).unwrap(), index);
                }
            }
        }
    }

    #[test]
    fn multiple_vectors_share_one_entropy_frame() {
        let mut frame = [0; 128];
        let books = [Codebook::new(5, 4).unwrap(), Codebook::new(16, 8).unwrap()];
        let vectors = [[1, -1, 0, 2, 0], [0, 0, 0, 0, -4]];
        let mut coder = entropy::Encoder::new(&mut frame);
        for vector in vectors {
            books[0].encode(&vector, &mut coder).unwrap();
        }
        let mut long = [0; 16];
        long[15] = -8;
        books[1].encode(&long, &mut coder).unwrap();
        let bytes = coder.finish().unwrap();
        let mut coder = entropy::Decoder::new(&frame[..bytes]);
        for expected in vectors {
            let mut result = [0; 5];
            books[0].decode(&mut coder, &mut result).unwrap();
            assert_eq!(result, expected);
        }
        let mut result = [0; 16];
        books[1].decode(&mut coder, &mut result).unwrap();
        assert_eq!(result, long);
    }

    #[test]
    fn installed_reference_cwrs_vectors_match_exactly() {
        // Generated by tools/reference/generate_opus_pvq_fixtures.py through
        // external libopus functions only. It checks every oracle codebook
        // before saving dimension, pulse-count and rank boundary samples.
        let data = include_bytes!("testdata/pvq-libopus-1.6.1.bin");
        assert_eq!(&data[..8], b"SNKPVQ01");
        let count = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let mut cursor = 12;
        let mut expected = [0i16; MAX_DIMENSIONS];
        let mut output = [0i16; MAX_DIMENSIONS];
        for case in 0..count {
            let n = usize::from(u16::from_le_bytes(
                data[cursor..cursor + 2].try_into().unwrap(),
            ));
            let k = usize::from(u16::from_le_bytes(
                data[cursor + 2..cursor + 4].try_into().unwrap(),
            ));
            cursor += 4;
            let numbers: [u32; 5] = std::array::from_fn(|j| {
                u32::from_le_bytes(data[cursor + j * 4..cursor + j * 4 + 4].try_into().unwrap())
            });
            cursor += 20;
            let packet = &data[cursor..cursor + 8];
            cursor += 8;
            for value in &mut expected[..n] {
                *value = i16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
                cursor += 2;
            }
            let book = Codebook::new(n, k).unwrap();
            assert_eq!(book.total(), numbers[0], "case {case}: N={n}, K={k}");
            assert_eq!(book.index_of(&expected[..n]).unwrap(), numbers[1]);
            book.vector_at(numbers[1], &mut output[..n]).unwrap();
            assert_eq!(output[..n], expected[..n]);
            let mut decoder = entropy::Decoder::new(packet);
            book.decode(&mut decoder, &mut output[..n]).unwrap();
            assert_eq!(output[..n], expected[..n]);
            assert_eq!(decoder.range(), numbers[2]);
            assert_eq!(decoder.tell(), u64::from(numbers[3]));
            assert_eq!(decoder.tell_eighths(), u64::from(numbers[4]));
            let mut bytes = [0; 8];
            let mut encoder = entropy::Encoder::new(&mut bytes);
            book.encode(&expected[..n], &mut encoder).unwrap();
            assert_eq!(encoder.range(), numbers[2]);
            assert_eq!(encoder.tell(), u64::from(numbers[3]));
            assert_eq!(encoder.tell_eighths(), u64::from(numbers[4]));
            encoder.finish_padded().unwrap();
            assert_eq!(bytes.as_slice(), packet);
        }
        assert_eq!(cursor, data.len());
    }

    #[test]
    fn quantization_preserves_pulses_signs_and_numeric_headroom() {
        let mut output = [0; MAX_DIMENSIONS];
        for n in [1, 2, 8, 22, 176] {
            let input: Vec<_> = (0..n).map(|j| (j as f32 * 0.71).sin()).collect();
            for k in 0..=MAX_PULSES {
                let norm = quantize(&input, k, &mut output[..n]).unwrap();
                assert_eq!(
                    output[..n]
                        .iter()
                        .map(|v| usize::from(v.unsigned_abs()))
                        .sum::<usize>(),
                    k
                );
                assert_eq!(
                    norm,
                    output[..n]
                        .iter()
                        .map(|&v| (i32::from(v) * i32::from(v)) as u32)
                        .sum()
                );
                assert!(
                    input
                        .iter()
                        .zip(&output[..n])
                        .all(|(&x, &y)| x * f32::from(y) >= 0.0)
                );
            }
        }
        let norm = quantize(&[f32::MAX, -f32::MAX], 128, &mut output[..2]).unwrap();
        assert_eq!(&output[..2], &[64, -64]);
        assert_eq!(norm, 8192);
        output[..2].fill(7);
        assert_eq!(
            quantize(&[0.0, f32::NAN], 4, &mut output[..2]),
            Err(Error::Sample)
        );
        assert_eq!(&output[..2], &[7, 7]);
    }

    #[test]
    fn rejects_invalid_arguments_without_changing_output() {
        assert_eq!(Codebook::new(0, 1).unwrap_err(), Error::Argument);
        assert_eq!(Codebook::new(176, 128).unwrap_err(), Error::Codebook);
        let book = Codebook::new(2, 1).unwrap();
        let mut output = [7, 7];
        assert_eq!(
            book.vector_at(book.total(), &mut output),
            Err(Error::Argument)
        );
        assert_eq!(output, [7, 7]);
        assert_eq!(book.index_of(&[i16::MIN, 0]), Err(Error::Argument));
    }
}
