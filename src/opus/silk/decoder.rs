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
}

impl Channel {
    fn new() -> Self {
        Self {
            metadata: ChannelMetadata::new(),
            nlsf: NlsfState::new(),
            synthesis: Synthesis::new(),
            coded: false,
        }
    }

    fn reset(&mut self) {
        self.metadata = ChannelMetadata::new();
        self.nlsf.reset();
        self.synthesis.reset();
        self.coded = false;
    }

    fn decode(
        &mut self,
        decoder: &mut RangeDecoder<'_>,
        frame: Frame,
        active: bool,
        mode: CodingMode,
        output: &mut [f32],
    ) -> Result<(), Error> {
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
        self.synthesis
            .frame(frame, &parameters, &excitation, output)?;
        self.coded = true;
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
        let transition = match frame.bandwidth {
            Bandwidth::Narrow => 64,
            Bandwidth::Medium => 96,
            Bandwidth::Wide => 128,
        };
        for (i, (&current_mid, &current_side)) in mid.iter().zip(side).enumerate() {
            let ratio = i.min(transition) as f64 / transition as f64;
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
        let short = duration == PacketDuration::Ms10;
        if !(1..=2).contains(&channels) || (frame.duration == FrameDuration::Ms10) != short {
            return Err(Error::Argument);
        }
        let count = duration.frames();
        let samples = frame.samples();
        let channel_count = usize::from(channels);
        let total = samples * count;
        if output.len() < total * channel_count {
            return Err(Error::Output);
        }
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
