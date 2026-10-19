// SPDX-License-Identifier: MIT
//! Safe, zero-allocation neural MDCT postfilter and denoiser.
//!
//! Operates strictly within each decoded frame across 16 fine spectral subbands
//! and a 7-tap polarity-equivariant local neighborhood, preserving zero
//! inter-frame error propagation after packet loss and zero heap allocations.

use crate::FRAME_SAMPLES;
use crate::neural_weights::{
    B1, B2, B3, BIN_HIDDEN, BIN_OUT, GLOBAL_DIM, GLOBAL_H1, GLOBAL_H2, HEAD_B1, HEAD_B2, HEAD_S1,
    HEAD_S2, HEAD_W1, HEAD_W2, LOCAL_DIM, NUM_SUBBANDS, S1, S2, S3, SUBBAND_OUT, SUBBAND_STRIDE,
    W1, W2, W3,
};

/// Frame-level codec parameters conditioning the neural MDCT refinement filter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameMetadata {
    /// Transform length in samples (`480` for 10 ms, `960` for 20 ms).
    pub samples: usize,
    /// Number of coded MDCT frequency bins (`160`, `240`, `320`, or `480`).
    pub coded_bins: usize,
    /// Payload size in bytes (excluding header and CRC).
    pub payload_bytes: usize,
    /// Whether the packet used the harmonic speech synthesis profile.
    pub is_harmonic: bool,
    /// Fundamental frequency in Hz when `is_harmonic` is true (`0` otherwise).
    pub f0: usize,
    /// Quantized band gain codes (`0..=208`) for the four coarse codec bands.
    pub codes: [u8; 4],
    /// Pulse allocation counts for the four coarse codec bands.
    pub allocation: [usize; 4],
}

#[inline]
fn fast_sig(x: f32) -> f32 {
    x / (1.0 + x.abs())
}

#[inline]
fn dot_i8_64(weights: &[i8; 64], inputs: &[f32; 64]) -> f32 {
    let mut sum0 = 0.0f32;
    let mut sum1 = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut sum3 = 0.0f32;
    let mut i = 0;
    while i < 64 {
        sum0 += f32::from(weights[i]) * inputs[i];
        sum1 += f32::from(weights[i + 1]) * inputs[i + 1];
        sum2 += f32::from(weights[i + 2]) * inputs[i + 2];
        sum3 += f32::from(weights[i + 3]) * inputs[i + 3];
        i += 4;
    }
    (sum0 + sum1) + (sum2 + sum3)
}

#[inline]
fn dot_i8_16(weights: &[i8; 16], inputs: &[f32; 16]) -> f32 {
    let mut sum0 = 0.0f32;
    let mut sum1 = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut sum3 = 0.0f32;
    let mut i = 0;
    while i < 16 {
        sum0 += f32::from(weights[i]) * inputs[i];
        sum1 += f32::from(weights[i + 1]) * inputs[i + 1];
        sum2 += f32::from(weights[i + 2]) * inputs[i + 2];
        sum3 += f32::from(weights[i + 3]) * inputs[i + 3];
        i += 4;
    }
    (sum0 + sum1) + (sum2 + sum3)
}

/// Compute coarse band RMS scales and the 64-element global frame feature vector.
pub(crate) fn compute_frame_features(
    coefficients: &[f32],
    meta: &FrameMetadata,
    edges: &[usize; 5],
    group_sigma: &mut [f32; 4],
    global_features: &mut [f32; GLOBAL_DIM],
) {
    for g in 0..4 {
        let start = edges[g];
        let end = edges[g + 1];
        let dim = (end - start).max(1);
        let mut energy = 0.0f32;
        for &val in &coefficients[start..end] {
            energy += val * val;
        }
        group_sigma[g] = (energy / dim as f32 + 1e-12).sqrt();
    }

    let sub_bins = meta.coded_bins / NUM_SUBBANDS;
    let inv_sub_bins = 1.0 / sub_bins as f32;
    let mut log_rms = [0.0f32; NUM_SUBBANDS];
    let mut density = [0.0f32; NUM_SUBBANDS];
    let mut max_log_rms = -10.0f32;

    for s in 0..NUM_SUBBANDS {
        let start = s * sub_bins;
        let end = start + sub_bins;
        let mut energy = 0.0f32;
        let mut active = 0usize;
        for &val in &coefficients[start..end] {
            energy += val * val;
            if val.abs() > 1e-6 {
                active += 1;
            }
        }
        let rms = (energy * inv_sub_bins).sqrt();
        let l_rms = ((rms + 1e-5).ln() * std::f32::consts::LOG2_E + 10.0) * 0.1;
        log_rms[s] = l_rms;
        density[s] = active as f32 * inv_sub_bins;
        if l_rms > max_log_rms {
            max_log_rms = l_rms;
        }
    }

    for s in 0..NUM_SUBBANDS {
        global_features[s] = log_rms[s];
        global_features[NUM_SUBBANDS + s] = log_rms[s] - max_log_rms;
        global_features[2 * NUM_SUBBANDS + s] = density[s];
    }

    let max_code = meta.codes.iter().copied().max().unwrap_or(0) as f32;
    for g in 0..4 {
        let dim = (edges[g + 1] - edges[g]).max(1) as f32;
        global_features[48 + g] = f32::from(meta.codes[g]) * (1.0 / 208.0);
        global_features[52 + g] = (f32::from(meta.codes[g]) - max_code) * (1.0 / 64.0);
        global_features[56 + g] = (meta.allocation[g] as f32 / dim).min(2.0);
    }
    global_features[60] = if meta.samples == 960 { 1.0 } else { 0.0 };
    global_features[61] = if meta.is_harmonic { 1.0 } else { 0.0 };
    global_features[62] = meta.f0 as f32 * (1.0 / 400.0);
    global_features[63] = (meta.payload_bytes as f32 * 8.0) / (meta.coded_bins as f32 * 2.0);
}

/// Refine decoded MDCT coefficients in-place using the quantized neural network.
pub(crate) fn refine_spectrum(coefficients: &mut [f32], meta: &FrameMetadata) {
    if meta.coded_bins == 0
        || meta.coded_bins > FRAME_SAMPLES
        || !meta.coded_bins.is_multiple_of(NUM_SUBBANDS)
    {
        return;
    }
    // Fast skip when weights are uninitialized (exact identity).
    if S1[0] == 0.0 && S3[0] == 0.0 && HEAD_S2[0][0] == 0.0 {
        return;
    }

    let mut occupied = 0usize;
    for &val in &coefficients[..meta.coded_bins] {
        if val.abs() > 1e-6 {
            occupied += 1;
            if occupied >= 2 {
                break;
            }
        }
    }
    if occupied < 2 {
        return;
    }

    let edges = crate::pulse::edges(meta.samples, meta.coded_bins);
    let mut group_sigma = [0.0f32; 4];
    let mut global_features = [0.0f32; GLOBAL_DIM];
    compute_frame_features(
        coefficients,
        meta,
        &edges,
        &mut group_sigma,
        &mut global_features,
    );

    // Scale 1: Frame & Subband Context MLP.
    let mut h1 = [0.0f32; GLOBAL_H1];
    for i in 0..GLOBAL_H1 {
        let pre = dot_i8_64(&W1[i], &global_features) * S1[i] + B1[i];
        h1[i] = pre.max(0.0);
    }

    let mut h2 = [0.0f32; GLOBAL_H2];
    for i in 0..GLOBAL_H2 {
        let pre = dot_i8_64(&W2[i], &h1) * S2[i] + B2[i];
        h2[i] = pre.max(0.0) + h1[i];
    }

    let mut subband_out = [0.0f32; SUBBAND_OUT];
    for i in 0..SUBBAND_OUT {
        subband_out[i] = dot_i8_64(&W3[i], &h2) * S3[i] + B3[i];
    }

    let mut subband_gains = [1.0f32; NUM_SUBBANDS];
    for s in 0..NUM_SUBBANDS {
        let g_logit = subband_out[s * SUBBAND_STRIDE];
        subband_gains[s] = (1.0 + 0.95 * fast_sig(g_logit)).clamp(0.05, 1.6);
    }

    // Prepare normalized spectrum across coded bins.
    let mut norm_x = [0.0f32; FRAME_SAMPLES];
    for g in 0..4 {
        if meta.codes[g] == 0 || group_sigma[g] <= 1e-6 {
            continue;
        }
        let inv_sigma = 1.0 / group_sigma[g];
        for k in edges[g]..edges[g + 1] {
            norm_x[k] = (coefficients[k] * inv_sigma).clamp(-8.0, 8.0);
        }
    }

    let sub_bins = meta.coded_bins / NUM_SUBBANDS;
    let inv_sub_bins = 1.0 / sub_bins as f32;
    let mut refined = [0.0f32; FRAME_SAMPLES];

    let k_4400 = (meta.samples * 11) / 60;
    let k_8000 = meta.samples / 3;
    let max_sigma = group_sigma.iter().copied().fold(1e-6f32, f32::max);

    // Scale 2: Polarity-equivariant local Conv1D head per bin.
    for g in 0..4 {
        if meta.codes[g] == 0 || group_sigma[g] <= 1e-6 {
            continue;
        }
        let sigma = group_sigma[g];
        let head = (usize::from(meta.is_harmonic) << 2) + g;
        let hw1 = &HEAD_W1[head];
        let hs1 = &HEAD_S1[head];
        let hb1 = &HEAD_B1[head];
        let hw2 = &HEAD_W2[head];
        let hs2 = &HEAD_S2[head];
        let hb2 = &HEAD_B2[head];

        for (k, refined_k) in refined
            .iter_mut()
            .enumerate()
            .take(edges[g + 1])
            .skip(edges[g])
        {
            let mut taps = [0.0f32; 7];
            let mut raw_taps = [0.0f32; 7];
            let mut energy_sum = 0.0f32;
            for d in 0..7 {
                let idx = k as isize + d as isize - 3;
                if idx >= 0 && (idx as usize) < meta.coded_bins {
                    let m = idx as usize;
                    let nx = norm_x[m];
                    taps[d] = nx;
                    raw_taps[d] = coefficients[m];
                    energy_sum += nx * nx;
                }
            }
            if energy_sum <= 1e-12 {
                continue;
            }

            // Dominant tap sign for exact polarity equivariance: f(-X) = -f(X).
            const NL_SIGN: [f32; 7] = [-1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
            let mut best_abs = -1.0f32;
            let mut s_ref = 1.0f32;
            let mut s_nl = 1.0f32;
            for &d in &[3usize, 2, 4, 1, 5, 0, 6] {
                let a = taps[d].abs();
                if a > best_abs {
                    best_abs = a;
                    s_ref = if taps[d] >= 0.0 { 1.0 } else { -1.0 };
                    s_nl = s_ref * NL_SIGN[d];
                }
            }

            let mut local_feat = [0.0f32; LOCAL_DIM];
            for d in 0..7 {
                local_feat[d] = s_ref * taps[d];
                local_feat[7 + d] = taps[d].abs();
            }
            local_feat[14] = if taps[3].abs() > 1e-5 { 1.0 } else { 0.0 };
            local_feat[15] = (k % sub_bins) as f32 * inv_sub_bins;

            let s = k / sub_bins;
            let cond_base = s * SUBBAND_STRIDE + 1;
            let mut h_bin = [0.0f32; BIN_HIDDEN];
            for i in 0..BIN_HIDDEN {
                let pre =
                    dot_i8_16(&hw1[i], &local_feat) * hs1[i] + hb1[i] + subband_out[cond_base + i];
                h_bin[i] = pre.max(0.0);
            }

            let mut out = [0.0f32; BIN_OUT];
            for i in 0..BIN_OUT {
                out[i] = dot_i8_16(&hw2[i], &h_bin) * hs2[i] + hb2[i];
            }

            let delta_g = 0.8 * fast_sig(out[0]);
            let w_m3 = 0.35 * fast_sig(out[1]);
            let w_m2 = 0.35 * fast_sig(out[2]);
            let w_m1 = 0.35 * fast_sig(out[3]);
            let w_p1 = 0.35 * fast_sig(out[4]);
            let w_p2 = 0.35 * fast_sig(out[5]);
            let w_p3 = 0.35 * fast_sig(out[6]);
            let is_32k = meta.payload_bytes * 15 >= meta.samples;
            let is_10ms_12k = meta.samples == 480 && meta.payload_bytes <= 15;
            let w_nl = if k >= k_4400 && k < k_8000 {
                let nl_gain = if meta.is_harmonic {
                    0.35
                } else if is_32k {
                    0.25
                } else if is_10ms_12k {
                    0.85
                } else {
                    0.50
                };
                nl_gain * fast_sig(out[7])
            } else {
                0.0
            };

            let e_loc = (energy_sum * (1.0 / 7.0)).sqrt().min(4.0);
            let filtered = (1.0 + delta_g) * raw_taps[3]
                + w_m3 * raw_taps[0]
                + w_m2 * raw_taps[1]
                + w_m1 * raw_taps[2]
                + w_p1 * raw_taps[4]
                + w_p2 * raw_taps[5]
                + w_p3 * raw_taps[6]
                + w_nl * s_nl * sigma * e_loc;

            let sb_gain = if k < k_4400 {
                1.0 + 0.15 * (subband_gains[s] - 1.0)
            } else {
                subband_gains[s]
            };
            *refined_k = sb_gain * filtered;
        }
        if g == 3 && edges[g] < k_8000 {
            let start = edges[g];
            let end_8k = edges[g + 1].min(k_8000);
            let dim = (end_8k - start).max(1) as f32;
            let mut orig_energy = 0.0f32;
            let mut ref_energy = 0.0f32;
            for k in start..end_8k {
                orig_energy += coefficients[k] * coefficients[k];
                ref_energy += refined[k] * refined[k];
            }
            let orig_sigma = (orig_energy / dim + 1e-12).sqrt();
            let ref_sigma = (ref_energy / dim + 1e-12).sqrt();
            let rel = (orig_sigma / max_sigma).clamp(0.0, 1.0);
            let is_32k = meta.payload_bytes * 15 >= meta.samples;
            let is_10ms_12k = meta.samples == 480 && meta.payload_bytes <= 15;
            let base_ratio = if meta.is_harmonic {
                1.00
            } else if is_32k {
                0.99
            } else if is_10ms_12k {
                1.28
            } else {
                1.02
            };
            let target_sigma = orig_sigma * (base_ratio - 0.26 * rel.sqrt());
            if ref_sigma < target_sigma && orig_sigma > 1e-6 {
                let scale = target_sigma / ref_sigma;
                for val in &mut refined[start..end_8k] {
                    *val *= scale;
                }
            }
        }
    }

    let is_32k = meta.payload_bytes * 15 >= meta.samples;
    let is_24k = !is_32k && meta.payload_bytes * 25 >= meta.samples;
    for k in 0..meta.coded_bins {
        let w = if is_32k {
            0.35
        } else if meta.is_harmonic {
            if k >= k_4400 { 0.35 } else { 0.20 }
        } else if is_24k {
            if k >= k_4400 { 0.55 } else { 0.40 }
        } else if k >= k_4400 {
            1.00
        } else {
            0.55
        };
        coefficients[k] = (1.0 - w) * coefficients[k] + w * refined[k];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_single_impulse_are_exact_identity() {
        let meta = FrameMetadata {
            samples: 480,
            coded_bins: 240,
            payload_bytes: 30,
            is_harmonic: false,
            f0: 0,
            codes: [120, 110, 100, 90],
            allocation: [12, 10, 8, 6],
        };
        let mut coeffs = [0.0f32; FRAME_SAMPLES];
        refine_spectrum(&mut coeffs, &meta);
        assert!(coeffs.iter().all(|&x| x == 0.0));

        coeffs[17] = 0.25;
        let snapshot = coeffs;
        refine_spectrum(&mut coeffs, &meta);
        assert_eq!(coeffs, snapshot);
    }

    #[test]
    fn polarity_equivariance_holds_across_all_profiles() {
        let profiles = [
            (480, 160, 15, false),
            (480, 160, 15, true),
            (480, 240, 30, false),
            (480, 480, 40, false),
            (960, 320, 30, false),
            (960, 320, 30, true),
            (960, 480, 60, false),
            (960, 960, 80, false),
        ];
        for (samples, coded_bins, payload_bytes, is_harmonic) in profiles {
            let meta = FrameMetadata {
                samples,
                coded_bins,
                payload_bytes,
                is_harmonic,
                f0: if is_harmonic { 180 } else { 0 },
                codes: [152, 144, 132, 118],
                allocation: [16, 14, 12, 10],
            };
            let mut pos = [0.0f32; FRAME_SAMPLES];
            let mut neg = [0.0f32; FRAME_SAMPLES];
            for k in 0..coded_bins {
                let v = ((k as f32 * 0.37).sin() * 0.12) + ((k as f32 * 1.13).cos() * 0.05);
                if v.abs() > 0.02 {
                    pos[k] = v;
                    neg[k] = -v;
                }
            }
            refine_spectrum(&mut pos, &meta);
            refine_spectrum(&mut neg, &meta);
            for k in 0..FRAME_SAMPLES {
                assert!(pos[k].is_finite() && neg[k].is_finite());
                assert!(
                    (pos[k] + neg[k]).abs() <= 1e-6,
                    "polarity mismatch at bin {k} for ({samples}, {coded_bins}, {is_harmonic}): {} vs {}",
                    pos[k],
                    neg[k]
                );
            }
        }
    }
}
