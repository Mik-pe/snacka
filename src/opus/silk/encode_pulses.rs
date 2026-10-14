// SPDX-License-Identifier: MIT
//! Owned inverse of the SILK excitation grammar, RFC 6716 section 4.2.7.8.

use super::excitation::{COUNT, LSB, RATE, SHELL, SIGN};
use super::tables::{COUNT_WEIGHTS, NEGATIVE_WEIGHTS, RATE_WEIGHTS, SHELL_WEIGHTS};
use super::{Frame, FrameType, SignalType};
use crate::entropy::{Encoder, Error};

const MAX_SAMPLES: usize = 320;
const MAX_BLOCKS: usize = MAX_SAMPLES / 16;

// Rounded -log2(weight/256) in Q16. Binary logarithm by repeated squaring
// creates these mathematical cost tables at compile time, without runtime
// logarithms or a second copy of the normative probability models.
const fn symbol_cost(weight: u16) -> u32 {
    if weight == 0 {
        return u32::MAX;
    }
    let integer = 15 - weight.leading_zeros();
    let mut value = (weight as u128) << (47 - integer);
    let mut fraction = 0;
    let mut bit = 24;
    while bit != 0 {
        bit -= 1;
        value = (value * value) >> 47;
        if value >= 1 << 48 {
            value >>= 1;
            fraction |= 1 << bit;
        }
    }
    ((8 - integer) * 16777216 - fraction + 128) >> 8
}

const fn costs<const N: usize>(weights: &[u16; N]) -> [u32; N] {
    let mut result = [0; N];
    let mut index = 0;
    while index < N {
        result[index] = symbol_cost(weights[index]);
        index += 1;
    }
    result
}

const RATE_COST: [[u32; 9]; 2] = [costs(&RATE_WEIGHTS[0]), costs(&RATE_WEIGHTS[1])];
const COUNT_COST: [[u32; 18]; 11] = {
    let mut result = [[0; 18]; 11];
    let mut index = 0;
    while index < 11 {
        result[index] = costs(&COUNT_WEIGHTS[index]);
        index += 1;
    }
    result
};
const SHELL_COST: [[[u32; 17]; 16]; 4] = {
    let mut result = [[[0; 17]; 16]; 4];
    let mut level = 0;
    while level < 4 {
        let mut count = 0;
        while count < 16 {
            result[level][count] = costs(&SHELL_WEIGHTS[level][count]);
            count += 1;
        }
        level += 1;
    }
    result
};
const SIGN_COST: [[[u32; 2]; 7]; 6] = {
    let mut result = [[[0; 2]; 7]; 6];
    let mut kind = 0;
    while kind < 6 {
        let mut count = 0;
        while count < 7 {
            let negative = NEGATIVE_WEIGHTS[kind][count];
            result[kind][count] = costs(&[negative, 256 - negative]);
            count += 1;
        }
        kind += 1;
    }
    result
};
const LSB_COST: [u32; 2] = costs(&[136, 120]);

struct Plan {
    magnitudes: [u16; MAX_SAMPLES],
    counts: [u8; MAX_BLOCKS],
    shifts: [u8; MAX_BLOCKS],
    rate: usize,
    blocks: usize,
}

impl Plan {
    fn new(frame: Frame, kind: FrameType, pulses: &[i16]) -> Result<Self, Error> {
        if pulses.len() != frame.coded_samples() {
            return Err(Error::Argument);
        }
        let mut plan = Self {
            magnitudes: [0; MAX_SAMPLES],
            counts: [0; MAX_BLOCKS],
            shifts: [0; MAX_BLOCKS],
            rate: 0,
            blocks: frame.shell_blocks(),
        };
        for (&pulse, magnitude) in pulses.iter().zip(&mut plan.magnitudes) {
            *magnitude = pulse.unsigned_abs();
        }
        for (index, block) in plan.magnitudes[..pulses.len()]
            .as_chunks::<16>()
            .0
            .iter()
            .enumerate()
        {
            let mut shift = 0;
            let count = loop {
                let count: u32 = block.iter().map(|&value| u32::from(value >> shift)).sum();
                if count <= 16 {
                    break count;
                }
                if shift == 10 {
                    return Err(Error::Argument);
                }
                shift += 1;
            };
            plan.counts[index] = count as u8;
            plan.shifts[index] = shift;
        }
        let class = usize::from(kind.signal == SignalType::Voiced);
        let mut best = u32::MAX;
        for rate in 0..9 {
            let mut cost = RATE_COST[class][rate];
            for (&count, &shift) in plan.counts[..plan.blocks]
                .iter()
                .zip(&plan.shifts[..plan.blocks])
            {
                // Later escapes and shell/LSB/sign costs do not depend on rate.
                cost += COUNT_COST[rate][if shift == 0 { count as usize } else { 17 }];
            }
            if cost < best {
                best = cost;
                plan.rate = rate;
            }
        }
        Ok(plan)
    }

    fn cost(&self, kind: FrameType, pulses: &[i16]) -> u32 {
        let class = usize::from(kind.signal == SignalType::Voiced);
        let mut cost = RATE_COST[class][self.rate];
        for (index, block) in self.magnitudes[..pulses.len()]
            .as_chunks::<16>()
            .0
            .iter()
            .enumerate()
        {
            let count = self.counts[index] as usize;
            let shift = self.shifts[index];
            if shift == 0 {
                cost += COUNT_COST[self.rate][count];
            } else {
                cost += COUNT_COST[self.rate][17];
                cost += u32::from(shift - 1) * COUNT_COST[9][17];
                cost += COUNT_COST[if shift == 10 { 10 } else { 9 }][count];
                for &value in block {
                    for bit in (0..shift).rev() {
                        cost += LSB_COST[usize::from(value >> bit & 1)];
                    }
                }
            }
            cost += shell_cost::<0>(block, count, shift);
            let sign = &SIGN_COST[kind.sign_table()][count.min(6)];
            for &pulse in &pulses[16 * index..16 * (index + 1)] {
                if pulse != 0 {
                    cost += sign[usize::from(pulse > 0)];
                }
            }
        }
        cost
    }
}

fn shell_cost<const LEVEL: usize>(values: &[u16], total: usize, shift: u8) -> u32 {
    if total == 0 {
        return 0;
    }
    let (left, right) = values.split_at(values.len() / 2);
    let first = left
        .iter()
        .map(|&value| usize::from(value >> shift))
        .sum::<usize>();
    let cost = SHELL_COST[LEVEL][total - 1][first];
    match LEVEL {
        0 => {
            cost + shell_cost::<1>(left, first, shift)
                + shell_cost::<1>(right, total - first, shift)
        }
        1 => {
            cost + shell_cost::<2>(left, first, shift)
                + shell_cost::<2>(right, total - first, shift)
        }
        2 => {
            cost + shell_cost::<3>(left, first, shift)
                + shell_cost::<3>(right, total - first, shift)
        }
        3 => cost,
        _ => unreachable!("four-level shell geometry"),
    }
}

fn shell<const LEVEL: usize>(
    encoder: &mut Encoder<'_>,
    values: &[u16],
    total: usize,
    shift: u8,
) -> Result<(), Error> {
    if total == 0 {
        return Ok(());
    }
    let (left, right) = values.split_at(values.len() / 2);
    let first = left
        .iter()
        .map(|&value| usize::from(value >> shift))
        .sum::<usize>();
    encoder.symbol(first, SHELL[LEVEL][total - 1])?;
    match LEVEL {
        0 => {
            shell::<1>(encoder, left, first, shift)?;
            shell::<1>(encoder, right, total - first, shift)?;
        }
        1 => {
            shell::<2>(encoder, left, first, shift)?;
            shell::<2>(encoder, right, total - first, shift)?;
        }
        2 => {
            shell::<3>(encoder, left, first, shift)?;
            shell::<3>(encoder, right, total - first, shift)?;
        }
        3 => {}
        _ => unreachable!("four-level shell geometry"),
    }
    Ok(())
}

/// Estimate the excitation symbols' entropy cost, rounded per symbol to 1/65,536 bit.
///
/// Includes rate/count/shell/LSB/sign symbols, excluding range termination and
/// the preceding seed. Magnitudes must fit the ten-bit escape domain and input
/// length must equal [`Frame::coded_samples`], including MB padding. No entropy
/// state, output or allocation is involved. Invalid inputs return `Argument`.
pub fn estimate_pulse_bits(frame: Frame, kind: FrameType, pulses: &[i16]) -> Result<f32, Error> {
    let plan = Plan::new(frame, kind, pulses)?;
    Ok(plan.cost(kind, pulses) as f32 / 65536.0)
}

/// Encode signed SILK excitation pulses into the existing arithmetic interval.
///
/// Length and escape-domain validation finish before touching encoder state.
/// Input is immutable and must contain every coded sample, including MB padding.
/// Rate selection minimizes a fixed Q16 estimate; shell, LSB and sign sections
/// follow the normative symbol order. No heap allocations or runtime logarithms
/// occur. An insufficient destination returns the encoder's terminal `Output`
/// error; this function does not encode the preceding two-bit seed.
pub fn encode_pulses(
    encoder: &mut Encoder<'_>,
    frame: Frame,
    kind: FrameType,
    pulses: &[i16],
) -> Result<(), Error> {
    let plan = Plan::new(frame, kind, pulses)?;
    encoder.symbol(
        plan.rate,
        RATE[usize::from(kind.signal == SignalType::Voiced)],
    )?;
    for (&count, &shift) in plan.counts[..plan.blocks]
        .iter()
        .zip(&plan.shifts[..plan.blocks])
    {
        if shift == 0 {
            encoder.symbol(count as usize, COUNT[plan.rate])?;
        } else {
            encoder.symbol(17, COUNT[plan.rate])?;
            for _ in 1..shift {
                encoder.symbol(17, COUNT[9])?;
            }
            encoder.symbol(count as usize, COUNT[if shift == 10 { 10 } else { 9 }])?;
        }
    }
    for (index, block) in plan.magnitudes[..pulses.len()]
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
    {
        shell::<0>(
            encoder,
            block,
            plan.counts[index] as usize,
            plan.shifts[index],
        )?;
    }
    for (index, block) in plan.magnitudes[..pulses.len()]
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
    {
        for &value in block {
            for bit in (0..plan.shifts[index]).rev() {
                encoder.symbol(usize::from(value >> bit & 1), LSB)?;
            }
        }
    }
    for (index, block) in pulses.as_chunks::<16>().0.iter().enumerate() {
        let model = SIGN[kind.sign_table()][usize::from(plan.counts[index].min(6))];
        for &pulse in block {
            if pulse != 0 {
                encoder.symbol(usize::from(pulse > 0), model)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::Decoder;
    use crate::opus::silk::{Bandwidth, FrameDuration, QuantizationOffset, decode_pulses};

    fn roundtrip(frame: Frame, kind: FrameType, pulses: &[i16]) {
        let mut packet = [0; 4096];
        let mut encoder = Encoder::new(&mut packet);
        let before = encoder.tell_eighths();
        encode_pulses(&mut encoder, frame, kind, pulses).unwrap();
        let cost = estimate_pulse_bits(frame, kind, pulses).unwrap();
        assert!((cost - (encoder.tell_eighths() - before) as f32 / 8.0).abs() < 2.0);
        let range = encoder.range();
        let fraction = encoder.tell_eighths();
        let length = encoder.finish().unwrap();
        let mut decoder = Decoder::new(&packet[..length]);
        let mut result = [0; MAX_SAMPLES];
        decode_pulses(&mut decoder, frame, kind, &mut result).unwrap();
        assert_eq!(&result[..pulses.len()], pulses);
        assert_eq!((decoder.range(), decoder.tell_eighths()), (range, fraction));
    }

    #[test]
    fn every_probability_cost_matches_independent_floating_logarithms() {
        for weight in 1..=256 {
            let expected = -((weight as f64) / 256.0).log2();
            assert!(
                (f64::from(symbol_cost(weight)) / 65536.0 - expected).abs() <= 0.5 / 65536.0 + 1e-8
            );
        }
    }

    #[test]
    fn all_external_decoder_pulse_vectors_reencode_exactly() {
        let bytes = include_bytes!("../../../tests/fixtures/opus-silk-pulses.bin");
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let mut cursor = 12;
        for _ in 0..count {
            let shape = bytes[cursor];
            let kind = bytes[cursor + 1];
            let length =
                u16::from_le_bytes(bytes[cursor + 2..cursor + 4].try_into().unwrap()) as usize;
            let samples =
                u16::from_le_bytes(bytes[cursor + 4..cursor + 6].try_into().unwrap()) as usize;
            cursor += 18 + length;
            let mut pulses = [0; MAX_SAMPLES];
            for value in &mut pulses[..samples] {
                *value = i16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap());
                cursor += 2;
            }
            let frame = Frame {
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
            };
            let kind = FrameType {
                signal: match kind / 2 {
                    0 => SignalType::Inactive,
                    1 => SignalType::Unvoiced,
                    _ => SignalType::Voiced,
                },
                offset: if kind & 1 == 0 {
                    QuantizationOffset::Low
                } else {
                    QuantizationOffset::High
                },
            };
            roundtrip(frame, kind, &pulses[..samples]);
        }
        assert_eq!(cursor, bytes.len());
    }

    #[test]
    fn invalid_domain_and_length_preserve_encoder_and_destination() {
        let frame = Frame {
            bandwidth: Bandwidth::Wide,
            duration: FrameDuration::Ms20,
        };
        let kind = FrameType {
            signal: SignalType::Unvoiced,
            offset: QuantizationOffset::Low,
        };
        let mut output = [0xa5; 2048];
        let mut encoder = Encoder::new(&mut output);
        let initial = (encoder.range(), encoder.tell_eighths());
        assert_eq!(
            encode_pulses(&mut encoder, frame, kind, &[0; 319]),
            Err(Error::Argument)
        );
        assert_eq!(
            encode_pulses(&mut encoder, frame, kind, &[i16::MIN; 320]),
            Err(Error::Argument)
        );
        assert_eq!((encoder.range(), encoder.tell_eighths()), initial);
        assert_eq!(output, [0xa5; 2048]);
        for pulse in [0, 1, -1, 17407, -17407] {
            let mut input = [0; 320];
            input[0] = pulse;
            roundtrip(frame, kind, &input);
        }
    }
}
