// SPDX-License-Identifier: MIT

const HISTORY: usize = 1_920;
const DECIMATION: usize = 6;
const WINDOW: usize = 160;

#[derive(Clone)]
pub(crate) struct Concealment {
    history: [f32; HISTORY],
    length: usize,
    period: f32,
    elapsed: usize,
    fade: usize,
    confidence: f32,
}

impl Concealment {
    pub(crate) fn new() -> Self {
        Self {
            history: [0.0; HISTORY],
            length: 0,
            period: 1.0,
            elapsed: 0,
            fade: 1_920,
            confidence: 0.0,
        }
    }

    pub(crate) fn observe(&mut self, samples: &[f32]) {
        // Correlation history must be contiguous received audio.
        if self.elapsed != 0 {
            self.length = 0;
        }
        let retained = self.length.min(HISTORY - samples.len());
        self.history
            .copy_within(self.length - retained..self.length, 0);
        self.history[retained..retained + samples.len()].copy_from_slice(samples);
        self.length = retained + samples.len();
        self.elapsed = 0;
    }

    // Fixed lane order preserves scalar ties and exposes independent accumulations.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn begin(&mut self) {
        if self.elapsed != 0 {
            return;
        }
        self.period = self.length.clamp(1, 480) as f32;
        self.fade = 1_920;
        self.confidence = 0.0;
        if self.length < HISTORY {
            return;
        }
        let mut downsampled = [0.0; HISTORY / DECIMATION];
        for (value, block) in downsampled
            .iter_mut()
            .zip(self.history.as_chunks::<DECIMATION>().0)
        {
            *value = block.iter().sum::<f32>() / DECIMATION as f32;
        }
        let current_energy = energy(&downsampled, WINDOW, 1);
        // A zero window has zero correlation at every lag, including canceled
        // high-frequency input. Preserve the waveform fallback in that case.
        if current_energy == 0.0
            && downsampled[downsampled.len() - WINDOW..]
                .iter()
                .all(|&value| value == 0.0)
        {
            return;
        }
        let mut best = 0.0;
        let mut lag = 0;
        for first in (14..=160).step_by(4) {
            let scores = if first + 3 <= 160 {
                correlations_consecutive4(&downsampled, WINDOW, first, 1, current_energy)
            } else {
                std::array::from_fn(|lane| {
                    correlation(
                        &downsampled,
                        WINDOW,
                        (first + lane).min(160),
                        1,
                        current_energy,
                    )
                })
            };
            for lane in 0..(161 - first).min(4) {
                if scores[lane] > best + 0.01 {
                    best = scores[lane];
                    lag = first + lane;
                }
            }
        }
        if best < 0.55 {
            return;
        }
        let current_energy = energy(&self.history, 960, 3);
        let mut best_lag = lag * DECIMATION;
        let mut score = -1.0;
        let first = best_lag.saturating_sub(DECIMATION).max(80);
        let last = (best_lag + DECIMATION).min(960);
        for group in (first..=last).step_by(4) {
            let scores = if group + 3 <= last {
                correlations_consecutive4(&self.history, 960, group, 3, current_energy)
            } else {
                std::array::from_fn(|lane| {
                    correlation(
                        &self.history,
                        960,
                        (group + lane).min(last),
                        3,
                        current_energy,
                    )
                })
            };
            for lane in 0..(last - group + 1).min(4) {
                if scores[lane] > score {
                    score = scores[lane];
                    best_lag = group + lane;
                }
            }
        }
        let offset = if best_lag > 80 && best_lag < 960 {
            let left = correlation(&self.history, 960, best_lag - 1, 3, current_energy);
            let right = correlation(&self.history, 960, best_lag + 1, 3, current_energy);
            let curvature = left - 2.0 * score + right;
            if curvature < -1e-6 {
                (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        self.period = best_lag as f32 + offset;
        self.fade = 3_840;
        self.confidence = score.clamp(0.0, 1.0);
    }

    pub(crate) fn sample(&self, ahead: usize) -> f32 {
        let elapsed = self.elapsed + ahead;
        if self.length == 0 || elapsed >= self.fade {
            return 0.0;
        }
        let position = self.length as f32 - self.period + elapsed as f32 % self.period;
        // Fractional modulo can round the position up to the history length.
        let index = (position.floor() as usize).min(self.length - 1);
        let fraction = position - index as f32;
        let left = self.history[index];
        let right = if index + 1 < self.length {
            self.history[index + 1]
        } else {
            let start = self.length as f32 - self.period;
            let index = start.floor() as usize;
            self.history[index]
                + (start - index as f32)
                    * (self.history[(index + 1).min(self.length - 1)] - self.history[index])
        };
        (left + fraction * (right - left)) * (1.0 - elapsed as f32 / self.fade as f32)
    }

    pub(crate) fn phase_sample(&self, ahead: usize, delta: f32) -> f32 {
        if delta == 0.0 {
            return self.sample(ahead);
        }
        let elapsed = self.elapsed + ahead;
        if self.length == 0 || elapsed >= self.fade {
            return 0.0;
        }
        let position =
            self.length as f32 - self.period + (elapsed as f32 + delta).rem_euclid(self.period);
        // Fractional modulo can round the position up to the history length.
        let index = (position.floor() as usize).min(self.length - 1);
        let fraction = position - index as f32;
        let left = self.history[index];
        let right = if index + 1 < self.length {
            self.history[index + 1]
        } else {
            let start = self.length as f32 - self.period;
            let index = start.floor() as usize;
            self.history[index]
                + (start - index as f32)
                    * (self.history[(index + 1).min(self.length - 1)] - self.history[index])
        };
        (left + fraction * (right - left)) * (1.0 - elapsed as f32 / self.fade as f32)
    }

    pub(crate) fn advance(&mut self, samples: usize) {
        self.elapsed = (self.elapsed + samples).min(self.fade);
    }

    pub(crate) fn missing(&self) -> bool {
        self.elapsed != 0
    }

    pub(crate) fn features(&self, samples: usize) -> [f32; 5] {
        [
            self.elapsed as f32 / 3840.0,
            (self.elapsed / samples) as f32 / 6.0,
            self.period / 960.0,
            self.confidence,
            self.fade as f32 / 3840.0,
        ]
    }

    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.length = 0;
        self.elapsed = 0;
    }
}

fn energy(samples: &[f32], window: usize, step: usize) -> f32 {
    let mut energy = 0.0;
    for index in (samples.len() - window..samples.len()).step_by(step) {
        energy += samples[index] * samples[index];
    }
    energy
}

fn correlation(
    samples: &[f32],
    window: usize,
    lag: usize,
    step: usize,
    current_energy: f32,
) -> f32 {
    let start = samples.len() - window;
    let mut product = 0.0;
    let mut previous_energy = 0.0;
    for index in (start..samples.len()).step_by(step) {
        let current = samples[index];
        let previous = samples[index - lag];
        product += current * previous;
        previous_energy += previous * previous;
    }
    product / (current_energy * previous_energy).sqrt().max(1e-20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_and_canceling_windows_retain_the_waveform_fallback() {
        for history in [
            [0.0; HISTORY],
            std::array::from_fn(|i| if i % 2 == 0 { 0.25 } else { -0.25 }),
        ] {
            let mut concealment = Concealment::new();
            for frame in history.chunks(960) {
                concealment.observe(frame);
            }
            concealment.begin();
            assert_eq!(concealment.period, 480.0);
            assert_eq!(concealment.fade, 1_920);
            assert_eq!(concealment.sample(0), history[HISTORY - 480]);
            concealment.advance(960);
            assert_eq!(concealment.sample(0), history[HISTORY - 480] * 0.5);
            concealment.advance(960);
            assert_eq!(concealment.sample(0), 0.0);
        }
    }

    #[test]
    fn normalized_pitch_correlation_matches_direct_f64_dot_products() {
        let mut random = 6716u32;
        let noise: [f32; HISTORY] = std::array::from_fn(|_| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random as f32 / u32::MAX as f32 - 0.5
        });
        let tones = std::array::from_fn(|index| {
            let time = index as f32 / 48_000.0;
            0.4 * (std::f32::consts::TAU * 213.0 * time).sin()
                + 0.2 * (std::f32::consts::TAU * 941.0 * time).cos()
        });
        for samples in [[0.0; HISTORY], tones, noise] {
            for (length, window, step, first, last) in
                [(320, WINDOW, 1, 14, 160), (HISTORY, 960, 3, 80, 960)]
            {
                let samples = &samples[..length];
                let current_energy = energy(samples, window, step);
                for lag in first..=last {
                    let mut product = 0.0f64;
                    let mut current = 0.0f64;
                    let mut previous = 0.0f64;
                    for index in (length - window..length).step_by(step) {
                        let a = f64::from(samples[index]);
                        let b = f64::from(samples[index - lag]);
                        product += a * b;
                        current += a * a;
                        previous += b * b;
                    }
                    let expected = product / (current * previous).sqrt().max(1e-20);
                    let actual = correlation(samples, window, lag, step, current_energy);
                    assert!((f64::from(actual) - expected).abs() < 2e-6);
                }
            }
        }
    }
}

#[cfg(test)]
mod phase_control_tests {
    use super::*;

    #[test]
    fn zero_offset_preserves_legacy_samples_and_fade() {
        let mut state = Concealment::new();
        state.length = HISTORY;
        state.period = 173.5;
        state.fade = 1920;
        for (i, value) in state.history.iter_mut().enumerate() {
            *value = (i as f32 * 0.013).sin() * 0.3;
        }
        for elapsed in [0, 480, 1919, 1920] {
            state.elapsed = elapsed;
            for ahead in 0..1080 {
                assert_eq!(
                    state.phase_sample(ahead, 0.0).to_bits(),
                    state.sample(ahead).to_bits()
                );
                for delta in [-8.0, -0.5, 0.5, 8.0] {
                    let value = state.phase_sample(ahead, delta);
                    assert!(value.is_finite() && value.abs() <= 0.3);
                    if elapsed + ahead >= state.fade {
                        assert_eq!(value, 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn fractional_read_wraps_without_moving_fade() {
        let mut state = Concealment::new();
        state.length = 4;
        state.history[..4].copy_from_slice(&[-1.0, -0.2, 0.5, 0.9]);
        state.period = 4.0;
        state.fade = 1920;
        assert!((state.phase_sample(0, 0.5) + 0.6).abs() < 2e-7);
        assert!((state.phase_sample(0, -0.5) + 0.05).abs() < 2e-7);
        state.elapsed = 480;
        assert!((state.phase_sample(0, 0.5) + 0.45).abs() < 2e-7);
        assert!((state.phase_sample(0, -0.5) + 0.0375).abs() < 2e-7);
    }
}

// SPDX-License-Identifier: MIT
// Consecutive lag lanes share one contiguous four-sample window.
#[inline(always)]
#[allow(clippy::needless_range_loop)]
fn correlations_consecutive4(
    samples: &[f32],
    window: usize,
    first: usize,
    step: usize,
    current_energy: f32,
) -> [f32; 4] {
    let start = samples.len() - window;
    let previous = &samples[start - first - 3..samples.len() - first];
    let mut product = [0.0f32; 4];
    let mut previous_energy = [0.0f32; 4];
    for (&current, values) in samples[start..]
        .iter()
        .zip(previous.array_windows::<4>())
        .step_by(step)
    {
        for lane in 0..4 {
            let past = values[3 - lane];
            product[lane] += current * past;
            previous_energy[lane] += past * past;
        }
    }
    std::array::from_fn(|lane| {
        product[lane] / (current_energy * previous_energy[lane]).sqrt().max(1e-20)
    })
}
#[cfg(test)]
mod consecutive_tests {
    use super::*;
    #[test]
    fn every_lane_preserves_scalar_sum_bits() {
        let mut seed = 20261028u32;
        for case in 0..200 {
            let samples: Vec<f32> = (0..1920)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    match case % 4 {
                        0 => 0.,
                        1 => (i as f32 * 0.031).sin(),
                        _ => (seed as f32 / u32::MAX as f32 - 0.5) * 0.5,
                    }
                })
                .collect();
            for (window, step, lo, hi) in [(160, 1, 14, 156), (960, 3, 80, 957)] {
                let view = if window == 160 {
                    &samples[..320]
                } else {
                    &samples[..]
                };
                let total = energy(view, window, step);
                for first in (lo..=hi).step_by(if window == 160 { 4 } else { 2 }) {
                    let four = correlations_consecutive4(view, window, first, step, total);
                    for (lane, &score) in four.iter().enumerate() {
                        assert_eq!(
                            score.to_bits(),
                            correlation(view, window, first + lane, step, total).to_bits()
                        );
                    }
                }
            }
        }
    }
}
