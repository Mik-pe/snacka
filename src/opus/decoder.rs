// SPDX-License-Identifier: MIT
//! Ordinary raw Opus decoding at 48 kHz, composing owned SILK and CELT layers.

use std::fmt;

use super::celt::{self, CeltDecoder, CeltLayer};
use super::packet::{Bandwidth, Mode, Packet, PacketError, Toc};
use super::range;
use super::silk::{self, Resampler, SilkDecoder};

const MAX_FRAME_SAMPLES: usize = 2880;
const MAX_INTERNAL_SAMPLES: usize = 960;
const CROSSLAP: usize = 120;

/// An invalid Opus packet, output buffer or decoded layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Output channels must be one or two.
    Argument,
    /// Complete ordinary Opus packet framing is invalid.
    Packet(PacketError),
    /// Output cannot hold every frame in the packet.
    Output,
    /// A layer or transition redundancy has invalid coded contents.
    Corrupt,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument => formatter.write_str("invalid Opus decoder channel count"),
            Self::Packet(error) => error.fmt(formatter),
            Self::Output => formatter.write_str("Opus output buffer is too small"),
            Self::Corrupt => formatter.write_str("invalid Opus coded contents"),
        }
    }
}

impl std::error::Error for Error {}

impl From<PacketError> for Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}

impl From<celt::Error> for Error {
    fn from(_: celt::Error) -> Self {
        Self::Corrupt
    }
}

impl From<range::Error> for Error {
    fn from(_: range::Error) -> Self {
        Self::Corrupt
    }
}

impl From<silk::Error> for Error {
    fn from(_: silk::Error) -> Self {
        Self::Corrupt
    }
}

#[derive(Clone, Copy)]
struct Redundancy {
    beginning: bool,
    offset: usize,
}

fn redundancy(
    decoder: &mut range::Decoder<'_>,
    mode: Mode,
    length: usize,
) -> Result<Option<Redundancy>, Error> {
    let remaining = (length as u64 * 8).saturating_sub(decoder.tell());
    let present = match mode {
        Mode::Silk => remaining >= 17,
        Mode::Hybrid => remaining >= 37 && decoder.bit(12)?,
        Mode::Celt => false,
    };
    if !present {
        return Ok(None);
    }
    let beginning = decoder.bit(1)?;
    let bytes = if mode == Mode::Hybrid {
        decoder.uniform(256)? as usize + 2
    } else {
        length.saturating_sub(decoder.tell().div_ceil(8) as usize)
    };
    if bytes < 2 || bytes > length || decoder.tell() > (length - bytes) as u64 * 8 {
        return Err(Error::Corrupt);
    }
    let offset = length - bytes;
    decoder.shrink(offset)?;
    Ok(Some(Redundancy { beginning, offset }))
}

fn silk_bandwidth(bandwidth: Bandwidth) -> silk::Bandwidth {
    match bandwidth {
        Bandwidth::Narrowband => silk::Bandwidth::Narrow,
        Bandwidth::Mediumband => silk::Bandwidth::Medium,
        _ => silk::Bandwidth::Wide,
    }
}

fn celt_end(bandwidth: Bandwidth) -> usize {
    match bandwidth {
        Bandwidth::Narrowband => 13,
        Bandwidth::Mediumband | Bandwidth::Wideband => 17,
        Bandwidth::Superwideband => 19,
        Bandwidth::Fullband => 21,
    }
}

fn silk_duration(samples: usize) -> Result<silk::PacketDuration, Error> {
    match samples {
        480 => Ok(silk::PacketDuration::Ms10),
        960 => Ok(silk::PacketDuration::Ms20),
        1920 => Ok(silk::PacketDuration::Ms40),
        2880 => Ok(silk::PacketDuration::Ms60),
        _ => Err(Error::Corrupt),
    }
}

/// Stateful 48 kHz decoder for raw single-stream SILK, Hybrid and CELT packets.
///
/// PCM is normalized interleaved floating point. Mono streams can be duplicated
/// to stereo output; stereo streams can be averaged to mono. Framing and full output capacity are checked
/// before changing history. A compressed error can advance part of the packet;
/// reset after `Corrupt`. Transform plans allocate only during construction;
/// packet decoding and configuration transitions use fixed working buffers.
///
/// Missing frames use an independent attenuation concealment policy. SILK uses
/// Snacka's causal resampler, permitted by RFC 6716 §4.2.9. These choices need not
/// reproduce the reference codec's concealment or resampling sample-for-sample.
pub struct Decoder {
    channels: usize,
    silk: SilkDecoder,
    celt: CeltDecoder,
    resamplers: [Resampler; 2],
    internal_bandwidth: Option<silk::Bandwidth>,
    silk_pcm: [f32; MAX_INTERNAL_SAMPLES * 2],
    lowband: [[f32; MAX_FRAME_SAMPLES]; 2],
    frame: [f32; MAX_FRAME_SAMPLES * 2],
    redundant: [f32; 2 * CROSSLAP * 2],
    history: [f32; CROSSLAP * 2],
    window: [f32; CROSSLAP],
    previous_mode: Option<Mode>,
    previous_bandwidth: Option<Bandwidth>,
    previous_redundancy_end: bool,
    loss_count: u32,
    final_range: u32,
}

impl Decoder {
    /// Prepare a decoder for one or two output channels.
    pub fn new(channels: usize) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Argument);
        }
        Ok(Self {
            channels,
            silk: SilkDecoder::new(),
            celt: CeltDecoder::new(channels).map_err(|_| Error::Argument)?,
            resamplers: std::array::from_fn(|_| Resampler::new(silk::Bandwidth::Wide)),
            internal_bandwidth: None,
            silk_pcm: [0.0; MAX_INTERNAL_SAMPLES * 2],
            lowband: [[0.0; MAX_FRAME_SAMPLES]; 2],
            frame: [0.0; MAX_FRAME_SAMPLES * 2],
            redundant: [0.0; 2 * CROSSLAP * 2],
            history: [0.0; CROSSLAP * 2],
            window: std::array::from_fn(|sample| {
                let phase = std::f64::consts::FRAC_PI_2 * (sample as f64 + 0.5) / CROSSLAP as f64;
                (std::f64::consts::FRAC_PI_2 * phase.sin().powi(2))
                    .sin()
                    .powi(2) as f32
            }),
            previous_mode: None,
            previous_bandwidth: None,
            previous_redundancy_end: false,
            loss_count: 0,
            final_range: 0,
        })
    }

    /// Decode a complete raw Opus packet and return samples per channel.
    /// Empty or one-byte compressed frames invoke the decoder's concealment.
    /// Trailing output capacity remains untouched.
    pub fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        let packet = Packet::parse(packet)?;
        let toc = packet.toc();
        let total = packet.samples(48_000)?;
        if output.len() < total * self.channels {
            return Err(Error::Output);
        }
        let samples = packet.frame_samples(48_000)?;
        for (index, bytes) in packet.frames().enumerate() {
            let begin = index * samples * self.channels;
            let destination = &mut output[begin..begin + samples * self.channels];
            if bytes.len() <= 1 {
                self.conceal(samples, destination);
                self.final_range = 0;
            } else {
                self.decode_frame(bytes, toc, samples)?;
                destination.copy_from_slice(&self.frame[..samples * self.channels]);
                self.loss_count = 0;
            }
            self.history[..CROSSLAP * self.channels]
                .copy_from_slice(&destination[(samples - CROSSLAP) * self.channels..]);
        }
        Ok(total)
    }

    fn conceal(&mut self, samples: usize, output: &mut [f32]) {
        self.loss_count = self.loss_count.saturating_add(1);
        let initial = 0.8f32.powi(self.loss_count.min(50) as i32);
        for (sample, destination) in output.chunks_exact_mut(self.channels).enumerate() {
            let attenuation = initial * (1.0 - sample as f32 / samples as f32);
            for (channel, value) in destination.iter_mut().enumerate() {
                *value = self.history[(sample % CROSSLAP) * self.channels + channel] * attenuation;
            }
        }
    }

    fn decode_frame(&mut self, bytes: &[u8], toc: Toc, samples: usize) -> Result<(), Error> {
        let mode = toc.mode();
        let bandwidth = toc.bandwidth();
        if mode == Mode::Celt {
            if self
                .previous_mode
                .is_some_and(|previous| previous != Mode::Celt)
                && !self.previous_redundancy_end
            {
                self.celt.reset();
            }
            let mut decoder = range::Decoder::new(bytes);
            self.celt.decode_layer(
                &mut decoder,
                CeltLayer {
                    samples,
                    coded_channels: usize::from(toc.channels()),
                    lm: toc.config() & 3,
                    start: 0,
                    end: celt_end(bandwidth),
                    packet_bytes: bytes.len(),
                },
                &mut self.frame[..samples * self.channels],
            )?;
            self.final_range = self.celt.final_range();
            self.previous_redundancy_end = false;
        } else {
            self.decode_lp_frame(bytes, toc, samples)?;
        }
        self.previous_mode = Some(mode);
        self.previous_bandwidth = Some(bandwidth);
        Ok(())
    }

    fn decode_lp_frame(&mut self, bytes: &[u8], toc: Toc, samples: usize) -> Result<(), Error> {
        let mode = toc.mode();
        let bandwidth = silk_bandwidth(toc.bandwidth());
        if self.previous_mode == Some(Mode::Celt) {
            self.silk.reset();
            self.internal_bandwidth = None;
        }
        if self.internal_bandwidth != Some(bandwidth) {
            for resampler in &mut self.resamplers {
                resampler.reset(bandwidth);
            }
            self.internal_bandwidth = Some(bandwidth);
        }
        let mut decoder = range::Decoder::new(bytes);
        let internal = self.silk.decode(
            &mut decoder,
            silk::Frame {
                bandwidth,
                duration: if samples == 480 {
                    silk::FrameDuration::Ms10
                } else {
                    silk::FrameDuration::Ms20
                },
            },
            silk_duration(samples)?,
            toc.channels(),
            &mut self.silk_pcm,
        )?;
        let ratio = self.resamplers[0].ratio();
        if internal * ratio != samples {
            return Err(Error::Corrupt);
        }
        let mut channel_pcm = [0.0; MAX_INTERNAL_SAMPLES];
        let coded_channels = usize::from(toc.channels());
        for channel in 0..self.channels {
            for (sample, destination) in channel_pcm[..internal].iter_mut().enumerate() {
                let begin = sample * coded_channels;
                *destination = if coded_channels == 1 {
                    self.silk_pcm[begin]
                } else if self.channels == 1 {
                    0.5 * (self.silk_pcm[begin] + self.silk_pcm[begin + 1])
                } else {
                    self.silk_pcm[begin + channel]
                };
            }
            self.resamplers[channel].process(
                &channel_pcm[..internal],
                &mut self.lowband[channel][..samples],
            )?;
        }
        for (sample, destination) in self.frame[..samples * self.channels]
            .chunks_exact_mut(self.channels)
            .enumerate()
        {
            for (channel, value) in destination.iter_mut().enumerate() {
                *value = self.lowband[channel][sample];
            }
        }
        let redundant = redundancy(&mut decoder, mode, bytes.len())?;
        let mut redundant_range = 0;
        if let Some(extra) = redundant.filter(|extra| extra.beginning) {
            redundant_range =
                self.decode_redundant(&bytes[extra.offset..], toc.bandwidth(), coded_channels)?;
        }
        let main_bytes = redundant.map_or(bytes.len(), |extra| extra.offset);
        if mode == Mode::Hybrid {
            // RFC 6716 Figure 18: beginning redundancy uses the preceding CELT
            // state, then CELT resets before the new Hybrid layer. Conversely,
            // an ending redundant frame from the preceding SILK packet prepares
            // the CELT history for an NB/MB -> Hybrid transition.
            let beginning = redundant.is_some_and(|extra| extra.beginning);
            let transition =
                self.previous_mode != Some(Mode::Hybrid) && !self.previous_redundancy_end;
            if beginning || transition {
                self.celt.reset();
            }
            self.celt.decode_layer(
                &mut decoder,
                CeltLayer {
                    samples,
                    coded_channels,
                    lm: if samples == 480 { 2 } else { 3 },
                    start: 17,
                    end: celt_end(toc.bandwidth()),
                    packet_bytes: main_bytes,
                },
                &mut self.lowband[0][..samples * self.channels],
            )?;
            for (sample, &highband) in self.frame[..samples * self.channels]
                .iter_mut()
                .zip(&self.lowband[0])
            {
                *sample += highband;
            }
        } else if self.previous_mode == Some(Mode::Hybrid) && toc.bandwidth() == Bandwidth::Wideband
        {
            // RFC 6716 §4.5: discharge the high-frequency MDCT overlap when
            // moving from Hybrid to WB SILK, without invoking concealment.
            let mut silence = range::Decoder::new(&[0xff, 0xfe]);
            self.celt.decode_layer(
                &mut silence,
                CeltLayer {
                    samples: CROSSLAP,
                    coded_channels,
                    lm: 0,
                    start: 0,
                    end: 21,
                    packet_bytes: 2,
                },
                &mut self.lowband[0][..CROSSLAP * self.channels],
            )?;
            for (sample, &tail) in self.frame[..CROSSLAP * self.channels]
                .iter_mut()
                .zip(&self.lowband[0])
            {
                *sample += tail;
            }
        }
        if let Some(extra) = redundant {
            if !extra.beginning {
                self.celt.reset();
                redundant_range =
                    self.decode_redundant(&bytes[extra.offset..], toc.bandwidth(), coded_channels)?;
            }
            self.crosslap(samples, extra.beginning);
        }
        self.previous_redundancy_end = redundant.is_some_and(|extra| !extra.beginning);
        self.final_range = decoder.range() ^ redundant_range;
        if self.frame[..samples * self.channels]
            .iter()
            .any(|sample| !sample.is_finite())
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    fn decode_redundant(
        &mut self,
        bytes: &[u8],
        bandwidth: Bandwidth,
        channels: usize,
    ) -> Result<u32, Error> {
        let mut decoder = range::Decoder::new(bytes);
        self.celt.decode_layer(
            &mut decoder,
            CeltLayer {
                samples: 2 * CROSSLAP,
                coded_channels: channels,
                lm: 1,
                start: 0,
                end: celt_end(bandwidth),
                packet_bytes: bytes.len(),
            },
            &mut self.redundant[..2 * CROSSLAP * self.channels],
        )?;
        Ok(self.celt.final_range())
    }

    fn crosslap(&mut self, samples: usize, beginning: bool) {
        if beginning {
            self.frame[..CROSSLAP * self.channels]
                .copy_from_slice(&self.redundant[..CROSSLAP * self.channels]);
        }
        let offset = if beginning {
            CROSSLAP
        } else {
            samples - CROSSLAP
        };
        for sample in 0..CROSSLAP {
            let mix = self.window[sample];
            for channel in 0..self.channels {
                let main = &mut self.frame[(offset + sample) * self.channels + channel];
                let redundant = self.redundant[(CROSSLAP + sample) * self.channels + channel];
                *main = if beginning {
                    (1.0 - mix) * redundant + mix * *main
                } else {
                    (1.0 - mix) * *main + mix * redundant
                };
            }
        }
    }

    /// Clear both layers, interpolation state, concealment and transition memory.
    pub fn reset(&mut self) {
        self.silk.reset();
        self.celt.reset();
        self.internal_bandwidth = None;
        self.history.fill(0.0);
        self.previous_mode = None;
        self.previous_bandwidth = None;
        self.previous_redundancy_end = false;
        self.loss_count = 0;
        self.final_range = 0;
    }

    /// Last decoded frame's interval XORed with its transition redundancy interval.
    pub const fn final_range(&self) -> u32 {
        self.final_range
    }
}

#[cfg(test)]
mod tests {
    use super::super::celt::{CeltConfig, CeltEncoder};
    use super::*;

    #[test]
    fn celt_packets_use_the_same_owned_layer_and_validate_before_history() {
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        let mut decoder = Decoder::new(1).unwrap();
        let mut direct = CeltDecoder::new(1).unwrap();
        let input = [0.1; 960];
        let mut packet = [0; 1276];
        let mut output = [0.0; 960];
        let mut expected = [0.0; 960];
        let length = encoder.encode(&input, &mut packet).unwrap();
        assert_eq!(
            decoder.decode(&packet[..length], &mut output[..10]),
            Err(Error::Output)
        );
        assert_eq!(decoder.final_range(), 0);
        decoder.decode(&packet[..length], &mut output).unwrap();
        direct.decode(&packet[..length], &mut expected).unwrap();
        assert_eq!(output, expected);
        for _ in 0..3 {
            let length = encoder.encode(&input, &mut packet).unwrap();
            decoder.decode(&packet[..length], &mut output).unwrap();
            direct.decode(&packet[..length], &mut expected).unwrap();
            assert_eq!(output, expected);
            assert_eq!(decoder.final_range(), direct.final_range());
        }
    }

    #[test]
    fn missing_frames_conceal_and_reset_is_silent() {
        let mut decoder = Decoder::new(1).unwrap();
        let mut output = [1.0; 960];
        assert_eq!(decoder.decode(&[0xf8], &mut output).unwrap(), 960);
        assert_eq!(output, [0.0; 960]);
        assert_eq!(decoder.final_range(), 0);
        decoder.reset();
        assert_eq!(decoder.decode(&[0], &mut output).unwrap(), 480);
        assert_eq!(&output[..480], &[0.0; 480]);
    }

    #[test]
    fn fixed_output_channels_map_mono_and_stereo_spectra() {
        let mut encoder = CeltEncoder::new(
            CeltConfig::new(
                64_000,
                celt::CeltFrameDuration::Ms20,
                2,
                Bandwidth::Fullband,
            )
            .unwrap(),
        );
        let mut mono = Decoder::new(1).unwrap();
        let mut stereo = Decoder::new(2).unwrap();
        let mut input = [0.0; 1920];
        let mut one = [0.0; 960];
        let mut two = [0.0; 1920];
        let mut packet = [0; 1276];
        for frame in 0..8 {
            for sample in 0..960 {
                let position = (sample + frame * 960) as f32;
                input[2 * sample] = 0.2 * (position * 0.031).sin();
                input[2 * sample + 1] = 0.15 * (position * 0.053).cos();
            }
            let length = encoder.encode(&input, &mut packet).unwrap();
            mono.decode(&packet[..length], &mut one).unwrap();
            stereo.decode(&packet[..length], &mut two).unwrap();
            assert_eq!(mono.final_range(), stereo.final_range());
            for (&actual, pair) in one.iter().zip(two.as_chunks::<2>().0) {
                assert!((actual - 0.5 * (pair[0] + pair[1])).abs() < 1e-6);
            }
        }
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        mono.reset();
        stereo.reset();
        for _ in 0..4 {
            let length = encoder.encode(&[0.1; 960], &mut packet).unwrap();
            mono.decode(&packet[..length], &mut one).unwrap();
            stereo.decode(&packet[..length], &mut two).unwrap();
            for (&actual, pair) in one.iter().zip(two.as_chunks::<2>().0) {
                assert_eq!(actual, pair[0]);
                assert_eq!(pair[0], pair[1]);
            }
        }
    }

    #[test]
    fn beginning_redundancy_cannot_leak_old_celt_history_into_hybrid() {
        // Independently generated 180/360/720 Hz synthetic tones encoded by
        // libopus 1.6.1: eight 20 ms music-biased frames at 128 kb/s, then one
        // voice-biased frame at 32 kb/s. This retained 80-byte transition has
        // beginning redundancy; its new Hybrid layer must start fresh.
        let hybrid = include_bytes!("testdata/celt-to-hybrid.opus");
        let mut first = Decoder::new(1).unwrap();
        let mut second = Decoder::new(1).unwrap();
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        let mut packet = [0; 1276];
        let mut output = [0.0; 960];
        let mut input = [0.0; 960];
        for (sample, value) in input.iter_mut().enumerate() {
            *value = 0.25 * (sample as f32 * 0.083).sin();
        }
        let length = encoder.encode(&input, &mut packet).unwrap();
        first.decode(&packet[..length], &mut output).unwrap();
        encoder.reset();
        for (sample, value) in input.iter_mut().enumerate() {
            *value = 0.15 * (sample as f32 * 0.059).cos();
        }
        let length = encoder.encode(&input, &mut packet).unwrap();
        second.decode(&packet[..length], &mut output).unwrap();
        first.decode(hybrid, &mut output).unwrap();
        second.decode(hybrid, &mut output).unwrap();
        assert!(
            first.redundant != second.redundant,
            "fixture must carry beginning redundancy"
        );
        // Cross-lapping may retain different old signals in the public PCM;
        // the newly reconstructed high-frequency layer must be identical.
        assert!(
            first.lowband[0][..960]
                .iter()
                .zip(&second.lowband[0][..960])
                .all(|(a, b)| a == b),
            "new Hybrid highband must be independent of the preceding redundant-frame history"
        );
        assert_eq!(first.final_range(), second.final_range());
    }
}
