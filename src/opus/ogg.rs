// SPDX-License-Identifier: MIT
//! Bounded streaming Ogg Opus files, independently implemented from
//! [RFC 7845](https://www.rfc-editor.org/rfc/rfc7845) and
//! [RFC 3533](https://www.rfc-editor.org/rfc/rfc3533).
//!
//! This module supports one logical stream and mapping families 0 (mono/stereo),
//! 1 (Vorbis surround order), 2/3 (ACN/SN3D Ambisonics) and 255 (unidentified
//! channels). [`OggChainReader`] handles sequential logical streams and
//! [`OggDemuxReader`] separates concurrent logical streams. Codec packets are
//! preserved without decoding them.
//! Container I/O allocates bounded buffers and belongs outside an audio callback.
//! Header gain is reported to the caller; [`OpusHead::gain_factor`] converts it.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::ops::Range;

use super::multistream::{ChannelMapping, MultistreamPacket};
#[cfg(test)]
use super::packet::Packet;
use super::packet::PacketError;
use super::projection::{ProjectionMapping, valid_channels};

/// Largest accepted metadata packet, including comment padding.
pub const MAX_PACKET_BYTES: usize = 1_048_576;
/// Largest accepted number of UTF-8 user comments.
pub const MAX_COMMENTS: usize = 1024;
/// Default audio packet resource bound, with page continuations when needed.
pub const MAX_WRITER_PACKET_BYTES: usize = 16 * 1_048_576;
const PAGE_PAYLOAD: usize = 65_025;
const HEADER_BYTES: usize = 27;
const NO_GRANULE: u64 = u64::MAX;

/// Explicit resource limit for assembled audio packets, including all padding.
///
/// The default 16 MiB accommodates 255 streams with 48 maximum-size frames
/// each. Opus padding is not format-bounded; callers may raise or lower this
/// application limit. Metadata remains separately bounded to 1 MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OggLimits {
    /// Maximum compressed audio packet bytes across all encoded streams.
    pub audio_packet_bytes: usize,
}

impl Default for OggLimits {
    fn default() -> Self {
        Self {
            audio_packet_bytes: MAX_WRITER_PACKET_BYTES,
        }
    }
}

impl OggLimits {
    fn validate(self) -> Result<(), Error> {
        if self.audio_packet_bytes == 0 {
            return Err(Error::Limit("zero audio packet limit"));
        }
        Ok(())
    }
}

/// An I/O error, invalid container, unsupported layout or exceeded resource bound.
#[derive(Debug)]
pub enum Error {
    /// The underlying reader or writer failed.
    Io(io::Error),
    /// EOF occurred within a page, packet or before the stream's EOS page.
    Truncated,
    /// A required header, flag or packet organization is invalid.
    Format(&'static str),
    /// An Ogg page checksum does not match its bytes.
    Checksum,
    /// Page sequence numbers are missing, repeated or out of order.
    Sequence,
    /// The file multiplexes or chains more than one logical stream.
    Serial,
    /// The identification header has an unsupported major version.
    Version(u8),
    /// The channel mapping family is unsupported.
    Mapping(u8),
    /// A mapped layout or one multistream audio packet is invalid.
    Multistream(super::multistream::Error),
    /// A packet, metadata count or timestamp exceeds the documented bound.
    Limit(&'static str),
    /// An audio packet has invalid ordinary Opus framing.
    Packet(PacketError),
    /// A granule position does not agree with decoded durations or trimming.
    Granule,
    /// The writer has failed or has already been finalized.
    Finished,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Truncated => formatter.write_str("truncated Ogg Opus stream"),
            Self::Format(reason) => write!(formatter, "invalid Ogg Opus stream: {reason}"),
            Self::Checksum => formatter.write_str("Ogg page checksum mismatch"),
            Self::Sequence => formatter.write_str("Ogg pages are missing or out of order"),
            Self::Serial => formatter.write_str("multiple Ogg logical streams are unsupported"),
            Self::Version(version) => write!(formatter, "unsupported OpusHead version {version}"),
            Self::Mapping(mapping) => {
                write!(formatter, "unsupported Opus channel mapping {mapping}")
            }
            Self::Multistream(error) => error.fmt(formatter),
            Self::Limit(reason) => write!(formatter, "Ogg resource bound exceeded: {reason}"),
            Self::Packet(error) => error.fmt(formatter),
            Self::Granule => formatter.write_str("invalid Ogg Opus granule position"),
            Self::Finished => formatter.write_str("Ogg writer is finished or failed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Packet(error) => Some(error),
            Self::Multistream(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            Self::Truncated
        } else {
            Self::Io(error)
        }
    }
}
impl From<PacketError> for Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}
impl From<super::multistream::Error> for Error {
    fn from(error: super::multistream::Error) -> Self {
        match error {
            super::multistream::Error::Packet(error) => Self::Packet(error),
            error => Self::Multistream(error),
        }
    }
}

/// Common identification metadata. All sample counts use 48 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpusHead {
    /// Output channels: one through 255, constrained by the selected mapping family.
    pub channels: u8,
    /// Initial decoded samples to discard, per channel at 48 kHz.
    pub pre_skip: u16,
    /// Original input rate as metadata; zero means unspecified.
    pub input_sample_rate: u32,
    /// Playback gain in signed Q7.8 decibels, applied in addition to user volume.
    pub output_gain_q8: i16,
}

impl OpusHead {
    fn validate(&self) -> Result<(), Error> {
        if !matches!(self.channels, 1 | 2) {
            return Err(Error::Format("channel count must be 1 or 2"));
        }
        Ok(())
    }

    /// Parse a strict 19-byte version-1, mapping-family-0 identification packet.
    pub fn parse(packet: &[u8]) -> Result<Self, Error> {
        if packet.len() < 19 || &packet[..8] != b"OpusHead" {
            return Err(Error::Format("missing OpusHead"));
        }
        if packet[8] != 1 {
            return Err(Error::Version(packet[8]));
        }
        if packet[18] != 0 {
            return Err(Error::Mapping(packet[18]));
        }
        if packet.len() != 19 {
            return Err(Error::Format("version-1 family-0 header length"));
        }
        let head = Self {
            channels: packet[9],
            pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
            input_sample_rate: u32::from_le_bytes(
                packet[12..16].try_into().expect("header length checked"),
            ),
            output_gain_q8: i16::from_le_bytes([packet[16], packet[17]]),
        };
        head.validate()?;
        Ok(head)
    }

    #[cfg(test)]
    fn bytes(&self) -> Result<[u8; 19], Error> {
        self.validate()?;
        Ok(self.base_bytes())
    }

    fn base_bytes(&self) -> [u8; 19] {
        let mut bytes = [0; 19];
        bytes[..8].copy_from_slice(b"OpusHead");
        bytes[8] = 1;
        bytes[9] = self.channels;
        bytes[10..12].copy_from_slice(&self.pre_skip.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.input_sample_rate.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.output_gain_q8.to_le_bytes());
        bytes
    }

    /// Linear playback multiplier corresponding to the header's Q7.8 dB gain.
    pub fn gain_factor(self) -> f32 {
        10.0f32.powf(f32::from(self.output_gain_q8) / 5120.0)
    }
}

/// Complete identification metadata for one Ogg Opus logical stream.
///
/// [`OpusHead`] retains the common fields and the simple family-0 constructor;
/// this type carries the stream counts and channel mapping needed for surround
/// and unidentified layouts. All pre-skip and granule counts remain at 48 kHz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusIdentification {
    /// Common identification fields, including the output channel count.
    pub head: OpusHead,
    /// Channel interpretation: 0 mono/stereo, 1 surround, 2/3 Ambisonics, 255 unidentified.
    pub mapping_family: u8,
    /// Decoded-stream to output-channel mapping.
    pub channel_mapping: ChannelMapping,
    /// Family-3 demixing matrix; its channel mapping is the coded identity layout.
    pub projection: Option<ProjectionMapping>,
}

impl OpusIdentification {
    /// Validate one supported family, its stream layout and common header fields.
    pub fn new(
        head: OpusHead,
        mapping_family: u8,
        channel_mapping: ChannelMapping,
    ) -> Result<Self, Error> {
        let identification = Self {
            head,
            mapping_family,
            channel_mapping,
            projection: None,
        };
        identification.validate()?;
        Ok(identification)
    }

    /// Construct standard single-stream mono/stereo identification metadata.
    pub fn mono_stereo(head: OpusHead) -> Result<Self, Error> {
        Self::new(head, 0, ChannelMapping::mono_stereo(head.channels)?)
    }

    /// Construct family-3 Ambisonics identification with a validated Q15 matrix.
    pub fn projected(head: OpusHead, projection: ProjectionMapping) -> Result<Self, Error> {
        let identification = Self {
            head,
            mapping_family: 3,
            channel_mapping: *projection.coded_layout(),
            projection: Some(projection),
        };
        identification.validate()?;
        Ok(identification)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.head.channels == 0 {
            return Err(Error::Format("zero output channels"));
        }
        if self.mapping_family == 3 {
            let projection = self
                .projection
                .as_ref()
                .ok_or(Error::Format("missing demixing matrix"))?;
            if usize::from(self.head.channels) != projection.channels()
                || self.channel_mapping != *projection.coded_layout()
            {
                return Err(Error::Format("projection header geometry"));
            }
            return Ok(());
        }
        if self.projection.is_some()
            || usize::from(self.head.channels) != self.channel_mapping.output_channels()
        {
            return Err(Error::Format("header channel count differs from mapping"));
        }
        match self.mapping_family {
            0 => {
                if self.channel_mapping != ChannelMapping::mono_stereo(self.head.channels)? {
                    return Err(Error::Format("family-0 channel layout"));
                }
            }
            1 if self.head.channels <= 8 => {}
            1 => return Err(Error::Format("family-1 channel count exceeds 8")),
            2 if valid_channels(self.head.channels) => {}
            2 => return Err(Error::Format("Ambisonics output channel count")),
            255 => {}
            family => return Err(Error::Mapping(family)),
        }
        Ok(())
    }

    /// Parse compatible-major OpusHead metadata, including explicit stream mapping.
    ///
    /// Minor versions 2 through 15 may append fields. Their known prefix is
    /// decoded and trailing extension data is accepted as required by RFC 7845.
    pub fn parse(packet: &[u8]) -> Result<Self, Error> {
        if packet.len() < 19 || &packet[..8] != b"OpusHead" {
            return Err(Error::Format("missing OpusHead"));
        }
        if packet[8] >= 16 {
            return Err(Error::Version(packet[8]));
        }
        let head = OpusHead {
            channels: packet[9],
            pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
            input_sample_rate: u32::from_le_bytes(
                packet[12..16].try_into().expect("header checked"),
            ),
            output_gain_q8: i16::from_le_bytes([packet[16], packet[17]]),
        };
        let family = packet[18];
        if family == 3 {
            if packet.len() < 21 {
                return Err(Error::Format("truncated projection stream counts"));
            }
            let coded = usize::from(packet[19]) + usize::from(packet[20]);
            let length = 21 + 2 * coded * usize::from(head.channels);
            if packet.len() < length || (packet[8] <= 1 && packet.len() != length) {
                return Err(Error::Format("projection identification length"));
            }
            let matrix = packet[21..length]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&bytes| i16::from_le_bytes(bytes))
                .collect();
            return Self::projected(
                head,
                ProjectionMapping::new(packet[19], packet[20], head.channels, matrix)?,
            );
        }
        let (layout, known_length) = if family == 0 {
            (ChannelMapping::mono_stereo(head.channels)?, 19)
        } else {
            if !matches!(family, 1 | 2 | 255) {
                return Err(Error::Mapping(family));
            }
            let length = 21 + usize::from(head.channels);
            if packet.len() < length {
                return Err(Error::Format("truncated channel mapping"));
            }
            (
                ChannelMapping::new(packet[19], packet[20], &packet[21..length])?,
                length,
            )
        };
        if packet[8] <= 1 && packet.len() != known_length {
            return Err(Error::Format("version-1 identification length"));
        }
        Self::new(head, family, layout)
    }

    /// Serialize the known metadata as a version-1 identification packet.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut bytes = self.head.base_bytes().to_vec();
        bytes[18] = self.mapping_family;
        if self.mapping_family != 0 {
            bytes.push(self.channel_mapping.streams());
            bytes.push(self.channel_mapping.coupled_streams());
            if let Some(projection) = &self.projection {
                for coefficient in projection.matrix() {
                    bytes.extend_from_slice(&coefficient.to_le_bytes());
                }
            } else {
                bytes.extend_from_slice(self.channel_mapping.channels());
            }
        }
        Ok(bytes)
    }
}

/// UTF-8 comment metadata, with at most [`MAX_COMMENTS`] comments and 1 MiB total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpusTags {
    /// Human-readable encoder/container vendor.
    pub vendor: String,
    /// User comments, conventionally in NAME=value form.
    pub comments: Vec<String>,
}

impl Default for OpusTags {
    fn default() -> Self {
        Self {
            vendor: "Snacka".to_owned(),
            comments: Vec::new(),
        }
    }
}

fn take_u32(bytes: &mut &[u8]) -> Result<u32, Error> {
    if bytes.len() < 4 {
        return Err(Error::Format("short OpusTags length"));
    }
    let value = u32::from_le_bytes(bytes[..4].try_into().expect("length checked"));
    *bytes = &bytes[4..];
    Ok(value)
}

fn take_text(bytes: &mut &[u8]) -> Result<String, Error> {
    let length = take_u32(bytes)? as usize;
    if length > bytes.len() {
        return Err(Error::Format("OpusTags string exceeds packet"));
    }
    let (text, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(std::str::from_utf8(text)
        .map_err(|_| Error::Format("OpusTags contains invalid UTF-8"))?
        .to_owned())
}

fn validate_gain_comments(comments: &[String]) -> Result<(), Error> {
    let mut seen = [false; 2];
    for comment in comments {
        let Some((name, value)) = comment.split_once('=') else {
            continue;
        };
        let kind = ["R128_TRACK_GAIN", "R128_ALBUM_GAIN"]
            .iter()
            .position(|known| name.eq_ignore_ascii_case(known));
        let Some(kind) = kind else {
            continue;
        };
        let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
        if seen[kind]
            || value.len() > 6
            || digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || value.parse::<i16>().is_err()
        {
            return Err(Error::Format("invalid or duplicate R128 gain comment"));
        }
        seen[kind] = true;
    }
    Ok(())
}

impl OpusTags {
    /// Parse bounded UTF-8 metadata; trailing binary padding is ignored.
    pub fn parse(packet: &[u8]) -> Result<Self, Error> {
        if packet.len() > MAX_PACKET_BYTES {
            return Err(Error::Limit("OpusTags packet"));
        }
        if !packet.starts_with(b"OpusTags") {
            return Err(Error::Format("missing OpusTags"));
        }
        let mut bytes = &packet[8..];
        let vendor = take_text(&mut bytes)?;
        let count = take_u32(&mut bytes)? as usize;
        if count > MAX_COMMENTS {
            return Err(Error::Limit("OpusTags comment count"));
        }
        if count > bytes.len() / 4 {
            return Err(Error::Format("OpusTags count exceeds packet"));
        }
        let mut comments = Vec::with_capacity(count);
        for _ in 0..count {
            comments.push(take_text(&mut bytes)?);
        }
        validate_gain_comments(&comments)?;
        Ok(Self { vendor, comments })
    }

    fn bytes(&self) -> Result<Vec<u8>, Error> {
        if self.comments.len() > MAX_COMMENTS {
            return Err(Error::Limit("OpusTags comment count"));
        }
        validate_gain_comments(&self.comments)?;
        let mut length = 16usize
            .checked_add(self.vendor.len())
            .ok_or(Error::Limit("OpusTags packet"))?;
        for comment in &self.comments {
            length = length
                .checked_add(4)
                .and_then(|n| n.checked_add(comment.len()))
                .ok_or(Error::Limit("OpusTags packet"))?;
        }
        if length > MAX_PACKET_BYTES {
            return Err(Error::Limit("OpusTags packet"));
        }
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(b"OpusTags");
        bytes.extend_from_slice(&(self.vendor.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.vendor.as_bytes());
        bytes.extend_from_slice(&(self.comments.len() as u32).to_le_bytes());
        for comment in &self.comments {
            bytes.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            bytes.extend_from_slice(comment.as_bytes());
        }
        Ok(bytes)
    }
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut value = 0;
    while value < 256 {
        let mut remainder = (value as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            remainder = if remainder & 0x8000_0000 != 0 {
                (remainder << 1) ^ 0x04c1_1db7
            } else {
                remainder << 1
            };
            bit += 1;
        }
        table[value] = remainder;
        value += 1;
    }
    table
}
const CRC: [u32; 256] = crc_table();
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |crc, &byte| {
        (crc << 8) ^ CRC[((crc >> 24) as u8 ^ byte) as usize]
    })
}

fn read_first_byte<R: Read>(source: &mut R) -> Result<Option<u8>, Error> {
    let mut byte = [0];
    loop {
        match source.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(byte[0])),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_physical_page<R: Read>(source: &mut R, bytes: &mut Vec<u8>) -> Result<bool, Error> {
    let Some(first) = read_first_byte(source)? else {
        return Ok(false);
    };
    bytes.resize(HEADER_BYTES, 0);
    bytes[0] = first;
    source.read_exact(&mut bytes[1..HEADER_BYTES])?;
    if &bytes[..4] != b"OggS" || bytes[4] != 0 {
        return Err(Error::Format("Ogg capture pattern or version"));
    }
    let expected_crc = u32::from_le_bytes(bytes[22..26].try_into().expect("fixed header"));
    let segments = usize::from(bytes[26]);
    bytes.resize(HEADER_BYTES + segments, 0);
    source.read_exact(&mut bytes[HEADER_BYTES..])?;
    let payload: usize = bytes[HEADER_BYTES..]
        .iter()
        .map(|&lace| usize::from(lace))
        .sum();
    bytes.resize(HEADER_BYTES + segments + payload, 0);
    source.read_exact(&mut bytes[HEADER_BYTES + segments..])?;
    bytes[22..26].fill(0);
    if checksum(bytes) != expected_crc {
        return Err(Error::Checksum);
    }
    Ok(true)
}

struct Page {
    packets: Vec<Vec<u8>>,
    flags: u8,
    granule: u64,
    incomplete: bool,
}

struct Demux<R> {
    source: R,
    serial: Option<u32>,
    sequence: u32,
    partial: Vec<u8>,
    page_bytes: Vec<u8>,
    ended: bool,
    require_eof: bool,
    packet_limit: usize,
    strict_opus: bool,
}

impl<R: Read> Demux<R> {
    fn new(source: R) -> Self {
        Self {
            source,
            serial: None,
            sequence: 0,
            partial: Vec::new(),
            page_bytes: Vec::new(),
            ended: false,
            require_eof: true,
            packet_limit: MAX_PACKET_BYTES,
            strict_opus: true,
        }
    }

    fn first_byte(&mut self) -> Result<Option<u8>, Error> {
        read_first_byte(&mut self.source)
    }

    fn page(&mut self) -> Result<Option<Page>, Error> {
        if self.ended {
            return Ok(None);
        }
        if !read_physical_page(&mut self.source, &mut self.page_bytes)? {
            return Ok(None);
        }
        self.assemble_page().map(Some)
    }

    fn assemble_page(&mut self) -> Result<Page, Error> {
        if self.ended {
            return Err(Error::Format("page after logical EOS"));
        }
        let flags = self.page_bytes[5];
        if flags & !7 != 0 {
            return Err(Error::Format("reserved page flags"));
        }
        let granule = u64::from_le_bytes(self.page_bytes[6..14].try_into().expect("fixed header"));
        if self.strict_opus && granule != NO_GRANULE && granule > i64::MAX as u64 {
            return Err(Error::Granule);
        }
        let serial = u32::from_le_bytes(self.page_bytes[14..18].try_into().expect("fixed header"));
        let sequence =
            u32::from_le_bytes(self.page_bytes[18..22].try_into().expect("fixed header"));
        let segments = usize::from(self.page_bytes[26]);
        if let Some(expected) = self.serial {
            if serial != expected {
                return Err(Error::Serial);
            }
            if flags & 2 != 0 {
                return Err(Error::Format("repeated beginning of stream"));
            }
        } else {
            if flags & 2 == 0 || flags & 1 != 0 || (self.strict_opus && flags != 2) {
                return Err(Error::Format("first page must be BOS"));
            }
            self.serial = Some(serial);
        }
        if sequence != self.sequence {
            return Err(Error::Sequence);
        }
        self.sequence = self.sequence.wrapping_add(1);
        if segments > 0 && (flags & 1 != 0) != !self.partial.is_empty() {
            return Err(Error::Format("orphan or missing continuation flag"));
        }
        if segments == 0 && flags & 1 != 0 {
            return Err(Error::Format("empty continued page"));
        }
        let mut packets = Vec::new();
        let mut offset = HEADER_BYTES + segments;
        for &lace in &self.page_bytes[HEADER_BYTES..HEADER_BYTES + segments] {
            let length = usize::from(lace);
            if self
                .partial
                .len()
                .checked_add(length)
                .is_none_or(|size| size > self.packet_limit)
            {
                return Err(Error::Limit("assembled packet"));
            }
            self.partial
                .extend_from_slice(&self.page_bytes[offset..offset + length]);
            offset += length;
            if lace < 255 {
                packets.push(std::mem::take(&mut self.partial));
            }
        }
        if self.strict_opus && packets.is_empty() && granule != NO_GRANULE {
            return Err(Error::Granule);
        }
        if self.strict_opus && !packets.is_empty() && granule == NO_GRANULE {
            return Err(Error::Granule);
        }
        if flags & 4 != 0 {
            if (self.strict_opus && packets.is_empty()) || !self.partial.is_empty() {
                return Err(Error::Format("incomplete EOS page"));
            }
            if self.require_eof && self.first_byte()?.is_some() {
                return Err(Error::Format("data after EOS"));
            }
            self.ended = true;
        }
        Ok(Page {
            packets,
            flags,
            granule,
            incomplete: !self.partial.is_empty(),
        })
    }
}

/// One complete compressed packet and the samples retained after skip/trimming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OggPacket {
    /// Ordinary Opus packet bytes, suitable for the owned codec decoder.
    pub data: Vec<u8>,
    /// Complete decoded duration, per channel at 48 kHz.
    pub samples_48khz: usize,
    /// End timestamp in the container's 48 kHz granule timeline.
    pub granule_position: u64,
    /// Per-channel sample indexes to retain from this decoded packet at 48 kHz.
    /// Decode the entire packet to preserve codec history, then discard outside
    /// this range. Multiply indexes by channels for interleaved PCM.
    pub keep: Range<usize>,
    /// This is the last complete packet of the logical stream.
    pub end_of_stream: bool,
}

/// Bounded sequential reader for one Ogg Opus logical stream.
///
/// Completed packets from one page are buffered so timestamps and final trims
/// are validated before any packet from that page is returned. EOF before EOS,
/// chained streams, orphan continuations and corrupt checksums return errors.
pub struct OggReader<R> {
    demux: Demux<R>,
    identification: OpusIdentification,
    tags: OpusTags,
    pending: VecDeque<OggPacket>,
    decoded: u64,
    origin: Option<u64>,
    previous_granule: Option<u64>,
    done: bool,
}

impl<R: Read> OggReader<R> {
    /// Read and validate the two mandatory headers before returning a reader.
    pub fn new(source: R) -> Result<Self, Error> {
        Self::with_limits(source, OggLimits::default())
    }

    /// Read mandatory headers with an explicit assembled audio packet bound.
    pub fn with_limits(source: R, limits: OggLimits) -> Result<Self, Error> {
        Self::with_eof_policy(source, true, limits)
    }

    fn with_eof_policy(source: R, require_eof: bool, limits: OggLimits) -> Result<Self, Error> {
        limits.validate()?;
        let mut demux = Demux::new(source);
        demux.require_eof = require_eof;
        let mut first = demux.page()?.ok_or(Error::Truncated)?;
        if first.flags != 2 || first.granule != 0 || first.incomplete || first.packets.len() != 1 {
            return Err(Error::Format("OpusHead must complete alone on BOS page"));
        }
        let identification = OpusIdentification::parse(&first.packets.remove(0))?;
        let tags = loop {
            let mut page = demux.page()?.ok_or(Error::Truncated)?;
            if page.flags & 4 != 0 {
                return Err(Error::Format("EOS before audio"));
            }
            if page.packets.is_empty() {
                continue;
            }
            if page.packets.len() != 1 || page.incomplete || page.granule != 0 {
                return Err(Error::Format("OpusTags must finish its page"));
            }
            break OpusTags::parse(&page.packets.remove(0))?;
        };
        demux.packet_limit = limits.audio_packet_bytes;
        Ok(Self {
            demux,
            identification,
            tags,
            pending: VecDeque::new(),
            decoded: 0,
            origin: None,
            previous_granule: None,
            done: false,
        })
    }

    /// Identification metadata, including channel count, initial skip and gain.
    pub fn head(&self) -> &OpusHead {
        &self.identification.head
    }
    /// Complete header metadata, including mapped stream counts and interpretation.
    pub fn identification(&self) -> &OpusIdentification {
        &self.identification
    }
    /// Channel mapping, or coded identity layout for family-3 demixing.
    pub fn mapping(&self) -> &ChannelMapping {
        &self.identification.channel_mapping
    }
    /// Parsed UTF-8 vendor and user comments.
    pub fn tags(&self) -> &OpusTags {
        &self.tags
    }
    /// Consume the reader and return its underlying input.
    pub fn into_inner(self) -> R {
        self.demux.source
    }

    fn audio_page(&mut self, page: Page) -> Result<(), Error> {
        if page.packets.is_empty() {
            return Ok(());
        }
        let mut durations = Vec::with_capacity(page.packets.len());
        let mut duration = 0u64;
        for packet in &page.packets {
            if packet.is_empty() {
                return Err(Error::Format("empty audio packet"));
            }
            let samples =
                MultistreamPacket::parse(packet, self.mapping().streams())?.samples(48_000)?;
            durations.push(samples);
            duration = duration.checked_add(samples as u64).ok_or(Error::Granule)?;
        }
        let decoded_end = self
            .decoded
            .checked_add(duration)
            .filter(|&n| n <= i64::MAX as u64)
            .ok_or(Error::Granule)?;
        let eos = page.flags & 4 != 0;
        let origin = match self.origin {
            Some(origin) => origin,
            None if page.granule >= decoded_end => page.granule - decoded_end,
            None if eos && page.granule >= u64::from(self.head().pre_skip) => 0,
            None => return Err(Error::Granule),
        };
        let expected = origin
            .checked_add(decoded_end)
            .filter(|&n| n <= i64::MAX as u64)
            .ok_or(Error::Granule)?;
        if (!eos && page.granule != expected)
            || (eos
                && (page.granule > expected
                    || page.granule < self.previous_granule.unwrap_or(origin)
                    || page.granule < origin + u64::from(self.head().pre_skip)))
        {
            return Err(Error::Granule);
        }
        let stop = if eos {
            page.granule - origin
        } else {
            decoded_end
        };
        let count = page.packets.len();
        let mut begin = self.decoded;
        for (index, (data, samples)) in page.packets.into_iter().zip(durations).enumerate() {
            let end = begin + samples as u64;
            let kept_start = u64::from(self.head().pre_skip)
                .saturating_sub(begin)
                .min(samples as u64) as usize;
            let kept_end = stop.saturating_sub(begin).min(samples as u64) as usize;
            self.pending.push_back(OggPacket {
                data,
                samples_48khz: samples,
                granule_position: (origin + end).min(page.granule),
                keep: kept_start.min(kept_end)..kept_end,
                end_of_stream: eos && index + 1 == count,
            });
            begin = end;
        }
        self.origin = Some(origin);
        self.decoded = decoded_end;
        self.previous_granule = Some(page.granule);
        self.done = eos;
        Ok(())
    }

    /// Return the next audio packet, or `None` only after a complete EOS page.
    pub fn next_packet(&mut self) -> Result<Option<OggPacket>, Error> {
        loop {
            if let Some(packet) = self.pending.pop_front() {
                return Ok(Some(packet));
            }
            if self.done {
                return Ok(None);
            }
            let page = self.demux.page()?.ok_or(Error::Truncated)?;
            self.audio_page(page)?;
        }
    }
}

/// Identification and comments for one logical stream within a physical chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusStream {
    /// Identification fields and channel mapping or projection matrix.
    pub identification: OpusIdentification,
    /// Parsed vendor and comments for this logical stream.
    pub tags: OpusTags,
}

enum StreamDecoder {
    Mapped(Box<super::multistream::MultistreamDecoder>),
    Projected(Box<super::projection::ProjectionDecoder>),
}

/// Owned decoder for an Ogg identification header, including matrix demixing.
///
/// Codec work after construction allocates nothing. Apply header gain and the
/// packet's kept sample range separately. A new logical stream needs a new
/// decoder because its mapping, projection and independent histories may differ.
pub struct OggDecoder {
    decoder: StreamDecoder,
}

impl OggDecoder {
    /// Prepare the header's output layout at 48 kHz.
    pub fn new(identification: &OpusIdentification) -> Result<Self, Error> {
        Self::with_sample_rate(identification, 48_000)
    }

    /// Prepare ordinary mapping or Ambisonics demixing at a standard Opus rate.
    pub fn with_sample_rate(
        identification: &OpusIdentification,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        identification.validate()?;
        let decoder = if let Some(projection) = &identification.projection {
            StreamDecoder::Projected(Box::new(
                super::projection::ProjectionDecoder::with_sample_rate(
                    projection.clone(),
                    sample_rate,
                )?,
            ))
        } else {
            StreamDecoder::Mapped(Box::new(
                super::multistream::MultistreamDecoder::with_sample_rate(
                    identification.channel_mapping,
                    sample_rate,
                )?,
            ))
        };
        Ok(Self { decoder })
    }

    /// Number of interleaved channels after mapping or projection.
    pub fn channels(&self) -> usize {
        match &self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.channels(),
            StreamDecoder::Projected(decoder) => decoder.channels(),
        }
    }
    /// Configured sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        match &self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.sample_rate(),
            StreamDecoder::Projected(decoder) => decoder.sample_rate(),
        }
    }
    /// XOR of every constituent stream's final entropy range.
    pub fn final_range(&self) -> u32 {
        match &self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.final_range(),
            StreamDecoder::Projected(decoder) => decoder.final_range(),
        }
    }
    /// Decode one complete compressed packet before applying container trimming.
    pub fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        Ok(match &mut self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.decode(packet, output)?,
            StreamDecoder::Projected(decoder) => decoder.decode(packet, output)?,
        })
    }
    /// Recover a missing interval; decode this packet again for its current audio.
    pub fn recover(
        &mut self,
        packet: &[u8],
        samples: usize,
        output: &mut [f32],
    ) -> Result<super::Recovery, Error> {
        Ok(match &mut self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.recover(packet, samples, output)?,
            StreamDecoder::Projected(decoder) => decoder.recover(packet, samples, output)?,
        })
    }
    /// Conceal a missing interval in the header's mapped output layout.
    pub fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, Error> {
        Ok(match &mut self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.conceal(samples, output)?,
            StreamDecoder::Projected(decoder) => decoder.conceal(samples, output)?,
        })
    }
    /// Clear codec histories while retaining the mapping or matrix.
    pub fn reset(&mut self) {
        match &mut self.decoder {
            StreamDecoder::Mapped(decoder) => decoder.reset(),
            StreamDecoder::Projected(decoder) => decoder.reset(),
        }
    }
}

/// A logical-stream boundary or audio packet in sequential Ogg playback order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OggChainEvent {
    /// New stream metadata. Reset or recreate the decoder before its audio.
    Stream(Box<OpusStream>),
    /// Audio in the latest announced stream, with its own skip and end trim.
    Packet(OggPacket),
}

/// Reader for a physical Ogg file containing sequential Opus logical streams.
///
/// Each stream independently validates its headers, CRC, sequence, granules,
/// pre-skip and EOS. Boundaries are explicit because channels, tags and gain may
/// change. Concurrent multiplexing remains an error. The chain is bounded to
/// 1,024 logical streams, with unique serial numbers within the physical file.
pub struct OggChainReader<R> {
    reader: Option<OggReader<BufReader<R>>>,
    announce: bool,
    done: bool,
    serials: Vec<u32>,
    limits: OggLimits,
}

impl<R: Read> OggChainReader<R> {
    /// Validate the first stream's mandatory headers and prepare ordered events.
    pub fn new(source: R) -> Result<Self, Error> {
        Self::with_limits(source, OggLimits::default())
    }

    /// Read chained streams with one explicit audio packet bound for each stream.
    pub fn with_limits(source: R, limits: OggLimits) -> Result<Self, Error> {
        let reader = OggReader::with_eof_policy(BufReader::new(source), false, limits)?;
        let serial = reader.demux.serial.expect("headers established serial");
        Ok(Self {
            reader: Some(reader),
            announce: true,
            done: false,
            serials: vec![serial],
            limits,
        })
    }

    /// Read one metadata boundary or trimmed compressed packet; `None` means clean EOF.
    pub fn next_event(&mut self) -> Result<Option<OggChainEvent>, Error> {
        loop {
            if self.done {
                return Ok(None);
            }
            let reader = self.reader.as_mut().ok_or(Error::Finished)?;
            if self.announce {
                self.announce = false;
                return Ok(Some(OggChainEvent::Stream(Box::new(OpusStream {
                    identification: reader.identification().clone(),
                    tags: reader.tags().clone(),
                }))));
            }
            if let Some(packet) = reader.next_packet()? {
                return Ok(Some(OggChainEvent::Packet(packet)));
            }
            let mut source = self.reader.take().expect("reader checked").into_inner();
            if source.fill_buf()?.is_empty() {
                self.done = true;
                return Ok(None);
            }
            if self.serials.len() == 1024 {
                return Err(Error::Limit("logical stream count"));
            }
            let reader = OggReader::with_eof_policy(source, false, self.limits)?;
            let serial = reader.demux.serial.expect("headers established serial");
            if self.serials.contains(&serial) {
                return Err(Error::Serial);
            }
            self.serials.push(serial);
            self.reader = Some(reader);
            self.announce = true;
        }
    }
}

/// One routed logical-stream header, trimmed Opus packet, or other-codec page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OggDemuxEvent {
    /// An Opus stream's validated mandatory headers, before its audio packets.
    Stream {
        /// Logical stream serial, unique within the physical file.
        serial: u32,
        /// Channel layout, gain, skip and comments for this independent stream.
        stream: Box<OpusStream>,
    },
    /// One complete Opus packet with this serial's independent trim/timeline.
    Packet {
        /// Logical stream serial identifying its decoder and metadata.
        serial: u32,
        /// Compressed audio with validated duration, skip and end trim.
        packet: OggPacket,
    },
    /// An assembled page in a non-Opus logical stream, preserved without interpretation.
    Opaque {
        /// Logical stream serial for the caller's other-codec demuxer.
        serial: u32,
        /// Raw granule field; `u64::MAX` denotes no completed packet.
        granule_position: u64,
        /// Complete packets on this page; partial packets remain internally buffered.
        packets: Vec<Vec<u8>>,
        /// This page begins its logical stream.
        begin_of_stream: bool,
        /// This page ends its logical stream.
        end_of_stream: bool,
    },
}

struct MuxHeaders {
    demux: Demux<io::Empty>,
    identification: Option<OpusIdentification>,
}

enum MuxPhase {
    Headers(Box<MuxHeaders>),
    Audio(Box<OggReader<io::Empty>>),
    Other(Box<Demux<io::Empty>>),
    Ended,
}

impl MuxPhase {
    fn demux(&mut self) -> Result<&mut Demux<io::Empty>, Error> {
        Ok(match self {
            Self::Headers(headers) => &mut headers.demux,
            Self::Audio(reader) => &mut reader.demux,
            Self::Other(demux) => demux,
            Self::Ended => return Err(Error::Format("page after logical EOS")),
        })
    }
    fn ended(&self) -> bool {
        match self {
            Self::Headers(_) => false,
            Self::Audio(reader) => reader.done,
            Self::Other(demux) => demux.ended,
            Self::Ended => true,
        }
    }
}

/// Page demultiplexer for concurrent or chained Ogg logical streams.
///
/// Every Opus serial retains independent headers, sequence, continuations,
/// duration, granules and trim. Non-Opus packets are routed as opaque events,
/// allowing a caller to handle video or other codecs without discarding data.
/// EOF requires EOS for every serial. A physical file is bounded to 1,024
/// unique serials; each assembled audio/opaque packet uses [`OggLimits`].
/// This I/O layer allocates, and must remain outside an audio callback.
pub struct OggDemuxReader<R> {
    source: R,
    page_bytes: Vec<u8>,
    streams: BTreeMap<u32, Option<MuxPhase>>,
    pending: VecDeque<OggDemuxEvent>,
    limits: OggLimits,
    done: bool,
    failed: bool,
}

impl<R: Read> OggDemuxReader<R> {
    /// Prepare lazy demultiplexing with default packet resource limits.
    pub fn new(source: R) -> Self {
        Self {
            source,
            page_bytes: Vec::new(),
            streams: BTreeMap::new(),
            pending: VecDeque::new(),
            limits: OggLimits::default(),
            done: false,
            failed: false,
        }
    }

    /// Prepare lazy demultiplexing with an explicit audio and opaque packet limit.
    pub fn with_limits(source: R, limits: OggLimits) -> Result<Self, Error> {
        limits.validate()?;
        Ok(Self {
            limits,
            ..Self::new(source)
        })
    }

    /// Return the next routed event; `None` means clean physical-file EOF.
    /// A damaged stream fails the demuxer and subsequent calls return `Finished`.
    pub fn next_event(&mut self) -> Result<Option<OggDemuxEvent>, Error> {
        if self.failed {
            return Err(Error::Finished);
        }
        let result = self.read_event();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_event(&mut self) -> Result<Option<OggDemuxEvent>, Error> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            if self.done {
                return Ok(None);
            }
            if !read_physical_page(&mut self.source, &mut self.page_bytes)? {
                if self.streams.is_empty()
                    || self
                        .streams
                        .values()
                        .any(|phase| phase.as_ref().is_none_or(|phase| !phase.ended()))
                {
                    return Err(Error::Truncated);
                }
                self.done = true;
                return Ok(None);
            }
            let serial =
                u32::from_le_bytes(self.page_bytes[14..18].try_into().expect("fixed header"));
            if !self.streams.contains_key(&serial) {
                if self.streams.len() == 1024 {
                    return Err(Error::Limit("logical stream count"));
                }
                let mut demux = Demux::new(io::empty());
                demux.require_eof = false;
                demux.strict_opus = false;
                self.streams.insert(
                    serial,
                    Some(MuxPhase::Headers(Box::new(MuxHeaders {
                        demux,
                        identification: None,
                    }))),
                );
            }
            let slot = self.streams.get_mut(&serial).expect("serial inserted");
            let mut phase = slot.take().expect("only completed event reads continue");
            std::mem::swap(&mut self.page_bytes, &mut phase.demux()?.page_bytes);
            let mut page = phase.demux()?.assemble_page()?;
            let phase = match phase {
                MuxPhase::Headers(mut headers) => {
                    if headers.identification.is_none() {
                        let opus = page
                            .packets
                            .first()
                            .is_some_and(|packet| packet.starts_with(b"OpusHead"))
                            || headers.demux.partial.starts_with(b"OpusHead");
                        if opus {
                            if page.flags != 2
                                || page.granule != 0
                                || page.incomplete
                                || page.packets.len() != 1
                            {
                                return Err(Error::Format(
                                    "OpusHead must complete alone on BOS page",
                                ));
                            }
                            headers.identification =
                                Some(OpusIdentification::parse(&page.packets.remove(0))?);
                            headers.demux.strict_opus = true;
                            MuxPhase::Headers(headers)
                        } else {
                            headers.demux.packet_limit = self.limits.audio_packet_bytes;
                            if headers.demux.partial.len() > self.limits.audio_packet_bytes
                                || page
                                    .packets
                                    .iter()
                                    .any(|packet| packet.len() > self.limits.audio_packet_bytes)
                            {
                                return Err(Error::Limit("opaque packet"));
                            }
                            self.pending.push_back(opaque_event(serial, page));
                            if headers.demux.ended {
                                MuxPhase::Ended
                            } else {
                                MuxPhase::Other(Box::new(headers.demux))
                            }
                        }
                    } else {
                        if page.flags & 4 != 0 {
                            return Err(Error::Format("EOS before audio"));
                        }
                        if page.packets.is_empty() {
                            MuxPhase::Headers(headers)
                        } else {
                            if page.packets.len() != 1 || page.incomplete || page.granule != 0 {
                                return Err(Error::Format("OpusTags must finish its page"));
                            }
                            let tags = OpusTags::parse(&page.packets.remove(0))?;
                            let identification = headers
                                .identification
                                .take()
                                .expect("identification checked");
                            headers.demux.packet_limit = self.limits.audio_packet_bytes;
                            self.pending.push_back(OggDemuxEvent::Stream {
                                serial,
                                stream: Box::new(OpusStream {
                                    identification: identification.clone(),
                                    tags: tags.clone(),
                                }),
                            });
                            MuxPhase::Audio(Box::new(OggReader {
                                demux: headers.demux,
                                identification,
                                tags,
                                pending: VecDeque::new(),
                                decoded: 0,
                                origin: None,
                                previous_granule: None,
                                done: false,
                            }))
                        }
                    }
                }
                MuxPhase::Audio(mut reader) => {
                    reader.audio_page(page)?;
                    while let Some(packet) = reader.pending.pop_front() {
                        self.pending
                            .push_back(OggDemuxEvent::Packet { serial, packet });
                    }
                    if reader.done {
                        MuxPhase::Ended
                    } else {
                        MuxPhase::Audio(reader)
                    }
                }
                MuxPhase::Other(demux) => {
                    self.pending.push_back(opaque_event(serial, page));
                    if demux.ended {
                        MuxPhase::Ended
                    } else {
                        MuxPhase::Other(demux)
                    }
                }
                MuxPhase::Ended => return Err(Error::Format("page after logical EOS")),
            };
            *slot = Some(phase);
        }
    }
}

fn opaque_event(serial: u32, page: Page) -> OggDemuxEvent {
    OggDemuxEvent::Opaque {
        serial,
        granule_position: page.granule,
        packets: page.packets,
        begin_of_stream: page.flags & 2 != 0,
        end_of_stream: page.flags & 4 != 0,
    }
}

struct PendingPacket {
    data: Vec<u8>,
    begin: u64,
    end: u64,
}

/// Ogg Opus writer that buffers its last audio packet until final trimming.
///
/// Audio packets and tags can span pages. Audio defaults to 16 MiB (configurable)
/// and metadata to 1 MiB. `finish` takes the desired sample count
/// after pre-skip; feed the encoder sufficient padding/drain to cover it.
pub struct OggWriter<W> {
    sink: W,
    serial: u32,
    sequence: u32,
    head: OpusHead,
    mapping: ChannelMapping,
    pending: Option<PendingPacket>,
    decoded: u64,
    failed: bool,
    limits: OggLimits,
}

impl<W: Write> OggWriter<W> {
    /// Write identification and comment headers for a new logical stream.
    /// The caller chooses its stream serial number.
    pub fn new(sink: W, serial: u32, head: OpusHead, tags: &OpusTags) -> Result<Self, Error> {
        Self::new_mapped(sink, serial, OpusIdentification::mono_stereo(head)?, tags)
    }

    /// Write headers for any supported ordinary or Ambisonics channel layout.
    pub fn new_mapped(
        sink: W,
        serial: u32,
        identification: OpusIdentification,
        tags: &OpusTags,
    ) -> Result<Self, Error> {
        Self::new_mapped_with_limits(sink, serial, identification, tags, OggLimits::default())
    }

    /// Write mapped headers with an explicit maximum audio packet size.
    pub fn new_mapped_with_limits(
        sink: W,
        serial: u32,
        identification: OpusIdentification,
        tags: &OpusTags,
        limits: OggLimits,
    ) -> Result<Self, Error> {
        limits.validate()?;
        let header = identification.to_bytes()?;
        let comments = tags.bytes()?;
        let mut writer = Self {
            sink,
            serial,
            sequence: 0,
            head: identification.head,
            mapping: identification.channel_mapping,
            pending: None,
            decoded: 0,
            failed: false,
            limits,
        };
        writer.packet_pages(&header, 2, 0)?;
        writer.packet_pages(&comments, 0, 0)?;
        Ok(writer)
    }

    fn page(&mut self, data: &[u8], laces: &[u8], flags: u8, granule: u64) -> Result<(), Error> {
        let mut page = Vec::with_capacity(HEADER_BYTES + laces.len() + data.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(laces.len() as u8);
        page.extend_from_slice(laces);
        page.extend_from_slice(data);
        let crc = checksum(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.failed = true;
        self.sink.write_all(&page)?;
        self.failed = false;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }

    fn packet_pages(&mut self, data: &[u8], first_flags: u8, granule: u64) -> Result<(), Error> {
        let mut offset = 0;
        loop {
            let remaining = data.len() - offset;
            let payload = remaining.min(PAGE_PAYLOAD);
            let complete = remaining < PAGE_PAYLOAD;
            let mut laces = [255u8; 255];
            let segments = if complete { payload / 255 + 1 } else { 255 };
            if complete {
                laces[segments - 1] = (payload % 255) as u8;
            }
            // BOS belongs only to the initial page; EOS belongs only to the
            // page that finishes the packet, including a zero terminal lace.
            let flags = if offset == 0 { first_flags & !4 } else { 1 }
                | if complete { first_flags & 4 } else { 0 };
            self.page(
                &data[offset..offset + payload],
                &laces[..segments],
                flags,
                if complete { granule } else { NO_GRANULE },
            )?;
            offset += payload;
            if complete {
                return Ok(());
            }
        }
    }

    /// Preserve one ordinary Opus packet and infer its duration from its TOC.
    /// Invalid packets and resource violations fail before writing prior data.
    pub fn write_packet(&mut self, packet: &[u8]) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Finished);
        }
        if packet.len() > self.limits.audio_packet_bytes {
            return Err(Error::Limit("audio packet"));
        }
        let samples =
            MultistreamPacket::parse(packet, self.mapping.streams())?.samples(48_000)? as u64;
        let end = self
            .decoded
            .checked_add(samples)
            .filter(|&n| n <= i64::MAX as u64)
            .ok_or(Error::Granule)?;
        if let Some(previous) = self.pending.take() {
            self.packet_pages(&previous.data, 0, previous.end)?;
        }
        self.pending = Some(PendingPacket {
            data: packet.to_owned(),
            begin: self.decoded,
            end,
        });
        self.decoded = end;
        Ok(())
    }

    /// Emit EOS with exactly `valid_samples` retained per channel at 48 kHz.
    /// The requested endpoint must be within the buffered final packet, after
    /// the header's pre-skip. No empty audio packet is synthesized.
    pub fn finish(mut self, valid_samples: u64) -> Result<W, Error> {
        if self.failed {
            return Err(Error::Finished);
        }
        let pending = self
            .pending
            .take()
            .ok_or(Error::Format("no audio packets"))?;
        let end = u64::from(self.head.pre_skip)
            .checked_add(valid_samples)
            .ok_or(Error::Granule)?;
        if end < pending.begin || end > pending.end {
            return Err(Error::Granule);
        }
        self.packet_pages(&pending.data, 4, end)?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn head(skip: u16) -> OpusHead {
        OpusHead {
            channels: 2,
            pre_skip: skip,
            input_sample_rate: 48_000,
            output_gain_q8: 0,
        }
    }

    // Polynomial long division independently verifies the production lookup.
    fn polynomial_crc(bytes: &[u8]) -> u32 {
        let mut remainder = 0u64;
        for &byte in bytes {
            remainder ^= u64::from(byte) << 24;
            for _ in 0..8 {
                remainder <<= 1;
                if remainder & (1 << 32) != 0 {
                    remainder ^= 0x1_04c1_1db7;
                }
            }
        }
        remainder as u32
    }

    fn page(sequence: u32, flags: u8, granule: u64, laces: &[u8], data: &[u8]) -> Vec<u8> {
        assert_eq!(
            laces.iter().map(|&value| usize::from(value)).sum::<usize>(),
            data.len()
        );
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OggS");
        bytes.extend_from_slice(&[0, flags]);
        bytes.extend_from_slice(&granule.to_le_bytes());
        bytes.extend_from_slice(&123u32.to_le_bytes());
        bytes.extend_from_slice(&sequence.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.push(laces.len() as u8);
        bytes.extend_from_slice(laces);
        bytes.extend_from_slice(data);
        let crc = polynomial_crc(&bytes);
        bytes[22..26].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn headers(skip: u16) -> Vec<u8> {
        let mut bytes = page(0, 2, 0, &[19], &head(skip).bytes().unwrap());
        let tags = OpusTags::default().bytes().unwrap();
        bytes.extend(page(1, 0, 0, &[tags.len() as u8], &tags));
        bytes
    }

    fn padded_packet(length: usize) -> Vec<u8> {
        assert!(length >= 4);
        let mut packet = vec![0x83, 0x41]; // CELT 2.5 ms, one padded CBR frame.
        let mut overhead = length - 3;
        while overhead > 255 {
            packet.push(255);
            overhead -= 255;
        }
        packet.push((overhead - 1) as u8);
        packet.push(0);
        packet.resize(length, 0);
        assert_eq!(Packet::parse(&packet).unwrap().samples(48_000), Ok(120));
        packet
    }

    #[test]
    fn crc_polynomial_and_literal_header_bytes() {
        assert_eq!(checksum(b"123456789"), 0x89a1_897f);
        let mut writer = OggWriter::new(Vec::new(), 123, head(10), &OpusTags::default()).unwrap();
        writer.write_packet(&[0x80, 0]).unwrap();
        let bytes = writer.finish(75).unwrap();
        assert_eq!(
            &bytes[..22],
            &[
                79, 103, 103, 83, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 123, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        assert_eq!(&bytes[26..28], &[1, 19]);
        assert_eq!(&bytes[28..47], &head(10).bytes().unwrap());
        let mut position = 0;
        while position < bytes.len() {
            let segments = usize::from(bytes[position + 26]);
            let payload: usize = bytes[position + 27..position + 27 + segments]
                .iter()
                .map(|&n| usize::from(n))
                .sum();
            let end = position + 27 + segments + payload;
            let mut page = bytes[position..end].to_owned();
            let expected = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].fill(0);
            assert_eq!(polynomial_crc(&page), expected);
            position = end;
        }
    }

    #[test]
    fn writer_reader_skip_trim_variable_durations_and_metadata() {
        let tags = OpusTags {
            vendor: "Snacka / 自有".to_owned(),
            comments: vec!["TITLE=rå jävla bra".to_owned()],
        };
        let mut writer = OggWriter::new(Vec::new(), 17, head(130), &tags).unwrap();
        writer.write_packet(&[0x80, 0]).unwrap(); // 120 samples.
        writer.write_packet(&[0x88, 0]).unwrap(); // 240 samples.
        writer.write_packet(&[0x90, 0]).unwrap(); // 480 samples.
        let bytes = writer.finish(601).unwrap(); // Granule731, lastframe trim109.
        let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.head(), &head(130));
        assert_eq!(reader.tags(), &tags);
        let first = reader.next_packet().unwrap().unwrap();
        assert_eq!(first.keep, 120..120);
        assert!(!first.end_of_stream);
        let second = reader.next_packet().unwrap().unwrap();
        assert_eq!(second.keep, 10..240);
        let last = reader.next_packet().unwrap().unwrap();
        assert_eq!(last.keep, 0..371);
        assert_eq!(last.granule_position, 731);
        assert!(last.end_of_stream);
        assert!(reader.next_packet().unwrap().is_none());
    }

    #[test]
    fn lacing_255_boundary_and_continuations_across_pages() {
        for length in [254, 255, 256, 510, 65_024] {
            let packet = padded_packet(length);
            let mut writer =
                OggWriter::new(Vec::new(), 123, head(0), &OpusTags::default()).unwrap();
            writer.write_packet(&packet).unwrap();
            let bytes = writer.finish(120).unwrap();
            let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
            assert_eq!(reader.next_packet().unwrap().unwrap().data, packet);
        }
        let packet = padded_packet(65_325);
        let mut bytes = headers(0);
        bytes.extend(page(2, 0, NO_GRANULE, &[255; 255], &packet[..65_025]));
        bytes.extend(page(3, 5, 120, &[255, 45], &packet[65_025..]));
        let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
        let result = reader.next_packet().unwrap().unwrap();
        assert_eq!(result.data, packet);
        assert_eq!(result.keep, 0..120);
        assert!(result.end_of_stream);
    }

    #[test]
    fn tags_can_span_pages_and_empty_audio_can_have_no_kept_samples() {
        let tags = OpusTags {
            vendor: "x".repeat(70_000),
            comments: Vec::new(),
        };
        let mut writer = OggWriter::new(Vec::new(), 77, head(120), &tags).unwrap();
        writer.write_packet(&[0x80, 0]).unwrap();
        let mut reader = OggReader::new(Cursor::new(writer.finish(0).unwrap())).unwrap();
        assert_eq!(reader.tags(), &tags);
        let packet = reader.next_packet().unwrap().unwrap();
        assert_eq!(packet.keep, 120..120);
        assert!(packet.end_of_stream);
    }

    #[test]
    fn multi_packet_pages_initial_offsets_and_eos_trims() {
        let mut bytes = headers(10);
        bytes.extend(page(2, 0, 1240, &[2, 2], &[0x80, 0, 0x80, 0]));
        bytes.extend(page(3, 4, 1300, &[2], &[0x80, 0]));
        let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
        let first = reader.next_packet().unwrap().unwrap();
        assert_eq!(first.keep, 10..120);
        assert_eq!(first.granule_position, 1120);
        let second = reader.next_packet().unwrap().unwrap();
        assert_eq!(second.keep, 0..120);
        assert_eq!(second.granule_position, 1240);
        let last = reader.next_packet().unwrap().unwrap();
        assert_eq!(last.keep, 0..60);
        assert_eq!(last.granule_position, 1300);
        let mut bytes = headers(10);
        bytes.extend(page(2, 4, 180, &[2, 2], &[0x80, 0, 0x80, 0]));
        let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.next_packet().unwrap().unwrap().keep, 10..120);
        assert_eq!(reader.next_packet().unwrap().unwrap().keep, 0..60);
    }

    fn stream_error(bytes: &[u8]) -> Error {
        match OggReader::new(Cursor::new(bytes)) {
            Err(error) => error,
            Ok(mut reader) => loop {
                match reader.next_packet() {
                    Err(error) => break error,
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("invalid stream accepted"),
                }
            },
        }
    }

    #[test]
    fn malformed_pages_reject_crc_sequence_serial_flags_and_granules() {
        let mut normal = headers(0);
        normal.extend(page(2, 4, 120, &[2], &[0x80, 0]));
        let mut crc = normal.clone();
        *crc.last_mut().unwrap() ^= 1;
        assert!(matches!(stream_error(&crc), Error::Checksum));
        for (seq, flags, gp) in [
            (3, 4, 120),
            (2, 5, 120),
            (2, 2, 120),
            (2, 4, NO_GRANULE - 1),
            (2, 0, 119),
            (2, 4, NO_GRANULE),
        ] {
            let mut bytes = headers(0);
            bytes.extend(page(seq, flags, gp, &[2], &[0x80, 0]));
            assert!(matches!(
                stream_error(&bytes),
                Error::Sequence | Error::Format(_) | Error::Granule
            ));
        }
        let mut bytes = headers(0);
        let mut wrong = page(2, 4, 120, &[2], &[0x80, 0]);
        wrong[14] = 124;
        wrong[22..26].fill(0);
        let crc = polynomial_crc(&wrong);
        wrong[22..26].copy_from_slice(&crc.to_le_bytes());
        bytes.extend(wrong);
        assert!(matches!(stream_error(&bytes), Error::Serial));
        let mut trailing = normal.clone();
        trailing.push(0);
        assert!(matches!(
            stream_error(&trailing),
            Error::Format("data after EOS")
        ));
        for end in 0..normal.len() {
            assert!(matches!(stream_error(&normal[..end]), Error::Truncated));
        }
        let mut bytes = headers(0);
        bytes.extend(page(2, 0, NO_GRANULE, &[255], &vec![0; 255]));
        bytes.extend(page(3, 4, 120, &[2], &[0x80, 0]));
        assert!(matches!(stream_error(&bytes), Error::Format(_)));
    }

    #[test]
    fn metadata_lengths_utf8_versions_and_resource_bounds_are_checked() {
        assert_eq!(OpusHead::parse(&head(0).bytes().unwrap()).unwrap(), head(0));
        let mut bytes = head(0).bytes().unwrap();
        bytes[8] = 16;
        assert!(matches!(OpusHead::parse(&bytes), Err(Error::Version(16))));
        bytes[8] = 1;
        bytes[18] = 1;
        assert!(matches!(OpusHead::parse(&bytes), Err(Error::Mapping(1))));
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(OpusTags::parse(&tags).is_err());
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&1u32.to_le_bytes());
        tags.push(255);
        tags.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(OpusTags::parse(&tags), Err(Error::Format(_))));
        let tags = OpusTags {
            vendor: String::new(),
            comments: vec![String::new(); MAX_COMMENTS + 1],
        };
        assert!(matches!(tags.bytes(), Err(Error::Limit(_))));
        let mut writer = OggWriter::new(Vec::new(), 0, head(0), &OpusTags::default()).unwrap();
        assert!(matches!(
            writer.write_packet(&vec![0; MAX_WRITER_PACKET_BYTES + 1]),
            Err(Error::Limit(_))
        ));
        writer.write_packet(&[0x80, 0]).unwrap();
        assert!(matches!(writer.finish(121), Err(Error::Granule)));
    }

    struct TinyRead<'a>(&'a [u8]);
    impl Read for TinyRead<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if output.is_empty() || self.0.is_empty() {
                return Ok(0);
            }
            output[0] = self.0[0];
            self.0 = &self.0[1..];
            Ok(1)
        }
    }

    #[test]
    fn short_underlying_reads_are_not_treated_as_eof() {
        let mut bytes = headers(0);
        bytes.extend(page(2, 4, 120, &[2], &[0x80, 0]));
        let mut reader = OggReader::new(TinyRead(&bytes)).unwrap();
        assert_eq!(reader.next_packet().unwrap().unwrap().keep, 0..120);
        assert!(reader.next_packet().unwrap().is_none());
    }

    #[test]
    fn gain_comments_preserve_signed_q8_values_and_reject_invalid_tags() {
        let tags = OpusTags {
            vendor: String::new(),
            comments: vec![
                "R128_TRACK_GAIN=-32768".to_owned(),
                "r128_album_gain=+32767".to_owned(),
            ],
        };
        assert_eq!(OpusTags::parse(&tags.bytes().unwrap()).unwrap(), tags);
        for comments in [
            vec!["R128_TRACK_GAIN=32768"],
            vec!["R128_TRACK_GAIN= 12"],
            vec!["R128_TRACK_GAIN=0000000"],
            vec!["R128_TRACK_GAIN=+"],
            vec!["R128_TRACK_GAIN=1", "r128_track_gain=2"],
        ] {
            let tags = OpusTags {
                vendor: String::new(),
                comments: comments.into_iter().map(str::to_owned).collect(),
            };
            assert!(matches!(tags.bytes(), Err(Error::Format(_))));
        }
        let mut identification = head(0);
        identification.output_gain_q8 = 20 * 256;
        assert!((identification.gain_factor() - 10.0).abs() < 1e-5);
    }

    #[test]
    fn mapped_headers_validate_layouts_and_compatible_minor_extensions() {
        for channels in 1..=8 {
            let layout =
                ChannelMapping::new(channels, 0, &(0..channels).collect::<Vec<_>>()).unwrap();
            let mut common = head(312);
            common.channels = channels;
            for family in [1, 255] {
                let identification = OpusIdentification::new(common, family, layout).unwrap();
                let bytes = identification.to_bytes().unwrap();
                assert_eq!(bytes.len(), 21 + usize::from(channels));
                assert_eq!(OpusIdentification::parse(&bytes).unwrap(), identification);
                for end in 0..bytes.len() {
                    assert!(OpusIdentification::parse(&bytes[..end]).is_err());
                }
                let mut extended = bytes.clone();
                extended.push(0xaa);
                assert!(OpusIdentification::parse(&extended).is_err());
                for version in 2..16 {
                    extended[8] = version;
                    assert_eq!(
                        OpusIdentification::parse(&extended).unwrap(),
                        identification
                    );
                }
                extended[8] = 16;
                assert!(matches!(
                    OpusIdentification::parse(&extended),
                    Err(Error::Version(16))
                ));
            }
        }
        let mut common = head(0);
        common.channels = 9;
        let layout = ChannelMapping::new(9, 0, &[0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert!(OpusIdentification::new(common, 1, layout).is_err());
        assert!(OpusIdentification::new(common, 255, layout).is_ok());
        assert!(OpusIdentification::new(common, 0, layout).is_err());
        common.channels = 2;
        let layout = ChannelMapping::new(1, 1, &[0, 1]).unwrap();
        assert!(matches!(
            OpusIdentification::new(common, 254, layout),
            Err(Error::Mapping(254))
        ));
        let mut bytes = OpusIdentification::new(common, 255, layout)
            .unwrap()
            .to_bytes()
            .unwrap();
        for (index, value) in [(9, 0), (19, 0), (20, 2), (21, 2)] {
            let original = bytes[index];
            bytes[index] = value;
            assert!(OpusIdentification::parse(&bytes).is_err());
            bytes[index] = original;
        }
    }

    #[test]
    fn multistream_container_preserves_layout_durations_and_trim() {
        use super::super::repacketize::Repacketizer;
        let layout = ChannelMapping::new(3, 1, &[0, 2, 1, 3, 255, 0]).unwrap();
        let mut common = head(150);
        common.channels = 6;
        let identification = OpusIdentification::new(common, 1, layout).unwrap();
        let mut writer = OggWriter::new_mapped(
            Vec::new(),
            123,
            identification.clone(),
            &OpusTags::default(),
        )
        .unwrap();
        let mut data = vec![0; 32];
        let mut offset = 0;
        for stream in 0..3 {
            let mut builder = Repacketizer::new();
            builder.push(&[0x90]).unwrap(); // Three mono 10 ms DTX streams.
            offset += if stream < 2 {
                builder
                    .write_self_delimited(0..1, &mut data[offset..])
                    .unwrap()
            } else {
                builder.write(0..1, &mut data[offset..]).unwrap()
            };
        }
        data.truncate(offset);
        for _ in 0..3 {
            writer.write_packet(&data).unwrap();
        }
        let bytes = writer.finish(1_150).unwrap();
        let mut reader = OggReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.identification(), &identification);
        assert_eq!(reader.mapping(), &layout);
        assert_eq!(reader.head(), &common);
        let mut kept = 0;
        let mut packets = 0;
        while let Some(packet) = reader.next_packet().unwrap() {
            assert_eq!(packet.data, data);
            assert_eq!(packet.samples_48khz, 480);
            kept += packet.keep.len();
            packets += 1;
        }
        assert_eq!((packets, kept), (3, 1_150));
    }

    #[test]
    fn writer_continues_large_audio_and_places_eos_only_on_completion() {
        for length in [65_025, 65_026, 130_050, MAX_WRITER_PACKET_BYTES] {
            let packet = padded_packet(length);
            let mut writer =
                OggWriter::new(Vec::new(), 123, head(0), &OpusTags::default()).unwrap();
            writer.write_packet(&packet).unwrap();
            let bytes = writer.finish(77).unwrap();
            let mut reader = OggReader::new(Cursor::new(&bytes)).unwrap();
            let decoded = reader.next_packet().unwrap().unwrap();
            assert_eq!(decoded.data, packet);
            assert_eq!(decoded.keep, 0..77);
            assert!(decoded.end_of_stream);
            assert!(reader.next_packet().unwrap().is_none());
            let mut position = 0;
            let mut audio_pages = 0;
            while position < bytes.len() {
                let segments = usize::from(bytes[position + 26]);
                let payload: usize = bytes[position + 27..position + 27 + segments]
                    .iter()
                    .map(|&n| usize::from(n))
                    .sum();
                let end = position + 27 + segments + payload;
                if audio_pages > 0 || bytes[position + 18..position + 22] == 2u32.to_le_bytes() {
                    let complete = bytes[position + 26 + segments] < 255;
                    assert_eq!(bytes[position + 5] & 4 != 0, complete);
                    assert_eq!(
                        u64::from_le_bytes(bytes[position + 6..position + 14].try_into().unwrap()),
                        if complete { 77 } else { NO_GRANULE }
                    );
                    audio_pages += 1;
                }
                position = end;
            }
            assert!(audio_pages >= 2);
        }
    }

    #[test]
    fn ambisonics_headers_cover_all_geometries_signed_matrices_and_bad_lengths() {
        for channels in 1..=255 {
            let mut common = head(0);
            common.channels = channels;
            let mut indices = vec![255; usize::from(channels)];
            indices[0] = 0;
            let layout = ChannelMapping::new(1, 0, &indices).unwrap();
            let identification = OpusIdentification::new(common, 2, layout);
            assert_eq!(identification.is_ok(), valid_channels(channels));
            if let Ok(identification) = identification {
                assert_eq!(
                    OpusIdentification::parse(&identification.to_bytes().unwrap()).unwrap(),
                    identification
                );
            }
        }
        let mut common = head(312);
        common.channels = 4;
        let matrix = vec![16384, -16384, 0, 32767, 8192, 16384, 0, -32768];
        let projection = ProjectionMapping::new(1, 1, 4, matrix.clone()).unwrap();
        let identification = OpusIdentification::projected(common, projection).unwrap();
        let bytes = identification.to_bytes().unwrap();
        assert_eq!(bytes.len(), 21 + matrix.len() * 2);
        assert_eq!(&bytes[18..25], &[3, 1, 1, 0, 64, 0, 192]);
        assert_eq!(OpusIdentification::parse(&bytes).unwrap(), identification);
        for end in 0..bytes.len() {
            assert!(OpusIdentification::parse(&bytes[..end]).is_err());
        }
        let mut invalid = bytes.clone();
        invalid.push(0);
        assert!(OpusIdentification::parse(&invalid).is_err());
        invalid[8] = 2;
        assert_eq!(OpusIdentification::parse(&invalid).unwrap(), identification);
        for (index, value) in [(9, 2), (19, 0), (20, 2)] {
            let mut invalid = bytes.clone();
            invalid[index] = value;
            assert!(OpusIdentification::parse(&invalid).is_err());
        }
        let mut writer =
            OggWriter::new_mapped(Vec::new(), 77, identification.clone(), &OpusTags::default())
                .unwrap();
        writer.write_packet(&[0x94]).unwrap(); // Stereo 10 ms DTX, one coupled stream.
        let mut reader = OggReader::new(Cursor::new(writer.finish(88).unwrap())).unwrap();
        assert_eq!(reader.identification(), &identification);
        let packet = reader.next_packet().unwrap().unwrap();
        assert_eq!(packet.keep, 312..400);
        let mut decoder = OggDecoder::new(reader.identification()).unwrap();
        assert_eq!(decoder.channels(), 4);
        let mut output = vec![99.0; 480 * 4];
        assert_eq!(decoder.decode(&packet.data, &mut output).unwrap(), 480);
        assert!(output.iter().all(|&value| value == 0.0));
    }

    fn one_stream(serial: u32, channels: u8, skip: u16, kept: u64) -> Vec<u8> {
        let mut common = head(skip);
        common.channels = channels;
        let mut writer = OggWriter::new(Vec::new(), serial, common, &OpusTags::default()).unwrap();
        writer.write_packet(&[0x90]).unwrap();
        writer.finish(kept).unwrap()
    }

    #[test]
    fn chained_streams_expose_independent_headers_skip_trim_and_reset_boundaries() {
        let first = one_stream(17, 1, 120, 300);
        let second = one_stream(18, 2, 312, 100);
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second);
        assert!(
            OggReader::new(Cursor::new(&bytes))
                .unwrap()
                .next_packet()
                .is_err()
        );
        let mut reader = OggChainReader::new(TinyRead(&bytes)).unwrap();
        let mut counts = Vec::new();
        let mut kept = Vec::new();
        while let Some(event) = reader.next_event().unwrap() {
            match event {
                OggChainEvent::Stream(stream) => counts.push(stream.identification.head.channels),
                OggChainEvent::Packet(packet) => {
                    assert!(packet.end_of_stream);
                    kept.push(packet.keep.len());
                }
            }
        }
        assert_eq!(counts, [1, 2]);
        assert_eq!(kept, [300, 100]);
        assert!(reader.next_event().unwrap().is_none());
        let mut repeated = first.clone();
        repeated.extend(one_stream(17, 1, 0, 480));
        let mut reader = OggChainReader::new(Cursor::new(repeated)).unwrap();
        assert!(matches!(
            reader.next_event().unwrap(),
            Some(OggChainEvent::Stream(_))
        ));
        assert!(matches!(
            reader.next_event().unwrap(),
            Some(OggChainEvent::Packet(_))
        ));
        assert!(matches!(reader.next_event(), Err(Error::Serial)));
        for extra in [vec![0], second[..second.len() - 1].to_owned()] {
            let mut invalid = first.clone();
            invalid.extend(extra);
            let mut reader = OggChainReader::new(Cursor::new(invalid)).unwrap();
            let rejected = loop {
                match reader.next_event() {
                    Err(_) => break true,
                    Ok(None) => break false,
                    Ok(Some(_)) => {}
                }
            };
            assert!(rejected, "truncated second chain was accepted");
        }
    }

    #[test]
    fn audio_limits_are_explicit_and_default_covers_255_maximum_streams() {
        use super::super::repacketize::Repacketizer;
        let mut frame = vec![0; super::super::packet::MAX_FRAME_BYTES + 1];
        frame[0] = 0x80;
        let mut builder = Repacketizer::new();
        for _ in 0..48 {
            builder.push(&frame).unwrap();
        }
        let mut self_delimited = vec![0; builder.self_delimited_size(0..48).unwrap()];
        builder
            .write_self_delimited(0..48, &mut self_delimited)
            .unwrap();
        let mut ordinary = vec![0; builder.output_size(0..48).unwrap()];
        builder.write(0..48, &mut ordinary).unwrap();
        let mut packet = Vec::with_capacity(254 * self_delimited.len() + ordinary.len());
        for _ in 0..254 {
            packet.extend_from_slice(&self_delimited);
        }
        packet.extend_from_slice(&ordinary);
        assert_eq!(packet.len(), 15_607_018);
        assert!(packet.len() < OggLimits::default().audio_packet_bytes);
        let mapping = ChannelMapping::new(255, 0, &(0..255).collect::<Vec<_>>()).unwrap();
        let mut common = head(0);
        common.channels = 255;
        let identification = OpusIdentification::new(common, 255, mapping).unwrap();
        let mut writer =
            OggWriter::new_mapped(Vec::new(), 77, identification, &OpusTags::default()).unwrap();
        writer.write_packet(&packet).unwrap();
        let bytes = writer.finish(5_760).unwrap();
        let mut reader = OggReader::new(Cursor::new(&bytes)).unwrap();
        let result = reader.next_packet().unwrap().unwrap();
        assert_eq!(result.data, packet);
        assert_eq!(result.keep, 0..5_760);
        let mut limited = OggReader::with_limits(
            Cursor::new(&bytes),
            OggLimits {
                audio_packet_bytes: packet.len() - 1,
            },
        )
        .unwrap();
        assert!(matches!(limited.next_packet(), Err(Error::Limit(_))));
        assert!(
            OggReader::with_limits(
                Cursor::new(&bytes),
                OggLimits {
                    audio_packet_bytes: 0
                }
            )
            .is_err()
        );
        let identification = OpusIdentification::mono_stereo(head(0)).unwrap();
        let mut writer = OggWriter::new_mapped_with_limits(
            Vec::new(),
            78,
            identification,
            &OpusTags::default(),
            OggLimits {
                audio_packet_bytes: 1,
            },
        )
        .unwrap();
        assert!(matches!(
            writer.write_packet(&[0x80, 0]),
            Err(Error::Limit(_))
        ));
        writer.write_packet(&[0x80]).unwrap();
        let bytes = writer.finish(120).unwrap();
        let mut reader = OggReader::with_limits(
            Cursor::new(bytes),
            OggLimits {
                audio_packet_bytes: 1,
            },
        )
        .unwrap();
        assert_eq!(reader.next_packet().unwrap().unwrap().data, [0x80]);
    }

    fn physical_pages(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut pages = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let segments = usize::from(bytes[offset + 26]);
            let payload: usize = bytes[offset + 27..offset + 27 + segments]
                .iter()
                .map(|&lace| usize::from(lace))
                .sum();
            let end = offset + HEADER_BYTES + segments + payload;
            pages.push(bytes[offset..end].to_owned());
            offset = end;
        }
        pages
    }

    fn fix_page_crc(page: &mut [u8]) {
        page[22..26].fill(0);
        let crc = polynomial_crc(page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
    }

    fn multiplex_fixture() -> (Vec<Vec<u8>>, Vec<u8>) {
        let packet = padded_packet(65_326);
        let mut common = head(0);
        common.channels = 1;
        let mut writer = OggWriter::new(Vec::new(), 17, common, &OpusTags::default()).unwrap();
        writer.write_packet(&packet).unwrap();
        let a = physical_pages(&writer.finish(77).unwrap());
        let b = physical_pages(&one_stream(18, 2, 312, 100));
        let other_header = page(0, 2, 0, &[12], b"video-header");
        let other_eos = page(1, 4, 1234, &[5], b"frame");
        (
            vec![
                a[0].clone(),
                b[0].clone(),
                other_header,
                a[1].clone(),
                b[1].clone(),
                a[2].clone(),
                other_eos,
                b[2].clone(),
                a[3].clone(),
            ],
            packet,
        )
    }

    #[test]
    fn multiplexed_pages_keep_per_serial_continuations_metadata_and_trim() {
        let (pages, expected) = multiplex_fixture();
        let bytes = pages.concat();
        let mut reader = OggDemuxReader::new(TinyRead(&bytes));
        let mut metadata = Vec::new();
        let mut audio = Vec::new();
        let mut opaque = Vec::new();
        while let Some(event) = reader.next_event().unwrap() {
            match event {
                OggDemuxEvent::Stream { serial, stream } => {
                    metadata.push((serial, stream.identification.head.channels))
                }
                OggDemuxEvent::Packet { serial, packet } => audio.push((serial, packet)),
                OggDemuxEvent::Opaque {
                    serial,
                    packets,
                    end_of_stream,
                    ..
                } => {
                    assert_eq!(serial, 123);
                    opaque.push((packets, end_of_stream));
                }
            }
        }
        assert_eq!(metadata, [(17, 1), (18, 2)]);
        assert_eq!(audio.len(), 2);
        assert_eq!((audio[0].0, audio[0].1.keep.clone()), (18, 312..412));
        assert_eq!((audio[1].0, audio[1].1.keep.clone()), (17, 0..77));
        assert_eq!(audio[1].1.data, expected);
        assert_eq!(
            opaque,
            [
                (vec![b"video-header".to_vec()], false),
                (vec![b"frame".to_vec()], true)
            ]
        );
        assert!(reader.next_event().unwrap().is_none());
        assert!(
            reader
                .streams
                .values()
                .all(|phase| matches!(phase, Some(MuxPhase::Ended)))
        );
        let chain = [one_stream(33, 1, 0, 480), one_stream(34, 2, 120, 300)].concat();
        let mut reader = OggDemuxReader::new(Cursor::new(chain));
        let mut serials = Vec::new();
        while let Some(event) = reader.next_event().unwrap() {
            if let OggDemuxEvent::Stream { serial, .. } = event {
                serials.push(serial);
            }
        }
        assert_eq!(serials, [33, 34]);
    }

    fn mux_error(bytes: &[u8]) -> Error {
        let mut reader = OggDemuxReader::new(Cursor::new(bytes));
        loop {
            match reader.next_event() {
                Err(error) => {
                    assert!(matches!(reader.next_event(), Err(Error::Finished)));
                    return error;
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("malformed multiplex accepted"),
            }
        }
    }

    #[test]
    fn multiplexed_corruption_missing_eos_and_wrong_stream_continuations_are_rejected() {
        let (normal, _) = multiplex_fixture();
        let bytes = normal.concat();
        for end in [0, 1, 26, normal[0].len() - 1, bytes.len() - 1] {
            assert!(matches!(mux_error(&bytes[..end]), Error::Truncated));
        }
        let mut crc = bytes.clone();
        *crc.last_mut().unwrap() ^= 1;
        assert!(matches!(mux_error(&crc), Error::Checksum));
        let mut sequence = normal.clone();
        sequence[7][18..22].copy_from_slice(&1u32.to_le_bytes());
        fix_page_crc(&mut sequence[7]);
        assert!(matches!(mux_error(&sequence.concat()), Error::Sequence));
        let mut continuation = normal.clone();
        continuation[8][5] &= !1;
        fix_page_crc(&mut continuation[8]);
        assert!(matches!(
            mux_error(&continuation.concat()),
            Error::Format(_)
        ));
        let mut route = normal.clone();
        route[8][14..18].copy_from_slice(&18u32.to_le_bytes());
        fix_page_crc(&mut route[8]);
        assert!(matches!(mux_error(&route.concat()), Error::Format(_)));
        let mut granule = normal.clone();
        granule[8][6..14].copy_from_slice(&NO_GRANULE.to_le_bytes());
        fix_page_crc(&mut granule[8]);
        assert!(matches!(mux_error(&granule.concat()), Error::Granule));
        let missing = normal[..8].concat();
        assert!(matches!(mux_error(&missing), Error::Truncated));
        let limit = OggLimits {
            audio_packet_bytes: 100,
        };
        let mut reader = OggDemuxReader::with_limits(Cursor::new(bytes), limit).unwrap();
        loop {
            match reader.next_event() {
                Err(error) => {
                    assert!(matches!(error, Error::Limit(_)));
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("packet limit ignored"),
            }
        }
    }
}
