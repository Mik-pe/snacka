// SPDX-License-Identifier: MIT
//! Range coding and reverse raw-bit packing from RFC 6716 sections 4.1 and 5.1.
//!
//! Coding uses a 31-bit interval with byte normalization. The frame's arithmetic
//! prefix and raw-bit suffix may overlap; zero extension of the prefix is required
//! by the format, while raw-bit reads remain bounded to the actual frame.

use std::fmt;

/// Invalid model, symbol or bitstream input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Cumulative probabilities are invalid or an argument is outside its domain.
    Argument,
    /// A requested raw-bit read exceeds the encoded frame.
    Truncated,
    /// A decoded value lies outside the encoded alphabet.
    Corrupt,
    /// The caller's output buffer is too small.
    Output,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Argument => "invalid entropy coding argument",
            Self::Truncated => "raw bits exceed the encoded frame",
            Self::Corrupt => "invalid entropy coded symbol",
            Self::Output => "entropy output buffer is too small",
        })
    }
}
impl std::error::Error for Error {}

/// A validated cumulative probability distribution.
///
/// Boundaries start at zero and strictly increase to a total no larger than
/// 65,535. A model borrows its immutable table and can be reused across streams.
#[derive(Debug, Clone, Copy)]
pub struct Model<'a> {
    boundaries: &'a [u16],
    total: u32,
}
impl<'a> Model<'a> {
    /// Validate cumulative boundaries once, before using the model in a stream.
    pub const fn new(boundaries: &'a [u16]) -> Result<Self, Error> {
        if boundaries.len() < 2 || boundaries[0] != 0 {
            return Err(Error::Argument);
        }
        let mut index = 1;
        while index < boundaries.len() {
            if boundaries[index - 1] >= boundaries[index] {
                return Err(Error::Argument);
            }
            index += 1;
        }
        Ok(Self {
            boundaries,
            total: boundaries[boundaries.len() - 1] as u32,
        })
    }

    /// Number of symbols in this model.
    pub fn symbols(&self) -> usize {
        self.boundaries.len() - 1
    }
}

#[cfg(feature = "opus")]
impl Model<'static> {
    /// Validate a fixed distribution during constant evaluation.
    ///
    /// Keep the result in a `const` or `static` to avoid revalidating it in a
    /// per-symbol audio path. Invalid normative tables fail to compile.
    pub(crate) const fn from_static(boundaries: &'static [u16]) -> Self {
        match Self::new(boundaries) {
            Ok(model) => model,
            Err(_) => panic!("invalid static entropy probability model"),
        }
    }
}

const NORMALIZE_BELOW: u32 = 1 << 23;
const INTERVAL_MASK: u32 = (1 << 31) - 1;

#[inline]
fn tell_fractional(total_bits: u64, range: u32) -> u64 {
    let integer = 32 - range.leading_zeros();
    let mut fraction = 0;
    let mut normalized = range >> (integer - 16);
    // Three binary logarithm steps produce eighth-bit precision without floats.
    for _ in 0..3 {
        normalized = (normalized * normalized) >> 15;
        let bit = normalized >> 16;
        normalized >>= bit;
        fraction = (fraction << 1) | bit;
    }
    total_bits * 8 - u64::from(integer * 8 + fraction)
}

/// A borrowed, allocation-free range decoder for one encoded entropy frame.
pub struct Decoder<'a> {
    frame: &'a [u8],
    front: usize,
    carry: u8,
    range: u32,
    value: u32,
    total_bits: u64,
    tail: usize,
    raw_buffer: u64,
    raw_available: u32,
    raw_consumed: usize,
}

impl<'a> Decoder<'a> {
    /// Initialize a frame, including the format's required zero extension.
    pub fn new(frame: &'a [u8]) -> Self {
        let first = frame.first().copied().unwrap_or(0);
        let mut decoder = Self {
            frame,
            front: usize::from(!frame.is_empty()),
            carry: first & 1,
            range: 128,
            value: u32::from(127 - (first >> 1)),
            total_bits: 9,
            tail: frame.len(),
            raw_buffer: 0,
            raw_available: 0,
            raw_consumed: 0,
        };
        decoder.normalize();
        decoder
    }

    /// Exclude trailing bytes before the first raw-bit read, preserving the
    /// arithmetic interval and its already-buffered lookahead.
    ///
    /// Opus hybrid frames use this after decoding the SILK layer to separate
    /// redundant audio from the CELT raw-bit suffix. Invalid lengths and calls
    /// after consuming raw bits leave every decoder field unchanged.
    pub fn shrink(&mut self, length: usize) -> Result<(), Error> {
        if length > self.frame.len() || self.raw_consumed != 0 || self.raw_available != 0 {
            return Err(Error::Argument);
        }
        self.frame = &self.frame[..length];
        self.tail = length;
        self.front = self.front.min(length);
        Ok(())
    }

    fn normalize(&mut self) {
        while self.range <= NORMALIZE_BELOW {
            let next = if self.front < self.frame.len() {
                let byte = self.frame[self.front];
                self.front += 1;
                byte
            } else {
                0
            };
            let symbol = (self.carry << 7) | (next >> 1);
            self.carry = next & 1;
            self.range <<= 8;
            self.value = ((self.value << 8) | u32::from(255 - symbol)) & INTERVAL_MASK;
            self.total_bits += 8;
        }
    }

    #[cfg(all(test, feature = "opus"))]
    #[inline]
    pub(crate) fn frequency(&self, total: u32) -> u32 {
        self.frequency_with_unit(total, self.range / total)
    }

    /// Select and consume one custom Opus probability interval using one unit.
    /// Invalid intervals leave decoder state intact.
    #[cfg(feature = "opus")]
    #[inline]
    pub(crate) fn inverse_interval<T>(
        &mut self,
        total: u32,
        locate: impl FnOnce(u32, u32) -> (T, u32, u32),
    ) -> Result<T, Error> {
        if total == 0 || total > 65_535 {
            return Err(Error::Argument);
        }
        let unit = self.range / total;
        let (symbol, low, high) = locate(self.value, unit);
        if low >= high || high > total {
            return Err(Error::Argument);
        }
        // The first interval includes the division remainder. Checking its
        // inverse thresholds also avoids dividing the value a second time.
        if self.value < unit * (total - high) || (low != 0 && self.value >= unit * (total - low)) {
            return Err(Error::Corrupt);
        }
        self.consume_with_unit(low, high, total, unit);
        Ok(symbol)
    }

    #[inline]
    fn frequency_with_unit(&self, total: u32, unit: u32) -> u32 {
        total - (self.value / unit + 1).min(total)
    }

    #[cfg(all(test, feature = "opus"))]
    #[inline]
    pub(crate) fn consume(&mut self, low: u32, high: u32, total: u32) {
        let unit = self.range / total;
        self.consume_with_unit(low, high, total, unit);
    }

    /// Decode an already validated inverse-CDF model for the Opus wrapper.
    #[cfg(feature = "opus")]
    #[inline]
    pub(crate) fn inverse_symbol(&mut self, table: &[u8], bits: u32) -> usize {
        let unit = self.range >> bits;
        if table.len() <= 16 {
            let mut previous = self.range;
            for (symbol, &tail) in table.iter().enumerate() {
                let upper = unit * u32::from(tail);
                if upper <= self.value {
                    self.value -= upper;
                    self.range = previous - upper;
                    self.normalize();
                    return symbol;
                }
                previous = upper;
            }
            unreachable!("validated inverse cumulative table ends at zero");
        }
        let total = 1 << bits;
        let symbol = table.partition_point(|&tail| unit * u32::from(tail) > self.value);
        let low = if symbol == 0 {
            0
        } else {
            total - u32::from(table[symbol - 1])
        };
        let high = total - u32::from(table[symbol]);
        self.consume_with_unit(low, high, total, unit);
        symbol
    }

    #[inline(always)]
    fn consume_with_unit(&mut self, low: u32, high: u32, total: u32, unit: u32) {
        let upper = unit * (total - high);
        self.value -= upper;
        self.range = if low == 0 {
            self.range - upper
        } else {
            unit * (high - low)
        };
        self.normalize();
    }

    /// Decode a symbol using a previously validated cumulative model.
    #[inline]
    pub fn symbol(&mut self, model: Model<'_>) -> usize {
        if model.total == 256 {
            // RFC 6716 section 4.1.3.3 permits comparing inverse cumulative
            // thresholds directly. The SILK alphabet's fixed denominator
            // removes both divisions; its small tables favor a linear scan.
            let unit = self.range >> 8;
            let mut index = 0;
            // Keep the preceding inverse boundary, including the interval's
            // division remainder for symbol zero. Their difference is exactly
            // the selected interval width, without a second multiplication.
            let mut previous = self.range;
            loop {
                let upper = unit * (256 - u32::from(model.boundaries[index + 1]));
                if upper <= self.value {
                    self.value -= upper;
                    self.range = previous - upper;
                    self.normalize();
                    return index;
                }
                previous = upper;
                index += 1;
            }
        }
        let unit = self.range / model.total;
        // Searching inverse thresholds also avoids value/unit for arbitrary
        // totals. Large alphabets retain a logarithmic search.
        let index = model.boundaries[1..]
            .partition_point(|&high| unit * (model.total - u32::from(high)) > self.value);
        self.consume_with_unit(
            u32::from(model.boundaries[index]),
            u32::from(model.boundaries[index + 1]),
            model.total,
            unit,
        );
        index
    }

    // SILK's binary models have an already validated denominator of 256.
    // Return symbol one; symbol zero retains the interval division remainder.
    #[cfg(feature = "opus")]
    #[inline]
    pub(crate) fn binary_256(&mut self, zero_frequency: u16) -> bool {
        debug_assert!((1..256).contains(&zero_frequency));
        let upper = (self.range >> 8) * (256 - u32::from(zero_frequency));
        let one = self.value < upper;
        if one {
            self.range = upper;
        } else {
            self.value -= upper;
            self.range -= upper;
        }
        self.normalize();
        one
    }

    /// Decode a bit whose probability of being one is `2^-log_probability`.
    ///
    /// Supported logarithms are 1 through 16. Invalid arguments leave state intact.
    #[inline]
    pub fn bit(&mut self, log_probability: u32) -> Result<bool, Error> {
        if !(1..=16).contains(&log_probability) {
            return Err(Error::Argument);
        }
        let unit = self.range >> log_probability;
        let one = self.value < unit;
        if one {
            self.range = unit;
        } else {
            self.value -= unit;
            self.range -= unit;
        }
        self.normalize();
        Ok(one)
    }

    /// Decode up to 32 raw bits from the frame's suffix, least significant bit first.
    ///
    /// Invalid arguments and truncated reads leave state intact.
    #[inline]
    pub fn raw(&mut self, count: u32) -> Result<u32, Error> {
        if count > 32 {
            return Err(Error::Argument);
        }
        if count as usize
            > self
                .frame
                .len()
                .saturating_mul(8)
                .saturating_sub(self.raw_consumed)
        {
            return Err(Error::Truncated);
        }
        while self.raw_available < count {
            self.tail -= 1;
            self.raw_buffer |= u64::from(self.frame[self.tail]) << self.raw_available;
            self.raw_available += 8;
        }
        let value = (self.raw_buffer & ((1u64 << count) - 1)) as u32;
        self.raw_buffer >>= count;
        self.raw_available -= count;
        self.raw_consumed += count as usize;
        self.total_bits += u64::from(count);
        Ok(value)
    }

    /// Decode an equiprobable integer from zero up to `total - 1`.
    #[inline]
    pub fn uniform(&mut self, total: u32) -> Result<u32, Error> {
        if total == 0 {
            return Err(Error::Argument);
        }
        if total == 1 {
            return Ok(0);
        }
        let low_bits = (32 - (total - 1).leading_zeros()).saturating_sub(8);
        let high_total = ((total - 1) >> low_bits) + 1;
        let unit = self.range / high_total;
        let high = self.frequency_with_unit(high_total, unit);
        self.consume_with_unit(high, high + 1, high_total, unit);
        if low_bits == 0 {
            return Ok(high);
        }
        let value = (high << low_bits) | self.raw(low_bits)?;
        if value >= total {
            Err(Error::Corrupt)
        } else {
            Ok(value)
        }
    }

    /// Current interval size, suitable for comparison with an independent decoder.
    pub fn range(&self) -> u32 {
        self.range
    }

    /// Conservative bit usage rounded up to a whole bit, including the initial bit.
    pub fn tell(&self) -> u64 {
        self.total_bits - u64::from(32 - self.range.leading_zeros())
    }

    /// Conservative bit usage in units of one eighth of a bit.
    pub fn tell_eighths(&self) -> u64 {
        tell_fractional(self.total_bits, self.range)
    }
}

/// An allocation-free range encoder into a caller-owned packet buffer.
///
/// Buffer exhaustion is terminal for this encoder. Use a new encoder and buffer
/// for a retry; an incomplete frame must never be sent to a decoder.
pub struct Encoder<'a> {
    output: &'a mut [u8],
    front: usize,
    tail: usize,
    pending: Option<u8>,
    deferred: usize,
    low: u32,
    range: u32,
    total_bits: u64,
    raw_buffer: u64,
    raw_available: u32,
    failure: Option<Error>,
}

impl<'a> Encoder<'a> {
    /// Begin a new frame with a caller-owned output buffer.
    pub fn new(output: &'a mut [u8]) -> Self {
        let tail = output.len();
        Self {
            output,
            front: 0,
            tail,
            pending: None,
            deferred: 0,
            low: 0,
            range: 1 << 31,
            total_bits: 33,
            raw_buffer: 0,
            raw_available: 0,
            failure: None,
        }
    }

    fn write_front(&mut self, byte: u8) -> Result<(), Error> {
        if self.front >= self.tail {
            self.failure = Some(Error::Output);
            return Err(Error::Output);
        }
        self.output[self.front] = byte;
        self.front += 1;
        Ok(())
    }

    fn emit(&mut self, digit: u32) -> Result<(), Error> {
        if digit == 255 {
            self.deferred += 1;
            return Ok(());
        }
        let carry = (digit >> 8) as u8;
        if let Some(pending) = self.pending {
            self.write_front(pending.wrapping_add(carry))?;
        }
        let fill = if carry == 0 { 255 } else { 0 };
        while self.deferred > 0 {
            self.write_front(fill)?;
            self.deferred -= 1;
        }
        self.pending = Some(digit as u8);
        Ok(())
    }

    #[cold]
    fn normalize_after_output_failure(&mut self) {
        // Output failure is terminal, but public interval and bit counters
        // remain readable. Complete arithmetic normalization without writes.
        while self.range <= NORMALIZE_BELOW {
            self.low = (self.low << 8) & INTERVAL_MASK;
            self.range <<= 8;
            self.total_bits += 8;
        }
    }

    pub(crate) fn interval(&mut self, low: u32, high: u32, total: u32) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let unit = self.range / total;
        if low == 0 {
            self.range -= unit * (total - high);
        } else {
            self.low += self.range - unit * (total - low);
            self.range = unit * (high - low);
        }
        while self.range <= NORMALIZE_BELOW {
            if let Err(error) = self.emit(self.low >> 23) {
                self.normalize_after_output_failure();
                return Err(error);
            }
            self.low = (self.low << 8) & INTERVAL_MASK;
            self.range <<= 8;
            self.total_bits += 8;
        }
        Ok(())
    }

    /// Encode a symbol using a previously validated cumulative model.
    pub fn symbol(&mut self, symbol: usize, model: Model<'_>) -> Result<(), Error> {
        let bounds = model
            .boundaries
            .get(symbol..)
            .and_then(|tail| tail.get(..2))
            .ok_or(Error::Argument)?;
        self.interval(u32::from(bounds[0]), u32::from(bounds[1]), model.total)
    }

    /// Encode a bit whose probability of being one is `2^-log_probability`.
    pub fn bit(&mut self, one: bool, log_probability: u32) -> Result<(), Error> {
        if !(1..=16).contains(&log_probability) {
            return Err(Error::Argument);
        }
        let total = 1 << log_probability;
        if one {
            self.interval(total - 1, total, total)
        } else {
            self.interval(0, total - 1, total)
        }
    }

    /// Encode up to 32 raw bits into the frame's suffix.
    pub fn raw(&mut self, value: u32, count: u32) -> Result<(), Error> {
        if count > 32 || u64::from(value) >= (1u64 << count) {
            return Err(Error::Argument);
        }
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.raw_buffer |= u64::from(value) << self.raw_available;
        self.raw_available += count;
        self.total_bits += u64::from(count);
        while self.raw_available >= 8 {
            if self.tail == self.front {
                self.failure = Some(Error::Output);
                return Err(Error::Output);
            }
            self.tail -= 1;
            self.output[self.tail] = self.raw_buffer as u8;
            self.raw_buffer >>= 8;
            self.raw_available -= 8;
        }
        Ok(())
    }

    /// Encode an equiprobable integer from zero up to `total - 1`.
    pub fn uniform(&mut self, value: u32, total: u32) -> Result<(), Error> {
        if total == 0 || value >= total {
            return Err(Error::Argument);
        }
        if total == 1 {
            return self.failure.map_or(Ok(()), Err);
        }
        let low_bits = (32 - (total - 1).leading_zeros()).saturating_sub(8);
        let high_total = ((total - 1) >> low_bits) + 1;
        let high = value >> low_bits;
        self.interval(high, high + 1, high_total)?;
        self.raw(value & ((1u32 << low_bits) - 1), low_bits)
    }

    /// Current interval size, suitable for comparison with an independent encoder.
    pub fn range(&self) -> u32 {
        self.range
    }

    /// Conservative bit usage rounded up to a whole bit, including the initial bit.
    pub fn tell(&self) -> u64 {
        self.total_bits - u64::from(32 - self.range.leading_zeros())
    }

    /// Conservative bit usage in units of one eighth of a bit.
    pub fn tell_eighths(&self) -> u64 {
        tell_fractional(self.total_bits, self.range)
    }

    /// Terminate and compact the frame; return its encoded length in bytes.
    ///
    /// The returned prefix of the caller's buffer is the entire encoded frame.
    pub fn finish(self) -> Result<usize, Error> {
        self.terminate(false)
    }

    /// Terminate a frame using the entire caller-provided buffer.
    ///
    /// Zero padding separates the arithmetic prefix from the raw suffix, which
    /// remains at the end of the buffer. The result is suitable for fixed budgets.
    pub fn finish_padded(self) -> Result<usize, Error> {
        self.terminate(true)
    }

    fn terminate(mut self, padded: bool) -> Result<usize, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let mut bits = self.range.leading_zeros();
        let mut mask = INTERVAL_MASK >> bits;
        let mut end = (self.low + mask) & !mask;
        if (end | mask) >= self.low + self.range {
            bits += 1;
            mask >>= 1;
            end = (self.low + mask) & !mask;
        }
        let mut remaining = bits as i32;
        while remaining > 0 {
            self.emit(end >> 23)?;
            end = (end << 8) & INTERVAL_MASK;
            remaining -= 8;
        }
        if self.pending.is_some() || self.deferred > 0 {
            self.emit(0)?;
        }
        let free = (-remaining) as u32;
        let extra = usize::from(self.raw_available > free);
        if self.front + extra > self.tail {
            return Err(Error::Output);
        }
        if padded {
            self.output[self.front..self.tail].fill(0);
            if self.raw_available != 0 {
                self.output[self.tail - 1] |= self.raw_buffer as u8;
            }
            return Ok(self.output.len());
        }
        let raw_bytes = self.output.len() - self.tail;
        let length = self.front + extra + raw_bytes;
        if extra != 0 {
            self.output[self.front] = 0;
        }
        self.output.copy_within(self.tail.., self.front + extra);
        if self.raw_available != 0 {
            self.output[self.front + extra - 1] |= self.raw_buffer as u8;
        }
        Ok(length)
    }
}

#[cfg(all(test, feature = "opus"))]
mod binary_tests {
    use super::*;

    #[test]
    fn binary_256_matches_models_with_raw_bits_and_zero_extension() {
        let bytes: [u8; 31] = std::array::from_fn(|n| (n as u8).wrapping_mul(73).wrapping_add(51));
        for length in 0..=bytes.len() {
            for weight in 1..256 {
                let boundaries = [0, weight, 256];
                let model = Model::new(&boundaries).unwrap();
                let mut ordinary = Decoder::new(&bytes[..length]);
                let mut binary = Decoder::new(&bytes[..length]);
                for index in 0..512 {
                    assert_eq!(
                        ordinary.symbol(model),
                        usize::from(binary.binary_256(weight))
                    );
                    if index % 7 == 0 {
                        let bits = (index % 32 + 1) as u32;
                        assert_eq!(ordinary.raw(bits), binary.raw(bits));
                    }
                    assert_eq!(
                        (
                            ordinary.front,
                            ordinary.carry,
                            ordinary.range,
                            ordinary.value,
                            ordinary.total_bits,
                            ordinary.tail,
                            ordinary.raw_buffer,
                            ordinary.raw_available,
                            ordinary.raw_consumed
                        ),
                        (
                            binary.front,
                            binary.carry,
                            binary.range,
                            binary.value,
                            binary.total_bits,
                            binary.tail,
                            binary.raw_buffer,
                            binary.raw_available,
                            binary.raw_consumed
                        ),
                    );
                }
            }
        }
    }
}
