// SPDX-License-Identifier: MIT
//! Finite-grid, noncausal diagnostic. Never linked into production.
pub const PHASES: &[f32] = &[
    0., -0.5, 0.5, -1., 1., -1.5, 1.5, -2., 2., -2.5, 2.5, -3., 3.,
    -4.,4.,-5.,5.,-6.,6.,-7.,7.,-8.,8.,
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
    PhaseNarrow,
    PhaseWide,
    PhaseNoise,
    Trajectory,
}
impl Kind {
    pub const ALL: [Self; 7] = [
        Self::Baseline,
        Self::Current,
        Self::GainEnv,
        Self::PhaseNarrow,
        Self::PhaseWide,
        Self::PhaseNoise,
        Self::Trajectory,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Current => "current",
            Self::GainEnv => "gain_env",
            Self::PhaseNarrow => "phase_narrow",
            Self::PhaseWide => "phase_wide",
            Self::PhaseNoise => "phase_noise",
            Self::Trajectory => "trajectory",
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
    assert_eq!(candidates.len(), PHASES.len()+1);
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
    if kind == Kind::GainEnv {
        r.gains = gains(base, target, false);
        return (apply(base, r.gains), r);
    }
    if kind == Kind::Trajectory {
        let selected=candidates.last().unwrap().clone();
        let (a,b,c)=stats(&selected,target);
        r.phase=trajectory(input,base.len()/2);
        r.after=rho(a,b,c);r.pp=a;r.pt=b;r.tt=c;
        return (selected,r);
    }
    // Both wide variants choose phase from the unmixed low/mid-band rho.
    // Noise is selected only afterwards; it cannot change the phase objective.
    let mut best=f64::INFINITY;
    let mut selected_index=0;
    for (i,p) in candidates[..PHASES.len()].iter().enumerate() {
        if kind==Kind::PhaseNarrow && PHASES[i].abs()>3. {continue;}
        let (a,b,c)=stats(p,target);let correlations=rho(a,b,c);
        let values:Vec<_>=correlations[..11].iter().copied().filter(|x|x.is_finite()).collect();
        let score=if values.is_empty(){0.}else{-values.iter().sum::<f64>()/values.len() as f64};
        if score<best {best=score;selected_index=i;}
    }
    let phase=&candidates[selected_index];
    let mut selected=phase.clone();let mut score=f64::INFINITY;
    for &alpha in if kind==Kind::PhaseNoise {NOISE} else {&NOISE[..1]} {
        let mixed=mix(phase,alpha,supported,seed);let g=gains(&mixed,target,false);let out=apply(&mixed,g);
        let candidate_score=high_spectral(&out,target,supported);
        if candidate_score<score {
            score=candidate_score;selected=out;
            let (a,b,c)=stats(&mixed,target);
            r.phase=PHASES[selected_index];r.noise=alpha;r.after=rho(a,b,c);r.pp=a;r.pt=b;r.tt=c;r.gains=g;
        }
    }
    (selected,r)
}

/// Damped quadratic phase from three received f0 values, no learned parameters.
/// Time is in native48kHz samples; missing/unvoiced/inconsistent pitch is exact0.
pub fn trajectory(input:&[f32;80],ahead:usize)->f32 {
    if [18,42,66].iter().any(|&i|input[i]<1.) {return 0.;}
    let f0=input[17]*500.;let f1=input[41]*500.;let f2=input[65]*500.;
    if [f0,f1,f2].iter().any(|&f|!(50. ..=500.).contains(&f)) {return 0.;}
    let period=input[75]*960.;
    if period<=0. || (48000./f2-period).abs()>period*0.2 {return 0.;}
    let n=input[72]*960.;if n<=0. {return 0.;}
    let t=input[73]*3840.+ahead as f32;
    let slope=(f2-f0)/(2.*n);
    let offset=0.5*slope/f2*t*t/(1.+t/(2.*n));
    offset.clamp(-8.,8.)
}

fn high_spectral(p:&[f32],t:&[f32],supported:usize)->f64 {
    let start=HZ[7]*p.len()/24000;let end=HZ[supported]*p.len()/24000;
    // Use the same four-bin log-energy form, confined to the supported highband.
    spectral(&p[start..end],&t[start..end],16)
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]fn absent_pitch_is_exact_identity(){assert_eq!(trajectory(&[0.;80],960),0.);}
 #[test]fn constant_pitch_is_exact_identity(){
  let mut x=[0.;80];for i in [17,41,65]{x[i]=0.4;}for i in [18,42,66]{x[i]=1.;}
  x[72]=1.;x[75]=0.25;assert_eq!(trajectory(&x,960),0.);
  x[65]=0.42;assert!(trajectory(&x,960)>0.);assert!(trajectory(&x,100000)<=8.);
 }
 #[test]fn noise_does_not_select_another_phase(){
  let base:Vec<f32>=(0..480).map(|i|(i as f32*0.13).sin()).collect();
  let candidates=vec![base.clone();PHASES.len()+1];let mut x=[0.;80];x[78]=1.;x[77]=1.;
  let (_,wide)=select(&candidates,&base,&x,Kind::PhaseWide,16,9);
  let (_,noise)=select(&candidates,&base,&x,Kind::PhaseNoise,16,9);
  assert_eq!(wide.phase,noise.phase);assert_eq!(noise.noise,0.);
 }
}
