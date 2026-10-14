// SPDX-License-Identifier: MIT
//! SILK frame gain, pitch, LTP and stereo metadata from RFC 6716 section 4.2.7.
//!
//! Gains precede spectral parameters on the wire; pitch follows them. Keeping
//! these entry points separate preserves that order without decoding a frame
//! twice. All arithmetic is fixed point, and decoded metadata uses fixed arrays.

use super::frame_tables::*;
use super::{Bandwidth, Error, Frame, FrameDuration, SignalType};
use crate::entropy::{Decoder, Model};

/// Wire context for one coded internal channel frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodingMode {
    /// First interval of this packet, or an LBRR frame after an uncoded interval.
    Independent,
    /// Later regular side frame after an uncoded interval: absolute gains and
    /// pitch are present, but the LTP scaling symbol is absent.
    IndependentNoLtpScale,
    /// The previous frame of the same type/channel in this packet was coded.
    Conditional,
}

/// Decoded long-term predictor for up to four 5 ms subframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PitchParameters {
    /// Clamped pitch lag in internal-rate samples for each subframe.
    pub(crate) lags: [usize; 4],
    /// Five-tap pitch predictor coefficients in Q14 for each subframe.
    pub(crate) coefficients: [[i16; 5]; 4],
    /// History scaling in Q14; unused for an unvoiced frame.
    pub(crate) scale: i16,
    /// Unclamped primary lag retained for conditional coding of the next frame.
    pub(crate) primary_lag: i32,
    /// Shared LTP codebook number, from 0 through 2.
    pub(crate) periodicity: u8,
}

impl PitchParameters {
    const UNVOICED: Self = Self {
        lags: [0; 4],
        coefficients: [[0; 5]; 4],
        scale: 0,
        primary_lag: 0,
        periodicity: 0,
    };
}

/// State needed to decode gain and pitch references for one SILK channel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelMetadata {
    previous_gain: Option<i32>,
    previous_signal: SignalType,
    previous_lag: i32,
}

impl Default for ChannelMetadata {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelMetadata {
    /// Start without a previous coded subframe, including after a channel reset.
    pub(crate) const fn new() -> Self {
        Self {
            previous_gain: None,
            previous_signal: SignalType::Inactive,
            previous_lag: 0,
        }
    }

    /// Decode two or four Q16 quantization gains before the spectral parameters.
    ///
    /// The caller chooses conditional coding from packet/channel ordering, not
    /// signal activity. Missing conditional history is rejected before reading.
    /// Reset this state after an uncoded side interval, so its first absolute
    /// gain is not constrained by a gain that is no longer available.
    pub(crate) fn decode_gains(
        &mut self,
        decoder: &mut Decoder<'_>,
        frame: Frame,
        signal: SignalType,
        coding: CodingMode,
    ) -> Result<[i32; 4], Error> {
        if coding == CodingMode::Conditional && self.previous_gain.is_none() {
            return Err(Error::Argument);
        }
        let mut result = [0; 4];
        for (subframe, output) in result.iter_mut().take(subframes(frame)).enumerate() {
            let index = if subframe == 0 && coding != CodingMode::Conditional {
                let high = decoder.symbol(GAIN[signal_index(signal)]) as i32;
                let low = decoder.symbol(UNIFORM8) as i32;
                let absolute = high * 8 + low;
                self.previous_gain
                    .map_or(absolute, |previous| absolute.max(previous - 16))
            } else {
                let delta = decoder.symbol(DELTA_GAIN) as i32;
                // The first conditional subframe has checked history; all later
                // subframes acquire it from the preceding iteration.
                delta_gain(self.previous_gain.unwrap(), delta)
            };
            self.previous_gain = Some(index);
            *output = gain_q16(index);
        }
        Ok(result)
    }

    /// Decode voiced pitch/filters after spectral parameters, or consume nothing
    /// for an inactive/unvoiced frame while recording its signal type.
    pub(crate) fn decode_pitch(
        &mut self,
        decoder: &mut Decoder<'_>,
        frame: Frame,
        signal: SignalType,
        coding: CodingMode,
    ) -> PitchParameters {
        if signal != SignalType::Voiced {
            self.previous_signal = signal;
            return PitchParameters::UNVOICED;
        }
        let rate_khz = match frame.bandwidth {
            Bandwidth::Narrow => 8,
            Bandwidth::Medium => 12,
            Bandwidth::Wide => 16,
        };
        let mut relative = None;
        if coding == CodingMode::Conditional && self.previous_signal == SignalType::Voiced {
            let delta = decoder.symbol(PITCH_DELTA) as i32;
            if delta != 0 {
                relative = Some(self.previous_lag + delta - 9);
            }
        }
        let primary = relative.unwrap_or_else(|| {
            let high = decoder.symbol(PITCH_HIGH) as i32;
            let low = decoder.symbol(match frame.bandwidth {
                Bandwidth::Narrow => UNIFORM4,
                Bandwidth::Medium => UNIFORM6,
                Bandwidth::Wide => UNIFORM8,
            }) as i32;
            high * (rate_khz / 2) + low + 2 * rate_khz
        });
        let contour = decoder.symbol(contour_model(frame));
        let offsets: &[i8] = match (frame.bandwidth, frame.duration) {
            (Bandwidth::Narrow, FrameDuration::Ms10) => &OFFSETS_NB10[contour],
            (Bandwidth::Narrow, FrameDuration::Ms20) => &OFFSETS_NB20[contour],
            (_, FrameDuration::Ms10) => &OFFSETS_WIDE10[contour],
            (_, FrameDuration::Ms20) => &OFFSETS_WIDE20[contour],
        };
        let periodicity = decoder.symbol(PERIODICITY);
        let model = [LTP_GAIN0, LTP_GAIN1, LTP_GAIN2][periodicity];
        let taps: &[[i8; 5]] = match periodicity {
            0 => &LTP_TAPS0,
            1 => &LTP_TAPS1,
            _ => &LTP_TAPS2,
        };
        let mut parameters = PitchParameters {
            primary_lag: primary,
            periodicity: periodicity as u8,
            scale: LTP_SCALES[0],
            ..PitchParameters::UNVOICED
        };
        for (subframe, &offset) in offsets.iter().enumerate() {
            parameters.lags[subframe] =
                (primary + i32::from(offset)).clamp(2 * rate_khz, 18 * rate_khz) as usize;
            let index = decoder.symbol(model);
            for (output, &tap) in parameters.coefficients[subframe]
                .iter_mut()
                .zip(&taps[index])
            {
                *output = i16::from(tap) * 128;
            }
        }
        if coding == CodingMode::Independent {
            parameters.scale = LTP_SCALES[decoder.symbol(LTP_SCALE)];
        }
        // The contour clamp is intentionally not applied to this history.
        self.previous_lag = primary;
        self.previous_signal = signal;
        parameters
    }
}

fn subframes(frame: Frame) -> usize {
    match frame.duration {
        FrameDuration::Ms10 => 2,
        FrameDuration::Ms20 => 4,
    }
}

fn signal_index(signal: SignalType) -> usize {
    match signal {
        SignalType::Inactive => 0,
        SignalType::Unvoiced => 1,
        SignalType::Voiced => 2,
    }
}

fn contour_model(frame: Frame) -> Model<'static> {
    match (frame.bandwidth, frame.duration) {
        (Bandwidth::Narrow, FrameDuration::Ms10) => CONTOUR_NB10,
        (Bandwidth::Narrow, FrameDuration::Ms20) => CONTOUR_NB20,
        (_, FrameDuration::Ms10) => CONTOUR_WIDE10,
        (_, FrameDuration::Ms20) => CONTOUR_WIDE20,
    }
}

fn delta_gain(previous: i32, delta: i32) -> i32 {
    (2 * delta - 16).max(previous + delta - 4).clamp(0, 63)
}

pub(super) fn gain_q16(index: i32) -> i32 {
    let logarithm = ((0x1d_1c71_i64 * i64::from(index)) >> 16) as i32 + 2090;
    let fraction = logarithm & 127;
    let integral = 1i32 << (logarithm >> 7);
    let correction = ((-174 * fraction * (128 - fraction)) >> 16) + fraction;
    integral + correction * (integral >> 7)
}

pub(super) fn encode_gain_symbol(
    encoder: &mut crate::entropy::Encoder<'_>,
    signal: SignalType,
    value: u8,
    independent: bool,
) -> Result<(), crate::entropy::Error> {
    if (independent && value > 63) || (!independent && value > 40) {
        return Err(crate::entropy::Error::Argument);
    }
    if independent {
        encoder.symbol(usize::from(value / 8), GAIN[signal_index(signal)])?;
        encoder.symbol(usize::from(value % 8), UNIFORM8)
    } else {
        encoder.symbol(usize::from(value), DELTA_GAIN)
    }
}

pub(super) fn encode_zero_stereo(
    encoder: &mut crate::entropy::Encoder<'_>,
    mid_only: Option<bool>,
) -> Result<(), crate::entropy::Error> {
    encoder.symbol(12, STEREO_JOINT)?;
    for _ in 0..2 {
        encoder.symbol(1, UNIFORM3)?;
        encoder.symbol(2, UNIFORM5)?;
    }
    if let Some(mid_only) = mid_only {
        encoder.symbol(usize::from(mid_only), MID_ONLY)?;
    }
    Ok(())
}

/// Stereo weights and the optional omission of the side channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StereoParameters {
    /// Mid-to-side prediction weights in Q13, prior to temporal interpolation.
    pub(crate) prediction_q13: [i16; 2],
    /// Whether the current side frame is absent.
    pub(crate) mid_only: bool,
}

/// Decode stereo metadata before the mid channel's frame type.
///
/// Call only for a stereo mid frame. For regular frames, `side_active` is the
/// corresponding side VAD flag; for LBRR it is the corresponding side LBRR flag.
pub(crate) fn decode_stereo(decoder: &mut Decoder<'_>, side_active: bool) -> StereoParameters {
    let joint = decoder.symbol(STEREO_JOINT);
    let mut weights = [0i32; 2];
    for (channel, output) in weights.iter_mut().enumerate() {
        let low = decoder.symbol(UNIFORM3);
        let fraction = decoder.symbol(UNIFORM5) as i32;
        let high = if channel == 0 { joint / 5 } else { joint % 5 };
        let index = 3 * high + low;
        let first = i32::from(STEREO_WEIGHTS[index]);
        let next = i32::from(STEREO_WEIGHTS[index + 1]);
        let step = ((next - first) * 6554) >> 16;
        *output = first + step * (2 * fraction + 1);
    }
    StereoParameters {
        prediction_q13: [(weights[0] - weights[1]) as i16, weights[1] as i16],
        mid_only: !side_active && decoder.symbol(MID_ONLY) != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::Encoder;

    // Frozen by independently invoking silk_gains_dequant in libopus 1.6.1.
    // These are output samples, not copied codec implementation or codebooks.
    const REFERENCE_GAINS: [i32; 64] = [
        81920, 96256, 112640, 131072, 153600, 180224, 210944, 246784, 286720, 335872, 397312,
        464896, 540672, 634880, 745472, 872448, 1019904, 1187840, 1392640, 1646592, 1925120,
        2244608, 2621440, 3080192, 3604480, 4194304, 4915200, 5767168, 6782976, 7929856, 9240576,
        10813440, 12713984, 14876672, 17301504, 20316160, 23855104, 28049408, 32768000, 38273024,
        44826624, 52690944, 61603840, 71827456, 83886080, 98566144, 115867648, 135266304,
        158334976, 185597952, 217055232, 253755392, 295698432, 346030080, 406847488, 480247808,
        557842432, 654311424, 767557632, 897581056, 1048576000, 1224736768, 1434451968, 1686110208,
    ];

    fn wire(events: &[(Model<'_>, usize)]) -> Vec<u8> {
        let mut storage = [0u8; 64];
        let mut encoder = Encoder::new(&mut storage);
        for &(model, index) in events {
            encoder.symbol(index, model).unwrap();
        }
        let length = encoder.finish().unwrap();
        storage[..length].to_vec()
    }

    fn frame(bandwidth: Bandwidth, duration: FrameDuration) -> Frame {
        Frame {
            bandwidth,
            duration,
        }
    }

    fn pitch_wire(
        frame: Frame,
        absolute: (usize, usize),
        contour: usize,
        book: usize,
        tap_index: usize,
        scaling: Option<usize>,
        relative: Option<usize>,
    ) -> Vec<u8> {
        let mut events = Vec::new();
        if let Some(index) = relative {
            events.push((PITCH_DELTA, index));
        }
        if relative.is_none_or(|index| index == 0) {
            events.push((PITCH_HIGH, absolute.0));
            events.push((
                match frame.bandwidth {
                    Bandwidth::Narrow => UNIFORM4,
                    Bandwidth::Medium => UNIFORM6,
                    Bandwidth::Wide => UNIFORM8,
                },
                absolute.1,
            ));
        }
        events.push((contour_model(frame), contour));
        events.push((PERIODICITY, book));
        events.extend(std::iter::repeat_n(
            ([LTP_GAIN0, LTP_GAIN1, LTP_GAIN2][book], tap_index),
            subframes(frame),
        ));
        if let Some(scale) = scaling {
            events.push((LTP_SCALE, scale));
        }
        // A following LCG seed detects accidental reads of an omitted scale.
        events.push((UNIFORM4, 3));
        wire(&events)
    }

    #[test]
    fn every_linear_gain_matches_frozen_native_output() {
        for (index, expected) in REFERENCE_GAINS.into_iter().enumerate() {
            assert_eq!(gain_q16(index as i32), expected);
        }
    }

    #[test]
    fn gains_decode_absolute_delta_and_cross_frame_clamp() {
        let geometry = frame(Bandwidth::Wide, FrameDuration::Ms20);
        for signal in [
            SignalType::Inactive,
            SignalType::Unvoiced,
            SignalType::Voiced,
        ] {
            for (absolute, &expected) in REFERENCE_GAINS.iter().enumerate() {
                let data = wire(&[
                    (GAIN[signal_index(signal)], absolute / 8),
                    (UNIFORM8, absolute % 8),
                    (DELTA_GAIN, 4),
                    (DELTA_GAIN, 4),
                    (DELTA_GAIN, 4),
                ]);
                let gains = ChannelMetadata::new()
                    .decode_gains(
                        &mut Decoder::new(&data),
                        geometry,
                        signal,
                        CodingMode::Independent,
                    )
                    .unwrap();
                assert_eq!(gains, [expected; 4]);
            }
        }
        let data = wire(&[
            (GAIN[0], 0),
            (UNIFORM8, 0),
            (DELTA_GAIN, 4),
            (DELTA_GAIN, 4),
            (DELTA_GAIN, 4),
        ]);
        let mut state = ChannelMetadata {
            previous_gain: Some(50),
            ..ChannelMetadata::new()
        };
        let gains = state
            .decode_gains(
                &mut Decoder::new(&data),
                geometry,
                SignalType::Inactive,
                CodingMode::IndependentNoLtpScale,
            )
            .unwrap();
        assert_eq!(gains, [REFERENCE_GAINS[34]; 4]);
    }

    #[test]
    fn every_delta_stays_in_index_domain_and_matches_rfc_equation() {
        let geometry = frame(Bandwidth::Narrow, FrameDuration::Ms10);
        for previous in 0..64i32 {
            for delta in 0..41i32 {
                let data = wire(&[(DELTA_GAIN, delta as usize), (DELTA_GAIN, 4)]);
                let mut state = ChannelMetadata {
                    previous_gain: Some(previous),
                    ..ChannelMetadata::new()
                };
                let gains = state
                    .decode_gains(
                        &mut Decoder::new(&data),
                        geometry,
                        SignalType::Voiced,
                        CodingMode::Conditional,
                    )
                    .unwrap();
                let expected = (2 * delta - 16).max(previous + delta - 4).clamp(0, 63);
                assert_eq!(
                    gains,
                    [
                        REFERENCE_GAINS[expected as usize],
                        REFERENCE_GAINS[expected as usize],
                        0,
                        0
                    ]
                );
                assert_eq!(state.previous_gain, Some(expected));
            }
        }
    }

    #[test]
    fn missing_gain_history_rejects_before_consuming_entropy() {
        let mut decoder = Decoder::new(&[0x23, 0xa5]);
        let before = (decoder.range(), decoder.tell_eighths());
        assert_eq!(
            ChannelMetadata::new().decode_gains(
                &mut decoder,
                frame(Bandwidth::Narrow, FrameDuration::Ms10),
                SignalType::Voiced,
                CodingMode::Conditional
            ),
            Err(Error::Argument)
        );
        assert_eq!((decoder.range(), decoder.tell_eighths()), before);
    }

    #[test]
    fn pitch_contours_and_filter_books_cover_all_geometries() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
                let geometry = frame(bandwidth, duration);
                let rate = match bandwidth {
                    Bandwidth::Narrow => 8,
                    Bandwidth::Medium => 12,
                    Bandwidth::Wide => 16,
                };
                for contour in 0..contour_model(geometry).symbols() {
                    for (book, taps) in [&LTP_TAPS0[..], &LTP_TAPS1[..], &LTP_TAPS2[..]]
                        .into_iter()
                        .enumerate()
                    {
                        for (index, coefficients) in taps.iter().enumerate() {
                            let data = pitch_wire(
                                geometry,
                                (31, rate / 2 - 1),
                                contour,
                                book,
                                index,
                                Some(2),
                                None,
                            );
                            let mut decoder = Decoder::new(&data);
                            let parameters = ChannelMetadata::new().decode_pitch(
                                &mut decoder,
                                geometry,
                                SignalType::Voiced,
                                CodingMode::Independent,
                            );
                            assert_eq!(parameters.primary_lag, 18 * rate as i32 - 1);
                            assert_eq!(parameters.scale, 8192);
                            assert_eq!(parameters.periodicity, book as u8);
                            for subframe in 0..subframes(geometry) {
                                assert!(
                                    (2 * rate..=18 * rate).contains(&parameters.lags[subframe])
                                );
                                assert_eq!(
                                    parameters.coefficients[subframe],
                                    coefficients.map(|tap| i16::from(tap) * 128)
                                );
                            }
                            assert_eq!(decoder.symbol(UNIFORM4), 3);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pitch_contours_match_external_native_sweep() {
        // Output digests frozen from 26,534 libopus 1.6.1 silk_decode_pitch
        // calls, including primary lags beyond both contour clamp boundaries.
        let families = [
            (
                Bandwidth::Narrow,
                FrameDuration::Ms10,
                8,
                0xa46d_529e_046d_52cdu64,
            ),
            (
                Bandwidth::Narrow,
                FrameDuration::Ms20,
                8,
                0xd728_dca7_4c2d_aecd,
            ),
            (
                Bandwidth::Medium,
                FrameDuration::Ms10,
                12,
                0x2066_fa02_f49a_4cfd,
            ),
            (
                Bandwidth::Medium,
                FrameDuration::Ms20,
                12,
                0x8d8b_f32f_07bf_9fa5,
            ),
            (
                Bandwidth::Wide,
                FrameDuration::Ms10,
                16,
                0x09ba_2289_e746_84ed,
            ),
            (
                Bandwidth::Wide,
                FrameDuration::Ms20,
                16,
                0x01ca_50ce_e7fb_6655,
            ),
        ];
        for (bandwidth, duration, rate, expected) in families {
            let geometry = frame(bandwidth, duration);
            let mut digest = 0xcbf2_9ce4_8422_2325u64;
            for lag_index in -16..=16 * rate + 22 {
                for contour in 0..contour_model(geometry).symbols() {
                    let data = pitch_wire(geometry, (0, 0), contour, 0, 1, None, Some(9));
                    let mut state = ChannelMetadata {
                        previous_signal: SignalType::Voiced,
                        previous_lag: 2 * rate + lag_index,
                        ..ChannelMetadata::new()
                    };
                    let parameters = state.decode_pitch(
                        &mut Decoder::new(&data),
                        geometry,
                        SignalType::Voiced,
                        CodingMode::Conditional,
                    );
                    for &lag in &parameters.lags[..subframes(geometry)] {
                        digest = (digest ^ lag as u64).wrapping_mul(0x100_0000_01b3);
                    }
                }
            }
            assert_eq!(digest, expected, "{bandwidth:?} {duration:?}");
        }
    }

    #[test]
    fn primary_pitch_history_is_not_contour_clamped() {
        let geometry = frame(Bandwidth::Narrow, FrameDuration::Ms10);
        let mut state = ChannelMetadata {
            previous_signal: SignalType::Voiced,
            previous_lag: 16,
            ..ChannelMetadata::new()
        };
        for expected in [8, 0] {
            let data = pitch_wire(geometry, (0, 0), 0, 0, 1, None, Some(1));
            let mut decoder = Decoder::new(&data);
            let parameters = state.decode_pitch(
                &mut decoder,
                geometry,
                SignalType::Voiced,
                CodingMode::Conditional,
            );
            assert_eq!(parameters.primary_lag, expected);
            assert_eq!(state.previous_lag, expected);
            assert_eq!(parameters.lags, [16, 16, 0, 0]);
            assert_eq!(parameters.scale, 15565);
            assert_eq!(decoder.symbol(UNIFORM4), 3);
        }
    }

    #[test]
    fn pitch_absolute_fallback_and_omitted_scale_do_not_desynchronize() {
        let geometry = frame(Bandwidth::Medium, FrameDuration::Ms20);
        for (previous_signal, coding, relative) in [
            (SignalType::Voiced, CodingMode::Conditional, Some(0)),
            (SignalType::Inactive, CodingMode::Conditional, None),
            (SignalType::Unvoiced, CodingMode::Conditional, None),
            (SignalType::Voiced, CodingMode::IndependentNoLtpScale, None),
        ] {
            let data = pitch_wire(geometry, (2, 3), 0, 0, 1, None, relative);
            let mut decoder = Decoder::new(&data);
            let mut state = ChannelMetadata {
                previous_signal,
                previous_lag: 200,
                ..ChannelMetadata::new()
            };
            let parameters = state.decode_pitch(&mut decoder, geometry, SignalType::Voiced, coding);
            assert_eq!(parameters.lags, [39; 4]);
            assert_eq!(parameters.scale, 15565);
            assert_eq!(decoder.symbol(UNIFORM4), 3);
        }
        for signal in [SignalType::Inactive, SignalType::Unvoiced] {
            let mut decoder = Decoder::new(&[0xff, 0]);
            let before = (decoder.range(), decoder.tell_eighths());
            let mut state = ChannelMetadata::new();
            assert_eq!(
                state.decode_pitch(&mut decoder, geometry, signal, CodingMode::Independent),
                PitchParameters::UNVOICED
            );
            assert_eq!((decoder.range(), decoder.tell_eighths()), before);
            assert_eq!(state.previous_signal, signal);
        }
    }

    #[test]
    fn stereo_metadata_matches_external_libopus_packets_and_interval() {
        // 64 packets independently encoded and decoded by libopus 1.6.1.
        // Each also carries a following seed to check complete symbol ordering.
        type StereoCase<'a> = (&'a [u8], bool, [i16; 2], bool, u32);
        let cases: &[StereoCase<'_>] = &[
            (&[4, 81, 0], true, [2708, -11156], false, 66546585),
            (&[7, 39, 64], false, [-5612, -7016], false, 14370556),
            (&[9, 72, 32], true, [-8961, -1459], false, 9396087),
            (&[10, 115, 184], false, [-16322, 6806], true, 608419840),
            (&[11, 209, 0], true, [-15940, 8044], false, 9580350),
            (&[18, 53], false, [5270, -10420], false, 71299958),
            (&[35, 52], true, [1362, -6812], false, 228159720),
            (&[49, 227, 0], false, [-4717, -1033], false, 58158368),
            (&[54, 23, 32], true, [-12370, 5150], false, 9506655),
            (&[55, 103, 32], false, [-13650, 7600], true, 608419840),
            (&[56, 96, 192], true, [5437, -7748], false, 28188312),
            (&[71, 58], false, [3155, -3155], false, 167205340),
            (&[164, 120], true, [2639, -328], false, 874608384),
            (&[178, 110], false, [-9099, 7214], false, 165261074),
            (&[199, 180, 0], true, [-6845, 9156], false, 29079180),
            (&[200, 40, 160], false, [13847, -9872], true, 620347392),
            (&[201, 147, 160], true, [11200, -5150], false, 9618498),
            (&[205, 199, 0], false, [8061, -2311], false, 58158368),
            (&[217, 20], true, [-655, 5450], false, 228159720),
            (&[242, 51], false, [-1182, 8192], false, 71852652),
            (&[244, 236, 96], true, [20522, -7896], false, 9580324),
            (&[245, 36, 8], false, [15116, -7220], true, 613138432),
            (&[246, 215, 224], true, [11234, 656], false, 9693060),
            (&[248, 166, 32], false, [6740, 5150], false, 14539592),
            (&[253, 241, 0], true, [906, 9512], false, 66546585),
            (&[1, 49, 0], false, [-3088, -8804], false, 50888604),
            (&[8, 210, 0], true, [-2598, -5150], false, 19013310),
            (&[9, 233, 80], false, [-8404, 656], true, 608419840),
            (&[10, 83, 96], true, [-17634, 7214], false, 9396087),
            (&[11, 71, 224], false, [-18464, 8044], false, 1804056320),
            (&[17, 12], true, [3054, -8804], false, 98070960),
            (&[23, 108], false, [-3859, -3565], false, 169130060),
            (&[52, 17], true, [-3729, -656], false, 76053240),
            (&[54, 21, 80], false, [-11605, 4385], true, 601341952),
            (&[55, 25, 128], true, [-16376, 9156], false, 9693060),
            (&[58, 176, 0], false, [11827, -9516], false, 21389981),
            (&[64, 40], true, [4891, -6350], false, 218650872),
            (&[133, 88], false, [984, -656], false, 663692352),
            (&[177, 132], true, [-6680, 4795], false, 220348050),
            (&[199, 88, 192], false, [-8959, 10418], true, 1804058624),
            (&[200, 69, 192], true, [17423, -12628], false, 9396087),
            (&[201, 34, 96], false, [11399, -7424], false, 1839431168),
            (&[207, 201], true, [4717, 1885], false, 76642800),
            (&[226, 154], false, [1452, 5150], false, 171119848),
            (&[240, 248], true, [-2198, 8800], false, 95066550),
            (&[244, 217, 104], false, [20082, -8192], true, 613138432),
            (&[245, 251, 0], true, [18157, -4795], false, 9396215),
            (&[246, 141, 192], false, [10545, -1033], false, 1825283840),
            (&[248, 128, 0], true, [6769, 4385], false, 18792225),
            (&[251, 94, 0], false, [696, 7748], false, 49909977),
            (&[1, 138, 0], true, [-736, -10420], false, 65772762),
            (&[7, 183, 112], false, [-3522, -6350], true, 1231159296),
            (&[9, 211, 64], true, [-7240, -656], false, 9693060),
            (&[10, 153, 48], false, [-16222, 7418], false, 1825283840),
            (&[11, 197, 160], true, [-17556, 9512], false, 9506655),
            (&[14, 4], false, [6552, -13364], false, 70470885),
            (&[44, 174], true, [3861, -7016], false, 229928400),
            (&[51, 168, 128], false, [-5123, 328], true, 19013248),
            (&[54, 48, 96], true, [-14026, 7010], false, 9768200),
            (&[55, 2, 192], false, [-15320, 7896], false, 1839424512),
            (&[57, 107, 0], true, [12628, -12628], false, 29079180),
            (&[62, 180], false, [4165, -6050], false, 167205340),
            (&[119, 88], true, [1983, -2311], false, 874608384),
            (&[195, 94, 0], false, [-4903, 7214], true, 54027072),
        ];
        for &(bytes, side_active, weights, mid_only, interval) in cases {
            let mut decoder = Decoder::new(bytes);
            let result = decode_stereo(&mut decoder, side_active);
            assert_eq!(result.prediction_q13, weights);
            assert_eq!(result.mid_only, mid_only);
            assert_eq!(decoder.range(), interval);
            assert_eq!(decoder.symbol(UNIFORM4), 2);
        }
    }

    #[test]
    fn stereo_weight_equations_and_mid_only_symbol_order() {
        for joint in 0..25 {
            for low in 0..3 {
                for fraction in 0..5 {
                    for side_active in [false, true] {
                        let mut events = vec![
                            (STEREO_JOINT, joint),
                            (UNIFORM3, low),
                            (UNIFORM5, fraction),
                            (UNIFORM3, 2 - low),
                            (UNIFORM5, 4 - fraction),
                        ];
                        if !side_active {
                            events.push((MID_ONLY, 1));
                        }
                        events.push((UNIFORM4, 2));
                        let data = wire(&events);
                        let mut decoder = Decoder::new(&data);
                        let result = decode_stereo(&mut decoder, side_active);
                        let left = 3 * (joint / 5) + low;
                        let right = 3 * (joint % 5) + 2 - low;
                        let interpolate = |index: usize, part: usize| {
                            let a = i32::from(STEREO_WEIGHTS[index]);
                            let b = i32::from(STEREO_WEIGHTS[index + 1]);
                            a + (((b - a) * 6554) >> 16) * (2 * part as i32 + 1)
                        };
                        let second = interpolate(right, 4 - fraction);
                        assert_eq!(
                            result.prediction_q13,
                            [(interpolate(left, fraction) - second) as i16, second as i16]
                        );
                        assert_eq!(result.mid_only, !side_active);
                        assert_eq!(decoder.symbol(UNIFORM4), 2);
                    }
                }
            }
        }
    }
}
