// SPDX-License-Identifier: MIT
//! Opus multistream channel layouts and concatenated packet framing.
//!
//! Each transport packet contains one self-delimiting packet per stream except
//! the last, which uses ordinary framing. All streams must have equal duration;
//! their internal channel counts and frame configurations may differ.

use super::packet::{Packet, PacketError};
use super::repacketize::Repacketizer;
use super::{DecodeError, Decoder, Recovery};
use std::fmt;
use std::iter::FusedIterator;

/// Invalid multistream channel layout or inconsistent stream packet framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Stream count is zero, coupled count exceeds it, or decoded channels exceed 255.
    Streams,
    /// Output channel count or a channel mapping index is invalid.
    Mapping,
    /// One encoded stream packet has invalid framing.
    Packet(PacketError),
    /// Encoded stream packets have different durations.
    Duration,
    /// The packet composition output is too small.
    Output,
    /// One constituent decoder rejected its contents or output arguments.
    Decode(DecodeError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Streams => formatter.write_str("invalid Opus multistream stream counts"),
            Self::Mapping => formatter.write_str("invalid Opus channel mapping"),
            Self::Packet(error) => error.fmt(formatter),
            Self::Duration => formatter.write_str("Opus multistream packet durations differ"),
            Self::Output => formatter.write_str("Opus multistream packet output is too small"),
            Self::Decode(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for Error {}
impl From<PacketError> for Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}
impl From<DecodeError> for Error {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

fn writer_error(error: super::repacketize::Error) -> Error {
    match error {
        super::repacketize::Error::Packet(error) => Error::Packet(error),
        super::repacketize::Error::Output => Error::Output,
        super::repacketize::Error::Incompatible
        | super::repacketize::Error::Duration
        | super::repacketize::Error::Range => Error::Duration,
    }
}

/// Validated mapping from decoded stream channels to up to 255 output channels.
///
/// Coupled streams are decoded to stereo first; remaining streams produce mono.
/// Mapping indices may repeat, omit decoded channels, or use 255 for silence.
/// Internal mono/stereo TOC flags do not constrain the configured output layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMapping {
    streams: u8,
    coupled: u8,
    channels: u8,
    mapping: [u8; 255],
}

impl ChannelMapping {
    /// Validate stream counts and one mapping index per output channel.
    pub fn new(streams: u8, coupled: u8, channels: &[u8]) -> Result<Self, Error> {
        let decoded = usize::from(streams) + usize::from(coupled);
        if streams == 0 || coupled > streams || decoded > 255 {
            return Err(Error::Streams);
        }
        if channels.is_empty()
            || channels.len() > 255
            || channels
                .iter()
                .any(|&index| index != 255 && usize::from(index) >= decoded)
        {
            return Err(Error::Mapping);
        }
        let mut mapping = [255; 255];
        mapping[..channels.len()].copy_from_slice(channels);
        Ok(Self {
            streams,
            coupled,
            channels: channels.len() as u8,
            mapping,
        })
    }

    /// Standard single-stream mono or stereo layout (mapping family 0).
    pub fn mono_stereo(channels: u8) -> Result<Self, Error> {
        match channels {
            1 => Self::new(1, 0, &[0]),
            2 => Self::new(1, 1, &[0, 1]),
            _ => Err(Error::Mapping),
        }
    }

    /// Number of encoded Opus streams in each transport packet.
    pub const fn streams(&self) -> u8 {
        self.streams
    }
    /// Number of initial streams configured for stereo output.
    pub const fn coupled_streams(&self) -> u8 {
        self.coupled
    }
    /// Total channels produced by the stream decoders before mapping.
    pub const fn decoded_channels(&self) -> usize {
        self.streams as usize + self.coupled as usize
    }
    /// Number of interleaved channels after mapping, including silence slots.
    pub const fn output_channels(&self) -> usize {
        self.channels as usize
    }
    /// One decoded channel index per output channel; 255 denotes pure silence.
    pub fn channels(&self) -> &[u8] {
        &self.mapping[..self.output_channels()]
    }
}

/// Completely validated multistream framing borrowing one transport packet.
#[derive(Debug, Clone, Copy)]
pub struct MultistreamPacket<'a> {
    bytes: &'a [u8],
    streams: u8,
    samples_48khz: usize,
}

impl<'a> MultistreamPacket<'a> {
    /// Compose ordinary stream packets into one multistream transport packet.
    ///
    /// All streams must have the same total duration, but their modes, channel
    /// counts and frame grouping may differ. Framing is compacted, including
    /// removal of input padding; every stream except the last is self-delimited.
    /// Input and capacity validation precede output changes. Processing does
    /// not allocate and leaves output beyond the returned byte count intact.
    pub fn write(streams: &[&[u8]], output: &mut [u8]) -> Result<usize, Error> {
        if streams.is_empty() || streams.len() > 255 {
            return Err(Error::Streams);
        }
        let mut builder = Repacketizer::new();
        let mut duration = None;
        let mut required = 0;
        for (index, &bytes) in streams.iter().enumerate() {
            let packet = Packet::parse(bytes)?;
            let samples = packet.samples(48_000)?;
            if duration.is_some_and(|previous| previous != samples) {
                return Err(Error::Duration);
            }
            duration = Some(samples);
            builder.clear();
            builder.push_packet(&packet).map_err(writer_error)?;
            let frames = 0..builder.frame_count();
            required += if index + 1 == streams.len() {
                builder.output_size(frames).map_err(writer_error)?
            } else {
                builder.self_delimited_size(frames).map_err(writer_error)?
            };
        }
        if output.len() < required {
            return Err(Error::Output);
        }
        let mut offset = 0;
        for (index, &bytes) in streams.iter().enumerate() {
            builder.clear();
            builder.push(bytes).map_err(writer_error)?;
            let frames = 0..builder.frame_count();
            offset += if index + 1 == streams.len() {
                builder.write(frames, &mut output[offset..required])
            } else {
                builder.write_self_delimited(frames, &mut output[offset..required])
            }
            .map_err(writer_error)?;
        }
        Ok(required)
    }

    /// Validate every stream boundary and equal decoded duration without allocation.
    pub fn parse(bytes: &'a [u8], streams: u8) -> Result<Self, Error> {
        if streams == 0 {
            return Err(Error::Streams);
        }
        let mut remaining = bytes;
        let mut duration = None;
        for stream in 0..streams {
            let packet = if stream + 1 == streams {
                Packet::parse(remaining)?
            } else {
                let (packet, consumed) = Packet::parse_self_delimited(remaining)?;
                remaining = &remaining[consumed..];
                packet
            };
            let samples = packet.samples(48_000)?;
            if duration.is_some_and(|previous| samples != previous) {
                return Err(Error::Duration);
            }
            duration = Some(samples);
        }
        Ok(Self {
            bytes,
            streams,
            samples_48khz: duration.expect("stream count is nonzero"),
        })
    }

    /// Original complete transport packet, including stream framing and padding.
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    /// Number of encoded streams validated in this packet.
    pub const fn streams(&self) -> u8 {
        self.streams
    }
    /// Decoded samples per channel at a standard Opus output rate.
    pub fn samples(&self, sample_rate: u32) -> Result<usize, PacketError> {
        match sample_rate {
            8000 | 12000 | 16000 | 24000 | 48000 => {
                Ok(self.samples_48khz * sample_rate as usize / 48000)
            }
            _ => Err(PacketError::UnsupportedSampleRate),
        }
    }
    /// Iterate over parsed streams in coded order, including uncoupled streams.
    pub fn packets(&self) -> Packets<'a> {
        Packets {
            remaining: self.bytes,
            streams: usize::from(self.streams),
        }
    }
}

/// Allocation-free iterator over already validated encoded stream packets.
#[derive(Debug, Clone)]
pub struct Packets<'a> {
    remaining: &'a [u8],
    streams: usize,
}

impl<'a> Iterator for Packets<'a> {
    type Item = Packet<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.streams == 0 {
            return None;
        }
        let packet = if self.streams == 1 {
            let packet =
                Packet::parse(self.remaining).expect("immutable packet was fully validated");
            self.remaining = &[];
            packet
        } else {
            let (packet, consumed) = Packet::parse_self_delimited(self.remaining)
                .expect("immutable packet was fully validated");
            self.remaining = &self.remaining[consumed..];
            packet
        };
        self.streams -= 1;
        Some(packet)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.streams, Some(self.streams))
    }
}
impl ExactSizeIterator for Packets<'_> {}
impl FusedIterator for Packets<'_> {}

/// Stateful multistream decoder with channel mapping and all standard Opus rates.
///
/// Construction prepares stream decoders, channel destinations and scratch PCM.
/// Decoding, recovery, concealment and reset perform no heap allocation. Entire
/// transport framing and output capacity are checked before changing history;
/// a codec-content error may advance earlier streams, requiring a reset.
pub struct MultistreamDecoder {
    mapping: ChannelMapping,
    sample_rate: u32,
    decoders: Vec<Decoder>,
    destinations: Vec<Vec<usize>>,
    scratch: Vec<f32>,
}

impl MultistreamDecoder {
    /// Prepare mapped output at 48 kHz.
    pub fn new(mapping: ChannelMapping) -> Result<Self, Error> {
        Self::with_sample_rate(mapping, 48_000)
    }

    /// Prepare mapped output at 8, 12, 16, 24 or 48 kHz.
    pub fn with_sample_rate(mapping: ChannelMapping, sample_rate: u32) -> Result<Self, Error> {
        let mut decoders = Vec::with_capacity(usize::from(mapping.streams()));
        for stream in 0..mapping.streams() {
            decoders.push(Decoder::with_sample_rate(
                if stream < mapping.coupled_streams() {
                    2
                } else {
                    1
                },
                sample_rate,
            )?);
        }
        let mut destinations = vec![Vec::new(); mapping.decoded_channels()];
        for (output, &decoded) in mapping.channels().iter().enumerate() {
            if decoded != 255 {
                destinations[usize::from(decoded)].push(output);
            }
        }
        let capacity = 2 * super::packet::MAX_PACKET_SAMPLES_48KHZ * sample_rate as usize / 48_000;
        Ok(Self {
            mapping,
            sample_rate,
            decoders,
            destinations,
            scratch: vec![0.0; capacity],
        })
    }

    /// Number of interleaved output channels, including any silent slots.
    pub const fn channels(&self) -> usize {
        self.mapping.output_channels()
    }
    /// Configured output sample rate in hertz.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Validated decoded-stream to output-channel layout.
    pub const fn mapping(&self) -> &ChannelMapping {
        &self.mapping
    }
    /// XOR of constituent final entropy ranges, matching multistream packet state.
    pub fn final_range(&self) -> u32 {
        self.decoders
            .iter()
            .fold(0, |range, decoder| range ^ decoder.final_range())
    }

    /// Decode one complete multistream transport packet and return samples/channel.
    /// All coded streams are processed, even when their output is not mapped.
    pub fn decode(&mut self, bytes: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        let packet = MultistreamPacket::parse(bytes, self.mapping.streams())?;
        let samples = packet.samples(self.sample_rate)?;
        self.check_output(samples, output)?;
        output[..samples * self.channels()].fill(0.0);
        for (stream, packet) in packet.packets().enumerate() {
            let coded_channels = self.decoders[stream].channels();
            let decoded = self.decoders[stream]
                .decode_parsed(&packet, &mut self.scratch[..samples * coded_channels])?;
            if decoded != samples {
                return Err(Error::Duration);
            }
            self.map_stream(stream, samples, output);
        }
        Ok(samples)
    }

    /// Recover one missing interval using the next multistream packet's in-band FEC.
    /// If any stream lacks redundancy, the aggregate result is `Concealed`.
    /// The next packet must subsequently be decoded normally for its own interval.
    pub fn recover(
        &mut self,
        bytes: &[u8],
        samples: usize,
        output: &mut [f32],
    ) -> Result<Recovery, Error> {
        let packet = MultistreamPacket::parse(bytes, self.mapping.streams())?;
        self.check_interval(samples, output)?;
        output[..samples * self.channels()].fill(0.0);
        let mut aggregate = Recovery::Fec;
        for (stream, packet) in packet.packets().enumerate() {
            let coded_channels = self.decoders[stream].channels();
            let recovery = self.decoders[stream].recover_parsed(
                &packet,
                samples,
                &mut self.scratch[..samples * coded_channels],
            )?;
            if recovery == Recovery::Concealed {
                aggregate = recovery;
            }
            self.map_stream(stream, samples, output);
        }
        Ok(aggregate)
    }

    /// Conceal a missing interval from each stream's history.
    /// Duration must be a positive multiple of 2.5 ms, no greater than 120 ms.
    pub fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, Error> {
        self.check_interval(samples, output)?;
        output[..samples * self.channels()].fill(0.0);
        for stream in 0..self.decoders.len() {
            let coded_channels = self.decoders[stream].channels();
            self.decoders[stream]
                .conceal(samples, &mut self.scratch[..samples * coded_channels])?;
            self.map_stream(stream, samples, output);
        }
        Ok(samples)
    }

    /// Clear all stream history and scratch PCM without changing the layout.
    pub fn reset(&mut self) {
        for decoder in &mut self.decoders {
            decoder.reset();
        }
        self.scratch.fill(0.0);
    }

    fn check_output(&self, samples: usize, output: &[f32]) -> Result<(), Error> {
        if output.len() < samples * self.channels() {
            return Err(DecodeError::Output.into());
        }
        Ok(())
    }

    fn check_interval(&self, samples: usize, output: &[f32]) -> Result<(), Error> {
        let quantum = self.sample_rate as usize / 400;
        if samples == 0 || samples > quantum * 48 || !samples.is_multiple_of(quantum) {
            return Err(DecodeError::Argument.into());
        }
        self.check_output(samples, output)
    }

    fn map_stream(&self, stream: usize, samples: usize, output: &mut [f32]) {
        let coupled = usize::from(self.mapping.coupled_streams());
        let coded_channels = self.decoders[stream].channels();
        let first_channel = if stream < coupled {
            stream * 2
        } else {
            stream + coupled
        };
        for coded in 0..coded_channels {
            for &destination in &self.destinations[first_channel + coded] {
                for (sample, frame) in output[..samples * self.channels()]
                    .chunks_exact_mut(self.channels())
                    .enumerate()
                {
                    frame[destination] = self.scratch[sample * coded_channels + coded];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_writes_independent_self_delimiters_and_preserves_frames() {
        let first = [0xf8, 1, 2, 3]; // One 20 ms CELT frame, three payload bytes.
        let second = [0x01, 4, 5, 6, 7]; // Two 10 ms SILK frames, two bytes each.
        let mut output = [0xa5; 20];
        let length = MultistreamPacket::write(&[&first, &second], &mut output).unwrap();
        assert_eq!(&output[..length], &[0xf8, 3, 1, 2, 3, 0x01, 4, 5, 6, 7]);
        assert!(output[length..].iter().all(|&byte| byte == 0xa5));
        let parsed = MultistreamPacket::parse(&output[..length], 2).unwrap();
        assert_eq!(parsed.samples(48_000).unwrap(), 960);
        let mut packets = parsed.packets();
        assert_eq!(
            packets.next().unwrap().frames().collect::<Vec<_>>(),
            [&first[1..]]
        );
        assert_eq!(
            packets.next().unwrap().frames().collect::<Vec<_>>(),
            [&second[1..3], &second[3..5]]
        );
    }

    #[test]
    fn composition_rejects_late_errors_before_output_changes_and_handles_maximum_streams() {
        let ordinary = [0xf8, 1, 2, 3];
        let shorter = [0xf0];
        let mut output = [0xa5; 16];
        assert_eq!(
            MultistreamPacket::write(&[], &mut output),
            Err(Error::Streams)
        );
        assert!(matches!(
            MultistreamPacket::write(&[&ordinary, &[]], &mut output),
            Err(Error::Packet(_))
        ));
        assert_eq!(
            MultistreamPacket::write(&[&ordinary, &shorter], &mut output),
            Err(Error::Duration)
        );
        assert_eq!(
            MultistreamPacket::write(&[&ordinary, &ordinary], &mut output[..8]),
            Err(Error::Output)
        );
        assert_eq!(output, [0xa5; 16]);
        let dtx = [0xe3, 48]; // 48 empty 2.5 ms frames, total 120 ms.
        let mut streams = [&dtx[..]; 256];
        let mut packed = [0xa5; 768];
        assert_eq!(
            MultistreamPacket::write(&streams, &mut packed),
            Err(Error::Streams)
        );
        let length = MultistreamPacket::write(&streams[..255], &mut packed).unwrap();
        assert_eq!(length, 254 * 3 + 2);
        let parsed = MultistreamPacket::parse(&packed[..length], 255).unwrap();
        assert_eq!(parsed.samples(48_000).unwrap(), 5760);
        assert!(
            parsed.packets().all(|packet| packet.frame_count() == 48
                && packet.frames().all(|frame| frame.is_empty()))
        );
        streams[254] = &[];
        let before = packed;
        assert!(MultistreamPacket::write(&streams[..255], &mut packed).is_err());
        assert_eq!(packed, before);
    }
    use crate::opus::repacketize::Repacketizer;

    fn transport(streams: &[&[u8]]) -> Vec<u8> {
        let mut result = Vec::new();
        for (index, &stream) in streams.iter().enumerate() {
            let mut builder = Repacketizer::new();
            builder.push(stream).unwrap();
            let size = if index + 1 == streams.len() {
                builder.output_size(0..builder.frame_count())
            } else {
                builder.self_delimited_size(0..builder.frame_count())
            }
            .unwrap();
            let offset = result.len();
            result.resize(offset + size, 0);
            if index + 1 == streams.len() {
                builder
                    .write(0..builder.frame_count(), &mut result[offset..])
                    .unwrap();
            } else {
                builder
                    .write_self_delimited(0..builder.frame_count(), &mut result[offset..])
                    .unwrap();
            }
        }
        result
    }

    #[test]
    fn layouts_cover_aliases_omitted_channels_and_silence() {
        let mapping = ChannelMapping::new(3, 1, &[0, 2, 1, 3, 255, 0]).unwrap();
        assert_eq!(mapping.decoded_channels(), 4);
        assert_eq!(mapping.output_channels(), 6);
        assert_eq!(mapping.channels(), [0, 2, 1, 3, 255, 0]);
        for (streams, coupled, channels) in [
            (0, 0, &[0][..]),
            (1, 2, &[0]),
            (255, 1, &[0]),
            (1, 0, &[]),
            (1, 0, &[1]),
        ] {
            assert!(ChannelMapping::new(streams, coupled, channels).is_err());
        }
        assert!(ChannelMapping::new(255, 0, &[254, 255]).is_ok());
        assert!(ChannelMapping::new(1, 0, &[0; 255]).is_ok());
        assert!(ChannelMapping::new(1, 0, &[0; 256]).is_err());
    }

    #[test]
    fn stream_boundaries_allow_different_tocs_with_equal_duration() {
        // One 20-ms SILK frame, one 20-ms stereo Hybrid frame, eight 2.5-ms CELT frames.
        let bytes = transport(&[&[9 << 3], &[(15 << 3) | 4], &[(16 << 3) | 3, 8]]);
        let packet = MultistreamPacket::parse(&bytes, 3).unwrap();
        assert_eq!(packet.streams(), 3);
        for rate in [8000, 12000, 16000, 24000, 48000] {
            assert_eq!(packet.samples(rate).unwrap(), rate as usize / 50);
        }
        let packets = packet.packets().collect::<Vec<_>>();
        assert_eq!(packets[0].toc().channels(), 1);
        assert_eq!(packets[1].toc().channels(), 2);
        assert_eq!(packets[2].frame_count(), 8);
        assert!(
            packets
                .iter()
                .all(|packet| packet.frames().all(|frame| frame.is_empty()))
        );
    }

    #[test]
    fn mismatched_durations_and_all_truncated_transport_prefixes_are_rejected() {
        assert!(matches!(
            MultistreamPacket::parse(&[], 0),
            Err(Error::Streams)
        ));
        let mismatch = transport(&[&[16 << 3], &[17 << 3]]);
        assert!(matches!(
            MultistreamPacket::parse(&mismatch, 2),
            Err(Error::Duration)
        ));
        let bytes = transport(&[&[128, 1, 2, 3], &[132, 4, 5], &[128, 6, 7, 8]]);
        // The final frame length is implicit: prefixes ending within it are
        // valid shorter frames; prefixes before its TOC must fail.
        let before_last = Packet::parse_self_delimited(&bytes).unwrap().1;
        let before_last = before_last
            + Packet::parse_self_delimited(&bytes[before_last..])
                .unwrap()
                .1;
        for length in 0..=before_last {
            assert!(MultistreamPacket::parse(&bytes[..length], 3).is_err());
        }
        assert!(MultistreamPacket::parse(&bytes, 3).is_ok());
    }

    #[test]
    fn mapped_dtx_loss_reset_and_refusal_preserve_output_contracts() {
        let mapping = ChannelMapping::new(3, 1, &[0, 2, 1, 3, 255, 0]).unwrap();
        let bytes = transport(&[&[31 << 3], &[(31 << 3) | 4], &[31 << 3]]);
        for rate in [8000, 12000, 16000, 24000, 48000] {
            let samples = rate as usize / 50;
            let mut decoder = MultistreamDecoder::with_sample_rate(mapping, rate).unwrap();
            let mut output = vec![99.0; samples * 6 + 7];
            assert_eq!(decoder.decode(&bytes, &mut output).unwrap(), samples);
            assert!(output[..samples * 6].iter().all(|value| value.is_finite()));
            assert!(output[samples * 6..].iter().all(|&value| value == 99.0));
            assert!(
                output[..samples * 6]
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .all(|frame| frame[4] == 0.0 && frame[0] == frame[5])
            );
            assert_eq!(decoder.conceal(samples, &mut output).unwrap(), samples);
            assert_eq!(
                decoder.recover(&bytes, samples, &mut output).unwrap(),
                Recovery::Concealed
            );
            let previous = decoder.final_range();
            let mut short = [99.0; 2];
            assert!(decoder.decode(&bytes, &mut short).is_err());
            assert_eq!(short, [99.0; 2]);
            assert_eq!(decoder.final_range(), previous);
            decoder.reset();
            assert_eq!(decoder.final_range(), 0);
            assert_eq!(decoder.decode(&bytes, &mut output).unwrap(), samples);
        }
    }
}
