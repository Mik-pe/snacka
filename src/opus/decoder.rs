// SPDX-License-Identifier: MIT
//! Raw Opus decoding and packet recovery, composing owned SILK and CELT layers.

use std::fmt;

use super::celt::{self, CeltDecoder, CeltLayer};
use super::packet::{Bandwidth, Mode, Packet, PacketError, Toc};
use super::range;
use super::silk::native_delay::NativeDelay;
use super::silk::resample::FilterPlan;
use super::silk::{self, Resampler, SilkDecoder};

const MAX_FRAME_SAMPLES: usize = 2880;
const MAX_INTERNAL_SAMPLES: usize = 960;
const CROSSLAP: usize = 120;
const OUTPUT_TAPS: usize = 97;

/// The result of recovering a missing interval from the following packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// The following SILK or Hybrid packet supplied in-band redundancy.
    Fec,
    /// Redundancy was unavailable; the interval was concealed from history.
    Concealed,
}

// A causal, unity-gain windowed-sinc decimator. At 48 kHz it is an exact
// identity and skips the ring and convolution. Pruned CELT and SILK sampled at
// or above their internal rate use phase-zero sampling without convolution.
// Lower-rate SILK uses this filter, with a one-millisecond group delay.
const OUTPUT_HISTORY: usize = 128;

struct OutputResampler {
    ratio: usize,
    taps: [f32; OUTPUT_TAPS],
    history: [[f32; OUTPUT_HISTORY]; 2],
    cursor: usize,
}
impl OutputResampler {
    fn new(rate: u32) -> Result<Self, Error> {
        let ratio = match rate {
            8_000 | 12_000 | 16_000 | 24_000 | 48_000 => 48_000 / rate as usize,
            _ => return Err(Error::Argument),
        };
        let mut taps = [0.0; OUTPUT_TAPS];
        if ratio > 1 {
            let cutoff = 0.45 / ratio as f64;
            let middle = (OUTPUT_TAPS - 1) as f64 * 0.5;
            for (index, tap) in taps.iter_mut().enumerate() {
                let position = index as f64 - middle;
                let phase = std::f64::consts::TAU * index as f64 / (OUTPUT_TAPS - 1) as f64;
                let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
                let sinc = if position == 0.0 {
                    2.0 * cutoff
                } else {
                    (std::f64::consts::TAU * cutoff * position).sin()
                        / (std::f64::consts::PI * position)
                };
                *tap = (window * sinc) as f32;
            }
            let gain = taps.iter().sum::<f32>();
            for tap in &mut taps {
                *tap /= gain;
            }
        }
        Ok(Self {
            ratio,
            taps,
            history: [[0.0; OUTPUT_HISTORY]; 2],
            cursor: 0,
        })
    }

    fn reset(&mut self) {
        self.history = [[0.0; OUTPUT_HISTORY]; 2];
        self.cursor = 0;
    }
    // Direct/fused lowband rendering still supplies the complete recent 48 kHz
    // tail. Re-anchor that chronological history before a mixed/full-rate frame
    // resumes this FIR; codec intervals always preserve sampling phase zero.
    fn prime(&mut self, previous: &[[f32; OUTPUT_TAPS - 1]]) {
        debug_assert!((1..=2).contains(&previous.len()));
        for (history, tail) in self.history[..previous.len()].iter_mut().zip(previous) {
            history[..OUTPUT_HISTORY - (OUTPUT_TAPS - 1)].fill(0.0);
            history[OUTPUT_HISTORY - (OUTPUT_TAPS - 1)..].copy_from_slice(tail);
        }
        self.cursor = 0;
    }
    fn process(&mut self, channels: usize, input: &[f32], output: &mut [f32], filter: bool) {
        if self.ratio == 1 {
            output[..input.len()].copy_from_slice(input);
            return;
        }
        match (channels, self.ratio) {
            (1, 2) => self.run::<1, 2>(input, output, filter),
            (1, 3) => self.run::<1, 3>(input, output, filter),
            (1, 4) => self.run::<1, 4>(input, output, filter),
            (1, 6) => self.run::<1, 6>(input, output, filter),
            (2, 2) => self.run::<2, 2>(input, output, filter),
            (2, 3) => self.run::<2, 3>(input, output, filter),
            (2, 4) => self.run::<2, 4>(input, output, filter),
            (2, 6) => self.run::<2, 6>(input, output, filter),
            _ => unreachable!(),
        }
    }
    fn run<const CHANNELS: usize, const RATIO: usize>(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        filter: bool,
    ) {
        // Every codec/recovery interval is a multiple of 2.5 ms, hence a
        // whole number of blocks at each supported output rate. Phase zero
        // is retained across packet boundaries without a per-sample modulo.
        debug_assert!(input.len().is_multiple_of(CHANNELS * RATIO));
        for (block, destination) in input
            .chunks_exact(CHANNELS * RATIO)
            .zip(output.as_chunks_mut::<CHANNELS>().0.iter_mut())
        {
            for (history, &value) in self.history[..CHANNELS].iter_mut().zip(&block[..CHANNELS]) {
                history[self.cursor] = value;
            }
            let head = OUTPUT_TAPS.min(self.cursor + 1);
            for (history, destination) in self.history[..CHANNELS].iter().zip(&mut *destination) {
                if filter {
                    let mut partial = [0.0f64; 4];
                    output_dot_reverse(&history[..=self.cursor], &self.taps[..head], &mut partial);
                    output_dot_reverse(
                        &history[self.cursor + 1..],
                        &self.taps[head..],
                        &mut partial,
                    );
                    *destination = (partial[0] + partial[1] + partial[2] + partial[3]) as f32;
                }
            }
            if !filter {
                destination.copy_from_slice(&block[..CHANNELS]);
            }
            self.cursor = (self.cursor + 1) & (OUTPUT_HISTORY - 1);
            for sample in block[CHANNELS..].as_chunks::<CHANNELS>().0 {
                for (history, &value) in self.history[..CHANNELS].iter_mut().zip(sample) {
                    history[self.cursor] = value;
                }
                self.cursor = (self.cursor + 1) & (OUTPUT_HISTORY - 1);
            }
        }
    }
}
fn output_dot_reverse(history: &[f32], coefficients: &[f32], partial: &mut [f64; 4]) {
    let mut values = history.iter().rev();
    let (groups, tail) = coefficients.as_chunks::<4>();
    for group in groups {
        partial[0] += f64::from(*values.next().unwrap()) * f64::from(group[0]);
        partial[1] += f64::from(*values.next().unwrap()) * f64::from(group[1]);
        partial[2] += f64::from(*values.next().unwrap()) * f64::from(group[2]);
        partial[3] += f64::from(*values.next().unwrap()) * f64::from(group[3]);
    }
    for (&coefficient, &value) in tail.iter().zip(values) {
        partial[0] += f64::from(value) * f64::from(coefficient);
    }
}

#[cfg(test)]
mod output_rate_tests {
    use super::*;

    #[test]
    fn output_phase_filter_switch_and_ring_wrap_match_literal_convolution() {
        for rate in [8000, 12000, 16000, 24000, 48000] {
            for channels in [1, 2] {
                let mut converter = OutputResampler::new(rate).unwrap();
                let ratio = 48000 / rate as usize;
                let input: Vec<f32> = (0..120 * 17 * channels)
                    .map(|index| ((index * 43 % 257) as f32 - 128.0) / 128.0)
                    .collect();
                let mut output = vec![2.0; 120 * channels / ratio + 1];
                for (part, block) in input.chunks_exact(120 * channels).enumerate() {
                    let filter = part % 3 != 0;
                    converter.process(channels, block, &mut output, filter);
                    for (sample, &actual) in output[..output.len() - 1].iter().enumerate() {
                        let channel = sample % channels;
                        let position = part * 120 + sample / channels * ratio;
                        if !filter || ratio == 1 {
                            assert_eq!(actual, input[position * channels + channel]);
                        } else {
                            let wanted = converter.taps[..OUTPUT_TAPS.min(position + 1)]
                                .iter()
                                .enumerate()
                                .map(|(lag, &tap)| {
                                    f64::from(tap)
                                        * f64::from(input[(position - lag) * channels + channel])
                                })
                                .sum::<f64>() as f32;
                            assert!((actual - wanted).abs() < 1e-12);
                        }
                    }
                    assert_eq!(output[output.len() - 1], 2.0);
                }
                converter.reset();
                converter.process(channels, &vec![0.0; 120 * channels], &mut output, true);
                assert!(
                    output[..output.len() - 1]
                        .iter()
                        .all(|&sample| sample == 0.0)
                );
            }
        }
    }

    #[test]
    fn primed_history_matches_full_rate_rendering_after_bypass() {
        for rate in [8000, 12000, 16000, 24000] {
            for channels in [1, 2] {
                let mut full = OutputResampler::new(rate).unwrap();
                let mut primed = OutputResampler::new(rate).unwrap();
                let source: [f32; 480] =
                    std::array::from_fn(|index| ((index * 43 % 257) as f32 - 128.0) / 128.0);
                let mut discarded = [0.0; 240];
                full.process(channels, &source[..120 * channels], &mut discarded, false);
                let mut tail = [[0.0; OUTPUT_TAPS - 1]; 2];
                for (channel, history) in tail.iter_mut().enumerate().take(channels) {
                    for (sample, value) in history.iter_mut().enumerate() {
                        *value = source[(120 - (OUTPUT_TAPS - 1) + sample) * channels + channel];
                    }
                }
                primed.prime(&tail[..channels]);
                let mut expected = [0.0; 240];
                let mut actual = [0.0; 240];
                let next = &source[120 * channels..240 * channels];
                full.process(channels, next, &mut expected, true);
                primed.process(channels, next, &mut actual, true);
                assert_eq!(actual, expected);
            }
        }
    }
}

// Vec's repeated scalar initialization writes straight to heap storage. Turning
// the resulting slice into a boxed array preserves fixed dimensions without
// first constructing a large array temporary on the caller's stack.
fn zeroed_buffer<const N: usize>() -> Box<[f32; N]> {
    vec![0.0; N]
        .into_boxed_slice()
        .try_into()
        .expect("fixed decoder buffer length")
}

/// An invalid Opus packet, output buffer or decoded layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Channels, sampling rate or requested missing duration are invalid.
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
            Self::Argument => formatter.write_str("invalid Opus decoder configuration or duration"),
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

/// Stateful decoder for raw single-stream SILK, Hybrid and CELT packets.
///
/// PCM is normalized interleaved floating point. Mono streams can be duplicated
/// to stereo output; stereo streams can be averaged to mono. Framing and full output capacity are checked
/// before changing history. A compressed error can advance part of the packet;
/// reset after `Corrupt`. Construction allocates transform plans and fixed
/// working memory on the heap. Packet decoding and configuration transitions
/// reuse that storage without allocating.
///
/// The default output rate is 48 kHz. CELT suppresses frequencies above the
/// output Nyquist limit. SILK output below its internal sampling rate uses a
/// causal low-pass filter with one millisecond of additional delay. SILK uses
/// Snacka's causal resampler, permitted by RFC 6716 §4.2.9. These choices need not
/// reproduce the reference codec's concealment or resampling sample-for-sample.
pub struct Decoder {
    channels: usize,
    sample_rate: u32,
    gain_q8: i16,
    gain_factor: f32,
    output_resampler: OutputResampler,
    silk: Box<SilkDecoder>,
    celt: Box<CeltDecoder>,
    resamplers: [Resampler; 2],
    native_delays: [NativeDelay; 2],
    filter_plans: Box<[Option<FilterPlan>; 2]>,
    output_history_bypassed: bool,
    hybrid_filter_pending: bool,
    internal_bandwidth: Option<silk::Bandwidth>,
    silk_pcm: Box<[f32; MAX_INTERNAL_SAMPLES * 2]>,
    lowband: [Box<[f32; MAX_FRAME_SAMPLES]>; 2],
    frame: Box<[f32; MAX_FRAME_SAMPLES * 2]>,
    frame_rate: u32,
    redundant: Box<[f32; 2 * CROSSLAP * 2]>,
    window: [f32; CROSSLAP],
    previous_mode: Option<Mode>,
    previous_bandwidth: Option<Bandwidth>,
    previous_coded_channels: usize,
    previous_redundancy_end: bool,
    final_range: u32,
}

impl Decoder {
    /// Prepare a decoder for one or two output channels.
    pub fn new(channels: usize) -> Result<Self, Error> {
        Self::with_sample_rate(channels, 48_000)
    }

    /// Prepare mono or stereo output at 8, 12, 16, 24 or 48 kHz.
    pub fn with_sample_rate(channels: usize, sample_rate: u32) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Argument);
        }
        let output_resampler = OutputResampler::new(sample_rate)?;
        let filter_plans = Box::new([
            if sample_rate == 8_000 {
                Some(FilterPlan::new(
                    silk::Bandwidth::Medium,
                    sample_rate,
                    &output_resampler.taps,
                )?)
            } else {
                None
            },
            if sample_rate <= 12_000 {
                Some(FilterPlan::new(
                    silk::Bandwidth::Wide,
                    sample_rate,
                    &output_resampler.taps,
                )?)
            } else {
                None
            },
        ]);
        let mut celt = CeltDecoder::new(channels).map_err(|_| Error::Argument)?;
        celt.set_output_downsample(48_000 / sample_rate as usize);
        Ok(Self {
            channels,
            sample_rate,
            gain_q8: 0,
            gain_factor: 1.0,
            output_resampler,
            silk: Box::new(SilkDecoder::new()),
            celt: Box::new(celt),
            resamplers: std::array::from_fn(|_| Resampler::new(silk::Bandwidth::Wide)),
            native_delays: std::array::from_fn(|_| NativeDelay::new(silk::Bandwidth::Wide)),
            filter_plans,
            output_history_bypassed: false,
            hybrid_filter_pending: false,
            internal_bandwidth: None,
            silk_pcm: zeroed_buffer(),
            lowband: std::array::from_fn(|_| zeroed_buffer()),
            frame: zeroed_buffer(),
            frame_rate: 48_000,
            redundant: zeroed_buffer(),
            window: std::array::from_fn(|sample| {
                let phase = std::f64::consts::FRAC_PI_2 * (sample as f64 + 0.5) / CROSSLAP as f64;
                (std::f64::consts::FRAC_PI_2 * phase.sin().powi(2))
                    .sin()
                    .powi(2) as f32
            }),
            previous_mode: None,
            previous_bandwidth: None,
            previous_coded_channels: channels,
            previous_redundancy_end: false,
            final_range: 0,
        })
    }

    /// Decode a complete raw Opus packet and return samples per channel.
    /// Empty or one-byte compressed frames invoke the decoder's concealment.
    /// Trailing output capacity remains untouched.
    /// A content error can advance history and write an output prefix; reset
    /// after [`Error::Corrupt`] before continuing with another packet.
    pub fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        let packet = Packet::parse(packet)?;
        self.decode_parsed(&packet, output)
    }

    pub(crate) fn decode_parsed(
        &mut self,
        packet: &Packet<'_>,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        let toc = packet.toc();
        let total = packet.samples(self.sample_rate)?;
        if output.len() < total * self.channels {
            return Err(Error::Output);
        }
        let samples = packet.frame_samples(48_000)?;
        let output_samples = packet.frame_samples(self.sample_rate)?;
        for (index, bytes) in packet.frames().enumerate() {
            let begin = index * output_samples * self.channels;
            let destination = &mut output[begin..begin + output_samples * self.channels];
            if bytes.len() <= 1 {
                if self.previous_mode.is_none() {
                    self.previous_mode = Some(toc.mode());
                    self.previous_bandwidth = Some(toc.bandwidth());
                    self.previous_coded_channels = usize::from(toc.channels());
                }
                self.conceal_frame(samples)?;
                self.final_range = 0;
            } else {
                self.decode_frame(bytes, toc, samples)?;
            }
            self.write_output(samples, destination)?;
        }
        Ok(total)
    }

    fn requires_output_filter(&self) -> bool {
        self.previous_mode.is_some_and(|mode| mode != Mode::Celt)
            && self.previous_bandwidth.is_some_and(|bandwidth| {
                let native = match silk_bandwidth(bandwidth) {
                    silk::Bandwidth::Narrow => 8_000,
                    silk::Bandwidth::Medium => 12_000,
                    silk::Bandwidth::Wide => 16_000,
                };
                self.sample_rate < native
            })
    }

    fn write_output(&mut self, samples: usize, output: &mut [f32]) -> Result<(), Error> {
        if self.frame_rate == self.sample_rate {
            let count = samples * self.sample_rate as usize / 48_000 * self.channels;
            output[..count].copy_from_slice(&self.frame[..count]);
        } else {
            debug_assert_eq!(self.frame_rate, 48_000);
            self.output_resampler.process(
                self.channels,
                &self.frame[..samples * self.channels],
                output,
                self.requires_output_filter(),
            );
            self.output_history_bypassed = false;
        }
        if self.gain_q8 != 0 {
            let count = samples * self.sample_rate as usize / 48_000 * self.channels;
            for sample in &mut output[..count] {
                let gained = *sample * self.gain_factor;
                if !gained.is_finite() {
                    return Err(Error::Corrupt);
                }
                *sample = gained;
            }
        }
        Ok(())
    }

    fn add_highband(&mut self, samples: usize) {
        let factor = 48_000 / self.frame_rate as usize;
        match self.channels {
            1 => {
                for (sample, value) in self.frame[..samples / factor].iter_mut().enumerate() {
                    *value += self.lowband[0][sample * factor];
                }
            }
            2 => {
                for (sample, values) in self.frame[..samples / factor * 2]
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    let source = sample * factor * 2;
                    values[0] += self.lowband[0][source];
                    values[1] += self.lowband[0][source + 1];
                }
            }
            _ => unreachable!(),
        }
    }

    /// Advance a missing interval and return samples per channel.
    ///
    /// `samples` counts samples at the configured output rate. It must be a
    /// positive multiple of 2.5 ms, at most 120 ms. Before the first packet,
    /// concealment produces silence. Output capacity and duration are validated
    /// before changing prediction or interpolation history.
    pub fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, Error> {
        let quantum = self.sample_rate as usize / 400;
        if samples == 0 || samples > quantum * 48 || !samples.is_multiple_of(quantum) {
            return Err(Error::Argument);
        }
        if output.len() < samples * self.channels {
            return Err(Error::Output);
        }
        let ratio = 48_000 / self.sample_rate as usize;
        let total = samples * ratio;
        let mut completed = 0;
        while completed < total {
            let remaining = total - completed;
            let frame = [960, 480, 240, 120]
                .into_iter()
                .find(|&n| n <= remaining)
                .unwrap();
            self.conceal_frame(frame)?;
            let start = completed / ratio * self.channels;
            let end = (completed + frame) / ratio * self.channels;
            self.write_output(frame, &mut output[start..end])?;
            completed += frame;
        }
        self.final_range = 0;
        Ok(samples)
    }

    /// Recover a missing interval using redundancy in the following packet.
    ///
    /// This advances the missing interval only. Call [`Self::decode`] with the
    /// same packet afterwards to play its current signal. `samples` follows
    /// [`Self::conceal`]'s duration rules. The available redundancy fills the
    /// tail of the missing interval. Earlier redundant audio that has already
    /// been played is skipped; a longer loss is concealed before the recovered
    /// tail. When no redundancy covers the requested tail, the whole interval
    /// is concealed. Hybrid recovery conceals its high-frequency CELT layer.
    pub fn recover(
        &mut self,
        packet: &[u8],
        samples: usize,
        output: &mut [f32],
    ) -> Result<Recovery, Error> {
        let packet = Packet::parse(packet)?;
        self.recover_parsed(&packet, samples, output)
    }

    pub(crate) fn recover_parsed(
        &mut self,
        packet: &Packet<'_>,
        samples: usize,
        output: &mut [f32],
    ) -> Result<Recovery, Error> {
        let quantum = self.sample_rate as usize / 400;
        if samples == 0 || samples > quantum * 48 || !samples.is_multiple_of(quantum) {
            return Err(Error::Argument);
        }
        if output.len() < samples * self.channels {
            return Err(Error::Output);
        }
        let toc = packet.toc();
        let frame_output = packet.frame_samples(self.sample_rate)?;
        let frame_samples = packet.frame_samples(48_000)?;
        let bytes = packet.frames().next().ok_or(Error::Corrupt)?;
        if toc.mode() == Mode::Celt || bytes.len() <= 1 {
            self.conceal(samples, output)?;
            return Ok(Recovery::Concealed);
        }
        let duration = silk_duration(frame_samples)?;
        let bandwidth = silk_bandwidth(toc.bandwidth());
        let frame = silk::Frame {
            bandwidth,
            duration: if frame_samples == 480 {
                silk::FrameDuration::Ms10
            } else {
                silk::FrameDuration::Ms20
            },
        };
        let output_ratio = 48_000 / self.sample_rate as usize;
        let recovered = samples.min(frame_output) * output_ratio;
        let native_ratio = match bandwidth {
            silk::Bandwidth::Narrow => 6,
            silk::Bandwidth::Medium => 4,
            silk::Bandwidth::Wide => 3,
        };
        let skip = (frame_samples - recovered) / native_ratio;
        let mut probe = range::Decoder::new(bytes);
        let header = silk::Header::decode(&mut probe, duration, toc.channels())?;
        let present = (0..3).any(|interval| {
            (interval + 1) * frame.samples() > skip
                && (0..usize::from(toc.channels()))
                    .any(|channel| header.redundancy(channel, interval) == Some(true))
        });
        if !present {
            let interval = probe.range();
            self.conceal(samples, output)?;
            self.final_range = interval;
            return Ok(Recovery::Concealed);
        }
        let prefix = samples.saturating_sub(frame_output);
        if prefix != 0 {
            self.conceal(prefix, &mut output[..prefix * self.channels])?;
        }
        self.prepare_lowband(bandwidth);
        let mut decoder = range::Decoder::new(bytes);
        let internal = self
            .silk
            .decode_fec_tail(
                &mut decoder,
                frame,
                duration,
                toc.channels(),
                recovered / native_ratio,
                &mut self.silk_pcm[..],
            )?
            .ok_or(Error::Corrupt)?;
        let direct = toc.mode() != Mode::Hybrid || self.sample_rate >= 48_000 / native_ratio as u32;
        self.render_lowband(internal, usize::from(toc.channels()), recovered, direct)?;
        if toc.mode() == Mode::Hybrid {
            if self.previous_mode != Some(Mode::Hybrid) {
                self.celt.reset();
            }
            self.celt.conceal_interval(
                recovered,
                17,
                celt_end(toc.bandwidth()),
                &mut self.lowband[0][..recovered * self.channels],
            )?;
            self.add_highband(recovered);
        }
        self.previous_mode = Some(toc.mode());
        self.previous_bandwidth = Some(toc.bandwidth());
        self.previous_coded_channels = usize::from(toc.channels());
        self.previous_redundancy_end = false;
        self.hybrid_filter_pending = toc.mode() == Mode::Hybrid && !direct;
        self.final_range = decoder.range();
        self.write_output(
            recovered,
            &mut output[prefix * self.channels..samples * self.channels],
        )?;
        Ok(Recovery::Fec)
    }

    fn conceal_frame(&mut self, samples: usize) -> Result<(), Error> {
        self.frame_rate = 48_000;
        if self.previous_mode == Some(Mode::Celt) {
            let end = self.previous_bandwidth.map_or(21, celt_end);
            self.celt
                .conceal_layer(samples, 0, end, &mut self.frame[..samples * self.channels])?;
            self.previous_redundancy_end = false;
            return Ok(());
        }
        if let Some(mode) = self.previous_mode {
            let bandwidth = silk_bandwidth(self.previous_bandwidth.unwrap_or(Bandwidth::Wideband));
            self.prepare_lowband(bandwidth);
            let frame = silk::Frame {
                bandwidth,
                duration: if samples <= 480 {
                    silk::FrameDuration::Ms10
                } else {
                    silk::FrameDuration::Ms20
                },
            };
            let internal = if samples < 480 {
                self.silk.conceal_samples(
                    frame,
                    self.previous_coded_channels as u8,
                    samples / self.resamplers[0].ratio(),
                    &mut self.silk_pcm[..],
                )?
            } else {
                self.silk.conceal(
                    frame,
                    silk_duration(samples)?,
                    self.previous_coded_channels as u8,
                    &mut self.silk_pcm[..],
                )?
            };
            let direct = mode != Mode::Hybrid
                || self.sample_rate >= 48_000 / self.resamplers[0].ratio() as u32;
            self.render_lowband(internal, self.previous_coded_channels, samples, direct)?;
            if mode == Mode::Hybrid {
                self.celt.conceal_layer(
                    samples,
                    17,
                    self.previous_bandwidth.map_or(21, celt_end),
                    &mut self.lowband[0][..samples * self.channels],
                )?;
                self.add_highband(samples);
                self.hybrid_filter_pending = !direct;
            }
        } else {
            self.frame[..samples * self.channels].fill(0.0);
        }
        self.previous_redundancy_end = false;
        Ok(())
    }

    fn decode_frame(&mut self, bytes: &[u8], toc: Toc, samples: usize) -> Result<(), Error> {
        self.frame_rate = 48_000;
        let mode = toc.mode();
        let bandwidth = toc.bandwidth();
        if mode == Mode::Celt {
            self.hybrid_filter_pending = false;
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
        self.previous_coded_channels = usize::from(toc.channels());
        Ok(())
    }

    fn prepare_lowband(&mut self, bandwidth: silk::Bandwidth) {
        if self.previous_mode == Some(Mode::Celt) {
            self.silk.reset();
            self.internal_bandwidth = None;
        }
        if self.internal_bandwidth != Some(bandwidth) {
            for resampler in &mut self.resamplers {
                resampler.reset(bandwidth);
            }
            for delay in &mut self.native_delays {
                delay.reset(bandwidth);
            }
            self.internal_bandwidth = Some(bandwidth);
            self.output_resampler.reset();
            self.output_history_bypassed = false;
        }
    }

    fn render_lowband(
        &mut self,
        internal: usize,
        coded_channels: usize,
        samples: usize,
        direct: bool,
    ) -> Result<(), Error> {
        if internal * self.resamplers[0].ratio() != samples || internal > MAX_INTERNAL_SAMPLES {
            return Err(Error::Corrupt);
        }
        let mut channel_pcm = [0.0; MAX_INTERNAL_SAMPLES];
        let native_rate = 48_000 / self.resamplers[0].ratio() as u32;
        self.frame_rate = if direct { self.sample_rate } else { 48_000 };
        if !direct && self.output_history_bypassed && self.sample_rate < native_rate {
            let mut tail = [[0.0; OUTPUT_TAPS - 1]; 2];
            for (resampler, history) in self.resamplers[..self.channels]
                .iter()
                .zip(&mut tail[..self.channels])
            {
                resampler.tail_48k(history)?;
            }
            self.output_resampler.prime(&tail[..self.channels]);
            self.output_history_bypassed = false;
        }
        let rendered = samples * self.frame_rate as usize / 48_000;
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
            if self.sample_rate == native_rate {
                if self.frame_rate == native_rate {
                    self.native_delays[channel].process(
                        &channel_pcm[..internal],
                        &mut self.lowband[channel][..internal],
                    )?;
                } else {
                    let mut native = [0.0; MAX_INTERNAL_SAMPLES];
                    self.native_delays[channel]
                        .process(&channel_pcm[..internal], &mut native[..internal])?;
                    self.lowband[channel][..rendered].fill(0.0);
                    let factor = self.resamplers[channel].ratio();
                    for (index, &value) in native[..internal].iter().enumerate() {
                        self.lowband[channel][index * factor] = value;
                    }
                }
            } else if self.frame_rate == 48_000 {
                self.resamplers[channel].process(
                    &channel_pcm[..internal],
                    &mut self.lowband[channel][..rendered],
                )?;
            } else if self.sample_rate >= native_rate {
                self.resamplers[channel].process_rate(
                    &channel_pcm[..internal],
                    &mut self.lowband[channel][..rendered],
                    self.frame_rate,
                )?;
            } else {
                let index = usize::from(self.resamplers[channel].ratio() == 3);
                let plan = self.filter_plans[index].as_ref().ok_or(Error::Corrupt)?;
                self.resamplers[channel].process_filtered(
                    &channel_pcm[..internal],
                    &mut self.lowband[channel][..rendered],
                    plan,
                    self.channels == 1 && internal >= 320,
                )?;
            }
        }
        match self.channels {
            1 => self.frame[..rendered].copy_from_slice(&self.lowband[0][..rendered]),
            2 => {
                for ((destination, &left), &right) in self.frame[..rendered * 2]
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(&self.lowband[0][..rendered])
                    .zip(&self.lowband[1][..rendered])
                {
                    *destination = [left, right];
                }
            }
            _ => unreachable!(),
        }
        if direct && self.frame_rate != 48_000 {
            self.output_history_bypassed = true;
        }
        Ok(())
    }

    fn decode_lp_frame(&mut self, bytes: &[u8], toc: Toc, samples: usize) -> Result<(), Error> {
        let mode = toc.mode();
        let bandwidth = silk_bandwidth(toc.bandwidth());
        self.prepare_lowband(bandwidth);
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
            &mut self.silk_pcm[..],
        )?;
        let coded_channels = usize::from(toc.channels());
        let redundant = redundancy(&mut decoder, mode, bytes.len())?;
        // A preceding full-band end frame can leave CELT overlap below the
        // Hybrid start band. Preserve the combined filter's delay and history
        // until that overlap has been discharged.
        let direct = redundant.is_none()
            && !(mode == Mode::Hybrid && self.previous_redundancy_end)
            && !self.hybrid_filter_pending;
        self.render_lowband(internal, coded_channels, samples, direct)?;
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
            self.add_highband(samples);
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
            self.add_highband(CROSSLAP);
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
        self.hybrid_filter_pending = false;
        self.final_range = decoder.range() ^ redundant_range;
        let rendered = samples * self.frame_rate as usize / 48_000;
        if self.frame[..rendered * self.channels]
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
    /// Output gain and the phase-inversion preference are retained.
    pub fn reset(&mut self) {
        self.silk.reset();
        self.celt.reset();
        self.internal_bandwidth = None;
        self.frame_rate = 48_000;
        self.output_resampler.reset();
        self.output_history_bypassed = false;
        self.hybrid_filter_pending = false;
        self.previous_mode = None;
        self.previous_bandwidth = None;
        self.previous_coded_channels = self.channels;
        self.previous_redundancy_end = false;
        self.final_range = 0;
    }

    /// Last consumed entropy interval. Ordinary packets report their last
    /// frame, XORed with any CELT transition redundancy. Recovery reports the
    /// header/LBRR prefix consumed from the following packet; independent PLC
    /// reports zero. This excludes random-state changes from concealment.
    pub const fn final_range(&self) -> u32 {
        self.final_range
    }

    /// Configured output sampling rate in Hz.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Configured interleaved output channel count.
    pub const fn channels(&self) -> usize {
        self.channels
    }

    /// Set output gain in signed Q8 decibels (256 units per dB).
    ///
    /// The default is zero. The full `i16` range is supported, with a linear
    /// factor of `10^(gain / 5120)`. Gain applies once after resampling to
    /// decoded, recovered and concealed samples, without changing codec history
    /// or the entropy interval. Floating output is not clipped to ±1; a
    /// non-finite gain product is rejected as [`Error::Corrupt`].
    /// [`Self::reset`] retains this setting.
    pub fn set_gain_q8(&mut self, gain: i16) {
        self.gain_q8 = gain;
        self.gain_factor = if gain == 0 {
            1.0
        } else {
            10.0f32.powf(f32::from(gain) / 5120.0)
        };
    }

    /// Configured output gain in signed Q8 decibels (256 units per dB).
    pub const fn gain_q8(&self) -> i16 {
        self.gain_q8
    }

    /// Permit opposite phase in CELT intensity-stereo bands.
    /// Enabled for stereo output by default, disabled for mono output to retain
    /// high-frequency energy while downmixing. SILK reconstruction is unaffected.
    pub fn set_phase_inversion(&mut self, enabled: bool) {
        self.celt.set_phase_inversion(enabled);
    }

    /// Whether CELT intensity stereo can reconstruct opposite channel phases.
    pub fn phase_inversion(&self) -> bool {
        self.celt.phase_inversion()
    }
}

#[cfg(test)]
mod tests {
    use super::super::celt::{CeltConfig, CeltEncoder};
    use super::*;

    #[test]
    fn gain_retains_float_headroom_and_rejects_numeric_overflow() {
        let mut decoder = Decoder::new(1).unwrap();
        let mut output = [17.0; 127];
        decoder.frame[..120].fill(8.0);
        decoder.set_gain_q8(i16::MAX);
        decoder.write_output(120, &mut output).unwrap();
        assert!(output[..120].iter().all(|&v| v.is_finite() && v > 1.0));
        assert_eq!(&output[120..], &[17.0; 7]);
        assert!(decoder.frame[..120].iter().all(|&v| v == 8.0));

        // A corrupt numerical state cannot become successful infinite output.
        // This directly exercises the output boundary, independently of how
        // entropy, shape normalization and stable filters bound packet PCM.
        decoder.frame[0] = f32::MAX;
        assert_eq!(decoder.write_output(120, &mut output), Err(Error::Corrupt));
        assert!(output.iter().all(|v| v.is_finite()));
        decoder.set_gain_q8(0);
        decoder.write_output(120, &mut output).unwrap();
        assert_eq!(output[0], f32::MAX);
        assert_eq!(&output[120..], &[17.0; 7]);
    }

    #[test]
    fn hybrid_low_rate_transition_and_loss_keep_combined_filter_history() {
        use super::super::{Application, Encoder, EncoderConfig, FrameDuration};

        for rate in [8_000, 12_000] {
            for channels in 1..=2 {
                let lp = EncoderConfig::new(
                    96_000 * channels as u32,
                    channels,
                    FrameDuration::Ms20,
                    Bandwidth::Narrowband,
                    Application::Voice,
                )
                .unwrap();
                let hybrid = EncoderConfig::new(
                    96_000 * channels as u32,
                    channels,
                    FrameDuration::Ms20,
                    Bandwidth::Fullband,
                    Application::Voice,
                )
                .unwrap();
                let mut encoder = Encoder::new(lp).unwrap();
                let mut direct = Decoder::with_sample_rate(channels, rate).unwrap();
                let mut full = Decoder::with_sample_rate(channels, rate).unwrap();
                let mut input = [0.0; 1920];
                let mut packet = [0; 4096];
                let mut actual = [0.0; 1920];
                let mut expected = [0.0; 1920];
                for part in 0..6 {
                    if part == 1 {
                        encoder.prepare_reconfigure(hybrid).unwrap();
                    }
                    if part == 3 {
                        let missing = rate as usize * 3 / 400;
                        direct.conceal(missing, &mut actual).unwrap();
                        full.conceal(missing, &mut expected).unwrap();
                        assert_eq!(actual[..missing * channels], expected[..missing * channels]);
                    }
                    for sample in 0..960 {
                        for channel in 0..channels {
                            input[sample * channels + channel] = 0.2
                                * ((part * 960 + sample) as f32
                                    * std::f32::consts::TAU
                                    * (180.0 + channel as f32 * 93.0)
                                    / 48_000.0)
                                    .sin();
                        }
                    }
                    let length = encoder
                        .encode(&input[..960 * channels], &mut packet)
                        .unwrap();
                    if part == 1 {
                        assert!(encoder.has_transition_redundancy());
                    }
                    // Force the literal complete LP+CELT filter in the oracle
                    // path, including packets that ordinarily use fusion.
                    full.hybrid_filter_pending = true;
                    let count = direct.decode(&packet[..length], &mut actual).unwrap();
                    full.decode(&packet[..length], &mut expected).unwrap();
                    assert_eq!(direct.final_range(), full.final_range());
                    for (&actual, &expected) in actual[..count * channels]
                        .iter()
                        .zip(&expected[..count * channels])
                    {
                        assert!((actual - expected).abs() < 2.0e-7);
                    }
                }
            }
        }
    }

    #[test]
    fn native_rate_delay_and_sparse_mixed_output_match_literal_history() {
        for (bandwidth, rate, delay) in [
            (silk::Bandwidth::Narrow, 8000, 4),
            (silk::Bandwidth::Medium, 12000, 9),
            (silk::Bandwidth::Wide, 16000, 12),
        ] {
            for channels in [1, 2] {
                for coded in [1, 2] {
                    let mut decoder = Decoder::with_sample_rate(channels, rate).unwrap();
                    decoder.prepare_lowband(bandwidth);
                    let factor = 48000 / rate as usize;
                    let mut history = [Vec::<f32>::new(), Vec::new()];
                    let mut output = [42.0; 5760];
                    for part in 0..18 {
                        let samples = [120, 240, 480, 960, 2880][part % 5];
                        let internal = samples / factor;
                        for i in 0..internal {
                            for ch in 0..coded {
                                decoder.silk_pcm[i * coded + ch] =
                                    ((part * 197 + i * 23 + ch * 17) % 251) as f32 / 1024.0 - 0.125;
                            }
                        }
                        let mut expected = [Vec::new(), Vec::new()];
                        for (ch, (past, expected)) in history[..channels]
                            .iter_mut()
                            .zip(&mut expected[..channels])
                            .enumerate()
                        {
                            for i in 0..internal {
                                let value = if coded == 1 {
                                    decoder.silk_pcm[i]
                                } else if channels == 1 {
                                    0.5 * (decoder.silk_pcm[2 * i] + decoder.silk_pcm[2 * i + 1])
                                } else {
                                    decoder.silk_pcm[2 * i + ch]
                                };
                                past.push(value);
                                let n = past.len();
                                expected.push(if n > delay { past[n - delay - 1] } else { 0.0 });
                            }
                        }
                        let direct = part % 3 != 2;
                        decoder
                            .render_lowband(internal, coded, samples, direct)
                            .unwrap();
                        decoder.write_output(samples, &mut output).unwrap();
                        for i in 0..internal {
                            for (ch, expected) in expected[..channels].iter().enumerate() {
                                assert_eq!(output[i * channels + ch], expected[i]);
                            }
                        }
                        assert!(output[internal * channels..].iter().all(|v| *v == 42.0));
                        output.fill(42.0);
                        if part == 8 {
                            decoder.reset();
                            decoder.prepare_lowband(bandwidth);
                            for v in &mut history {
                                v.clear();
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_filtering_and_primed_transitions_match_full_rate_composition() {
        for (bandwidth, rate) in [
            (silk::Bandwidth::Medium, 8_000),
            (silk::Bandwidth::Wide, 8_000),
            (silk::Bandwidth::Wide, 12_000),
        ] {
            for channels in 1..=2 {
                let mut direct = Decoder::with_sample_rate(channels, rate).unwrap();
                let mut full = Decoder::with_sample_rate(channels, rate).unwrap();
                for decoder in [&mut direct, &mut full] {
                    decoder.previous_mode = Some(Mode::Silk);
                    decoder.previous_bandwidth = Some(match bandwidth {
                        silk::Bandwidth::Medium => Bandwidth::Mediumband,
                        silk::Bandwidth::Wide => Bandwidth::Wideband,
                        _ => unreachable!(),
                    });
                    decoder.prepare_lowband(bandwidth);
                }
                let factor = direct.resamplers[0].ratio();
                let mut actual = [42.0; 5761];
                let mut expected = [42.0; 5761];
                let mut time = 0;
                for part in 0..18 {
                    let samples = [120, 240, 480, 960, 2880][part % 5];
                    let internal = samples / factor;
                    let coded_channels = part % 2 + 1;
                    for sample in 0..internal {
                        for channel in 0..coded_channels {
                            let value =
                                ((time + sample * 43 + channel * 61) % 257) as f32 / 257.0 - 0.5;
                            direct.silk_pcm[sample * coded_channels + channel] = value;
                            full.silk_pcm[sample * coded_channels + channel] = value;
                        }
                    }
                    // Both paths resume the complete output FIR on this third
                    // packet; the direct path reconstructs its prior tail.
                    direct
                        .render_lowband(internal, coded_channels, samples, part % 3 != 2)
                        .unwrap();
                    full.render_lowband(internal, coded_channels, samples, false)
                        .unwrap();
                    direct.write_output(samples, &mut actual).unwrap();
                    full.write_output(samples, &mut expected).unwrap();
                    let count = samples * rate as usize / 48_000 * channels;
                    for (&actual, &expected) in actual[..count].iter().zip(&expected[..count]) {
                        assert!((actual - expected).abs() < 2.0e-7);
                    }
                    assert_eq!(actual[count], 42.0);
                    actual[..count].fill(42.0);
                    expected[..count].fill(42.0);
                    time += internal;
                }
            }
        }
    }

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
    fn standard_output_rates_preserve_packet_timing_and_decimation_phase() {
        for channels in 1..=2 {
            let mut encoder = CeltEncoder::new(
                CeltConfig::new(
                    48_000,
                    celt::CeltFrameDuration::Ms20,
                    channels,
                    Bandwidth::Narrowband,
                )
                .unwrap(),
            );
            let rates = [8_000, 12_000, 16_000, 24_000, 48_000];
            let mut decoders: Vec<_> = rates
                .into_iter()
                .map(|rate| Decoder::with_sample_rate(channels, rate).unwrap())
                .collect();
            let mut input = [0.0; 1920];
            let mut packet = [0; 1276];
            let mut full = [0.0; 1920];
            let mut reduced = [42.0; 1920];
            for frame in 0..6 {
                for sample in 0..960 {
                    for channel in 0..channels {
                        let time = (frame * 960 + sample) as f32 / 48_000.0;
                        input[sample * channels + channel] = 0.2
                            * (std::f32::consts::TAU * (180 + channel * 93) as f32 * time).sin();
                    }
                }
                let length = encoder
                    .encode(&input[..960 * channels], &mut packet)
                    .unwrap();
                decoders[4].decode(&packet[..length], &mut full).unwrap();
                for (decoder, rate) in decoders[..4].iter_mut().zip(rates) {
                    reduced.fill(42.0);
                    let count = decoder.decode(&packet[..length], &mut reduced).unwrap();
                    assert_eq!(count, rate as usize / 50);
                    assert_eq!(decoder.sample_rate(), rate);
                    assert_eq!(decoder.channels(), channels);
                    assert_eq!(decoder.final_range(), encoder.final_range());
                    let factor = 48_000 / rate as usize;
                    for sample in 0..count {
                        for channel in 0..channels {
                            assert_eq!(
                                reduced[sample * channels + channel],
                                full[sample * factor * channels + channel]
                            );
                        }
                    }
                    assert!(
                        reduced[count * channels..]
                            .iter()
                            .all(|&sample| sample == 42.0)
                    );
                }
            }
        }
        assert!(matches!(
            Decoder::with_sample_rate(1, 44_100),
            Err(Error::Argument)
        ));
    }

    #[test]
    fn explicit_loss_validates_before_state_and_preserves_periodic_audio() {
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        let mut decoder = Decoder::new(1).unwrap();
        let mut input = [0.0; 960];
        let mut packet = [0; 1276];
        let mut output = [0.0; 960];
        for frame in 0..8 {
            for (sample, value) in input.iter_mut().enumerate() {
                *value = 0.2
                    * (std::f32::consts::TAU * 200.0 * (frame * 960 + sample) as f32 / 48_000.0)
                        .sin();
            }
            let length = encoder.encode(&input, &mut packet).unwrap();
            decoder.decode(&packet[..length], &mut output).unwrap();
        }
        let range = decoder.final_range();
        assert_eq!(decoder.conceal(119, &mut output), Err(Error::Argument));
        assert_eq!(decoder.conceal(960, &mut output[..959]), Err(Error::Output));
        assert_eq!(decoder.final_range(), range);
        let previous_rms =
            (output.iter().map(|sample| sample * sample).sum::<f32>() / 960.0).sqrt();
        decoder.conceal(960, &mut output).unwrap();
        assert_eq!(decoder.final_range(), 0);
        let first_rms = (output.iter().map(|sample| sample * sample).sum::<f32>() / 960.0).sqrt();
        assert!(
            first_rms > previous_rms * 0.8 && first_rms < previous_rms * 1.2,
            "short loss changed RMS {previous_rms} -> {first_rms}"
        );
        for _ in 0..20 {
            decoder.conceal(960, &mut output).unwrap();
            assert!(
                output
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() < 0.5)
            );
        }
        let last_rms = (output.iter().map(|sample| sample * sample).sum::<f32>() / 960.0).sqrt();
        assert!(last_rms < first_rms * 0.05);
        decoder.reset();
        decoder.conceal(960, &mut output).unwrap();
        assert_eq!(output, [0.0; 960]);
    }

    #[test]
    fn absent_fec_matches_explicit_concealment_and_keeps_current_packet_available() {
        let mut encoder = CeltEncoder::new(CeltConfig::default());
        let mut first = Decoder::new(1).unwrap();
        let mut second = Decoder::new(1).unwrap();
        let mut packet = [0; 1276];
        let mut a = [0.0; 960];
        let mut b = [0.0; 960];
        let input = std::array::from_fn::<_, 960, _>(|sample| 0.2 * (sample as f32 * 0.071).sin());
        for _ in 0..4 {
            let length = encoder.encode(&input, &mut packet).unwrap();
            first.decode(&packet[..length], &mut a).unwrap();
            second.decode(&packet[..length], &mut b).unwrap();
        }
        let length = encoder.encode(&input, &mut packet).unwrap();
        assert_eq!(
            first.recover(&packet[..length], 960, &mut a).unwrap(),
            Recovery::Concealed
        );
        second.conceal(960, &mut b).unwrap();
        assert_eq!(a, b);
        first.decode(&packet[..length], &mut a).unwrap();
        second.decode(&packet[..length], &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(first.final_range(), encoder.final_range());
    }

    #[test]
    fn mono_intensity_stereo_preserves_energy_and_phase_preference_survives_reset() {
        // Synthetic 137 Hz harmonic signal, generated by the native mobile
        // interoperability tool: stereo WB, 2.5 ms, 96 kb/s. Its intensity bands
        // contain inversion flags. No foreign audio is stored in this fixture.
        let packet = include_bytes!("testdata/stereo-phase.opus");
        let mut mono = Decoder::with_sample_rate(1, 12_000).unwrap();
        let mut inverted = Decoder::with_sample_rate(1, 12_000).unwrap();
        let mut stereo = Decoder::with_sample_rate(2, 12_000).unwrap();
        assert!(!mono.phase_inversion());
        assert!(stereo.phase_inversion());
        inverted.set_phase_inversion(true);
        stereo.set_phase_inversion(false);
        let mut a = [0.0; 30];
        let mut b = [0.0; 30];
        let mut c = [0.0; 60];
        mono.decode(packet, &mut a).unwrap();
        inverted.decode(packet, &mut b).unwrap();
        stereo.decode(packet, &mut c).unwrap();
        assert!(a.iter().zip(&b).any(|(a, b)| (a - b).abs() > 0.002));
        for (&actual, pair) in a.iter().zip(c.as_chunks::<2>().0) {
            assert!((actual - 0.5 * (pair[0] + pair[1])).abs() < 1e-7);
        }
        assert_eq!(mono.final_range(), inverted.final_range());
        assert_eq!(mono.final_range(), stereo.final_range());
        inverted.reset();
        stereo.reset();
        assert!(inverted.phase_inversion());
        assert!(!stereo.phase_inversion());
    }

    #[test]
    fn silk_redundancy_and_partial_losses_preserve_following_packet_ranges() {
        // Existing independently generated native corpus: all SILK bandwidths,
        // durations and channel layouts, with actual LBRR payloads.
        let data = include_bytes!("../../tests/fixtures/opus-silk-decode.bin");
        let cases = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let mut decoder = Decoder::new(2).unwrap();
        let mut output = [0.0; 5760];
        let mut pending: Option<Vec<u8>> = None;
        let mut position = 12;
        let mut recovered = false;
        let mut recoveries = 0;
        for _ in 0..cases {
            let band = data[position + 2];
            let duration = data[position + 3];
            let channels = data[position + 4];
            let length =
                u16::from_le_bytes(data[position + 5..position + 7].try_into().unwrap()) as usize;
            let native_samples =
                u16::from_le_bytes(data[position + 7..position + 9].try_into().unwrap()) as usize;
            let range = u32::from_le_bytes(data[position + 9..position + 13].try_into().unwrap());
            let reset =
                u32::from_le_bytes(data[position + 13..position + 17].try_into().unwrap()) != 0;
            position += 21;
            if reset {
                decoder.reset();
                pending = None;
                recovered = false;
            }
            let payload = &data[position..position + length];
            let count = match duration {
                10 => 0,
                20 => 1,
                40 => 2,
                60 => 3,
                _ => unreachable!(),
            };
            let toc = ((band * 4 + count) << 3) | ((channels - 1) << 2);
            let mut packet = vec![toc];
            packet.extend_from_slice(payload);
            let samples = duration as usize * 48;
            let header = silk::Header::decode(
                &mut range::Decoder::new(payload),
                silk_duration(samples).unwrap(),
                channels,
            )
            .unwrap();
            let fec = (0..3).any(|interval| header.redundancy(0, interval) == Some(true));
            if recovered {
                decoder.decode(&packet, &mut output).unwrap();
                assert_eq!(decoder.final_range(), range);
            } else if fec && pending.is_some() {
                assert_eq!(
                    decoder.recover(&packet, samples, &mut output).unwrap(),
                    Recovery::Fec
                );
                assert!(
                    output[..samples * 2]
                        .iter()
                        .all(|sample| sample.is_finite())
                );
                decoder.decode(&packet, &mut output).unwrap();
                assert_eq!(decoder.final_range(), range);
                // A sub-frame loss advances precisely 2.5 ms of LP state.
                decoder.conceal(120, &mut output).unwrap();
                assert!(output[..240].iter().all(|sample| sample.is_finite()));
                recovered = true;
                recoveries += 1;
                pending = None;
            } else {
                if let Some(previous) = pending.take() {
                    decoder.decode(&previous, &mut output).unwrap();
                }
                pending = Some(packet);
            }
            position += length + native_samples * usize::from(channels) * 2;
        }
        assert_eq!(position, data.len());
        assert!(
            recoveries >= 12,
            "fixture must cover recovery across native SILK geometry: {recoveries}"
        );
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
        mono.set_phase_inversion(true);
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
