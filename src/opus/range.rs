// SPDX-License-Identifier: MIT
//! Opus probability models over Snacka's shared RFC 6716 entropy coder.
//!
//! The byte interval, reverse raw-bit suffix, normalization and finalization
//! remain owned by [`crate::entropy`]. This module adds inverse cumulative
//! tables and the integer Laplace model needed by CELT's coarse energies.
//! The Laplace rounding and finite tail were independently established against
//! the installed reference library through its external C test interface.

pub(crate) use crate::entropy::Error;

const LAPLACE_TOTAL: u32 = 32_768;
const LAPLACE_TAIL_RESERVE: u32 = 32;

/// A validated inverse cumulative probability table, with zero-probability
/// interior symbols permitted by RFC 6716 section 4.1.3.3.
#[derive(Clone, Copy)]
pub(crate) struct Icdf<'a> {
    table: &'a [u8],
    total: u32,
    bits: u32,
}

impl<'a> Icdf<'a> {
    pub(crate) fn new(table: &'a [u8], bits: u32) -> Result<Self, Error> {
        if !(1..=8).contains(&bits) || table.is_empty() {
            return Err(Error::Argument);
        }
        let total = 1 << bits;
        if u32::from(table[0]) >= total
            || table.last() != Some(&0)
            || table.windows(2).any(|pair| pair[0] < pair[1])
        {
            return Err(Error::Argument);
        }
        Ok(Self { table, total, bits })
    }

    fn interval(self, symbol: usize) -> Result<(u32, u32), Error> {
        let high = self
            .table
            .get(symbol)
            .map(|&tail| self.total - u32::from(tail))
            .ok_or(Error::Argument)?;
        let low = if symbol == 0 {
            0
        } else {
            self.total - u32::from(self.table[symbol - 1])
        };
        if low == high {
            return Err(Error::Argument);
        }
        Ok((low, high))
    }
}

#[derive(Clone, Copy)]
struct Laplace {
    zero: u32,
    decay: u32,
}

impl Laplace {
    fn new(zero: u32, decay: u32) -> Result<Self, Error> {
        if zero == 0 || zero > LAPLACE_TOTAL - LAPLACE_TAIL_RESERVE || decay > 16_384 {
            return Err(Error::Argument);
        }
        Ok(Self { zero, decay })
    }

    fn first(self) -> u32 {
        1 + (((LAPLACE_TOTAL - LAPLACE_TAIL_RESERVE - self.zero) * (16_384 - self.decay)) >> 15)
    }

    fn next(self, frequency: u32) -> u32 {
        1 + (((frequency - 1) * self.decay) >> 14)
    }

    fn interval(self, value: i32) -> (i32, u32, u32) {
        if value == 0 {
            return (0, 0, self.zero);
        }
        let requested = value.unsigned_abs();
        let mut magnitude = 1u32;
        let mut low = self.zero;
        let mut frequency = self.first();
        while magnitude < requested && frequency > 1 {
            low += 2 * frequency;
            magnitude += 1;
            frequency = self.next(frequency);
        }
        if frequency == 1 {
            let remaining = LAPLACE_TOTAL - low;
            // The negative member precedes the positive member of each pair.
            // An odd residual gives the final negative member one extra value.
            let pairs = (remaining + u32::from(value < 0)) / 2;
            let extra = (requested - magnitude).min(pairs - 1);
            magnitude += extra;
            low += 2 * extra;
        }
        if value > 0 {
            low += frequency;
        }
        let actual = if value < 0 {
            -(magnitude as i32)
        } else {
            magnitude as i32
        };
        (actual, low, low + frequency)
    }

    #[cfg(test)]
    fn locate(self, frequency_value: u32) -> (i32, u32, u32) {
        if frequency_value < self.zero {
            return (0, 0, self.zero);
        }
        let mut magnitude = 1u32;
        let mut low = self.zero;
        let mut frequency = self.first();
        while frequency > 1 && frequency_value >= low + 2 * frequency {
            low += 2 * frequency;
            magnitude += 1;
            frequency = self.next(frequency);
        }
        if frequency == 1 {
            let extra = (frequency_value - low) / 2;
            magnitude += extra;
            low += 2 * extra;
        }
        let positive = frequency_value >= low + frequency;
        if positive {
            low += frequency;
        }
        let actual = if positive {
            magnitude as i32
        } else {
            -(magnitude as i32)
        };
        (actual, low, low + frequency)
    }

    fn locate_inverse(self, value: u32, unit: u32) -> (i32, u32, u32) {
        if value >= unit * (LAPLACE_TOTAL - self.zero) {
            return (0, 0, self.zero);
        }
        let mut magnitude = 1u32;
        let mut low = self.zero;
        let mut frequency = self.first();
        // Replacing each frequency comparison with its inverse threshold
        // avoids value/unit for the common geometric part of this alphabet.
        while frequency > 1 && value < unit * (LAPLACE_TOTAL - low - 2 * frequency) {
            low += 2 * frequency;
            magnitude += 1;
            frequency = self.next(frequency);
        }
        if frequency == 1 {
            let frequency_value = LAPLACE_TOTAL - (value / unit + 1).min(LAPLACE_TOTAL);
            let extra = (frequency_value - low) / 2;
            magnitude += extra;
            low += 2 * extra;
        }
        let positive = value < unit * (LAPLACE_TOTAL - low - frequency);
        if positive {
            low += frequency;
        }
        let actual = if positive {
            magnitude as i32
        } else {
            -(magnitude as i32)
        };
        (actual, low, low + frequency)
    }
}

pub(crate) struct Encoder<'a> {
    coder: crate::entropy::Encoder<'a>,
}

impl<'a> Encoder<'a> {
    pub(crate) fn new(output: &'a mut [u8]) -> Self {
        Self {
            coder: crate::entropy::Encoder::new(output),
        }
    }

    pub(crate) fn bit(&mut self, one: bool, logp: u32) -> Result<(), Error> {
        self.coder.bit(one, logp)
    }

    pub(crate) fn raw(&mut self, value: u32, bits: u32) -> Result<(), Error> {
        self.coder.raw(value, bits)
    }

    pub(crate) fn uniform(&mut self, value: u32, total: u32) -> Result<(), Error> {
        self.coder.uniform(value, total)
    }

    pub(crate) fn interval(&mut self, low: u32, high: u32, total: u32) -> Result<(), Error> {
        if total == 0 || total > 65_535 || low >= high || high > total {
            return Err(Error::Argument);
        }
        self.coder.interval(low, high, total)
    }

    pub(crate) fn icdf(&mut self, symbol: usize, table: &[u8], bits: u32) -> Result<(), Error> {
        self.symbol(symbol, Icdf::new(table, bits)?)
    }

    pub(crate) fn symbol(&mut self, symbol: usize, model: Icdf<'_>) -> Result<(), Error> {
        let (low, high) = model.interval(symbol)?;
        self.coder.interval(low, high, model.total)
    }

    pub(crate) fn laplace(&mut self, value: i32, zero: u32, decay: u32) -> Result<i32, Error> {
        let (actual, low, high) = Laplace::new(zero, decay)?.interval(value);
        self.coder.interval(low, high, LAPLACE_TOTAL)?;
        Ok(actual)
    }

    pub(crate) fn range(&self) -> u32 {
        self.coder.range()
    }
    pub(crate) fn tell(&self) -> u64 {
        self.coder.tell()
    }
    pub(crate) fn tell_frac(&self) -> u64 {
        self.coder.tell_eighths()
    }
    pub(crate) fn finish_padded(self) -> Result<usize, Error> {
        self.coder.finish_padded()
    }
    pub(crate) fn finish(self) -> Result<usize, Error> {
        self.coder.finish()
    }
}

pub(crate) struct Decoder<'a> {
    coder: crate::entropy::Decoder<'a>,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(frame: &'a [u8]) -> Self {
        Self {
            coder: crate::entropy::Decoder::new(frame),
        }
    }

    pub(crate) fn bit(&mut self, logp: u32) -> Result<bool, Error> {
        self.coder.bit(logp)
    }
    pub(crate) fn raw(&mut self, bits: u32) -> Result<u32, Error> {
        self.coder.raw(bits)
    }
    pub(crate) fn uniform(&mut self, total: u32) -> Result<u32, Error> {
        self.coder.uniform(total)
    }

    #[cfg(test)]
    pub(crate) fn frequency(&self, total: u32) -> Result<u32, Error> {
        if total == 0 || total > 65_535 {
            return Err(Error::Argument);
        }
        Ok(self.coder.frequency(total))
    }

    #[cfg(test)]
    pub(crate) fn update(&mut self, low: u32, high: u32, total: u32) -> Result<(), Error> {
        if total == 0 || total > 65_535 || low >= high || high > total {
            return Err(Error::Argument);
        }
        let frequency = self.coder.frequency(total);
        if frequency < low || frequency >= high {
            return Err(Error::Corrupt);
        }
        self.coder.consume(low, high, total);
        Ok(())
    }

    /// Locate a custom model's symbol and consume its interval without
    /// recomputing the range unit. The closure receives its cumulative bin.
    pub(crate) fn mapped_interval<T>(
        &mut self,
        total: u32,
        locate: impl FnOnce(u32) -> (T, u32, u32),
    ) -> Result<T, Error> {
        self.coder.inverse_interval(total, |value, unit| {
            locate(total - (value / unit + 1).min(total))
        })
    }

    pub(crate) fn icdf(&mut self, table: &[u8], bits: u32) -> Result<usize, Error> {
        Ok(self.symbol(Icdf::new(table, bits)?))
    }

    pub(crate) fn symbol(&mut self, model: Icdf<'_>) -> usize {
        // Comparing inverse thresholds selects exactly the same interval as
        // frequency(), while avoiding the value/unit division and reloading
        // the model's cumulative boundaries during its canonical update.
        self.coder.inverse_symbol(model.table, model.bits)
    }

    pub(crate) fn laplace(&mut self, zero: u32, decay: u32) -> Result<i32, Error> {
        let model = Laplace::new(zero, decay)?;
        self.coder.inverse_interval(LAPLACE_TOTAL, |inverse, unit| {
            model.locate_inverse(inverse, unit)
        })
    }

    pub(crate) fn range(&self) -> u32 {
        self.coder.range()
    }
    pub(crate) fn tell(&self) -> u64 {
        self.coder.tell()
    }
    pub(crate) fn tell_frac(&self) -> u64 {
        self.coder.tell_eighths()
    }
}

impl<'a> std::ops::Deref for Encoder<'a> {
    type Target = crate::entropy::Encoder<'a>;
    fn deref(&self) -> &Self::Target {
        &self.coder
    }
}

impl std::ops::DerefMut for Encoder<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.coder
    }
}

impl<'a> std::ops::Deref for Decoder<'a> {
    type Target = crate::entropy::Decoder<'a>;
    fn deref(&self) -> &Self::Target {
        &self.coder
    }
}

impl std::ops::DerefMut for Decoder<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.coder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: [(&[u8], u32); 4] = [
        (&[29, 26, 0], 5),
        (&[200, 200, 128, 0], 8),
        (&[7, 4, 2, 0], 3),
        (&[0], 1),
    ];

    #[test]
    fn mapped_intervals_match_frequency_update_and_preserve_invalid_state() {
        let mut packet = [0; 128];
        let mut seed = 0x9876_5432u32;
        for byte in &mut packet {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (seed >> 24) as u8;
        }
        let mut baseline = Decoder::new(&packet);
        let mut mapped = Decoder::new(&packet);
        for _ in 0..256 {
            for total in [2, 3, 17, 255, 256, 32768, 65535] {
                let frequency = baseline.frequency(total).unwrap();
                let width = 1 + total / 13;
                let low = frequency / width * width;
                let high = (low + width).min(total);
                baseline.update(low, high, total).unwrap();
                let symbol = mapped
                    .mapped_interval(total, |selected| {
                        assert_eq!(selected, frequency);
                        (selected / width, low, high)
                    })
                    .unwrap();
                assert_eq!(symbol, frequency / width);
                assert_eq!(
                    (mapped.range(), mapped.tell(), mapped.tell_frac()),
                    (baseline.range(), baseline.tell(), baseline.tell_frac())
                );
            }
            assert_eq!(mapped.bit(3).unwrap(), baseline.bit(3).unwrap());
        }
        let mut decoder = Decoder::new(&packet);
        let initial = (decoder.range(), decoder.tell_frac());
        for total in [0, 65536, u32::MAX] {
            assert_eq!(
                decoder.mapped_interval(total, |_| panic!("invalid total reached model")),
                Err::<(), _>(Error::Argument)
            );
        }
        for (low, high) in [(1, 1), (2, 1), (0, 18)] {
            assert_eq!(
                decoder.mapped_interval(17, |_| ((), low, high)),
                Err(Error::Argument)
            );
        }
        let frequency = decoder.frequency(17).unwrap();
        let wrong = (frequency + 1) % 17;
        assert_eq!(
            decoder.mapped_interval(17, |_| ((), wrong, wrong + 1)),
            Err(Error::Corrupt)
        );
        assert_eq!((decoder.range(), decoder.tell_frac()), initial);
    }

    #[test]
    fn inverse_tables_match_all_denominators_and_large_alphabets() {
        let mut packet = [0; 128];
        let mut seed = 0x1234_6789u32;
        for byte in &mut packet {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (seed >> 24) as u8;
        }
        for bits in 1..=8 {
            let total = 1 << bits;
            let table: Vec<_> = (0..total)
                .map(|symbol| (total - symbol - 1) as u8)
                .collect();
            let model = Icdf::new(&table, bits).unwrap();
            let mut baseline = Decoder::new(&packet);
            let mut inverse = Decoder::new(&packet);
            for _ in 0..1024 {
                let expected = baseline.frequency(total).unwrap();
                baseline.update(expected, expected + 1, total).unwrap();
                assert_eq!(inverse.symbol(model), expected as usize);
                assert_eq!(
                    (inverse.range(), inverse.tell(), inverse.tell_frac()),
                    (baseline.range(), baseline.tell(), baseline.tell_frac())
                );
            }
        }
    }

    #[test]
    fn inverse_laplace_matches_every_cumulative_bin_and_remainder() {
        for (zero, decay) in [
            (10000, 10000),
            (6000, 12000),
            (256, 16300),
            (10000, 0),
            (30000, 1000),
            (1, 16384),
            (32736, 0),
            (32736, 16384),
            (1, 0),
        ] {
            let model = Laplace::new(zero, decay).unwrap();
            for range in [8_388_609u32, 16_777_947, 268_435_456, 2_147_483_647] {
                let unit = range / LAPLACE_TOTAL;
                for frequency in 0..LAPLACE_TOTAL {
                    let expected = model.locate(frequency);
                    let lower = (LAPLACE_TOTAL - frequency - 1) * unit;
                    for inverse in [lower, lower + unit - 1] {
                        assert_eq!(model.locate_inverse(inverse, unit), expected);
                    }
                }
                assert_eq!(model.locate_inverse(range - 1, unit), model.locate(0));
            }
        }
    }

    #[test]
    fn independent_icdf_laplace_reference_matches_packets_and_every_state() {
        // The reference encoded these streams and independently decoded every
        // symbol before writing them. Neither C code nor its build is required
        // by this test. This extends the shared entropy fixture with ICDF zero
        // bins, Laplace tails/clipping and state-sensitive mixed operations.
        let fixture = include_bytes!("testdata/range-libopus-1.6.1.bin");
        assert_eq!(&fixture[..8], b"SNKOPR01");
        let frames = u32::from_le_bytes(fixture[8..12].try_into().unwrap());
        let mut cursor = 12;
        for frame_index in 0..frames {
            let size = u16::from_le_bytes(fixture[cursor..cursor + 2].try_into().unwrap()) as usize;
            let events = u16::from_le_bytes(fixture[cursor + 2..cursor + 4].try_into().unwrap());
            cursor += 4;
            let reference = &fixture[cursor..cursor + size];
            cursor += size;
            let mut packet = [0; 4096];
            let mut encoder = Encoder::new(&mut packet[..size]);
            let mut decoder = Decoder::new(reference);
            for event_index in 0..events {
                let kind = fixture[cursor];
                let [parameter, second, requested, actual, range, tell, fraction]: [u32; 7] =
                    std::array::from_fn(|field| {
                        let at = cursor + 1 + field * 4;
                        u32::from_le_bytes(fixture[at..at + 4].try_into().unwrap())
                    });
                let (encoded, decoded) = match kind {
                    0 => {
                        let (table, bits) = TABLES[parameter as usize];
                        encoder.icdf(requested as usize, table, bits).unwrap();
                        (requested, decoder.icdf(table, bits).unwrap() as u32)
                    }
                    1 => (
                        encoder
                            .laplace(requested as i32, parameter, second)
                            .unwrap() as u32,
                        decoder.laplace(parameter, second).unwrap() as u32,
                    ),
                    2 => {
                        encoder.bit(requested != 0, parameter).unwrap();
                        (requested, u32::from(decoder.bit(parameter).unwrap()))
                    }
                    3 => {
                        encoder.raw(requested, parameter).unwrap();
                        (requested, decoder.raw(parameter).unwrap())
                    }
                    4 => {
                        encoder.uniform(requested, parameter).unwrap();
                        (requested, decoder.uniform(parameter).unwrap())
                    }
                    _ => panic!("invalid reference event"),
                };
                assert_eq!(
                    (encoded, decoded),
                    (actual, actual),
                    "frame {frame_index}, event {event_index}"
                );
                let expected = (range, u64::from(tell), u64::from(fraction));
                assert_eq!(
                    (encoder.range(), encoder.tell(), encoder.tell_frac()),
                    expected,
                    "encoder frame {frame_index}, event {event_index}"
                );
                assert_eq!(
                    (decoder.range(), decoder.tell(), decoder.tell_frac()),
                    expected,
                    "decoder frame {frame_index}, event {event_index}"
                );
                cursor += 29;
            }
            assert_eq!(encoder.finish_padded().unwrap(), size);
            assert_eq!(
                &packet[..size],
                reference,
                "exact reference packet {frame_index}"
            );
        }
        assert_eq!(cursor, fixture.len());
    }

    #[test]
    fn laplace_extremes_clip_safely_and_all_integer_bins_round_trip() {
        for (zero, decay) in [
            (1, 0),
            (1, 16_384),
            (32_736, 0),
            (32_736, 16_384),
            (10_000, 10_000),
        ] {
            let model = Laplace::new(zero, decay).unwrap();
            for frequency in 0..LAPLACE_TOTAL {
                let (value, low, high) = model.locate(frequency);
                assert!(low <= frequency && frequency < high);
                assert_eq!(model.interval(value), (value, low, high));
            }
            for requested in [i32::MIN, i32::MAX] {
                let (actual, low, high) = model.interval(requested);
                assert!(low < high && high <= LAPLACE_TOTAL);
                assert_eq!(model.locate(low), (actual, low, high));
            }
        }
    }

    #[test]
    fn invalid_models_and_intervals_leave_coder_state_intact() {
        let mut packet = [0; 16];
        let mut encoder = Encoder::new(&mut packet);
        let initial = (encoder.range(), encoder.tell_frac());
        for (table, bits) in [
            (&[7, 0][..], 0),
            (&[0][..], 9),
            (&[8, 0][..], 3),
            (&[7, 4][..], 3),
            (&[4, 7, 0][..], 3),
        ] {
            assert!(matches!(encoder.icdf(0, table, bits), Err(Error::Argument)));
        }
        assert_eq!(
            encoder.icdf(1, &[200, 200, 128, 0], 8),
            Err(Error::Argument)
        );
        for (zero, decay) in [(0, 0), (32_737, 0), (1, 16_385), (u32::MAX, u32::MAX)] {
            assert_eq!(encoder.laplace(0, zero, decay), Err(Error::Argument));
        }
        assert_eq!(encoder.interval(1, 1, 2), Err(Error::Argument));
        assert_eq!((encoder.range(), encoder.tell_frac()), initial);
        let mut decoder = Decoder::new(&[0; 16]);
        let initial = (decoder.range(), decoder.tell_frac());
        assert_eq!(decoder.frequency(0), Err(Error::Argument));
        assert_eq!(decoder.update(1, 1, 2), Err(Error::Argument));
        assert_eq!(decoder.update(1, 2, 2), Err(Error::Corrupt));
        assert_eq!((decoder.range(), decoder.tell_frac()), initial);
    }
}
