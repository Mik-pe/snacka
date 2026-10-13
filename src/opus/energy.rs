// SPDX-License-Identifier: MIT AND BSD-3-Clause
//! CELT's coarse and two fine log-energy refinements (RFC 6716 §§4.3.2, 5.3.3).
//!
//! Numeric wire tables come from RFC 6716's normative Appendix A archive,
//! `celt/quant_bands.c`; `MAX_FINE_BITS` comes from `celt/rate.h`. The arithmetic
//! below independently expresses the prediction filter and midpoint quantizers.
//! Energies are base-2 log amplitudes after subtracting `BAND_MEANS`. History
//! includes every fine refinement, while the current frame's frequency predictor
//! only includes coarse residuals. All working storage is fixed-size stack data.

use super::range::{Decoder, Encoder, Error as RangeError};

pub(crate) const BANDS: usize = 21;
pub(crate) const MAX_FINE_BITS: u8 = 8;
pub(crate) type BandEnergies = [[f32; BANDS]; 2];

/// Standard-mode band means, exactly representable in binary floating point.
pub(crate) const BAND_MEANS: [f32; BANDS] = [
    6.4375, 6.25, 5.75, 5.3125, 5.0625, 4.8125, 4.5, 4.375, 4.875, 4.6875, 4.5625, 4.4375, 4.875,
    4.625, 4.3125, 4.5, 4.375, 4.625, 4.75, 4.4375, 3.75,
];

const ALPHA_Q15: [u16; 4] = [29440, 26112, 21248, 16384];
const BETA_Q15: [u16; 4] = [30147, 22282, 12124, 6554];
const INTRA_BETA_Q15: u16 = 4915;
const SMALL_RESIDUAL_ICDF: [u8; 3] = [2, 1, 0];

// Pairs are (zero probability, magnitude decay), in Q8. First index is log2
// frame-size multiplier relative to 120 samples; second is inter/intra.
const COARSE_PROBABILITIES: [[[u8; 42]; 2]; 4] = [
    [
        [
            72, 127, 65, 129, 66, 128, 65, 128, 64, 128, 62, 128, 64, 128, 64, 128, 92, 78, 92, 79,
            92, 78, 90, 79, 116, 41, 115, 40, 114, 40, 132, 26, 132, 26, 145, 17, 161, 12, 176, 10,
            177, 11,
        ],
        [
            24, 179, 48, 138, 54, 135, 54, 132, 53, 134, 56, 133, 55, 132, 55, 132, 61, 114, 70,
            96, 74, 88, 75, 88, 87, 74, 89, 66, 91, 67, 100, 59, 108, 50, 120, 40, 122, 37, 97, 43,
            78, 50,
        ],
    ],
    [
        [
            83, 78, 84, 81, 88, 75, 86, 74, 87, 71, 90, 73, 93, 74, 93, 74, 109, 40, 114, 36, 117,
            34, 117, 34, 143, 17, 145, 18, 146, 19, 162, 12, 165, 10, 178, 7, 189, 6, 190, 8, 177,
            9,
        ],
        [
            23, 178, 54, 115, 63, 102, 66, 98, 69, 99, 74, 89, 71, 91, 73, 91, 78, 89, 86, 80, 92,
            66, 93, 64, 102, 59, 103, 60, 104, 60, 117, 52, 123, 44, 138, 35, 133, 31, 97, 38, 77,
            45,
        ],
    ],
    [
        [
            61, 90, 93, 60, 105, 42, 107, 41, 110, 45, 116, 38, 113, 38, 112, 38, 124, 26, 132, 27,
            136, 19, 140, 20, 155, 14, 159, 16, 158, 18, 170, 13, 177, 10, 187, 8, 192, 6, 175, 9,
            159, 10,
        ],
        [
            21, 178, 59, 110, 71, 86, 75, 85, 84, 83, 91, 66, 88, 73, 87, 72, 92, 75, 98, 72, 105,
            58, 107, 54, 115, 52, 114, 55, 112, 56, 129, 51, 132, 40, 150, 33, 140, 29, 98, 35, 77,
            42,
        ],
    ],
    [
        [
            42, 121, 96, 66, 108, 43, 111, 40, 117, 44, 123, 32, 120, 36, 119, 33, 127, 33, 134,
            34, 139, 21, 147, 23, 152, 20, 158, 25, 154, 26, 166, 21, 173, 16, 184, 13, 184, 10,
            150, 13, 139, 15,
        ],
        [
            22, 178, 63, 114, 74, 82, 84, 83, 92, 82, 103, 62, 96, 72, 96, 67, 101, 73, 107, 72,
            113, 55, 118, 52, 125, 52, 118, 52, 117, 55, 135, 49, 137, 39, 157, 32, 145, 29, 97,
            33, 77, 40,
        ],
    ],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Argument,
    Budget,
    Range(RangeError),
}

impl From<RangeError> for Error {
    fn from(value: RangeError) -> Self {
        Self::Range(value)
    }
}

/// A validated standard-mode energy envelope region and packet bit budget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameConfig {
    start: usize,
    end: usize,
    channels: usize,
    lm: usize,
    packet_bits: u32,
}

impl FrameConfig {
    pub(crate) fn new(
        start: usize,
        end: usize,
        channels: usize,
        lm: usize,
        packet_bits: u32,
    ) -> Result<Self, Error> {
        if start >= end || end > BANDS || !(1..=2).contains(&channels) || lm > 3 {
            return Err(Error::Argument);
        }
        Ok(Self {
            start,
            end,
            channels,
            lm,
            packet_bits,
        })
    }

    fn remaining(self, tell: u64) -> u32 {
        u64::from(self.packet_bits).saturating_sub(tell) as u32
    }

    fn validate_energies(self, energies: &BandEnergies) -> Result<(), Error> {
        if energies[..self.channels].iter().any(|channel| {
            channel[self.start..self.end]
                .iter()
                .any(|energy| !energy.is_finite())
        }) {
            return Err(Error::Argument);
        }
        Ok(())
    }

    fn validate_fine(self, bits: &[u8; BANDS]) -> Result<u32, Error> {
        let selected = &bits[self.start..self.end];
        if selected.iter().any(|&count| count > MAX_FINE_BITS) {
            return Err(Error::Argument);
        }
        Ok(selected.iter().map(|&count| u32::from(count)).sum::<u32>() * self.channels as u32)
    }
}

/// Read the intra flag at its position in the CELT header. When the packet has
/// fewer than three bits left, the flag is absent and inter prediction applies.
pub(crate) fn decode_intra(decoder: &mut Decoder<'_>, config: FrameConfig) -> Result<bool, Error> {
    if config.remaining(decoder.tell()) < 3 {
        return Ok(false);
    }
    Ok(decoder.bit(3)?)
}

/// Write the intra flag; the returned mode accounts for an omitted flag at an
/// exhausted budget and must be passed to `encode_coarse`.
pub(crate) fn encode_intra(
    encoder: &mut Encoder<'_>,
    config: FrameConfig,
    intra: bool,
) -> Result<bool, Error> {
    if config.remaining(encoder.tell()) < 3 {
        return Ok(false);
    }
    encoder.bit(intra, 3)?;
    Ok(intra)
}

/// Previous reconstructed, mean-centered log energies, including fine bits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EnergyState {
    pub(crate) energies: BandEnergies,
}

impl EnergyState {
    pub(crate) const fn new() -> Self {
        Self {
            energies: [[0.0; BANDS]; 2],
        }
    }

    pub(crate) fn reset(&mut self) {
        self.energies = [[0.0; BANDS]; 2];
    }

    /// Run the coarse recurrence using residuals supplied by either coding side.
    /// A frame error leaves history intact; an encoder error remains terminal for
    /// the entropy frame, following the shared range encoder's contract.
    #[allow(clippy::needless_range_loop)] // Wire order is band first, then channel.
    fn coarse(
        &mut self,
        config: FrameConfig,
        intra: bool,
        mut residual: impl FnMut(usize, usize, f32) -> Result<i32, Error>,
    ) -> Result<(), Error> {
        config.validate_energies(&self.energies)?;
        let alpha = if intra {
            0.0
        } else {
            f32::from(ALPHA_Q15[config.lm]) / 32768.0
        };
        let beta = f32::from(if intra {
            INTRA_BETA_Q15
        } else {
            BETA_Q15[config.lm]
        }) / 32768.0;
        let mut updated = self.energies;
        let mut frequency = [0.0; 2];
        for band in config.start..config.end {
            for channel in 0..config.channels {
                let predicted = alpha * self.energies[channel][band].max(-9.0) + frequency[channel];
                let integer = residual(band, channel, predicted)? as f32;
                updated[channel][band] = predicted + integer;
                // q_b = d_b - d_(b-1) + beta*q_(b-1), where
                // d_b = E_b - alpha*E_previous_frame_b.
                frequency[channel] = frequency[channel] + integer - beta * integer;
            }
        }
        self.energies = updated;
        Ok(())
    }

    /// Independently choose the nearest coarse cell in the alphabet available at
    /// the current budget. Encoder rate control can select intra/inter beforehand.
    pub(crate) fn encode_coarse(
        &mut self,
        encoder: &mut Encoder<'_>,
        config: FrameConfig,
        intra: bool,
        target: &BandEnergies,
    ) -> Result<(), Error> {
        config.validate_energies(target)?;
        let probabilities = &COARSE_PROBABILITIES[config.lm][usize::from(intra)];
        self.coarse(config, intra, |band, channel, predicted| {
            let nearest = (target[channel][band] - predicted + 0.5).floor() as i32;
            let available = config.remaining(encoder.tell());
            let quantized = match available {
                15.. => encoder.laplace(
                    nearest,
                    u32::from(probabilities[band * 2]) << 7,
                    u32::from(probabilities[band * 2 + 1]) << 6,
                )?,
                2..=14 => {
                    let bounded = nearest.clamp(-1, 1);
                    let symbol = match bounded {
                        -1 => 1,
                        1 => 2,
                        _ => 0,
                    };
                    encoder.icdf(symbol, &SMALL_RESIDUAL_ICDF, 2)?;
                    bounded
                }
                1 => {
                    let bounded = nearest.clamp(-1, 0);
                    encoder.bit(bounded < 0, 1)?;
                    bounded
                }
                _ => -1,
            };
            Ok(quantized)
        })
    }

    pub(crate) fn decode_coarse(
        &mut self,
        decoder: &mut Decoder<'_>,
        config: FrameConfig,
        intra: bool,
    ) -> Result<(), Error> {
        let probabilities = &COARSE_PROBABILITIES[config.lm][usize::from(intra)];
        self.coarse(config, intra, |band, _, _| {
            let quantized = match config.remaining(decoder.tell()) {
                15.. => decoder.laplace(
                    u32::from(probabilities[band * 2]) << 7,
                    u32::from(probabilities[band * 2 + 1]) << 6,
                )?,
                2..=14 => match decoder.icdf(&SMALL_RESIDUAL_ICDF, 2)? {
                    0 => 0,
                    1 => -1,
                    _ => 1,
                },
                1 => -i32::from(decoder.bit(1)?),
                _ => -1,
            };
            Ok(quantized)
        })
    }

    fn fine(
        &mut self,
        config: FrameConfig,
        bits: &[u8; BANDS],
        tell: u64,
        mut symbol: impl FnMut(usize, usize, u32, f32) -> Result<u32, Error>,
    ) -> Result<(), Error> {
        config.validate_energies(&self.energies)?;
        let required = config.validate_fine(bits)?;
        if required > config.remaining(tell) {
            return Err(Error::Budget);
        }
        let mut updated = self.energies;
        for (band, &count) in bits.iter().enumerate().take(config.end).skip(config.start) {
            if count == 0 {
                continue;
            }
            let levels = (1u32 << count) as f32;
            for (channel, history) in updated.iter_mut().enumerate().take(config.channels) {
                let value = symbol(band, channel, u32::from(count), history[band])?;
                history[band] += (value as f32 + 0.5) / levels - 0.5;
            }
        }
        self.energies = updated;
        Ok(())
    }

    pub(crate) fn encode_fine(
        &mut self,
        encoder: &mut Encoder<'_>,
        config: FrameConfig,
        bits: &[u8; BANDS],
        target: &BandEnergies,
    ) -> Result<(), Error> {
        config.validate_energies(target)?;
        self.fine(
            config,
            bits,
            encoder.tell(),
            |band, channel, count, current| {
                let levels = 1u32 << count;
                let cell = ((target[channel][band] - current + 0.5) * levels as f32)
                    .floor()
                    .clamp(0.0, (levels - 1) as f32) as u32;
                encoder.raw(cell, count)?;
                Ok(cell)
            },
        )
    }

    pub(crate) fn decode_fine(
        &mut self,
        decoder: &mut Decoder<'_>,
        config: FrameConfig,
        bits: &[u8; BANDS],
    ) -> Result<(), Error> {
        self.fine(config, bits, decoder.tell(), |_, _, count, _| {
            Ok(decoder.raw(count)?)
        })
    }

    fn finalize(
        &mut self,
        config: FrameConfig,
        bits: &[u8; BANDS],
        priorities: &[u8; BANDS],
        bits_left: u32,
        tell: u64,
        mut symbol: impl FnMut(usize, usize, f32) -> Result<bool, Error>,
    ) -> Result<u32, Error> {
        config.validate_energies(&self.energies)?;
        config.validate_fine(bits)?;
        if priorities[config.start..config.end]
            .iter()
            .any(|&priority| priority > 1)
        {
            return Err(Error::Argument);
        }
        if bits_left > config.remaining(tell) {
            return Err(Error::Budget);
        }
        let mut updated = self.energies;
        let mut used = 0;
        for priority in 0..=1 {
            for band in config.start..config.end {
                if bits_left - used < config.channels as u32 {
                    break;
                }
                if priorities[band] != priority || bits[band] >= MAX_FINE_BITS {
                    continue;
                }
                let correction = 0.25 / (1u32 << bits[band]) as f32;
                for (channel, history) in updated.iter_mut().enumerate().take(config.channels) {
                    let upper = symbol(band, channel, history[band])?;
                    history[band] += if upper { correction } else { -correction };
                }
                used += config.channels as u32;
            }
        }
        self.energies = updated;
        Ok(used)
    }

    pub(crate) fn encode_final(
        &mut self,
        encoder: &mut Encoder<'_>,
        config: FrameConfig,
        bits: &[u8; BANDS],
        priorities: &[u8; BANDS],
        bits_left: u32,
        target: &BandEnergies,
    ) -> Result<u32, Error> {
        config.validate_energies(target)?;
        self.finalize(
            config,
            bits,
            priorities,
            bits_left,
            encoder.tell(),
            |band, channel, current| {
                let upper = target[channel][band] >= current;
                encoder.raw(u32::from(upper), 1)?;
                Ok(upper)
            },
        )
    }

    pub(crate) fn decode_final(
        &mut self,
        decoder: &mut Decoder<'_>,
        config: FrameConfig,
        bits: &[u8; BANDS],
        priorities: &[u8; BANDS],
        bits_left: u32,
    ) -> Result<u32, Error> {
        self.finalize(
            config,
            bits,
            priorities,
            bits_left,
            decoder.tell(),
            |_, _, _| Ok(decoder.raw(1)? != 0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(frame: usize) -> BandEnergies {
        std::array::from_fn(|channel| {
            std::array::from_fn(|band| {
                ((band * 13 + channel * 7 + frame * 3) % 37) as f32 * 0.1875 - 6.0
            })
        })
    }

    #[test]
    fn reconstructed_energy_matches_the_normative_reference() {
        // Expected digest generated by the unmodified RFC 6716 Appendix A C
        // decoder with float arithmetic and contraction disabled. Each sample
        // records all 42 f32 bit patterns, entropy tell, and entropy interval.
        // This exercises 768 independent frame decodes and 1,152 stage samples.
        fn word(hash: &mut u64, value: u32) {
            for byte in value.to_le_bytes() {
                *hash = (*hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
            }
        }
        fn sample(hash: &mut u64, state: &EnergyState, decoder: &Decoder<'_>) {
            for channel in &state.energies {
                for energy in channel {
                    word(hash, energy.to_bits());
                }
            }
            word(hash, decoder.tell() as u32);
            word(hash, decoder.range());
        }
        let mut hash = 14_695_981_039_346_656_037;
        let mut packet = [0; 256];
        let fine = std::array::from_fn(|band| (band % 5) as u8);
        let priorities = std::array::from_fn(|band| (band % 2) as u8);
        for size in [1, 2, 3, 4, 8, 16, 128, 256] {
            for channels in 1..=2 {
                for lm in 0..=3 {
                    let config = FrameConfig::new(0, BANDS, channels, lm, size as u32 * 8).unwrap();
                    let mut state = EnergyState::new();
                    for frame in 0..12 {
                        for (byte, value) in packet.iter_mut().enumerate().take(size) {
                            *value = (byte * 73 + frame * 41 + lm * 29 + channels * 17) as u8;
                        }
                        let mut decoder = Decoder::new(&packet[..size]);
                        let intra = decode_intra(&mut decoder, config).unwrap();
                        state.decode_coarse(&mut decoder, config, intra).unwrap();
                        sample(&mut hash, &state, &decoder);
                        if size >= 128 {
                            state.decode_fine(&mut decoder, config, &fine).unwrap();
                            sample(&mut hash, &state, &decoder);
                            state
                                .decode_final(&mut decoder, config, &fine, &priorities, 19)
                                .unwrap();
                            sample(&mut hash, &state, &decoder);
                        }
                    }
                }
            }
        }
        assert_eq!(hash, 0x8feb_6ab8_b67b_d15a);
    }

    #[test]
    fn mono_and_stereo_histories_agree_after_every_refinement() {
        for channels in 1..=2 {
            for lm in 0..=3 {
                let mut encoded = EnergyState::new();
                let mut decoded = EnergyState::new();
                for frame in 0..12 {
                    let config = FrameConfig::new(0, BANDS, channels, lm, 2048).unwrap();
                    let target = target(frame);
                    let fine = std::array::from_fn(|band| (band % 5) as u8);
                    let priorities = std::array::from_fn(|band| (band % 2) as u8);
                    let mut bytes = [0; 256];
                    let mut encoder = Encoder::new(&mut bytes);
                    let intra = encode_intra(&mut encoder, config, frame % 4 == 0).unwrap();
                    encoded
                        .encode_coarse(&mut encoder, config, intra, &target)
                        .unwrap();
                    let coarse = encoded.clone();
                    encoded
                        .encode_fine(&mut encoder, config, &fine, &target)
                        .unwrap();
                    let refined = encoded.clone();
                    let used = encoded
                        .encode_final(&mut encoder, config, &fine, &priorities, 19, &target)
                        .unwrap();
                    let range = encoder.range();
                    let tell = encoder.tell();
                    encoder.finish_padded().unwrap();
                    let mut decoder = Decoder::new(&bytes);
                    assert_eq!(decode_intra(&mut decoder, config).unwrap(), intra);
                    decoded.decode_coarse(&mut decoder, config, intra).unwrap();
                    assert_eq!(decoded, coarse);
                    decoded.decode_fine(&mut decoder, config, &fine).unwrap();
                    assert_eq!(decoded, refined);
                    assert_eq!(
                        decoded
                            .decode_final(&mut decoder, config, &fine, &priorities, 19)
                            .unwrap(),
                        used
                    );
                    assert_eq!(decoded, encoded);
                    assert_eq!(decoder.tell(), tell);
                    assert_eq!(decoder.range(), range);
                }
            }
        }
    }

    #[test]
    fn every_coarse_budget_alphabet_roundtrips() {
        for bits in 1..=64 {
            for channels in 1..=2 {
                let config = FrameConfig::new(3, 17, channels, 2, bits).unwrap();
                let mut encoded = EnergyState::new();
                let mut decoded = EnergyState::new();
                let mut bytes = [0; 8];
                let mut encoder = Encoder::new(&mut bytes);
                let intra = encode_intra(&mut encoder, config, true).unwrap();
                encoded
                    .encode_coarse(&mut encoder, config, intra, &target(1))
                    .unwrap();
                assert!(encoder.tell() <= u64::from(bits).max(1));
                let range = encoder.range();
                encoder.finish_padded().unwrap();
                let mut decoder = Decoder::new(&bytes);
                assert_eq!(decode_intra(&mut decoder, config).unwrap(), intra);
                decoded.decode_coarse(&mut decoder, config, intra).unwrap();
                assert_eq!(encoded, decoded, "budget={bits}, channels={channels}");
                assert_eq!(decoder.range(), range);
                // Unselected bands and channels retain their exact histories.
                assert_eq!(decoded.energies[0][0], 0.0);
                if channels == 1 {
                    assert_eq!(decoded.energies[1], [0.0; BANDS]);
                }
            }
        }
    }

    #[test]
    fn intra_history_is_independent_and_inter_prediction_clamps_old_energy() {
        let config = FrameConfig::new(0, 4, 2, 3, 0).unwrap();
        let mut a = EnergyState {
            energies: [[-100.0; BANDS]; 2],
        };
        let mut b = EnergyState {
            energies: [[100.0; BANDS]; 2],
        };
        a.decode_coarse(&mut Decoder::new(&[]), config, true)
            .unwrap();
        b.decode_coarse(&mut Decoder::new(&[]), config, true)
            .unwrap();
        assert_eq!(&a.energies[0][..4], &b.energies[0][..4]);
        let mut a = EnergyState {
            energies: [[-100.0; BANDS]; 2],
        };
        let mut b = EnergyState {
            energies: [[-9.0; BANDS]; 2],
        };
        a.decode_coarse(&mut Decoder::new(&[]), config, false)
            .unwrap();
        b.decode_coarse(&mut Decoder::new(&[]), config, false)
            .unwrap();
        assert_eq!(&a.energies[0][..4], &b.energies[0][..4]);
        assert_eq!(a.energies[0][0], -5.5);
    }

    #[test]
    fn final_bits_follow_priorities_and_keep_stereo_bands_whole() {
        let config = FrameConfig::new(0, 4, 2, 0, 64).unwrap();
        let bits = [0; BANDS];
        let mut priorities = [0; BANDS];
        priorities[0] = 1;
        priorities[2] = 1;
        let target = [[1.0; BANDS]; 2];
        let mut state = EnergyState::new();
        let mut bytes = [0; 8];
        let mut encoder = Encoder::new(&mut bytes);
        assert_eq!(
            state
                .encode_final(&mut encoder, config, &bits, &priorities, 5, &target)
                .unwrap(),
            4
        );
        assert_eq!(&state.energies[0][..4], &[0.0, 0.25, 0.0, 0.25]);
        assert_eq!(state.energies[0], state.energies[1]);
        encoder.finish_padded().unwrap();
        let mut decoded = EnergyState::new();
        assert_eq!(
            decoded
                .decode_final(&mut Decoder::new(&bytes), config, &bits, &priorities, 5)
                .unwrap(),
            4
        );
        assert_eq!(decoded, state);
    }

    #[test]
    fn fine_errors_leave_history_and_entropy_untouched() {
        let config = FrameConfig::new(0, 2, 2, 0, 8).unwrap();
        let mut bytes = [0; 1];
        let mut encoder = Encoder::new(&mut bytes);
        let mut state = EnergyState::new();
        let saved = state.clone();
        let range = encoder.range();
        assert_eq!(
            state.encode_fine(&mut encoder, config, &[8; BANDS], &target(0)),
            Err(Error::Budget)
        );
        assert_eq!(state, saved);
        assert_eq!(encoder.range(), range);
        assert_eq!(encoder.tell(), 1);
        assert_eq!(
            state.encode_fine(&mut encoder, config, &[9; BANDS], &target(0)),
            Err(Error::Argument)
        );
        let mut nonfinite = target(0);
        nonfinite[1][0] = f32::NAN;
        assert_eq!(
            state.encode_coarse(&mut encoder, config, true, &nonfinite),
            Err(Error::Argument)
        );
        assert_eq!(state, saved);
        state.energies[1][0] = f32::INFINITY;
        assert_eq!(
            state.decode_coarse(&mut Decoder::new(&[]), config, false),
            Err(Error::Argument)
        );
        for args in [
            (1, 1, 1, 0),
            (0, 22, 1, 0),
            (0, 1, 0, 0),
            (0, 1, 3, 0),
            (0, 1, 1, 4),
        ] {
            assert!(matches!(
                FrameConfig::new(args.0, args.1, args.2, args.3, 64),
                Err(Error::Argument)
            ));
        }
    }
}
