// SPDX-License-Identifier: MIT
//! Owned CELT loss prediction; independent decoder policy.

use super::transform::MAX_FRAME;

const LOSS_HISTORY: usize = 4096;
const LOSS_ORDER: usize = 12;
const RECENT: usize = 1442;
pub(crate) const LOOKAHEAD: usize = 120;
const LOSS_LOOKAHEAD: usize = LOOKAHEAD;

fn loss_buffer<const N: usize>() -> Box<[f32; N]> {
    vec![0.0; N]
        .into_boxed_slice()
        .try_into()
        .expect("fixed loss history")
}

// PLC is decoder policy, rather than a wire algorithm (RFC 6716 section 4.4).
// This independently designed predictor separates a stable all-pole spectral
// envelope from periodic excitation. Its analysis runs only at the first loss;
// consecutive losses retain their pitch and fade according to elapsed time.
pub(crate) struct LossPredictor {
    history: [Box<[f32; LOSS_HISTORY]>; 2],
    recent: [Box<[f32; RECENT]>; 2],
    cursor: usize,
    available: usize,
    elapsed: usize,
    coefficients: [[f32; LOSS_ORDER]; 2],
    residual_rms: [f32; 2],
    peak: [f32; 2],
    pitch: usize,
    periodicity: f32,
    seed: u32,
}

impl LossPredictor {
    pub(crate) const fn elapsed(&self) -> usize {
        self.elapsed
    }
    pub(crate) const fn periodicity(&self) -> f32 {
        self.periodicity
    }

    pub(crate) fn new() -> Self {
        Self {
            history: std::array::from_fn(|_| loss_buffer()),
            recent: std::array::from_fn(|_| loss_buffer()),
            cursor: 0,
            available: 0,
            elapsed: 0,
            coefficients: [[0.0; LOSS_ORDER]; 2],
            residual_rms: [0.0; 2],
            peak: [0.0; 2],
            pitch: 120,
            periodicity: 0.0,
            seed: 0x5ac3_1e97,
        }
    }

    pub(crate) fn reset(&mut self) {
        for history in &mut self.history {
            history.fill(0.0);
        }
        self.cursor = 0;
        self.available = 0;
        self.elapsed = 0;
        self.seed = 0x5ac3_1e97;
    }

    fn past(&self, channel: usize, offset: usize) -> f32 {
        if offset > self.available {
            0.0
        } else {
            self.history[channel][self.cursor.wrapping_sub(offset) & (LOSS_HISTORY - 1)]
                .clamp(-8.0, 8.0)
        }
    }

    pub(crate) fn observe(
        &mut self,
        pcm: &[[f32; MAX_FRAME]; 2],
        samples: usize,
        channels: usize,
        coded: bool,
    ) {
        let first = samples.min(LOSS_HISTORY - self.cursor);
        for (history, channel) in self.history[..channels].iter_mut().zip(pcm) {
            history[self.cursor..self.cursor + first].copy_from_slice(&channel[..first]);
            history[..samples - first].copy_from_slice(&channel[first..samples]);
        }
        self.cursor = (self.cursor + samples) & (LOSS_HISTORY - 1);
        self.available = (self.available + samples).min(LOSS_HISTORY);
        if coded {
            self.elapsed = 0;
        } else {
            self.elapsed = self.elapsed.saturating_add(samples);
        }
    }

    fn prepare(&mut self, channels: usize, allow_pitch: bool) {
        let count = self.available.min(960);
        if count <= LOSS_ORDER {
            self.coefficients = [[0.0; LOSS_ORDER]; 2];
            self.residual_rms = [0.0; 2];
            self.peak = [0.0; 2];
            self.periodicity = 0.0;
            return;
        }
        for channel in 0..channels {
            // Resolve the circular boundary and clamp once. The following LPC
            // and pitch passes reuse a contiguous newest-to-oldest window.
            let mut peak = 0.0f32;
            for index in 0..count {
                let sample = self.past(channel, index + 1);
                self.recent[channel][index] = sample;
                peak = peak.max(sample.abs());
            }
            for index in count..RECENT {
                self.recent[channel][index] = self.past(channel, index + 1);
            }
            let recent = &self.recent[channel];
            if peak == 0.0 {
                self.coefficients[channel] = [0.0; LOSS_ORDER];
                self.residual_rms[channel] = 0.0;
                self.peak[channel] = 0.0;
                continue;
            }
            let mut correlations = [0.0f64; LOSS_ORDER + 1];
            for offset in 1..=count {
                let value = f64::from(recent[offset - 1]);
                for (lag, correlation) in correlations.iter_mut().enumerate() {
                    *correlation += value * f64::from(recent[offset + lag - 1]);
                }
            }
            correlations[0] = correlations[0] * 1.001 + 1e-15;
            let mut coefficients = [0.0f64; LOSS_ORDER];
            let mut error = correlations[0];
            for order in 0..LOSS_ORDER {
                let residual = correlations[order + 1]
                    - coefficients[..order]
                        .iter()
                        .enumerate()
                        .map(|(i, &coefficient)| coefficient * correlations[order - i])
                        .sum::<f64>();
                let reflection = (residual / error.max(1e-15)).clamp(-0.94, 0.94);
                let previous = coefficients;
                for i in 0..order {
                    coefficients[i] = previous[i] - reflection * previous[order - 1 - i];
                }
                coefficients[order] = reflection;
                error *= 1.0 - reflection * reflection;
            }
            for (order, (&coefficient, destination)) in coefficients
                .iter()
                .zip(&mut self.coefficients[channel])
                .enumerate()
            {
                *destination = (coefficient * 0.995f64.powi(order as i32 + 1)) as f32;
            }
            let mut residual_energy = 0.0f64;
            for offset in 1..=count {
                let predicted = self.coefficients[channel]
                    .iter()
                    .enumerate()
                    .map(|(lag, &coefficient)| {
                        f64::from(coefficient) * f64::from(recent[offset + lag])
                    })
                    .sum::<f64>();
                residual_energy += (f64::from(recent[offset - 1]) - predicted).powi(2);
            }
            self.residual_rms[channel] = (residual_energy / count as f64).sqrt() as f32;
            self.peak[channel] = peak;
        }
        self.periodicity = 0.0;
        if !allow_pitch
            || self.available < 480
            || self.peak[..channels].iter().all(|&peak| peak == 0.0)
        {
            return;
        }
        let maximum = 960.min(self.available - 240);
        let window = 480.min(self.available - maximum);
        for recent in &mut self.recent[..channels] {
            for index in 0..RECENT - 1 {
                recent[index] -= recent[index + 1];
            }
        }
        if channels == 1 {
            self.find_pitch::<1>(maximum, window);
        } else {
            self.find_pitch::<2>(maximum, window);
        }
    }

    fn find_pitch<const CHANNELS: usize>(&mut self, maximum: usize, window: usize) {
        // The reference-window energy is invariant across every lag. Preserve
        // its original offset/channel addition order while calculating once.
        let mut first = 1e-15f64;
        for offset in (1..=window).step_by(4) {
            for channel in 0..CHANNELS {
                first += f64::from(self.recent[channel][offset - 1]).powi(2);
            }
        }
        let score = |lag: usize| {
            let mut cross = 0.0f64;
            let mut second = 1e-15f64;
            for offset in (1..=window).step_by(4) {
                for channel in 0..CHANNELS {
                    // Differencing reduces DC and gives pitch harmonics a
                    // comparable weight without an additional filter state.
                    let a = self.recent[channel][offset - 1];
                    let b = self.recent[channel][offset + lag - 1];
                    cross += f64::from(a) * f64::from(b);
                    second += f64::from(b).powi(2);
                }
            }
            (cross / (first * second).sqrt()) as f32
        };
        let mut best = 0.0;
        let mut pitch = 40;
        for lag in (40..=maximum).step_by(4) {
            let candidate = score(lag);
            if candidate > best + 0.01 {
                best = candidate;
                pitch = lag;
            }
        }
        for lag in pitch.saturating_sub(3).max(40)..=(pitch + 3).min(maximum) {
            let candidate = score(lag);
            if candidate > best {
                best = candidate;
                pitch = lag;
            }
        }
        self.pitch = pitch;
        self.periodicity = ((best - 0.45) / 0.45).clamp(0.0, 1.0);
    }

    fn envelope(age: usize) -> f32 {
        // Preserve the first twenty milliseconds, then progressively fade a
        // prolonged outage. At 400 ms only comfort-level energy remains.
        if age <= 960 {
            1.0
        } else {
            (-(age.saturating_sub(960) as f32) / 4800.0).exp()
        }
    }

    pub(crate) fn predict(
        &mut self,
        samples: usize,
        channels: usize,
        allow_pitch: bool,
        output: &mut [Box<[f32; MAX_FRAME + LOSS_LOOKAHEAD]>; 2],
    ) {
        if self.elapsed == 0 {
            self.prepare(channels, allow_pitch);
        }
        if self.peak[..channels].iter().all(|&peak| peak == 0.0) {
            for channel in &mut output[..channels] {
                channel[..samples + LOSS_LOOKAHEAD].fill(0.0);
            }
            return;
        }
        let periodicity = if allow_pitch { self.periodicity } else { 0.0 };
        let noise_mix = (1.0 - periodicity * periodicity).sqrt();
        for (channel, values) in output.iter_mut().enumerate().take(channels) {
            for sample in 0..samples + LOSS_LOOKAHEAD {
                let previous = |lag: usize| {
                    if sample >= lag {
                        values[sample - lag]
                    } else {
                        self.past(channel, lag - sample)
                    }
                };
                let envelope = Self::envelope(self.elapsed.saturating_add(sample));
                let former = Self::envelope(
                    self.elapsed
                        .saturating_add(sample)
                        .saturating_sub(self.pitch),
                );
                let periodic = previous(self.pitch) * envelope / former.max(1e-10);
                // A nonzero periodic sample is unchanged by the zero-weighted
                // spectral/noise terms. Retain their signed-zero arithmetic
                // for zero samples, and always advance the random state below.
                let exact_periodic = periodicity == 1.0 && periodic != 0.0;
                let spectral = if exact_periodic {
                    0.0
                } else {
                    self.coefficients[channel]
                        .iter()
                        .enumerate()
                        .map(|(lag, &coefficient)| coefficient * previous(lag + 1))
                        .sum::<f32>()
                };
                self.seed ^= self.seed << 13;
                self.seed ^= self.seed >> 17;
                self.seed ^= self.seed << 5;
                let value = if exact_periodic {
                    periodic
                } else {
                    let noise = (self.seed as f32 / u32::MAX as f32 - 0.5) * 3.464_101_6;
                    periodicity * periodic
                        + (1.0 - periodicity) * spectral
                        + noise_mix * noise * self.residual_rms[channel] * envelope
                };
                let limit = (self.peak[channel] * 1.5 * envelope).min(8.0);
                values[sample] = value.clamp(-limit, limit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_differences_preserve_signed_zero_clipping_and_ring_wrap() {
        let mut predictor = LossPredictor::new();
        let mut pcm = [[0.0; MAX_FRAME]; 2];
        for frame in 0..7 {
            for (channel, values) in pcm.iter_mut().enumerate() {
                for (index, value) in values.iter_mut().enumerate() {
                    *value = match (frame * MAX_FRAME + index + channel) % 5 {
                        0 => -0.0,
                        1 => 0.0,
                        2 => 12.0,
                        3 => -12.0,
                        _ => 0.25,
                    };
                }
            }
            predictor.observe(&pcm, MAX_FRAME, 2, true);
        }
        predictor.prepare(2, true);
        for channel in 0..2 {
            for index in 0..RECENT - 1 {
                let scalar =
                    predictor.past(channel, index + 1) - predictor.past(channel, index + 2);
                assert_eq!(predictor.recent[channel][index].to_bits(), scalar.to_bits());
            }
        }
    }

    #[test]
    fn reset_clears_forecast_and_long_loss_stays_bounded_and_fades() {
        let mut predictor = LossPredictor::new();
        let mut pcm = [[0.0; MAX_FRAME]; 2];
        for (index, value) in pcm[0].iter_mut().enumerate() {
            *value = 0.3 * (std::f32::consts::TAU * index as f32 / 240.0).sin();
        }
        for _ in 0..8 {
            predictor.observe(&pcm, MAX_FRAME, 1, true);
        }
        let mut forecast = std::array::from_fn(|_| loss_buffer());
        let mut first_peak = 0.0f32;
        let mut final_peak = 0.0f32;
        for loss in 0..24 {
            predictor.predict(MAX_FRAME, 1, true, &mut forecast);
            let mut peak = 0.0f32;
            for &sample in &forecast[0][..MAX_FRAME] {
                assert!(sample.is_finite() && sample.abs() <= 8.0);
                peak = peak.max(sample.abs());
            }
            if loss == 0 {
                first_peak = peak;
            }
            final_peak = peak;
            pcm[0].copy_from_slice(&forecast[0][..MAX_FRAME]);
            predictor.observe(&pcm, MAX_FRAME, 1, false);
        }
        assert!(first_peak > 0.1);
        assert!(final_peak < first_peak * 0.05);
        predictor.reset();
        predictor.predict(MAX_FRAME, 2, true, &mut forecast);
        assert!(
            forecast
                .iter()
                .all(|channel| channel.iter().all(|&value| value == 0.0))
        );
        assert_eq!(predictor.elapsed(), 0);
        assert_eq!(predictor.periodicity(), 0.0);
        assert_eq!(predictor.seed, 0x5ac3_1e97);
    }
    #[test]
    fn periodic_shortcut_keeps_zero_bits_and_advances_noise_state() {
        // Frozen forecast-bit traces from the original scalar predictor.
        // In particular, IEEE signed zero depends on the untouched spectral
        // sum and noise arithmetic even when both receive a zero weight.
        for (value, expected_hash) in [
            (0.0, 0xfcfd_a4c0_7d71_b8a5),
            (-0.0, 0x79eb_fb1c_fd71_b8a5),
            (f32::from_bits(1), 0x251e_d3f9_672d_4e85),
            (-f32::from_bits(1), 0x77ed_45fe_672d_4e85),
        ] {
            let mut predictor = LossPredictor::new();
            predictor.available = LOSS_HISTORY;
            predictor.elapsed = 1;
            predictor.pitch = 40;
            predictor.periodicity = 1.0;
            predictor.coefficients = [[0.1; LOSS_ORDER]; 2];
            predictor.residual_rms = [1.0; 2];
            predictor.peak = [0.5; 2];
            for history in &mut predictor.history {
                history.fill(value);
            }
            let mut forecast = std::array::from_fn(|_| loss_buffer());
            predictor.predict(120, 2, true, &mut forecast);
            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            for channel in &forecast {
                for sample in &channel[..240] {
                    hash = (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0100_0000_01b3);
                }
            }
            assert_eq!(hash, expected_hash);
            assert_eq!(predictor.seed, 0x97fc_082a);
        }
    }
}
