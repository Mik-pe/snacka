// SPDX-License-Identifier: MIT AND BSD-3-Clause
//! CELT frame flags and per-band time/frequency signaling from RFC 6716.

use super::range::{Decoder, Encoder, Error};
use super::transform::PostFilterParams;

// RFC 6716 tables 60--63. Index: transient*4 + selection*2 + change.
const TF: [[i8; 8]; 4] = [
    [0, -1, 0, -1, 0, -1, 0, -1],
    [0, -1, 0, -2, 1, 0, 1, -1],
    [0, -2, 0, -3, 2, 0, 1, -1],
    [0, -2, 0, -3, 3, 0, 1, -1],
];

pub(crate) struct Header {
    pub(crate) silence: bool,
    pub(crate) transient: bool,
    pub(crate) filter: PostFilterParams,
}

#[cfg(test)]
pub(crate) fn decode_header(
    decoder: &mut Decoder<'_>,
    lm: u8,
    packet_bits: usize,
) -> Result<Header, Error> {
    decode_header_with_start(decoder, lm, 0, packet_bits)
}

pub(crate) fn decode_header_with_start(
    decoder: &mut Decoder<'_>,
    lm: u8,
    start_band: usize,
    packet_bits: usize,
) -> Result<Header, Error> {
    if lm > 3 || start_band >= 21 {
        return Err(Error::Argument);
    }
    let mut used = decoder.tell() as usize;
    let silence = used >= packet_bits || (used == 1 && decoder.bit(15)?);
    if silence {
        return Ok(Header {
            silence: true,
            transient: false,
            filter: PostFilterParams::DISABLED,
        });
    }
    used = decoder.tell() as usize;
    let mut filter = PostFilterParams::DISABLED;
    if start_band == 0 && used + 16 <= packet_bits {
        if decoder.bit(1)? {
            // Table 56 fixes six octaves; the largest period is 1022.
            let octave = decoder.uniform(6)?;
            let period = (16usize << octave) + decoder.raw(4 + octave)? as usize - 1;
            let gain = (decoder.raw(3)? + 1) as f32 * (3.0 / 32.0);
            let tapset = if decoder.tell() as usize + 2 <= packet_bits {
                decoder.icdf(&[2, 1, 0], 2)?
            } else {
                0
            };
            filter = PostFilterParams::new(period, gain, tapset).map_err(|_| Error::Corrupt)?;
        }
        used = decoder.tell() as usize;
    }
    let transient = lm > 0 && used + 3 <= packet_bits && decoder.bit(3)?;
    Ok(Header {
        silence,
        transient,
        filter,
    })
}

pub(crate) fn encode_header(
    encoder: &mut Encoder<'_>,
    lm: u8,
    packet_bits: usize,
    transient: bool,
) -> Result<(), Error> {
    if lm > 3 || (lm == 0 && transient) || packet_bits < 16 {
        return Err(Error::Argument);
    }
    encoder.bit(false, 15)?;
    if encoder.tell() as usize + 16 <= packet_bits {
        encoder.bit(false, 1)?;
    }
    if lm > 0 && encoder.tell() as usize + 3 <= packet_bits {
        encoder.bit(transient, 3)?;
    }
    Ok(())
}

pub(crate) fn decode_tf(
    decoder: &mut Decoder<'_>,
    lm: u8,
    transient: bool,
    start: usize,
    end: usize,
    packet_bits: usize,
) -> Result<[i8; 21], Error> {
    if lm > 3 || start >= end || end > 21 {
        return Err(Error::Argument);
    }
    let first_logp = if transient { 2 } else { 4 };
    let reserve = lm > 0 && decoder.tell() as usize + first_logp < packet_bits;
    let budget = packet_bits.saturating_sub(usize::from(reserve));
    let mut output = [0i8; 21];
    let mut changed = false;
    let mut current = false;
    for (offset, value) in output[start..end].iter_mut().enumerate() {
        let logp = if offset == 0 {
            first_logp
        } else if transient {
            4
        } else {
            5
        };
        if decoder.tell() as usize + logp <= budget {
            current ^= decoder.bit(logp as u32)?;
            changed |= current;
        }
        *value = i8::from(current);
    }
    let base = 4 * usize::from(transient);
    let table = TF[usize::from(lm)];
    let differs = table[base + usize::from(changed)] != table[base + 2 + usize::from(changed)];
    let selection = reserve && differs && decoder.bit(1)?;
    for value in &mut output[start..end] {
        *value = table[base + 2 * usize::from(selection) + *value as usize];
    }
    Ok(output)
}

pub(crate) struct TfChoices {
    pub(crate) changes: [bool; 21],
    pub(crate) selection: bool,
}

pub(crate) fn encode_tf(
    encoder: &mut Encoder<'_>,
    lm: u8,
    transient: bool,
    start: usize,
    end: usize,
    packet_bits: usize,
    decisions: &TfChoices,
) -> Result<[i8; 21], Error> {
    let choices = &decisions.changes;
    let selection = decisions.selection;
    if lm > 3 || start >= end || end > 21 {
        return Err(Error::Argument);
    }
    let first_logp = if transient { 2 } else { 4 };
    let reserve = lm > 0 && encoder.tell() as usize + first_logp < packet_bits;
    let budget = packet_bits.saturating_sub(usize::from(reserve));
    let mut output = [0i8; 21];
    let mut changed = false;
    let mut current = false;
    for index in start..end {
        let logp = if index == start {
            first_logp
        } else if transient {
            4
        } else {
            5
        };
        if encoder.tell() as usize + logp <= budget {
            encoder.bit(current ^ choices[index], logp as u32)?;
            current = choices[index];
            changed |= current;
        }
        output[index] = i8::from(current);
    }
    let base = 4 * usize::from(transient);
    let table = TF[usize::from(lm)];
    let differs = table[base + usize::from(changed)] != table[base + 2 + usize::from(changed)];
    let selection = if reserve && differs {
        encoder.bit(selection, 1)?;
        selection
    } else {
        false
    };
    for value in &mut output[start..end] {
        *value = table[base + 2 * usize::from(selection) + *value as usize];
    }
    Ok(output)
}

pub(crate) fn decode_spread(decoder: &mut Decoder<'_>, packet_bits: usize) -> Result<u8, Error> {
    if decoder.tell() as usize + 4 <= packet_bits {
        Ok(decoder.icdf(&[25, 23, 2, 0], 5)? as u8)
    } else {
        Ok(2)
    }
}

pub(crate) fn encode_spread(
    encoder: &mut Encoder<'_>,
    packet_bits: usize,
    spread: u8,
) -> Result<u8, Error> {
    if spread > 3 {
        return Err(Error::Argument);
    }
    if encoder.tell() as usize + 4 <= packet_bits {
        encoder.icdf(usize::from(spread), &[25, 23, 2, 0], 5)?;
        Ok(spread)
    } else {
        Ok(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_match_on_every_tf_choice_and_truncated_budget() {
        for lm in 0..=3 {
            for transient in [false, true] {
                if lm == 0 && transient {
                    continue;
                }
                for capacity in [2, 3, 8, 20, 80] {
                    for selection in [false, true] {
                        for mask in [0u32, 1, 0x55555, 0x1fffff] {
                            let choices = std::array::from_fn(|index| mask & (1 << index) != 0);
                            let mut bytes = [0; 80];
                            let packet_bits = capacity * 8;
                            let mut enc = Encoder::new(&mut bytes[..capacity]);
                            encode_header(&mut enc, lm, packet_bits, transient).unwrap();
                            let decisions = TfChoices {
                                changes: choices,
                                selection,
                            };
                            let tf =
                                encode_tf(&mut enc, lm, transient, 0, 21, packet_bits, &decisions)
                                    .unwrap();
                            let spread = encode_spread(&mut enc, packet_bits, 2).unwrap();
                            let range = enc.range();
                            enc.finish_padded().unwrap();
                            let mut dec = Decoder::new(&bytes[..capacity]);
                            let header = decode_header(&mut dec, lm, packet_bits).unwrap();
                            assert!(!header.silence);
                            assert_eq!(header.transient, transient);
                            assert_eq!(
                                decode_tf(&mut dec, lm, transient, 0, 21, packet_bits).unwrap(),
                                tf
                            );
                            assert_eq!(decode_spread(&mut dec, packet_bits).unwrap(), spread);
                            assert_eq!(dec.range(), range);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn recognizes_standard_silence_codeword() {
        let mut decoder = Decoder::new(&[0xff, 0xfe]);
        assert!(decode_header(&mut decoder, 3, 16).unwrap().silence);
    }
}
