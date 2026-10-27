// SPDX-License-Identifier: MIT
// Four independent lag accumulations; preserve each scalar addition order.
fn correlations4(
    samples: &[f32],
    window: usize,
    lags: [usize; 4],
    step: usize,
    current_energy: f32,
) -> [f32; 4] {
    let mut product = [0.; 4];
    let mut previous_energy = [0.; 4];
    for index in (samples.len() - window..samples.len()).step_by(step) {
        let current = samples[index];
        let previous = lags.map(|lag| samples[index - lag]);
        for lane in 0..4 {
            product[lane] += current * previous[lane];
            previous_energy[lane] += previous[lane] * previous[lane];
        }
    }
    std::array::from_fn(|lane| {
        product[lane] / (current_energy * previous_energy[lane]).sqrt().max(1e-20)
    })
}
#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn all_batched_scores_match_scalar_float_bits() {
        let mut seed = 20261027u32;
        for case in 0..200 {
            let samples: Vec<f32> = (0..1920)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    match case % 5 {
                        0 => 0.,
                        1 => (i as f32 * 0.029).sin(),
                        2 => {
                            if i % 2 == 0 {
                                0.25
                            } else {
                                -0.25
                            }
                        }
                        _ => (seed as f32 / u32::MAX as f32 - 0.5) * 0.001f32.powi(case % 4),
                    }
                })
                .collect();
            for (window, step, lo, hi) in [(160, 1, 14, 160), (960, 3, 80, 960)] {
                let power = energy(&samples, window, step);
                for first in (lo..=hi).step_by(4) {
                    let lags = std::array::from_fn(|i| (first + i).min(hi));
                    let scores = correlations4(&samples, window, lags, step, power);
                    for i in 0..4 {
                        assert_eq!(
                            scores[i].to_bits(),
                            correlation(&samples, window, lags[i], step, power).to_bits()
                        );
                    }
                }
            }
        }
    }
}
