// SPDX-License-Identifier: MIT
//! Experimental conversion only. Never interpret the installed gain head as d.
pub const MODEL_KIND: &str = "target-log-power-phase-v1";
pub const EPS: f32 = 1e-12;
pub const LOWER: f32 = 0.7875;
pub const UPPER: f32 = 1.2125;

pub fn log_power(coefficients: &[f32]) -> [f32; 16] {
    let edges = crate::neural_plc::edges(coefficients.len());
    std::array::from_fn(|band| {
        let values = &coefficients[edges[band]..edges[band + 1]];
        (values.iter().map(|x| x * x).sum::<f32>() / values.len() as f32 + EPS).ln()
    })
}

pub fn gains(candidate: &[f32], last: &[f32; 16], delta: &[f32; 16]) -> [f32; 16] {
    if candidate
        .iter()
        .chain(last)
        .chain(delta)
        .any(|x| !x.is_finite())
    {
        return [1.0; 16];
    }
    let predicted = log_power(candidate);
    std::array::from_fn(|band| {
        (0.5 * (delta[band] + last[band] - predicted[band]))
            .clamp(LOWER.ln(), UPPER.ln())
            .exp()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fading_candidate_needs_a_fading_target_for_identity() {
        for n in [480, 960] {
            let last = log_power(&vec![0.1; n]);
            for fade in [0.8, 0.5, 0.2] {
                let candidate = vec![0.1 * fade; n];
                let current = log_power(&candidate);
                let identity = std::array::from_fn(|i| current[i] - last[i]);
                assert!(
                    gains(&candidate, &last, &identity)
                        .iter()
                        .all(|g| (*g - 1.0).abs() < 2e-6)
                );
                assert!(
                    gains(&candidate, &last, &[0.0; 16])
                        .iter()
                        .all(|g| (*g - UPPER).abs() < 2e-6)
                );
            }
        }
    }
    #[test]
    fn silence_invalid_values_and_extremes_are_safe() {
        let silence = [0.0; 960];
        let last = log_power(&silence);
        assert_eq!(gains(&silence, &last, &[0.0; 16]), [1.0; 16]);
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(gains(&silence, &last, &[value; 16]), [1.0; 16]);
        }
        for value in [-1e20, 1e20] {
            assert!(
                gains(&silence, &last, &[value; 16])
                    .iter()
                    .all(|g| g.is_finite() && *g >= LOWER - 1e-6 && *g <= UPPER + 1e-6)
            );
        }
    }
}
