// SPDX-License-Identifier: MIT
//! Opus packet framing defined by RFC 6716 section 3.
//!
//! Parsing borrows the encoded frames without decoding audio or allocating memory.

use std::fmt;

/// Invalid Opus packet framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    /// The packet or a required header byte is missing.
    Truncated,
    /// Frame count or total duration exceeds the format limits.
    Duration,
    /// A frame is larger than 1,275 bytes or its declared length is invalid.
    Length,
    /// Padding consumes more bytes than the packet contains.
    Padding,
    /// The output sampling rate is not supported by Opus.
    SampleRate,
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Truncated => "truncated Opus packet header",
            Self::Duration => "invalid Opus packet duration",
            Self::Length => "invalid Opus frame length",
            Self::Padding => "invalid Opus packet padding",
            Self::SampleRate => "unsupported Opus sample rate",
        })
    }
}

impl std::error::Error for PacketError {}

/// A validated single-stream Opus packet with borrowed frame payloads.
#[derive(Debug)]
pub struct Packet<'a> {
    bytes: &'a [u8],
    configuration: u8,
    channels: u8,
    count: u8,
    start: usize,
    lengths: [u16; 48],
}

fn byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, PacketError> {
    let value = *bytes.get(*cursor).ok_or(PacketError::Truncated)?;
    *cursor += 1;
    Ok(value)
}

fn frame_length(bytes: &[u8], cursor: &mut usize) -> Result<usize, PacketError> {
    let first = usize::from(byte(bytes, cursor)?);
    if first < 252 {
        Ok(first)
    } else {
        Ok(first + 4 * usize::from(byte(bytes, cursor)?))
    }
}

impl<'a> Packet<'a> {
    /// Validate framing and locate all frames in a packet.
    ///
    /// Zero-length frames are valid and request concealment from an audio decoder.
    /// Transport headers and the optional self-delimiting multistream layout
    /// are outside this parser's scope.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, PacketError> {
        let mut cursor = 0;
        let toc = byte(bytes, &mut cursor)?;
        let mut packet = Self {
            bytes,
            configuration: toc >> 3,
            channels: 1 + ((toc >> 2) & 1),
            count: 1,
            start: 0,
            lengths: [0; 48],
        };
        let mut end = bytes.len();
        let mut variable = false;
        match toc & 3 {
            0 => {}
            1 => packet.count = 2,
            2 => {
                packet.count = 2;
                variable = true;
            }
            3 => {
                let header = byte(bytes, &mut cursor)?;
                packet.count = header & 63;
                variable = header & 128 != 0;
                if header & 64 != 0 {
                    loop {
                        let padding = byte(&bytes[..end], &mut cursor)?;
                        // A continuation byte counts 254 padding bytes; its own
                        // header byte is accounted for by advancing the cursor.
                        let size = if padding == 255 {
                            254
                        } else {
                            usize::from(padding)
                        };
                        end = end.checked_sub(size).ok_or(PacketError::Padding)?;
                        if cursor > end {
                            return Err(PacketError::Padding);
                        }
                        if padding != 255 {
                            break;
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
        let count = usize::from(packet.count);
        if count == 0 || count > 48 || count * packet.frame_ticks() > 5_760 {
            return Err(PacketError::Duration);
        }
        if variable {
            let mut declared = 0;
            for length in &mut packet.lengths[..count - 1] {
                let size = frame_length(&bytes[..end], &mut cursor)?;
                declared += size;
                *length = size as u16;
            }
            let remainder = end
                .checked_sub(cursor + declared)
                .ok_or(PacketError::Length)?;
            if remainder > 1_275 {
                return Err(PacketError::Length);
            }
            packet.lengths[count - 1] = remainder as u16;
        } else {
            let payload = end.checked_sub(cursor).ok_or(PacketError::Length)?;
            if !payload.is_multiple_of(count) || payload / count > 1_275 {
                return Err(PacketError::Length);
            }
            packet.lengths[..count].fill((payload / count) as u16);
        }
        packet.start = cursor;
        Ok(packet)
    }

    fn frame_ticks(&self) -> usize {
        match self.configuration {
            0..=11 => [480, 960, 1_920, 2_880][usize::from(self.configuration & 3)],
            12..=15 => 480 << (self.configuration & 1),
            _ => 120 << (self.configuration & 3),
        }
    }

    /// The five-bit TOC configuration number, from 0 to 31.
    pub fn configuration(&self) -> u8 {
        self.configuration
    }

    /// Number of channels encoded in this packet.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Number of audio frames in this packet, from 1 to 48.
    pub fn frame_count(&self) -> usize {
        usize::from(self.count)
    }

    /// Total decoded samples per channel at a supported Opus output rate.
    pub fn samples(&self, rate: u32) -> Result<usize, PacketError> {
        if ![8_000, 12_000, 16_000, 24_000, 48_000].contains(&rate) {
            return Err(PacketError::SampleRate);
        }
        Ok(self.frame_count() * self.frame_ticks() * rate as usize / 48_000)
    }

    /// Iterate encoded frame payloads in stream order, excluding headers and padding.
    pub fn frames(&self) -> impl ExactSizeIterator<Item = &'a [u8]> + '_ {
        let mut cursor = self.start;
        self.lengths[..self.frame_count()]
            .iter()
            .map(move |&length| {
                let start = cursor;
                cursor += usize::from(length);
                &self.bytes[start..cursor]
            })
    }
}
