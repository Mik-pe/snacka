// SPDX-License-Identifier: MIT
//! Allocation-free RFC 6716 section 4.2.7.8 excitation decoder.

use super::tables::{COUNT_WEIGHTS, NEGATIVE_WEIGHTS, RATE_WEIGHTS, SHELL_WEIGHTS};
use super::{Error, Frame, FrameType, SignalType};
use crate::entropy::{Decoder, Model};

const MAX_SAMPLES: usize = 320;
const MAX_BLOCKS: usize = MAX_SAMPLES / 16;
const SEED: Model<'static> = Model::from_static(&[0, 64, 128, 192, 256]);
const LSB: Model<'static> = Model::from_static(&[0, 136, 256]);

const fn cumulative<const N: usize, const M: usize>(weights: &[u16; N]) -> [u16; M] {
    assert!(M == N + 1);
    let mut boundaries = [0; M];
    let mut index = 0;
    while index < N {
        boundaries[index + 1] = boundaries[index] + weights[index];
        index += 1;
    }
    assert!(boundaries[N] == 256);
    boundaries
}

static RATE_BOUNDARIES: [[u16; 10]; 2] =
    [cumulative(&RATE_WEIGHTS[0]), cumulative(&RATE_WEIGHTS[1])];
const RATE: [Model<'static>; 2] = [
    Model::from_static(&RATE_BOUNDARIES[0]),
    Model::from_static(&RATE_BOUNDARIES[1]),
];

const fn count_boundaries() -> [[u16; 19]; 11] {
    let mut tables = [[0; 19]; 11];
    let mut level = 0;
    while level < tables.len() {
        tables[level] = cumulative(&COUNT_WEIGHTS[level]);
        level += 1;
    }
    tables
}
static COUNT_BOUNDARIES: [[u16; 19]; 11] = count_boundaries();
const fn count_models() -> [Model<'static>; 11] {
    let mut models = [Model::from_static(&[0, 256]); 11];
    let mut level = 0;
    while level < models.len() {
        let length = if level == 10 { 18 } else { 19 };
        models[level] = Model::from_static(COUNT_BOUNDARIES[level].split_at(length).0);
        level += 1;
    }
    models
}
const COUNT: [Model<'static>; 11] = count_models();

const fn shell_boundaries() -> [[[u16; 18]; 16]; 4] {
    let mut tables = [[[0; 18]; 16]; 4];
    let mut level = 0;
    while level < 4 {
        let mut pulses = 0;
        while pulses < 16 {
            tables[level][pulses] = cumulative(&SHELL_WEIGHTS[level][pulses]);
            pulses += 1;
        }
        level += 1;
    }
    tables
}
static SHELL_BOUNDARIES: [[[u16; 18]; 16]; 4] = shell_boundaries();
const fn shell_models() -> [[Model<'static>; 16]; 4] {
    let mut models = [[Model::from_static(&[0, 256]); 16]; 4];
    let mut level = 0;
    while level < 4 {
        let mut pulses = 0;
        while pulses < 16 {
            models[level][pulses] =
                Model::from_static(SHELL_BOUNDARIES[level][pulses].split_at(pulses + 3).0);
            pulses += 1;
        }
        level += 1;
    }
    models
}
const SHELL: [[Model<'static>; 16]; 4] = shell_models();

const fn sign_boundaries() -> [[[u16; 3]; 7]; 6] {
    let mut tables = [[[0; 3]; 7]; 6];
    let mut kind = 0;
    while kind < 6 {
        let mut pulses = 0;
        while pulses < 7 {
            tables[kind][pulses] = [0, NEGATIVE_WEIGHTS[kind][pulses], 256];
            pulses += 1;
        }
        kind += 1;
    }
    tables
}
static SIGN_BOUNDARIES: [[[u16; 3]; 7]; 6] = sign_boundaries();
const fn sign_models() -> [[Model<'static>; 7]; 6] {
    let mut models = [[Model::from_static(&[0, 256]); 7]; 6];
    let mut kind = 0;
    while kind < 6 {
        let mut pulses = 0;
        while pulses < 7 {
            models[kind][pulses] = Model::from_static(&SIGN_BOUNDARIES[kind][pulses]);
            pulses += 1;
        }
        kind += 1;
    }
    models
}
const SIGN: [[Model<'static>; 7]; 6] = sign_models();

#[inline]
fn shell<const LEVEL: usize>(decoder: &mut Decoder<'_>, output: &mut [i16], pulses: usize) {
    if pulses == 0 {
        output.fill(0);
        return;
    }
    let left = decoder.symbol(SHELL[LEVEL][pulses - 1]);
    let right = pulses - left;
    if LEVEL == 3 {
        output[0] = left as i16;
        output[1] = right as i16;
    } else {
        let (first, second) = output.split_at_mut(output.len() / 2);
        match LEVEL {
            0 => {
                shell::<1>(decoder, first, left);
                shell::<1>(decoder, second, right);
            }
            1 => {
                shell::<2>(decoder, first, left);
                shell::<2>(decoder, second, right);
            }
            2 => {
                shell::<3>(decoder, first, left);
                shell::<3>(decoder, second, right);
            }
            _ => unreachable!("four-level shell geometry"),
        }
    }
}

/// Decode signed excitation pulse magnitudes, including coded MB padding.
///
/// `output` must hold [`Frame::coded_samples`]. The returned count excludes
/// padding, but the final eight coded samples of a 10 ms mediumband frame
/// are written and fully consumed, including their LSBs and signs. Buffer
/// failure leaves both output and entropy state unchanged. This function
/// uses no heap allocation and does not decode the preceding LCG seed.
pub fn decode_pulses(
    decoder: &mut Decoder<'_>,
    frame: Frame,
    kind: FrameType,
    output: &mut [i16],
) -> Result<usize, Error> {
    let coded = frame.coded_samples();
    if output.len() < coded {
        return Err(Error::Output);
    }
    let output = &mut output[..coded];
    let blocks = frame.shell_blocks();
    let rate = decoder.symbol(RATE[usize::from(kind.signal == SignalType::Voiced)]);
    let mut counts = [0u8; MAX_BLOCKS];
    let mut extra_bits = [0u8; MAX_BLOCKS];

    // Counts for every block precede every shell split in the bitstream.
    for (count, bits) in counts[..blocks].iter_mut().zip(&mut extra_bits[..blocks]) {
        let mut value = decoder.symbol(COUNT[rate]);
        while value == 17 {
            *bits += 1;
            value = decoder.symbol(COUNT[if *bits == 10 { 10 } else { 9 }]);
        }
        *count = value as u8;
    }
    for (block, &count) in output
        .as_chunks_mut::<16>()
        .0
        .iter_mut()
        .zip(&counts[..blocks])
    {
        shell::<0>(decoder, block, usize::from(count));
    }
    // All LSBs precede signs, including coefficient locations with no pulses.
    for (block, &bits) in output
        .as_chunks_mut::<16>()
        .0
        .iter_mut()
        .zip(&extra_bits[..blocks])
    {
        if bits != 0 {
            for magnitude in block {
                for _ in 0..bits {
                    *magnitude = *magnitude * 2 + decoder.symbol(LSB) as i16;
                }
            }
        }
    }
    for (block, &count) in output
        .as_chunks_mut::<16>()
        .0
        .iter_mut()
        .zip(&counts[..blocks])
    {
        let model = SIGN[kind.sign_table()][usize::from(count.min(6))];
        for magnitude in block {
            if *magnitude != 0 && decoder.symbol(model) == 0 {
                *magnitude = -*magnitude;
            }
        }
    }
    Ok(frame.samples())
}

/// Reconstruct Q23 excitation from signed magnitudes and the 2-bit LCG seed.
///
/// The seed must be 0 through 3. Quantization offsets and pseudorandom sign
/// modulation follow RFC 6716 section 4.2.7.8.6, using explicit modulo-2^32
/// arithmetic. Output capacity or seed failure leaves output unchanged.
/// The returned seed is useful for inspecting a complete frame's state.
pub fn reconstruct_excitation(
    pulses: &[i16],
    kind: FrameType,
    seed: u8,
    output: &mut [i32],
) -> Result<u32, Error> {
    if seed > 3 {
        return Err(Error::Argument);
    }
    if output.len() < pulses.len() {
        return Err(Error::Output);
    }
    let mut seed = u32::from(seed);
    let offset = kind.quantization_offset();
    for (&raw, excitation) in pulses.iter().zip(output) {
        let raw = i32::from(raw);
        let value = (raw << 8) - raw.signum() * 20 + offset;
        seed = seed.wrapping_mul(196_314_165).wrapping_add(907_633_515);
        *excitation = if seed & 0x8000_0000 == 0 {
            value
        } else {
            -value
        };
        seed = seed.wrapping_add_signed(raw);
    }
    Ok(seed)
}

/// Decode the LCG seed and complete Q23 excitation for one SILK frame.
///
/// The entropy decoder must be positioned after LTP side information, at
/// the seed symbol. Output holds only meaningful samples; MB padding is
/// consumed into bounded stack scratch. Capacity failure consumes no bits.
/// This primitive does not apply quantization gains or prediction filters.
pub fn decode_excitation(
    decoder: &mut Decoder<'_>,
    frame: Frame,
    kind: FrameType,
    output: &mut [i32],
) -> Result<usize, Error> {
    let samples = frame.samples();
    if output.len() < samples {
        return Err(Error::Output);
    }
    let seed = decoder.symbol(SEED) as u8;
    let mut pulses = [0; MAX_SAMPLES];
    decode_pulses(decoder, frame, kind, &mut pulses)?;
    reconstruct_excitation(&pulses[..samples], kind, seed, output)?;
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::silk::{Bandwidth, FrameDuration, QuantizationOffset};

    fn geometry(shape: u8) -> Frame {
        Frame {
            bandwidth: match shape % 3 {
                0 => Bandwidth::Narrow,
                1 => Bandwidth::Medium,
                _ => Bandwidth::Wide,
            },
            duration: if shape < 3 {
                FrameDuration::Ms10
            } else {
                FrameDuration::Ms20
            },
        }
    }

    fn kind(index: u8) -> FrameType {
        FrameType {
            signal: match index / 2 {
                0 => SignalType::Inactive,
                1 => SignalType::Unvoiced,
                _ => SignalType::Voiced,
            },
            offset: if index & 1 == 0 {
                QuantizationOffset::Low
            } else {
                QuantizationOffset::High
            },
        }
    }

    #[test]
    fn complete_pulses_and_entropy_state_match_independent_libopus() {
        let data = include_bytes!("../../../tests/fixtures/opus-silk-pulses.bin");
        assert_eq!(&data[..8], b"SNKSLK01");
        let cases = u32::from_le_bytes(data[8..12].try_into().unwrap());
        assert_eq!(cases, 610);
        let mut cursor = 12;
        let mut maximum = 0;
        for case in 0..cases {
            let frame = geometry(data[cursor]);
            let frame_type = kind(data[cursor + 1]);
            let length =
                u16::from_le_bytes(data[cursor + 2..cursor + 4].try_into().unwrap()) as usize;
            let coded =
                u16::from_le_bytes(data[cursor + 4..cursor + 6].try_into().unwrap()) as usize;
            let state: [u32; 3] = std::array::from_fn(|index| {
                let offset = cursor + 6 + index * 4;
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
            });
            cursor += 18;
            assert_eq!(coded, frame.coded_samples());
            let mut decoder = Decoder::new(&data[cursor..cursor + length]);
            cursor += length;
            let mut output = [i16::MIN; MAX_SAMPLES + 1];
            assert_eq!(
                decode_pulses(&mut decoder, frame, frame_type, &mut output).unwrap(),
                frame.samples()
            );
            for (index, &actual) in output[..coded].iter().enumerate() {
                let offset = cursor + index * 2;
                let expected = i16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                assert_eq!(actual, expected, "case {case}, coefficient {index}");
                maximum = maximum.max(actual.abs());
            }
            assert!(output[coded..].iter().all(|&value| value == i16::MIN));
            assert_eq!(
                (decoder.range(), decoder.tell(), decoder.tell_eighths()),
                (state[0], u64::from(state[1]), u64::from(state[2])),
                "case {case}"
            );
            cursor += coded * 2;
        }
        assert_eq!(maximum, 17_407);
        assert_eq!(cursor, data.len());
    }

    #[test]
    fn q23_reconstruction_matches_rfc_equations_with_modulo_seed() {
        let pulses = [0, 1, -1, 16, -16, 7, -7, 1023, -1023, 17407, -17407, 0];
        let kind = kind(0);
        let mut output = [i32::MIN; 13];
        let seed = reconstruct_excitation(&pulses, kind, 0, &mut output).unwrap();
        assert_eq!(
            &output[..12],
            &[
                25, -261, -211, -4101, -4051, 1797, 1747, -261893, 261843, 4456197, -4456147, -25
            ]
        );
        assert_eq!(seed, 4_016_127_668);
        let seed = reconstruct_excitation(&pulses, kind, 3, &mut output).unwrap();
        assert_eq!(
            &output[..12],
            &[
                25, -261, -211, 4101, -4051, -1797, -1747, -261893, 261843, -4456197, 4456147, -25
            ]
        );
        assert_eq!(seed, 2_616_706_919);
        assert_eq!(output[12], i32::MIN);
    }

    #[test]
    fn invalid_capacity_and_seed_leave_buffers_and_range_untouched() {
        let frame = geometry(1); // The coded MB tail makes 120 output slots insufficient.
        let mut decoder = Decoder::new(&[255; 128]);
        let before = (decoder.range(), decoder.tell());
        let mut raw = [123; 120];
        assert_eq!(
            decode_pulses(&mut decoder, frame, kind(0), &mut raw),
            Err(Error::Output)
        );
        assert_eq!(raw, [123; 120]);
        let mut q23 = [123; 119];
        assert_eq!(
            decode_excitation(&mut decoder, frame, kind(0), &mut q23),
            Err(Error::Output)
        );
        assert_eq!((decoder.range(), decoder.tell()), before);
        assert_eq!(q23, [123; 119]);
        assert_eq!(
            reconstruct_excitation(&[1, 2], kind(0), 4, &mut q23),
            Err(Error::Argument)
        );
        assert_eq!(
            reconstruct_excitation(&[1, 2], kind(0), 0, &mut q23[..1]),
            Err(Error::Output)
        );
        assert_eq!(q23, [123; 119]);
    }

    #[test]
    fn zero_entropy_excitation_is_offset_noise_and_discards_mb_padding() {
        for shape in 0..6 {
            let frame = geometry(shape);
            for index in 0..6 {
                let kind = kind(index);
                let mut decoder = Decoder::new(&[0; 128]);
                let mut output = [i32::MIN; MAX_SAMPLES + 1];
                let samples = decode_excitation(&mut decoder, frame, kind, &mut output).unwrap();
                assert_eq!(samples, frame.samples());
                assert!(
                    output[..samples]
                        .iter()
                        .all(|&value| value.abs() == kind.quantization_offset())
                );
                assert!(output[samples..].iter().all(|&value| value == i32::MIN));
            }
        }
    }
}
