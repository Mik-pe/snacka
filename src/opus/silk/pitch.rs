// SPDX-License-Identifier: MIT
//! Independent residual-period analysis and SILK's transmitted LTP filters.

use super::frame_tables::*;
use super::quantize::History;
use super::{Bandwidth, Frame, FrameDuration};
use crate::entropy::{Encoder, Error};

const PREFIX: usize = 306;
const MAX_SAMPLES: usize = 320;

#[derive(Clone, Copy)]
pub(super) struct Pitch {
    pub(super) lag: usize,
    pub(super) book: u8,
    pub(super) indices: [u8; 4],
    pub(super) taps: [[i16; 5]; 4],
}

impl Pitch {
    pub(super) const NONE: Self = Self {
        lag: 0,
        book: 0,
        indices: [0; 4],
        taps: [[0; 5]; 4],
    };

    pub(super) fn encode(
        &self,
        encoder: &mut Encoder<'_>,
        frame: Frame,
        conditional: bool,
        previous_voiced: bool,
        independent_scale: bool,
    ) -> Result<(), Error> {
        if self.lag == 0 {
            return Ok(());
        }
        let rate = rate_khz(frame.bandwidth);
        if conditional && previous_voiced {
            // Escape to an absolute lag. This also keeps copied LBRR frames
            // independent of the regular frame's previous pitch context.
            encoder.symbol(0, PITCH_DELTA)?;
        }
        let index = self.lag - 2 * rate;
        encoder.symbol(index / (rate / 2), PITCH_HIGH)?;
        encoder.symbol(
            index % (rate / 2),
            match frame.bandwidth {
                Bandwidth::Narrow => UNIFORM4,
                Bandwidth::Medium => UNIFORM6,
                Bandwidth::Wide => UNIFORM8,
            },
        )?;
        let contour = match (frame.bandwidth, frame.duration) {
            (Bandwidth::Narrow, FrameDuration::Ms10) => CONTOUR_NB10,
            (Bandwidth::Narrow, FrameDuration::Ms20) => CONTOUR_NB20,
            (_, FrameDuration::Ms10) => CONTOUR_WIDE10,
            (_, FrameDuration::Ms20) => CONTOUR_WIDE20,
        };
        encoder.symbol(0, contour)?; // Constant lag through the frame.
        encoder.symbol(usize::from(self.book), PERIODICITY)?;
        let model = [LTP_GAIN0, LTP_GAIN1, LTP_GAIN2][usize::from(self.book)];
        let subframes = if frame.duration == FrameDuration::Ms10 {
            2
        } else {
            4
        };
        for &index in &self.indices[..subframes] {
            encoder.symbol(usize::from(index), model)?;
        }
        if independent_scale {
            encoder.symbol(0, LTP_SCALE)?;
        }
        Ok(())
    }
}

fn rate_khz(bandwidth: Bandwidth) -> usize {
    match bandwidth {
        Bandwidth::Narrow => 8,
        Bandwidth::Medium => 12,
        Bandwidth::Wide => 16,
    }
}

pub(super) fn analyze(
    samples: &[f32],
    frame: Frame,
    coefficients: &[i16],
    history: &History,
) -> Pitch {
    analyze_with_threshold(samples, frame, coefficients, history, 0.42)
}

pub(super) fn analyze_with_threshold(
    samples: &[f32],
    frame: Frame,
    coefficients: &[i16],
    history: &History,
    threshold: f64,
) -> Pitch {
    debug_assert!(threshold.is_finite() && (0.0..=1.0).contains(&threshold));
    let mut output = [0.0; PREFIX + MAX_SAMPLES];
    output[..PREFIX].copy_from_slice(&history.output);
    for (destination, &source) in output[PREFIX..].iter_mut().zip(samples) {
        *destination = f64::from(source);
    }
    // Q12 scaling is a power of two. Prepare the bounded predictor once,
    // retaining each residual's original tap order.
    let mut scaled = [0.0; 16];
    for (destination, &coefficient) in scaled.iter_mut().zip(coefficients) {
        *destination = f64::from(coefficient) / 4096.0;
    }
    let mut residual = [0.0; PREFIX + MAX_SAMPLES];
    for index in coefficients.len()..PREFIX + samples.len() {
        let prediction = scaled[..coefficients.len()]
            .iter()
            .enumerate()
            .map(|(tap, &value)| output[index - tap - 1] * value)
            .sum::<f64>();
        residual[index] = output[index] - prediction;
    }
    let minimum = 2 * rate_khz(frame.bandwidth);
    // The absolute lag code carries 32 high symbols of `rate / 2` low values
    // above `minimum`, so the largest encodable lag is `18 * rate - 1`.
    let maximum = 18 * rate_khz(frame.bandwidth) - 1;
    let energy = residual[PREFIX..PREFIX + samples.len()]
        .iter()
        .map(|value| value * value)
        .sum::<f64>();
    if energy < 1e-12 {
        return Pitch::NONE;
    }
    let mut lag = minimum;
    let mut best = 0.0;
    let mut candidate = minimum;
    while candidate + 12 <= maximum {
        let values = pitch_scores::<4>(&residual, samples.len(), energy, candidate);
        for (lane, value) in values.into_iter().enumerate() {
            if value > best {
                best = value;
                lag = candidate + lane * 4;
            }
        }
        candidate += 16;
    }
    while candidate <= maximum {
        let value = pitch_scores::<1>(&residual, samples.len(), energy, candidate)[0];
        if value > best {
            best = value;
            lag = candidate;
        }
        candidate += 4;
    }
    let center = lag;
    for candidate in center.saturating_sub(4).max(minimum)..=(center + 4).min(maximum) {
        let value = pitch_scores::<1>(&residual, samples.len(), energy, candidate)[0];
        if value > best {
            best = value;
            lag = candidate;
        }
    }
    if best < threshold {
        return Pitch::NONE;
    }
    let width = 5 * rate_khz(frame.bandwidth);
    let count = samples.len() / width;
    let mut covariance = [[[0.0; 5]; 5]; 4];
    let mut correlation = [[0.0; 5]; 4];
    let mut targets = [0.0; 4];
    for subframe in 0..count {
        for position in PREFIX + subframe * width..PREFIX + (subframe + 1) * width {
            let mut past = [0.0; 5];
            for (tap, value) in past.iter_mut().enumerate() {
                let previous = position - lag + 2 - tap;
                let scale = if previous < PREFIX {
                    15565.0 / 16384.0
                } else {
                    1.0
                };
                *value = residual[previous] * scale;
            }
            targets[subframe] += residual[position] * residual[position];
            for left in 0..5 {
                correlation[subframe][left] += past[left] * residual[position];
                for right in 0..=left {
                    covariance[subframe][left][right] += past[left] * past[right];
                }
            }
        }
    }
    let books = [&LTP_TAPS0[..], &LTP_TAPS1[..], &LTP_TAPS2[..]];
    let mut choice = Pitch::NONE;
    let mut best_cost = f64::INFINITY;
    for (book, filters) in books.iter().enumerate() {
        let mut indices = [0; 4];
        let mut taps = [[0; 5]; 4];
        let mut cost = 0.0;
        for subframe in 0..count {
            let mut minimum_error = f64::INFINITY;
            for (index, filter) in filters.iter().enumerate() {
                // The residual history in a conforming decoder may round
                // differently. A strict contraction prevents those small
                // differences from accumulating through the pitch loop.
                if filter
                    .iter()
                    .map(|&value| i16::from(value).abs())
                    .sum::<i16>()
                    > 120
                {
                    continue;
                }
                let normalized = filter.map(|value| f64::from(value) / 128.0);
                let mut error = targets[subframe];
                for left in 0..5 {
                    error -= 2.0 * normalized[left] * correlation[subframe][left];
                    error += normalized[left] * normalized[left] * covariance[subframe][left][left];
                    for right in 0..left {
                        error += 2.0
                            * normalized[left]
                            * normalized[right]
                            * covariance[subframe][left][right];
                    }
                }
                if error < minimum_error {
                    minimum_error = error;
                    indices[subframe] = index as u8;
                    taps[subframe] = filter.map(|value| i16::from(value) * 128);
                }
            }
            cost += minimum_error;
        }
        // The larger books need extra wire bits. A small residual-energy price
        // prevents selecting them for a negligible prediction improvement.
        cost += energy * book as f64 * 0.006;
        if cost < best_cost {
            best_cost = cost;
            choice = Pitch {
                lag,
                book: book as u8,
                indices,
                taps,
            };
        }
    }
    choice
}

fn pitch_scores<const LANES: usize>(
    residual: &[f64; PREFIX + MAX_SAMPLES],
    samples: usize,
    energy: f64,
    lag: usize,
) -> [f64; LANES] {
    // Adjacent coarse lags share the current residual. Each lane keeps the
    // scalar search's ascending sample order, including its energy floor.
    let mut cross = [0.0; LANES];
    let mut previous = [1e-18; LANES];
    for index in (PREFIX..PREFIX + samples).step_by(2) {
        let current = residual[index];
        for lane in 0..LANES {
            let past = residual[index - lag - lane * 4];
            cross[lane] += current * past;
            previous[lane] += past * past;
        }
    }
    std::array::from_fn(|lane| {
        if cross[lane] <= 0.0 {
            0.0
        } else {
            cross[lane] * cross[lane] / (previous[lane] * energy * 0.5)
                - (lag + lane * 4) as f64 * 0.000_02
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::SignalType;
    use super::super::frame::{ChannelMetadata, CodingMode};
    use super::*;

    #[test]
    fn relaxed_pitch_search_keeps_filters_contractive_and_silence_unvoiced() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let frame = Frame {
                bandwidth,
                duration: super::super::FrameDuration::Ms20,
            };
            let order = if bandwidth == Bandwidth::Wide { 16 } else { 10 };
            let mut samples = [0.0; MAX_SAMPLES];
            let length = frame.samples();
            assert_eq!(
                analyze_with_threshold(
                    &samples[..length],
                    frame,
                    &[0; 16][..order],
                    &History::new(),
                    0.15
                )
                .lag,
                0
            );
            for (i, sample) in samples[..length].iter_mut().enumerate() {
                *sample =
                    0.1 * (i as f32 * 0.071).sin() + 0.07 * ((i * 37 % 101) as f32 / 50.0 - 1.0);
            }
            let pitch = analyze_with_threshold(
                &samples[..length],
                frame,
                &[0; 16][..order],
                &History::new(),
                0.15,
            );
            if pitch.lag != 0 {
                assert!((2 * rate_khz(bandwidth)..=18 * rate_khz(bandwidth)).contains(&pitch.lag));
                for taps in pitch.taps {
                    assert!(
                        taps.into_iter().map(i32::from).map(i32::abs).sum::<i32>() <= 120 * 128
                    );
                }
            }
        }
    }

    #[test]
    fn batched_pitch_scores_match_independent_scalar_correlations() {
        let residual: [f64; PREFIX + MAX_SAMPLES] = std::array::from_fn(|i| match i % 7 {
            0 => -0.0,
            1 => 0.0,
            _ => ((i * 37 % 103) as f64 - 51.0) / 64.0,
        });
        for samples in [80, 120, 160, 240, 320] {
            let energy = residual[PREFIX..PREFIX + samples]
                .iter()
                .map(|x| x * x)
                .sum::<f64>();
            for lag in (16..=276).step_by(4) {
                let actual = pitch_scores::<4>(&residual, samples, energy, lag);
                for (lane, value) in actual.into_iter().enumerate() {
                    let candidate = lag + lane * 4;
                    let mut cross = 0.0;
                    let mut previous = 1e-18;
                    for index in (PREFIX..PREFIX + samples).step_by(2) {
                        cross += residual[index] * residual[index - candidate];
                        previous += residual[index - candidate] * residual[index - candidate];
                    }
                    let expected = if cross <= 0.0 {
                        0.0
                    } else {
                        cross * cross / (previous * energy * 0.5) - candidate as f64 * 0.000_02
                    };
                    assert_eq!(value.to_bits(), expected.to_bits());
                }
            }
        }
    }

    #[test]
    fn transmitted_pitch_filters_match_canonical_decoder() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let frame = Frame {
                bandwidth,
                duration: FrameDuration::Ms20,
            };
            let rate = rate_khz(bandwidth) * 1000;
            let samples: [f32; 320] = std::array::from_fn(|i| {
                0.2 * (i as f32 * 2.0 * std::f32::consts::PI * 200.0 / rate as f32).sin()
            });
            let pitch = analyze(
                &samples[..frame.samples()],
                frame,
                &[0; 10],
                &History::new(),
            );
            assert_ne!(pitch.lag, 0);
            for conditional in [false, true] {
                let mut bytes = [0; 64];
                let mut encoder = Encoder::new(&mut bytes);
                pitch
                    .encode(&mut encoder, frame, conditional, conditional, !conditional)
                    .unwrap();
                let interval = encoder.range();
                let length = encoder.finish().unwrap();
                let mut decoder = crate::entropy::Decoder::new(&bytes[..length]);
                let mut metadata = ChannelMetadata::new();
                if conditional {
                    // Seed only the previous signal using an independent copy.
                    let mut first_bytes = [0; 64];
                    let mut first = Encoder::new(&mut first_bytes);
                    pitch.encode(&mut first, frame, false, false, true).unwrap();
                    let first_length = first.finish().unwrap();
                    metadata.decode_pitch(
                        &mut crate::entropy::Decoder::new(&first_bytes[..first_length]),
                        frame,
                        SignalType::Voiced,
                        CodingMode::Independent,
                    );
                }
                let decoded = metadata.decode_pitch(
                    &mut decoder,
                    frame,
                    SignalType::Voiced,
                    if conditional {
                        CodingMode::Conditional
                    } else {
                        CodingMode::Independent
                    },
                );
                assert_eq!(decoded.lags, [pitch.lag; 4]);
                assert_eq!(decoded.coefficients, pitch.taps);
                assert_eq!(decoded.scale, 15565);
                assert_eq!(decoder.range(), interval);
            }
        }
    }

    #[test]
    fn longest_period_selects_an_encodable_lag() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let frame = Frame {
                bandwidth,
                duration: FrameDuration::Ms20,
            };
            let rate = rate_khz(bandwidth);
            let period = 18 * rate;
            // Smooth pulses one wire-format-limit period apart; nearby lags
            // still correlate strongly, so the frame should remain voiced.
            let samples: [f32; 320] = std::array::from_fn(|i| {
                let offset = (i % period) as f32 - 6.0;
                if offset.abs() < 6.0 {
                    0.5 * (1.0 + (offset * std::f32::consts::PI / 6.0).cos())
                } else {
                    0.0
                }
            });
            let pitch = analyze(
                &samples[..frame.samples()],
                frame,
                &[0; 10],
                &History::new(),
            );
            assert_ne!(pitch.lag, 0);
            assert!((2 * rate..18 * rate).contains(&pitch.lag));
            let mut bytes = [0; 64];
            let mut encoder = Encoder::new(&mut bytes);
            pitch
                .encode(&mut encoder, frame, false, false, true)
                .unwrap();
        }
    }
}
