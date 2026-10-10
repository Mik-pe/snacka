// SPDX-License-Identifier: MIT

const HISTORY: usize = 1_920;
const DECIMATION: usize = 6;
const WINDOW: usize = 160;

pub(crate) struct Concealment {
    history: [f32; HISTORY],
    length: usize,
    period: f32,
    elapsed: usize,
    fade: usize,
}

impl Concealment {
    pub(crate) fn new() -> Self {
        Self {
            history: [0.0; HISTORY],
            length: 0,
            period: 1.0,
            elapsed: 0,
            fade: 1_920,
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
        let mut best = 0.0;
        let mut lag = 0;
        for candidate in 14..=160 {
            let score = correlation(&downsampled, WINDOW, candidate, 1);
            // Near-equivalent harmonic peaks should not extend the repeated cycle.
            if score > best + 0.01 {
                best = score;
                lag = candidate;
            }
        }
        if best < 0.55 {
            return;
        }
        let mut best_lag = lag * DECIMATION;
        let mut score = -1.0;
        for candidate in
            best_lag.saturating_sub(DECIMATION).max(80)..=(best_lag + DECIMATION).min(960)
        {
            let current = correlation(&self.history, 960, candidate, 3);
            if current > score {
                score = current;
                best_lag = candidate;
            }
        }
        let offset = if best_lag > 80 && best_lag < 960 {
            let left = correlation(&self.history, 960, best_lag - 1, 3);
            let right = correlation(&self.history, 960, best_lag + 1, 3);
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

    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.length = 0;
        self.elapsed = 0;
    }
}

fn correlation(samples: &[f32], window: usize, lag: usize, step: usize) -> f32 {
    let start = samples.len() - window;
    let mut product = 0.0;
    let mut current_energy = 0.0;
    let mut previous_energy = 0.0;
    for index in (start..samples.len()).step_by(step) {
        let current = samples[index];
        let previous = samples[index - lag];
        product += current * previous;
        current_energy += current * current;
        previous_energy += previous * previous;
    }
    product / (current_energy * previous_energy).sqrt().max(1e-20)
}
