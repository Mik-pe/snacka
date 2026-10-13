// SPDX-License-Identifier: MIT
//! Stateful 48 kHz CELT-only Opus packets, assembled from Snacka's owned kernels.

use std::fmt;

use super::allocation::{self, AllocationConfig, AllocationPreferences, BAND_EDGES, BANDS};
use super::bands::{self, CollapseFrame, ShapeFrame, ShapeSettings, ShapeWorkspace};
use super::energy::{self, BAND_MEANS, BandEnergies, EnergyState, FrameConfig};
use super::flags;
use super::packet::{Bandwidth, MAX_FRAME_BYTES, Mode, Packet, PacketError};
use super::range;
use super::transform::{Analysis, MAX_FRAME, PostFilterParams, Synthesis};

const RATE_DENOMINATOR: u64 = 48_000 * 8;
const MAX_PACKET_BYTES: usize = MAX_FRAME_BYTES + 1;

/// Invalid configuration, PCM, packet framing, or compressed CELT contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A bitrate, channel count, or bandwidth is outside the supported domain.
    Argument,
    /// Input PCM has the wrong length, non-finite samples, or values outside [-1, 1].
    Input,
    /// The caller's output slice cannot hold the complete result.
    Output,
    /// Ordinary Opus packet framing is invalid.
    Packet(PacketError),
    /// This codec accepts the CELT layer; the packet selects SILK or Hybrid.
    UnsupportedMode,
    /// Encoded channels differ from the decoder's configured channel count.
    Channels,
    /// Compressed contents or a coding operation violate a CELT invariant.
    Corrupt,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument => formatter.write_str("invalid CELT configuration"),
            Self::Input => formatter.write_str("invalid CELT input PCM"),
            Self::Output => formatter.write_str("CELT output buffer is too small"),
            Self::Packet(error) => error.fmt(formatter),
            Self::UnsupportedMode => formatter.write_str("packet requires SILK or Hybrid decoding"),
            Self::Channels => {
                formatter.write_str("Opus packet channels differ from decoder channels")
            }
            Self::Corrupt => formatter.write_str("invalid CELT coded contents"),
        }
    }
}

impl std::error::Error for Error {}

impl From<PacketError> for Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}

impl From<range::Error> for Error {
    fn from(_: range::Error) -> Self {
        Self::Corrupt
    }
}

impl From<energy::Error> for Error {
    fn from(_: energy::Error) -> Self {
        Self::Corrupt
    }
}

impl From<crate::transform::Error> for Error {
    fn from(_: crate::transform::Error) -> Self {
        Self::Corrupt
    }
}

/// The four CELT frame durations, at a fixed 48 kHz PCM sample rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeltFrameDuration {
    /// 120 samples per channel (2.5 ms).
    Ms2_5,
    /// 240 samples per channel (5 ms).
    Ms5,
    /// 480 samples per channel (10 ms).
    Ms10,
    /// 960 samples per channel (20 ms).
    Ms20,
}

impl CeltFrameDuration {
    /// PCM samples per channel in one frame at 48 kHz.
    pub const fn samples(self) -> usize {
        120 << self.lm()
    }

    /// Base-2 multiplier relative to the 120-sample transform.
    pub const fn lm(self) -> u8 {
        match self {
            Self::Ms2_5 => 0,
            Self::Ms5 => 1,
            Self::Ms10 => 2,
            Self::Ms20 => 3,
        }
    }
}

/// CELT encoder configuration. Bitrate includes the raw Opus TOC byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CeltConfig {
    bitrate: u32,
    duration: CeltFrameDuration,
    channels: usize,
    bandwidth: Bandwidth,
}

impl CeltConfig {
    /// Configure 48 kHz CELT. Bitrate is 6,000 through 512,000 bits/s, subject
    /// to frame duration: every packet must fit 3 through 1,276 bytes. CELT has
    /// Narrowband, Wideband, Superwideband and Fullband TOC configurations.
    pub fn new(
        bitrate: u32,
        duration: CeltFrameDuration,
        channels: usize,
        bandwidth: Bandwidth,
    ) -> Result<Self, Error> {
        let config = Self {
            bitrate,
            duration,
            channels,
            bandwidth,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(self) -> Result<(), Error> {
        let numerator = u64::from(self.bitrate) * self.duration.samples() as u64;
        let smallest = numerator / RATE_DENOMINATOR;
        let largest = numerator.div_ceil(RATE_DENOMINATOR);
        if !(6_000..=512_000).contains(&self.bitrate)
            || !(1..=2).contains(&self.channels)
            || self.bandwidth == Bandwidth::Mediumband
            || smallest < 3
            || largest > MAX_PACKET_BYTES as u64
        {
            return Err(Error::Argument);
        }
        Ok(())
    }

    /// Total encoded bitrate, including raw Opus framing.
    pub const fn bitrate(self) -> u32 {
        self.bitrate
    }
    /// Configured frame duration.
    pub const fn duration(self) -> CeltFrameDuration {
        self.duration
    }
    /// Interleaved PCM channel count, one or two.
    pub const fn channels(self) -> usize {
        self.channels
    }
    /// Encoded audio bandwidth.
    pub const fn bandwidth(self) -> Bandwidth {
        self.bandwidth
    }

    /// Change bitrate while preserving frame duration, channels and bandwidth.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), Error> {
        let updated = Self { bitrate, ..*self };
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    fn toc(self) -> u8 {
        let group = match self.bandwidth {
            Bandwidth::Narrowband => 0,
            Bandwidth::Wideband => 1,
            Bandwidth::Superwideband => 2,
            Bandwidth::Fullband => 3,
            Bandwidth::Mediumband => unreachable!("validated CELT bandwidth"),
        };
        ((16 + 4 * group + self.duration.lm()) << 3) | ((self.channels as u8 - 1) << 2)
    }
}

impl Default for CeltConfig {
    fn default() -> Self {
        Self {
            bitrate: 32_000,
            duration: CeltFrameDuration::Ms20,
            channels: 1,
            bandwidth: Bandwidth::Fullband,
        }
    }
}

fn end_band(bandwidth: Bandwidth) -> usize {
    match bandwidth {
        Bandwidth::Narrowband => 13,
        Bandwidth::Wideband => 17,
        Bandwidth::Superwideband => 19,
        Bandwidth::Fullband => 21,
        Bandwidth::Mediumband => unreachable!("CELT TOC has no mediumband"),
    }
}

/// CELT geometry within an enclosing Opus frame, after any SILK and redundancy
/// flags have consumed the shared arithmetic interval.
#[derive(Clone, Copy)]
pub(crate) struct CeltLayer {
    pub(crate) samples: usize,
    pub(crate) coded_channels: usize,
    pub(crate) lm: u8,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) packet_bytes: usize,
}

impl CeltLayer {
    fn validate(self) -> Result<(), Error> {
        if self.lm > 3
            || self.samples != 120 << self.lm
            || self.start >= self.end
            || self.end > BANDS
            || !(1..=2).contains(&self.coded_channels)
            || !(1..=MAX_FRAME_BYTES).contains(&self.packet_bytes)
        {
            return Err(Error::Argument);
        }
        Ok(())
    }
}

fn shape_frame<'a>(
    spectrum: &'a mut [[f32; MAX_FRAME]; 2],
    channels: usize,
    samples: usize,
    amplitudes: &'a BandEnergies,
    tf: &'a [i8; BANDS],
    allocation: &'a allocation::AllocationPlan,
) -> ShapeFrame<'a> {
    let [left, right] = spectrum;
    ShapeFrame {
        left: &mut left[..samples],
        right: (channels == 2).then_some(&mut right[..samples]),
        energies: amplitudes,
        tf,
        allocation,
    }
}

/// Stateful 48 kHz CELT encoder emitting ordinary single-frame raw Opus packets.
///
/// Construction prepares transform plans. Encoding uses caller-owned output and
/// fixed working buffers, with no processing allocations. Encoder PCM is
/// interleaved and normalized to [-1, 1]. Its transform delay is 120 samples.
pub struct CeltEncoder {
    config: CeltConfig,
    analysis: [Analysis; 2],
    pcm: [[f32; MAX_FRAME]; 2],
    spectrum: [[f32; MAX_FRAME]; 2],
    amplitudes: BandEnergies,
    energy: EnergyState,
    shapes: ShapeWorkspace,
    rate_remainder: u64,
    frames: u64,
    previous_coded_bands: usize,
    final_range: u32,
}

impl CeltEncoder {
    /// Prepare an encoder for a validated configuration.
    pub fn new(config: CeltConfig) -> Self {
        Self {
            config,
            analysis: std::array::from_fn(|_| Analysis::new()),
            pcm: [[0.0; MAX_FRAME]; 2],
            spectrum: [[0.0; MAX_FRAME]; 2],
            amplitudes: [[0.0; BANDS]; 2],
            energy: EnergyState::new(),
            shapes: ShapeWorkspace::new(),
            rate_remainder: 0,
            frames: 0,
            previous_coded_bands: end_band(config.bandwidth),
            final_range: 0,
        }
    }

    /// Encode exactly one frame and return the complete packet byte count.
    /// Invalid PCM or insufficient output capacity leaves stream history intact.
    /// If coding returns `Corrupt`, reset the encoder before continuing.
    pub fn encode(&mut self, input: &[f32], output: &mut [u8]) -> Result<usize, Error> {
        let samples = self.config.duration.samples();
        let channels = self.config.channels;
        if input.len() != samples * channels
            || input
                .iter()
                .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
        {
            return Err(Error::Input);
        }
        let numerator = self.rate_remainder + u64::from(self.config.bitrate) * samples as u64;
        let length = (numerator / RATE_DENOMINATOR) as usize;
        if output.len() < length {
            return Err(Error::Output);
        }
        let lm = self.config.duration.lm();
        let end = end_band(self.config.bandwidth);
        let bits = (length - 1) * 8;
        let eighth_bits = (length - 1) as i32 * 64;
        let energy_config = FrameConfig::new(0, end, channels, usize::from(lm), bits as u32)?;
        let allocation_config = AllocationConfig::new(0, end, lm, channels)?;
        let mut target = [[0.0; BANDS]; 2];
        self.amplitudes = [[0.0; BANDS]; 2];
        for channel in 0..channels {
            for (sample, destination) in self.pcm[channel][..samples].iter_mut().enumerate() {
                *destination = input[sample * channels + channel];
            }
            self.analysis[channel].forward(
                &self.pcm[channel][..samples],
                &mut self.spectrum[channel][..samples],
                false,
            )?;
            for band in 0..end {
                let begin = BAND_EDGES[band] << lm;
                let finish = BAND_EDGES[band + 1] << lm;
                let coefficients = &mut self.spectrum[channel][begin..finish];
                let amplitude = coefficients
                    .iter()
                    .map(|value| value * value)
                    .sum::<f32>()
                    .sqrt();
                self.amplitudes[channel][band] = amplitude;
                target[channel][band] = (amplitude * 32768.0).max(1e-15).log2() - BAND_MEANS[band];
                if amplitude > 0.0 {
                    for coefficient in coefficients {
                        *coefficient /= amplitude;
                    }
                }
            }
            self.spectrum[channel][BAND_EDGES[end] << lm..samples].fill(0.0);
        }
        output[0] = self.config.toc();
        let mut encoder = range::Encoder::new(&mut output[1..length]);
        flags::encode_header(&mut encoder, lm, bits, false)?;
        let intra =
            energy::encode_intra(&mut encoder, energy_config, self.frames.is_multiple_of(64))?;
        self.energy
            .encode_coarse(&mut encoder, energy_config, intra, &target)?;
        let tf = flags::encode_tf(
            &mut encoder,
            lm,
            false,
            0,
            end,
            bits,
            &flags::TfChoices {
                changes: [false; BANDS],
                selection: false,
            },
        )?;
        let spread = flags::encode_spread(&mut encoder, bits, 2)?;
        let side = allocation::encode_side_info(
            allocation_config,
            &mut encoder,
            eighth_bits,
            &[0; BANDS],
            5,
        )?;
        let budget = allocation::available_budget(
            allocation_config,
            eighth_bits,
            encoder.tell_frac(),
            false,
        )?;
        let plan = allocation::encode_allocation(
            allocation_config,
            &side,
            budget.remaining_eighth_bits,
            &mut encoder,
            AllocationPreferences {
                previous_coded_bands: self.previous_coded_bands,
                intensity: end,
                dual_stereo: false,
            },
        )?;
        self.energy
            .encode_fine(&mut encoder, energy_config, &plan.fine_energy_bits, &target)?;
        let settings = ShapeSettings {
            lm,
            start: 0,
            end,
            transient: false,
            spread,
            total_eighth_bits: eighth_bits,
        };
        bands::encode_shapes(
            &mut encoder,
            settings,
            shape_frame(
                &mut self.spectrum,
                channels,
                samples,
                &self.amplitudes,
                &tf,
                &plan,
            ),
            &mut self.shapes,
        )?;
        let remaining = (bits as u64).saturating_sub(encoder.tell()) as u32;
        self.energy.encode_final(
            &mut encoder,
            energy_config,
            &plan.fine_energy_bits,
            &plan.fine_priority,
            remaining,
            &target,
        )?;
        let final_range = encoder.range();
        encoder.finish_padded()?;
        self.final_range = final_range;
        self.shapes.seed = final_range;
        self.previous_coded_bands = plan.coded_bands;
        self.frames = self.frames.wrapping_add(1);
        self.rate_remainder = numerator % RATE_DENOMINATOR;
        Ok(length)
    }

    /// Clear prediction, transform overlap, fractional rate accounting and range.
    pub fn reset(&mut self) {
        for analysis in &mut self.analysis {
            analysis.reset();
        }
        self.energy.reset();
        self.shapes.seed = 0;
        self.rate_remainder = 0;
        self.frames = 0;
        self.previous_coded_bands = end_band(self.config.bandwidth);
        self.final_range = 0;
    }

    /// Update bitrate without discarding audio or prediction history.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), Error> {
        self.config.set_bitrate(bitrate)
    }

    /// Current encoder configuration.
    pub const fn config(&self) -> CeltConfig {
        self.config
    }
    /// Arithmetic interval at the end of the most recently encoded frame.
    pub const fn final_range(&self) -> u32 {
        self.final_range
    }
}

/// Stateful 48 kHz CELT-only Opus decoder with interleaved floating-point output.
///
/// Complete packet framing and output capacity are checked before stream history
/// changes. A packet can contain several frames; if a later compressed frame is
/// corrupt, earlier frames may already have advanced history. Reset after a
/// `Corrupt` result. Construction allocates transform plans; decoding allocates
/// nothing. SILK and Hybrid configurations return `UnsupportedMode`.
pub struct CeltDecoder {
    channels: usize,
    synthesis: [Synthesis; 2],
    pcm: [[f32; MAX_FRAME]; 2],
    spectrum: [[f32; MAX_FRAME]; 2],
    amplitudes: BandEnergies,
    energy: EnergyState,
    previous: BandEnergies,
    older: BandEnergies,
    shapes: ShapeWorkspace,
    final_range: u32,
}

impl CeltDecoder {
    /// Prepare a decoder for one or two output channels.
    pub fn new(channels: usize) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Argument);
        }
        Ok(Self {
            channels,
            synthesis: std::array::from_fn(|_| Synthesis::new()),
            pcm: [[0.0; MAX_FRAME]; 2],
            spectrum: [[0.0; MAX_FRAME]; 2],
            amplitudes: [[0.0; BANDS]; 2],
            energy: EnergyState::new(),
            previous: [[-28.0; BANDS]; 2],
            older: [[-28.0; BANDS]; 2],
            shapes: ShapeWorkspace::new(),
            final_range: 0,
        })
    }

    /// Decode a raw single-stream Opus packet and return samples per channel.
    /// The output slice must fit every frame, and trailing capacity is untouched.
    /// Empty compressed frames require concealment and return `Corrupt`.
    /// A compressed error can advance decoder history; reset after `Corrupt`
    /// before continuing with another packet.
    pub fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        let packet = Packet::parse(packet)?;
        if packet.toc().mode() != Mode::Celt {
            return Err(Error::UnsupportedMode);
        }
        if usize::from(packet.toc().channels()) != self.channels {
            return Err(Error::Channels);
        }
        let total = packet.samples(48_000)?;
        if output.len() < total * self.channels {
            return Err(Error::Output);
        }
        let samples = packet.frame_samples(48_000)?;
        let lm = packet.toc().config() & 3;
        let end = end_band(packet.toc().bandwidth());
        for (index, frame) in packet.frames().enumerate() {
            self.decode_frame(frame, samples, lm, end)?;
            let offset = index * samples * self.channels;
            for (sample, destination) in output[offset..offset + samples * self.channels]
                .chunks_exact_mut(self.channels)
                .enumerate()
            {
                for (channel, value) in destination.iter_mut().enumerate() {
                    *value = self.pcm[channel][sample];
                }
            }
        }
        Ok(total)
    }

    fn decode_frame(
        &mut self,
        frame: &[u8],
        samples: usize,
        lm: u8,
        end: usize,
    ) -> Result<(), Error> {
        if frame.is_empty() {
            return Err(Error::Corrupt);
        }
        let mut decoder = range::Decoder::new(frame);
        self.decode_layer_inner(
            &mut decoder,
            CeltLayer {
                samples,
                coded_channels: self.channels,
                lm,
                start: 0,
                end,
                packet_bytes: frame.len(),
            },
        )
    }

    /// Decode one CELT layer using an existing entropy state. The enclosing codec
    /// supplies the reduced packet size after excluding transition redundancy.
    pub(crate) fn decode_layer(
        &mut self,
        decoder: &mut range::Decoder<'_>,
        layer: CeltLayer,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        layer.validate()?;
        if output.len() < layer.samples * self.channels {
            return Err(Error::Output);
        }
        self.decode_layer_inner(decoder, layer)?;
        for (sample, destination) in output[..layer.samples * self.channels]
            .chunks_exact_mut(self.channels)
            .enumerate()
        {
            for (channel, value) in destination.iter_mut().enumerate() {
                *value = self.pcm[channel][sample];
            }
        }
        Ok(layer.samples)
    }

    fn decode_layer_inner(
        &mut self,
        decoder: &mut range::Decoder<'_>,
        layer: CeltLayer,
    ) -> Result<(), Error> {
        layer.validate()?;
        let CeltLayer {
            samples,
            coded_channels: channels,
            lm,
            start,
            end,
            packet_bytes,
        } = layer;
        let bits = packet_bytes * 8;
        let eighth_bits = packet_bytes as i32 * 64;
        let energy_config = FrameConfig::new(start, end, channels, usize::from(lm), bits as u32)?;
        let allocation_config = AllocationConfig::new(start, end, lm, channels)?;
        if channels == 1 {
            let [first, second] = &mut self.energy.energies;
            for (left, &right) in first.iter_mut().zip(second.iter()) {
                *left = left.max(right);
            }
        }
        let header = flags::decode_header_with_start(decoder, lm, start, bits)?;
        self.spectrum = [[0.0; MAX_FRAME]; 2];
        if header.silence {
            self.energy.energies = [[-28.0; BANDS]; 2];
        } else {
            let intra = energy::decode_intra(decoder, energy_config)?;
            self.energy.decode_coarse(decoder, energy_config, intra)?;
            let tf = flags::decode_tf(decoder, lm, header.transient, start, end, bits)?;
            let spread = flags::decode_spread(decoder, bits)?;
            let side = allocation::decode_side_info(allocation_config, decoder, eighth_bits)?;
            let budget = allocation::available_budget(
                allocation_config,
                eighth_bits,
                decoder.tell_frac(),
                header.transient,
            )?;
            let plan = allocation::decode_allocation(
                allocation_config,
                &side,
                budget.remaining_eighth_bits,
                decoder,
            )?;
            self.energy
                .decode_fine(decoder, energy_config, &plan.fine_energy_bits)?;
            self.amplitudes = [[1.0; BANDS]; 2];
            let settings = ShapeSettings {
                lm,
                start,
                end,
                transient: header.transient,
                spread,
                total_eighth_bits: eighth_bits - i32::from(budget.anti_collapse_reserved) * 8,
            };
            bands::decode_shapes(
                decoder,
                settings,
                shape_frame(
                    &mut self.spectrum,
                    channels,
                    samples,
                    &self.amplitudes,
                    &tf,
                    &plan,
                ),
                &mut self.shapes,
            )?;
            let anti_collapse = budget.anti_collapse_reserved && decoder.raw(1)? != 0;
            let remaining = (bits as u64).saturating_sub(decoder.tell()) as u32;
            self.energy.decode_final(
                decoder,
                energy_config,
                &plan.fine_energy_bits,
                &plan.fine_priority,
                remaining,
            )?;
            if anti_collapse {
                let [left, right] = &mut self.spectrum;
                bands::anti_collapse(
                    settings,
                    CollapseFrame {
                        left: &mut left[..samples],
                        right: (channels == 2).then_some(&mut right[..samples]),
                        current: &self.energy.energies,
                        previous: &self.previous,
                        older: &self.older,
                        allocation: &plan,
                    },
                    &self.shapes,
                )?;
            }
            for channel in 0..channels {
                for (band, &mean) in BAND_MEANS.iter().enumerate().take(end).skip(start) {
                    // Bound the numerical synthesis domain for arbitrary packet
                    // bytes without changing entropy or prediction decisions.
                    let amplitude = (self.energy.energies[channel][band] + mean)
                        .clamp(-60.0, 32.0)
                        .exp2()
                        / 32768.0;
                    let begin = BAND_EDGES[band] << lm;
                    let finish = BAND_EDGES[band + 1] << lm;
                    for coefficient in &mut self.spectrum[channel][begin..finish] {
                        *coefficient *= amplitude;
                    }
                }
            }
        }
        if decoder.tell() > bits as u64 {
            return Err(Error::Corrupt);
        }
        if channels == 1 {
            self.energy.energies[1] = self.energy.energies[0];
            if self.channels == 2 {
                self.spectrum[1] = self.spectrum[0];
            }
        } else if self.channels == 1 {
            let [first, second] = &mut self.spectrum;
            for (left, &right) in first[..samples].iter_mut().zip(&second[..samples]) {
                *left = 0.5 * (*left + right);
            }
        }
        for channel in 0..self.channels {
            self.synthesis[channel].inverse(
                &self.spectrum[channel][..samples],
                &mut self.pcm[channel][..samples],
                header.transient,
                if header.silence {
                    PostFilterParams::DISABLED
                } else {
                    header.filter
                },
            )?;
            if self.pcm[channel][..samples]
                .iter()
                .any(|sample| !sample.is_finite())
            {
                return Err(Error::Corrupt);
            }
        }
        if !header.transient {
            self.older = self.previous;
            self.previous = self.energy.energies;
        } else {
            for (previous, current) in self.previous.iter_mut().zip(&self.energy.energies) {
                for (before, &now) in previous.iter_mut().zip(current) {
                    *before = before.min(now);
                }
            }
        }
        for channel in 0..2 {
            self.energy.energies[channel][..start].fill(0.0);
            self.previous[channel][..start].fill(-28.0);
            self.older[channel][..start].fill(-28.0);
            self.energy.energies[channel][end..].fill(0.0);
            self.previous[channel][end..].fill(-28.0);
            self.older[channel][end..].fill(-28.0);
        }
        self.final_range = decoder.range();
        self.shapes.seed = self.final_range;
        Ok(())
    }

    /// Clear transform overlap, prediction histories, fold noise and final range.
    pub fn reset(&mut self) {
        for synthesis in &mut self.synthesis {
            synthesis.reset();
        }
        self.energy.reset();
        self.previous = [[-28.0; BANDS]; 2];
        self.older = [[-28.0; BANDS]; 2];
        self.shapes.seed = 0;
        self.final_range = 0;
    }

    /// Arithmetic interval of the last frame in the most recently decoded packet.
    pub const fn final_range(&self) -> u32 {
        self.final_range
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_frame_durations_channels_bandwidths_and_rates_roundtrip() {
        for duration in [
            CeltFrameDuration::Ms2_5,
            CeltFrameDuration::Ms5,
            CeltFrameDuration::Ms10,
            CeltFrameDuration::Ms20,
        ] {
            for channels in 1..=2 {
                for bandwidth in [
                    Bandwidth::Narrowband,
                    Bandwidth::Wideband,
                    Bandwidth::Superwideband,
                    Bandwidth::Fullband,
                ] {
                    for rate in [12_000, 32_000, 96_000] {
                        let config = CeltConfig::new(rate, duration, channels, bandwidth).unwrap();
                        let mut encoder = CeltEncoder::new(config);
                        let mut decoder = CeltDecoder::new(channels).unwrap();
                        let mut input = [0.0; 2 * MAX_FRAME];
                        let mut output = [0.0; 2 * MAX_FRAME];
                        let mut packet = [0; MAX_PACKET_BYTES];
                        for frame in 0..4 {
                            for sample in 0..duration.samples() {
                                for channel in 0..channels {
                                    let time =
                                        (sample + frame * duration.samples()) as f32 / 48_000.0;
                                    input[sample * channels + channel] = 0.2
                                        * (time
                                            * std::f32::consts::TAU
                                            * (440 + channel * 173) as f32)
                                            .sin();
                                }
                            }
                            let length = encoder
                                .encode(&input[..duration.samples() * channels], &mut packet)
                                .unwrap();
                            assert_eq!(
                                Packet::parse(&packet[..length]).unwrap().toc().bandwidth(),
                                bandwidth
                            );
                            assert_eq!(
                                decoder.decode(&packet[..length], &mut output).unwrap(),
                                duration.samples()
                            );
                            assert_eq!(
                                decoder.final_range(),
                                encoder.final_range(),
                                "duration={duration:?} channels={channels} bandwidth={bandwidth:?} rate={rate} frame={frame}"
                            );
                            assert!(
                                output[..duration.samples() * channels]
                                    .iter()
                                    .all(|sample| sample.is_finite())
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn validation_preserves_history_and_reset_reproduces_packets() {
        let config = CeltConfig::default();
        let input = [0.1; MAX_FRAME];
        let mut encoder = CeltEncoder::new(config);
        let mut packet = [0; MAX_PACKET_BYTES];
        assert_eq!(
            encoder.encode(&input[..100], &mut packet),
            Err(Error::Input)
        );
        assert_eq!(encoder.encode(&input, &mut packet[..3]), Err(Error::Output));
        let size = encoder.encode(&input, &mut packet).unwrap();
        let first = packet;
        let range = encoder.final_range();
        encoder.reset();
        assert_eq!(encoder.encode(&input, &mut packet).unwrap(), size);
        assert_eq!(packet[..size], first[..size]);
        assert_eq!(encoder.final_range(), range);
        let mut decoder = CeltDecoder::new(1).unwrap();
        let mut output = [0.0; MAX_FRAME];
        assert_eq!(
            decoder.decode(&[0], &mut output),
            Err(Error::UnsupportedMode)
        );
        assert_eq!(
            decoder.decode(&[0xfc, 0xff, 0xfe], &mut output),
            Err(Error::Channels)
        );
        assert_eq!(
            decoder.decode(&packet[..size], &mut output[..100]),
            Err(Error::Output)
        );
        assert_eq!(decoder.final_range(), 0);
        decoder.decode(&packet[..size], &mut output).unwrap();
        let first = output;
        decoder.reset();
        decoder.decode(&packet[..size], &mut output).unwrap();
        assert_eq!(output, first);
    }

    #[test]
    fn byte_rate_keeps_fractional_remainder_and_silence_decodes() {
        let config =
            CeltConfig::new(12_345, CeltFrameDuration::Ms5, 1, Bandwidth::Fullband).unwrap();
        let mut encoder = CeltEncoder::new(config);
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut total = 0;
        for _ in 0..200 {
            total += encoder.encode(&[0.0; 240], &mut packet).unwrap();
        }
        assert_eq!(total, 12_345 / 8);
        let mut decoder = CeltDecoder::new(1).unwrap();
        let mut output = [f32::NAN; MAX_FRAME];
        assert_eq!(
            decoder.decode(&[0xf8, 0xff, 0xfe], &mut output).unwrap(),
            960
        );
        assert_eq!(output, [0.0; MAX_FRAME]);
        assert!(CeltConfig::new(6_000, CeltFrameDuration::Ms2_5, 1, Bandwidth::Fullband).is_err());
        assert!(CeltConfig::new(512_000, CeltFrameDuration::Ms20, 1, Bandwidth::Fullband).is_err());
    }

    #[test]
    fn two_frame_packets_preserve_sequence_and_report_the_last_range() {
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        let input = [0.1; MAX_FRAME];
        let mut first = [0; MAX_PACKET_BYTES];
        let mut second = [0; MAX_PACKET_BYTES];
        let length = encoder.encode(&input, &mut first).unwrap();
        assert_eq!(encoder.encode(&input, &mut second).unwrap(), length);
        let second_range = encoder.final_range();
        let mut packet = [0; 2 * MAX_PACKET_BYTES];
        packet[0] = first[0] | 1;
        packet[1..length].copy_from_slice(&first[1..length]);
        packet[length..2 * length - 1].copy_from_slice(&second[1..length]);
        let mut sequential = CeltDecoder::new(1).unwrap();
        let mut expected = [0.0; 2 * MAX_FRAME];
        sequential
            .decode(&first[..length], &mut expected[..MAX_FRAME])
            .unwrap();
        sequential
            .decode(&second[..length], &mut expected[MAX_FRAME..])
            .unwrap();
        let mut batched = CeltDecoder::new(1).unwrap();
        let mut actual = [0.0; 2 * MAX_FRAME];
        assert_eq!(
            batched
                .decode(&packet[..2 * length - 1], &mut actual)
                .unwrap(),
            2 * MAX_FRAME
        );
        assert_eq!(actual, expected);
        assert_eq!(batched.final_range(), second_range);
    }
}
