// SPDX-License-Identifier: MIT
//! Bounded streaming Ogg Opus files, independently implemented from
//! [RFC 7845](https://www.rfc-editor.org/rfc/rfc7845) and
//! [RFC 3533](https://www.rfc-editor.org/rfc/rfc3533).
//!
//! This module supports one logical stream, version 1 Opus headers and mapping
//! family 0 (mono or stereo). Chaining, multiplexing and surround mappings are
//! explicitly rejected. Codec packets are preserved without decoding them.
//! Container I/O allocates bounded buffers and belongs outside an audio callback.
//! Header gain is reported to the caller; [`OpusHead::gain_factor`] converts it.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::ops::Range;

use super::packet::{Packet, PacketError};

/// Largest accepted assembled packet or comment header, including padding.
pub const MAX_PACKET_BYTES: usize = 1_048_576;
/// Largest accepted number of UTF-8 user comments.
pub const MAX_COMMENTS: usize = 1024;
/// Largest audio packet written on one page, including its terminal lace.
pub const MAX_WRITER_PACKET_BYTES: usize = 65_024;
const PAGE_PAYLOAD: usize = 65_025;
const HEADER_BYTES: usize = 27;
const NO_GRANULE: u64 = u64::MAX;

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
    /// This implementation supports OpusHead version 1 only.
    Version(u8),
    /// The channel mapping is outside the supported mono/stereo family 0.
    Mapping(u8),
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

/// Mapping-family-0 identification metadata. All sample counts use 48 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpusHead {
    /// Output channels: one mono channel or left/right stereo.
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

    fn bytes(&self) -> Result<[u8; 19], Error> {
        self.validate()?;
        let mut bytes = [0; 19];
        bytes[..8].copy_from_slice(b"OpusHead");
        bytes[8] = 1;
        bytes[9] = self.channels;
        bytes[10..12].copy_from_slice(&self.pre_skip.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.input_sample_rate.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.output_gain_q8.to_le_bytes());
        Ok(bytes)
    }

    /// Linear playback multiplier corresponding to the header's Q7.8 dB gain.
    pub fn gain_factor(self) -> f32 {
        10.0f32.powf(f32::from(self.output_gain_q8) / 5120.0)
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
        }
    }

    fn first_byte(&mut self) -> Result<Option<u8>, Error> {
        let mut byte = [0];
        loop {
            match self.source.read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(_) => return Ok(Some(byte[0])),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn page(&mut self) -> Result<Option<Page>, Error> {
        if self.ended {
            return Ok(None);
        }
        let Some(first) = self.first_byte()? else {
            return Ok(None);
        };
        self.page_bytes.resize(HEADER_BYTES, 0);
        self.page_bytes[0] = first;
        self.source
            .read_exact(&mut self.page_bytes[1..HEADER_BYTES])?;
        if &self.page_bytes[..4] != b"OggS" || self.page_bytes[4] != 0 {
            return Err(Error::Format("Ogg capture pattern or version"));
        }
        let flags = self.page_bytes[5];
        if flags & !7 != 0 {
            return Err(Error::Format("reserved page flags"));
        }
        let granule = u64::from_le_bytes(self.page_bytes[6..14].try_into().expect("fixed header"));
        if granule != NO_GRANULE && granule > i64::MAX as u64 {
            return Err(Error::Granule);
        }
        let serial = u32::from_le_bytes(self.page_bytes[14..18].try_into().expect("fixed header"));
        let sequence =
            u32::from_le_bytes(self.page_bytes[18..22].try_into().expect("fixed header"));
        let expected_crc =
            u32::from_le_bytes(self.page_bytes[22..26].try_into().expect("fixed header"));
        let segments = usize::from(self.page_bytes[26]);
        self.page_bytes.resize(HEADER_BYTES + segments, 0);
        self.source
            .read_exact(&mut self.page_bytes[HEADER_BYTES..])?;
        let payload: usize = self.page_bytes[HEADER_BYTES..]
            .iter()
            .map(|&lace| usize::from(lace))
            .sum();
        self.page_bytes.resize(HEADER_BYTES + segments + payload, 0);
        self.source
            .read_exact(&mut self.page_bytes[HEADER_BYTES + segments..])?;
        self.page_bytes[22..26].fill(0);
        if checksum(&self.page_bytes) != expected_crc {
            return Err(Error::Checksum);
        }
        if let Some(expected) = self.serial {
            if serial != expected {
                return Err(Error::Serial);
            }
            if flags & 2 != 0 {
                return Err(Error::Format("repeated beginning of stream"));
            }
        } else {
            if flags != 2 {
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
            if self.partial.len() > MAX_PACKET_BYTES - length {
                return Err(Error::Limit("assembled packet"));
            }
            self.partial
                .extend_from_slice(&self.page_bytes[offset..offset + length]);
            offset += length;
            if lace < 255 {
                packets.push(std::mem::take(&mut self.partial));
            }
        }
        if packets.is_empty() && granule != NO_GRANULE {
            return Err(Error::Granule);
        }
        if !packets.is_empty() && granule == NO_GRANULE {
            return Err(Error::Granule);
        }
        if flags & 4 != 0 {
            if packets.is_empty() || !self.partial.is_empty() {
                return Err(Error::Format("incomplete EOS page"));
            }
            if self.first_byte()?.is_some() {
                return Err(Error::Format("data after EOS"));
            }
            self.ended = true;
        }
        Ok(Some(Page {
            packets,
            flags,
            granule,
            incomplete: !self.partial.is_empty(),
        }))
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

/// Bounded sequential reader for one mono/stereo Ogg Opus logical stream.
///
/// Completed packets from one page are buffered so timestamps and final trims
/// are validated before any packet from that page is returned. EOF before EOS,
/// chained streams, orphan continuations and corrupt checksums return errors.
pub struct OggReader<R> {
    demux: Demux<R>,
    head: OpusHead,
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
        let mut demux = Demux::new(source);
        let mut first = demux.page()?.ok_or(Error::Truncated)?;
        if first.flags != 2 || first.granule != 0 || first.incomplete || first.packets.len() != 1 {
            return Err(Error::Format("OpusHead must complete alone on BOS page"));
        }
        let head = OpusHead::parse(&first.packets.remove(0))?;
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
        Ok(Self {
            demux,
            head,
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
        &self.head
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
            let samples = Packet::parse(packet)?.samples(48_000)?;
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
            None if eos && page.granule >= u64::from(self.head.pre_skip) => 0,
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
                    || page.granule < origin + u64::from(self.head.pre_skip)))
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
            let kept_start = u64::from(self.head.pre_skip)
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

struct PendingPacket {
    data: Vec<u8>,
    begin: u64,
    end: u64,
}

/// Ogg Opus writer that buffers its last audio packet until final trimming.
///
/// Each audio packet occupies one page and is bounded to 65,024 bytes. Larger
/// tags can span pages, up to 1 MiB. `finish` takes the desired sample count
/// after pre-skip; feed the encoder sufficient padding/drain to cover it.
pub struct OggWriter<W> {
    sink: W,
    serial: u32,
    sequence: u32,
    head: OpusHead,
    pending: Option<PendingPacket>,
    decoded: u64,
    failed: bool,
}

impl<W: Write> OggWriter<W> {
    /// Write identification and comment headers for a new logical stream.
    /// The caller chooses its stream serial number.
    pub fn new(sink: W, serial: u32, head: OpusHead, tags: &OpusTags) -> Result<Self, Error> {
        let identification = head.bytes()?;
        let comments = tags.bytes()?;
        let mut writer = Self {
            sink,
            serial,
            sequence: 0,
            head,
            pending: None,
            decoded: 0,
            failed: false,
        };
        writer.packet_pages(&identification, 2, 0)?;
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
            let flags = if offset == 0 { first_flags } else { 1 };
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
        if packet.len() > MAX_WRITER_PACKET_BYTES {
            return Err(Error::Limit("one-page audio packet"));
        }
        let samples = Packet::parse(packet)?.samples(48_000)? as u64;
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
}
