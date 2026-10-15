// SPDX-License-Identifier: MIT
//! Allocation-free Opus frame splitting, concatenation and padding.
//!
//! Repacketization changes transport framing without decoding or re-encoding
//! audio. Borrowed input packets must remain available while the builder exists.
//! Only base audio frames are retained; input padding and its extensions are
//! discarded, and any padding requested on output contains zero bytes.

use super::packet::{MAX_FRAMES, MAX_PACKET_SAMPLES_48KHZ, Packet, PacketError, Toc};
use std::fmt;
use std::ops::Range;

/// Invalid input, incompatible frame geometry, range or output capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Input packet framing is invalid.
    Packet(PacketError),
    /// Configuration or encoded channel count differs from preceding frames.
    Incompatible,
    /// The combined packet would exceed 48 frames or 120 ms.
    Duration,
    /// The requested frame range is empty or outside the retained frames.
    Range,
    /// Output capacity or the requested padded length is insufficient.
    Output,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Packet(error) => error.fmt(formatter),
            Self::Incompatible => formatter.write_str("incompatible Opus frame configuration"),
            Self::Duration => formatter.write_str("repacketized Opus duration exceeds 120 ms"),
            Self::Range => formatter.write_str("invalid Opus frame range"),
            Self::Output => formatter.write_str("Opus repacketizer output is too small"),
        }
    }
}

impl std::error::Error for Error {}
impl From<PacketError> for Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}

/// Fixed-capacity builder retaining at most 48 borrowed compressed frames.
#[derive(Debug, Clone)]
pub struct Repacketizer<'a> {
    toc: Option<Toc>,
    count: usize,
    frames: [&'a [u8]; MAX_FRAMES],
}

impl Default for Repacketizer<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Repacketizer<'a> {
    /// Start an empty builder without allocation.
    pub const fn new() -> Self {
        Self {
            toc: None,
            count: 0,
            frames: [&[]; MAX_FRAMES],
        }
    }

    /// Number of retained frames, including zero-byte DTX frames.
    pub const fn frame_count(&self) -> usize {
        self.count
    }

    /// Configuration of retained frames, or `None` for an empty builder.
    pub const fn toc(&self) -> Option<Toc> {
        self.toc
    }

    /// Drop retained frame references and start a new packet geometry.
    pub fn clear(&mut self) {
        self.toc = None;
        self.count = 0;
        self.frames.fill(&[]);
    }

    /// Validate and append an ordinary Opus packet. Refusal preserves state.
    pub fn push(&mut self, bytes: &'a [u8]) -> Result<(), Error> {
        self.push_packet(&Packet::parse(bytes)?)
    }

    /// Append already parsed ordinary or self-delimiting frames.
    ///
    /// Configuration, internal channel count and individual frame duration must
    /// match. Different compressed lengths and packet packing codes are allowed.
    pub fn push_packet(&mut self, packet: &Packet<'a>) -> Result<(), Error> {
        let toc = packet.toc();
        if self
            .toc
            .is_some_and(|previous| previous.byte() & 0xfc != toc.byte() & 0xfc)
        {
            return Err(Error::Incompatible);
        }
        let count = self.count + packet.frame_count();
        if count > MAX_FRAMES || count * toc.frame_samples(48_000)? > MAX_PACKET_SAMPLES_48KHZ {
            return Err(Error::Duration);
        }
        for (target, frame) in self.frames[self.count..count]
            .iter_mut()
            .zip(packet.frames())
        {
            *target = frame;
        }
        self.count = count;
        self.toc = Some(toc);
        Ok(())
    }

    /// Minimum ordinary packet size for a nonempty retained frame range.
    pub fn output_size(&self, frames: Range<usize>) -> Result<usize, Error> {
        self.size(frames, false)
    }

    /// Minimum Appendix B packet size for a nonempty retained frame range.
    pub fn self_delimited_size(&self, frames: Range<usize>) -> Result<usize, Error> {
        self.size(frames, true)
    }

    /// Write the shortest ordinary framing for a retained frame range.
    /// Invalid ranges or insufficient capacity leave `output` untouched.
    pub fn write(&self, frames: Range<usize>, output: &mut [u8]) -> Result<usize, Error> {
        self.write_inner(frames, output, false, None)
    }

    /// Write the shortest self-delimiting framing for a retained frame range.
    pub fn write_self_delimited(
        &self,
        frames: Range<usize>,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        self.write_inner(frames, output, true, None)
    }

    /// Write ordinary framing with an exact requested total byte length.
    ///
    /// Added padding bytes are zeroed. A request equal to [`Self::output_size`]
    /// uses the shortest framing; larger requests select code 3 as required.
    pub fn write_padded(
        &self,
        frames: Range<usize>,
        output: &mut [u8],
        length: usize,
    ) -> Result<usize, Error> {
        self.write_inner(frames, output, false, Some(length))
    }

    fn selected(&self, range: Range<usize>) -> Result<&[&'a [u8]], Error> {
        if range.start >= range.end || range.end > self.count {
            return Err(Error::Range);
        }
        Ok(&self.frames[range])
    }

    fn size(&self, range: Range<usize>, delimited: bool) -> Result<usize, Error> {
        let frames = self.selected(range)?;
        let variable = frames.windows(2).any(|pair| pair[0].len() != pair[1].len());
        Ok(packet_size(frames, variable, false, delimited))
    }

    fn write_inner(
        &self,
        range: Range<usize>,
        output: &mut [u8],
        delimited: bool,
        padded: Option<usize>,
    ) -> Result<usize, Error> {
        let frames = self.selected(range)?;
        let variable = frames.windows(2).any(|pair| pair[0].len() != pair[1].len());
        let minimum = packet_size(frames, variable, false, delimited);
        let target = padded.unwrap_or(minimum);
        let force_code_three = target > minimum;
        let base = packet_size(frames, variable, force_code_three, delimited);
        if target < base || output.len() < target {
            return Err(Error::Output);
        }
        let padding = target - base;
        let code = match frames.len() {
            1 if !force_code_three => 0,
            2 if !force_code_three && !variable => 1,
            2 if !force_code_three => 2,
            _ => 3,
        };
        output[0] = (self.toc.expect("nonempty builder has TOC").byte() & 0xfc) | code;
        let mut position = 1;
        if code == 3 {
            output[position] =
                frames.len() as u8 | (u8::from(variable) << 7) | (u8::from(padding != 0) << 6);
            position += 1;
            if padding != 0 {
                let continuation = (padding - 1) / 255;
                output[position..position + continuation].fill(255);
                position += continuation;
                output[position] = ((padding - 1) % 255) as u8;
                position += 1;
            }
        }
        if variable && frames.len() > 1 {
            for frame in &frames[..frames.len() - 1] {
                write_length(frame.len(), output, &mut position);
            }
        }
        if delimited {
            write_length(frames.last().unwrap().len(), output, &mut position);
        }
        for frame in frames {
            output[position..position + frame.len()].copy_from_slice(frame);
            position += frame.len();
        }
        output[position..target].fill(0);
        Ok(target)
    }
}

fn packet_size(frames: &[&[u8]], variable: bool, force_three: bool, delimited: bool) -> usize {
    let headers = 1 + usize::from(frames.len() > 2 || force_three);
    let explicit = if variable {
        frames[..frames.len() - 1]
            .iter()
            .map(|frame| length_size(frame.len()))
            .sum()
    } else {
        0
    };
    headers
        + explicit
        + usize::from(delimited) * length_size(frames.last().unwrap().len())
        + frames.iter().map(|frame| frame.len()).sum::<usize>()
}

fn length_size(length: usize) -> usize {
    1 + usize::from(length >= 252)
}

fn write_length(length: usize, output: &mut [u8], position: &mut usize) {
    if length < 252 {
        output[*position] = length as u8;
        *position += 1;
    } else {
        output[*position] = (252 + length % 4) as u8;
        output[*position + 1] = ((length - 252) / 4) as u8;
        *position += 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_framing_matches_public_libopus_goldens() {
        // Frozen output from opus_repacketizer_cat/out_range in libopus 1.6.1.
        type Case<'a> = (&'a [&'a [u8]], &'a [u8]);
        let cases: &[Case<'_>] = &[
            (&[&[128, 97, 98, 99]], &[128, 97, 98, 99]),
            (&[&[128, 97], &[128, 98]], &[129, 97, 98]),
            (&[&[128, 97], &[128, 98, 99]], &[130, 1, 97, 98, 99]),
            (&[&[128, 97], &[128, 98], &[128, 99]], &[131, 3, 97, 98, 99]),
            (
                &[&[128], &[128, 97], &[128, 98, 99]],
                &[131, 131, 0, 1, 97, 98, 99],
            ),
        ];
        for &(inputs, expected) in cases {
            let mut builder = Repacketizer::new();
            for input in inputs {
                builder.push(input).unwrap();
            }
            let mut output = [0xa5; 64];
            let size = builder
                .write(0..builder.frame_count(), &mut output)
                .unwrap();
            assert_eq!(&output[..size], expected);
            assert!(output[size..].iter().all(|&value| value == 0xa5));
            assert_eq!(builder.output_size(0..builder.frame_count()).unwrap(), size);
        }
    }

    #[test]
    fn padding_matches_native_small_headers_and_all_lacing_boundaries() {
        let mut builder = Repacketizer::new();
        builder.push(&[128, 97, 98, 99]).unwrap();
        let goldens: &[&[u8]] = &[
            &[128, 97, 98, 99],
            &[131, 1, 97, 98, 99],
            &[131, 65, 0, 97, 98, 99],
            &[131, 65, 1, 97, 98, 99, 0],
            &[131, 65, 2, 97, 98, 99, 0, 0],
        ];
        let mut output = [0xa5; 4100];
        for target in 4..=4100 {
            output.fill(0xa5);
            assert_eq!(
                builder.write_padded(0..1, &mut output, target).unwrap(),
                target
            );
            let packet = Packet::parse(&output[..target]).unwrap();
            assert_eq!(packet.frames().collect::<Vec<_>>(), [&[97, 98, 99][..]]);
            assert!(output[target..].iter().all(|&value| value == 0xa5));
            if let Some(expected) = goldens.get(target - 4) {
                assert_eq!(&output[..target], *expected);
            }
            let padding = packet.padding_len();
            assert!(
                output[target - padding..target]
                    .iter()
                    .all(|&value| value == 0)
            );
        }
    }

    #[test]
    fn self_delimiting_frames_and_ranges_preserve_payloads() {
        for lengths in [
            vec![0],
            vec![251],
            vec![252],
            vec![1275],
            vec![0, 0],
            vec![251, 252],
            vec![0, 1, 251, 252, 1275],
            vec![1275; 48],
        ] {
            let inputs = lengths
                .iter()
                .enumerate()
                .map(|(index, &length)| {
                    let mut packet = vec![128];
                    packet.resize(length + 1, index as u8);
                    packet
                })
                .collect::<Vec<_>>();
            let mut builder = Repacketizer::new();
            for packet in &inputs {
                builder.push(packet).unwrap();
            }
            for begin in 0..inputs.len() {
                let range = begin..inputs.len();
                let size = builder.self_delimited_size(range.clone()).unwrap();
                let mut output = vec![0xa5; size + 8];
                assert_eq!(
                    builder
                        .write_self_delimited(range.clone(), &mut output)
                        .unwrap(),
                    size
                );
                let (packet, consumed) = Packet::parse_self_delimited(&output).unwrap();
                assert_eq!(consumed, size);
                assert_eq!(
                    packet.frames().collect::<Vec<_>>(),
                    inputs[range]
                        .iter()
                        .map(|packet| &packet[1..])
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn refusal_preserves_builder_and_output_and_clear_allows_new_geometry() {
        let mut builder = Repacketizer::new();
        builder.push(&[128, 10]).unwrap();
        assert_eq!(builder.push(&[132]), Err(Error::Incompatible));
        assert!(builder.push(&[]).is_err());
        assert_eq!(builder.frame_count(), 1);
        let mut output = [0xa5; 1];
        assert_eq!(builder.write(0..1, &mut output), Err(Error::Output));
        assert_eq!(output, [0xa5]);
        assert_eq!(builder.write(0..0, &mut output), Err(Error::Range));
        assert_eq!(builder.write(0..usize::MAX, &mut output), Err(Error::Range));
        assert_eq!(output, [0xa5]);
        builder.clear();
        assert_eq!(builder.frame_count(), 0);
        assert_eq!(builder.toc(), None);
        builder.push(&[132]).unwrap();
    }

    #[test]
    fn every_configuration_obeys_120_ms_and_dtx_frame_limits() {
        for config in 0u8..32 {
            let input = [config << 3];
            let limit =
                MAX_PACKET_SAMPLES_48KHZ / Toc::from_byte(input[0]).frame_samples(48_000).unwrap();
            let mut builder = Repacketizer::new();
            for _ in 0..limit {
                builder.push(&input).unwrap();
            }
            assert_eq!(builder.push(&input), Err(Error::Duration));
            assert_eq!(builder.frame_count(), limit);
            let mut output = [0; 64];
            let size = builder.write(0..limit, &mut output).unwrap();
            let packet = Packet::parse(&output[..size]).unwrap();
            assert_eq!(packet.frame_count(), limit);
            assert!(packet.frames().all(|frame| frame.is_empty()));
        }
    }
}
