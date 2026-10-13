// SPDX-License-Identifier: MIT
//! Owned SILK decoding specified by RFC 6716 section 4.2.
//!
//! [`SilkDecoder`] reconstructs normalized mono or interleaved stereo PCM at
//! the layer's 8, 12 or 16 kHz internal sample rate. Its range decoder is shared
//! with the enclosing Opus frame, including Hybrid mode. Bit-exact side
//! information and excitation are exposed separately from the non-bit-exact
//! prediction synthesis specified by section 4.2.7.9.

mod decoder;
mod excitation;
mod frame;
mod frame_tables;
mod lpc;
pub(crate) mod resample;
mod synthesis;
mod tables;

use crate::entropy::{Decoder, Model};

pub use decoder::SilkDecoder;
pub use excitation::{decode_excitation, decode_pulses, reconstruct_excitation};
pub use lpc::{
    LpcCoefficients, NlsfIndices, NlsfState, decode_nlsf_indices, interpolate_nlsf,
    inverse_prediction_gain, nlsf_to_lpc, reconstruct_nlsf, stabilize_nlsf,
};
pub(crate) use resample::Resampler;

/// Invalid SILK primitive argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The channel count or another parameter is outside the supported domain.
    Argument,
    /// The caller's output cannot hold the requested frame.
    Output,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Argument => "invalid SILK primitive argument",
            Self::Output => "SILK output buffer is too small",
        })
    }
}

impl std::error::Error for Error {}

/// Internal SILK sampling bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    /// Narrowband, with 8 kHz internal samples.
    Narrow,
    /// Mediumband, with 12 kHz internal samples.
    Medium,
    /// Wideband, with 16 kHz internal samples.
    Wide,
}

/// Duration of one internal SILK frame, before Opus frame grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDuration {
    /// Two 5 ms subframes.
    Ms10,
    /// Four 5 ms subframes.
    Ms20,
}

/// Internal SILK frame geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Internal sampling bandwidth.
    pub bandwidth: Bandwidth,
    /// Internal frame duration.
    pub duration: FrameDuration,
}

impl Frame {
    /// Number of meaningful samples in the internal frame.
    pub const fn samples(self) -> usize {
        let samples_10_ms = match self.bandwidth {
            Bandwidth::Narrow => 80,
            Bandwidth::Medium => 120,
            Bandwidth::Wide => 160,
        };
        samples_10_ms
            * match self.duration {
                FrameDuration::Ms10 => 1,
                FrameDuration::Ms20 => 2,
            }
    }

    /// Number of 16-sample excitation shell blocks, including MB padding.
    pub const fn shell_blocks(self) -> usize {
        self.samples().div_ceil(16)
    }

    /// Number of coded excitation samples, including the 10 ms MB tail.
    pub const fn coded_samples(self) -> usize {
        self.shell_blocks() * 16
    }
}

/// Duration of the enclosing Opus frame for SILK layer header parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDuration {
    /// A single 10 ms internal frame.
    Ms10,
    /// A single 20 ms internal frame.
    Ms20,
    /// Two 20 ms internal frames.
    Ms40,
    /// Three 20 ms internal frames.
    Ms60,
}

impl PacketDuration {
    const fn frames(self) -> usize {
        match self {
            Self::Ms10 | Self::Ms20 => 1,
            Self::Ms40 => 2,
            Self::Ms60 => 3,
        }
    }
}

/// Decoded VAD and per-frame low-bitrate redundancy flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    channels: u8,
    frames: u8,
    voice: [u8; 2],
    redundancy: [u8; 2],
}

impl Header {
    /// Decode RFC 6716 sections 4.2.3-4.2.4 without allocating.
    ///
    /// Channel zero is mid/mono; channel one is side. Header bits for both
    /// channels precede the optional per-frame redundancy symbols. Invalid
    /// channel counts leave the entropy decoder unchanged.
    pub fn decode(
        decoder: &mut Decoder<'_>,
        duration: PacketDuration,
        channels: u8,
    ) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Argument);
        }
        let frames = duration.frames();
        let mut header = Self {
            channels,
            frames: frames as u8,
            voice: [0; 2],
            redundancy: [0; 2],
        };
        for channel in 0..usize::from(channels) {
            for frame in 0..frames {
                header.voice[channel] |= u8::from(decoder.bit(1).unwrap()) << frame;
            }
            header.redundancy[channel] = u8::from(decoder.bit(1).unwrap());
        }
        for channel in 0..usize::from(channels) {
            if header.redundancy[channel] != 0 && frames > 1 {
                header.redundancy[channel] =
                    (decoder.symbol(if frames == 2 { LBRR_40 } else { LBRR_60 }) + 1) as u8;
            }
        }
        Ok(header)
    }

    /// Whether one internal channel/frame has speech activity.
    pub fn voice_activity(self, channel: usize, frame: usize) -> Option<bool> {
        self.flag(self.voice, channel, frame)
    }

    /// Whether one internal channel/frame has a preceding LBRR frame.
    pub fn redundancy(self, channel: usize, frame: usize) -> Option<bool> {
        self.flag(self.redundancy, channel, frame)
    }

    fn flag(self, flags: [u8; 2], channel: usize, frame: usize) -> Option<bool> {
        if channel >= usize::from(self.channels) || frame >= usize::from(self.frames) {
            return None;
        }
        Some(flags[channel] & (1 << frame) != 0)
    }
}

const LBRR_40: Model<'static> = Model::from_static(&[0, 53, 106, 256]);
const LBRR_60: Model<'static> = Model::from_static(&[0, 41, 61, 90, 131, 146, 174, 256]);

/// Signal class carried by one SILK frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    /// No voice activity.
    Inactive,
    /// Active signal without periodic long-term prediction.
    Unvoiced,
    /// Active signal with periodic long-term prediction.
    Voiced,
}

/// Excitation quantization offset class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationOffset {
    /// Low offset.
    Low,
    /// High offset.
    High,
}

/// Joint signal and quantization type of one internal SILK frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameType {
    /// Signal class.
    pub signal: SignalType,
    /// Quantization offset class.
    pub offset: QuantizationOffset,
}

impl FrameType {
    /// Decode the frame type with its externally supplied VAD/LBRR activity.
    pub fn decode(decoder: &mut Decoder<'_>, active: bool) -> Self {
        const INACTIVE: Model<'static> = Model::from_static(&[0, 26, 256]);
        const ACTIVE: Model<'static> = Model::from_static(&[0, 24, 98, 246, 256]);
        let index = if active {
            decoder.symbol(ACTIVE) + 2
        } else {
            decoder.symbol(INACTIVE)
        };
        Self {
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

    const fn sign_table(self) -> usize {
        let signal = match self.signal {
            SignalType::Inactive => 0,
            SignalType::Unvoiced => 1,
            SignalType::Voiced => 2,
        };
        signal * 2
            + match self.offset {
                QuantizationOffset::Low => 0,
                QuantizationOffset::High => 1,
            }
    }

    const fn quantization_offset(self) -> i32 {
        match (self.signal, self.offset) {
            (SignalType::Voiced, QuantizationOffset::Low) => 8,
            (SignalType::Voiced, QuantizationOffset::High)
            | (SignalType::Inactive | SignalType::Unvoiced, QuantizationOffset::Low) => 25,
            (SignalType::Inactive | SignalType::Unvoiced, QuantizationOffset::High) => 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_flags_match_direct_msb_header_extraction_for_all_geometries() {
        for duration in [
            PacketDuration::Ms10,
            PacketDuration::Ms20,
            PacketDuration::Ms40,
            PacketDuration::Ms60,
        ] {
            for channels in 1..=2 {
                for first in 0..=255 {
                    let mut packet = [0; 16];
                    packet[0] = first;
                    let header =
                        Header::decode(&mut Decoder::new(&packet), duration, channels).unwrap();
                    let frames = duration.frames();
                    for channel in 0..usize::from(channels) {
                        let mut redundancy_present = false;
                        for frame in 0..frames {
                            let shift = 7 - (channel * (frames + 1) + frame);
                            assert_eq!(
                                header.voice_activity(channel, frame),
                                Some(first & (1 << shift) != 0)
                            );
                            redundancy_present |= header.redundancy(channel, frame).unwrap();
                        }
                        let shift = 7 - (channel * (frames + 1) + frames);
                        assert_eq!(redundancy_present, first & (1 << shift) != 0);
                    }
                    assert_eq!(header.voice_activity(usize::from(channels), 0), None);
                    assert_eq!(header.redundancy(0, frames), None);
                }
            }
        }
    }

    #[test]
    fn invalid_header_channel_count_does_not_consume_bits() {
        for channels in [0, 3, 255] {
            let mut decoder = Decoder::new(&[255; 16]);
            let state = (decoder.range(), decoder.tell());
            assert_eq!(
                Header::decode(&mut decoder, PacketDuration::Ms60, channels),
                Err(Error::Argument)
            );
            assert_eq!((decoder.range(), decoder.tell()), state);
        }
    }

    #[test]
    fn frame_types_respect_vad_and_all_normative_offsets() {
        for byte in 0..=255 {
            let packet = [byte; 8];
            let inactive = FrameType::decode(&mut Decoder::new(&packet), false);
            assert_eq!(inactive.signal, SignalType::Inactive);
            let active = FrameType::decode(&mut Decoder::new(&packet), true);
            assert_ne!(active.signal, SignalType::Inactive);
        }
        for signal in [
            SignalType::Inactive,
            SignalType::Unvoiced,
            SignalType::Voiced,
        ] {
            for offset in [QuantizationOffset::Low, QuantizationOffset::High] {
                let kind = FrameType { signal, offset };
                let expected = match (signal, offset) {
                    (SignalType::Voiced, QuantizationOffset::Low) => 8,
                    (SignalType::Voiced, QuantizationOffset::High)
                    | (_, QuantizationOffset::Low) => 25,
                    _ => 60,
                };
                assert_eq!(kind.quantization_offset(), expected);
            }
        }
    }
}
