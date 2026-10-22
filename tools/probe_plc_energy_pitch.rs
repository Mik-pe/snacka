// SPDX-License-Identifier: MIT
//! Standalone feasibility probe, not a production decoder or quality test.
//! rustc --edition=2024 -C opt-level=3 tools/probe_plc_energy_pitch.rs -o /tmp/plc-probe
#![allow(dead_code)]
use std::{hint::black_box, time::Instant};

const FRAME_SAMPLES: usize = 960;
#[path = "../src/neural_plc.rs"]
mod neural_plc;
#[path = "../src/neural_plc_weights.rs"]
mod neural_plc_weights;
const EPS: f32 = 1e-12;
const LOWER: f32 = 0.7875;
const UPPER: f32 = 1.2125;

fn log_energy(coefficients: &[f32]) -> [f32; 16] {
    let edges = neural_plc::edges(coefficients.len());
    std::array::from_fn(|band| {
        let values = &coefficients[edges[band]..edges[band + 1]];
        (values.iter().map(|x| x * x).sum::<f32>() / values.len() as f32 + EPS).ln()
    })
}

// delta_log_energy predicts log(mean target power + eps) relative to the last
// received frame. Measure the *actual phase-shifted* candidate after its MDCT.
// Clamp in log space before exp: finite extreme logits cannot overflow exp.
fn energy_gains(
    candidate: &[f32],
    last_log_energy: &[f32; 16],
    delta_log_energy: &[f32; 16],
) -> [f32; 16] {
    let current = log_energy(candidate);
    let low = LOWER.ln();
    let high = UPPER.ln();
    std::array::from_fn(|band| {
        (0.5 * (delta_log_energy[band] + last_log_energy[band] - current[band]))
            .clamp(low, high)
            .exp()
    })
}

mod native {
    // Compile the current native search unchanged; diagnostics can read private
    // state inside this module without adding a public codec API or new state.
    include!("../src/concealment.rs");

    pub fn history(values: &[f32; 1920]) -> Concealment {
        let mut state = Concealment::new();
        state.observe(values);
        state.begin();
        state
    }

    pub fn summary(state: &Concealment) -> (f32, f32) {
        (state.period, state.confidence)
    }

    fn local_period(state: &Concealment, delay: usize) -> Option<(f32, f32, bool)> {
        // Matched 10 ms windows, 10 ms apart. A full preceding 20 ms window
        // does not fit alongside a positive lag in the native 40 ms history.
        if state.length < HISTORY || state.confidence < 0.55 {
            return None;
        }
        let window = 480;
        let end = state.length.checked_sub(delay)?;
        let center = state.period.round() as usize;
        let low = center.saturating_sub(6).max(80);
        let high = (center + 6).min(960).min(end.checked_sub(window)?);
        if low >= high {
            return None;
        }
        let samples = &state.history[..end];
        let power = energy(samples, window, 3);
        if power <= 1e-20 {
            return None;
        }
        let mut lag = low;
        let mut best = -1.0;
        for candidate in low..=high {
            let score = correlation(samples, window, candidate, 3, power);
            if score > best {
                lag = candidate;
                best = score;
            }
        }
        if best < 0.55 {
            return None;
        }
        let edge = lag == low || lag == high;
        let fraction = if !edge {
            let left = correlation(samples, window, lag - 1, 3, power);
            let right = correlation(samples, window, lag + 1, 3, power);
            let curvature = left - 2.0 * best + right;
            if curvature < -1e-6 {
                (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        Some((lag as f32 + fraction, best, edge))
    }

    pub fn previous_only(state: &Concealment) -> Option<(f32, f32, bool)> {
        local_period(state, 480)
    }

    pub fn matched_drift(state: &Concealment) -> Option<(f32, f32, bool)> {
        let current = local_period(state, 0)?;
        let previous = local_period(state, 480)?;
        Some((
            current.0 - previous.0,
            current.1.min(previous.1),
            current.2 || previous.2,
        ))
    }
}

fn signal(frequency: f32, slope_hz_per_second: f32) -> [f32; 1920] {
    std::array::from_fn(|index| {
        let t = index as f32 / 48000.0;
        let phase = std::f32::consts::TAU * (frequency * t + 0.5 * slope_hz_per_second * t * t);
        0.25 * phase.sin() + 0.08 * (2.0 * phase + 0.2).sin()
    })
}

fn noisy_signal() -> [f32; 1920] {
    let mut values = signal(173.0, 35.0);
    let mut state = 20261022u32;
    for value in &mut values {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *value += 0.16 * (state as f32 / u32::MAX as f32 - 0.5);
    }
    values
}

fn timing<T>(name: &str, n: usize, mut operation: impl FnMut() -> T) {
    for _ in 0..4096 {
        black_box(operation());
    }
    for repetition in 0..21 {
        let calls = 8192;
        let start = Instant::now();
        for _ in 0..calls {
            black_box(operation());
        }
        println!(
            "{name},{n},{repetition},{calls},{:.6}",
            start.elapsed().as_nanos() as f64 / calls as f64
        );
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--diagnose") {
        println!("frequency,slope,period,confidence,drift,local_confidence,edge");
        for frequency in [60.0, 80.0, 120.0, 200.0, 350.0] {
            for slope in [-100.0, 0.0, 100.0] {
                let state = native::history(&signal(frequency, slope));
                let (period, confidence) = native::summary(&state);
                if let Some((drift, local_confidence, edge)) = native::matched_drift(&state) {
                    println!(
                        "{frequency},{slope},{period},{confidence},{drift},{local_confidence},{edge}"
                    );
                } else {
                    println!("{frequency},{slope},{period},{confidence},NA,NA,NA");
                }
            }
        }
        let state = native::history(&noisy_signal());
        let (period, confidence) = native::summary(&state);
        let (drift, local_confidence, edge) =
            native::matched_drift(&state).expect("active noisy example");
        println!("173_noisy,35,{period},{confidence},{drift},{local_confidence},{edge}");
        return;
    }
    println!("component,samples,repetition,calls,nanoseconds_per_call");
    for samples in [480, 960] {
        let coefficients: Vec<f32> = (0..samples)
            .map(|i| (i as f32 * 0.371).sin() * 0.17)
            .collect();
        let last = log_energy(&coefficients);
        let delta = std::array::from_fn(|i| (i as f32 * 0.13).sin() * 0.2);
        timing("band_log_energy", samples, || {
            log_energy(black_box(&coefficients))
        });
        timing("energy_gains", samples, || {
            energy_gains(
                black_box(&coefficients),
                black_box(&last),
                black_box(&delta),
            )
        });
    }
    let values = noisy_signal();
    let state = native::history(&values);
    let confidence = native::summary(&state).1;
    assert!((0.55..0.985).contains(&confidence));
    assert!(native::matched_drift(&state).is_some());
    timing("native_pitch_begin", 1920, || {
        native::history(black_box(&values))
    });
    timing("previous_13_lags", 1920, || {
        native::previous_only(black_box(&state))
    });
    timing("matched_26_lags", 1920, || {
        native::matched_drift(black_box(&state))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytic_gains_match_unsaturated_and_clamped_teacher_ratios() {
        for samples in [480, 960] {
            let last: Vec<f32> = (0..samples)
                .map(|i| 0.1 + 0.01 * (i as f32).sin())
                .collect();
            let previous = log_energy(&last);
            for amplitude in [0.0, 0.2, 0.9, 1.0, 1.1, 3.0] {
                let target: Vec<_> = last.iter().map(|x| x * amplitude).collect();
                let target_log = log_energy(&target);
                let delta = std::array::from_fn(|i| target_log[i] - previous[i]);
                let actual = energy_gains(&last, &previous, &delta);
                let edges = neural_plc::edges(samples);
                for band in 0..16 {
                    let mean = last[edges[band]..edges[band + 1]]
                        .iter()
                        .map(|x| x * x)
                        .sum::<f32>()
                        / (edges[band + 1] - edges[band]) as f32;
                    let expected = ((mean * amplitude * amplitude + EPS) / (mean + EPS))
                        .sqrt()
                        .clamp(LOWER, UPPER);
                    assert!((actual[band] - expected).abs() < 2e-6);
                }
            }
        }
    }

    #[test]
    fn finite_extremes_and_silence_stay_bounded() {
        let silence = vec![0.0; 960];
        let last = log_energy(&silence);
        for delta in [[0.0; 16], [1e20; 16], [-1e20; 16]] {
            for gain in energy_gains(&silence, &last, &delta) {
                assert!(gain.is_finite() && (LOWER - 1e-7..=UPPER + 1e-7).contains(&gain));
            }
        }
        assert_eq!(energy_gains(&silence, &last, &[0.0; 16]), [1.0; 16]);
    }

    #[test]
    fn local_pitch_rejects_silence_and_incomplete_history() {
        assert!(native::matched_drift(&native::history(&[0.0; 1920])).is_none());
        let mut state = native::Concealment::new();
        state.observe(&[0.1; 960]);
        state.begin();
        assert!(native::matched_drift(&state).is_none());
    }

    #[test]
    fn local_pitch_recovers_stationarity_and_the_sign_of_slow_drift() {
        for frequency in [80.0, 120.0, 200.0, 350.0] {
            let steady = native::matched_drift(&native::history(&signal(frequency, 0.0))).unwrap();
            assert!(steady.0.abs() < 0.1 && !steady.2);
            for slope in [-40.0, 40.0] {
                let drift =
                    native::matched_drift(&native::history(&signal(frequency, slope))).unwrap();
                assert!(drift.0 * slope < 0.0);
            }
        }
    }

    #[test]
    fn normalized_history_cannot_recover_exact_last_band_energy() {
        for samples in [480, 960] {
            let mut a = vec![0.1; samples];
            let mut b = a.clone();
            let edge = neural_plc::edges(samples)[1];
            a[..edge].fill(1e-5);
            b[..edge].fill(2e-5);
            let mut first = neural_plc::History::new();
            let mut second = neural_plc::History::new();
            for _ in 0..3 {
                first.observe(&a, 24000, 0);
                second.observe(&b, 24000, 0);
            }
            assert_eq!(
                first.input(samples, [0.0; 5]),
                second.input(samples, [0.0; 5])
            );
            assert!((log_energy(&a)[0] - log_energy(&b)[0]).abs() > 1.0);
        }
    }
}
