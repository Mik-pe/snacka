// SPDX-License-Identifier: MIT
//! Stateful 48 kHz CELT-only Opus packets, assembled from Snacka's owned kernels.

use std::fmt;

use super::allocation::{self, AllocationConfig, AllocationPreferences, BAND_EDGES, BANDS};
use super::bands::{self, CollapseFrame, ShapeFrame, ShapeSettings, ShapeWorkspace};
use super::energy::{self, BAND_MEANS, BandEnergies, EnergyState, FrameConfig};
use super::flags;
use super::packet::{Bandwidth, MAX_FRAME_BYTES, Mode, Packet, PacketError};
use super::plc::{LOOKAHEAD as LOSS_LOOKAHEAD, LossPredictor};
use super::range;
use super::transform::{Analysis, MAX_FRAME, PostFilterParams, Synthesis};

const RATE_DENOMINATOR: u64 = 48_000 * 8;
const MAX_PACKET_BYTES: usize = MAX_FRAME_BYTES + 1;

fn loss_buffer<const N: usize>() -> Box<[f32; N]> {
    vec![0.0; N]
        .into_boxed_slice()
        .try_into()
        .expect("fixed loss history")
}

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

fn validate_encoder_pcm(input: &[f32], samples: usize, channels: usize) -> Result<(), Error> {
    if input.len() != samples * channels
        || input
            .iter()
            .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
    {
        Err(Error::Input)
    } else {
        Ok(())
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
        validate_encoder_pcm(input, samples, channels)?;
        let numerator = self.rate_remainder + u64::from(self.config.bitrate) * samples as u64;
        let length = (numerator / RATE_DENOMINATOR) as usize;
        if output.len() < length {
            return Err(Error::Output);
        }
        output[0] = self.config.toc();
        let mut encoder = range::Encoder::new(&mut output[1..length]);
        self.encode_layer_inner(
            input,
            &mut encoder,
            CeltLayer {
                samples,
                coded_channels: channels,
                lm: self.config.duration.lm(),
                start: 0,
                end: end_band(self.config.bandwidth),
                packet_bytes: length - 1,
            },
        )?;
        encoder.finish_padded()?;
        self.rate_remainder = numerator % RATE_DENOMINATOR;
        Ok(length)
    }

    /// Append a CELT layer to the enclosing codec's entropy interval.
    /// The caller supplies complete payload capacity and terminates the coder.
    pub(crate) fn encode_layer(
        &mut self,
        input: &[f32],
        encoder: &mut range::Encoder<'_>,
        layer: CeltLayer,
    ) -> Result<(), Error> {
        layer.validate()?;
        validate_encoder_pcm(input, layer.samples, layer.coded_channels)?;
        if layer.start != 0 && encoder.tell() <= 1 {
            return Err(Error::Argument);
        }
        self.encode_layer_inner(input, encoder, layer)
    }

    fn encode_layer_inner(
        &mut self,
        input: &[f32],
        encoder: &mut range::Encoder<'_>,
        layer: CeltLayer,
    ) -> Result<(), Error> {
        let CeltLayer {
            samples,
            coded_channels: channels,
            lm,
            start,
            end,
            packet_bytes,
        } = layer;
        let bits = packet_bytes * 8;
        if encoder.tell() >= bits as u64 {
            return Err(Error::Corrupt);
        }
        let eighth_bits = packet_bytes as i32 * 64;
        let energy_config = FrameConfig::new(start, end, channels, usize::from(lm), bits as u32)?;
        let allocation_config = AllocationConfig::new(start, end, lm, channels)?;
        if channels == 1 {
            let [first, second] = &mut self.energy.energies;
            for (left, &right) in first.iter_mut().zip(second.iter()) {
                *left = left.max(right);
            }
        }
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
            for band in start..end {
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
            self.spectrum[channel][..BAND_EDGES[start] << lm].fill(0.0);
            self.spectrum[channel][BAND_EDGES[end] << lm..samples].fill(0.0);
        }
        if channels == 1 {
            let [first, second] = &mut self.analysis;
            second.copy_history_from(first);
        }
        if start == 0 {
            flags::encode_header(encoder, lm, bits, false)?;
        } else if lm > 0 && encoder.tell() as usize + 3 <= bits {
            encoder.bit(false, 3)?;
        }
        let intra = energy::encode_intra(encoder, energy_config, self.frames.is_multiple_of(64))?;
        self.energy
            .encode_coarse(encoder, energy_config, intra, &target)?;
        let tf = flags::encode_tf(
            encoder,
            lm,
            false,
            start,
            end,
            bits,
            &flags::TfChoices {
                changes: [false; BANDS],
                selection: false,
            },
        )?;
        let spread = flags::encode_spread(encoder, bits, 2)?;
        let side =
            allocation::encode_side_info(allocation_config, encoder, eighth_bits, &[0; BANDS], 5)?;
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
            encoder,
            AllocationPreferences {
                previous_coded_bands: self.previous_coded_bands.clamp(start + 1, end),
                intensity: end,
                dual_stereo: false,
            },
        )?;
        self.energy
            .encode_fine(encoder, energy_config, &plan.fine_energy_bits, &target)?;
        let settings = ShapeSettings {
            lm,
            start,
            end,
            transient: false,
            spread,
            total_eighth_bits: eighth_bits,
        };
        bands::encode_shapes(
            encoder,
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
            encoder,
            energy_config,
            &plan.fine_energy_bits,
            &plan.fine_priority,
            remaining,
            &target,
        )?;
        self.final_range = encoder.range();
        self.shapes.seed = self.final_range;
        self.previous_coded_bands = plan.coded_bands;
        self.frames = self.frames.wrapping_add(1);
        if channels == 1 {
            self.energy.energies[1] = self.energy.energies[0];
        }
        for energies in &mut self.energy.energies {
            energies[..start].fill(0.0);
            energies[end..].fill(0.0);
        }
        Ok(())
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

    /// Encode an independent five-millisecond transition payload without TOC.
    /// The caller chooses its complete byte capacity and appends it to an LP
    /// frame, whose interval is XORed with the returned redundant interval.
    pub(crate) fn encode_redundant(
        &mut self,
        input: &[f32],
        channels: usize,
        bandwidth: Bandwidth,
        payload: &mut [u8],
    ) -> Result<u32, Error> {
        if !(1..=2).contains(&channels) || !(2..=MAX_FRAME_BYTES).contains(&payload.len()) {
            return Err(Error::Argument);
        }
        let bandwidth = if bandwidth == Bandwidth::Mediumband {
            Bandwidth::Wideband
        } else {
            bandwidth
        };
        let layer = CeltLayer {
            samples: 240,
            coded_channels: channels,
            lm: 1,
            start: 0,
            end: end_band(bandwidth),
            packet_bytes: payload.len(),
        };
        validate_encoder_pcm(input, layer.samples, channels)?;
        let mut encoder = range::Encoder::new(payload);
        self.encode_layer(input, &mut encoder, layer)?;
        let interval = encoder.range();
        encoder.finish_padded()?;
        Ok(interval)
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
    output_downsample: usize,
    synthesis: [Synthesis; 2],
    pcm: [[f32; MAX_FRAME]; 2],
    spectrum: [[f32; MAX_FRAME]; 2],
    amplitudes: BandEnergies,
    energy: EnergyState,
    previous: BandEnergies,
    older: BandEnergies,
    shapes: ShapeWorkspace,
    loss: LossPredictor,
    prediction: [Box<[f32; MAX_FRAME + LOSS_LOOKAHEAD]>; 2],
    last_start: usize,
    last_end: usize,
    final_range: u32,
}

impl CeltDecoder {
    /// Prepare a decoder for one or two output channels.
    pub fn new(channels: usize) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Argument);
        }
        let mut shapes = ShapeWorkspace::new();
        shapes.phase_inversion = channels == 2;
        Ok(Self {
            channels,
            output_downsample: 1,
            synthesis: std::array::from_fn(|_| Synthesis::new()),
            pcm: [[0.0; MAX_FRAME]; 2],
            spectrum: [[0.0; MAX_FRAME]; 2],
            amplitudes: [[0.0; BANDS]; 2],
            energy: EnergyState::new(),
            previous: [[-28.0; BANDS]; 2],
            older: [[-28.0; BANDS]; 2],
            shapes,
            loss: LossPredictor::new(),
            prediction: std::array::from_fn(|_| loss_buffer()),
            last_start: 0,
            last_end: BANDS,
            final_range: 0,
        })
    }

    /// Decode a raw single-stream Opus packet and return samples per channel.
    /// The output slice must fit every frame, and trailing capacity is untouched.
    /// Empty or one-byte compressed frames invoke packet-loss concealment.
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
            self.write_pcm(
                samples,
                &mut output[offset..offset + samples * self.channels],
            );
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
        if frame.len() <= 1 {
            return self.conceal_inner(samples, 0, end);
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
        self.write_pcm(layer.samples, output);
        Ok(layer.samples)
    }

    fn write_pcm(&self, samples: usize, output: &mut [f32]) {
        match self.channels {
            1 => output[..samples].copy_from_slice(&self.pcm[0][..samples]),
            2 => {
                for (destination, (&left, &right)) in output[..2 * samples]
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(self.pcm[0][..samples].iter().zip(&self.pcm[1][..samples]))
                {
                    *destination = [left, right];
                }
            }
            _ => unreachable!("validated output channel count"),
        }
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
        for spectrum in &mut self.spectrum[..channels] {
            spectrum[..samples].fill(0.0);
        }
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
                let [first, second] = &mut self.spectrum;
                second[..samples].copy_from_slice(&first[..samples]);
            }
        } else if self.channels == 1 {
            let [first, second] = &mut self.spectrum;
            for (left, &right) in first[..samples].iter_mut().zip(&second[..samples]) {
                *left = 0.5 * (*left + right);
            }
        }
        let zero_spectrum =
            header.silence || BAND_EDGES[start] << lm >= samples / self.output_downsample;
        for channel in 0..self.channels {
            // Discard frequencies that cannot be represented by the enclosing
            // decoder's output rate before sampling its synthesis waveform.
            self.spectrum[channel][samples / self.output_downsample..samples].fill(0.0);
            let filter = if header.silence {
                PostFilterParams::DISABLED
            } else {
                header.filter
            };
            if zero_spectrum {
                self.synthesis[channel].inverse_zero(&mut self.pcm[channel][..samples], filter)?;
            } else {
                self.synthesis[channel].inverse(
                    &self.spectrum[channel][..samples],
                    &mut self.pcm[channel][..samples],
                    header.transient,
                    filter,
                )?;
            }
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
        self.last_start = start;
        self.last_end = end;
        self.loss.observe(&self.pcm, samples, self.channels, true);
        Ok(())
    }

    /// Conceal a missing interval at 48 kHz and return samples per channel.
    /// The interval must be a positive 2.5 ms multiple, at most 120 ms.
    /// History advances just as it does when decoding an ordinary packet.
    pub fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, Error> {
        self.conceal_interval(samples, self.last_start, self.last_end, output)
    }

    pub(crate) fn conceal_interval(
        &mut self,
        samples: usize,
        start: usize,
        end: usize,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        if samples == 0 || samples > 5760 || !samples.is_multiple_of(120) {
            return Err(Error::Argument);
        }
        if start >= end || end > BANDS {
            return Err(Error::Argument);
        }
        if output.len() < samples * self.channels {
            return Err(Error::Output);
        }
        let mut completed = 0;
        while completed < samples {
            let remaining = samples - completed;
            let frame = [960, 480, 240, 120]
                .into_iter()
                .find(|&n| n <= remaining)
                .unwrap();
            self.conceal_layer(
                frame,
                start,
                end,
                &mut output[completed * self.channels..(completed + frame) * self.channels],
            )?;
            completed += frame;
        }
        Ok(samples)
    }

    pub(crate) fn set_output_downsample(&mut self, factor: usize) {
        debug_assert!([1, 2, 3, 4, 6].contains(&factor));
        self.output_downsample = factor;
    }

    /// Permit opposite phase in intensity-coded high-frequency stereo bands.
    /// The default is enabled for stereo output and disabled for mono output.
    /// Disabling phase inversion preserves high-frequency energy when downmixing.
    /// This changes synthesis policy without changing coded symbols or history.
    pub fn set_phase_inversion(&mut self, enabled: bool) {
        self.shapes.phase_inversion = enabled;
    }

    /// Whether intensity stereo can reconstruct opposite channel phases.
    pub const fn phase_inversion(&self) -> bool {
        self.shapes.phase_inversion
    }

    pub(crate) fn conceal_layer(
        &mut self,
        samples: usize,
        start: usize,
        end: usize,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        if ![120, 240, 480, 960].contains(&samples) || start >= end || end > BANDS {
            return Err(Error::Argument);
        }
        if output.len() < samples * self.channels {
            return Err(Error::Output);
        }
        self.conceal_inner(samples, start, end)?;
        self.write_pcm(samples, output);
        Ok(samples)
    }

    fn conceal_inner(&mut self, samples: usize, start: usize, end: usize) -> Result<(), Error> {
        let lm = (samples / 120).trailing_zeros();
        if BAND_EDGES[start] << lm >= samples / self.output_downsample {
            // No active CELT frequency can reach this output rate. Discharge
            // prior overlap without inventing audible out-of-band PLC noise.
            for channel in 0..self.channels {
                self.synthesis[channel].inverse_zero(
                    &mut self.pcm[channel][..samples],
                    PostFilterParams::DISABLED,
                )?;
            }
        } else {
            self.loss
                .predict(samples, self.channels, start == 0, &mut self.prediction);
            for channel in 0..self.channels {
                self.synthesis[channel].conceal(
                    &self.prediction[channel][..samples + LOSS_LOOKAHEAD],
                    &mut self.pcm[channel][..samples],
                )?;
            }
        }
        self.loss.observe(&self.pcm, samples, self.channels, false);
        if self.loss.periodicity() < 0.6 || self.loss.elapsed() > 1920 {
            for energies in &mut self.energy.energies {
                for energy in &mut energies[start..end] {
                    *energy = (*energy - 0.3 * samples as f32 / 960.0).max(-28.0);
                }
            }
        }
        self.last_start = start;
        self.last_end = end;
        self.final_range = 0;
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
        self.loss.reset();
        self.last_start = 0;
        self.last_end = BANDS;
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
    fn inaudible_hybrid_losses_discharge_overlap_without_new_noise() {
        for downsample in [3, 4, 6] {
            for channels in 1..=2 {
                let mut encoder = CeltEncoder::new(
                    CeltConfig::new(
                        96_000,
                        CeltFrameDuration::Ms5,
                        channels,
                        Bandwidth::Narrowband,
                    )
                    .unwrap(),
                );
                let mut actual = CeltDecoder::new(channels).unwrap();
                let mut expected = CeltDecoder::new(channels).unwrap();
                actual.set_output_downsample(downsample);
                expected.set_output_downsample(downsample);
                let mut input = [0.0; 480];
                let mut packet = [0; 1276];
                let mut actual_pcm = [0.0; 1920];
                let mut expected_pcm = [0.0; 1920];
                for frame in 0..4 {
                    for sample in 0..240 {
                        for channel in 0..channels {
                            input[sample * channels + channel] = 0.2
                                * ((frame * 240 + sample) as f32
                                    * std::f32::consts::TAU
                                    * (180.0 + channel as f32 * 93.0)
                                    / 48_000.0)
                                    .sin();
                        }
                    }
                    let length = encoder
                        .encode(&input[..240 * channels], &mut packet)
                        .unwrap();
                    actual.decode(&packet[..length], &mut actual_pcm).unwrap();
                    expected
                        .decode(&packet[..length], &mut expected_pcm)
                        .unwrap();
                }
                let mut elapsed = 0;
                for (lm, samples) in [(0, 120), (1, 240), (2, 480), (3, 960)] {
                    actual
                        .conceal_layer(samples, 17, 21, &mut actual_pcm)
                        .unwrap();
                    let mut silence = range::Decoder::new(&[0xff, 0xfe]);
                    expected
                        .decode_layer(
                            &mut silence,
                            CeltLayer {
                                samples,
                                coded_channels: channels,
                                lm,
                                start: 0,
                                end: 21,
                                packet_bytes: 2,
                            },
                            &mut expected_pcm,
                        )
                        .unwrap();
                    assert_eq!(
                        actual_pcm[..samples * channels],
                        expected_pcm[..samples * channels]
                    );
                    elapsed += samples;
                    assert_eq!(actual.loss.elapsed(), elapsed);
                    assert_eq!(actual.final_range(), 0);
                }
            }
        }
    }

    #[test]
    fn inactive_spectrum_storage_cannot_leak_across_geometry_or_channel_changes() {
        let mut control = CeltDecoder::new(2).unwrap();
        let mut poisoned = CeltDecoder::new(2).unwrap();
        let durations = [
            CeltFrameDuration::Ms20,
            CeltFrameDuration::Ms2_5,
            CeltFrameDuration::Ms10,
            CeltFrameDuration::Ms5,
        ];
        let bands = [
            Bandwidth::Fullband,
            Bandwidth::Narrowband,
            Bandwidth::Wideband,
            Bandwidth::Superwideband,
        ];
        let mut input = [0.0; 1920];
        let mut expected = [0.0; 1920];
        let mut actual = [0.0; 1920];
        let mut packet = [0; 1276];
        for transition in 0..32 {
            let duration = durations[transition % 4];
            let channels = transition / 2 % 2 + 1;
            let bandwidth = bands[(transition / 3) % 4];
            let config = CeltConfig::new(32_000, duration, channels, bandwidth).unwrap();
            let mut encoder = CeltEncoder::new(config);
            let samples = duration.samples();
            for (sample, value) in input[..samples * channels].iter_mut().enumerate() {
                *value = 0.2 * (sample as f32 * 0.023 + transition as f32 * 0.1).sin();
            }
            let length = encoder
                .encode(&input[..samples * channels], &mut packet)
                .unwrap();
            let payload = &packet[1..length];
            let layer = CeltLayer {
                samples,
                coded_channels: channels,
                lm: duration.lm(),
                start: 0,
                end: end_band(bandwidth),
                packet_bytes: payload.len(),
            };
            for spectrum in &mut poisoned.spectrum {
                spectrum.fill(f32::NAN);
            }
            control
                .decode_layer(&mut range::Decoder::new(payload), layer, &mut expected)
                .unwrap();
            poisoned
                .decode_layer(&mut range::Decoder::new(payload), layer, &mut actual)
                .unwrap();
            assert_eq!(&actual[..samples * 2], &expected[..samples * 2]);
            assert!(
                actual[..samples * 2]
                    .iter()
                    .all(|sample| sample.is_finite())
            );
            assert_eq!(poisoned.final_range(), control.final_range());
        }
    }

    #[test]
    fn hybrid_layers_append_to_an_existing_interval_and_preserve_energy_history() {
        for channels in 1..=2 {
            for lm in 2..=3 {
                for end in [19, 21] {
                    let samples = 120 << lm;
                    let mut encoder = CeltEncoder::new(CeltConfig::default());
                    let mut decoder = CeltDecoder::new(channels).unwrap();
                    let mut input = [0.0; 1920];
                    let mut output = [0.0; 1920];
                    let mut packet = [0; 160];
                    let layer = CeltLayer {
                        samples,
                        coded_channels: channels,
                        lm,
                        start: 17,
                        end,
                        packet_bytes: packet.len(),
                    };
                    for frame in 0..4 {
                        for sample in 0..samples {
                            for channel in 0..channels {
                                let time = (frame * samples + sample) as f32 / 48_000.0;
                                input[sample * channels + channel] = 0.2
                                    * (std::f32::consts::TAU
                                        * (10_200 + channel * 500) as f32
                                        * time)
                                        .sin();
                            }
                        }
                        let mut enc = range::Encoder::new(&mut packet);
                        enc.uniform(100 + frame as u32, 1000).unwrap();
                        encoder
                            .encode_layer(&input[..samples * channels], &mut enc, layer)
                            .unwrap();
                        let interval = enc.range();
                        enc.finish_padded().unwrap();
                        let mut dec = range::Decoder::new(&packet);
                        assert_eq!(dec.uniform(1000).unwrap(), 100 + frame as u32);
                        decoder.decode_layer(&mut dec, layer, &mut output).unwrap();
                        assert_eq!(dec.range(), interval);
                        assert_eq!(decoder.final_range(), encoder.final_range());
                        for channel in 0..channels {
                            for band in 17..end {
                                assert!(
                                    (encoder.energy.energies[channel][band]
                                        - decoder.energy.energies[channel][band])
                                        .abs()
                                        < 1e-5
                                );
                            }
                        }
                        assert!(
                            output[..samples * channels]
                                .iter()
                                .all(|sample| sample.is_finite())
                        );
                    }
                }
            }
        }
    }

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
