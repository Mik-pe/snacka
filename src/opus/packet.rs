// SPDX-License-Identifier: MIT
//! Borrowed Opus packet framing from RFC 6716 section 3.
//!
//! Parsing validates the complete packet before exposing any frames. It checks
//! framing, lengths, padding and duration, but does not validate entropy-coded
//! frame contents. Ordinary framing and the self-delimiting variant from
//! Appendix B share the same borrowed frame representation.

use std::fmt;
use std::iter::FusedIterator;

/// Maximum compressed size of an individual Opus frame, in bytes.
pub const MAX_FRAME_BYTES: usize = 1275;
/// Maximum number of frames in an Opus packet.
pub const MAX_FRAMES: usize = 48;
/// Maximum packet duration, expressed as samples per channel at 48 kHz.
pub const MAX_PACKET_SAMPLES_48KHZ: usize = 5760;

/// Invalid Opus framing or an unsupported output sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    /// The packet does not contain a TOC byte.
    Empty,
    /// A required framing byte or two-byte length is missing.
    Truncated,
    /// The signaled number of frames is zero or greater than 48.
    InvalidFrameCount,
    /// The packet would contain more than 120 ms of audio.
    InvalidDuration,
    /// Padding consumes bytes required by the packet headers or frame payloads.
    InvalidPadding,
    /// A constant-size packet payload does not divide evenly into its frames.
    InvalidCbrLength,
    /// The explicit frame lengths exceed the available payload.
    InvalidFrameLength,
    /// An implicit frame length is greater than 1275 bytes.
    FrameTooLarge,
    /// The requested rate is not 8, 12, 16, 24 or 48 kHz.
    UnsupportedSampleRate,
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "empty Opus packet",
            Self::Truncated => "truncated Opus packet header",
            Self::InvalidFrameCount => "invalid Opus frame count",
            Self::InvalidDuration => "Opus packet exceeds 120 ms",
            Self::InvalidPadding => "invalid Opus packet padding",
            Self::InvalidCbrLength => "Opus CBR payload is not divisible by frame count",
            Self::InvalidFrameLength => "Opus frame lengths exceed payload",
            Self::FrameTooLarge => "Opus frame exceeds 1275 bytes",
            Self::UnsupportedSampleRate => "unsupported Opus sample rate",
        })
    }
}

impl std::error::Error for PacketError {}

/// Codec layers selected by an Opus TOC configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The SILK linear-prediction layer only.
    Silk,
    /// Both SILK and CELT, with the high frequencies coded by CELT.
    Hybrid,
    /// The CELT transform layer only.
    Celt,
}

/// Encoded audio bandwidth, independent of the decoder's output sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    /// Frequencies up to 4 kHz.
    Narrowband,
    /// Frequencies up to 6 kHz.
    Mediumband,
    /// Frequencies up to 8 kHz.
    Wideband,
    /// Frequencies up to 12 kHz.
    Superwideband,
    /// Frequencies up to 20 kHz.
    Fullband,
}

/// The table-of-contents byte that selects one of 32 Opus configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toc(u8);

impl Toc {
    /// Interpret a TOC byte; all 256 byte values select valid TOC fields.
    ///
    /// Packet framing must still be validated with [`Packet::parse`].
    pub const fn from_byte(byte: u8) -> Self {
        Self(byte)
    }

    /// The original TOC byte.
    pub const fn byte(self) -> u8 {
        self.0
    }

    /// The configuration number, from 0 through 31.
    pub const fn config(self) -> u8 {
        self.0 >> 3
    }

    /// The selected codec layers.
    pub const fn mode(self) -> Mode {
        match self.config() {
            0..=11 => Mode::Silk,
            12..=15 => Mode::Hybrid,
            _ => Mode::Celt,
        }
    }

    /// The encoded audio bandwidth.
    pub const fn bandwidth(self) -> Bandwidth {
        match self.config() {
            0..=3 | 16..=19 => Bandwidth::Narrowband,
            4..=7 => Bandwidth::Mediumband,
            8..=11 | 20..=23 => Bandwidth::Wideband,
            12..=13 | 24..=27 => Bandwidth::Superwideband,
            _ => Bandwidth::Fullband,
        }
    }

    /// The encoded channel count: one for mono, two for stereo.
    pub const fn channels(self) -> u8 {
        1 + ((self.0 >> 2) & 1)
    }

    /// The frame packing code, from 0 through 3.
    pub const fn frame_code(self) -> u8 {
        self.0 & 3
    }

    /// Samples per channel in one frame at a standard Opus output rate.
    ///
    /// Valid rates are 8000, 12000, 16000, 24000 and 48000 Hz.
    pub const fn frame_samples(self, sample_rate: u32) -> Result<usize, PacketError> {
        match sample_rate {
            8000 | 12000 | 16000 | 24000 | 48000 => {
                Ok(self.frame_samples_48khz() * sample_rate as usize / 48000)
            }
            _ => Err(PacketError::UnsupportedSampleRate),
        }
    }

    const fn frame_samples_48khz(self) -> usize {
        let config = self.config();
        if config < 12 {
            match config & 3 {
                0 => 480,
                1 => 960,
                2 => 1920,
                _ => 2880,
            }
        } else if config < 16 {
            480 << (config & 1)
        } else {
            120 << (config & 3)
        }
    }
}

/// A fully validated Opus packet borrowing the original encoded bytes.
///
/// Frame lengths occupy a fixed stack array, and parsing never allocates. Empty
/// frames are legal and represent a missing or discontinuously transmitted
/// frame. Successful parsing does not establish that frame contents are valid
/// Opus audio; the codec decoder performs that validation.
#[derive(Debug, Clone)]
pub struct Packet<'a> {
    bytes: &'a [u8],
    payload: &'a [u8],
    lengths: [u16; MAX_FRAMES],
    count: usize,
    padding: usize,
    toc: Toc,
    vbr: bool,
}

impl<'a> Packet<'a> {
    /// Validate ordinary RFC 6716 packet framing without copying the payload.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, PacketError> {
        let toc = Toc::from_byte(*bytes.first().ok_or(PacketError::Empty)?);
        let mut lengths = [0; MAX_FRAMES];
        let mut start = 1;
        let mut end = bytes.len();
        let count;
        let vbr;

        match toc.frame_code() {
            0 => {
                count = 1;
                vbr = false;
                lengths[0] = checked_length(end - start)?;
            }
            1 => {
                count = 2;
                vbr = false;
                fill_cbr(&mut lengths[..count], end - start)?;
            }
            2 => {
                count = 2;
                vbr = true;
                let first = read_length(bytes, &mut start, end)?;
                let second = (end - start)
                    .checked_sub(usize::from(first))
                    .ok_or(PacketError::InvalidFrameLength)?;
                lengths[0] = first;
                lengths[1] = checked_length(second)?;
            }
            _ => {
                let control = *bytes.get(start).ok_or(PacketError::Truncated)?;
                start += 1;
                count = usize::from(control & 0x3f);
                if count == 0 || count > MAX_FRAMES {
                    return Err(PacketError::InvalidFrameCount);
                }
                if count * toc.frame_samples_48khz() > MAX_PACKET_SAMPLES_48KHZ {
                    return Err(PacketError::InvalidDuration);
                }
                vbr = control & 0x80 != 0;
                if control & 0x40 != 0 {
                    loop {
                        // Reserving padding as it is read bounds the loop and
                        // prevents a length header from reading trailing padding.
                        let value = *bytes
                            .get(start)
                            .filter(|_| start < end)
                            .ok_or(PacketError::InvalidPadding)?;
                        start += 1;
                        let additional = usize::from(value) - usize::from(value == 255);
                        end = end
                            .checked_sub(additional)
                            .filter(|&new_end| new_end >= start)
                            .ok_or(PacketError::InvalidPadding)?;
                        if value != 255 {
                            break;
                        }
                    }
                }
                if vbr {
                    let mut explicit_bytes = 0;
                    for length in &mut lengths[..count - 1] {
                        *length = read_length(bytes, &mut start, end)?;
                        explicit_bytes += usize::from(*length);
                    }
                    let last = (end - start)
                        .checked_sub(explicit_bytes)
                        .ok_or(PacketError::InvalidFrameLength)?;
                    lengths[count - 1] = checked_length(last)?;
                } else {
                    fill_cbr(&mut lengths[..count], end - start)?;
                }
            }
        }

        Ok(Self {
            bytes,
            payload: &bytes[start..end],
            lengths,
            count,
            padding: bytes.len() - end,
            toc,
            vbr,
        })
    }

    /// Validate one Appendix B self-delimiting packet at the start of `bytes`.
    ///
    /// Returns its metadata and consumed byte count, leaving subsequent stream
    /// packets untouched. The transport must select this framing explicitly:
    /// it cannot be auto-detected. [`Self::bytes`] includes the extra frame-size
    /// header, so use its parsed frames or repacketize before ordinary decoding.
    pub fn parse_self_delimited(bytes: &'a [u8]) -> Result<(Self, usize), PacketError> {
        let toc = Toc::from_byte(*bytes.first().ok_or(PacketError::Empty)?);
        let mut start = 1;
        let mut padding = 0usize;
        let (count, vbr) = match toc.frame_code() {
            0 => (1, false),
            1 => (2, false),
            2 => (2, true),
            _ => {
                let control = *bytes.get(start).ok_or(PacketError::Truncated)?;
                start += 1;
                let count = usize::from(control & 0x3f);
                if count == 0 || count > MAX_FRAMES {
                    return Err(PacketError::InvalidFrameCount);
                }
                if count * toc.frame_samples_48khz() > MAX_PACKET_SAMPLES_48KHZ {
                    return Err(PacketError::InvalidDuration);
                }
                if control & 0x40 != 0 {
                    loop {
                        let value = *bytes.get(start).ok_or(PacketError::InvalidPadding)?;
                        start += 1;
                        let additional = usize::from(value) - usize::from(value == 255);
                        padding = padding
                            .checked_add(additional)
                            .filter(|&count| count <= bytes.len() - start)
                            .ok_or(PacketError::InvalidPadding)?;
                        if value != 255 {
                            break;
                        }
                    }
                }
                (count, control & 0x80 != 0)
            }
        };
        let mut lengths = [0; MAX_FRAMES];
        if vbr {
            for length in &mut lengths[..count - 1] {
                *length = read_length(bytes, &mut start, bytes.len())?;
            }
        }
        let last = read_length(bytes, &mut start, bytes.len())?;
        if vbr {
            lengths[count - 1] = last;
        } else {
            lengths[..count].fill(last);
        }
        let size = lengths[..count]
            .iter()
            .map(|&length| usize::from(length))
            .sum::<usize>();
        let end = start
            .checked_add(size)
            .ok_or(PacketError::InvalidFrameLength)?;
        let consumed = end
            .checked_add(padding)
            .filter(|&size| size <= bytes.len())
            .ok_or(PacketError::InvalidFrameLength)?;
        Ok((
            Self {
                bytes: &bytes[..consumed],
                payload: &bytes[start..end],
                lengths,
                count,
                padding,
                toc,
                vbr,
            },
            consumed,
        ))
    }

    /// The original packet, including all framing and trailing padding.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The table-of-contents fields.
    pub const fn toc(&self) -> Toc {
        self.toc
    }

    /// Number of compressed frames, including any zero-length frames.
    pub const fn frame_count(&self) -> usize {
        self.count
    }

    /// Whether frame lengths are explicitly signaled as variable-size framing.
    ///
    /// This describes packet framing, not the encoder's bitrate policy.
    pub const fn is_vbr(&self) -> bool {
        self.vbr
    }

    /// Number of trailing padding bytes, excluding padding length headers.
    pub const fn padding_len(&self) -> usize {
        self.padding
    }

    /// Trailing padding bytes, excluding the packet's padding length headers.
    ///
    /// Optional extensions may occupy this region. Their validity is separate
    /// from base packet framing and can be inspected with [`Self::extensions`].
    pub fn padding(&self) -> &'a [u8] {
        &self.bytes[self.bytes.len() - self.padding..]
    }

    /// Inspect optional padding extensions without allocating or changing audio.
    ///
    /// Invalid extension framing is reported by the iterator, and does not make
    /// this packet's base audio invalid. This does not enable any audio extension.
    pub fn extensions(&self) -> super::extensions::Extensions<'a> {
        super::extensions::Extensions::from_packet(self.padding(), self.count)
    }

    /// Samples per channel in each frame at the requested output rate.
    pub const fn frame_samples(&self, sample_rate: u32) -> Result<usize, PacketError> {
        self.toc.frame_samples(sample_rate)
    }

    /// Total samples per channel at the requested output rate.
    pub const fn samples(&self, sample_rate: u32) -> Result<usize, PacketError> {
        match self.frame_samples(sample_rate) {
            Ok(samples) => Ok(samples * self.count),
            Err(error) => Err(error),
        }
    }

    /// Iterate over frame payloads borrowed from this packet, without allocation.
    pub fn frames(&self) -> Frames<'_, 'a> {
        Frames {
            payload: self.payload,
            lengths: &self.lengths[..self.count],
        }
    }
}

fn checked_length(length: usize) -> Result<u16, PacketError> {
    if length > MAX_FRAME_BYTES {
        Err(PacketError::FrameTooLarge)
    } else {
        Ok(length as u16)
    }
}

fn fill_cbr(lengths: &mut [u16], bytes: usize) -> Result<(), PacketError> {
    if !bytes.is_multiple_of(lengths.len()) {
        return Err(PacketError::InvalidCbrLength);
    }
    lengths.fill(checked_length(bytes / lengths.len())?);
    Ok(())
}

fn read_length(bytes: &[u8], position: &mut usize, end: usize) -> Result<u16, PacketError> {
    if *position >= end {
        return Err(PacketError::Truncated);
    }
    let first = u16::from(bytes[*position]);
    *position += 1;
    if first < 252 {
        return Ok(first);
    }
    if *position >= end {
        return Err(PacketError::Truncated);
    }
    let second = u16::from(bytes[*position]);
    *position += 1;
    Ok(first + 4 * second)
}

/// Allocation-free, exact-size iterator over validated borrowed frame payloads.
#[derive(Debug, Clone)]
pub struct Frames<'packet, 'data> {
    payload: &'data [u8],
    lengths: &'packet [u16],
}

impl<'data> Iterator for Frames<'_, 'data> {
    type Item = &'data [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let (&length, remaining) = self.lengths.split_first()?;
        let (frame, payload) = self.payload.split_at(usize::from(length));
        self.lengths = remaining;
        self.payload = payload;
        Some(frame)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.lengths.len();
        (count, Some(count))
    }
}

impl ExactSizeIterator for Frames<'_, '_> {}
impl FusedIterator for Frames<'_, '_> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(bytes: &[u8]) -> Vec<&[u8]> {
        Packet::parse(bytes).unwrap().frames().collect()
    }

    #[test]
    fn every_toc_configuration_and_standard_rate() {
        let rates = [8000, 12000, 16000, 24000, 48000];
        let frame_samples = [
            480, 960, 1920, 2880, 480, 960, 1920, 2880, 480, 960, 1920, 2880, 480, 960, 480, 960,
            120, 240, 480, 960, 120, 240, 480, 960, 120, 240, 480, 960, 120, 240, 480, 960,
        ];
        let bandwidths = [
            Bandwidth::Narrowband,
            Bandwidth::Mediumband,
            Bandwidth::Wideband,
            Bandwidth::Superwideband,
            Bandwidth::Fullband,
            Bandwidth::Narrowband,
            Bandwidth::Wideband,
            Bandwidth::Superwideband,
            Bandwidth::Fullband,
        ];
        for config in 0u8..32 {
            for channels in 1..=2 {
                let byte = (config << 3) | ((channels - 1) << 2);
                let encoded = [byte];
                let packet = Packet::parse(&encoded).unwrap();
                let toc = packet.toc();
                assert_eq!(toc.byte(), byte);
                assert_eq!(toc.config(), config);
                assert_eq!(toc.channels(), channels);
                assert_eq!(toc.frame_code(), 0);
                let (mode, bandwidth_index) = match config {
                    0..=11 => (Mode::Silk, usize::from(config / 4)),
                    12..=15 => (Mode::Hybrid, 3 + usize::from((config - 12) / 2)),
                    _ => (Mode::Celt, 5 + usize::from((config - 16) / 4)),
                };
                assert_eq!(toc.mode(), mode);
                assert_eq!(toc.bandwidth(), bandwidths[bandwidth_index]);
                for rate in rates {
                    let expected = frame_samples[usize::from(config)] * rate as usize / 48000;
                    assert_eq!(packet.frame_samples(rate), Ok(expected));
                    assert_eq!(packet.samples(rate), Ok(expected));
                }
            }
        }
        for rate in [0, 1, 7999, 8001, 44100, u32::MAX] {
            assert_eq!(
                Toc::from_byte(0).frame_samples(rate),
                Err(PacketError::UnsupportedSampleRate)
            );
        }
    }

    #[test]
    fn codes_zero_one_and_two_expose_exact_payloads() {
        assert_eq!(frames(&[0x00, 1, 2, 3]), [&[1, 2, 3][..]]);
        assert_eq!(frames(&[0x01, 1, 2, 3, 4]), [&[1, 2][..], &[3, 4][..]]);
        assert_eq!(frames(&[0x02, 1, 10, 20, 30]), [&[10][..], &[20, 30][..]]);
        assert_eq!(frames(&[0x02, 0, 10]), [&[][..], &[10][..]]);
        assert_eq!(frames(&[0x02, 1, 10]), [&[10][..], &[][..]]);
        assert_eq!(frames(&[0x00]), [&[][..]]);
        assert_eq!(frames(&[0x01]), [&[][..], &[][..]]);
        assert_eq!(frames(&[0x02, 0]), [&[][..], &[][..]]);
    }

    #[test]
    fn code_three_cbr_and_vbr_allow_empty_frames() {
        assert_eq!(
            frames(&[0x83, 3, 10, 20, 30]),
            [&[10][..], &[20][..], &[30][..]]
        );
        assert_eq!(
            frames(&[0x83, 0x83, 1, 2, 10, 20, 21, 30, 31, 32]),
            [&[10][..], &[20, 21][..], &[30, 31, 32][..]]
        );
        assert_eq!(
            frames(&[0x83, 0x83, 0, 1, 10]),
            [&[][..], &[10][..], &[][..]]
        );
        assert_eq!(frames(&[0x83, 0x83, 0, 0]), [&[][..], &[][..], &[][..]]);
        assert_eq!(frames(&[0x83, 0x81, 10, 20]), [&[10, 20][..]]);
        assert_eq!(frames(&[0x83, 1, 10, 20]), [&[10, 20][..]]);
        let vbr = Packet::parse(&[0x83, 0x83, 0, 0]).unwrap();
        assert!(vbr.is_vbr());
        assert_eq!(vbr.frame_count(), 3);
        assert_eq!(vbr.samples(48000), Ok(360));
        assert!(!Packet::parse(&[0x83, 3]).unwrap().is_vbr());
    }

    #[test]
    fn padding_is_reserved_before_frame_lengths_are_read() {
        let cbr = [0x83, 0x42, 2, 10, 20, 77, 88];
        let packet = Packet::parse(&cbr).unwrap();
        assert_eq!(packet.padding_len(), 2);
        assert_eq!(packet.bytes(), cbr);
        assert_eq!(packet.frames().collect::<Vec<_>>(), [&[10][..], &[20][..]]);
        assert_eq!(
            frames(&[0x83, 0xc2, 2, 1, 10, 20, 30, 99, 99]),
            [&[10][..], &[20, 30][..]]
        );
        assert_eq!(frames(&[0x83, 0x41, 0]), [&[][..]]);

        for (headers, padding) in [
            (&[254][..], 254),
            (&[255, 0][..], 254),
            (&[255, 1][..], 255),
            (&[255, 255, 2][..], 510),
        ] {
            let mut bytes = vec![0x83, 0x41];
            bytes.extend_from_slice(headers);
            bytes.push(55);
            bytes.resize(bytes.len() + padding, 0xa5);
            let packet = Packet::parse(&bytes).unwrap();
            assert_eq!(packet.padding_len(), padding);
            assert_eq!(packet.frames().next(), Some(&[55][..]));
        }
        assert_eq!(
            Packet::parse(&[0x83, 0xc2, 1, 252, 0]).unwrap_err(),
            PacketError::Truncated
        );
    }

    #[test]
    fn every_representable_vbr_frame_length() {
        for size in 0..=MAX_FRAME_BYTES {
            let mut bytes = vec![0x82];
            if size < 252 {
                bytes.push(size as u8);
            } else {
                bytes.push(252 + (size & 3) as u8);
                bytes.push(((size - 252) / 4) as u8);
            }
            bytes.resize(bytes.len() + size, 0x5a);
            bytes.push(0x6b);
            let packet = Packet::parse(&bytes).unwrap();
            let mut frame_iter = packet.frames();
            assert_eq!(frame_iter.next().unwrap().len(), size);
            assert_eq!(frame_iter.next(), Some(&[0x6b][..]));
            assert_eq!(frame_iter.next(), None);
        }
    }

    #[test]
    fn maximum_duration_for_every_configuration() {
        for config in 0u8..32 {
            let samples = Toc::from_byte(config << 3).frame_samples(48000).unwrap();
            let limit = MAX_PACKET_SAMPLES_48KHZ / samples;
            let bytes = [(config << 3) | 3, limit as u8];
            let packet = Packet::parse(&bytes).unwrap();
            assert_eq!(packet.frame_count(), limit);
            assert_eq!(packet.samples(48000), Ok(limit * samples));
            let too_long = [(config << 3) | 3, (limit + 1) as u8];
            assert!(Packet::parse(&too_long).is_err());
        }
        assert_eq!(Packet::parse(&[0x83, 48]).unwrap().samples(48000), Ok(5760));
        assert_eq!(Packet::parse(&[0x1b, 2]).unwrap().samples(48000), Ok(5760));
        assert_eq!(
            Packet::parse(&[0x1b, 3]).unwrap_err(),
            PacketError::InvalidDuration
        );
    }

    #[test]
    fn full_size_packets_with_48_frames_keep_every_boundary() {
        let mut cbr = vec![0x83, 48];
        for index in 0..48 {
            cbr.extend(std::iter::repeat_n(index, MAX_FRAME_BYTES));
        }
        let mut vbr = vec![0x83, 0x80 | 48];
        for _ in 0..47 {
            vbr.extend_from_slice(&[255, 255]);
        }
        vbr.extend_from_slice(&cbr[2..]);

        for bytes in [&cbr, &vbr] {
            let packet = Packet::parse(bytes).unwrap();
            assert_eq!(packet.frame_count(), 48);
            assert_eq!(packet.samples(8000), Ok(960));
            for (index, frame) in packet.frames().enumerate() {
                assert_eq!(frame.len(), MAX_FRAME_BYTES);
                assert!(frame.iter().all(|&value| usize::from(value) == index));
            }
        }

        // The final, implicit frame may be empty, but its predecessors must fit.
        assert!(Packet::parse(&vbr[..vbr.len() - MAX_FRAME_BYTES]).is_ok());
        assert_eq!(
            Packet::parse(&vbr[..vbr.len() - MAX_FRAME_BYTES - 1]).unwrap_err(),
            PacketError::InvalidFrameLength
        );
    }

    #[test]
    fn rejects_malformed_headers_lengths_and_padding() {
        let cases: &[(&[u8], PacketError)] = &[
            (&[], PacketError::Empty),
            (&[2], PacketError::Truncated),
            (&[2, 252], PacketError::Truncated),
            (&[2, 1], PacketError::InvalidFrameLength),
            (&[2, 255, 255], PacketError::InvalidFrameLength),
            (&[1, 0], PacketError::InvalidCbrLength),
            (&[3], PacketError::Truncated),
            (&[3, 0], PacketError::InvalidFrameCount),
            (&[3, 0x80], PacketError::InvalidFrameCount),
            (&[0x83, 49], PacketError::InvalidFrameCount),
            (&[0x83, 63], PacketError::InvalidFrameCount),
            (&[3, 0x41], PacketError::InvalidPadding),
            (&[3, 0x41, 1], PacketError::InvalidPadding),
            (&[3, 0x41, 255, 0], PacketError::InvalidPadding),
            (&[0x83, 2, 0], PacketError::InvalidCbrLength),
            (&[0x83, 0x82], PacketError::Truncated),
            (&[0x83, 0x82, 252], PacketError::Truncated),
            (&[0x83, 0x82, 1], PacketError::InvalidFrameLength),
            (
                &[0x83, 0x83, 2, 2, 1, 2, 3],
                PacketError::InvalidFrameLength,
            ),
        ];
        for &(bytes, expected) in cases {
            assert_eq!(Packet::parse(bytes).unwrap_err(), expected, "{bytes:?}");
        }
    }

    #[test]
    fn enforces_frame_size_limit_for_every_implicit_length() {
        for size in [MAX_FRAME_BYTES, MAX_FRAME_BYTES + 1] {
            let code_zero = vec![0; size + 1];
            let mut code_one = vec![0; 2 * size + 1];
            code_one[0] = 1;
            let mut code_two = vec![0; size + 2];
            code_two[0] = 2;
            let mut cbr_three = vec![0; size + 2];
            cbr_three[..2].copy_from_slice(&[3, 1]);
            let mut vbr_three = vec![0; size + 2];
            vbr_three[..2].copy_from_slice(&[3, 0x81]);
            for bytes in [&code_zero, &code_one, &code_two, &cbr_three, &vbr_three] {
                if size == MAX_FRAME_BYTES {
                    assert!(Packet::parse(bytes).is_ok());
                } else {
                    assert_eq!(
                        Packet::parse(bytes).unwrap_err(),
                        PacketError::FrameTooLarge
                    );
                }
            }
        }
    }

    #[test]
    fn iterator_is_exact_size_and_fused() {
        let packet = Packet::parse(&[0x83, 3, 10, 20, 30]).unwrap();
        let mut iter = packet.frames();
        for count in (1..=3).rev() {
            assert_eq!(iter.len(), count);
            assert_eq!(iter.size_hint(), (count, Some(count)));
            assert!(iter.next().is_some());
        }
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn arbitrary_bytes_do_not_panic_or_expose_invalid_frames() {
        fn check(bytes: &[u8]) {
            if let Ok(packet) = Packet::parse(bytes) {
                let sizes = packet
                    .frames()
                    .map(|frame| {
                        assert!(frame.len() <= MAX_FRAME_BYTES);
                        frame.len()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(sizes.len(), packet.frame_count());
                assert!((1..=MAX_FRAMES).contains(&sizes.len()));
                assert!(packet.samples(48000).unwrap() <= MAX_PACKET_SAMPLES_48KHZ);
                assert!(sizes.iter().sum::<usize>() + packet.padding_len() < bytes.len());
            }
        }
        check(&[]);
        for first in 0u8..=255 {
            check(&[first]);
            for second in 0u8..=255 {
                check(&[first, second]);
            }
        }
        let mut state = 0x4d59_5df4_d0f3_3173u64;
        let mut bytes = [0u8; 2048];
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = state as usize % (bytes.len() + 1);
            for byte in &mut bytes[..len] {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            check(&bytes[..len]);
        }
    }
}
