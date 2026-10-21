// SPDX-License-Identifier: MIT
//! Noncausal diagnostic only. Never linked into the shipped codec.
pub const LOW: f64 = 0.7875;
pub const HIGH: f64 = 1.2125;
pub const TOLERANCE: f64 = 1e-9;
const REGULARIZATION: f64 = 0.01 / 16.0;
const ENVELOPE: f64 = 0.1 / 16.0;
const FLOOR: f64 = 0.001;

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Baseline,
    Current,
    Mdct,
    Envelope,
    Combined,
    Wide,
}
impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Baseline,
        Self::Current,
        Self::Mdct,
        Self::Envelope,
        Self::Combined,
        Self::Wide,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Current => "current",
            Self::Mdct => "oracle_mdct",
            Self::Envelope => "oracle_env",
            Self::Combined => "oracle_combined",
            Self::Wide => "oracle_wide",
        }
    }
}

pub fn objective(a: f64, b: f64, c: f64, g: f64) -> f64 {
    (a * g * g - 2.0 * b * g + c).max(0.0)
        + REGULARIZATION * (g - 1.0).powi(2)
        + ENVELOPE * ((a * g * g + FLOOR) / (c + FLOOR)).ln().powi(2)
}

/// Interval branch-and-bound. The quadratic and monotone log terms each supply
/// a lower bound on an interval. Their sum bounds the whole objective even
/// when it is nonconvex. Termination certifies absolute objective gap <= 1e-9
/// per band, apart from floating-point rounding (not symbolic exactness).
pub fn combined(a: f64, b: f64, c: f64, low: f64, high: f64) -> f64 {
    let mut best = 1.0f64.clamp(low, high);
    let mut value = objective(a, b, c, best);
    for g in [
        low,
        high,
        if a > 0.0 {
            (b / a).clamp(low, high)
        } else {
            best
        },
        if a > 0.0 {
            (c / a).sqrt().clamp(low, high)
        } else {
            best
        },
    ] {
        let f = objective(a, b, c, g);
        if f < value {
            value = f;
            best = g;
        }
    }
    let mut pending = vec![(low, high)];
    let mut iterations = 0;
    while let Some((left, right)) = pending.pop() {
        iterations += 1;
        assert!(
            iterations < 100_000,
            "oracle optimizer did not certify convergence: {a} {b} {c}"
        );
        let q = ((b + REGULARIZATION) / (a + REGULARIZATION)).clamp(left, right);
        let qmin = (a * q * q - 2.0 * b * q + c + REGULARIZATION * (q - 1.0).powi(2)).max(0.0);
        let l = ((a * left * left + FLOOR) / (c + FLOOR)).ln();
        let r = ((a * right * right + FLOOR) / (c + FLOOR)).ln();
        let logmin = if l <= 0.0 && r >= 0.0 {
            0.0
        } else {
            l.abs().min(r.abs()).powi(2)
        };
        let mid = (left + right) * 0.5;
        assert!(mid > left && mid < right, "oracle interval lost precision");
        let f = objective(a, b, c, mid);
        if f < value {
            value = f;
            best = mid;
        }
        // Taylor lower bound retains cancellation between the two gradients.
        // M bounds |F''| throughout this interval. Taking the tighter of both
        // bounds avoids pathological splitting when the terms oppose strongly.
        let d = a * mid * mid + FLOOR;
        let derivative = 2.0 * ((a + REGULARIZATION) * mid - (b + REGULARIZATION))
            + 4.0 * ENVELOPE * a * mid / d * (d / (c + FLOOR)).ln();
        let curvature = 2.0 * (a + REGULARIZATION)
            + 4.0
                * ENVELOPE
                * a
                * ((FLOOR + a * right * right) * l.abs().max(r.abs()) + 2.0 * a * right * right)
                / (a * left * left + FLOOR).powi(2);
        let radius = (right - left) * 0.5;
        let taylor = f - derivative.abs() * radius - 0.5 * curvature * radius * radius;
        if value - (qmin + ENVELOPE * logmin).max(taylor) <= TOLERANCE {
            continue;
        }
        pending.push((left, mid));
        pending.push((mid, right));
    }
    best
}

pub fn optimal(kind: Kind, pp: f64, pt: f64, tt: f64, total: f64) -> f64 {
    match kind {
        Kind::Mdct => {
            if pp > 0.0 {
                (pt / pp).clamp(LOW, HIGH)
            } else {
                1.0
            }
        }
        Kind::Envelope => {
            if pp > 0.0 {
                (tt / pp).sqrt().clamp(LOW, HIGH)
            } else {
                1.0
            }
        }
        Kind::Combined => combined(pp / total, pt / total, tt / total, LOW, HIGH),
        Kind::Wide => combined(pp / total, pt / total, tt / total, 0.5, 1.5),
        _ => 1.0,
    }
}

pub struct Record {
    pub eligible: bool,
    pub horizon: u8,
    pub pp: [f64; 16],
    pub pt: [f64; 16],
    pub tt: [f64; 16],
    pub mlp: [f64; 16],
    pub gain: [f64; 16],
}
impl Record {
    pub fn normalized_errors(&self) -> (f64, f64, f64) {
        let total = (self.pp.iter().sum::<f64>() + self.tt.iter().sum::<f64>()).max(1e-10);
        let mut mdct = 0.0;
        let mut applied = 0.0;
        let mut training_scale = 0.0;
        for b in 0..16 {
            let (a, p, c) = (self.pp[b] / total, self.pt[b] / total, self.tt[b] / total);
            let g = self.gain[b];
            mdct += (a * g * g - 2.0 * p * g + c).max(0.0);
            applied += objective(a, p, c, g);
            training_scale += objective(a, p, c, 1.0 + (g - 1.0) / 0.425);
        }
        (mdct, applied, training_scale)
    }
    pub fn write(
        &self,
        out: &mut impl std::io::Write,
        frame: usize,
        samples: usize,
    ) -> std::io::Result<()> {
        out.write_all(&(frame as u32).to_le_bytes())?;
        out.write_all(&(samples as u16).to_le_bytes())?;
        out.write_all(&[u8::from(self.eligible), self.horizon])?;
        for x in self
            .pp
            .iter()
            .chain(&self.pt)
            .chain(&self.tt)
            .chain(&self.mlp)
        {
            out.write_all(&x.to_le_bytes())?;
        }
        let total = (self.pp.iter().sum::<f64>() + self.tt.iter().sum::<f64>()).max(1e-10);
        for kind in [Kind::Mdct, Kind::Envelope, Kind::Combined, Kind::Wide] {
            for b in 0..16 {
                let g = if self.eligible {
                    optimal(kind, self.pp[b], self.pt[b], self.tt[b], total)
                } else {
                    1.0
                };
                out.write_all(&(g as f32).to_le_bytes())?;
            }
        }
        Ok(())
    }
}

pub fn refine(coefficients: &mut [f32], target: &[f32], input: &[f32; 80], kind: Kind) -> Record {
    let eligible = input[78] >= 1.0
        && input[76] < 0.985
        && input[73] < input[77]
        && coefficients.iter().any(|&x| x != 0.0);
    let mut record = Record {
        eligible,
        horizon: (input[74] * 6.0).round() as u8 + 1,
        pp: [0.0; 16],
        pt: [0.0; 16],
        tt: [0.0; 16],
        mlp: [1.0; 16],
        gain: [1.0; 16],
    };
    let edges = crate::neural_plc::edges(coefficients.len());
    for b in 0..16 {
        for (&p, &t) in coefficients[edges[b]..edges[b + 1]]
            .iter()
            .zip(&target[edges[b]..edges[b + 1]])
        {
            let (p, t) = (f64::from(p), f64::from(t));
            record.pp[b] += p * p;
            record.pt[b] += p * t;
            record.tt[b] += t * t;
        }
    }
    if !eligible {
        return record;
    }
    record.mlp = crate::neural_plc::gains(input).map(f64::from);
    if matches!(kind, Kind::Current) {
        record.gain = record.mlp;
        crate::neural_plc::refine(coefficients, input);
    } else if !matches!(kind, Kind::Baseline) {
        let total = (record.pp.iter().sum::<f64>() + record.tt.iter().sum::<f64>()).max(1e-10);
        for b in 0..16 {
            let g = optimal(kind, record.pp[b], record.pt[b], record.tt[b], total) as f32;
            record.gain[b] = f64::from(g);
            for x in &mut coefficients[edges[b]..edges[b + 1]] {
                *x *= g;
            }
        }
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn certified_solver_beats_dense_grid_including_nonconvex_and_zero_energy() {
        let mut state = 123456789u64;
        for _ in 0..128 {
            let mut rand = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 11) as f64 / (1u64 << 53) as f64
            };
            let a = 10f64.powf(-9.0 + 9.0 * rand());
            let c = 10f64.powf(-9.0 + 9.0 * rand());
            let b = (2.0 * rand() - 1.0) * (a * c).sqrt();
            for (low, high) in [(LOW, HIGH), (0.5, 1.5)] {
                let g = combined(a, b, c, low, high);
                assert!((low..=high).contains(&g));
                let grid = (0..=4096)
                    .map(|i| objective(a, b, c, low + (high - low) * i as f64 / 4096.0))
                    .fold(f64::INFINITY, f64::min);
                assert!(objective(a, b, c, g) <= grid + TOLERANCE * 2.0);
            }
            let tight = combined(a, b, c, LOW, HIGH);
            let wide = combined(a, b, c, 0.5, 1.5);
            assert!(objective(a, b, c, wide) <= objective(a, b, c, tight) + TOLERANCE * 2.0);
        }
        assert_eq!(combined(0.0, 0.0, 0.0, LOW, HIGH), 1.0);
    }
    #[test]
    fn mdct_and_envelope_oracles_disagree_for_opposite_phase() {
        assert_eq!(optimal(Kind::Mdct, 1.0, -1.2, 1.44, 2.44), LOW);
        assert!((optimal(Kind::Envelope, 1.0, -1.2, 1.44, 2.44) - 1.2).abs() < 1e-12);
    }
}
