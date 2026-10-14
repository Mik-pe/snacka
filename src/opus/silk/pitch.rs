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
    let mut output = [0.0; PREFIX + MAX_SAMPLES];
    output[..PREFIX].copy_from_slice(&history.output);
    for (destination, &source) in output[PREFIX..].iter_mut().zip(samples) {
        *destination = f64::from(source);
    }
    let mut residual = [0.0; PREFIX + MAX_SAMPLES];
    for index in coefficients.len()..PREFIX + samples.len() {
        let prediction = coefficients
            .iter()
            .enumerate()
            .map(|(tap, &value)| output[index - tap - 1] * f64::from(value) / 4096.0)
            .sum::<f64>();
        residual[index] = output[index] - prediction;
    }
    let minimum = 2 * rate_khz(frame.bandwidth);
    let maximum = 18 * rate_khz(frame.bandwidth);
    let energy = residual[PREFIX..PREFIX + samples.len()]
        .iter()
        .map(|value| value * value)
        .sum::<f64>();
    if energy < 1e-12 {
        return Pitch::NONE;
    }
    let score = |lag: usize| {
        let mut cross = 0.0;
        let mut previous = 1e-18;
        // A decimated correlation search selects the region; refinement below
        // uses the same bounded calculation around that region.
        for index in (PREFIX..PREFIX + samples.len()).step_by(2) {
            cross += residual[index] * residual[index - lag];
            previous += residual[index - lag] * residual[index - lag];
        }
        if cross <= 0.0 {
            return 0.0;
        }
        cross * cross / (previous * energy * 0.5) - lag as f64 * 0.000_02
    };
    let mut lag = minimum;
    let mut best = 0.0;
    for candidate in (minimum..=maximum).step_by(4) {
        let value = score(candidate);
        if value > best {
            best = value;
            lag = candidate;
        }
    }
    let center = lag;
    for candidate in center.saturating_sub(4).max(minimum)..=(center + 4).min(maximum) {
        let value = score(candidate);
        if value > best {
            best = value;
            lag = candidate;
        }
    }
    if best < 0.42 {
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

#[cfg(test)]
mod tests {
    use super::super::SignalType;
    use super::super::frame::{ChannelMetadata, CodingMode};
    use super::*;

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
}
