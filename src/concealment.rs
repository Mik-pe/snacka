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
        for candidate in 14..=160 {
            let score = correlation(&downsampled, WINDOW, candidate, 1, current_energy);
            // Near-equivalent harmonic peaks should not extend the repeated cycle.
            if score > best + 0.01 {
                best = score;
                lag = candidate;
            }
        }
        if best < 0.55 {
            return;
        }
        let current_energy = energy(&self.history, 960, 3);
        let mut best_lag = lag * DECIMATION;
        let mut score = -1.0;
        for candidate in
            best_lag.saturating_sub(DECIMATION).max(80)..=(best_lag + DECIMATION).min(960)
        {
            let current = correlation(&self.history, 960, candidate, 3, current_energy);
            if current > score {
                score = current;
                best_lag = candidate;
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
