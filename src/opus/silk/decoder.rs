// SPDX-License-Identifier: MIT
//! Stateful SILK layer decoding at its internal sampling rate.

use super::frame::{ChannelMetadata, CodingMode, StereoParameters, decode_stereo};
use super::synthesis::{Parameters, Synthesis};
use super::{
    Bandwidth, Error, Frame, FrameDuration, FrameType, Header, NlsfState, PacketDuration,
    SignalType, decode_excitation, decode_nlsf_indices, decode_pulses,
};
use crate::entropy::{Decoder as RangeDecoder, Model};

const MAX_SAMPLES: usize = 320;
const SEED: Model<'static> = Model::from_static(&[0, 64, 128, 192, 256]);

struct Channel {
    metadata: ChannelMetadata,
    nlsf: NlsfState,
    synthesis: Synthesis,
    coded: bool,
    previous: Option<Parameters>,
    lost_samples: u32,
    conceal_seed: u32,
}

impl Channel {
    fn new() -> Self {
        Self {
            metadata: ChannelMetadata::new(),
            nlsf: NlsfState::new(),
            synthesis: Synthesis::new(),
            coded: false,
            previous: None,
            lost_samples: 0,
            conceal_seed: 0x9e37_79b9,
        }
    }

    fn reset(&mut self) {
        self.metadata = ChannelMetadata::new();
        self.nlsf.reset();
        self.synthesis.reset();
        self.coded = false;
        self.previous = None;
        self.lost_samples = 0;
        self.conceal_seed = 0x9e37_79b9;
    }

    fn decode(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        active: bool,
        mode: CodingMode,
        output: &mut [f32],
    ) -> Result<(), Error> {
        self.decode_segment(decoder, frame, active, mode, 0..frame.samples(), output)
    }

    fn decode_segment(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        active: bool,
        mode: CodingMode,
        segment: std::ops::Range<usize>,
        output: &mut [f32],
    ) -> Result<(), Error> {
        let origin = segment.start;
        let samples = segment.end.checked_sub(origin).ok_or(Error::Argument)?;
        if segment.end > frame.samples() {
            return Err(Error::Argument);
        }
        if output.len() < samples {
            return Err(Error::Output);
        }
        let kind = FrameType::decode(decoder, active);
        let gains = self
            .metadata
            .decode_gains(decoder, frame, kind.signal, mode)?;
        let lpc = self.nlsf.decode(decoder, frame, kind.signal)?;
        let pitch = self
            .metadata
            .decode_pitch(decoder, frame, kind.signal, mode);
        let mut excitation = [0; MAX_SAMPLES];
        decode_excitation(decoder, frame, kind, &mut excitation)?;
        let parameters = Parameters {
            gains_q16: gains,
            lpc_q12: [lpc.first, lpc.second],
            interpolation: lpc.interpolation,
            voiced: kind.signal == SignalType::Voiced,
            pitch_lags: pitch.lags,
            ltp_q14: pitch.coefficients,
            ltp_scale_q14: pitch.scale,
        };
        if origin == 0 && samples == frame.samples() {
            self.synthesis
                .frame(frame, &parameters, &excitation, output)?;
        } else {
            self.synthesis
                .frame_slice(frame, &parameters, &excitation, origin, samples, output)?;
        }
        self.coded = true;
        if samples == 0 {
            return Ok(());
        }
        self.previous = Some(parameters);
        self.lost_samples = 0;
        self.conceal_seed = self
            .conceal_seed
            .wrapping_add_signed(excitation[origin + samples - 1]);
        Ok(())
    }

    fn conceal(&mut self, frame: Frame, output: &mut [f32]) -> Result<(), Error> {
        self.conceal_samples(frame, frame.samples(), output)
    }

    fn conceal_samples(
        &mut self,
        frame: Frame,
        samples: usize,
        output: &mut [f32],
    ) -> Result<(), Error> {
        if output.len() < samples {
            return Err(Error::Output);
        }
        self.lost_samples = self.lost_samples.saturating_add(samples as u32);
        if let Some(parameters) = self.previous {
            let rate = match frame.bandwidth {
                Bandwidth::Narrow => 8_000.0,
                Bandwidth::Medium => 12_000.0,
                Bandwidth::Wide => 16_000.0,
            };
            let attenuation = 0.9f64.powf(f64::from(self.lost_samples) / (0.02 * rate));
            self.synthesis.conceal_samples(
                frame,
                samples,
                &parameters,
                attenuation,
                &mut self.conceal_seed,
                output,
            )?;
        } else {
            output[..samples].fill(0.0);
        }
        self.coded = false;
        Ok(())
    }
}

#[derive(Default)]
struct Stereo {
    mid: [f32; 2],
    side: f32,
    prediction: [i16; 2],
}

impl Stereo {
    fn mono(&mut self, mid: &[f32], output: &mut [f32]) {
        for (&current, sample) in mid.iter().zip(output) {
            *sample = self.mid[1];
            self.mid = [self.mid[1], current];
        }
        self.side = 0.0;
    }

    fn unmix(
        &mut self,
        frame: Frame,
        weights: [i16; 2],
        mid: &[f32],
        side: &[f32],
        output: &mut [f32],
    ) {
        self.unmix_segment(frame, weights, mid, side, 0, output)
    }

    fn unmix_segment(
        &mut self,
        frame: Frame,
        weights: [i16; 2],
        mid: &[f32],
        side: &[f32],
        origin: usize,
        output: &mut [f32],
    ) {
        let transition = match frame.bandwidth {
            Bandwidth::Narrow => 64,
            Bandwidth::Medium => 96,
            Bandwidth::Wide => 128,
        };
        for (i, (&current_mid, &current_side)) in mid.iter().zip(side).enumerate() {
            let ratio = (origin + i).min(transition) as f64 / transition as f64;
            let weight = |index: usize| {
                (f64::from(self.prediction[index])
                    + ratio
                        * f64::from(i32::from(weights[index]) - i32::from(self.prediction[index])))
                    / 8192.0
            };
            let low_pass =
                (f64::from(self.mid[0]) + 2.0 * f64::from(self.mid[1]) + f64::from(current_mid))
                    * 0.25;
            let middle = f64::from(self.mid[1]);
            let predicted_side = f64::from(self.side) + weight(0) * low_pass + weight(1) * middle;
            output[2 * i] = (middle + predicted_side).clamp(-1.0, 1.0) as f32;
            output[2 * i + 1] = (middle - predicted_side).clamp(-1.0, 1.0) as f32;
            self.mid = [self.mid[1], current_mid];
            self.side = current_side;
        }
        self.prediction = weights;
    }
}

/// An owned, allocation-free SILK decoder at 8, 12 or 16 kHz.
///
/// This decodes regular internal SILK frames and consumes all preceding
/// low-bitrate redundancy. It outputs normalized mono or interleaved stereo
/// PCM, including the one-sample stereo-unmixing delay. Resampling, Opus
/// mode transitions and packet-loss recovery belong to the enclosing codec.
pub struct SilkDecoder {
    channels: [Channel; 2],
    bandwidth: Option<Bandwidth>,
    input_channels: u8,
    stereo: Stereo,
}

impl Default for SilkDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SilkDecoder {
    /// Create a decoder with empty channel histories.
    pub fn new() -> Self {
        Self {
            channels: [Channel::new(), Channel::new()],
            bandwidth: None,
            input_channels: 0,
            stereo: Stereo::default(),
        }
    }

    /// Clear all prediction, stereo and sampling-rate history.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn prepare(&mut self, frame: Frame, channels: u8) {
        if self.bandwidth != Some(frame.bandwidth) {
            self.reset();
            self.bandwidth = Some(frame.bandwidth);
        }
        if self.input_channels != channels {
            self.channels[1].reset();
            self.stereo.side = 0.0;
            self.stereo.prediction = [0; 2];
        }
        self.input_channels = channels;
    }

    /// Conceal one missing enclosing frame at the layer's internal rate.
    /// The independent LPC/LTP extrapolator updates both synthesis and stereo
    /// histories, so the following coded packet resumes from the concealed audio.
    /// Invalid geometry and capacity leave state and output untouched.
    pub fn conceal(
        &mut self,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        let total = validate_geometry(frame, duration, channels, output.len())?;
        self.prepare(frame, channels);
        let samples = frame.samples();
        let mut mid = [0.0; MAX_SAMPLES];
        let mut side = [0.0; MAX_SAMPLES];
        for interval in 0..duration.frames() {
            self.channels[0].conceal(frame, &mut mid)?;
            if channels == 2 {
                self.channels[1].conceal(frame, &mut side)?;
            }
            let begin = interval * samples * usize::from(channels);
            let destination = &mut output[begin..begin + samples * usize::from(channels)];
            if channels == 1 {
                self.stereo.mono(&mid[..samples], destination);
            } else {
                self.stereo.unmix(
                    frame,
                    self.stereo.prediction,
                    &mid[..samples],
                    &side[..samples],
                    destination,
                );
            }
        }
        Ok(total)
    }

    /// Conceal exactly a multiple of 2.5 ms, up to 20 ms, without advancing
    /// prediction or stereo history beyond the returned native-rate samples.
    pub fn conceal_samples(
        &mut self,
        frame: Frame,
        channels: u8,
        samples: usize,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        let quantum = match frame.bandwidth {
            Bandwidth::Narrow => 20,
            Bandwidth::Medium => 30,
            Bandwidth::Wide => 40,
        };
        if !(1..=2).contains(&channels) || samples > quantum * 8 || !samples.is_multiple_of(quantum)
        {
            return Err(Error::Argument);
        }
        if output.len() < samples * usize::from(channels) {
            return Err(Error::Output);
        }
        if samples == 0 {
            return Ok(0);
        }
        self.prepare(frame, channels);
        let mut mid = [0.0; MAX_SAMPLES];
        let mut side = [0.0; MAX_SAMPLES];
        self.channels[0].conceal_samples(frame, samples, &mut mid)?;
        if channels == 2 {
            self.channels[1].conceal_samples(frame, samples, &mut side)?;
        }
        if channels == 1 {
            self.stereo.mono(&mid[..samples], &mut output[..samples]);
        } else {
            self.stereo.unmix(
                frame,
                self.stereo.prediction,
                &mid[..samples],
                &side[..samples],
                &mut output[..samples * 2],
            );
        }
        Ok(samples)
    }

    /// Recover a complete preceding signal from low-bitrate redundancy.
    /// The following packet defines its mode, bandwidth, channels and duration.
    /// Missing pieces are concealed; absent redundancy returns `None` without
    /// changing output or audio history. Reinitialize entropy before decoding
    /// that same packet's regular signal.
    pub fn decode_fec(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        output: &mut [f32],
    ) -> Result<Option<usize>, Error> {
        let samples = validate_geometry(frame, duration, channels, output.len())?;
        self.decode_fec_tail(decoder, frame, duration, channels, samples, output)
    }

    /// Recover the last `samples` native-rate samples represented by LBRR.
    /// Requested duration must be a positive 2.5 ms multiple no longer than the
    /// current enclosing frame. Earlier LBRR symbols are consumed to preserve
    /// conditional coding, while only the requested tail advances synthesis.
    /// Redundancy outside that tail does not count as recovered audio.
    pub fn decode_fec_tail(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        samples: usize,
        output: &mut [f32],
    ) -> Result<Option<usize>, Error> {
        let total = validate_geometry(frame, duration, channels, usize::MAX)?;
        let quantum = match frame.bandwidth {
            Bandwidth::Narrow => 20,
            Bandwidth::Medium => 30,
            Bandwidth::Wide => 40,
        };
        if samples == 0 || samples > total || !samples.is_multiple_of(quantum) {
            return Err(Error::Argument);
        }
        if output.len() < samples * usize::from(channels) {
            return Err(Error::Output);
        }
        let width = frame.samples();
        let skip = total - samples;
        let header = Header::decode(decoder, duration, channels)?;
        let present = (0..duration.frames()).any(|interval| {
            (interval + 1) * width > skip
                && (0..usize::from(channels))
                    .any(|channel| header.redundancy(channel, interval) == Some(true))
        });
        if !present {
            return Ok(None);
        }
        self.prepare(frame, channels);
        let mut mid = [0.0; MAX_SAMPLES];
        let mut side = [0.0; MAX_SAMPLES];
        let mut previous = [false; 2];
        let mut emitted = 0;
        for interval in 0..duration.frames() {
            let origin = skip.saturating_sub(interval * width).min(width);
            let count = width - origin;
            let coded = [
                header.redundancy(0, interval).unwrap(),
                channels == 2 && header.redundancy(1, interval).unwrap(),
            ];
            let stereo = if channels == 2 && coded[0] {
                decode_stereo(decoder, coded[1])
            } else {
                StereoParameters {
                    prediction_q13: self.stereo.prediction,
                    mid_only: channels == 1,
                }
            };
            if coded[0] {
                self.channels[0].decode_segment(
                    decoder,
                    frame,
                    true,
                    if previous[0] {
                        CodingMode::Conditional
                    } else {
                        CodingMode::Independent
                    },
                    origin..origin + count,
                    &mut mid,
                )?;
            } else if count != 0 {
                self.channels[0].conceal_samples(frame, count, &mut mid)?;
            }
            if channels == 2 {
                if coded[1] {
                    self.channels[1].decode_segment(
                        decoder,
                        frame,
                        true,
                        if previous[1] {
                            CodingMode::Conditional
                        } else {
                            CodingMode::Independent
                        },
                        origin..origin + count,
                        &mut side,
                    )?;
                } else if count != 0 {
                    if stereo.mid_only {
                        side[..count].fill(0.0);
                        self.channels[1].reset();
                    } else {
                        self.channels[1].conceal_samples(frame, count, &mut side)?;
                    }
                }
            }
            previous = coded;
            if count == 0 {
                if coded[0] && channels == 2 {
                    self.stereo.prediction = stereo.prediction_q13;
                }
                continue;
            }
            let destination = &mut output
                [emitted * usize::from(channels)..(emitted + count) * usize::from(channels)];
            if channels == 1 {
                self.stereo.mono(&mid[..count], destination);
            } else {
                self.stereo.unmix_segment(
                    frame,
                    stereo.prediction_q13,
                    &mid[..count],
                    &side[..count],
                    origin,
                    destination,
                );
            }
            emitted += count;
        }
        debug_assert_eq!(emitted, samples);
        Ok(Some(samples))
    }

    /// Decode one SILK layer from the enclosing frame's entropy decoder.
    ///
    /// `frame` gives the internal bandwidth and the 10/20 ms frame duration.
    /// `duration` gives the entire enclosing 10/20/40/60 ms Opus frame.
    /// `output` must hold `frame.samples() * internal_frames * channels`
    /// values. The return value counts samples per channel. Invalid geometry
    /// or output capacity fails before consuming entropy or changing history.
    pub fn decode(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        output: &mut [f32],
    ) -> Result<usize, Error> {
        let total = validate_geometry(frame, duration, channels, output.len())?;
        let count = duration.frames();
        let samples = frame.samples();
        let channel_count = usize::from(channels);
        self.prepare(frame, channels);
        let header = Header::decode(decoder, duration, channels)?;
        consume_redundancy(decoder, frame, header)?;

        let mut mid = [0.0; MAX_SAMPLES];
        let mut side = [0.0; MAX_SAMPLES];
        for interval in 0..count {
            let stereo = if channels == 2 {
                decode_stereo(decoder, header.voice_activity(1, interval).unwrap())
            } else {
                StereoParameters {
                    prediction_q13: [0; 2],
                    mid_only: true,
                }
            };
            self.channels[0].decode(
                decoder,
                frame,
                header.voice_activity(0, interval).unwrap(),
                regular_mode(interval, self.channels[0].coded),
                &mut mid,
            )?;
            if channels == 2 && !stereo.mid_only {
                self.channels[1].decode(
                    decoder,
                    frame,
                    header.voice_activity(1, interval).unwrap(),
                    regular_mode(interval, self.channels[1].coded),
                    &mut side,
                )?;
            } else {
                side.fill(0.0);
                self.channels[1].reset();
            }
            let begin = interval * samples * channel_count;
            let frame_output = &mut output[begin..begin + samples * channel_count];
            if channels == 1 {
                self.stereo.mono(&mid[..samples], frame_output);
            } else {
                self.stereo.unmix(
                    frame,
                    stereo.prediction_q13,
                    &mid[..samples],
                    &side[..samples],
                    frame_output,
                );
            }
        }
        Ok(total)
    }
}

fn validate_geometry(
    frame: Frame,
    duration: PacketDuration,
    channels: u8,
    capacity: usize,
) -> Result<usize, Error> {
    if !(1..=2).contains(&channels)
        || (frame.duration == FrameDuration::Ms10) != (duration == PacketDuration::Ms10)
    {
        return Err(Error::Argument);
    }
    let total = frame.samples() * duration.frames();
    if capacity < total * usize::from(channels) {
        return Err(Error::Output);
    }
    Ok(total)
}

fn regular_mode(interval: usize, previous_coded: bool) -> CodingMode {
    if interval == 0 {
        CodingMode::Independent
    } else if previous_coded {
        CodingMode::Conditional
    } else {
        CodingMode::IndependentNoLtpScale
    }
}

fn consume_redundancy(
    decoder: &mut RangeDecoder<'_>,
    frame: Frame,
    header: Header,
) -> Result<(), Error> {
    let mut metadata = [ChannelMetadata::new(), ChannelMetadata::new()];
    let mut previous = [false; 2];
    let mut pulses = [0; MAX_SAMPLES];
    for interval in 0..usize::from(header.frames) {
        for channel in 0..usize::from(header.channels) {
            let present = header.redundancy(channel, interval).unwrap();
            if present {
                if header.channels == 2 && channel == 0 {
                    let _ = decode_stereo(decoder, header.redundancy(1, interval).unwrap());
                }
                let kind = FrameType::decode(decoder, true);
                let mode = if previous[channel] {
                    CodingMode::Conditional
                } else {
                    CodingMode::Independent
                };
                let _ = metadata[channel].decode_gains(decoder, frame, kind.signal, mode)?;
                let _ = decode_nlsf_indices(decoder, frame, kind.signal);
                let _ = metadata[channel].decode_pitch(decoder, frame, kind.signal, mode);
                let _ = decoder.symbol(SEED);
                decode_pulses(decoder, frame, kind, &mut pulses)?;
            }
            previous[channel] = present;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redundancy_tails_consume_full_symbols_but_emit_only_selected_time() {
        let data = include_bytes!("../../../tests/fixtures/opus-silk-decode.bin");
        let cases = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let mut position = 12;
        let mut recoveries = 0;
        for _ in 0..cases {
            let band = data[position + 2];
            let duration = data[position + 3];
            let channels = data[position + 4];
            let length =
                u16::from_le_bytes(data[position + 5..position + 7].try_into().unwrap()) as usize;
            let total =
                u16::from_le_bytes(data[position + 7..position + 9].try_into().unwrap()) as usize;
            position += 21;
            let payload = &data[position..position + length];
            let frame = Frame {
                bandwidth: match band {
                    0 => Bandwidth::Narrow,
                    1 => Bandwidth::Medium,
                    _ => Bandwidth::Wide,
                },
                duration: if duration == 10 {
                    FrameDuration::Ms10
                } else {
                    FrameDuration::Ms20
                },
            };
            let duration = match duration {
                10 => PacketDuration::Ms10,
                20 => PacketDuration::Ms20,
                40 => PacketDuration::Ms40,
                _ => PacketDuration::Ms60,
            };
            let header =
                Header::decode(&mut RangeDecoder::new(payload), duration, channels).unwrap();
            let mut full = SilkDecoder::new();
            let mut full_entropy = RangeDecoder::new(payload);
            let mut output = [0.0; 1920];
            full.decode_fec(&mut full_entropy, frame, duration, channels, &mut output)
                .unwrap();
            let quantum = frame.samples()
                / if frame.duration == FrameDuration::Ms10 {
                    4
                } else {
                    8
                };
            for samples in [quantum, quantum * 3, frame.samples(), total] {
                let skip = total - samples;
                let present = (0..duration.frames()).any(|interval| {
                    (interval + 1) * frame.samples() > skip
                        && (0..usize::from(channels))
                            .any(|channel| header.redundancy(channel, interval) == Some(true))
                });
                let mut tail = SilkDecoder::new();
                let mut entropy = RangeDecoder::new(payload);
                output.fill(42.0);
                let decoded = tail
                    .decode_fec_tail(
                        &mut entropy,
                        frame,
                        duration,
                        channels,
                        samples,
                        &mut output,
                    )
                    .unwrap();
                assert_eq!(decoded, present.then_some(samples));
                if present {
                    recoveries += 1;
                    assert_eq!(
                        (entropy.range(), entropy.tell()),
                        (full_entropy.range(), full_entropy.tell())
                    );
                    assert!(
                        output[..samples * usize::from(channels)]
                            .iter()
                            .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
                    );
                } else {
                    assert!(tail.bandwidth.is_none());
                    assert!(output.iter().all(|&sample| sample == 42.0));
                }
                assert!(
                    output[samples * usize::from(channels)..]
                        .iter()
                        .all(|&sample| sample == 42.0)
                );
            }
            position += length + total * usize::from(channels) * 2;
        }
        assert_eq!(position, data.len());
        assert!(
            recoveries > 300,
            "all native geometries must exercise selected redundancy tails"
        );
    }

    #[test]
    fn fec_consumes_actual_native_redundancy_and_missing_flags_do_not_mutate_audio() {
        let data = include_bytes!("../../../tests/fixtures/opus-silk-decode.bin");
        let cases = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let mut cursor = 12;
        let mut recovered = 0;
        let mut absent = 0;
        for _ in 0..cases {
            let band = data[cursor + 2];
            let ms = data[cursor + 3];
            let channels = data[cursor + 4];
            let length =
                u16::from_le_bytes(data[cursor + 5..cursor + 7].try_into().unwrap()) as usize;
            let samples =
                u16::from_le_bytes(data[cursor + 7..cursor + 9].try_into().unwrap()) as usize;
            cursor += 21;
            let payload = &data[cursor..cursor + length];
            cursor += length + samples * usize::from(channels) * 2;
            let frame = Frame {
                bandwidth: match band {
                    0 => Bandwidth::Narrow,
                    1 => Bandwidth::Medium,
                    _ => Bandwidth::Wide,
                },
                duration: if ms == 10 {
                    FrameDuration::Ms10
                } else {
                    FrameDuration::Ms20
                },
            };
            let duration = match ms {
                10 => PacketDuration::Ms10,
                20 => PacketDuration::Ms20,
                40 => PacketDuration::Ms40,
                _ => PacketDuration::Ms60,
            };
            let mut expected = RangeDecoder::new(payload);
            let header = Header::decode(&mut expected, duration, channels).unwrap();
            consume_redundancy(&mut expected, frame, header).unwrap();
            let mut silk = SilkDecoder::new();
            let mut decoder = RangeDecoder::new(payload);
            let mut output = [123.0; 1920];
            match silk
                .decode_fec(&mut decoder, frame, duration, channels, &mut output)
                .unwrap()
            {
                Some(count) => {
                    recovered += 1;
                    assert_eq!(count, samples);
                    assert!(
                        output[..samples * usize::from(channels)]
                            .iter()
                            .all(|value| value.is_finite() && value.abs() <= 1.0)
                    );
                    assert!(
                        silk.channels
                            .iter()
                            .any(|channel| channel.previous.is_some())
                    );
                }
                None => {
                    absent += 1;
                    assert_eq!(output, [123.0; 1920]);
                    assert_eq!(silk.bandwidth, None);
                    assert_eq!(silk.input_channels, 0);
                }
            }
            assert_eq!(
                (decoder.range(), decoder.tell(), decoder.tell_eighths()),
                (expected.range(), expected.tell(), expected.tell_eighths())
            );
        }
        assert!(recovered > 100, "recovered={recovered} absent={absent}");
        let frame = Frame {
            bandwidth: Bandwidth::Wide,
            duration: FrameDuration::Ms20,
        };
        let mut silk = SilkDecoder::new();
        let mut output = [123.0; 320];
        assert_eq!(
            silk.decode_fec(
                &mut RangeDecoder::new(&[0, 0]),
                frame,
                PacketDuration::Ms20,
                1,
                &mut output
            )
            .unwrap(),
            None
        );
        assert_eq!(output, [123.0; 320]);
        assert_eq!(silk.bandwidth, None);
        assert_eq!(cursor, data.len());
    }

    #[test]
    fn all_loss_geometries_are_bounded_and_partial_calls_keep_exact_sample_clock() {
        for (bandwidth, quantum) in [
            (Bandwidth::Narrow, 20),
            (Bandwidth::Medium, 30),
            (Bandwidth::Wide, 40),
        ] {
            for channels in [1, 2] {
                let mut silk = SilkDecoder::new();
                let frame = Frame {
                    bandwidth,
                    duration: FrameDuration::Ms20,
                };
                let mut output = [123.0; 1920];
                assert_eq!(
                    silk.conceal_samples(frame, channels, quantum, &mut output)
                        .unwrap(),
                    quantum
                );
                assert_eq!(silk.channels[0].lost_samples, quantum as u32);
                assert!(
                    output[..quantum * usize::from(channels)]
                        .iter()
                        .all(|&value| value == 0.0)
                );
                assert!(
                    output[quantum * usize::from(channels)..]
                        .iter()
                        .all(|&value| value == 123.0)
                );
                let before = silk.channels[0].lost_samples;
                assert_eq!(
                    silk.conceal_samples(frame, channels, quantum - 1, &mut output),
                    Err(Error::Argument)
                );
                assert_eq!(
                    silk.conceal_samples(frame, channels, quantum, &mut output[..quantum - 1]),
                    Err(Error::Output)
                );
                assert_eq!(silk.channels[0].lost_samples, before);
                silk.conceal_samples(frame, channels, 0, &mut []).unwrap();
                assert_eq!(silk.channels[0].lost_samples, before);
                for duration in [
                    PacketDuration::Ms20,
                    PacketDuration::Ms40,
                    PacketDuration::Ms60,
                ] {
                    let count = silk
                        .conceal(frame, duration, channels, &mut output)
                        .unwrap();
                    assert_eq!(count, frame.samples() * duration.frames());
                    assert!(
                        output[..count * usize::from(channels)]
                            .iter()
                            .all(|&value| value == 0.0)
                    );
                }
                let frame = Frame {
                    bandwidth,
                    duration: FrameDuration::Ms10,
                };
                assert_eq!(
                    silk.conceal(frame, PacketDuration::Ms10, channels, &mut output)
                        .unwrap(),
                    quantum * 4
                );
            }
        }
    }

    #[test]
    fn complete_coded_layers_match_native_ranges_and_time_aligned_pcm() {
        let data = include_bytes!("../../../tests/fixtures/opus-silk-decode.bin");
        assert_eq!(&data[..8], b"SNKSLD01");
        let cases = u32::from_le_bytes(data[8..12].try_into().unwrap());
        assert_eq!(cases, 456);
        let mut cursor = 12;
        let mut silk = SilkDecoder::new();
        let mut output = [0.0; 1920];
        let mut delayed = [0.0f32; 24];
        let mut position = 0;
        let mut errors = [0.0; 24];
        let mut energies = [0.0; 24];
        let mut redundancy_frames = 0;
        for case in 0..cases {
            let stream = u16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap()) as usize;
            let band = data[cursor + 2];
            let duration = data[cursor + 3];
            let channels = data[cursor + 4];
            let length =
                u16::from_le_bytes(data[cursor + 5..cursor + 7].try_into().unwrap()) as usize;
            let samples =
                u16::from_le_bytes(data[cursor + 7..cursor + 9].try_into().unwrap()) as usize;
            let range = u32::from_le_bytes(data[cursor + 9..cursor + 13].try_into().unwrap());
            let reset = u32::from_le_bytes(data[cursor + 13..cursor + 17].try_into().unwrap()) != 0;
            cursor += 21;
            if reset {
                silk.reset();
                delayed.fill(0.0);
                position = 0;
            }
            let frame = Frame {
                bandwidth: match band {
                    0 => Bandwidth::Narrow,
                    1 => Bandwidth::Medium,
                    _ => Bandwidth::Wide,
                },
                duration: if duration == 10 {
                    FrameDuration::Ms10
                } else {
                    FrameDuration::Ms20
                },
            };
            let duration = match duration {
                10 => PacketDuration::Ms10,
                20 => PacketDuration::Ms20,
                40 => PacketDuration::Ms40,
                _ => PacketDuration::Ms60,
            };
            let payload = &data[cursor..cursor + length];
            cursor += length;
            let header =
                Header::decode(&mut RangeDecoder::new(payload), duration, channels).unwrap();
            for channel in 0..usize::from(channels) {
                for interval in 0..duration.frames() {
                    redundancy_frames += usize::from(header.redundancy(channel, interval).unwrap());
                }
            }
            let mut entropy = RangeDecoder::new(payload);
            assert_eq!(
                silk.decode(&mut entropy, frame, duration, channels, &mut output)
                    .unwrap(),
                samples
            );
            assert_eq!(
                entropy.range(),
                range,
                "coded frame {case}, stream {stream}"
            );

            // The oracle's pass-through resampler at its internal sample rate
            // adds 4 NB, 9 MB or 12 WB samples. Our layer stops before that
            // non-normative stage, so compare after this fixed alignment.
            let delay = [4, 9, 12][usize::from(band)] * usize::from(channels);
            for (index, &actual) in output[..samples * usize::from(channels)].iter().enumerate() {
                let offset = cursor + index * 2;
                let expected = f64::from(i16::from_le_bytes(
                    data[offset..offset + 2].try_into().unwrap(),
                )) / 32768.0;
                let aligned = f64::from(delayed[position]);
                delayed[position] = actual;
                position = (position + 1) % delay;
                let difference = aligned - expected;
                errors[stream] += difference * difference;
                energies[stream] += expected * expected;
                assert!(actual.is_finite() && (-1.0..=1.0).contains(&actual));
            }
            cursor += samples * usize::from(channels) * 2;
        }
        assert_eq!(cursor, data.len());
        assert!(
            redundancy_frames > 100,
            "fixture must exercise in-band redundancy"
        );
        for stream in 0..24 {
            let snr = 10.0 * (energies[stream] / errors[stream]).log10();
            assert!(snr >= 30.0, "stream {stream}: aligned SNR {snr:.2} dB");
        }
    }

    #[test]
    fn stereo_unmixing_has_one_sample_delay_and_preserves_mid_side_energy() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let mut mid = [0.0; 80];
        let mut side = [0.0; 80];
        mid[0] = 0.25;
        side[0] = 0.125;
        let mut output = [123.0; 160];
        Stereo::default().unmix(frame, [0; 2], &mid, &side, &mut output);
        assert_eq!(&output[..6], &[0.0, 0.0, 0.375, 0.125, 0.0, 0.0]);
    }

    #[test]
    fn mono_delay_carries_across_internal_frames() {
        let mut stereo = Stereo::default();
        let mut first = [0.0; 80];
        first[79] = 0.75;
        let mut output = [123.0; 80];
        stereo.mono(&first, &mut output);
        assert_eq!(output, [0.0; 80]);
        stereo.mono(&[0.0; 80], &mut output);
        assert_eq!(output[0], 0.75);
        assert!(output[1..].iter().all(|&value| value == 0.0));
    }

    #[test]
    fn invalid_capacity_and_geometry_preserve_entropy() {
        let mut silk = SilkDecoder::new();
        let mut entropy = RangeDecoder::new(&[255; 16]);
        let before = (entropy.range(), entropy.tell());
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms20,
        };
        let mut output = [123.0; 160];
        assert_eq!(
            silk.decode(&mut entropy, frame, PacketDuration::Ms10, 1, &mut output),
            Err(Error::Argument)
        );
        assert_eq!(
            silk.decode(
                &mut entropy,
                frame,
                PacketDuration::Ms20,
                1,
                &mut output[..159]
            ),
            Err(Error::Output)
        );
        assert_eq!((entropy.range(), entropy.tell()), before);
        assert_eq!(output, [123.0; 160]);
        assert_eq!(silk.bandwidth, None);
    }
}
