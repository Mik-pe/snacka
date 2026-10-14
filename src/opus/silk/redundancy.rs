// SPDX-License-Identifier: MIT
//! Causal SILK low-bitrate redundancy, RFC 6716 sections 4.2.3–4.2.7.
//!
//! A successful packet saves its actual analyzed channel frames. The next
//! packet can transmit those frames before its regular frames, with fresh gain
//! references for the independent LBRR stream. Geometry changes omit redundancy
//! rather than claiming that differently sized cached audio covers the loss.

use super::encoder::{CodedFrame, encode_frame, estimated_cost};
use super::frame::encode_zero_stereo;
use super::{Frame, FrameDuration, LBRR_40, LBRR_60, PacketDuration, SignalType};
use crate::entropy::{Encoder, Error};

const MAX_FRAMES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Geometry {
    frame: Frame,
    duration: PacketDuration,
    channels: u8,
}

impl Geometry {
    fn new(frame: Frame, duration: PacketDuration, channels: u8) -> Result<Self, Error> {
        if !(1..=2).contains(&channels)
            || (frame.duration == FrameDuration::Ms10) != (duration == PacketDuration::Ms10)
        {
            return Err(Error::Argument);
        }
        Ok(Self {
            frame,
            duration,
            channels,
        })
    }
}

/// Persistent bounded storage for the immediately preceding encoded packet.
/// Commit only after all regular packet stages were successfully serialized.
pub(super) struct RedundancyCache {
    geometry: Option<Geometry>,
    stages: [CodedFrame; MAX_FRAMES],
    activity: [[bool; 3]; 2],
}

impl RedundancyCache {
    pub(super) const fn new() -> Self {
        Self {
            geometry: None,
            stages: [CodedFrame::EMPTY; MAX_FRAMES],
            activity: [[false; 3]; 2],
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(super) fn remember(
        &mut self,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        stages: &[CodedFrame; MAX_FRAMES],
        activity: [[bool; 3]; 2],
    ) -> Result<(), Error> {
        let geometry = Geometry::new(frame, duration, channels)?;
        let subframes = if frame.duration == FrameDuration::Ms10 {
            2
        } else {
            4
        };
        for interval in 0..duration.frames() {
            for channel in 0..usize::from(channels) {
                let stage = &stages[interval * 2 + channel];
                if stage.coded
                    && (!(0..=63).contains(&stage.gain_index)
                        || stage.gains[1..subframes].iter().any(|&gain| gain > 40))
                {
                    return Err(Error::Argument);
                }
            }
        }
        self.geometry = Some(geometry);
        self.stages = *stages;
        self.activity = activity;
        Ok(())
    }

    /// Return actual previous speech with LBRR-local gain contexts. Both
    /// channels are selected independently, allowing side-only active speech.
    /// Geometry transitions deliberately carry no cached LBRR.
    pub(super) fn for_packet(
        &self,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        enabled: bool,
    ) -> Result<Option<RedundancyPlan>, Error> {
        let geometry = Geometry::new(frame, duration, channels)?;
        if !enabled || self.geometry != Some(geometry) {
            return Ok(None);
        }
        let mut plan = RedundancyPlan {
            geometry,
            stages: [CodedFrame::EMPTY; MAX_FRAMES],
            masks: [0; 2],
        };
        let mut previous = [None; 2];
        let mut previous_voiced = [false; 2];
        let subframes = if frame.duration == FrameDuration::Ms10 {
            2
        } else {
            4
        };
        for interval in 0..duration.frames() {
            for (channel, previous_gain) in
                previous.iter_mut().enumerate().take(usize::from(channels))
            {
                let slot = interval * 2 + channel;
                let mut stage = self.stages[slot];
                if !stage.coded
                    || !self.activity[channel][interval]
                    || stage.kind.signal == SignalType::Inactive
                {
                    *previous_gain = None;
                    previous_voiced[channel] = false;
                    continue;
                }
                let first_gain = stage.gain_index;
                stage.conditional = previous_gain.is_some();
                stage.previous_voiced = stage.conditional && previous_voiced[channel];
                stage.independent_scale = !stage.conditional;
                stage.gains[0] = if let Some(previous) = *previous_gain {
                    conditional_gain(previous, first_gain).ok_or(Error::Argument)?
                } else {
                    first_gain as u8
                };
                let mut last_gain = first_gain;
                for &symbol in &stage.gains[1..subframes] {
                    last_gain = delta_gain(last_gain, symbol);
                }
                *previous_gain = Some(last_gain);
                previous_voiced[channel] = stage.kind.signal == SignalType::Voiced;
                plan.stages[slot] = stage;
                plan.masks[channel] |= 1 << interval;
            }
        }
        Ok(plan.masks.iter().any(|&mask| mask != 0).then_some(plan))
    }
}

/// Ready-to-serialize previous packet stages in interval/channel wire order.
/// Emit stereo metadata for a selected mid stage, then mid and side stages.
/// A selected side without a selected mid carries no new stereo metadata.
#[derive(Clone, Copy)]
pub(super) struct RedundancyPlan {
    geometry: Geometry,
    pub(super) stages: [CodedFrame; MAX_FRAMES],
    pub(super) masks: [u8; 2],
}

impl RedundancyPlan {
    pub(super) fn present(&self, channel: usize, interval: usize) -> bool {
        channel < usize::from(self.geometry.channels)
            && interval < self.geometry.duration.frames()
            && self.masks[channel] & (1 << interval) != 0
    }

    /// Estimated incremental LBRR cost, excluding the regular VAD/global
    /// header bits that are present even without redundancy. The estimate uses
    /// canonical metadata and pulse models, including packed interval masks.
    pub(super) fn estimate_bits(&self) -> Result<f32, Error> {
        let mut scratch = [0; 64];
        let mut encoder = Encoder::new(&mut scratch);
        let before = encoder.tell_eighths();
        encode_header(
            &mut encoder,
            self.geometry.duration,
            self.geometry.channels,
            &[[false; 3]; 2],
            Some(self),
        )?;
        let regular_header =
            usize::from(self.geometry.channels) * (self.geometry.duration.frames() + 1);
        let mut bits = (encoder.tell_eighths() - before) as f32 / 8.0 - regular_header as f32;
        for interval in 0..self.geometry.duration.frames() {
            if self.geometry.channels == 2 && self.present(0, interval) {
                let before = encoder.tell_eighths();
                encode_zero_stereo(&mut encoder, (!self.present(1, interval)).then_some(true))?;
                bits += (encoder.tell_eighths() - before) as f32 / 8.0;
            }
            for channel in 0..usize::from(self.geometry.channels) {
                if self.present(channel, interval) {
                    bits +=
                        estimated_cost(self.geometry.frame, &self.stages[interval * 2 + channel])?;
                }
            }
        }
        Ok(bits)
    }

    /// Serialize real cached prediction/excitation before regular packet
    /// frames. The caller writes `encode_header` first and owns termination.
    pub(super) fn encode_frames(&self, encoder: &mut Encoder<'_>) -> Result<(), Error> {
        for interval in 0..self.geometry.duration.frames() {
            if self.geometry.channels == 2 && self.present(0, interval) {
                encode_zero_stereo(encoder, (!self.present(1, interval)).then_some(true))?;
            }
            for channel in 0..usize::from(self.geometry.channels) {
                if self.present(channel, interval) {
                    encode_frame(
                        encoder,
                        self.geometry.frame,
                        &self.stages[interval * 2 + channel],
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Header ordering is channel VAD + global LBRR, followed by the optional
/// per-channel packed interval masks. All geometry is checked before output.
pub(super) fn encode_header(
    encoder: &mut Encoder<'_>,
    duration: PacketDuration,
    channels: u8,
    activity: &[[bool; 3]; 2],
    redundancy: Option<&RedundancyPlan>,
) -> Result<(), Error> {
    if !(1..=2).contains(&channels)
        || redundancy.is_some_and(|plan| {
            plan.geometry.duration != duration || plan.geometry.channels != channels
        })
    {
        return Err(Error::Argument);
    }
    let frames = duration.frames();
    for (channel, active) in activity.iter().enumerate().take(usize::from(channels)) {
        for &voice in &active[..frames] {
            encoder.bit(voice, 1)?;
        }
        encoder.bit(redundancy.is_some_and(|plan| plan.masks[channel] != 0), 1)?;
    }
    if frames > 1 {
        for channel in 0..usize::from(channels) {
            if let Some(mask) = redundancy
                .map(|plan| plan.masks[channel])
                .filter(|&mask| mask != 0)
            {
                encoder.symbol(
                    usize::from(mask - 1),
                    if frames == 2 { LBRR_40 } else { LBRR_60 },
                )?;
            }
        }
    }
    Ok(())
}

fn delta_gain(previous: i32, symbol: u8) -> i32 {
    let symbol = i32::from(symbol);
    (2 * symbol - 16).max(previous + symbol - 4).clamp(0, 63)
}

fn conditional_gain(previous: i32, target: i32) -> Option<u8> {
    // Neutral gain changes have the common delta symbol four, including
    // saturation at either endpoint. Prefer it before the remaining alphabet.
    if delta_gain(previous, 4) == target {
        return Some(4);
    }
    (0..=40).find(|&symbol| delta_gain(previous, symbol) == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::Decoder;
    use crate::opus::silk::{Bandwidth, Header, SilkDecoder, SilkEncoder};

    fn geometry(duration: PacketDuration) -> Frame {
        Frame {
            bandwidth: Bandwidth::Wide,
            duration: if duration == PacketDuration::Ms10 {
                FrameDuration::Ms10
            } else {
                FrameDuration::Ms20
            },
        }
    }

    fn speech_stages() -> [CodedFrame; MAX_FRAMES] {
        std::array::from_fn(|slot| {
            let mut stage = CodedFrame::EMPTY;
            stage.kind.signal = SignalType::Unvoiced;
            stage.gains = [4; 4];
            stage.gain_index = 30 + (slot % 2) as i32;
            stage.conditional = true;
            stage.coded = true;
            stage.pulses[slot] = slot as i16 + 1;
            stage
        })
    }

    #[test]
    fn all_activity_and_redundancy_masks_use_the_canonical_header_grammar() {
        for duration in [
            PacketDuration::Ms10,
            PacketDuration::Ms20,
            PacketDuration::Ms40,
            PacketDuration::Ms60,
        ] {
            let frame = geometry(duration);
            let limit = 1u8 << duration.frames();
            for channels in 1..=2 {
                for mid in 0..limit {
                    for side in 0..if channels == 2 { limit } else { 1 } {
                        let masks = [mid, side];
                        let activity = std::array::from_fn(|channel| {
                            std::array::from_fn(|interval| masks[channel] & (1 << interval) != 0)
                        });
                        let mut cache = RedundancyCache::new();
                        cache
                            .remember(frame, duration, channels, &speech_stages(), activity)
                            .unwrap();
                        let plan = cache.for_packet(frame, duration, channels, true).unwrap();
                        assert_eq!(plan.as_ref().map_or([0; 2], |plan| plan.masks), masks);
                        let current_activity = [[true, false, true], [false, true, false]];
                        let mut bytes = [0; 64];
                        let mut encoder = Encoder::new(&mut bytes);
                        encode_header(
                            &mut encoder,
                            duration,
                            channels,
                            &current_activity,
                            plan.as_ref(),
                        )
                        .unwrap();
                        let expected = (encoder.range(), encoder.tell_eighths());
                        let length = encoder.finish().unwrap();
                        let mut decoder = Decoder::new(&bytes[..length]);
                        let header = Header::decode(&mut decoder, duration, channels).unwrap();
                        for channel in 0..usize::from(channels) {
                            for interval in 0..duration.frames() {
                                assert_eq!(
                                    header.voice_activity(channel, interval),
                                    Some(current_activity[channel][interval])
                                );
                                assert_eq!(
                                    header.redundancy(channel, interval),
                                    Some(activity[channel][interval])
                                );
                            }
                        }
                        assert_eq!((decoder.range(), decoder.tell_eighths()), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn missing_intervals_reset_gain_pitch_and_scaling_contexts() {
        let duration = PacketDuration::Ms60;
        let frame = geometry(duration);
        let mut stages = speech_stages();
        stages[0].kind.signal = SignalType::Voiced;
        stages[2].kind.signal = SignalType::Unvoiced;
        stages[4].kind.signal = SignalType::Voiced;
        let mut cache = RedundancyCache::new();
        cache
            .remember(
                frame,
                duration,
                2,
                &stages,
                [[true; 3], [true, false, true]],
            )
            .unwrap();
        let plan = cache.for_packet(frame, duration, 2, true).unwrap().unwrap();
        for (channel, original) in stages[..2].iter().enumerate() {
            let stage = &plan.stages[channel];
            assert!(!stage.conditional && stage.independent_scale && !stage.previous_voiced);
            assert_eq!(stage.gains[0], original.gain_index as u8);
        }
        assert!(plan.stages[2].conditional && plan.stages[2].previous_voiced);
        assert!(!plan.stages[2].independent_scale);
        assert!(plan.stages[4].conditional && !plan.stages[4].previous_voiced);
        assert!(!plan.stages[3].coded);
        assert!(!plan.stages[5].conditional && plan.stages[5].independent_scale);
        assert_eq!(plan.stages[5].gains[0], 31);
        for (slot, stage) in plan
            .stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| stage.coded)
        {
            assert_eq!(stage.pulses, stages[slot].pulses);
        }
        assert!(!plan.present(2, 0) && !plan.present(0, 3));
    }

    #[test]
    fn cache_reset_transitions_and_invalid_commit_cannot_relabel_old_audio() {
        let duration = PacketDuration::Ms20;
        let frame = geometry(duration);
        let mut cache = RedundancyCache::new();
        assert!(
            cache
                .for_packet(frame, duration, 1, true)
                .unwrap()
                .is_none()
        );
        cache
            .remember(frame, duration, 1, &speech_stages(), [[true; 3]; 2])
            .unwrap();
        assert!(
            cache
                .for_packet(frame, duration, 1, false)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .for_packet(frame, duration, 2, true)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .for_packet(frame, PacketDuration::Ms60, 1, true)
                .unwrap()
                .is_none()
        );
        let different = Frame {
            bandwidth: Bandwidth::Narrow,
            ..frame
        };
        assert!(
            cache
                .for_packet(different, duration, 1, true)
                .unwrap()
                .is_none()
        );
        let mut invalid = speech_stages();
        invalid[0].gain_index = 64;
        assert_eq!(
            cache.remember(frame, duration, 1, &invalid, [[true; 3]; 2]),
            Err(Error::Argument)
        );
        assert!(
            cache
                .for_packet(frame, duration, 1, true)
                .unwrap()
                .is_some()
        );
        cache.reset();
        assert!(
            cache
                .for_packet(frame, duration, 1, true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn exact_gain_references_cover_the_complete_delta_alphabet() {
        for previous in 0..=63 {
            for target in 0..=63 {
                let symbol = conditional_gain(previous, target);
                assert_eq!(
                    symbol.is_some(),
                    (0..=40).any(|symbol| delta_gain(previous, symbol) == target)
                );
                if let Some(symbol) = symbol {
                    assert_eq!(delta_gain(previous, symbol), target);
                    if target == previous {
                        assert_eq!(symbol, 4);
                    }
                }
            }
        }
    }

    #[test]
    fn real_stereo_activity_gaps_keep_partial_lbrr_and_following_wire_order() {
        let frame = geometry(PacketDuration::Ms60);
        let samples = frame.samples() * 3;
        let mut signal = [0.0; 1920];
        let mut encoder_state = SilkEncoder::new();
        encoder_state.set_fec(true);
        for packet in 0..2 {
            for (index, pair) in signal.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let interval = index / frame.samples();
                let mid = if interval == 1 {
                    0.0
                } else {
                    0.12 * (index as f32 * (0.08 + packet as f32 * 0.01)).sin()
                };
                let side = if interval == 0 {
                    0.0
                } else {
                    0.07 * (index as f32 * 0.11).cos()
                };
                *pair = [mid + side, mid - side];
            }
            let mut bytes = [0; 4096];
            let mut encoder = Encoder::new(&mut bytes);
            let statistics = encoder_state
                .encode_layer(
                    &mut encoder,
                    frame,
                    PacketDuration::Ms60,
                    2,
                    &signal[..samples * 2],
                    48 * 60 * 2,
                )
                .unwrap();
            let expected_range = encoder.range();
            let length = encoder.finish().unwrap();
            if packet == 1 {
                assert!(statistics.redundancy);
                let header =
                    Header::decode(&mut Decoder::new(&bytes[..length]), PacketDuration::Ms60, 2)
                        .unwrap();
                for (channel, flags) in [[true, false, true], [false, true, true]]
                    .iter()
                    .enumerate()
                {
                    for (interval, &present) in flags.iter().enumerate() {
                        assert_eq!(header.redundancy(channel, interval), Some(present));
                    }
                }
                let mut decoder_state = SilkDecoder::new();
                let mut output = [0.0; 1920];
                let mut fec = Decoder::new(&bytes[..length]);
                assert_eq!(
                    decoder_state
                        .decode_fec(&mut fec, frame, PacketDuration::Ms60, 2, &mut output)
                        .unwrap(),
                    Some(samples)
                );
                assert!(output.iter().all(|sample| sample.is_finite()));
                let mut regular = Decoder::new(&bytes[..length]);
                assert_eq!(
                    decoder_state
                        .decode(&mut regular, frame, PacketDuration::Ms60, 2, &mut output)
                        .unwrap(),
                    samples
                );
                assert_eq!(regular.range(), expected_range);
            }
        }
    }

    #[test]
    fn actual_previous_audio_is_recovered_and_following_payload_keeps_its_range() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for (duration, milliseconds) in [
                (PacketDuration::Ms10, 10),
                (PacketDuration::Ms20, 20),
                (PacketDuration::Ms40, 40),
                (PacketDuration::Ms60, 60),
            ] {
                let frame = Frame {
                    bandwidth,
                    ..geometry(duration)
                };
                let count = frame.samples() * duration.frames();
                let rate = frame.samples() * 1000 / milliseconds.min(20) as usize;
                for channels in 1..=2 {
                    let channels_usize = usize::from(channels);
                    let mut encoder_state = SilkEncoder::new();
                    encoder_state.set_fec(true);
                    let mut decoder_state = SilkDecoder::new();
                    let mut signal = [0.0; 1920];
                    let mut recovered = [0.0; 1920];
                    let mut current = [0.0; 1920];
                    for packet in 0..4 {
                        let previous_signal = signal;
                        for index in 0..count {
                            for channel in 0..channels_usize {
                                let frequency =
                                    160.0 + 70.0 * packet as f32 + 20.0 * channel as f32;
                                signal[index * channels_usize + channel] = 0.16
                                    * (index as f32 * 2.0 * std::f32::consts::PI * frequency
                                        / rate as f32)
                                        .sin();
                            }
                        }
                        let mut bytes = [0; 4096];
                        let mut encoder = Encoder::new(&mut bytes);
                        let statistics = encoder_state
                            .encode_layer(
                                &mut encoder,
                                frame,
                                duration,
                                channels,
                                &signal[..count * channels_usize],
                                48 * milliseconds * u32::from(channels),
                            )
                            .unwrap();
                        assert_eq!(
                            statistics.redundancy,
                            packet != 0,
                            "{bandwidth:?}/{duration:?}/{channels}/{packet}"
                        );
                        let expected_range = encoder.range();
                        let length = encoder.finish().unwrap();
                        if packet % 2 == 1 {
                            let mut fec = Decoder::new(&bytes[..length]);
                            assert_eq!(
                                decoder_state
                                    .decode_fec(&mut fec, frame, duration, channels, &mut recovered)
                                    .unwrap(),
                                Some(count)
                            );
                            assert!(
                                recovered[..count * channels_usize]
                                    .iter()
                                    .all(|sample| sample.is_finite())
                            );
                            let energy = recovered[..count * channels_usize]
                                .iter()
                                .map(|sample| f64::from(*sample).powi(2))
                                .sum::<f64>();
                            assert!(
                                energy > 1e-5,
                                "FEC must contain actual audio {bandwidth:?}/{duration:?}/{channels}/{packet}"
                            );
                            let mut error = 0.0;
                            let mut reference_energy = 0.0;
                            for index in 1..count {
                                for channel in 0..channels_usize {
                                    // SILK's channel mixing retains one native
                                    // sample of delay, including mono output.
                                    let source = f64::from(
                                        previous_signal[(index - 1) * channels_usize + channel],
                                    );
                                    let difference =
                                        f64::from(recovered[index * channels_usize + channel])
                                            - source;
                                    error += difference * difference;
                                    reference_energy += source * source;
                                }
                            }
                            let snr = 10.0 * (reference_energy / error).log10();
                            assert!(
                                snr > 15.0,
                                "FEC must reconstruct the previous signal {bandwidth:?}/{duration:?}/{channels}/{packet} SNR={snr}"
                            );
                        }
                        // Every even packet is lost; its next packet restores
                        // that interval and then advances regular decoding.
                        if packet % 2 == 1 {
                            let mut decoder = Decoder::new(&bytes[..length]);
                            assert_eq!(
                                decoder_state
                                    .decode(&mut decoder, frame, duration, channels, &mut current)
                                    .unwrap(),
                                count
                            );
                            assert_eq!(decoder.range(), expected_range);
                            assert!(
                                current[..count * channels_usize]
                                    .iter()
                                    .all(|sample| sample.is_finite())
                            );
                        }
                    }
                }
            }
        }
    }
}
