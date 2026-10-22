// SPDX-License-Identifier: MIT
//! Finite-grid, noncausal diagnostic. Never linked into production.
pub const PHASES: &[f32] = &[
    0., -0.5, 0.5, -1., 1., -1.5, 1.5, -2., 2., -2.5, 2.5, -3., 3.,
];
const NOISE: &[f32] = &[0., 0.25, 0.5, 0.75, 1.];
const LOW: f64 = 0.7875;
const HIGH: f64 = 1.2125;
pub const HZ: [usize; 17] = [
    0, 200, 400, 600, 800, 1000, 1300, 1600, 2000, 2500, 3200, 4000, 5200, 6800, 9000, 13000, 24000,
];
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Baseline,
    Current,
    GainEnv,
    GainMse,
    PhaseEnv,
    JointEnv,
    JointMse,
}
impl Kind {
    pub const ALL: [Self; 7] = [
        Self::Baseline,
        Self::Current,
        Self::GainEnv,
        Self::GainMse,
        Self::PhaseEnv,
        Self::JointEnv,
        Self::JointMse,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Current => "current",
            Self::GainEnv => "gain_env",
            Self::GainMse => "gain_mse",
            Self::PhaseEnv => "phase_env",
            Self::JointEnv => "joint_env",
            Self::JointMse => "joint_mse",
        }
    }
}
#[derive(Clone)]
pub struct Record {
    pub phase: f32,
    pub noise: f32,
    pub eligible: bool,
    pub horizon: usize,
    pub before: [f64; 16],
    pub after: [f64; 16],
    pub pp: [f64; 16],
    pub pt: [f64; 16],
    pub tt: [f64; 16],
    pub gains: [f64; 16],
}
pub fn stats(p: &[f32], t: &[f32]) -> ([f64; 16], [f64; 16], [f64; 16]) {
    let e = HZ.map(|h| h * p.len() / 24000);
    let (mut pp, mut pt, mut tt) = ([0.; 16], [0.; 16], [0.; 16]);
    for b in 0..16 {
        for k in e[b]..e[b + 1] {
            let (x, y) = (p[k] as f64, t[k] as f64);
            pp[b] += x * x;
            pt[b] += x * y;
            tt[b] += y * y;
        }
    }
    (pp, pt, tt)
}
fn rho(pp: [f64; 16], pt: [f64; 16], tt: [f64; 16]) -> [f64; 16] {
    std::array::from_fn(|b| {
        if pp[b] > 1e-10 && tt[b] > 1e-10 {
            (pt[b] / (pp[b] * tt[b]).sqrt()).clamp(-1., 1.)
        } else {
            f64::NAN
        }
    })
}
fn gains(p: &[f32], t: &[f32], mse: bool) -> [f64; 16] {
    let (pp, pt, tt) = stats(p, t);
    std::array::from_fn(|b| {
        if pp[b] > 0. {
            (if mse {
                pt[b] / pp[b]
            } else {
                (tt[b] / pp[b]).sqrt()
            })
            .clamp(LOW, HIGH)
        } else {
            1.
        }
    })
}
fn apply(p: &[f32], g: [f64; 16]) -> Vec<f32> {
    let e = HZ.map(|h| h * p.len() / 24000);
    let mut out = p.to_vec();
    for b in 0..16 {
        for x in &mut out[e[b]..e[b + 1]] {
            *x *= g[b] as f32;
        }
    }
    out
}
/// Fixed causal pseudo-random MDCT noise; preserve energy and avoid seed search.
fn mix(p: &[f32], alpha: f32, supported: usize, seed: u32) -> Vec<f32> {
    if alpha == 0. {
        return p.to_vec();
    }
    let mut state = seed.max(1);
    let mut noise = vec![0.; p.len()];
    for x in &mut noise {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *x = state as f64 / u32::MAX as f64 - 0.5;
    }
    let e = HZ.map(|h| h * p.len() / 24000);
    let mut out = p.to_vec();
    for b in 7..supported {
        let range = e[b]..e[b + 1];
        let energy = p[range.clone()]
            .iter()
            .map(|&x| (x as f64).powi(2))
            .sum::<f64>();
        if energy == 0. {
            continue;
        }
        let dot = range.clone().map(|k| p[k] as f64 * noise[k]).sum::<f64>() / energy;
        for k in range.clone() {
            noise[k] -= dot * p[k] as f64;
        }
        let norm = range.clone().map(|k| noise[k] * noise[k]).sum::<f64>();
        if norm < 1e-30 {
            continue;
        }
        let scale = (energy / norm).sqrt();
        for k in range {
            out[k] = ((1. - alpha as f64).sqrt() * p[k] as f64
                + (alpha as f64).sqrt() * noise[k] * scale) as f32;
        }
    }
    out
}
fn spectral(p: &[f32], t: &[f32], supported: usize) -> f64 {
    let end = HZ[supported] * p.len() / 24000;
    let total = p[..end]
        .iter()
        .chain(&t[..end])
        .map(|&v| (v as f64).powi(2))
        .sum::<f64>()
        .max(1e-10);
    p[..end]
        .chunks(4)
        .zip(t[..end].chunks(4))
        .map(|(x, y)| {
            let a = x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / total;
            let b = y.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / total;
            ((a + 0.001).ln() - (b + 0.001).ln()).powi(2)
        })
        .sum::<f64>()
        / end.div_ceil(4) as f64
}
pub fn select(
    candidates: &[Vec<f32>],
    target: &[f32],
    input: &[f32; 80],
    kind: Kind,
    supported: usize,
    seed: u32,
) -> (Vec<f32>, Record) {
    assert_eq!(candidates.len(), 13);
    assert!(supported == 13 || supported == 16);
    let base = &candidates[0];
    let (pp, pt, tt) = stats(base, target);
    let before = rho(pp, pt, tt);
    let eligible = input[78] >= 1.
        && input[76] < 0.985
        && input[73] < input[77]
        && base.iter().any(|&x| x != 0.);
    let mut r = Record {
        phase: 0.,
        noise: 0.,
        eligible,
        horizon: (input[74] * 6.).round() as usize + 1,
        before,
        after: before,
        pp,
        pt,
        tt,
        gains: [1.; 16],
    };
    if !eligible || kind == Kind::Baseline {
        return (base.clone(), r);
    }
    if kind == Kind::Current {
        let mut out = base.clone();
        crate::neural_plc::refine(&mut out, input);
        r.gains = crate::neural_plc::gains(input).map(f64::from);
        return (out, r);
    }
    if matches!(kind, Kind::GainEnv | Kind::GainMse) {
        r.gains = gains(base, target, kind == Kind::GainMse);
        return (apply(base, r.gains), r);
    }
    let mut selected = base.clone();
    let mut best = f64::INFINITY;
    for (i, p) in candidates.iter().enumerate() {
        for &alpha in if kind == Kind::PhaseEnv {
            &NOISE[..1]
        } else {
            NOISE
        } {
            let mixed = mix(p, alpha, supported, seed);
            let g = gains(&mixed, target, kind == Kind::JointMse);
            let out = apply(&mixed, g);
            let (a, b, c) = stats(&mixed, target);
            let correlations = rho(a, b, c);
            let score = if kind == Kind::PhaseEnv {
                let values: Vec<_> = correlations[..11]
                    .iter()
                    .copied()
                    .filter(|x| x.is_finite())
                    .collect();
                if values.is_empty() {
                    0.
                } else {
                    -values.iter().sum::<f64>() / values.len() as f64
                }
            } else if kind == Kind::JointMse {
                let end = HZ[supported] * out.len() / 24000;
                out[..end]
                    .iter()
                    .zip(&target[..end])
                    .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
                    .sum::<f64>()
            } else {
                spectral(&out, target, supported)
            };
            if score < best {
                best = score;
                selected = out;
                r.phase = PHASES[i];
                r.noise = alpha;
                r.after = correlations;
                r.pp = a;
                r.pt = b;
                r.tt = c;
                r.gains = g;
            }
        }
    }
    (selected, r)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noise_preserves_energy_and_unchanged_lowbands() {
        let p: Vec<_> = (0..960).map(|i| (i as f32 * 0.31).sin()).collect();
        let e = HZ.map(|h| h * 960 / 24000);
        for a in NOISE {
            let m = mix(&p, *a, 13, 20261022);
            assert_eq!(&p[..e[7]], &m[..e[7]]);
            assert_eq!(&p[e[13]..], &m[e[13]..]);
            let (pp, _, tt) = stats(&p, &m);
            for b in 0..16 {
                assert!((pp[b] - tt[b]).abs() < 1e-5 * pp[b].max(1.));
            }
            assert_eq!(m, mix(&p, *a, 13, 20261022));
        }
        assert!(mix(&[0.; 960], 1., 16, 9).iter().all(|&x| x == 0.));
    }
    #[test]
    fn identity_and_safety_guard() {
        let p = vec![0.2; 960];
        let candidates = vec![p.clone(); 13];
        let mut x = [0.; 80];
        x[78] = 1.;
        x[77] = 1.;
        for kind in Kind::ALL {
            let (out, r) = select(&candidates, &p, &x, kind, 16, 20261022);
            if kind != Kind::Current {
                assert_eq!(out, p);
            }
            assert!(r.phase == 0. && r.noise == 0.);
        }
        x[78] = 0.;
        for kind in Kind::ALL {
            assert_eq!(select(&candidates, &p, &x, kind, 16, 1).0, p);
        }
    }
    #[test]
    fn phase_search_recovers_known_candidate() {
        let target: Vec<_> = (0..960).map(|i| (i as f32 * 0.3).sin()).collect();
        let mut c = vec![target.iter().map(|x| -x).collect::<Vec<_>>(); 13];
        c[5] = target.clone();
        let mut x = [0.; 80];
        x[78] = 1.;
        x[77] = 1.;
        for k in [Kind::PhaseEnv, Kind::JointMse] {
            let (out, r) = select(&c, &target, &x, k, 16, 12);
            assert_eq!(r.phase, PHASES[5]);
            assert_eq!(r.noise, 0.);
            assert_eq!(out, target);
        }
        let (out, r) = select(&c, &target, &x, Kind::JointEnv, 16, 12);
        assert_eq!(r.phase, 0.);
        assert_eq!(out, c[0]);
    }
}
