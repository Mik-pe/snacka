// SPDX-License-Identifier: MIT
//! Owned ordinary Opus encoding across SILK, Hybrid and CELT.

use std::fmt;

use super::celt::{CeltConfig, CeltEncoder, CeltFrameDuration, CeltLayer};
use super::dtx;
use super::encoder_transition::{self, Position, Transition};
use super::packet::{Bandwidth, Mode};
use super::range;
use super::repacketize::Repacketizer;
use super::silk::{self, Downsampler, SilkEncoder};

const MAX_SAMPLES: usize = 2880;
const MAX_NATIVE_SAMPLES: usize = 960;
const MAX_FRAME_BYTES: usize = 1275;
const MAX_FRAMES: usize = 6;

/// Encoding intent used when the configuration selects its mode automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Application {
    /// Speech, preferring SILK or Hybrid at ordinary voice frame durations.
    Voice,
    /// General audio, preferring transform coding.
    Audio,
    /// CELT transform coding with 2.5/5/10/20 ms frame choices.
    LowDelay,
}

/// Packet duration at the public 48 kHz PCM boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDuration {
    /// 120 samples per channel.
    Ms2_5,
    /// 240 samples per channel.
    Ms5,
    /// 480 samples per channel.
    Ms10,
    /// 960 samples per channel.
    Ms20,
    /// 1920 samples per channel.
    Ms40,
    /// 2880 samples per channel.
    Ms60,
    /// 3840 samples per channel, composed from four 20 ms frames.
    Ms80,
    /// 4800 samples per channel, composed from five 20 ms frames.
    Ms100,
    /// 5760 samples per channel, composed from six 20 ms frames.
    Ms120,
}

impl FrameDuration {
    /// Samples per channel required at 48 kHz.
    pub const fn samples(self) -> usize {
        match self {
            Self::Ms2_5 => 120,
            Self::Ms5 => 240,
            Self::Ms10 => 480,
            Self::Ms20 => 960,
            Self::Ms40 => 1920,
            Self::Ms60 => 2880,
            Self::Ms80 => 3840,
            Self::Ms100 => 4800,
            Self::Ms120 => 5760,
        }
    }
}

/// Packet rate policy independent of the standard codec's wire models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Pad complete packets to the configured fractional bitrate clock.
    Constant,
    /// Compact coded payloads within the configured packet bitrate cap.
    Variable,
}

/// Validated ordinary Opus encoder configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    bitrate: u32,
    channels: usize,
    duration: FrameDuration,
    bandwidth: Bandwidth,
    application: Application,
    mode: Option<Mode>,
    rate: RateControl,
    fec: bool,
    dtx: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate: 32_000,
            channels: 1,
            duration: FrameDuration::Ms20,
            bandwidth: Bandwidth::Wideband,
            application: Application::Voice,
            mode: None,
            rate: RateControl::Variable,
            fec: false,
            dtx: false,
        }
    }
}

impl EncoderConfig {
    /// Configure normalized 48 kHz PCM with one or two channels.
    pub fn new(
        bitrate: u32,
        channels: usize,
        duration: FrameDuration,
        bandwidth: Bandwidth,
        application: Application,
    ) -> Result<Self, Error> {
        let result = Self {
            bitrate,
            channels,
            duration,
            bandwidth,
            application,
            ..Self::default()
        };
        result.validate()?;
        Ok(result)
    }

    /// Select a specific wire mode instead of the application preference.
    pub fn with_mode(self, mode: Mode) -> Result<Self, Error> {
        let result = Self {
            mode: Some(mode),
            ..self
        };
        result.validate()?;
        Ok(result)
    }

    /// Choose compact variable packets or full-packet constant-rate padding.
    pub const fn with_rate_control(self, rate: RateControl) -> Self {
        Self { rate, ..self }
    }

    /// Enable previous-packet SILK redundancy when the bitrate budget permits.
    /// CELT-only packets have no SILK redundancy layer.
    pub fn with_fec(self, enabled: bool) -> Result<Self, Error> {
        let result = Self {
            fec: enabled,
            ..self
        };
        result.validate()?;
        Ok(result)
    }

    /// Suppress continuing quiet audio after a 200 ms hangover.
    /// A real inactive refresh is encoded at least every 400 ms. Suppressed
    /// intervals emit a compact empty frame even under constant rate policy.
    pub fn with_dtx(self, enabled: bool) -> Result<Self, Error> {
        let result = Self {
            dtx: enabled,
            ..self
        };
        result.validate()?;
        Ok(result)
    }

    /// Requested packet bitrate including framing and any padding.
    pub const fn bitrate(self) -> u32 {
        self.bitrate
    }
    /// Number of interleaved channels.
    pub const fn channels(self) -> usize {
        self.channels
    }
    /// Packet duration.
    pub const fn duration(self) -> FrameDuration {
        self.duration
    }
    /// Encoded audio bandwidth.
    pub const fn bandwidth(self) -> Bandwidth {
        self.bandwidth
    }
    /// Current rate policy.
    pub const fn rate_control(self) -> RateControl {
        self.rate
    }
    /// Whether previous-packet redundancy is requested.
    pub const fn fec(self) -> bool {
        self.fec
    }
    /// Whether quiet intervals use discontinuous transmission.
    pub const fn dtx(self) -> bool {
        self.dtx
    }
    /// Resolved standard Opus wire mode.
    pub const fn mode(self) -> Mode {
        if let Some(mode) = self.mode {
            return mode;
        }
        if self.duration.samples() < 480
            || matches!(self.application, Application::Audio | Application::LowDelay)
        {
            if matches!(self.bandwidth, Bandwidth::Mediumband) {
                Mode::Silk
            } else {
                Mode::Celt
            }
        } else if matches!(
            self.bandwidth,
            Bandwidth::Superwideband | Bandwidth::Fullband
        ) {
            Mode::Hybrid
        } else {
            Mode::Silk
        }
    }

    /// Output capacity covering the next packet's bitrate clock ceiling.
    pub const fn max_packet_bytes(self) -> usize {
        (self.bitrate as usize * self.duration.samples()).div_ceil(48_000 * 8)
    }

    fn frame_count(self) -> usize {
        if self.mode() == Mode::Silk && self.duration.samples() <= MAX_SAMPLES {
            1
        } else {
            self.duration.samples().div_ceil(960)
        }
    }

    fn validate(self) -> Result<(), Error> {
        let samples = self.duration.samples();
        let mode = self.mode();
        let invalid_band = match mode {
            Mode::Silk => matches!(
                self.bandwidth,
                Bandwidth::Superwideband | Bandwidth::Fullband
            ),
            Mode::Hybrid => !matches!(
                self.bandwidth,
                Bandwidth::Superwideband | Bandwidth::Fullband
            ),
            Mode::Celt => self.bandwidth == Bandwidth::Mediumband,
        };
        let frames = self.frame_count();
        let overhead = if frames == 1 { 1 } else { 2 + 2 * (frames - 1) };
        let minimum_bytes = u64::from(self.bitrate) * samples as u64 / (48_000 * 8);
        if !(1..=2).contains(&self.channels)
            || !(6000..=510_000).contains(&self.bitrate)
            || (mode != Mode::Celt && samples < 480)
            || invalid_band
            || minimum_bytes < (overhead + 2 * frames) as u64
            || (self.fec && mode == Mode::Celt)
        {
            return Err(Error::Argument);
        }
        Ok(())
    }
}

/// Invalid encoder configuration, input, capacity or entropy contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A configuration or mode/bandwidth combination is outside its domain.
    Argument,
    /// PCM length, finite values or normalized range are invalid.
    Input,
    /// Output or the packet's coded bitrate budget is insufficient.
    Output,
    /// A codec layer or packet writer encountered an internal coding failure.
    Coding,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Argument => "invalid Opus encoder configuration",
            Self::Input => "invalid Opus input PCM",
            Self::Output => "Opus output or bitrate budget is too small",
            Self::Coding => "Opus layer encoding failed",
        })
    }
}
impl std::error::Error for Error {}

impl From<crate::entropy::Error> for Error {
    fn from(error: crate::entropy::Error) -> Self {
        match error {
            crate::entropy::Error::Output => Self::Output,
            crate::entropy::Error::Argument => Self::Argument,
            _ => Self::Coding,
        }
    }
}
impl From<super::celt::Error> for Error {
    fn from(error: super::celt::Error) -> Self {
        match error {
            super::celt::Error::Output => Self::Output,
            super::celt::Error::Input => Self::Input,
            super::celt::Error::Argument => Self::Argument,
            _ => Self::Coding,
        }
    }
}

/// Stateful owned 48 kHz encoder for SILK, Hybrid and CELT packets.
///
/// Construction prepares every transform plan and rate-conversion state.
/// Processing uses fixed workspaces and caller-owned output. SILK carries
/// native-rate spectral prediction and excitation; Hybrid appends the CELT
/// highband into its shared entropy interval. These APIs select codec behavior
/// directly and do not depend on an external codec implementation.
pub struct Encoder {
    config: EncoderConfig,
    celt: Box<CeltEncoder>,
    silk: Box<SilkEncoder>,
    downsamplers: [Downsampler; 2],
    input_bandwidth: Option<silk::Bandwidth>,
    previous_mode: Option<Mode>,
    previous_bandwidth: Option<Bandwidth>,
    channel_pcm: [[f32; MAX_SAMPLES]; 2],
    native_pcm: [f32; MAX_NATIVE_SAMPLES * 2],
    frames: [[u8; MAX_FRAME_BYTES + 1]; MAX_FRAMES],
    rate_remainder: u64,
    final_range: u32,
    dtx_state: dtx::State,
    last_dtx: bool,
    has_fec: bool,
    pending_config: Option<EncoderConfig>,
    previous_redundancy_end: bool,
    transition_redundancy: bool,
}

impl Encoder {
    /// Prepare a fully validated configuration and owned working memory.
    pub fn new(config: EncoderConfig) -> Result<Self, Error> {
        config.validate()?;
        let celt_config = CeltConfig::new(
            32_000,
            CeltFrameDuration::Ms20,
            config.channels,
            Bandwidth::Fullband,
        )?;
        let mut silk = SilkEncoder::new();
        silk.set_fec(config.fec);
        Ok(Self {
            config,
            celt: Box::new(CeltEncoder::new(celt_config)),
            silk: Box::new(silk),
            downsamplers: std::array::from_fn(|_| Downsampler::new(silk::Bandwidth::Wide)),
            input_bandwidth: None,
            previous_mode: None,
            previous_bandwidth: None,
            channel_pcm: [[0.0; MAX_SAMPLES]; 2],
            native_pcm: [0.0; MAX_NATIVE_SAMPLES * 2],
            frames: [[0; MAX_FRAME_BYTES + 1]; MAX_FRAMES],
            rate_remainder: 0,
            final_range: 0,
            dtx_state: dtx::State::new(),
            last_dtx: false,
            has_fec: false,
            pending_config: None,
            previous_redundancy_end: false,
            transition_redundancy: false,
        })
    }

    /// Current validated configuration.
    pub const fn config(&self) -> EncoderConfig {
        self.config
    }
    /// Arithmetic interval of the last coded frame in the previous packet.
    pub const fn final_range(&self) -> u32 {
        self.final_range
    }
    /// Whether the previous packet's first frame carried previous-audio LBRR.
    /// For packets composed from several frames this recovers the lost tail;
    /// earlier missing audio is concealed by [`super::Decoder::recover`].
    pub const fn has_fec(&self) -> bool {
        self.has_fec
    }
    /// Whether the last packet carries a five-millisecond transition frame.
    pub const fn has_transition_redundancy(&self) -> bool {
        self.transition_redundancy
    }
    /// Nominal input-to-decoded delay, in samples at the 48 kHz PCM boundary.
    /// CELT's transform delay is exactly 120 samples. SILK's own rate filters
    /// have the same low-frequency group delay; their phase varies with frequency.
    pub const fn algorithmic_delay_samples(&self) -> usize {
        120
    }
    /// Whether the previous packet represents a suppressed quiet interval.
    /// A transport can omit this empty packet while advancing its timestamp.
    /// Configuration changes preserve this last-packet outcome.
    pub const fn was_dtx(&self) -> bool {
        self.last_dtx
    }

    /// Reset prediction, transform, rate conversion and bitrate-clock history.
    pub fn reset(&mut self) {
        self.celt.reset();
        self.silk.reset();
        self.input_bandwidth = None;
        self.previous_mode = None;
        self.previous_bandwidth = None;
        self.rate_remainder = 0;
        self.final_range = 0;
        self.dtx_state.reset();
        self.last_dtx = false;
        self.has_fec = false;
        self.pending_config = None;
        self.previous_redundancy_end = false;
        self.transition_redundancy = false;
    }

    /// Apply an already validated configuration without allocating.
    ///
    /// Bitrate and duration changes preserve prediction and transform history.
    /// A changed channel layout restarts input filtering; a changed SILK rate
    /// restarts its rate-specific predictor on the next packet. Mode changes
    /// initialize the newly active codec layer when that packet is encoded.
    /// The fractional byte clock restarts when its rate or duration changes.
    pub fn reconfigure(&mut self, config: EncoderConfig) -> Result<(), Error> {
        config.validate()?;
        if config.channels != self.config.channels {
            self.input_bandwidth = None;
        }
        if config.bitrate != self.config.bitrate || config.duration != self.config.duration {
            self.rate_remainder = 0;
        }
        if config.dtx != self.config.dtx {
            self.dtx_state.reset();
        }
        self.silk.set_fec(config.fec);
        self.config = config;
        self.pending_config = None;
        Ok(())
    }

    /// Queue a configuration change after the next successfully encoded packet.
    ///
    /// That packet still takes the current configuration's PCM geometry. An
    /// LP-to-CELT switch or a change of SILK rate adds an end transition frame
    /// when its budget permits, retaining CELT history for the following packet. Immediate changes
    /// can instead use [`Self::reconfigure`].
    pub fn prepare_reconfigure(&mut self, config: EncoderConfig) -> Result<(), Error> {
        config.validate()?;
        self.pending_config = Some(config);
        Ok(())
    }

    /// Encode exactly one configured packet from normalized interleaved PCM.
    /// Framing capacity and input validity are checked before changing history.
    /// A layer coding failure may advance history; reset before continuing.
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
        let target_bytes = (numerator / (48_000 * 8)) as usize;
        if output.len() < target_bytes {
            return Err(Error::Output);
        }
        let mode = self.config.mode();
        let count = self.config.frame_count();
        let frame_samples = samples / count;
        let overhead = if count == 1 { 1 } else { 2 + 2 * (count - 1) };
        if target_bytes <= overhead + count {
            return Err(Error::Output);
        }
        let payload_bytes = ((target_bytes - overhead) / count).min(MAX_FRAME_BYTES);
        let rate_changed = self.previous_bandwidth.is_some_and(|bandwidth| {
            silk_bandwidth(bandwidth) != silk_bandwidth(self.config.bandwidth)
        });
        let beginning = mode != Mode::Celt
            && (self.previous_mode == Some(Mode::Celt)
                || (mode == Mode::Silk && self.previous_redundancy_end && rate_changed));
        let ending = mode != Mode::Celt
            && self.pending_config.is_some_and(|config| {
                config.mode() == Mode::Celt
                    || silk_bandwidth(config.bandwidth) != silk_bandwidth(self.config.bandwidth)
            });
        let position = if payload_bytes >= 10 {
            if beginning {
                Some(Position::Beginning)
            } else if ending {
                Some(Position::End)
            } else {
                None
            }
        } else {
            None
        };
        // Reconfiguration may restart the admission clock while the receiver
        // still has a concealed interval since our last emitted coded frame.
        let previously_suppressed = self.dtx_state.suppressed() || self.last_dtx;
        let suppress =
            self.config.dtx && self.dtx_state.decide(input, samples) == dtx::Decision::Suppress;
        if previously_suppressed && !suppress {
            // The receiver has concealed omitted intervals. Restart the
            // encoder's codec history before the next real refresh.
            self.silk.reset();
            self.celt.reset();
        }
        if self.previous_mode != Some(mode) {
            if (mode == Mode::Celt && !self.previous_redundancy_end)
                || (mode == Mode::Hybrid
                    && position != Some(Position::Beginning)
                    && !self.previous_redundancy_end)
            {
                self.celt.reset();
            }
            if self.previous_mode == Some(Mode::Celt) {
                self.silk.reset();
                self.input_bandwidth = None;
            }
        }
        let mut sizes = [0; MAX_FRAMES];
        let mut interval = 0;
        let mut has_fec = false;
        let mut transition_redundancy = false;
        let mut redundant = [0; 257];
        for (index, size) in sizes.iter_mut().enumerate().take(count) {
            let begin = index * frame_samples * channels;
            let pcm = &input[begin..begin + frame_samples * channels];
            let toc = toc(mode, self.config.bandwidth, frame_samples, channels)?;
            self.frames[index][0] = toc;
            let internal = if mode != Mode::Celt {
                self.prepare_native(pcm, frame_samples, channels)?
            } else {
                0
            };
            if suppress {
                *size = 1;
                continue;
            }
            let transition = position
                .filter(|position| match position {
                    Position::Beginning => index == 0,
                    Position::End => index + 1 == count,
                })
                .map(|position| Transition {
                    position,
                    bytes: (payload_bytes / 4).clamp(2, 257),
                });
            let main_bytes = payload_bytes - transition.map_or(0, |transition| transition.bytes);
            let mut redundant_interval = 0;
            if let Some(transition) =
                transition.filter(|transition| transition.position == Position::Beginning)
            {
                redundant_interval = self.celt.encode_redundant(
                    &pcm[..240 * channels],
                    channels,
                    self.config.bandwidth,
                    &mut redundant[..transition.bytes],
                )?;
                if mode == Mode::Hybrid {
                    self.celt.reset();
                }
            }
            let mut range = range::Encoder::new(&mut self.frames[index][1..1 + main_bytes]);
            if mode != Mode::Celt {
                let bandwidth = silk_bandwidth(self.config.bandwidth);
                let target = if mode == Mode::Hybrid {
                    (main_bytes * 8 * 3 / 4).saturating_sub(if transition.is_some() {
                        32
                    } else {
                        16
                    })
                } else {
                    (main_bytes * 8).saturating_sub(8)
                };
                let statistics = self.silk.encode_layer(
                    &mut range,
                    silk::Frame {
                        bandwidth,
                        duration: if frame_samples == 480 {
                            silk::FrameDuration::Ms10
                        } else {
                            silk::FrameDuration::Ms20
                        },
                    },
                    silk_duration(frame_samples)?,
                    channels as u8,
                    &self.native_pcm[..internal * channels],
                    target as u32,
                )?;
                if index == 0 {
                    has_fec = statistics.redundancy;
                }
                encoder_transition::write_header(&mut range, mode, payload_bytes, transition)
                    .map_err(|_| Error::Coding)?;
            }
            if mode != Mode::Silk {
                let lm = match frame_samples {
                    120 => 0,
                    240 => 1,
                    480 => 2,
                    960 => 3,
                    _ => return Err(Error::Argument),
                };
                self.celt.encode_layer(
                    pcm,
                    &mut range,
                    CeltLayer {
                        samples: frame_samples,
                        coded_channels: channels,
                        lm,
                        start: if mode == Mode::Hybrid { 17 } else { 0 },
                        end: end_band(self.config.bandwidth),
                        packet_bytes: main_bytes,
                    },
                )?;
            }
            interval = range.range();
            let length = if mode == Mode::Silk && transition.is_some() {
                let boundary = encoder_transition::finish_silk(range).map_err(|_| Error::Coding)?;
                interval = boundary.range;
                self.frames[index][1 + boundary.compact_bytes..1 + boundary.bytes].fill(0);
                boundary.bytes
            } else if mode == Mode::Silk {
                range.finish()?
            } else {
                range.finish_padded()?
            };
            if let Some(transition) = transition {
                if transition.position == Position::End {
                    self.celt.reset();
                    redundant_interval = self.celt.encode_redundant(
                        &pcm[(frame_samples - 240) * channels..],
                        channels,
                        self.config.bandwidth,
                        &mut redundant[..transition.bytes],
                    )?;
                }
                self.frames[index][1 + length..1 + length + transition.bytes]
                    .copy_from_slice(&redundant[..transition.bytes]);
                interval ^= redundant_interval;
                *size = 1 + length + transition.bytes;
                transition_redundancy = true;
            } else {
                *size = length + 1;
            }
        }
        let mut packets = Repacketizer::new();
        for (frame, &size) in self.frames.iter().zip(&sizes).take(count) {
            packets.push(&frame[..size]).map_err(|_| Error::Coding)?;
        }
        let length = if self.config.rate == RateControl::Constant && !suppress {
            packets
                .write_padded(0..count, output, target_bytes)
                .map_err(|_| Error::Output)?
        } else {
            packets.write(0..count, output).map_err(|_| Error::Output)?
        };
        self.rate_remainder = numerator % (48_000 * 8);
        if !suppress {
            self.previous_mode = Some(mode);
            self.previous_bandwidth = Some(self.config.bandwidth);
        }
        self.final_range = interval;
        self.has_fec = has_fec;
        self.last_dtx = suppress;
        self.transition_redundancy = transition_redundancy;
        self.previous_redundancy_end = transition_redundancy && position == Some(Position::End);
        if let Some(config) = self.pending_config.take() {
            self.reconfigure(config)?;
        }
        Ok(length)
    }

    fn prepare_native(
        &mut self,
        pcm: &[f32],
        samples: usize,
        channels: usize,
    ) -> Result<usize, Error> {
        let bandwidth = silk_bandwidth(self.config.bandwidth);
        if self.input_bandwidth != Some(bandwidth) {
            for downsampler in &mut self.downsamplers {
                downsampler.reset(bandwidth);
            }
            self.input_bandwidth = Some(bandwidth);
        }
        let internal = samples / self.downsamplers[0].ratio();
        let mut native_channel = [0.0; MAX_NATIVE_SAMPLES];
        for channel in 0..channels {
            for sample in 0..samples {
                self.channel_pcm[channel][sample] = pcm[sample * channels + channel];
            }
            self.downsamplers[channel]
                .process(
                    &self.channel_pcm[channel][..samples],
                    &mut native_channel[..internal],
                )
                .map_err(|_| Error::Coding)?;
            for (sample, &value) in native_channel[..internal].iter().enumerate() {
                self.native_pcm[sample * channels + channel] = value.clamp(-1.0, 1.0);
            }
        }
        Ok(internal)
    }
}

fn silk_bandwidth(bandwidth: Bandwidth) -> silk::Bandwidth {
    match bandwidth {
        Bandwidth::Narrowband => silk::Bandwidth::Narrow,
        Bandwidth::Mediumband => silk::Bandwidth::Medium,
        _ => silk::Bandwidth::Wide,
    }
}
fn silk_duration(samples: usize) -> Result<silk::PacketDuration, Error> {
    match samples {
        480 => Ok(silk::PacketDuration::Ms10),
        960 => Ok(silk::PacketDuration::Ms20),
        1920 => Ok(silk::PacketDuration::Ms40),
        2880 => Ok(silk::PacketDuration::Ms60),
        _ => Err(Error::Argument),
    }
}
fn end_band(bandwidth: Bandwidth) -> usize {
    match bandwidth {
        Bandwidth::Narrowband => 13,
        Bandwidth::Mediumband | Bandwidth::Wideband => 17,
        Bandwidth::Superwideband => 19,
        Bandwidth::Fullband => 21,
    }
}
fn toc(mode: Mode, bandwidth: Bandwidth, samples: usize, channels: usize) -> Result<u8, Error> {
    let config = match mode {
        Mode::Silk => {
            let group = match bandwidth {
                Bandwidth::Narrowband => 0,
                Bandwidth::Mediumband => 1,
                Bandwidth::Wideband => 2,
                _ => return Err(Error::Argument),
            };
            let duration = match samples {
                480 => 0,
                960 => 1,
                1920 => 2,
                2880 => 3,
                _ => return Err(Error::Argument),
            };
            4 * group + duration
        }
        Mode::Hybrid => {
            12 + 2 * u8::from(bandwidth == Bandwidth::Fullband) + u8::from(samples == 960)
        }
        Mode::Celt => {
            let group = match bandwidth {
                Bandwidth::Narrowband => 0,
                Bandwidth::Wideband => 1,
                Bandwidth::Superwideband => 2,
                Bandwidth::Fullband => 3,
                _ => return Err(Error::Argument),
            };
            let duration = match samples {
                120 => 0,
                240 => 1,
                480 => 2,
                960 => 3,
                _ => return Err(Error::Argument),
            };
            16 + 4 * group + duration
        }
    };
    Ok((config << 3) | ((channels as u8 - 1) << 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::Decoder;

    #[test]
    fn long_packets_cover_all_modes_budgets_and_quiet_refreshes() {
        for (mode, bandwidth) in [
            (Mode::Silk, Bandwidth::Narrowband),
            (Mode::Silk, Bandwidth::Mediumband),
            (Mode::Silk, Bandwidth::Wideband),
            (Mode::Hybrid, Bandwidth::Superwideband),
            (Mode::Hybrid, Bandwidth::Fullband),
            (Mode::Celt, Bandwidth::Narrowband),
            (Mode::Celt, Bandwidth::Wideband),
            (Mode::Celt, Bandwidth::Superwideband),
            (Mode::Celt, Bandwidth::Fullband),
        ] {
            for duration in [
                FrameDuration::Ms80,
                FrameDuration::Ms100,
                FrameDuration::Ms120,
            ] {
                for channels in [1, 2] {
                    for rate in [RateControl::Constant, RateControl::Variable] {
                        let config = EncoderConfig::new(
                            64_123 * channels as u32,
                            channels,
                            duration,
                            bandwidth,
                            Application::Voice,
                        )
                        .unwrap()
                        .with_mode(mode)
                        .unwrap()
                        .with_rate_control(rate)
                        .with_dtx(true)
                        .unwrap();
                        let samples = duration.samples();
                        let mut encoder = Encoder::new(config).unwrap();
                        let mut decoder = Decoder::new(channels).unwrap();
                        let mut packet = vec![0; config.max_packet_bytes() + 1];
                        let mut output = vec![0.0; samples * channels];
                        let mut input = vec![0.0; samples * channels];
                        for (index, value) in input.iter_mut().enumerate() {
                            *value = 0.2 * (index as f32 * 0.017).sin();
                        }
                        let mut total = 0;
                        for index in 0..3 {
                            let length = encoder.encode(&input, &mut packet).unwrap();
                            let parsed = super::super::Packet::parse(&packet[..length]).unwrap();
                            assert_eq!(parsed.samples(48_000).unwrap(), samples);
                            assert_eq!(parsed.frame_count(), samples / 960);
                            assert_eq!(parsed.toc().mode(), mode);
                            assert_eq!(
                                decoder.decode(&packet[..length], &mut output).unwrap(),
                                samples
                            );
                            assert_eq!(decoder.final_range(), encoder.final_range());
                            total += length;
                            if rate == RateControl::Constant {
                                assert_eq!(
                                    total as u64,
                                    u64::from(config.bitrate()) * samples as u64 * (index + 1)
                                        / 384_000
                                );
                            }
                        }
                        input.fill(0.0);
                        let mut suppressed = false;
                        let mut refreshed = false;
                        for _ in 0..12 {
                            let length = encoder.encode(&input, &mut packet).unwrap();
                            refreshed |= suppressed && !encoder.was_dtx();
                            suppressed |= encoder.was_dtx();
                            assert_eq!(
                                decoder.decode(&packet[..length], &mut output).unwrap(),
                                samples
                            );
                            assert_eq!(decoder.final_range(), encoder.final_range());
                            assert!(output.iter().all(|value| value.is_finite()));
                        }
                        assert!(suppressed && refreshed);
                        input.fill(0.15);
                        let length = encoder.encode(&input, &mut packet).unwrap();
                        assert!(!encoder.was_dtx());
                        assert_eq!(
                            decoder.decode(&packet[..length], &mut output).unwrap(),
                            samples
                        );
                        assert_eq!(decoder.final_range(), encoder.final_range());
                    }
                }
            }
        }
    }

    #[test]
    fn all_modes_have_valid_packet_geometry_and_matching_final_ranges() {
        for bandwidth in [
            Bandwidth::Narrowband,
            Bandwidth::Mediumband,
            Bandwidth::Wideband,
            Bandwidth::Superwideband,
            Bandwidth::Fullband,
        ] {
            for application in [Application::Voice, Application::Audio] {
                for duration in [
                    FrameDuration::Ms2_5,
                    FrameDuration::Ms5,
                    FrameDuration::Ms10,
                    FrameDuration::Ms20,
                    FrameDuration::Ms40,
                    FrameDuration::Ms60,
                ] {
                    for channels in 1..=2 {
                        let Ok(config) = EncoderConfig::new(
                            48_000 * channels as u32,
                            channels,
                            duration,
                            bandwidth,
                            application,
                        ) else {
                            continue;
                        };
                        let mut encoder = Encoder::new(config).unwrap();
                        let mut decoder = Decoder::new(channels).unwrap();
                        let mut signal = [0.0; MAX_SAMPLES * 2];
                        let mut decoded = [0.0; MAX_SAMPLES * 2];
                        let mut packet = [0; 4096];
                        let samples = duration.samples();
                        for frame in 0..2 {
                            for sample in 0..samples {
                                for channel in 0..channels {
                                    let t = (frame * samples + sample) as f32 / 48_000.0;
                                    signal[sample * channels + channel] = 0.1
                                        * (t * 2.0
                                            * std::f32::consts::PI
                                            * (220.0 + channel as f32 * 30.0))
                                            .sin();
                                }
                            }
                            let length = encoder.encode(&signal[..samples * channels], &mut packet)
                                .unwrap_or_else(|error| panic!("{application:?}/{bandwidth:?}/{duration:?}/{channels}ch: {error}"));
                            assert!(length <= config.max_packet_bytes());
                            let count = decoder.decode(&packet[..length], &mut decoded)
                                .unwrap_or_else(|error| panic!("decode {application:?}/{bandwidth:?}/{duration:?}/{channels}ch: {error}"));
                            assert_eq!(count, samples);
                            assert_eq!(
                                decoder.final_range(),
                                encoder.final_range(),
                                "{application:?}/{bandwidth:?}/{duration:?}/{channels}ch"
                            );
                            assert!(
                                decoded[..samples * channels]
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
    fn constant_rate_and_reconfiguration_keep_valid_streams() {
        let first = EncoderConfig::new(
            32_123,
            1,
            FrameDuration::Ms20,
            Bandwidth::Wideband,
            Application::Voice,
        )
        .unwrap()
        .with_rate_control(RateControl::Constant);
        let second = EncoderConfig::new(
            96_000,
            2,
            FrameDuration::Ms40,
            Bandwidth::Fullband,
            Application::Audio,
        )
        .unwrap()
        .with_rate_control(RateControl::Constant);
        let mut encoder = Encoder::new(first).unwrap();
        let mut decoder = Decoder::new(2).unwrap();
        let mut packet = [0; 4096];
        let input = [0.0; MAX_SAMPLES * 2];
        let mut output = [0.0; MAX_SAMPLES * 2];
        for config in [first, second, first] {
            encoder.reconfigure(config).unwrap();
            assert_eq!(encoder.config(), config);
            for frame in 0..3 {
                let length = encoder
                    .encode(
                        &input[..config.channels * config.duration.samples()],
                        &mut packet,
                    )
                    .unwrap();
                let numerator = u64::from(config.bitrate) * config.duration.samples() as u64;
                assert_eq!(
                    length as u64,
                    numerator * (frame + 1) / 384_000 - numerator * frame / 384_000
                );
                assert_eq!(
                    decoder.decode(&packet[..length], &mut output).unwrap(),
                    config.duration.samples()
                );
                assert_eq!(decoder.final_range(), encoder.final_range());
            }
        }
    }

    #[test]
    fn minimum_floor_budget_and_invalid_io_are_rejected() {
        assert_eq!(
            EncoderConfig::new(
                8000,
                1,
                FrameDuration::Ms2_5,
                Bandwidth::Fullband,
                Application::Audio
            ),
            Err(Error::Argument)
        );
        let mut encoder = Encoder::new(EncoderConfig::default()).unwrap();
        let mut packet = [0; 100];
        assert_eq!(
            encoder.encode(&[f32::NAN; 960], &mut packet),
            Err(Error::Input)
        );
        assert_eq!(encoder.final_range(), 0);
        assert_eq!(encoder.encode(&[0.0; 960], &mut []), Err(Error::Output));
        assert_eq!(encoder.final_range(), 0);
        assert!(packet.iter().all(|&value| value == 0));
    }

    #[test]
    fn dtx_preserves_duration_then_resumes_real_speech_and_periodic_refreshes() {
        assert!(
            EncoderConfig::default()
                .with_mode(Mode::Celt)
                .unwrap()
                .with_fec(true)
                .is_err()
        );
        for (application, bandwidth) in [
            (Application::Voice, Bandwidth::Wideband),
            (Application::Voice, Bandwidth::Fullband),
            (Application::Audio, Bandwidth::Fullband),
        ] {
            for duration in [
                FrameDuration::Ms2_5,
                FrameDuration::Ms5,
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms40,
                FrameDuration::Ms60,
            ] {
                for channels in 1..=2 {
                    let config = EncoderConfig::new(
                        48_000 * channels as u32,
                        channels,
                        duration,
                        bandwidth,
                        application,
                    )
                    .unwrap()
                    .with_dtx(true)
                    .unwrap();
                    let mut encoder = Encoder::new(config).unwrap();
                    let mut decoder = Decoder::new(channels).unwrap();
                    let mut input = [0.0; MAX_SAMPLES * 2];
                    let mut output = [0.0; MAX_SAMPLES * 2];
                    let mut packet = [0; 4096];
                    let samples = duration.samples();
                    let mut omitted = 0;
                    let mut refresh = 0;
                    for index in 0..36_000usize.div_ceil(samples) {
                        let length = encoder
                            .encode(&input[..samples * channels], &mut packet)
                            .unwrap();
                        if encoder.was_dtx() {
                            omitted += 1;
                            assert!(length <= 2);
                            assert_eq!(encoder.final_range(), 0);
                        } else if index * samples >= 9600 {
                            refresh += 1;
                        }
                        assert_eq!(
                            decoder.decode(&packet[..length], &mut output).unwrap(),
                            samples
                        );
                        assert_eq!(decoder.final_range(), encoder.final_range());
                        assert!(
                            output[..samples * channels]
                                .iter()
                                .all(|value| value.is_finite())
                        );
                    }
                    assert!(omitted > 0 && refresh > 0);
                    for sample in 0..samples {
                        for channel in 0..channels {
                            input[sample * channels + channel] = 0.1 * (sample as f32 * 0.02).sin();
                        }
                    }
                    let length = encoder
                        .encode(&input[..samples * channels], &mut packet)
                        .unwrap();
                    assert!(!encoder.was_dtx());
                    assert!(length > 1);
                    assert_eq!(
                        decoder.decode(&packet[..length], &mut output).unwrap(),
                        samples
                    );
                    assert_eq!(decoder.final_range(), encoder.final_range());
                }
            }
        }
    }

    #[test]
    fn beginning_and_prepared_end_transitions_preserve_exact_following_ranges() {
        for bandwidth in [
            Bandwidth::Narrowband,
            Bandwidth::Mediumband,
            Bandwidth::Wideband,
            Bandwidth::Superwideband,
            Bandwidth::Fullband,
        ] {
            for duration in [
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms40,
                FrameDuration::Ms60,
            ] {
                for channels in 1..=2 {
                    for rate in [RateControl::Variable, RateControl::Constant] {
                        let celt = EncoderConfig::new(
                            96_000 * channels as u32,
                            channels,
                            FrameDuration::Ms20,
                            Bandwidth::Fullband,
                            Application::Audio,
                        )
                        .unwrap()
                        .with_rate_control(rate);
                        let lp = EncoderConfig::new(
                            96_000 * channels as u32,
                            channels,
                            duration,
                            bandwidth,
                            Application::Voice,
                        )
                        .unwrap()
                        .with_rate_control(rate);
                        let mut encoder = Encoder::new(celt).unwrap();
                        let mut decoder = Decoder::new(channels).unwrap();
                        let mut signal = [0.0; MAX_SAMPLES * 2];
                        let mut output = [0.0; MAX_SAMPLES * 2];
                        let mut packet = [0; 4096];
                        let mut position = 0;
                        for step in 0..5 {
                            if step == 1 {
                                encoder.reconfigure(lp).unwrap();
                            }
                            if step == 3 {
                                encoder.prepare_reconfigure(celt).unwrap();
                            }
                            let config = encoder.config();
                            let samples = config.duration.samples();
                            for sample in 0..samples {
                                for channel in 0..channels {
                                    signal[sample * channels + channel] = 0.1
                                        * ((position + sample) as f32
                                            * 2.0
                                            * std::f32::consts::PI
                                            * (180.0 + channel as f32 * 40.0)
                                            / 48_000.0)
                                            .sin();
                                }
                            }
                            let length = encoder.encode(&signal[..samples * channels], &mut packet)
                                .unwrap_or_else(|error| panic!("{bandwidth:?}/{duration:?}/{channels}ch/{rate:?}/{step}: {error}"));
                            assert_eq!(encoder.has_transition_redundancy(), step == 1 || step == 3);
                            assert_eq!(
                                decoder.decode(&packet[..length], &mut output).unwrap(),
                                samples
                            );
                            assert_eq!(
                                decoder.final_range(),
                                encoder.final_range(),
                                "{bandwidth:?}/{duration:?}/{channels}ch/{rate:?}/{step}"
                            );
                            assert!(
                                output[..samples * channels]
                                    .iter()
                                    .all(|sample| sample.is_finite())
                            );
                            position += samples;
                        }
                        assert_eq!(encoder.config(), celt);
                    }
                }
            }
        }
    }

    #[test]
    fn prepared_silk_rate_changes_use_both_required_transition_halves() {
        for (old_band, new_band) in [
            (Bandwidth::Narrowband, Bandwidth::Mediumband),
            (Bandwidth::Mediumband, Bandwidth::Wideband),
            (Bandwidth::Wideband, Bandwidth::Narrowband),
            (Bandwidth::Narrowband, Bandwidth::Fullband),
            (Bandwidth::Fullband, Bandwidth::Mediumband),
            (Bandwidth::Wideband, Bandwidth::Fullband),
            (Bandwidth::Fullband, Bandwidth::Wideband),
        ] {
            for channels in 1..=2 {
                let old = EncoderConfig::new(
                    96_000 * channels as u32,
                    channels,
                    FrameDuration::Ms20,
                    old_band,
                    Application::Voice,
                )
                .unwrap();
                let new = EncoderConfig::new(
                    96_000 * channels as u32,
                    channels,
                    FrameDuration::Ms20,
                    new_band,
                    Application::Voice,
                )
                .unwrap();
                let mut encoder = Encoder::new(old).unwrap();
                let mut decoder = Decoder::new(channels).unwrap();
                let input: [f32; 1920] =
                    std::array::from_fn(|sample| 0.1 * (sample as f32 * 0.02).sin());
                let mut output = [0.0; 1920];
                let mut bytes = [0; 4096];
                let changed_rate = silk_bandwidth(old_band) != silk_bandwidth(new_band);
                for step in 0..4 {
                    if step == 1 {
                        encoder.prepare_reconfigure(new).unwrap();
                    }
                    let length = encoder
                        .encode(&input[..960 * channels], &mut bytes)
                        .unwrap();
                    assert_eq!(
                        encoder.has_transition_redundancy(),
                        changed_rate && (step == 1 || (step == 2 && new.mode() == Mode::Silk))
                    );
                    assert_eq!(decoder.decode(&bytes[..length], &mut output).unwrap(), 960);
                    assert_eq!(
                        decoder.final_range(),
                        encoder.final_range(),
                        "{old_band:?}->{new_band:?}/{channels}ch/{step}"
                    );
                }
            }
        }
    }

    #[test]
    fn emitted_dtx_status_survives_immediate_and_queued_configuration_changes() {
        let enabled = EncoderConfig::default().with_dtx(true).unwrap();
        let disabled = enabled.with_dtx(false).unwrap();
        let quiet = [0.0; 960];
        let speech: [f32; 960] = std::array::from_fn(|i| 0.1 * (i as f32 * 0.02).sin());
        for queued in [false, true] {
            let mut encoder = Encoder::new(enabled).unwrap();
            let mut decoder = Decoder::new(1).unwrap();
            let mut packet = [0; 100];
            let mut output = [0.0; 960];
            for index in 0..11 {
                if queued && index == 10 {
                    encoder.prepare_reconfigure(disabled).unwrap();
                }
                let length = encoder.encode(&quiet, &mut packet).unwrap();
                assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
            }
            assert!(encoder.was_dtx());
            if !queued {
                encoder.reconfigure(disabled).unwrap();
            }
            assert!(!encoder.config().dtx());
            assert!(
                encoder.was_dtx(),
                "reconfiguration erased the emitted packet's status"
            );
            assert_eq!(encoder.encode(&speech, &mut []), Err(Error::Output));
            assert!(
                encoder.was_dtx(),
                "a rejected encode changed the previous packet's status"
            );
            let length = encoder.encode(&speech, &mut packet).unwrap();
            assert!(!encoder.was_dtx());
            assert!(length > 1);
            assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
            assert_eq!(decoder.final_range(), encoder.final_range());
            encoder.reset();
            assert!(!encoder.was_dtx());
        }
    }
}
