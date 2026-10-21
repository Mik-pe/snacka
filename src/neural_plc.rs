// SPDX-License-Identifier: MIT
//! Loss-only, finite-history envelope prediction. No recurrent synthesized history.
use crate::{FRAME_SAMPLES, neural_plc_weights as weights};

const BANDS: usize = 16;
const FEATURES: usize = 24;
const HZ: [usize; 17] = [
    0, 200, 400, 600, 800, 1000, 1300, 1600, 2000, 2500, 3200, 4000, 5200, 6800, 9000, 13000, 24000,
];

pub(crate) fn edges(samples: usize) -> [usize; 17] {
    HZ.map(|hz| hz * samples / 24000)
}

#[derive(Clone)]
pub(crate) struct History {
    frames: [[f32; FEATURES]; 3],
    count: usize,
}

impl History {
    pub(crate) const fn new() -> Self {
        Self {
            frames: [[0.0; FEATURES]; 3],
            count: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn observe(&mut self, coefficients: &[f32], bandwidth: usize, f0: usize) {
        self.frames.rotate_left(1);
        let previous = self.frames[1][16];
        let frame = &mut self.frames[2];
        frame.fill(0.0);
        let boundaries = edges(coefficients.len());
        let mut largest = -20.0f32;
        let mut total = 0.0;
        for band in 0..BANDS {
            let values = &coefficients[boundaries[band]..boundaries[band + 1]];
            let energy: f32 = values.iter().map(|x| x * x).sum();
            total += energy;
            frame[band] = (energy / values.len() as f32 + 1e-12).ln() * 0.5;
            largest = largest.max(frame[band]);
        }
        for value in &mut frame[..BANDS] {
            *value = ((*value - largest) * 0.125).clamp(-1.0, 0.0);
        }
        frame[16] = ((total / coefficients.len() as f32 + 1e-12).ln() * 0.05).clamp(-1.5, 0.5);
        frame[17] = f0 as f32 / 500.0;
        frame[18] = if f0 == 0 { 0.0 } else { 1.0 };
        frame[19] = bandwidth as f32 / 24000.0;
        frame[20] = if self.count == 0 {
            0.0
        } else {
            (frame[16] - previous).clamp(-1.0, 1.0)
        };
        frame[21] = coefficients.iter().filter(|&&x| x.abs() > 1e-6).count() as f32
            / coefficients.len() as f32;
        frame[22] = coefficients.len() as f32 / FRAME_SAMPLES as f32;
        frame[23] = 1.0;
        self.count = (self.count + 1).min(3);
    }

    pub(crate) fn input(&self, samples: usize, loss: [f32; 5]) -> [f32; 80] {
        let mut input = [0.0; 80];
        for (index, frame) in self.frames.iter().enumerate() {
            input[index * FEATURES..(index + 1) * FEATURES].copy_from_slice(frame);
        }
        input[72] = samples as f32 / FRAME_SAMPLES as f32;
        input[73..78].copy_from_slice(&loss);
        input[78] = self.count as f32 / 3.0;
        input[79] = self.frames[2][16];
        input
    }
}

fn layer<const INPUT: usize, const OUTPUT: usize>(
    input: &[f32; INPUT],
    weights: &[[i8; INPUT]; OUTPUT],
    scales: &[f32; OUTPUT],
    biases: &[f32; OUTPUT],
    relu: bool,
) -> [f32; OUTPUT] {
    std::array::from_fn(|row| {
        let sum: f32 = weights[row]
            .iter()
            .zip(input)
            .map(|(&w, &x)| f32::from(w) * x)
            .sum();
        let value = sum * scales[row] + biases[row];
        if relu { value.max(0.0) } else { value }
    })
}

pub(crate) fn gains(input: &[f32; 80]) -> [f32; 16] {
    let first = layer(input, &weights::W1, &weights::S1, &weights::B1, true);
    let second = layer(&first, &weights::W2, &weights::S2, &weights::B2, true);
    let output = layer(&second, &weights::W3, &weights::S3, &weights::B3, false);
    output.map(|x| 1.0 + weights::STRENGTH * 0.5 * x.tanh())
}

pub(crate) fn refine(coefficients: &mut [f32], input: &[f32; 80]) {
    if !weights::ENABLED
        || input[78] < 1.0
        || input[76] >= 0.985
        || input[73] >= input[77]
        || coefficients.iter().all(|&x| x == 0.0)
    {
        return;
    }
    let gains = gains(input);
    let boundaries = edges(coefficients.len());
    for band in 0..BANDS {
        for value in &mut coefficients[boundaries[band]..boundaries[band + 1]] {
            *value *= gains[band];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_rust_matches_frozen_l4_pytorch_inference() {
        const {
            assert!(weights::ENABLED);
        }
        let records = include_bytes!("../tests/testdata/neural-plc-parity.bin");
        let (records, trailing) = records.as_chunks::<384>();
        assert!(trailing.is_empty());
        assert_eq!(records.len(), 32);
        for record in records {
            let input: [f32; 80] = std::array::from_fn(|i| {
                f32::from_le_bytes(record[i * 4..i * 4 + 4].try_into().unwrap())
            });
            let actual = gains(&input);
            for (index, value) in actual.into_iter().enumerate() {
                let offset = (80 + index) * 4;
                let expected = f32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
                let expected = 1.0 + weights::STRENGTH * (expected - 1.0);
                assert!(
                    (value - expected).abs() < 5e-5,
                    "{index}: Rust={value}, GPU={expected}"
                );
                assert!((0.5..=1.5).contains(&value));
            }
        }
    }

    #[test]
    fn active_spectral_correction_preserves_exact_polarity() {
        let mut input = [0.1; 80];
        input[73] = 0.0;
        input[76] = 0.2;
        input[77] = 1.0;
        input[78] = 1.0;
        for samples in [480, 960] {
            let mut positive: Vec<_> = (0..samples)
                .map(|i| (i as f32 * 0.037).sin() * 0.4)
                .collect();
            let mut negative: Vec<_> = positive.iter().map(|x| -x).collect();
            refine(&mut positive, &input);
            refine(&mut negative, &input);
            for (&left, &right) in positive.iter().zip(&negative) {
                assert_eq!(left, -right);
            }
        }
    }

    #[test]
    fn history_is_polarity_invariant_and_reset_discards_prior_audio() {
        for samples in [480, 960] {
            let mut positive = History::new();
            let mut negative = History::new();
            let values: Vec<_> = (0..samples)
                .map(|i| (i as f32 * 0.017).sin() * 0.1)
                .collect();
            for _ in 0..4 {
                positive.observe(&values, 8000, 0);
                negative.observe(&values.iter().map(|x| -x).collect::<Vec<_>>(), 8000, 0);
            }
            assert_eq!(
                positive.input(samples, [0.0; 5]),
                negative.input(samples, [0.0; 5])
            );
            positive.reset();
            assert_eq!(
                positive.input(samples, [0.0; 5]),
                History::new().input(samples, [0.0; 5])
            );
        }
    }
}
