// SPDX-License-Identifier: MIT
//! Byte boundaries and range-coded transition side information from RFC 6716
//! sections 4.5.1.1–4.5.1.3. The caller owns CELT history, five-millisecond
//! redundancy coding and causal selection of beginning/end transitions.

use super::packet::{MAX_FRAME_BYTES, Mode};
use super::range::{Encoder, Error};

/// Location of the five-millisecond CELT signal within its enclosing LP frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Position {
    /// Current frame follows CELT; use preceding CELT state before Hybrid reset.
    Beginning,
    /// Current frame precedes CELT; reset before coding redundancy, then retain state.
    End,
}

/// The byte-aligned CELT payload appended after the complete main LP/Hybrid frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) position: Position,
    /// Bytes excluding the redundant frame's absent TOC byte.
    pub(crate) bytes: usize,
}

/// Encode transition side information after all SILK symbols and before Hybrid CELT.
///
/// `total_frame_bytes` is the enclosing frame's available payload budget,
/// including redundancy. Hybrid lengths use the standard 2+uint(256) syntax.
/// SILK lengths remain implicit; [`finish_silk`] determines their byte boundary.
/// Invalid dimensions and insufficient signaling space fail before changing the
/// arithmetic state. A subsequent entropy capacity failure requires a reset.
pub(crate) fn write_header(
    encoder: &mut Encoder<'_>,
    mode: Mode,
    total_frame_bytes: usize,
    transition: Option<Transition>,
) -> Result<(), Error> {
    if total_frame_bytes > MAX_FRAME_BYTES || total_frame_bytes == 0 {
        return Err(Error::Argument);
    }
    if let Some(transition) = transition
        && (mode == Mode::Celt
            || transition.bytes < 2
            || transition.bytes > total_frame_bytes
            || (mode == Mode::Hybrid && transition.bytes > 257))
    {
        return Err(Error::Argument);
    }
    let remaining = (total_frame_bytes as u64 * 8)
        .checked_sub(encoder.tell())
        .ok_or(Error::Output)?;
    if let Some(transition) = transition {
        let threshold = if mode == Mode::Hybrid { 37 } else { 17 };
        if remaining < threshold {
            return Err(Error::Output);
        }
        if mode == Mode::Hybrid {
            encoder.bit(true, 12)?;
        }
        encoder.bit(transition.position == Position::Beginning, 1)?;
        if mode == Mode::Hybrid {
            encoder.uniform((transition.bytes - 2) as u32, 256)?;
        }
    } else if mode == Mode::Hybrid && remaining >= 37 {
        encoder.bit(false, 12)?;
    }
    Ok(())
}

/// Compact arithmetic prefix and its implicit SILK/redundancy byte boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SilkBoundary {
    /// Byte boundary computed by the receiver's post-direction `ceil(tell/8)`.
    pub(crate) bytes: usize,
    /// Bytes actually written by arithmetic termination; zero any gap up to `bytes`.
    pub(crate) compact_bytes: usize,
    /// Main frame's final arithmetic interval, to XOR with redundant CELT's interval.
    pub(crate) range: u32,
}

/// Terminate a SILK main frame without changing its implicit redundancy boundary.
///
/// The caller must zero `[compact_bytes..bytes]` and append the CELT payload at
/// `bytes`. Padding the main frame to its original reserved capacity would make
/// a receiver treat those extra bytes as part of the redundant CELT frame.
pub(crate) fn finish_silk(encoder: Encoder<'_>) -> Result<SilkBoundary, Error> {
    let bytes = encoder.tell().div_ceil(8) as usize;
    let range = encoder.range();
    let compact_bytes = encoder.finish()?;
    if compact_bytes > bytes {
        return Err(Error::Corrupt);
    }
    Ok(SilkBoundary {
        bytes,
        compact_bytes,
        range,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::range::Decoder;

    fn prefix(encoder: &mut Encoder<'_>) {
        for value in 0..32 {
            encoder.uniform(value % 4, 4).unwrap();
        }
    }
    fn consume_prefix(decoder: &mut Decoder<'_>) {
        for value in 0..32 {
            assert_eq!(decoder.uniform(4).unwrap(), value % 4);
        }
    }

    #[test]
    fn exact_implicit_and_explicit_lengths_cover_two_and_257_bytes() {
        for mode in [Mode::Silk, Mode::Hybrid] {
            for position in [Position::Beginning, Position::End] {
                for redundancy_bytes in [2, 3, 255, 256, 257] {
                    let mut main = [0; 64];
                    let mut encoder = Encoder::new(&mut main);
                    prefix(&mut encoder);
                    write_header(
                        &mut encoder,
                        mode,
                        64 + redundancy_bytes,
                        Some(Transition {
                            position,
                            bytes: redundancy_bytes,
                        }),
                    )
                    .unwrap();
                    let range = encoder.range();
                    let length = if mode == Mode::Silk {
                        let boundary = finish_silk(encoder).unwrap();
                        assert_eq!(boundary.range, range);
                        main[boundary.compact_bytes..boundary.bytes].fill(0);
                        boundary.bytes
                    } else {
                        encoder.finish_padded().unwrap()
                    };
                    let mut frame = main[..length].to_vec();
                    frame.extend((0..redundancy_bytes).map(|index| index as u8));
                    let mut decoder = Decoder::new(&frame);
                    consume_prefix(&mut decoder);
                    assert!(
                        frame.len() as u64 * 8 - decoder.tell()
                            >= if mode == Mode::Hybrid { 37 } else { 17 }
                    );
                    if mode == Mode::Hybrid {
                        assert!(decoder.bit(12).unwrap());
                    }
                    assert_eq!(decoder.bit(1).unwrap(), position == Position::Beginning);
                    let parsed_bytes = if mode == Mode::Hybrid {
                        decoder.uniform(256).unwrap() as usize + 2
                    } else {
                        frame.len() - decoder.tell().div_ceil(8) as usize
                    };
                    assert_eq!(parsed_bytes, redundancy_bytes);
                    assert_eq!(decoder.range(), range);
                }
            }
        }
    }

    #[test]
    fn signaling_threshold_and_invalid_arguments_preserve_arithmetic_state() {
        for (mode, total, transition, error) in [
            (
                Mode::Celt,
                10,
                Some(Transition {
                    position: Position::Beginning,
                    bytes: 2,
                }),
                Error::Argument,
            ),
            (
                Mode::Hybrid,
                50,
                Some(Transition {
                    position: Position::Beginning,
                    bytes: 258,
                }),
                Error::Argument,
            ),
            (
                Mode::Silk,
                50,
                Some(Transition {
                    position: Position::Beginning,
                    bytes: 1,
                }),
                Error::Argument,
            ),
            (
                Mode::Silk,
                2,
                Some(Transition {
                    position: Position::End,
                    bytes: 2,
                }),
                Error::Output,
            ),
            (
                Mode::Hybrid,
                4,
                Some(Transition {
                    position: Position::End,
                    bytes: 2,
                }),
                Error::Output,
            ),
            (Mode::Hybrid, 0, None, Error::Argument),
            (Mode::Hybrid, usize::MAX, None, Error::Argument),
        ] {
            let mut output = [0; 64];
            let mut encoder = Encoder::new(&mut output);
            let state = (encoder.tell(), encoder.range());
            assert_eq!(
                write_header(&mut encoder, mode, total, transition),
                Err(error)
            );
            assert_eq!((encoder.tell(), encoder.range()), state);
        }
        for total in [4, 5] {
            let mut output = [0; 5];
            let mut encoder = Encoder::new(&mut output[..total]);
            let before = encoder.range();
            write_header(&mut encoder, Mode::Hybrid, total, None).unwrap();
            let after = encoder.range();
            encoder.finish_padded().unwrap();
            assert_eq!(before == after, total == 4);
            if total == 5 {
                assert!(!Decoder::new(&output).bit(12).unwrap());
            }
        }
    }

    #[test]
    fn compact_termination_never_moves_the_silk_boundary() {
        let mut seed = 0x6716u32;
        for symbols in 0..256 {
            let mut output = [0; 512];
            let mut encoder = Encoder::new(&mut output);
            for _ in 0..symbols {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                encoder.uniform(seed % 113, 113).unwrap();
            }
            encoder.bit(true, 1).unwrap();
            let boundary = finish_silk(encoder).unwrap();
            assert!(boundary.compact_bytes <= boundary.bytes);
            assert!(boundary.bytes <= output.len());
        }
    }
}
